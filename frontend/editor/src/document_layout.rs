// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use operation::{op::Op, operation::Operation};
use rope::{
    cursor::Cursor, cursor::SeekMode, metrics::Measure, metrics::MetricId, metrics::Metrics,
    rope::Rope,
};
use text::{text::Text, text_view::TextView};
use tokenize::{rewrite, Safepoint};

use crate::{
    markup::{BlockStyle, TextDecorationInterval},
    shaped_line::{block_gap, display_byte_of, DisplayText, Excision},
};

const DOCUMENT_RANK: usize = 3;
const VERTICAL_PX: MetricId = MetricId(0);
const LINE_STARTS: MetricId = MetricId(1);
pub(crate) const BYTES: MetricId = MetricId(2);

#[derive(Clone)]
pub(crate) struct LayoutElement {
    /// A repair may resume here: the rows after an aligned,
    /// undamaged row come out the same however the line was entered.
    pub(crate) safepoint: bool,
    /// The first row of a hard line: where an edit that changes line
    /// structure stops mattering.
    pub(crate) line_start: bool,
    /// The row's hard line holds right-to-left text. Such a line is
    /// always laid whole: shaped as a suffix its rows measure a
    /// shade differently, even under the line's base direction.
    pub(crate) bidi: bool,
    pub(crate) byte_size: u32,
    pub(crate) height: f32,

    pub(crate) width: f32,

    pub(crate) spacer_above: f32,
}

#[derive(Clone)]
struct Damage {
    intervals: intervals::Intervals<u32, ()>,
    next_key: u32,
}

impl Damage {
    fn new() -> Self {
        Self {
            intervals: intervals::Intervals::new(),
            next_key: 0,
        }
    }

    fn add(&mut self, range: Range<u32>) {
        let range = range.start..range.end.max(range.start.saturating_add(1));
        let key = self.next_key;
        self.next_key = self.next_key.wrapping_add(1);
        self.intervals.insert([intervals::Interval {
            range,
            greedy_left: true,
            greedy_right: true,
            key,
            value: (),
        }]);
    }

    fn first(&self) -> Option<u32> {
        use intervals::{IntervalQuery, Order};
        self.intervals
            .query(0..u32::MAX, Order::Ascending)
            .find(|interval| interval.range.end > interval.range.start)
            .map(|interval| interval.range.start)
    }

    fn first_at_or_after(&self, byte: u32) -> Option<u32> {
        use intervals::{IntervalQuery, Order};
        self.intervals
            .query(byte..u32::MAX, Order::Ascending)
            .filter(|interval| interval.range.end > byte)
            .map(|interval| interval.range.start.max(byte))
            .next()
    }

    fn any_intersecting(&self, range: Range<u32>) -> bool {
        use intervals::{IntervalQuery, Order};
        self.intervals
            .query(range.clone(), Order::Ascending)
            .any(|interval| interval.range.start < range.end && range.start < interval.range.end)
    }

    fn subtract(&mut self, range: Range<u32>) {
        use intervals::{IntervalQuery, Order};
        let overlapping: Vec<(u32, Range<u32>)> = self
            .intervals
            .query(range.clone(), Order::Ascending)
            .filter(|interval| interval.range.start < range.end && range.start < interval.range.end)
            .map(|interval| (*interval.key, interval.range.clone()))
            .collect();
        if overlapping.is_empty() {
            return;
        }
        let keys: Vec<u32> = overlapping.iter().map(|(key, _)| *key).collect();
        self.intervals.remove(keys.iter());
        for (_, damaged) in overlapping {
            if damaged.start < range.start {
                self.add(damaged.start..range.start);
            }
            if range.end < damaged.end {
                self.add(range.end..damaged.end);
            }
        }
    }

    fn edit(&mut self, operation: &Operation) {
        self.intervals
            .edit(crate::markup::interval_steps(operation));
    }

    fn is_empty(&self) -> bool {
        self.first().is_none()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LayoutMeasure;

#[derive(Clone)]
pub struct DocumentLayout {
    rope: Rope<LayoutElement, LayoutMeasure>,

    widths: crate::width_tree::WidthTree,
    damage: Damage,
    layout_width: f32,

    window: Option<Range<u32>>,

    healed: Option<Range<u32>>,

    spacer_writes: Option<Range<u32>>,

    swap_fold: Option<Range<u32>>,

    shaped_theme: std::sync::Arc<str>,
}

pub(crate) const SYNC_LAYOUT_HEIGHT: f32 = 4_000.0;

impl DocumentLayout {
    pub fn build(
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        measure: crate::markup::InlayMeasure<'_>,
        theme: &crate::theme::Theme,
        window: Option<Range<u32>>,
    ) -> Self {
        let mut layout = Self::new();
        layout.window = window;
        layout.repair_layout_bounded(text, markup, measure, theme, 0, SYNC_LAYOUT_HEIGHT);

        let byte_count = text.view().byte_count().min(u32::MAX as usize) as u32;
        let covered = layout.byte_size();
        if covered < byte_count {
            let mut elements = Vec::with_capacity(layout.len() + 1);
            if !layout.rope.is_empty() {
                let mut cursor = layout.rope.cursor();
                assert!(cursor.seek(BYTES, 0, SeekMode::After));
                loop {
                    elements.push(cursor.element().clone());
                    if !cursor.advance() {
                        break;
                    }
                }
            }
            elements.push(LayoutElement {
                safepoint: true,
                line_start: true,
                bidi: false,
                byte_size: byte_count - covered,
                height: 0.0,
                width: 0.0,
                spacer_above: 0.0,
            });
            layout.rope = Rope::from_iter(elements);
            layout.resync_widths(covered..byte_count);
            layout.damage.add(covered..byte_count);
        }
        layout
    }

    #[cfg(test)]
    /// Test-support: the float-width builders, paying the
    /// per-call measure seed — production threads `InlayMeasure`.
    pub(crate) fn build_slow(
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        width: f32,
        theme: &crate::theme::Theme,
        window: Option<Range<u32>>,
    ) -> Self {
        {
            let mut seeded = imba::store::Store::new();
            crate::env::Themes::set(&mut seeded, theme.clone());
            let ui = crate::test_document::test_ui();
            let measure = crate::markup::InlayMeasure {
                width,
                store: &seeded,
                ui: &ui,
            };
            Self::build(text, markup, measure, theme, window)
        }
    }

    #[cfg(test)]
    pub(crate) fn build_complete_slow(
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        width: f32,
        theme: &crate::theme::Theme,
        window: Option<Range<u32>>,
    ) -> Self {
        {
            let mut seeded = imba::store::Store::new();
            crate::env::Themes::set(&mut seeded, theme.clone());
            let ui = crate::test_document::test_ui();
            let measure = crate::markup::InlayMeasure {
                width,
                store: &seeded,
                ui: &ui,
            };
            Self::build_complete(text, markup, measure, theme, window)
        }
    }

    #[cfg(test)]
    pub(crate) fn repair_layout_bounded_slow(
        &mut self,
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        width: f32,
        theme: &crate::theme::Theme,
        byte_start: u32,
        height_budget: f32,
    ) {
        {
            let mut seeded = imba::store::Store::new();
            crate::env::Themes::set(&mut seeded, theme.clone());
            let ui = crate::test_document::test_ui();
            let measure = crate::markup::InlayMeasure {
                width,
                store: &seeded,
                ui: &ui,
            };
            self.repair_layout_bounded(text, markup, measure, theme, byte_start, height_budget)
        }
    }

    #[cfg(test)]
    pub(crate) fn repair_layout_slow(
        &mut self,
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        width: f32,
        theme: &crate::theme::Theme,
        byte_start: u32,
    ) {
        {
            let mut seeded = imba::store::Store::new();
            crate::env::Themes::set(&mut seeded, theme.clone());
            let ui = crate::test_document::test_ui();
            let measure = crate::markup::InlayMeasure {
                width,
                store: &seeded,
                ui: &ui,
            };
            self.repair_layout(text, markup, measure, theme, byte_start)
        }
    }

    pub fn build_complete(
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        measure: crate::markup::InlayMeasure<'_>,
        theme: &crate::theme::Theme,
        window: Option<Range<u32>>,
    ) -> Self {
        let mut layout = Self::build(text, markup, measure, theme, window);
        if let Some(pending) = layout.repair_pending() {
            layout.repair_layout(text, markup, measure, theme, pending);
        }
        layout
    }

    fn new() -> Self {
        Self {
            rope: Rope::new(),
            widths: crate::width_tree::WidthTree::new(),
            damage: Damage::new(),
            layout_width: 0.0,
            window: None,
            healed: None,
            spacer_writes: None,
            swap_fold: None,
            shaped_theme: "".into(),
        }
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn shaped_theme(&self) -> &str {
        &self.shaped_theme
    }

    pub fn height(&self) -> f32 {
        LayoutMeasure::metric_at(&self.metrics(), VERTICAL_PX) as f32
    }

    pub(crate) fn bounded(&self) -> bool {
        self.window.is_some()
    }

    /// The window to lay out, after the text has been edited. The
    /// trailing empty line after a final newline is a whole-text
    /// affair — a window over the whole text has one, a proper
    /// window has not — so when the window's extent flips between
    /// the two, the last hard line is damaged and its start comes
    /// back for the repair to begin at (the 2026-10-26 property
    /// finding: a deletion before an editing window pulled it to
    /// the text's start, and the trailing line never appeared).
    pub(crate) fn set_window(&mut self, window: Option<Range<u32>>) -> Option<u32> {
        let byte_size = self.byte_size();
        let whole = |window: &Option<Range<u32>>| {
            window
                .as_ref()
                .is_none_or(|window| window.start == 0 && window.end >= byte_size)
        };
        let flipped = whole(&self.window) != whole(&window);
        self.window = window;
        if !flipped || byte_size == 0 {
            return None;
        }
        let from = self.line_start_at_or_before(byte_size - 1);
        self.damage.add(from..byte_size);
        Some(from)
    }

    pub fn layout_width(&self) -> f32 {
        self.layout_width
    }

    pub fn edit(&mut self, operation: &Operation) -> u32 {
        let mut cursor = self.rope.cursor();
        let mut byte_count = self.byte_size();
        let mut offset = 0u32;
        let mut repair_start = None;

        let mut damaged = Vec::new();
        // The element a deletion ends in lost its head; it begins
        // again where the deletion landed, pushed right by any
        // insert at the same spot. It needs damage of its own, or a
        // repair resumes on it as if it were untouched (the
        // 2026-10-26 property finding: a cross-line delete kept the
        // second line's stale row, wrong width and all).
        // A deletion's landing: (where it lands, whether a newline
        // went with it, whether it began at a line's first byte). The
        // landing moves right under an insert at the same spot.
        let mut landing: Option<(u32, bool, bool)> = None;
        let mut landings: Vec<(u32, bool, bool)> = Vec::new();
        // Where a newline came or went: the rows after it up to the
        // next hard line start changed line, and a line's facts —
        // its block style, its alignment, its last row's gap — sit on
        // every row of it. Those rows re-lay; nothing past the next
        // line start does.
        let mut reshaped_lines: Vec<u32> = Vec::new();
        // Positions an inserted newline now precedes: the element
        // standing there starts a line, whatever it used to be.
        let mut starts_now: Vec<u32> = Vec::new();
        let reshapes = |text: &str| {
            text.contains('\n') || text.chars().any(|ch| strong_direction(ch) == Some(true))
        };

        for op in operation.iter() {
            match op {
                Op::Retain(len) => {
                    landings.extend(landing.take());
                    offset = offset.saturating_add(len);
                }
                Op::Insert(text) => {
                    let len = text.len().min(u32::MAX as usize) as u32;
                    let start = Self::insert_bytes(&mut cursor, offset, len, byte_count);
                    damaged.push(start);
                    repair_start =
                        Some(repair_start.map_or(start, |existing: u32| existing.min(start)));
                    byte_count = byte_count.saturating_add(len);
                    let from = offset;
                    offset = offset.saturating_add(len);
                    if let Some((_, newline, line_head)) = landing {
                        landing = Some((offset, newline, line_head));
                    }
                    if reshapes(&text) {
                        // The line the insert starts in and the one
                        // it ends in: with a newline inside, the two
                        // halves of the old line are lines of their
                        // own now, each styled over its own bytes.
                        reshaped_lines.push(from);
                        reshaped_lines.push(offset);
                    }
                    if text.ends_with('\n') {
                        starts_now.push(offset);
                    }
                }
                Op::Delete(text) => {
                    let len = text.len().min(u32::MAX as usize) as u32;
                    let (start, line_head) =
                        Self::delete_bytes(&mut cursor, offset, len, &mut byte_count);
                    damaged.push(start);
                    let newline = reshapes(&text) || landing.is_some_and(|(_, newline, _)| newline);
                    let line_head = line_head || landing.is_some_and(|(_, _, head)| head);
                    landing = Some((offset, newline, line_head));
                    repair_start =
                        Some(repair_start.map_or(start, |existing: u32| existing.min(start)));
                }
            }
        }
        landings.extend(landing);
        let flag_line_start =
            |cursor: &mut Cursor<LayoutElement, LayoutMeasure>, at: u32, starts: bool| {
                if at < byte_count && cursor.seek(BYTES, at, SeekMode::After) {
                    while cursor.element().byte_size == 0 && cursor.advance() {}
                    if LayoutMeasure::metric_at(&cursor.position(), BYTES) == at
                        && cursor.element().line_start != starts
                    {
                        let mut element = cursor.element().clone();
                        element.line_start = starts;
                        cursor.delete(1);
                        cursor.insert(Rope::from_iter([element]));
                    }
                }
            };
        for (at, newline, line_head) in landings {
            damaged.push(at);
            if newline {
                reshaped_lines.push(at);
            }
            if newline || line_head {
                // The element standing at the landing starts a line
                // exactly when what precedes it does: an inserted
                // newline, or the newline (or text start) that
                // preceded the line whose head the deletion took. A
                // deleted newline alone makes it a continuation. The
                // flag must be true to the text — a bidi line's rows
                // are laid from the line start the flags give, and a
                // first row flagged as a continuation sends the
                // repair to the previous line, where it converges
                // before it ever reaches the damage (the 2026-10-27
                // property finding: a never-settling worker).
                flag_line_start(&mut cursor, at, starts_now.contains(&at) || line_head);
            }
        }
        for at in starts_now {
            flag_line_start(&mut cursor, at, true);
        }

        self.rope = cursor.rope();
        self.widths.edit(operation);

        if self.byte_size() == 0 {
            self.damage = Damage::new();
            self.widths.clear();
            return repair_start.unwrap_or(offset);
        }
        self.damage.edit(operation);
        for start in damaged {
            let start = start.min(self.byte_size().saturating_sub(1));
            self.damage.add(start..start.saturating_add(1));
        }
        // One cursor for the restart and every reshaped line: the
        // seeks are near each other, and a cursor climbs only as far
        // as the next target needs, where a fresh one descends from
        // the root every time.
        let byte_size = self.byte_size();
        let mut cursor = self.rope.cursor();
        let mut restart =
            Self::restart_before_on(&mut cursor, byte_size, repair_start.unwrap_or(offset));
        for at in reshaped_lines {
            // The hard line the newline edit lands in is a different
            // line now — joined with its neighbour or cut from its
            // tail — and its block style, resolved over the whole
            // line, may have changed for every row of it, the ones
            // before the edit included.
            let from = Self::line_start_at_or_before_on(&mut cursor, byte_size, at);
            let to = Self::next_line_start_after_on(&mut cursor, byte_size, at);
            if to > from {
                self.damage.add(from..to);
            }
            restart = restart.min(from);
        }
        restart
    }

    /// The start of the hard line holding `byte`: the nearest
    /// line-start element at or before it, or 0.
    fn line_start_at_or_before(&self, byte: u32) -> u32 {
        if self.rope.is_empty() {
            return 0;
        }
        Self::line_start_at_or_before_on(&mut self.rope.cursor(), self.byte_size(), byte)
    }

    /// `line_start_at_or_before` on a caller's cursor, which may stand
    /// anywhere in a non-empty layout of `byte_size` bytes.
    fn line_start_at_or_before_on(
        cursor: &mut Cursor<LayoutElement, LayoutMeasure>,
        byte_size: u32,
        byte: u32,
    ) -> u32 {
        let byte = byte.min(byte_size.saturating_sub(1));
        if !cursor.seek(BYTES, byte, SeekMode::After) {
            return 0;
        }
        if cursor.element().line_start {
            return LayoutMeasure::metric_at(&cursor.position(), BYTES);
        }
        let starts_before = LayoutMeasure::metric_at(&cursor.position(), LINE_STARTS);
        if starts_before == 0 || !cursor.seek(LINE_STARTS, starts_before, SeekMode::Before) {
            return 0;
        }
        LayoutMeasure::metric_at(&cursor.position(), BYTES)
    }

    /// The start of the first hard line beginning after `byte`, or
    /// the layout's end.
    fn next_line_start_after(&self, byte: u32) -> u32 {
        if self.rope.is_empty() {
            return self.byte_size();
        }
        Self::next_line_start_after_on(&mut self.rope.cursor(), self.byte_size(), byte)
    }

    /// `next_line_start_after` on a caller's cursor, which may stand
    /// anywhere in a non-empty layout of `byte_size` bytes.
    fn next_line_start_after_on(
        cursor: &mut Cursor<LayoutElement, LayoutMeasure>,
        byte_size: u32,
        byte: u32,
    ) -> u32 {
        if byte >= byte_size || !cursor.seek(BYTES, byte, SeekMode::After) {
            return byte_size;
        }
        loop {
            let at = LayoutMeasure::metric_at(&cursor.position(), BYTES);
            if at > byte && cursor.element().line_start {
                return at;
            }
            if !cursor.advance() {
                return byte_size;
            }
        }
    }

    /// Where a repair of a change at `byte` starts: the row BEFORE
    /// the one holding it. A greedy break ends a row where the next
    /// row's first word stopped fitting, so a change to that word —
    /// and nothing further down — can pull text back up a row. One
    /// row of context is exact; the whole hard line would be paid
    /// for nothing (the 2026-10-26 property finding: a deletion in
    /// row two left row one wrapped early).
    fn restart_before(&self, byte: u32) -> u32 {
        if self.rope.is_empty() {
            return 0;
        }
        Self::restart_before_on(&mut self.rope.cursor(), self.byte_size(), byte)
    }

    /// `restart_before` on a caller's cursor, which may stand anywhere
    /// in a non-empty layout of `byte_size` bytes.
    fn restart_before_on(
        cursor: &mut Cursor<LayoutElement, LayoutMeasure>,
        byte_size: u32,
        byte: u32,
    ) -> u32 {
        let byte = byte.min(byte_size.saturating_sub(1));
        if !cursor.seek(BYTES, byte, SeekMode::After) {
            return 0;
        }
        if cursor.element().bidi {
            return Self::line_start_at_or_before_on(cursor, byte_size, byte);
        }
        let row = LayoutMeasure::metric_at(&cursor.position(), BYTES);
        if row == 0 || !cursor.seek(BYTES, row, SeekMode::Before) {
            return row;
        }
        LayoutMeasure::metric_at(&cursor.position(), BYTES)
    }

    pub(crate) fn mark_modified(&mut self, byte_start: u32) -> u32 {
        if self.byte_size() == 0 {
            return 0;
        }
        let byte_start = byte_start.min(self.byte_size().saturating_sub(1));
        self.damage.add(byte_start..byte_start.saturating_add(1));
        self.restart_before(byte_start)
    }

    pub(crate) fn mark_modified_in(&mut self, range: Range<u32>) -> u32 {
        if self.byte_size() == 0 {
            return 0;
        }

        let byte_size = self.byte_size();
        let range = range.start.min(byte_size)..range.end.min(byte_size);
        if range.start >= range.end {
            return self.mark_modified(range.start);
        }
        self.damage.add(range.clone());
        self.restart_before(range.start)
    }

    pub fn repair_layout(
        &mut self,
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        measure: crate::markup::InlayMeasure<'_>,
        theme: &crate::theme::Theme,
        byte_start: u32,
    ) {
        let started = crate::startup_profile::start();
        self.repair_region(text, markup, measure, theme, byte_start, f32::MAX);
        while let Some(pending) = self.repair_pending() {
            self.repair_region(text, markup, measure, theme, pending, f32::MAX);
            debug_assert!(
                self.repair_pending().is_none_or(|next| next > pending),
                "layout repair must make progress: pending {pending} -> {:?} (byte_size {})",
                self.repair_pending(),
                self.byte_size(),
            );
        }
        self.assert_repaired();
        crate::startup_profile::log("DocumentLayout::repair_layout", started);
    }

    pub fn repair_layout_bounded(
        &mut self,
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        measure: crate::markup::InlayMeasure<'_>,
        theme: &crate::theme::Theme,
        byte_start: u32,
        height_budget: f32,
    ) {
        self.repair_region(text, markup, measure, theme, byte_start, height_budget);
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn repair_pending(&self) -> Option<u32> {
        self.damage.first()
    }

    pub(crate) fn repair_pending_at_or_after(&self, byte: u32) -> Option<u32> {
        self.damage.first_at_or_after(byte)
    }

    pub(crate) fn damage_intersects(&self, range: Range<u32>) -> bool {
        self.damage.any_intersecting(range)
    }

    fn widen(acc: &mut Option<Range<u32>>, range: Range<u32>) {
        if range.start >= range.end {
            return;
        }
        *acc = Some(match acc.take() {
            Some(current) => current.start.min(range.start)..current.end.max(range.end),
            None => range,
        });
    }

    pub(crate) fn note_healed(&mut self, range: Range<u32>) {
        Self::widen(&mut self.healed, range);
    }

    pub(crate) fn take_healed(&mut self) -> Option<Range<u32>> {
        self.healed.take()
    }

    pub(crate) fn take_stale_for_swap(&mut self) -> Option<Range<u32>> {
        let mut stale = self.healed.take();
        if let Some(writes) = self.spacer_writes.take() {
            Self::widen(&mut stale, writes);
        }
        stale
    }

    pub(crate) fn note_swap_fold(&mut self, range: Range<u32>) {
        Self::widen(&mut self.swap_fold, range);
    }

    pub(crate) fn take_swap_fold(&mut self) -> Option<Range<u32>> {
        self.swap_fold.take()
    }

    fn repair_region(
        &mut self,
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        measure: crate::markup::InlayMeasure<'_>,
        theme: &crate::theme::Theme,
        byte_start: u32,
        height_budget: f32,
    ) {
        let width = measure.width;
        self.layout_width = width;
        self.shaped_theme = theme.name_shared();

        let text_count = text.view().byte_count().min(u32::MAX as usize) as u32;
        let window = match &self.window {
            Some(range) => range.start.min(text_count)..range.end.min(text_count),
            None => 0..text_count,
        };
        let mut cursor = self.rope.cursor();
        let repair_start = match self.rope.is_empty() {
            true => 0,
            false => {
                let mut start =
                    Self::seek_repair_start(&mut cursor, byte_start.min(self.byte_size()));
                // A bidi line is laid whole. The text says whether the
                // line is one — an old row's flag may outlive the
                // right-to-left text that earned it.
                if !cursor.element().line_start && line_has_rtl(text, start) {
                    start = self.line_start_at_or_before(start);
                    cursor = self.rope.cursor();
                    start = Self::seek_repair_start(&mut cursor, start);
                }
                start
            }
        };

        #[cfg(debug_assertions)]
        {
            let mut probe = text.view();
            debug_assert!(
                probe.is_char_boundary(repair_start as usize),
                "repair resume point {repair_start} is not a char boundary: \
                 a stale layout element boundary was corrupted"
            );
            debug_assert!(
                probe.is_char_boundary(window.start as usize)
                    && probe.is_char_boundary(window.end as usize),
                "fragment window {window:?} is not on char boundaries: \
                 the fragment interval was corrupted"
            );
        }
        let tokens = BudgetedItems {
            inner: WindowedItems::new(
                text,
                markup,
                repair_start,
                window,
                text_count,
                measure,
                theme,
            )
            .peekable(),
            remaining: height_budget,

            remaining_items: 512,
            in_bidi_line: false,
        };

        let report = rewrite(
            &mut cursor,
            tokens,
            &LayoutSafepoints {
                damage: &self.damage,
            },
            BYTES,
            |location| LayoutMeasure::metric_at(&location, BYTES) > repair_start,
        );
        self.rope = cursor.rope();

        let stop = LayoutMeasure::metric_at(&report.location, BYTES);
        if !report.stopped && stop > 0 && stop < text_count && stop < self.byte_size() {
            // The budget ended the pass before a token starting at
            // `stop`; the old element left standing there — often a
            // cut remnant, which `cut` flags as no line start — begins
            // where that token begins. The text knows whether a hard
            // line starts there: the byte before is a newline.
            let mut before = Vec::new();
            text.view()
                .byte_range_into(stop as usize - 1, stop as usize, &mut before);
            let starts_line = before.first() == Some(&b'\n');
            let mut cursor = self.rope.cursor();
            if cursor.seek(BYTES, stop, SeekMode::After) {
                while cursor.element().byte_size == 0 && cursor.advance() {}
                if LayoutMeasure::metric_at(&cursor.position(), BYTES) == stop
                    && cursor.element().line_start != starts_line
                {
                    let mut element = cursor.element().clone();
                    element.line_start = starts_line;
                    cursor.delete(1);
                    cursor.insert(Rope::from_iter([element]));
                    self.rope = cursor.rope();
                }
            }
        }
        if !report.stopped && stop >= text_count && stop < self.byte_size() {
            // The text ran out under the producer and the old layout
            // still goes on: whatever follows describes bytes that no
            // longer exist. Drop it rather than re-arm damage on it
            // forever — a pass can never reach it.
            let old_size = self.byte_size();
            let mut cursor = self.rope.cursor();
            if cursor.seek(BYTES, stop, SeekMode::After) {
                let (head, _) = cursor.split();
                self.rope = head;
                self.widths
                    .replace_bytes(u64::from(stop)..u64::from(old_size), Vec::new());
            }
        }
        if crate::env_flags::trace_resize() {
            eprintln!(
                "[repair] start={repair_start} stop={stop} span={} budget={height_budget}",
                stop.saturating_sub(repair_start)
            );
        }
        self.note_healed(repair_start..stop);
        self.resync_widths(repair_start..stop);
        if stop > repair_start {
            self.damage.subtract(repair_start..stop);
            if !report.stopped && stop < self.byte_size() {
                self.damage.add(stop..stop.saturating_add(1));
            }
        } else {
            self.damage
                .subtract(repair_start..repair_start.saturating_add(1));
        }
    }

    fn resync_widths(&mut self, range: Range<u32>) {
        if range.start >= range.end {
            return;
        }
        let mut spans = Vec::new();
        for (start, element) in self.spans_from(range.start) {
            if start >= range.end {
                break;
            }
            if element.byte_size == 0 {
                continue;
            }
            spans.push(crate::width_tree::WidthSpan {
                bytes: element.byte_size,
                width: element.width,
            });
        }
        self.widths
            .replace_bytes(u64::from(range.start)..u64::from(range.end), spans);
        debug_assert_eq!(
            self.widths.byte_size(),
            u64::from(self.byte_size()),
            "width mirror byte coverage diverged from the layout"
        );
    }

    pub fn max_width(&self) -> f32 {
        self.widths.max()
    }

    pub(crate) fn max_width_in(&self, range: Range<u32>) -> f32 {
        self.widths.max_in(range)
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn element_heights(&self) -> Vec<(u32, f32)> {
        let mut out = Vec::new();
        if self.rope.is_empty() {
            return out;
        }
        let mut cursor = self.rope.cursor();
        assert!(cursor.seek(BYTES, 0, SeekMode::After));
        let mut byte = 0u32;
        loop {
            out.push((byte, cursor.element().height));
            byte = byte.saturating_add(cursor.element().byte_size);
            if !cursor.advance() {
                break;
            }
        }
        out
    }

    pub fn find_misaligned_boundary(&self, text: &Text) -> Option<u32> {
        if self.rope.is_empty() {
            return None;
        }

        let mut view = text.view();
        let byte_count = view.byte_count();
        if self.byte_size() as usize != byte_count {
            return Some(self.byte_size());
        }
        let mut cursor = self.rope.cursor();
        assert!(cursor.seek(BYTES, 0, SeekMode::After));
        let mut boundary = 0u32;
        loop {
            boundary = boundary.saturating_add(cursor.element().byte_size);
            if (boundary as usize) < byte_count {
                let mut byte = Vec::with_capacity(1);
                view.byte_range_into(boundary as usize, boundary as usize + 1, &mut byte);
                if byte.first().is_some_and(|byte| byte & 0xC0 == 0x80) {
                    return Some(boundary);
                }
            }
            if !cursor.advance() {
                break;
            }
        }
        None
    }

    fn assert_repaired(&self) {
        debug_assert!(self.damage.is_empty());
        debug_assert_eq!(
            self.widths.byte_size(),
            u64::from(self.byte_size()),
            "width mirror byte coverage diverged from the layout"
        );
        let metrics = self.metrics();
        debug_assert!(LayoutMeasure::metric_at(&metrics, LINE_STARTS) <= self.len() as u32);
        let _byte_size = LayoutMeasure::metric_at(&metrics, BYTES);
    }

    pub(crate) fn spans_from(&self, byte: u32) -> impl Iterator<Item = (u32, LayoutElement)> + '_ {
        let cursor = (!self.rope.is_empty() && byte < self.byte_size()).then(|| {
            let mut cursor = self.rope.cursor();
            assert!(cursor.seek(BYTES, byte, SeekMode::After));
            cursor
        });
        let mut at = cursor
            .as_ref()
            .map(|cursor| LayoutMeasure::metric_at(&cursor.position(), BYTES));
        let mut cursor = cursor;
        std::iter::from_fn(move || {
            let inner = cursor.as_mut()?;
            let start = at?;
            let element = inner.element().clone();
            at = Some(start.saturating_add(element.byte_size));
            if !inner.advance() {
                cursor = None;
            }
            Some((start, element))
        })
    }

    pub(crate) fn is_unlaid(&self) -> bool {
        self.rope.is_empty()
    }

    pub(crate) fn set_spacer(&mut self, byte: u32, spacer: f32) {
        let mut cursor = self.rope.cursor();
        if !cursor.seek(BYTES, byte, SeekMode::After) {
            return;
        }
        Self::widen(&mut self.spacer_writes, byte..byte.saturating_add(1));
        let mut element = cursor.element().clone();
        element.spacer_above = spacer;
        cursor.delete(1u32);
        cursor.insert(Rope::from_iter([element]));
        self.rope = cursor.rope();
    }

    pub fn height_before(&self, byte: u32) -> f32 {
        if self.rope.is_empty() {
            return 0.0;
        }
        let mut cursor = self.rope.cursor();
        let target = byte.min(self.byte_size().saturating_sub(1));
        if !cursor.seek(BYTES, target, SeekMode::After) {
            return 0.0;
        }
        LayoutMeasure::metric_at(&cursor.position(), VERTICAL_PX) as f32
    }

    pub fn byte_at_y(&self, y: f32) -> u32 {
        let (_, _, byte) = self.cursor_at_y(y);
        byte
    }

    #[doc(hidden)]
    pub fn element_spacers(&self) -> Vec<f32> {
        let mut out = Vec::new();
        if self.is_empty() {
            return out;
        }
        let mut cursor = self.rope.cursor();
        assert!(cursor.seek(BYTES, 0, SeekMode::After));
        loop {
            out.push(cursor.element().spacer_above);
            if !cursor.advance() {
                break;
            }
        }
        out
    }

    #[doc(hidden)]
    pub fn element_byte_ranges(&self) -> Vec<std::ops::Range<u32>> {
        let mut ranges = Vec::new();
        if self.is_empty() {
            return ranges;
        }
        let (mut cursor, _, mut byte_start) = self.cursor_at_y(0.0);
        loop {
            let byte_end = byte_start.saturating_add(cursor.element().byte_size);
            ranges.push(byte_start..byte_end);
            byte_start = byte_end;
            if !cursor.advance() {
                break;
            }
        }
        ranges
    }

    pub(crate) fn cursor_at_y(
        &self,
        scroll_y: f32,
    ) -> (rope::cursor::Cursor<LayoutElement, LayoutMeasure>, f32, u32) {
        let mut cursor = self.rope.cursor();
        let document_y = match cursor.seek(VERTICAL_PX, scroll_y.max(0.0) as u32, SeekMode::After) {
            true => LayoutMeasure::metric_at(&cursor.position(), VERTICAL_PX) as f32,
            false => 0.0,
        };
        let byte_start = LayoutMeasure::metric_at(&cursor.position(), BYTES);
        (cursor, document_y, byte_start)
    }

    pub(crate) fn byte_band(&self, top: f32, bottom: f32) -> std::ops::Range<u32> {
        let (_, _, first) = self.cursor_at_y(top.max(0.0));
        if bottom >= self.height() {
            let total = LayoutMeasure::metric_at(&self.metrics(), BYTES);
            return first..total.max(first);
        }
        let (cursor, _, last_start) = self.cursor_at_y(bottom.max(0.0));
        let last_end = last_start + cursor.element().byte_size;
        first..last_end.max(first)
    }

    pub(crate) fn cursor_at_byte(
        &self,
        byte: u32,
    ) -> Option<(rope::cursor::Cursor<LayoutElement, LayoutMeasure>, f32, u32)> {
        if self.rope.is_empty() {
            return None;
        }

        let mut cursor = self.rope.cursor();
        cursor.seek(BYTES, byte.min(self.byte_size()), SeekMode::After);
        let document_y = LayoutMeasure::metric_at(&cursor.position(), VERTICAL_PX) as f32;
        let byte_start = LayoutMeasure::metric_at(&cursor.position(), BYTES);
        Some((cursor, document_y, byte_start))
    }

    pub(crate) fn cursor_at_caret(
        &self,
        byte: u32,
    ) -> Option<(rope::cursor::Cursor<LayoutElement, LayoutMeasure>, f32, u32)> {
        let (mut cursor, mut document_y, mut byte_start) = self.cursor_at_byte(byte)?;
        loop {
            let item = cursor.element();
            let byte_end = byte_start.saturating_add(item.byte_size);
            if byte < byte_end {
                break;
            }
            let extent = item.height + item.spacer_above;
            if !cursor.advance() {
                break;
            }
            document_y += extent;
            byte_start = byte_end;
        }
        Some((cursor, document_y, byte_start))
    }

    pub fn is_empty(&self) -> bool {
        self.rope.is_empty()
    }

    fn metrics(&self) -> Metrics<DOCUMENT_RANK> {
        self.rope.metrics()
    }

    fn len(&self) -> usize {
        self.rope.len()
    }

    pub(crate) fn byte_size(&self) -> u32 {
        LayoutMeasure::metric_at(&self.metrics(), BYTES)
    }

    /// The element holding `byte_start`, past any zero-byte ones
    /// there. A repair starts exactly where asked: an edit's own start
    /// already stands a row early (`restart_before`), a budget stop
    /// resumes where it stopped.
    fn seek_repair_start(
        cursor: &mut Cursor<LayoutElement, LayoutMeasure>,
        byte_start: u32,
    ) -> u32 {
        assert!(
            cursor.seek(BYTES, byte_start, SeekMode::After),
            "layout repair byte offset must be seekable"
        );
        while cursor.element().byte_size == 0 && cursor.advance() {}

        LayoutMeasure::metric_at(&cursor.position(), BYTES)
    }

    fn insert_bytes(
        cursor: &mut Cursor<LayoutElement, LayoutMeasure>,
        offset: u32,
        len: u32,
        byte_count: u32,
    ) -> u32 {
        if len == 0 {
            return offset;
        }

        if byte_count == 0 {
            cursor.insert(Rope::from_iter([LayoutElement {
                safepoint: true,
                line_start: true,
                bidi: false,
                byte_size: len,
                height: 0.0,
                width: 0.0,
                spacer_above: 0.0,
            }]));
            return 0;
        }

        let target = offset.min(byte_count);
        assert!(
            cursor.seek(BYTES, target, SeekMode::Before),
            "layout edit byte offset must be seekable"
        );
        let start = LayoutMeasure::metric_at(&cursor.position(), BYTES);
        let mut element = cursor.element().clone();
        element.byte_size = element.byte_size.saturating_add(len);
        cursor.delete(1);
        cursor.insert(Rope::from_iter([element]));
        start
    }

    /// Removes `len` bytes at `offset` from the elements holding them.
    /// Returns the start of the first element touched and whether the
    /// deletion began at a line's first byte — the element standing
    /// at `offset` afterwards is then that line's first row, whatever
    /// its own flag says.
    fn delete_bytes(
        cursor: &mut Cursor<LayoutElement, LayoutMeasure>,
        offset: u32,
        mut len: u32,
        byte_count: &mut u32,
    ) -> (u32, bool) {
        if len == 0 || *byte_count == 0 {
            return (offset, false);
        }

        let mut repair_start = offset;
        let mut line_head = false;
        let mut first = true;
        len = len.min(byte_count.saturating_sub(offset));

        while len != 0 && *byte_count != 0 {
            let target = offset.min(*byte_count);
            if !cursor.seek(BYTES, target, SeekMode::After) {
                break;
            }

            let mut start = LayoutMeasure::metric_at(&cursor.position(), BYTES);

            while cursor
                .element()
                .byte_size
                .saturating_sub(target.saturating_sub(start))
                == 0
            {
                if !cursor.advance() {
                    return (repair_start, line_head);
                }
                start = LayoutMeasure::metric_at(&cursor.position(), BYTES);
            }
            repair_start = repair_start.min(start);
            let mut element = cursor.element().clone();
            let local = target.saturating_sub(start);
            if first {
                line_head = local == 0 && element.line_start;
                first = false;
            }
            let available = element.byte_size.saturating_sub(local);
            let removed = len.min(available);
            if removed == 0 {
                break;
            }

            element.byte_size = element.byte_size.saturating_sub(removed);
            cursor.delete(1);
            if element.byte_size != 0 {
                cursor.insert(Rope::from_iter([element]));
            }
            *byte_count = byte_count.saturating_sub(removed);
            len -= removed;
        }

        (repair_start, line_head)
    }
}

impl LayoutElement {
    fn new(byte_size: usize, height: f32) -> Self {
        Self {
            safepoint: true,
            line_start: true,
            bidi: false,
            byte_size: byte_size.min(u32::MAX as usize) as u32,
            height,
            width: 0.0,
            spacer_above: 0.0,
        }
    }
}

impl Measure<LayoutElement> for LayoutMeasure {
    type Metrics = Metrics<DOCUMENT_RANK>;

    fn zero() -> Self::Metrics {
        Metrics::zero()
    }

    fn metric_at(metrics: &Self::Metrics, id: MetricId) -> u32 {
        metrics.metric_at(id)
    }

    fn add_assign(metrics: &mut Self::Metrics, other: Self::Metrics) {
        metrics.add_assign(other);
    }

    fn sub_assign(metrics: &mut Self::Metrics, other: Self::Metrics) {
        metrics.sub_assign(other);
    }

    fn measure(item: &LayoutElement) -> Self::Metrics {
        Metrics([
            (item.height + item.spacer_above)
                .ceil()
                .min(u32::MAX as f32) as u32,
            u32::from(item.line_start),
            item.byte_size,
        ])
    }
}

struct LayoutSafepoints<'a> {
    damage: &'a Damage,
}

impl Safepoint<LayoutElement> for LayoutSafepoints<'_> {
    fn can_resume(&self, element: &LayoutElement, alignment: u32) -> bool {
        // A bidi line is laid whole: no resuming on its inner rows.
        if element.bidi && !element.line_start {
            return false;
        }
        let start = match element.byte_size {
            0 => alignment.saturating_sub(1),
            _ => alignment,
        };
        !self
            .damage
            .any_intersecting(start..alignment.saturating_add(element.byte_size.max(1)))
    }

    fn is_safepoint(&self, element: &LayoutElement) -> bool {
        element.safepoint
    }

    fn cut(&self, mut element: LayoutElement, offset: u32) -> LayoutElement {
        element.byte_size = element.byte_size.saturating_sub(offset);
        element.safepoint = true;
        element.line_start = false;
        element
    }
}

struct WindowedItems<'a> {
    inner: LayoutItems<'a>,

    at: u32,
    window: std::ops::Range<u32>,
    text_end: u32,
    suffix_emitted: bool,
}

impl<'a> WindowedItems<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        text: &'a Text,
        markup: crate::markup::OverlaidMarkup<'a, 'a>,
        start_byte: u32,
        window: std::ops::Range<u32>,
        text_end: u32,
        measure: crate::markup::InlayMeasure<'a>,
        theme: &'a crate::theme::Theme,
    ) -> Self {
        let inner_start = start_byte.max(window.start).min(window.end);

        let trailing_line = window.start == 0 && window.end >= text_end;
        Self {
            inner: LayoutItems::new(
                text,
                markup,
                inner_start,
                window.end,
                trailing_line,
                measure,
                theme,
            ),
            at: start_byte,
            window,
            text_end,
            suffix_emitted: false,
        }
    }

    fn placeholder(byte_size: u32) -> LayoutElement {
        LayoutElement {
            safepoint: true,
            line_start: true,
            bidi: false,
            byte_size,
            height: 0.0,
            width: 0.0,
            spacer_above: 0.0,
        }
    }
}

impl Iterator for WindowedItems<'_> {
    type Item = LayoutElement;

    fn next(&mut self) -> Option<Self::Item> {
        if self.at < self.window.start {
            let gap = self.window.start - self.at;
            self.at = self.window.start;
            return Some(Self::placeholder(gap));
        }
        if let Some(element) = self.inner.next() {
            self.at = self.at.saturating_add(element.byte_size);
            return Some(element);
        }
        if self.at < self.text_end && !self.suffix_emitted {
            self.suffix_emitted = true;
            let gap = self.text_end - self.at;
            self.at = self.text_end;
            return Some(Self::placeholder(gap));
        }
        None
    }
}

struct LayoutItems<'a> {
    view: TextView,
    markup: crate::markup::OverlaidMarkup<'a, 'a>,
    pending: std::vec::IntoIter<LayoutElement>,
    run: PlainRun,

    trailing_line: bool,
    measure: crate::markup::InlayMeasure<'a>,
    theme: &'a crate::theme::Theme,
}

struct PlainRun {
    end: u32,
    consumed: u32,
}

const PARAGRAPH_CHUNK: u32 = 16 * 1024;

fn next_plain_paragraph(view: &mut TextView, run: &mut PlainRun) -> Option<(String, u32)> {
    if run.consumed >= run.end {
        return None;
    }
    let start = run.consumed;
    let grid_end = (start / PARAGRAPH_CHUNK)
        .saturating_add(1)
        .saturating_mul(PARAGRAPH_CHUNK)
        .min(run.end);
    let mut bytes = Vec::new();
    view.byte_range_into(start as usize, grid_end as usize, &mut bytes);
    match bytes.iter().position(|byte| *byte == b'\n') {
        Some(newline) => bytes.truncate(newline + 1),
        None if grid_end == run.end => {}
        None => {
            let line = view.line_at(start as usize);
            let line_start = view.line_start_offset(line).min(u32::MAX as usize) as u32;
            let line_end = view.line_end_offset(line).min(u32::MAX as usize) as u32;
            match line_end.saturating_sub(line_start) <= PARAGRAPH_CHUNK {
                true => {
                    let end = line_end.min(run.end);
                    if end as usize > start as usize + bytes.len() {
                        let mut tail = Vec::new();
                        view.byte_range_into(start as usize + bytes.len(), end as usize, &mut tail);
                        bytes.extend_from_slice(&tail);
                    }
                }

                false => {
                    let mut cut = bytes.len();
                    let spare_end = grid_end.saturating_add(3).min(run.end);
                    if spare_end > grid_end {
                        let mut spare = Vec::new();
                        view.byte_range_into(grid_end as usize, spare_end as usize, &mut spare);
                        bytes.extend_from_slice(&spare);
                    }
                    while cut < bytes.len() && (bytes[cut] & 0xC0) == 0x80 {
                        cut += 1;
                    }
                    bytes.truncate(cut);
                }
            }
        }
    }
    let text = String::from_utf8(bytes).expect("plain runs start and end on char boundaries");
    if text.is_empty() {
        return None;
    }
    run.consumed = start.saturating_add(text.len().min(u32::MAX as usize) as u32);
    Some((text, start))
}

impl<'a> LayoutItems<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        text: &'a Text,
        markup: crate::markup::OverlaidMarkup<'a, 'a>,
        start_byte: u32,
        end_byte: u32,
        trailing_line: bool,
        measure: crate::markup::InlayMeasure<'a>,
        theme: &'a crate::theme::Theme,
    ) -> Self {
        let view = text.view();
        let text_byte_count = view.byte_count().min(u32::MAX as usize) as u32;
        let text_byte_count = text_byte_count.min(end_byte);
        let start_byte = start_byte.min(text_byte_count);

        Self {
            view,
            markup,
            pending: Vec::new().into_iter(),
            run: PlainRun {
                end: text_byte_count,
                consumed: start_byte,
            },
            trailing_line,
            measure,
            theme,
        }
    }
}

struct BudgetedItems<'a> {
    inner: std::iter::Peekable<WindowedItems<'a>>,
    remaining: f32,

    remaining_items: u32,
    /// The last row handed out was of a bidi line, which is laid
    /// whole: a spent budget runs on to the line's end.
    in_bidi_line: bool,
}

impl Iterator for BudgetedItems<'_> {
    type Item = LayoutElement;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining <= 0.0 || self.remaining_items == 0 {
            let next = self.inner.peek()?;
            if next.byte_size != 0 && !(self.in_bidi_line && !next.line_start) {
                return None;
            }
            let item = self.inner.next()?;
            self.in_bidi_line = item.bidi;
            return Some(item);
        }
        let item = self.inner.next()?;
        self.in_bidi_line = item.bidi;
        self.remaining -= item.height;
        self.remaining_items = self.remaining_items.saturating_sub(1);
        Some(item)
    }
}

impl Iterator for LayoutItems<'_> {
    type Item = LayoutElement;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.pending.next() {
                return Some(item);
            }

            let pos = self.run.consumed;
            if pos < self.run.end {
                let covered_to = self
                    .markup
                    .all_inlays_in(pos..pos.saturating_add(1))
                    .iter()
                    .filter(|hit| matches!(hit.inlay.mode(), crate::markup::InlayMode::Instead(_)))
                    .filter(|hit| hit.range.start < pos && hit.range.end > pos)
                    .map(|hit| hit.range.end)
                    .max();
                if let Some(covered_to) = covered_to {
                    let end = covered_to.min(self.run.end);

                    let stop = match end >= self.run.end {
                        true => self.run.end,
                        false => {
                            let line = self.view.line_at(end as usize);
                            (self.view.line_start_offset(line).min(u32::MAX as usize) as u32)
                                .min(self.run.end)
                        }
                    };
                    if stop > pos {
                        if crate::env_flags::trace_skip() {
                            let hits = self.markup.all_inlays_in(pos..stop);
                            let nested: Vec<_> = hits
                                .iter()
                                .filter(|hit| hit.range.start >= pos)
                                .map(|hit| (hit.inlay.mode(), hit.range.clone()))
                                .collect();
                            eprintln!("[skip] {pos}..{stop} nested={nested:?}");
                        }
                        self.run.consumed = stop;
                        return Some(LayoutElement::new((stop - pos) as usize, 0.0));
                    }
                }
            }

            let (raw, absolute_start) = next_plain_paragraph(&mut self.view, &mut self.run)?;
            let mut bidi = raw.chars().any(|ch| strong_direction(ch) == Some(true));
            let mut unit_end =
                absolute_start.saturating_add(raw.len().min(u32::MAX as usize) as u32);
            let mut segments: Vec<(String, u32)> = vec![(raw, 0)];

            let mut inlay_hits = self.markup.all_inlays_in(absolute_start..unit_end);
            loop {
                let cross = inlay_hits.iter().find(|hit| {
                    hit.inlay.mode()
                        == crate::markup::InlayMode::Instead(crate::markup::InsteadKind::Inline)
                        && hit.range.start >= absolute_start
                        && hit.range.start < unit_end
                        && hit.range.end > unit_end
                });
                let Some(cross) = cross else {
                    break;
                };
                let resume = cross.range.end;
                if resume >= self.run.end {
                    unit_end = self.run.end;
                    self.run.consumed = self.run.end;
                    break;
                }
                let (tail, tail_start) = read_line_tail(&mut self.view, resume, self.run.end);
                if tail.is_empty() {
                    unit_end = resume;
                    self.run.consumed = resume;
                    break;
                }
                let tail_len = tail.len().min(u32::MAX as usize) as u32;
                segments.push((tail, tail_start - absolute_start));
                unit_end = tail_start.saturating_add(tail_len);
                self.run.consumed = unit_end;
                inlay_hits = self.markup.all_inlays_in(absolute_start..unit_end);
            }

            let mut inline = Vec::new();
            let mut hidden = Vec::new();
            let mut marks = self.markup.marks_inline_hidden_in(
                absolute_start..unit_end,
                &mut inline,
                &mut hidden,
            );
            // A unit entered mid-line — a repair resuming a row in, or
            // a chunk of a giant line — still wears its HARD LINE's
            // block style: the style is the line's, whichever of its
            // bytes the interval touches, and a fresh build lays the
            // whole line under it.
            let line_start = {
                let line = self.view.line_at(absolute_start as usize);
                self.view.line_start_offset(line).min(u32::MAX as usize) as u32
            };
            // ...and is shaped under the hard line's base direction,
            // read off the line's first strong character when that
            // character lies before the unit; else the unit's own
            // first strong character is the line's too.
            let mut base_direction = imba::shape::BaseDirection::Auto;
            if line_start < absolute_start {
                let mut scratch_inline = Vec::new();
                let mut scratch_hidden = Vec::new();
                marks = self.markup.marks_inline_hidden_in(
                    line_start..unit_end,
                    &mut scratch_inline,
                    &mut scratch_hidden,
                );
                let mut prefix = Vec::new();
                self.view.byte_range_into(
                    line_start as usize,
                    absolute_start as usize,
                    &mut prefix,
                );
                let prefix = String::from_utf8_lossy(&prefix);
                base_direction = match prefix.chars().find_map(strong_direction) {
                    Some(true) => imba::shape::BaseDirection::Rtl,
                    Some(false) => imba::shape::BaseDirection::Ltr,
                    None => imba::shape::BaseDirection::Auto,
                };
                bidi |= prefix.chars().any(|ch| strong_direction(ch) == Some(true));
            }
            let mut items = Vec::new();
            append_paragraph(
                segments,
                inlay_hits,
                absolute_start,
                unit_end,
                line_start == absolute_start,
                base_direction,
                bidi,
                self.trailing_line && unit_end == self.run.end,
                marks,
                &inline,
                &hidden,
                &mut items,
                self.measure,
                self.theme,
                self.markup,
            );
            self.pending = items.into_iter();
        }
    }
}

/// Whether the hard line holding `byte` has right-to-left text.
fn line_has_rtl(text: &Text, byte: u32) -> bool {
    let mut view = text.view();
    let count = view.byte_count();
    if count == 0 || byte as usize >= count {
        return false;
    }
    let line = view.line_at(byte as usize);
    let start = view.line_start_offset(line);
    let end = view.line_end_offset(line).min(count);
    let mut bytes = Vec::new();
    view.byte_range_into(start, end, &mut bytes);
    String::from_utf8_lossy(&bytes)
        .chars()
        .any(|ch| strong_direction(ch) == Some(true))
}

/// `Some(true)` for a strongly right-to-left character, `Some(false)`
/// for a strongly left-to-right one, `None` for a neutral.
fn strong_direction(ch: char) -> Option<bool> {
    let code = u32::from(ch);
    let rtl = matches!(
        code,
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF
    );
    if rtl {
        return Some(true);
    }
    ch.is_alphabetic().then_some(false)
}

fn read_line_tail(view: &mut TextView, at: u32, limit: u32) -> (String, u32) {
    let line = view.line_at(at as usize);
    let mut end = (view.line_end_offset(line).min(u32::MAX as usize) as u32).min(limit);
    if end.saturating_sub(at) > PARAGRAPH_CHUNK {
        let mut cut = at.saturating_add(PARAGRAPH_CHUNK);
        while cut < end && !view.is_char_boundary(cut as usize) {
            cut += 1;
        }
        end = cut;
    }
    let mut bytes: Vec<u8> = Vec::with_capacity(end.saturating_sub(at) as usize);
    view.byte_range_into(at as usize, end as usize, &mut bytes);
    (
        String::from_utf8(bytes).expect("line tails start and end on char boundaries"),
        at,
    )
}

#[allow(clippy::too_many_arguments)]
fn append_paragraph(
    segments: Vec<(String, u32)>,
    inlay_hits: Vec<crate::markup::InlayInterval<'_>>,
    absolute_start: u32,
    unit_end: u32,
    starts_line: bool,
    base_direction: imba::shape::BaseDirection,
    bidi: bool,
    final_unit: bool,
    marks: BlockStyle,
    inline: &[TextDecorationInterval],
    hidden: &[Range<u32>],
    items: &mut Vec<LayoutElement>,
    measure: crate::markup::InlayMeasure<'_>,
    theme: &crate::theme::Theme,
    markup: crate::markup::OverlaidMarkup<'_, '_>,
) {
    let byte_size = unit_end.saturating_sub(absolute_start) as usize;
    let resolved = marks.resolved(theme);
    let item = LayoutElement::new(byte_size, resolved.block_height.unwrap_or(0.0));
    // Every row resumes a repair: a row boundary is a break
    // opportunity, and the rows after one come out the same whether
    // the line was laid from its start or from there. The one
    // exception is the trailing empty line after a final newline:
    // its height is its line's, so it is laid with its line and never
    // resumed on its own.
    let unit_len = unit_end.saturating_sub(absolute_start);
    let mut at = 0u32;
    items.extend(
        visual_lines(
            item,
            marks,
            segments,
            inlay_hits,
            absolute_start,
            unit_end,
            final_unit,
            base_direction,
            measure,
            theme,
            markup,
            inline,
            hidden,
        )
        .map(move |mut row| {
            // A unit entered mid-line (a repair resuming a row in, a
            // chunk of a giant line) starts no hard line.
            row.line_start = at == 0 && starts_line;
            row.bidi = bidi;
            row.safepoint = !(row.byte_size == 0 && at == unit_len);
            at = at.saturating_add(row.byte_size);
            row
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn visual_lines<'a>(
    mut item: LayoutElement,
    marks: BlockStyle,
    segments: Vec<(String, u32)>,
    inlay_hits: Vec<crate::markup::InlayInterval<'a>>,
    absolute_start: u32,
    unit_end: u32,
    final_unit: bool,
    base_direction: imba::shape::BaseDirection,
    measure: crate::markup::InlayMeasure<'a>,
    theme: &'a crate::theme::Theme,
    markup: crate::markup::OverlaidMarkup<'a, 'a>,
    inline: &[TextDecorationInterval],
    hidden: &[Range<u32>],
) -> VisualLines<'a> {
    let width = measure.width;
    let paragraph_range = absolute_start..unit_end;
    let unit_len = unit_end.saturating_sub(absolute_start) as usize;

    let covered_by_instead = inlay_hits.iter().any(|hit| {
        hit.inlay.mode() == crate::markup::InlayMode::Instead(crate::markup::InsteadKind::FullLine)
            && hit.range.start <= paragraph_range.start
            && hit.range.end >= paragraph_range.end
    });
    if covered_by_instead {
        let mut item = item;
        item.height = height_from_hits(0.0, paragraph_range, &inlay_hits, measure);
        return VisualLines::Single(Some(item));
    }
    let resolved = marks.resolved(theme);

    let end_gap = if marks.continues_past() {
        0.0
    } else {
        block_gap(&resolved)
    };
    if resolved.rule.is_some() {
        item.height = height_from_hits(
            resolved.block_height.unwrap_or(0.0),
            paragraph_range.clone(),
            &inlay_hits,
            measure,
        );
        return VisualLines::Single(Some(item));
    }

    let mut display_hidden: Vec<Range<u32>> = hidden.to_vec();
    for hit in &inlay_hits {
        if hit.inlay.mode() == crate::markup::InlayMode::Instead(crate::markup::InsteadKind::Inline)
        {
            let start = hit
                .range
                .start
                .clamp(paragraph_range.start, paragraph_range.end);
            let end = hit
                .range
                .end
                .clamp(paragraph_range.start, paragraph_range.end);
            if start < end {
                display_hidden.push(start - absolute_start..end - absolute_start);
            }
        }
    }
    display_hidden.sort_by_key(|range| range.start);

    let mut text = DisplayText::build_segments(
        resolved.rule.is_some(),
        &segments,
        &display_hidden,
        false,
        unit_len,
    );
    text.add_inline_placeholders(markup, paragraph_range.clone(), measure);

    let inline = text.map_decorations(inline);
    let rows_share_metrics =
        crate::shaped_line::rows_share_metrics(&inline, text.placeholders(), theme);
    let mut inputs = crate::shaped_line::shape_inputs(&resolved, &text, &inline, theme);
    inputs.base_direction = base_direction;
    let line = inputs.shape(
        measure.ui,
        Some((width - resolved.inset.unwrap_or(0.0) * 2.0).max(60.0)),
    );

    let inset = resolved.inset.unwrap_or(0.0);
    item.width = line.width() + inset * 2.0;
    let rows = line.rows();
    let excisions = inputs.excisions;
    let mut line_count = rows.len();

    let trailing_empty_line = line_count > 1 && text.as_str().ends_with('\n') && final_unit;
    if line_count > 1 && text.as_str().ends_with('\n') && !trailing_empty_line {
        line_count -= 1;
    }

    if line_count == 0 {
        item.height = height_from_hits(end_gap, paragraph_range, &inlay_hits, measure);
        return VisualLines::Single(Some(item));
    }

    VisualLines::Split(LineSplit {
        item,
        text,
        rows,
        excisions,
        end_gap,
        inset,
        gap_before_trailing_empty: trailing_empty_line,
        line_index: 0,
        line_count,
        byte_start: 0,
        text_start: 0,
        absolute_start,
        measure,
        inlay_hits,
        rows_share_metrics,
        shared_text_height: None,
    })
}

fn height_from_hits(
    text_height: f32,
    range: std::ops::Range<u32>,
    hits: &[crate::markup::InlayInterval<'_>],
    measure: crate::markup::InlayMeasure<'_>,
) -> f32 {
    crate::markup::OverlaidMarkup::metrics_from(hits, &range, measure).total_height(text_height)
}

#[allow(clippy::large_enum_variant)]
enum VisualLines<'a> {
    Single(Option<LayoutElement>),
    Split(LineSplit<'a>),
}

impl Iterator for VisualLines<'_> {
    type Item = LayoutElement;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            VisualLines::Single(item) => item.take(),
            VisualLines::Split(lines) => lines.next(),
        }
    }
}

struct LineSplit<'a> {
    item: LayoutElement,
    text: DisplayText,
    rows: Vec<imba::shape::ShapedRow>,
    excisions: Vec<Excision>,

    end_gap: f32,
    inset: f32,

    gap_before_trailing_empty: bool,
    line_index: usize,
    line_count: usize,
    byte_start: usize,
    text_start: usize,
    absolute_start: u32,
    measure: crate::markup::InlayMeasure<'a>,

    inlay_hits: Vec<crate::markup::InlayInterval<'a>>,

    rows_share_metrics: bool,

    shared_text_height: Option<f32>,
}

impl LineSplit<'_> {
    fn text_height(&mut self, line_index: usize) -> Option<f32> {
        if let Some(height) = self.shared_text_height {
            return Some(height);
        }
        let row = self.rows.get(line_index)?;
        let height = (row.bottom - row.top).ceil().max(1.0);
        if self.rows_share_metrics {
            self.shared_text_height = Some(height);
        }
        Some(height)
    }
}

impl Iterator for LineSplit<'_> {
    type Item = LayoutElement;

    fn next(&mut self) -> Option<Self::Item> {
        if self.line_index >= self.line_count {
            return None;
        }

        let line_index = self.line_index;
        let is_last = line_index + 1 == self.line_count;
        let row_end = self
            .rows
            .get(line_index)
            .map(|row| display_byte_of(&self.excisions, row.text_range.end))
            .unwrap_or_else(|| self.text.as_str().len());
        let text_end = self.text.line_text_end_at(row_end, is_last);
        let byte_start = self.byte_start;
        let byte_end = self.text.line_byte_end(text_end, self.byte_start, is_last);
        let byte_size = byte_end.saturating_sub(self.byte_start);
        self.byte_start = byte_end;
        self.text_start = text_end;
        self.line_index += 1;

        let mut item = self.item.clone();
        item.byte_size = byte_size.min(u32::MAX as usize) as u32;
        // The row's OWN width, not the line's widest: a repair that
        // enters a line mid-way re-lays only rows, and a per-line
        // figure stamped on every row would go stale on the rest.
        if let Some(row) = self.rows.get(line_index) {
            item.width = row.width + self.inset * 2.0;
        }
        let mut text_height = self.text_height(line_index)?;
        let is_before_trailing_empty =
            self.gap_before_trailing_empty && line_index + 2 == self.line_count;
        if is_last || is_before_trailing_empty {
            text_height += self.end_gap;
        }
        item.height = height_from_hits(
            text_height,
            self.absolute_start
                .saturating_add(byte_start.min(u32::MAX as usize) as u32)
                ..self
                    .absolute_start
                    .saturating_add(byte_end.min(u32::MAX as usize) as u32),
            &self.inlay_hits,
            self.measure,
        );
        Some(item)
    }
}

#[cfg(test)]
mod delete_accounting_tests {
    use super::*;

    fn element(byte_size: u32, height: f32) -> LayoutElement {
        LayoutElement {
            safepoint: true,
            line_start: true,
            bidi: false,
            byte_size,
            height,
            width: 0.0,
            spacer_above: 0.0,
        }
    }

    fn layout_of(elements: Vec<LayoutElement>) -> DocumentLayout {
        let mut layout = DocumentLayout::new();
        layout.rope = Rope::from_iter(elements);
        layout
    }

    #[test]
    fn deleting_across_a_zero_byte_element_keeps_byte_accounting() {
        let mut layout = layout_of(vec![element(10, 20.0), element(0, 0.0), element(10, 20.0)]);
        layout.edit(&Operation::from_ops([
            Op::Retain(8),
            Op::Delete("abcd".to_owned()),
        ]));
        assert_eq!(layout.byte_size(), 16, "all four bytes leave the layout");
        assert_eq!(layout.element_byte_ranges(), vec![0..8, 8..8, 8..16]);
    }

    #[test]
    fn deleting_at_a_zero_byte_element_keeps_byte_accounting() {
        let mut layout = layout_of(vec![element(10, 20.0), element(0, 0.0), element(10, 20.0)]);
        layout.edit(&Operation::from_ops([
            Op::Retain(10),
            Op::Delete("ab".to_owned()),
        ]));
        assert_eq!(layout.byte_size(), 18, "both bytes leave the layout");
        assert_eq!(layout.element_byte_ranges(), vec![0..10, 10..10, 10..18]);
    }
}

#[cfg(test)]
mod tail_replacement_tests {
    use super::*;

    fn element(byte_size: u32) -> LayoutElement {
        LayoutElement {
            safepoint: true,
            line_start: true,
            bidi: false,
            byte_size,
            height: 20.0,
            width: 0.0,
            spacer_above: 0.0,
        }
    }

    fn layout_of(count: usize) -> DocumentLayout {
        let mut layout = DocumentLayout::new();
        layout.rope = Rope::from_iter((0..count).map(|_| element(1)));
        layout
    }

    /// A replacement that reaches the end of the text — delete the
    /// tail, insert new text there — crossing a leaf boundary of the
    /// layout rope. The deletes empty the last leaf one element at a
    /// time; the insert then seeks the rope's end and must land on
    /// the element that ends there. This was the 2026-10-25 abort
    /// (`layout edit byte offset must be seekable`) on an 82 KB
    /// docsync offer.
    #[test]
    fn replacing_the_tail_across_a_leaf_boundary_keeps_byte_accounting() {
        for count in [65usize, 100, 200, 1000] {
            let kept = (count / 2) as u32;
            let mut layout = layout_of(count);
            layout.edit(&Operation::from_ops([
                Op::Retain(kept),
                Op::Delete("x".repeat(count - kept as usize)),
                Op::Insert("yyy".to_owned()),
            ]));
            assert_eq!(layout.byte_size(), kept + 3, "count {count}");
        }
    }

    /// The same shape with the insert spelled before the delete, as
    /// a diff producer may order it.
    #[test]
    fn inserting_before_deleting_the_tail_keeps_byte_accounting() {
        for count in [65usize, 200] {
            let kept = (count / 2) as u32;
            let mut layout = layout_of(count);
            layout.edit(&Operation::from_ops([
                Op::Retain(kept),
                Op::Insert("yyy".to_owned()),
                Op::Delete("x".repeat(count - kept as usize)),
            ]));
            assert_eq!(layout.byte_size(), kept + 3, "count {count}");
        }
    }
}
