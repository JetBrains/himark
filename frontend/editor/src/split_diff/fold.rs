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

/// Where a fold's rows stand: the first folded line (1-based) and
/// the last, or `None` when the fold reaches the file's end — a rope
/// seek per edge, never a scan.
pub(crate) fn fold_place(text: &Text, start: u32, end: u32, lines: u32) -> FoldPlace {
    let mut view = text.view();
    let first_line = view.line_at(start as usize).0 as u32 + 1;
    let len = text.byte_count().min(u32::MAX as usize) as u32;
    let last_line = (end < len).then(|| first_line + lines.saturating_sub(1));
    FoldPlace {
        first_line,
        last_line,
    }
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

/// Where a fold's rows stand in the file, for the band's two edges:
/// the first folded line, and the last — or `None` when the fold
/// runs to the end of the file ("end").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct FoldPlace {
    pub first_line: u32,
    pub last_line: Option<u32>,
}

#[derive(Clone)]
pub(crate) struct FoldStrip {
    pub lines: u32,

    pub place: FoldPlace,

    /// The split's LEFT pane mounts the fold as a silent spacer: it
    /// reserves the aligned height, while the right pane's strip —
    /// projected onto the pair-wide overlay host — is the one shared,
    /// interactive face.
    silent: bool,
}

impl FoldStrip {
    pub(crate) fn new(lines: u32, place: FoldPlace) -> Self {
        Self {
            lines,
            place,
            silent: false,
        }
    }

    pub(crate) fn spacer(lines: u32) -> Self {
        Self {
            lines,
            place: FoldPlace {
                first_line: 0,
                last_line: None,
            },
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
        let theme = crate::env::Themes::of(store);
        FoldStripLayout {
            chrome: theme.ui().diff.clone(),
            gutter: theme.ui().editor_gutter.clone(),
            ink: theme.ui().ink.clone(),
            lines: self.lines,
            place: self.place,
            silent: self.silent,
            ui,
        }
    }
}

/// The fold BAND (docs/Design.md): a fold has two EDGES, each
/// carrying the line number it opens from and a chevron pointing
/// into the fold — `⌄` on the top edge reveals the fold's first
/// lines, `⌃` on the bottom edge its last — and a summary at the
/// code's text edge ("598 lines folded", "rest of file folded") with
/// "Show all" after it. Reified as a layout struct because the band
/// spans the incoming width at a height the silent spacer must agree
/// with.
struct FoldStripLayout<'a> {
    chrome: crate::theme::DiffChrome,
    gutter: crate::theme::EditorGutterChrome,
    ink: crate::theme::InkChrome,
    lines: u32,
    place: FoldPlace,
    silent: bool,
    ui: &'a imba::ui::UiCtx,
}

impl imba::layout::LayoutValue for FoldStripLayout<'_> {}

impl<'a> imba::layout::Layout<'a, FoldCommand> for FoldStripLayout<'a> {
    fn layout(
        self,
        arena: &'a imba::arena::Arena,
        constraints: imba::constraints::Constraints,
    ) -> imba::ThunkBox<'a, FoldCommand> {
        use imba::layout::{Alignment, Column, Fill, Insets, LayoutExt as _, Row, ZBox};
        let width = constraints.max.width.max(1.0);
        let height = strip_height(&self.chrome);
        if self.silent {
            // The spacer face: same reserved height, nothing painted,
            // nothing answered — the shared strip renders elsewhere.
            return imba::ThunkBox::new(arena, imba::leaf::leaf::<FoldCommand>(width, height));
        }
        let FoldStripLayout {
            chrome,
            gutter,
            ink,
            lines,
            place,
            ui,
            ..
        } = self;
        let edge_h = chrome.fold_edge_height;
        let number_font = crate::editor_view::gutter_font(gutter.number_size);
        let number_ink = ink.mono_dim.0;
        let gutter_right = gutter.width - chrome.fold_gutter_inset;

        // One EDGE: the number right-aligned in the gutter, the
        // chevron after it, the pair ending at the gutter's inset.
        let edge = |number: String, down: bool, command: FoldCommand| {
            let chevron = Chevron {
                size: chrome.fold_chevron_size,
                hit: chrome.fold_chevron_hit,
                stroke: chrome.fold_chevron_stroke,
                color: chrome.fold_button.0,
                down,
            };
            Row::new(arena)
                .align_items(imba::layout::CrossAlign::Center)
                .weighted(1.0, Fill::new().height(0.0))
                .child(imba::layout::text(ui, number, number_font.clone(), number_ink))
                .child(chevron.pad_insets(Insets {
                    left: chrome.fold_number_gap,
                    ..Default::default()
                }))
                .sized(gutter_right.max(1.0), edge_h)
                .on_click(move || command)
        };
        let last = match place.last_line {
            Some(line) => line.to_string(),
            None => "end".to_owned(),
        };
        let edges = Column::new(arena)
            .child(edge(place.first_line.to_string(), true, FoldCommand::RevealTop))
            .child(edge(last, false, FoldCommand::RevealBottom));

        // The summary at the text edge, riding the band's middle.
        let summary_font = chrome_font(chrome.fold_text_size);
        let summary = match place.last_line {
            Some(_) => format!("{lines} lines folded"),
            None => "rest of file folded".to_owned(),
        };
        let words = Row::new(arena)
            .child_by_baseline(imba::layout::text(
                ui,
                summary,
                summary_font.clone(),
                chrome.fold_text.0,
            ))
            .child_by_baseline(
                imba::layout::text(ui, "Show all", summary_font, chrome.fold_button.0)
                    .on_click(|| FoldCommand::Remove)
                    .pad_insets(Insets {
                        left: chrome.fold_summary_gap,
                        ..Default::default()
                    }),
            );

        let background = chrome.fold_background.0;
        let band = ZBox::new(arena)
            .child(edges)
            .child_aligned(
                Alignment::CenterStart,
                words.pad_insets(Insets {
                    left: gutter.width,
                    ..Default::default()
                }),
            )
            .sized(width, height)
            .backdrop(move |_arena: &imba::arena::Arena, canvas: &skia_safe::Canvas, rect| {
                let mut paint = skia_safe::Paint::default();
                paint.set_color(background);
                canvas.draw_rect(rect, &paint);
            })
            // A press anywhere else on the band is swallowed so it
            // never reaches the document underneath.
            .shield();
        band.layout(
            arena,
            imba::constraints::Constraints::tight(skia_safe::Size::new(width, height)),
        )
    }
}

/// The strip's whole footprint — the silent spacer and the painted
/// face MUST agree, or the split's sides drift apart at every fold.
fn strip_height(chrome: &crate::theme::DiffChrome) -> f32 {
    chrome.fold_edge_height * 2.0
}

/// The edge's glyph: a chevron pointing INTO the fold, in a square
/// hit box — the one bespoke mark on the band.
struct Chevron {
    size: f32,
    hit: f32,
    stroke: f32,
    color: skia_safe::Color,
    down: bool,
}

impl imba::layout::LayoutValue for Chevron {}

impl<'a, Command: 'a> imba::layout::Layout<'a, Command> for Chevron {
    fn layout(
        self,
        arena: &'a imba::arena::Arena,
        _constraints: imba::constraints::Constraints,
    ) -> imba::ThunkBox<'a, Command> {
        use imba::thunk_ext::ThunkExt as _;
        let Chevron {
            size,
            hit,
            stroke,
            color,
            down,
        } = self;
        imba::ThunkBox::new(
            arena,
            imba::leaf::leaf::<Command>(hit, hit).paint_instead(move |_arena, canvas, rect| {
                let mut paint = skia_safe::Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(color);
                paint.set_style(skia_safe::paint::Style::Stroke);
                paint.set_stroke_width(stroke);
                paint.set_stroke_cap(skia_safe::paint::Cap::Round);
                paint.set_stroke_join(skia_safe::paint::Join::Round);
                let cx = rect.center_x();
                let cy = rect.center_y();
                let arm = size * 0.31;
                let rise = size * 0.16;
                let (near, far) = match down {
                    true => (cy - rise, cy + rise),
                    false => (cy + rise, cy - rise),
                };
                let mut path = skia_safe::PathBuilder::new();
                path.move_to((cx - arm, near));
                path.line_to((cx, far));
                path.line_to((cx + arm, near));
                canvas.draw_path(&path.detach(), &paint);
            }),
        )
    }
}

/// The band's words wear the chrome face, like every label outside
/// the code.
fn chrome_font(size: f32) -> skia_safe::Font {
    let mut font = skia_safe::Font::from_typeface(crate::embedded_fonts::chrome_typeface(), size);
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
