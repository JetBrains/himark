// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use operation::{Op, Operation};
use rope::{Cursor, Measure, MetricId, Metrics, Rope, SeekMode};
use skia_safe::textlayout::{FontCollection, LineMetrics, Paragraph};
use text::{Text, TextView};
use tokenize::{rewrite, Safepoint};

use crate::{
    markup::{BlockStyle, TextDecorationInterval},
    shaped_line::{block_gap, paragraph, DisplayText},
};

const DOCUMENT_RANK: usize = 3;
const VERTICAL_PX: MetricId = MetricId(0);
const SAFEPOINTS: MetricId = MetricId(1);
pub(crate) const BYTES: MetricId = MetricId(2);

#[derive(Clone)]
pub(crate) struct LayoutElement {
    pub(crate) safepoint: bool,
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

pub const SYNC_LAYOUT_HEIGHT: f32 = 4_000.0;

impl DocumentLayout {
    pub fn build(
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        measure: crate::markup::InlayMeasure<'_>,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        window: Option<Range<u32>>,
    ) -> Self {
        let mut layout = Self::new();
        layout.window = window;
        layout.repair_layout_bounded(text, markup, measure, fonts, theme, 0, SYNC_LAYOUT_HEIGHT);

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

    /// Test-support: the float-width builders, paying the
    /// per-call measure seed — production threads `InlayMeasure`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn build_slow(
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        width: f32,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        window: Option<Range<u32>>,
    ) -> Self {
        {
            let mut seeded = imba::store::Store::new();
            crate::env::Themes::set(&mut seeded, theme.clone());
            let ui = imba::UiCtx::dont_use_too_slow();
            let measure = crate::markup::InlayMeasure {
                width,
                store: &seeded,
                ui: &ui,
            };
            Self::build(text, markup, measure, fonts, theme, window)
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn build_complete_slow(
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        width: f32,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        window: Option<Range<u32>>,
    ) -> Self {
        {
            let mut seeded = imba::store::Store::new();
            crate::env::Themes::set(&mut seeded, theme.clone());
            let ui = imba::UiCtx::dont_use_too_slow();
            let measure = crate::markup::InlayMeasure {
                width,
                store: &seeded,
                ui: &ui,
            };
            Self::build_complete(text, markup, measure, fonts, theme, window)
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn repair_layout_bounded_slow(
        &mut self,
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        width: f32,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        byte_start: u32,
        height_budget: f32,
    ) {
        {
            let mut seeded = imba::store::Store::new();
            crate::env::Themes::set(&mut seeded, theme.clone());
            let ui = imba::UiCtx::dont_use_too_slow();
            let measure = crate::markup::InlayMeasure {
                width,
                store: &seeded,
                ui: &ui,
            };
            self.repair_layout_bounded(
                text,
                markup,
                measure,
                fonts,
                theme,
                byte_start,
                height_budget,
            )
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn repair_layout_slow(
        &mut self,
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        width: f32,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        byte_start: u32,
    ) {
        {
            let mut seeded = imba::store::Store::new();
            crate::env::Themes::set(&mut seeded, theme.clone());
            let ui = imba::UiCtx::dont_use_too_slow();
            let measure = crate::markup::InlayMeasure {
                width,
                store: &seeded,
                ui: &ui,
            };
            self.repair_layout(text, markup, measure, fonts, theme, byte_start)
        }
    }

    pub fn build_complete(
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        measure: crate::markup::InlayMeasure<'_>,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        window: Option<Range<u32>>,
    ) -> Self {
        let mut layout = Self::build(text, markup, measure, fonts, theme, window);
        if let Some(pending) = layout.repair_pending() {
            layout.repair_layout(text, markup, measure, fonts, theme, pending);
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

    pub fn shaped_theme(&self) -> &str {
        &self.shaped_theme
    }

    pub fn height(&self) -> f32 {
        LayoutMeasure::metric_at(&self.metrics(), VERTICAL_PX) as f32
    }

    pub(crate) fn bounded(&self) -> bool {
        self.window.is_some()
    }

    pub(crate) fn set_window(&mut self, window: Option<Range<u32>>) {
        self.window = window;
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

        for op in operation.iter() {
            match op {
                Op::Retain(len) => {
                    offset = offset.saturating_add(len);
                }
                Op::Insert(text) => {
                    let len = text.len().min(u32::MAX as usize) as u32;
                    let start = Self::insert_bytes(&mut cursor, offset, len, byte_count);
                    damaged.push(start);
                    repair_start =
                        Some(repair_start.map_or(start, |existing: u32| existing.min(start)));
                    byte_count = byte_count.saturating_add(len);
                    offset = offset.saturating_add(len);
                }
                Op::Delete(text) => {
                    let len = text.len().min(u32::MAX as usize) as u32;
                    let start = Self::delete_bytes(&mut cursor, offset, len, &mut byte_count);
                    damaged.push(start);
                    repair_start =
                        Some(repair_start.map_or(start, |existing: u32| existing.min(start)));
                }
            }
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
        repair_start.unwrap_or(offset)
    }

    pub fn mark_modified(&mut self, byte_start: u32) -> u32 {
        if self.byte_size() == 0 {
            return 0;
        }
        let byte_start = byte_start.min(self.byte_size().saturating_sub(1));
        self.damage.add(byte_start..byte_start.saturating_add(1));
        byte_start
    }

    pub fn mark_modified_in(&mut self, range: Range<u32>) -> u32 {
        if self.byte_size() == 0 {
            return 0;
        }

        let byte_size = self.byte_size();
        let range = range.start.min(byte_size)..range.end.min(byte_size);
        if range.start >= range.end {
            return self.mark_modified(range.start);
        }
        self.damage.add(range.clone());
        range.start
    }

    pub fn repair_layout(
        &mut self,
        text: &Text,
        markup: crate::markup::OverlaidMarkup<'_, '_>,
        measure: crate::markup::InlayMeasure<'_>,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        byte_start: u32,
    ) {
        let started = crate::startup_profile::start();
        self.repair_region(text, markup, measure, fonts, theme, byte_start, f32::MAX);
        while let Some(pending) = self.repair_pending() {
            self.repair_region(text, markup, measure, fonts, theme, pending, f32::MAX);
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
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        byte_start: u32,
        height_budget: f32,
    ) {
        self.repair_region(
            text,
            markup,
            measure,
            fonts,
            theme,
            byte_start,
            height_budget,
        );
    }

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
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        byte_start: u32,
        height_budget: f32,
    ) {
        let width = measure.width;
        self.layout_width = width;
        self.shaped_theme = theme.name_shared();

        let mut cursor = self.rope.cursor();
        let repair_start = match self.rope.is_empty() {
            true => 0,
            false => Self::seek_repair_start(&mut cursor, byte_start.min(self.byte_size())),
        };
        let text_count = text.view().byte_count().min(u32::MAX as usize) as u32;
        let window = match &self.window {
            Some(range) => range.start.min(text_count)..range.end.min(text_count),
            None => 0..text_count,
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
                fonts,
                theme,
            )
            .peekable(),
            remaining: height_budget,

            remaining_items: 512,
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

    pub fn max_width_in(&self, range: Range<u32>) -> f32 {
        self.widths.max_in(range)
    }

    #[cfg(any(test, feature = "test-support"))]
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
        debug_assert!(LayoutMeasure::metric_at(&metrics, SAFEPOINTS) <= self.len() as u32);
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
    ) -> (rope::Cursor<LayoutElement, LayoutMeasure>, f32, u32) {
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
    ) -> Option<(rope::Cursor<LayoutElement, LayoutMeasure>, f32, u32)> {
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
    ) -> Option<(rope::Cursor<LayoutElement, LayoutMeasure>, f32, u32)> {
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

    fn seek_repair_start(
        cursor: &mut Cursor<LayoutElement, LayoutMeasure>,
        byte_start: u32,
    ) -> u32 {
        assert!(
            cursor.seek(BYTES, byte_start, SeekMode::After),
            "layout repair byte offset must be seekable"
        );

        if !cursor.element().safepoint {
            let safepoints = LayoutMeasure::metric_at(&cursor.position(), SAFEPOINTS);
            if safepoints == 0 {
                assert!(
                    cursor.seek(BYTES, 0, SeekMode::After),
                    "layout start must be seekable"
                );
            } else {
                assert!(
                    cursor.seek(SAFEPOINTS, safepoints, SeekMode::Before),
                    "previous layout safepoint must be seekable"
                );
            }
        }

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

    fn delete_bytes(
        cursor: &mut Cursor<LayoutElement, LayoutMeasure>,
        offset: u32,
        mut len: u32,
        byte_count: &mut u32,
    ) -> u32 {
        if len == 0 || *byte_count == 0 {
            return offset;
        }

        let mut repair_start = offset;
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
                    return repair_start;
                }
                start = LayoutMeasure::metric_at(&cursor.position(), BYTES);
            }
            repair_start = repair_start.min(start);
            let mut element = cursor.element().clone();
            let local = target.saturating_sub(start);
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

        repair_start
    }
}

impl LayoutElement {
    fn new(byte_size: usize, height: f32) -> Self {
        Self {
            safepoint: true,
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
            u32::from(item.safepoint),
            item.byte_size,
        ])
    }
}

struct LayoutSafepoints<'a> {
    damage: &'a Damage,
}

impl Safepoint<LayoutElement> for LayoutSafepoints<'_> {
    fn can_resume(&self, element: &LayoutElement, alignment: u32) -> bool {
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
        fonts: &'a FontCollection,
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
                fonts,
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
    fonts: &'a FontCollection,
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
        fonts: &'a FontCollection,
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
            fonts,
            theme,
        }
    }
}

struct BudgetedItems<'a> {
    inner: std::iter::Peekable<WindowedItems<'a>>,
    remaining: f32,

    remaining_items: u32,
}

impl Iterator for BudgetedItems<'_> {
    type Item = LayoutElement;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining <= 0.0 || self.remaining_items == 0 {
            if self.inner.peek()?.byte_size != 0 {
                return None;
            }
            return self.inner.next();
        }
        let item = self.inner.next()?;
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
            let marks = self.markup.marks_inline_hidden_in(
                absolute_start..unit_end,
                &mut inline,
                &mut hidden,
            );
            let mut items = Vec::new();
            append_paragraph(
                segments,
                inlay_hits,
                absolute_start,
                unit_end,
                self.trailing_line && unit_end == self.run.end,
                marks,
                &inline,
                &hidden,
                &mut items,
                self.measure,
                self.fonts,
                self.theme,
                self.markup,
            );
            self.pending = items.into_iter();
        }
    }
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
    final_unit: bool,
    marks: BlockStyle,
    inline: &[TextDecorationInterval],
    hidden: &[Range<u32>],
    items: &mut Vec<LayoutElement>,
    measure: crate::markup::InlayMeasure<'_>,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &crate::theme::Theme,
    markup: crate::markup::OverlaidMarkup<'_, '_>,
) {
    let byte_size = unit_end.saturating_sub(absolute_start) as usize;
    let resolved = marks.resolved(theme);
    let item = LayoutElement::new(byte_size, resolved.block_height.unwrap_or(0.0));
    items.extend(visual_lines(
        item,
        marks,
        segments,
        inlay_hits,
        absolute_start,
        unit_end,
        final_unit,
        measure,
        fonts.clone(),
        theme,
        markup,
        inline,
        hidden,
    ));
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
    measure: crate::markup::InlayMeasure<'a>,
    fonts: FontCollection,
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
    let mut paragraph = paragraph(
        &resolved,
        text.as_str(),
        fonts,
        theme,
        &inline,
        text.placeholders(),
    );

    paragraph.layout((width - resolved.inset.unwrap_or(0.0) * 2.0).max(60.0));

    item.width = paragraph.longest_line() + resolved.inset.unwrap_or(0.0) * 2.0;
    let mut line_count = paragraph.line_number();

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
        paragraph,
        end_gap,
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
    paragraph: Paragraph,

    end_gap: f32,

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
        let height = line_height_from_metrics(&self.paragraph.get_line_metrics_at(line_index)?);
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
        let text_end = self
            .text
            .line_text_end(&self.paragraph, line_index, is_last);
        let byte_start = self.byte_start;
        let byte_end = self.text.line_byte_end(text_end, self.byte_start, is_last);
        let byte_size = byte_end.saturating_sub(self.byte_start);
        self.byte_start = byte_end;
        self.text_start = text_end;
        self.line_index += 1;

        let mut item = self.item.clone();
        item.byte_size = byte_size.min(u32::MAX as usize) as u32;
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

fn line_height_from_metrics(metrics: &LineMetrics<'_>) -> f32 {
    (metrics.ascent.abs() + metrics.descent).ceil().max(1.0) as f32
}

#[cfg(test)]
mod delete_accounting_tests {
    use super::*;

    fn element(byte_size: u32, height: f32) -> LayoutElement {
        LayoutElement {
            safepoint: true,
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
