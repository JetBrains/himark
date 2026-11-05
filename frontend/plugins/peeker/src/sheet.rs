// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The jump sheet's typographic pieces. The SHEET itself, the text
//! styles and the key-hint footer moved to `hikit::sheet` (shared
//! with the folder picker and the palette); what stays is the row
//! label whose typed letters are marked — the peeker's own.

use editor::theme::TypeRole;
use gfx::{Paint, Rect};
use hikit::ui::TextStyle;
use imba::{
    arena::Arena,
    layout::{Insets, LayoutExt as _},
    ui::UiCtx,
};

pub(crate) use hikit::sheet::{key_hints, mono, styled, Sheet};

/// The typed letters' mark: a heavier cut of the label's face and a
/// LIVE underline under the run.
#[derive(Clone)]
pub(crate) struct MatchMark {
    pub(crate) style: TextStyle,
    pub(crate) underline: gfx::Color,
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
        color: gfx::Color,
        underline: gfx::Color,
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
            .backdrop(
                move |_arena: &Arena, canvas: &gfx::Canvas<'_>, rect: Rect| {
                    let mut paint = Paint::default();
                    paint.set_color(color);
                    canvas.draw_rect(
                        Rect::from_xywh(
                            rect.left,
                            rect.top + ascent + offset,
                            rect.width(),
                            thickness,
                        ),
                        &paint,
                    );
                },
            )
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

#[cfg(test)]
mod tests {
    use super::match_runs;

    #[test]
    fn runs_merge_adjacent_letters_and_fold_case() {
        assert_eq!(match_runs("Select All", "sel"), vec![0..3]);
        assert_eq!(match_runs("Select All", "sa"), vec![0..1, 7..8]);
        assert_eq!(
            match_runs("Select All", "xyz"),
            Vec::<std::ops::Range<usize>>::new()
        );
        assert_eq!(
            match_runs("Select All", ""),
            Vec::<std::ops::Range<usize>>::new()
        );
    }
}
