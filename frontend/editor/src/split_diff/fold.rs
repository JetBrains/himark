use std::ops::Range;

use operation::Operation;
use text::Text;

pub(crate) const FOLDS_ENABLED: bool = true;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FoldPhase {
    Waiting,

    Owed,

    Done,
}

pub(crate) const FOLD_CONTEXT: u32 = 3;

pub(crate) const FOLD_MIN_LINES: u32 = 3;

pub(crate) const FOLD_STEP: u32 = 5;

#[derive(Clone, PartialEq, Debug)]
pub struct FoldSpec {
    pub left: Range<u32>,
    pub right: Range<u32>,

    pub lines: u32,
}

pub(crate) fn derive_folds(
    diff: &Operation,
    left_text: &Text,
    region: Range<u32>,
    context: u32,
) -> Vec<FoldSpec> {
    let mut folds = Vec::new();

    if diff.iter().all(|op| matches!(op, operation::Op::Retain(_))) {
        return folds;
    }
    let mut scan = LineScan::new(left_text);
    let left_len = scan.view.byte_count().min(u32::MAX as usize) as u32;
    let right_len = diff.transform_offset(left_len, operation::Bias::Right);

    let mut at = region.start;
    while let Some(run) = diff.next_retained_old(at) {
        if run.start >= region.end {
            break;
        }
        at = run.end.max(run.start + 1);
        if run.end <= region.start {
            continue;
        }

        let right_start = diff.transform_offset(run.start, operation::Bias::Right);

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
    view: text::TextView,
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
        let diff = crate::diff::diff(&left, &right);

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
        let diff = crate::diff::diff(&left, &right);
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
        let diff = crate::diff::diff(&left, &right);
        let len = left.view().byte_count() as u32;
        assert!(derive_folds(&diff, &left, 0..len, FOLD_CONTEXT).is_empty());
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

#[derive(Clone)]
pub struct FoldStrip {
    pub lines: u32,
}

impl FoldStrip {
    pub(crate) fn new(lines: u32) -> Self {
        Self { lines }
    }

    fn buttons(chrome: &crate::theme::DiffChrome, width: f32) -> [skia_safe::Rect; 5] {
        let size = chrome.fold_button_size;
        let gap = size * 0.35;
        let top = (chrome.fold_height - size) * 0.5;
        let mut right = width - gap;
        core::array::from_fn(|index| {
            let _ = index;
            let rect = skia_safe::Rect::from_xywh(right - size, top, size, size);
            right -= size + gap;
            rect
        })
    }
}

const FOLD_BUTTONS: [FoldCommand; 5] = [
    FoldCommand::Remove,
    FoldCommand::HideBottom,
    FoldCommand::RevealBottom,
    FoldCommand::HideTop,
    FoldCommand::RevealTop,
];

impl imba::View for FoldStrip {
    type Command = FoldCommand;

    fn perform(
        &mut self,
        _store: &mut imba::store::Store,
        _ui: &imba::UiCtx,
        _command: FoldCommand,
        _fx: &mut imba::effect::Effects<'_, FoldCommand>,
    ) {
    }

    fn layout<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a imba::store::Store,
        _ui: &'a imba::UiCtx,
        constraints: imba::constraints::Constraints,
    ) -> impl imba::Thunk<'a, FoldCommand> + 'a {
        use imba::event::{Event, EventResult, MouseButton};
        use imba::thunk_ext::ThunkExt;
        let chrome = crate::env::Themes::of(store).ui().diff.clone();
        let width = constraints.max.width.max(1.0);
        let height = chrome.fold_height;
        let lines = self.lines;
        let buttons = Self::buttons(&chrome, width);
        let font = crate::split_diff::fold::strip_font(chrome.fold_text_size);
        let strip = imba::leaf::leaf(width, height)
            .paint_instead(move |_arena, canvas, rect| {
                paint_strip(canvas, rect, &chrome, lines, &buttons, &font);
            })
            .event(move |_arena, event, _size| match event {
                Event::MouseDown {
                    button: MouseButton::Left,
                    point,
                    ..
                } => {
                    for (rect, command) in buttons.iter().zip(FOLD_BUTTONS) {
                        if point.x >= rect.left
                            && point.x < rect.right
                            && point.y >= rect.top
                            && point.y < rect.bottom
                        {
                            return EventResult::Command(command);
                        }
                    }
                    EventResult::Handled
                }
                _ => EventResult::Ignored,
            });
        let _ = arena;
        strip
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

fn paint_strip(
    canvas: &skia_safe::Canvas,
    rect: skia_safe::Rect,
    chrome: &crate::theme::DiffChrome,
    lines: u32,
    buttons: &[skia_safe::Rect; 5],
    font: &skia_safe::Font,
) {
    use skia_safe::{Paint, PathBuilder, Point};
    canvas.save();
    canvas.translate((rect.left, rect.top));

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(chrome.fold_background.0);
    canvas.draw_rect(
        skia_safe::Rect::from_wh(rect.width(), rect.height()),
        &paint,
    );
    paint.set_color(chrome.fold_rule.0);
    canvas.draw_rect(
        skia_safe::Rect::from_xywh(0.0, 0.0, rect.width(), 1.0),
        &paint,
    );
    canvas.draw_rect(
        skia_safe::Rect::from_xywh(0.0, rect.height() - 1.0, rect.width(), 1.0),
        &paint,
    );

    paint.set_color(chrome.fold_text.0);
    let label = format!("⋯ {lines} unchanged lines");
    let baseline = (rect.height() + chrome.fold_text_size * 0.7) * 0.5;
    canvas.draw_str(
        &label,
        (chrome.fold_button_size * 0.5, baseline),
        font,
        &paint,
    );

    paint.set_color(chrome.fold_button.0);
    paint.set_style(skia_safe::paint::Style::Stroke);
    paint.set_stroke_width(2.0);
    for (index, button) in buttons.iter().enumerate() {
        let cx = button.center_x();
        let cy = button.center_y();
        let arm = button.width() * 0.22;
        let mut path = PathBuilder::new();
        match FOLD_BUTTONS[index] {
            FoldCommand::Remove => {
                path.move_to(Point::new(cx - arm, cy - arm));
                path.line_to(Point::new(cx + arm, cy + arm));
                path.move_to(Point::new(cx - arm, cy + arm));
                path.line_to(Point::new(cx + arm, cy - arm));
            }
            FoldCommand::RevealTop | FoldCommand::HideBottom => {
                path.move_to(Point::new(cx - arm, cy + arm * 0.6));
                path.line_to(Point::new(cx, cy - arm * 0.8));
                path.line_to(Point::new(cx + arm, cy + arm * 0.6));
            }
            FoldCommand::HideTop | FoldCommand::RevealBottom => {
                path.move_to(Point::new(cx - arm, cy - arm * 0.6));
                path.line_to(Point::new(cx, cy + arm * 0.8));
                path.line_to(Point::new(cx + arm, cy - arm * 0.6));
            }
        }
        canvas.draw_path(&path.detach(), &paint);

        let bar_y = match FOLD_BUTTONS[index] {
            FoldCommand::RevealTop | FoldCommand::HideTop => Some(button.top + 2.0),
            FoldCommand::RevealBottom | FoldCommand::HideBottom => Some(button.bottom - 2.0),
            FoldCommand::Remove => None,
        };
        if let Some(bar_y) = bar_y {
            let mut bar = PathBuilder::new();
            bar.move_to(Point::new(cx - arm, bar_y));
            bar.line_to(Point::new(cx + arm, bar_y));
            canvas.draw_path(&bar.detach(), &paint);
        }
    }
    canvas.restore();
}
