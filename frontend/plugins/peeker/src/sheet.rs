// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The jump sheet's typographic pieces: the SHEET it stands on, the
//! row label whose typed letters are marked, and the key-hint
//! footer (the palette crate keeps the same set; both belong in
//! `hikit::ui`). Each is a named modifier or layout value — the views
//! compose them, nothing places pixels by hand.

use editor::theme::{SheetChrome, TypeRole};
use hikit::ui::TextStyle;
use imba::{
    arena::Arena,
    layout::{Insets, LayoutExt as _},
    ui::UiCtx,
};
use skia_safe::{Paint, Rect, Size};

/// THE one boxed surface the design keeps: a centered sheet — flat
/// fill, hairline border, soft radius, soft drop shadow. The page
/// behind dims nothing.
#[derive(Clone)]
pub(crate) struct Sheet(pub(crate) SheetChrome);

impl Sheet {
    /// The sheet's width in a window `room` wide: its own width, or
    /// the window less a gutter each side.
    pub(crate) fn width_in(&self, room: Size) -> f32 {
        self.0.width.min(room.width - 2.0 * self.0.gutter).max(1.0)
    }

    /// The tallest the sheet may grow in `room`.
    pub(crate) fn max_height_in(&self, room: Size) -> f32 {
        (room.height - self.0.top - self.0.bottom).max(1.0)
    }

    /// The sheet's rect for content `size`, centered under the top.
    pub(crate) fn rect_in(&self, room: Size, size: Size) -> Rect {
        Rect::from_xywh(
            ((room.width - size.width) * 0.5).max(0.0),
            self.0.top,
            size.width,
            size.height,
        )
    }

    /// The fill and border, under the content it decorates.
    pub(crate) fn wrap<'a, Command: 'a>(
        &self,
        inner: impl imba::thunk_ext::ThunkExt<'a, Command> + 'a,
    ) -> impl imba::Thunk<'a, Command> + 'a {
        hikit::ui::Surface::bordered(self.0.fill.0, self.0.border.0)
            .radius(self.0.radius)
            .wrap(inner)
    }

    /// The soft drop shadow, painted by whatever stands BEHIND the
    /// sheet — the sheet's own box clips what it paints.
    pub(crate) fn shadow_painter(&self, sheet: Rect) -> impl Fn(&Arena, &skia_safe::Canvas, Rect) {
        let chrome = self.0.clone();
        move |_arena, canvas, _rect| {
            if chrome.shadow_blur <= 0.0 {
                return;
            }
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(chrome.shadow.0);
            // A CSS blur radius is twice the gaussian's sigma.
            paint.set_mask_filter(skia_safe::MaskFilter::blur(
                skia_safe::BlurStyle::Normal,
                chrome.shadow_blur * 0.5,
                false,
            ));
            canvas.draw_round_rect(
                sheet.with_offset((0.0, chrome.shadow_dy)),
                chrome.radius,
                chrome.radius,
                &paint,
            );
        }
    }
}

/// A chrome style at a ramp rung, in `color`.
pub(crate) fn styled(ui: &UiCtx, role: &TypeRole, color: skia_safe::Color) -> TextStyle {
    TextStyle {
        font: hikit::fonts::role_font(ui, role),
        color,
        tracking: role.tracking,
        shaper: imba::layout::TextShaper::of(ui),
    }
}

/// The mono face at `size`, in `color` — keys and shortcuts.
pub(crate) fn mono(ui: &UiCtx, size: f32, color: skia_safe::Color) -> TextStyle {
    TextStyle {
        font: hikit::fonts::mono_font(ui, size),
        color,
        tracking: 0.0,
        shaper: imba::layout::TextShaper::of(ui),
    }
}

/// The typed letters' mark: a heavier cut of the label's face and a
/// LIVE underline under the run.
#[derive(Clone)]
pub(crate) struct MatchMark {
    pub(crate) style: TextStyle,
    pub(crate) underline: skia_safe::Color,
    pub(crate) thickness: f32,
    /// The underline's drop below the baseline.
    pub(crate) offset: f32,
}

impl MatchMark {
    /// `plain`'s role, re-cut at `weight` in `color`.
    pub(crate) fn of(
        ui: &UiCtx,
        role: &TypeRole,
        weight: f32,
        color: skia_safe::Color,
        underline: skia_safe::Color,
        thickness: f32,
        offset: f32,
    ) -> Self {
        Self {
            style: styled(ui, &TypeRole { weight, ..*role }, color),
            underline,
            thickness,
            offset,
        }
    }

    /// One marked run: the text, underlined at its baseline.
    fn run<'a, Command: 'a>(
        &self,
        content: &str,
    ) -> impl imba::layout::Layout<'a, Command> + imba::layout::LayoutValue + 'a {
        let metrics = self.style.font.metrics().1;
        let ascent = -metrics.ascent;
        let (color, thickness, offset) = (self.underline, self.thickness, self.offset);
        // The underline may hang below the text's own box, which
        // clips what it paints: the run grows a foot to hold it.
        let foot = (offset + thickness - metrics.descent).max(0.0).ceil();
        hikit::ui::text(&self.style, content)
            .pad_insets(Insets {
                bottom: foot,
                ..Default::default()
            })
            .backdrop(move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: Rect| {
            let mut paint = Paint::default();
            paint.set_color(color);
            canvas.draw_rect(
                Rect::from_xywh(rect.left, rect.top + ascent + offset, rect.width(), thickness),
                &paint,
            );
        })
    }
}

/// The byte ranges of `name` that a subsequence match of `query`
/// lands on, adjacent letters merged into one run — what the row
/// marks. Empty when the query does not match the name (it matched
/// something else, an id).
pub(crate) fn match_runs(name: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let mut wanted = query.chars().flat_map(char::to_lowercase).peekable();
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for (at, ch) in name.char_indices() {
        let Some(&next) = wanted.peek() else { break };
        if ch.to_lowercase().eq(std::iter::once(next)) {
            wanted.next();
            let end = at + ch.len_utf8();
            match runs.last_mut() {
                Some(run) if run.end == at => run.end = end,
                _ => runs.push(at..end),
            }
        }
    }
    match wanted.peek() {
        Some(_) => Vec::new(),
        None => runs,
    }
}

/// A label as runs on one baseline: the plain stretches in `plain`,
/// the matched ones marked.
pub(crate) fn marked_label<'a, Command: 'a>(
    arena: &'a Arena,
    name: &str,
    runs: &[std::ops::Range<usize>],
    plain: &TextStyle,
    mark: &MatchMark,
) -> imba::layout::Row<'a, Command> {
    let mut row = imba::layout::Row::new(arena);
    let mut at = 0;
    for run in runs {
        if run.start > at {
            row = row.child_by_baseline(hikit::ui::text(plain, &name[at..run.start]));
        }
        row = row.child_by_baseline(mark.run(&name[run.clone()]));
        at = run.end;
    }
    if at < name.len() || name.is_empty() {
        row = row.child_by_baseline(hikit::ui::text(plain, &name[at..]));
    }
    row
}

/// The footer's key hints — "↑↓ move   ⏎ run   esc close": each key
/// in the mono face, its word in the label's, the hints a fixed gap
/// apart on one baseline.
pub(crate) fn key_hints<'a, Command: 'a>(
    arena: &'a Arena,
    hints: &[(&str, &str)],
    key: &TextStyle,
    word: &TextStyle,
    gap: f32,
) -> imba::layout::Row<'a, Command> {
    let mut row = imba::layout::Row::new(arena);
    for (index, (keys, verb)) in hints.iter().enumerate() {
        let lead = match index {
            0 => 0.0,
            _ => gap,
        };
        row = row
            .child_by_baseline(hikit::ui::text(key, *keys).pad_insets(Insets {
                left: lead,
                ..Default::default()
            }))
            .child_by_baseline(hikit::ui::text(word, format!(" {verb}")));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::match_runs;

    #[test]
    fn runs_merge_adjacent_letters_and_fold_case() {
        assert_eq!(match_runs("Select All", "sel"), vec![0..3]);
        assert_eq!(match_runs("Select All", "sa"), vec![0..1, 7..8]);
        assert_eq!(match_runs("Select All", "xyz"), Vec::<std::ops::Range<usize>>::new());
        assert_eq!(match_runs("Select All", ""), Vec::<std::ops::Range<usize>>::new());
    }
}
