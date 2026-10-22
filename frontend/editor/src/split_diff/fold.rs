// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use operation::operation::Operation;
use text::text::Text;

pub(crate) const FOLDS_ENABLED: bool = true;

pub(crate) const FOLD_CONTEXT: u32 = 3;

pub(crate) const FOLD_MIN_LINES: u32 = 3;

pub(crate) const FOLD_STEP: u32 = 10;

#[derive(Clone, PartialEq, Debug)]
pub struct FoldSpec {
    pub left: Range<u32>,
    pub right: Range<u32>,

    pub lines: u32,
}

/// `of \ bans`, in order — the pieces of a derived fold the user has
/// not revealed (`Diff::fold_bans`, base coordinates).
pub(crate) fn subtract_bans(of: &Range<u32>, bans: &crate::diff::FoldBans) -> Vec<Range<u32>> {
    use intervals::{IntervalQuery, Order};
    let mut pieces = Vec::new();
    let mut at = of.start;
    for ban in bans.query(of.clone(), Order::Ascending) {
        if ban.range.end <= at {
            continue;
        }
        if ban.range.start >= of.end {
            break;
        }
        if ban.range.start > at {
            pieces.push(at..ban.range.start.min(of.end));
        }
        at = at.max(ban.range.end);
        if at >= of.end {
            break;
        }
    }
    if at < of.end {
        pieces.push(at..of.end);
    }
    pieces
}

pub(crate) fn derive_folds(
    diff: &Operation,
    left_text: &Text,
    region: Range<u32>,
    context: u32,
) -> Vec<FoldSpec> {
    let mut folds = Vec::new();

    if diff
        .iter()
        .all(|op| matches!(op, operation::op::Op::Retain(_)))
    {
        return folds;
    }
    let mut scan = LineScan::new(left_text);
    let left_len = scan.view.byte_count().min(u32::MAX as usize) as u32;
    let right_len = diff.transform_offset(left_len, operation::operation::Bias::Right);

    let mut at = region.start;
    while let Some(run) = diff.next_retained_old(at) {
        if run.start >= region.end {
            break;
        }
        at = run.end.max(run.start + 1);
        if run.end <= region.start {
            continue;
        }

        let right_start = diff.transform_offset(run.start, operation::operation::Bias::Right);

        let first_line = match scan.line_start_at_or_after(run.start, left_len) {
            Some(byte) if byte < run.end => byte,
            _ => continue,
        };
        let last_end = match scan.line_end_at_or_before(run.end, left_len) {
            Some(byte) if byte > first_line => byte,
            _ => continue,
        };

        let right_run_end = right_start + (run.end - run.start);
        let mut start = first_line;
        if run.start > 0 || right_start > 0 {
            for _ in 0..context {
                match scan.next_line_start(start, last_end) {
                    Some(next) => start = next,
                    None => break,
                }
            }
        }
        let mut end = last_end;
        if run.end < left_len || right_run_end < right_len {
            for _ in 0..context {
                match scan.previous_line_start(end) {
                    Some(previous) if previous > start => end = previous,
                    _ => break,
                }
            }
        }
        if end <= start {
            continue;
        }
        let lines = scan.count_lines(start, end);
        if lines < FOLD_MIN_LINES {
            continue;
        }
        let offset = start - run.start;
        let len = end - start;
        folds.push(FoldSpec {
            left: start..end,
            right: right_start + offset..right_start + offset + len,
            lines,
        });
    }
    folds
}

pub(crate) struct LineScan {
    view: text::text_view::TextView,
    scratch: Vec<u8>,
}

const SCAN_WINDOW: u32 = 4096;

impl LineScan {
    pub(crate) fn new(text: &Text) -> Self {
        Self {
            view: text.view(),
            scratch: Vec::with_capacity(SCAN_WINDOW as usize),
        }
    }

    pub(crate) fn next_newline(&mut self, from: u32, limit: u32) -> Option<u32> {
        let limit = limit.min(self.view.byte_count().min(u32::MAX as usize) as u32);
        let mut at = from;
        while at < limit {
            let end = at.saturating_add(SCAN_WINDOW).min(limit);
            self.scratch.clear();
            self.view
                .byte_range_into(at as usize, end as usize, &mut self.scratch);
            if let Some(hit) = self.scratch.iter().position(|byte| *byte == b'\n') {
                return Some(at + hit as u32);
            }
            at = end;
        }
        None
    }

    pub(crate) fn previous_newline(&mut self, before: u32) -> Option<u32> {
        let mut end = before;
        while end > 0 {
            let start = end.saturating_sub(SCAN_WINDOW);
            self.scratch.clear();
            self.view
                .byte_range_into(start as usize, end as usize, &mut self.scratch);
            if let Some(hit) = self.scratch.iter().rposition(|byte| *byte == b'\n') {
                return Some(start + hit as u32);
            }
            end = start;
        }
        None
    }

    pub(crate) fn line_start_at_or_after(&mut self, byte: u32, len: u32) -> Option<u32> {
        if byte == 0 {
            return Some(0);
        }
        self.scratch.clear();
        self.view
            .byte_range_into((byte - 1) as usize, byte as usize, &mut self.scratch);
        if self.scratch.first() == Some(&b'\n') {
            return Some(byte);
        }
        self.next_newline(byte, len).map(|newline| newline + 1)
    }

    pub(crate) fn next_line_start(&mut self, byte: u32, limit: u32) -> Option<u32> {
        let start = self.next_newline(byte, limit)? + 1;
        (start < limit).then_some(start)
    }

    pub(crate) fn line_end_at_or_before(&mut self, byte: u32, len: u32) -> Option<u32> {
        if byte >= len {
            return Some(len);
        }

        self.previous_newline(byte).map(|newline| newline + 1)
    }

    pub(crate) fn previous_line_start(&mut self, byte: u32) -> Option<u32> {
        if byte < 2 {
            return None;
        }

        match self.previous_newline(byte - 1) {
            Some(newline) => Some(newline + 1),
            None => Some(0),
        }
    }

    pub(crate) fn count_lines(&mut self, start: u32, end: u32) -> u32 {
        let mut lines = 0;
        let mut at = start;
        let mut trailing = false;
        while at < end {
            let window_end = at.saturating_add(SCAN_WINDOW).min(end);
            self.scratch.clear();
            self.view
                .byte_range_into(at as usize, window_end as usize, &mut self.scratch);
            lines += self.scratch.iter().filter(|byte| **byte == b'\n').count() as u32;
            trailing = self.scratch.last() != Some(&b'\n');
            at = window_end;
        }
        if trailing {
            lines += 1;
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(source: &str) -> Text {
        Text::from_string_exact(source)
    }

    #[test]
    fn folds_the_middle_of_a_retained_run_with_context() {
        let left_lines: Vec<String> = std::iter::once("LEFT ONLY".to_owned())
            .chain((0..12).map(|n| format!("same {n}")))
            .chain(std::iter::once("LEFT TAIL".to_owned()))
            .collect();
        let right_lines: Vec<String> = std::iter::once("RIGHT ONLY".to_owned())
            .chain((0..12).map(|n| format!("same {n}")))
            .chain(std::iter::once("RIGHT TAIL".to_owned()))
            .collect();
        let left = text(&(left_lines.join("\n") + "\n"));
        let right = text(&(right_lines.join("\n") + "\n"));
        let diff = myersdiff::diff(&left, &right);

        let len = left.view().byte_count() as u32;
        let folds = derive_folds(&diff, &left, 0..len, FOLD_CONTEXT);
        assert_eq!(folds.len(), 1, "one middle fold: {folds:?}");
        let fold = &folds[0];

        assert_eq!(fold.lines, 6, "{fold:?}");
        let left_view = left.view().substring(fold.left.clone());
        assert!(
            left_view.starts_with("same 3\n") && left_view.ends_with("same 8\n"),
            "context trimmed at both edges: {left_view:?}"
        );

        let right_view = right.view().substring(fold.right.clone());
        assert_eq!(left_view, right_view, "identical folded bytes");
    }

    #[test]
    fn document_edges_fold_to_the_boundary() {
        let base: Vec<String> = (0..8).map(|n| format!("same {n}")).collect();
        let left = text(&(base.join("\n") + "\nLEFT END\n"));
        let right = text(&(base.join("\n") + "\nRIGHT END\n"));
        let diff = myersdiff::diff(&left, &right);
        let len = left.view().byte_count() as u32;
        let folds = derive_folds(&diff, &left, 0..len, FOLD_CONTEXT);
        assert_eq!(folds.len(), 1, "{folds:?}");
        let fold = &folds[0];
        assert_eq!(fold.left.start, 0, "folds from the very top");
        assert_eq!(fold.lines, 5, "8 lines minus 3 bottom context: {fold:?}");
    }

    #[test]
    fn short_runs_yield_nothing() {
        let left = text("same a\nsame b\nLEFT\nsame c\nsame d\n");
        let right = text("same a\nsame b\nRIGHT\nsame c\nsame d\n");
        let diff = myersdiff::diff(&left, &right);
        let len = left.view().byte_count() as u32;
        assert!(derive_folds(&diff, &left, 0..len, FOLD_CONTEXT).is_empty());
    }

    use crate::diff::{rewrite_bans, FoldBans};

    fn ban_ranges(bans: &FoldBans) -> Vec<Range<u32>> {
        use intervals::{IntervalQuery, Order};
        bans.query(0..u32::MAX, Order::Ascending)
            .map(|interval| interval.range)
            .collect()
    }

    fn bans_of(ranges: &[Range<u32>]) -> FoldBans {
        let mut bans = FoldBans::new();
        for range in ranges {
            rewrite_bans(&mut bans, range.clone(), 0..0);
        }
        bans
    }

    #[test]
    fn a_ban_is_the_extent_minus_the_kept_fold() {
        let mut bans = FoldBans::new();
        // A full Remove bans the whole extent.
        rewrite_bans(&mut bans, 10..50, 0..0);
        assert_eq!(ban_ranges(&bans), vec![10..50]);
        // A Hide gives the middle back to the derivation.
        rewrite_bans(&mut bans, 10..50, 20..40);
        assert_eq!(ban_ranges(&bans), vec![10..20, 40..50]);
        // A ban outside the extent survives a rewrite within it.
        rewrite_bans(&mut bans, 90..100, 0..0);
        rewrite_bans(&mut bans, 10..50, 15..50);
        assert_eq!(ban_ranges(&bans), vec![10..15, 90..100]);
        // A full Hide un-bans the extent entirely.
        rewrite_bans(&mut bans, 10..50, 10..50);
        assert_eq!(ban_ranges(&bans), vec![90..100]);
        // Touching pieces merge into one canonical range.
        rewrite_bans(&mut bans, 80..90, 0..0);
        assert_eq!(ban_ranges(&bans), vec![80..100]);
    }

    #[test]
    fn subtraction_keeps_the_unrevealed_pieces_in_order() {
        assert_eq!(subtract_bans(&(0..100), &bans_of(&[])), vec![0..100]);
        assert_eq!(
            subtract_bans(&(0..100), &bans_of(&[40..60])),
            vec![0..40, 60..100]
        );
        assert_eq!(
            subtract_bans(&(0..100), &bans_of(&[0..100])),
            Vec::<Range<u32>>::new()
        );
        assert_eq!(
            subtract_bans(&(20..80), &bans_of(&[0..30, 50..60, 90..95])),
            vec![30..50, 60..80]
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FoldCommand {
    RevealTop,

    HideTop,

    RevealBottom,

    HideBottom,

    Remove,
}

impl std::fmt::Display for FoldCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FoldCommand::RevealTop => out.write_str("fold reveal top"),
            FoldCommand::HideTop => out.write_str("fold hide top"),
            FoldCommand::RevealBottom => out.write_str("fold reveal bottom"),
            FoldCommand::HideBottom => out.write_str("fold hide bottom"),
            FoldCommand::Remove => out.write_str("fold remove"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct FoldStrip {
    pub lines: u32,

    /// The split's LEFT pane mounts the fold as a silent spacer: it
    /// reserves the aligned height, while the right pane's strip —
    /// projected onto the pair-wide overlay host — is the one shared,
    /// interactive face.
    silent: bool,
}

impl FoldStrip {
    pub(crate) fn new(lines: u32) -> Self {
        Self {
            lines,
            silent: false,
        }
    }

    pub(crate) fn spacer(lines: u32) -> Self {
        Self {
            lines,
            silent: true,
        }
    }
}


impl imba::View for FoldStrip {
    type Command = FoldCommand;

    fn perform(
        &mut self,
        _store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        _command: FoldCommand,
        _fx: &mut imba::effect::Effects<'_, FoldCommand>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        store: &'a imba::store::Store,
        ui: &'a imba::ui::UiCtx,
    ) -> impl imba::layout::Layout<'a, FoldCommand> + imba::layout::LayoutValue + 'a {
        FoldStripLayout {
            chrome: crate::env::Themes::of(store).ui().diff.clone(),
            lines: self.lines,
            silent: self.silent,
            shaper: imba::layout::TextShaper::of(ui),
        }
    }
}

/// The strip, REIFIED (docs/ui/UI.md stage 2): the label `Text` padded
/// down to the chrome's computed baseline, a weighted `Fill` gap, and
/// the five right-anchored glyph buttons — each a `fixed` painted
/// leaf whose click zone is its own rect. A layout STRUCT because the
/// strip's extent is the incoming width (the old leaf's
/// `constraints.max.width.max(1.0)`) at the chrome's fold height.
struct FoldStripLayout {
    chrome: crate::theme::DiffChrome,
    lines: u32,
    silent: bool,
    shaper: std::rc::Rc<imba::layout::TextShaper>,
}

impl imba::layout::LayoutValue for FoldStripLayout {}

impl<'a> imba::layout::Layout<'a, FoldCommand> for FoldStripLayout {
    fn layout(
        self,
        arena: &'a imba::arena::Arena,
        constraints: imba::constraints::Constraints,
    ) -> imba::ThunkBox<'a, FoldCommand> {
        let width = constraints.max.width.max(1.0);
        let (chip, gap) = chip_metrics(&self.chrome);
        let height = strip_height(&self.chrome);
        if self.silent {
            // The spacer face: same reserved height, nothing painted,
            // nothing answered — the shared strip renders elsewhere.
            return imba::ThunkBox::new(arena, imba::leaf::leaf::<FoldCommand>(width, height));
        }
        imba::ThunkBox::new(
            arena,
            imba::eager(FoldFace {
                chrome: self.chrome,
                lines: self.lines,
                shaper: self.shaper,
                width,
                height,
                chip,
                gap,
            }),
        )
    }
}

fn chip_metrics(chrome: &crate::theme::DiffChrome) -> (f32, f32) {
    ((chrome.fold_text_size * 1.5).ceil(), 6.0)
}

/// The strip's whole footprint — the silent spacer and the painted
/// face MUST agree, or the split's sides drift apart at every fold.
fn strip_height(chrome: &crate::theme::DiffChrome) -> f32 {
    let (chip, gap) = chip_metrics(chrome);
    chip * 3.0 + gap * 2.0 + 12.0
}

/// The fold's face (docs/himark-islands-canvas-many-files.png): a
/// hairline through the middle, a chip stack at the left — `+N
/// lines` above, `unfold` riding the line, `+N lines` below — and
/// the folded count dimmed at the right edge.
struct FoldFace {
    chrome: crate::theme::DiffChrome,
    lines: u32,
    shaper: std::rc::Rc<imba::layout::TextShaper>,
    width: f32,
    height: f32,
    chip: f32,
    gap: f32,
}

impl FoldFace {
    /// The three chips, top to bottom, with their rects and labels.
    fn chips(&self) -> [(FoldCommand, skia_safe::Rect, String); 3] {
        let font = strip_font(self.chrome.fold_text_size);
        let inset = self.chrome.fold_text_size;
        let pad = self.chrome.fold_text_size * 0.7;
        let reveal = format!("+ {} lines", FOLD_STEP.min(self.lines));
        let reveal_w = self.shaper.advance(&font, &reveal) + pad * 2.0;
        // The unfold chip leads with its glyph.
        let unfold_w =
            self.shaper.advance(&font, "unfold") + pad * 2.0 + self.chrome.fold_text_size * 0.9;
        let mid = self.height * 0.5;
        [
            (
                FoldCommand::RevealTop,
                skia_safe::Rect::from_xywh(
                    inset,
                    mid - self.chip * 1.5 - self.gap,
                    reveal_w,
                    self.chip,
                ),
                reveal.clone(),
            ),
            (
                FoldCommand::Remove,
                skia_safe::Rect::from_xywh(inset, mid - self.chip * 0.5, unfold_w, self.chip),
                "unfold".to_owned(),
            ),
            (
                FoldCommand::RevealBottom,
                skia_safe::Rect::from_xywh(inset, mid + self.chip * 0.5 + self.gap, reveal_w, self.chip),
                reveal,
            ),
        ]
    }
}

impl<'a> imba::Widget<'a, FoldCommand> for FoldFace {
    fn size(&self) -> skia_safe::Size {
        skia_safe::Size::new(self.width, self.height)
    }

    fn handle_event(
        &self,
        _arena: &imba::arena::Arena,
        event: &imba::event::Event<'_>,
        _viewport: skia_safe::Rect,
    ) -> imba::event::EventResult<FoldCommand> {
        use imba::event::{Event, EventResult, MouseButton};
        match event {
            Event::Paint { canvas, .. } => {
                let chrome = &self.chrome;
                let font = strip_font(chrome.fold_text_size);
                let (ascent, descent) = {
                    let metrics = font.metrics().1;
                    (-metrics.ascent, metrics.descent)
                };
                let mut paint = skia_safe::Paint::default();
                paint.set_anti_alias(true);
                let mid = self.height * 0.5;
                let chips = self.chips();

                // The folded count, dimmed at the right edge, riding
                // the line's level.
                let label = format!("{} lines folded", self.lines);
                let label_w = self.shaper.advance(&font, &label);
                let label_x = self.width - chrome.fold_text_size - label_w;
                self.shaper.draw(
                    canvas,
                    &font,
                    &label,
                    chrome.fold_text.0,
                    0.0,
                    label_x,
                    mid + (ascent - descent) * 0.5,
                );

                // The hairline, from past the chips to short of the
                // count.
                let chips_right = chips
                    .iter()
                    .map(|(_, rect, _)| rect.right)
                    .fold(0.0_f32, f32::max);
                paint.set_color(chrome.fold_rule.0);
                paint.set_stroke_width(1.0);
                paint.set_style(skia_safe::paint::Style::Stroke);
                canvas.draw_line(
                    (chips_right + chrome.fold_text_size, mid),
                    (label_x - chrome.fold_text_size, mid),
                    &paint,
                );

                // The chips: filled, hairline-framed, labels centered.
                for (command, rect, text) in &chips {
                    paint.set_style(skia_safe::paint::Style::Fill);
                    paint.set_color(chrome.fold_background.0);
                    canvas.draw_round_rect(*rect, 6.0, 6.0, &paint);
                    paint.set_style(skia_safe::paint::Style::Stroke);
                    paint.set_color(chrome.fold_rule.0);
                    canvas.draw_round_rect(*rect, 6.0, 6.0, &paint);

                    let mut text_x = rect.left + chrome.fold_text_size * 0.7;
                    if *command == FoldCommand::Remove {
                        // The unfold glyph: two chevrons parting.
                        let cx = text_x + chrome.fold_text_size * 0.3;
                        let cy = rect.center_y();
                        let arm = chrome.fold_text_size * 0.26;
                        paint.set_color(chrome.fold_button.0);
                        paint.set_stroke_width(1.6);
                        paint.set_stroke_cap(skia_safe::paint::Cap::Round);
                        let mut path = skia_safe::PathBuilder::new();
                        path.move_to((cx - arm, cy - arm * 0.5));
                        path.line_to((cx, cy - arm * 1.3));
                        path.line_to((cx + arm, cy - arm * 0.5));
                        path.move_to((cx - arm, cy + arm * 0.5));
                        path.line_to((cx, cy + arm * 1.3));
                        path.line_to((cx + arm, cy + arm * 0.5));
                        canvas.draw_path(&path.detach(), &paint);
                        text_x = cx + arm + chrome.fold_text_size * 0.45;
                    }
                    paint.set_style(skia_safe::paint::Style::Fill);
                    self.shaper.draw(
                        canvas,
                        &font,
                        text,
                        chrome.fold_text.0,
                        0.0,
                        text_x,
                        rect.center_y() + (ascent - descent) * 0.5,
                    );
                }
                EventResult::handled()
            }
            Event::MouseDown {
                button: MouseButton::Left,
                point,
                ..
            } => {
                for (command, rect, _) in self.chips() {
                    if point.x >= rect.left
                        && point.x < rect.right
                        && point.y >= rect.top
                        && point.y < rect.bottom
                    {
                        return EventResult::command(command);
                    }
                }
                // A press anywhere else on the strip is swallowed so
                // it never reaches the document underneath.
                EventResult::handled()
            }
            _ => imba::event::EventResult::Ignored,
        }
    }
}

pub(crate) fn strip_font(size: f32) -> skia_safe::Font {
    static TYPEFACE: std::sync::OnceLock<skia_safe::Typeface> = std::sync::OnceLock::new();
    let typeface = TYPEFACE.get_or_init(|| {
        skia_safe::FontMgr::new()
            .legacy_make_typeface(None, skia_safe::FontStyle::bold())
            .expect("a system typeface")
    });
    let mut font = skia_safe::Font::from_typeface(typeface.clone(), size);
    font.set_edging(skia_safe::font::Edging::AntiAlias);
    font
}



/// TEST SUPPORT: the fold strips a marks markup carries — the right
/// side's interactive strips (silent spacers are the left side's).
#[doc(hidden)]
pub fn strip_ranges(
    markup: &crate::markup::Markup,
    marks: crate::markup::MarkupId,
) -> Vec<Range<u32>> {
    markup
        .inlays_in(0..u32::MAX, crate::markup::MarkupLayer::Markup(marks))
        .filter(|inlay| inlay.inlay.view_as::<FoldStrip>().is_some())
        .map(|inlay| inlay.range.clone())
        .collect()
}
