// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The canvas SUMMARY line: the working copy's byline — the `+A −R`
//! totals, the file count, and the expansion state, words in the
//! brief rung at the code text edge, no capsule. A press on the state
//! toggles every file: collapse all while anything stands open,
//! expand all after. The canvas computes the numbers and the state;
//! the line only lays them out.

use imba::{
    arena::Arena,
    constraints::Constraints,
    layout::{Insets, Layout, LayoutExt as _, Row},
    store::Store,
    ui::UiCtx,
};

use editor::env;

/// What the canvas asks the line to show.
pub struct SummarySpec {
    pub totals: crate::canvas::Totals,
    /// Of the files, how many stand collapsed.
    pub collapsed: usize,
}

pub struct SummaryLine {
    brief: hikit::ui::TextStyle,
    added_color: skia_safe::Color,
    removed_color: skia_safe::Color,
    state_color: skia_safe::Color,
    gap: f32,
    pad: f32,
    lead: f32,

    added: Option<String>,
    removed: Option<String>,
    files: String,
    state: String,
}

/// The strip the line takes above the canvas's first row: its inset
/// above and below one brief line — the line as the brief FONT
/// stands it, the same box `SummaryLine::strip_height` answers. The
/// canvas pads its list down by this much; the click tests mirror it.
pub fn strip_height(store: &Store, ui: &UiCtx) -> f32 {
    let pad = env::Themes::of(store)
        .part(::hikit::chrome::DiffChrome::KEY)
        .summary_pad;
    pad * 2.0 + hikit::ui::brief(store, ui).line_height()
}

impl SummaryLine {
    pub fn new(store: &Store, ui: &UiCtx, spec: SummarySpec) -> Self {
        let theme = env::Themes::of(store);
        let ink = theme.part(::editor::theme::InkChrome::KEY);
        let diff = theme.part(::hikit::chrome::DiffChrome::KEY);
        let totals = spec.totals;
        let files = match totals.files {
            1 => "1 file".to_owned(),
            n => format!("{n} files"),
        };
        let state = match (
            spec.collapsed,
            totals.files - spec.collapsed.min(totals.files),
        ) {
            (0, _) => "All expanded".to_owned(),
            (_, 0) => "All collapsed".to_owned(),
            (_, open) => format!("{open} expanded"),
        };
        Self {
            brief: hikit::ui::brief(store, ui),
            added_color: ink.added.0,
            removed_color: ink.removed.0,
            state_color: ink.ink_2.0,
            gap: diff.summary_gap,
            pad: diff.summary_pad,
            lead: theme
                .part(::editor::theme::WindowChrome::KEY)
                .code_text_edge,
            added: (totals.added > 0).then(|| format!("+{}", totals.added)),
            removed: (totals.removed > 0).then(|| format!("−{}", totals.removed)),
            files,
            state,
        }
    }

    /// The strip's height — see `strip_height`.
    pub fn strip_height(&self) -> f32 {
        self.pad * 2.0 + self.brief.line_height()
    }

    /// Where the line starts: the code text edge.
    pub fn lead(&self) -> f32 {
        self.lead
    }

    /// The line as a layout, the state word pressing `on_press`.
    pub fn on_press<'a, Command: 'a>(
        self,
        on_press: impl Fn() -> Command + 'a,
    ) -> SummaryLayout<impl Fn() -> Command + 'a> {
        SummaryLayout {
            line: self,
            on_press,
        }
    }
}

pub struct SummaryLayout<F> {
    line: SummaryLine,
    on_press: F,
}

impl<F> imba::layout::LayoutValue for SummaryLayout<F> {}

impl<'a, Command: 'a, F: Fn() -> Command + 'a> Layout<'a, Command> for SummaryLayout<F> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, Command> {
        let SummaryLayout { line, on_press } = self;
        use hikit::ui::text;
        let brief = &line.brief;
        // `+A −R` one word space apart, then the dotted words.
        let space = brief.shaper.advance(&brief.font, " ");
        let mut row = Row::new(arena);
        let mut first = true;
        let pad = |gap: f32, first: &mut bool| {
            let left = if *first { 0.0 } else { gap };
            *first = false;
            Insets {
                left,
                ..Default::default()
            }
        };
        if let Some(added) = &line.added {
            let insets = pad(space, &mut first);
            row = row.child_by_baseline(
                text(&brief.clone().colored(line.added_color), added).pad_insets(insets),
            );
        }
        if let Some(removed) = &line.removed {
            let insets = pad(space, &mut first);
            row = row.child_by_baseline(
                text(&brief.clone().colored(line.removed_color), removed).pad_insets(insets),
            );
        }
        if !first {
            let insets = pad(line.gap, &mut first);
            row = row.child_by_baseline(text(brief, "·").pad_insets(insets));
        }
        let insets = pad(line.gap, &mut first);
        row = row.child_by_baseline(text(brief, line.files.clone()).pad_insets(insets));
        let insets = pad(line.gap, &mut first);
        row = row.child_by_baseline(text(brief, "·").pad_insets(insets));
        let insets = pad(line.gap, &mut first);
        row = row.child_by_baseline(
            text(&brief.clone().colored(line.state_color), line.state.clone())
                .on_click(move || on_press())
                .pad_insets(insets),
        );
        row.pad_insets(Insets {
            top: line.pad,
            bottom: line.pad,
            ..Default::default()
        })
        .layout(arena, constraints)
    }
}
