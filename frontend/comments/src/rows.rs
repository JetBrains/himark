// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The comments dock's rows, in the Files tab's language: folders,
//! directories and files are TREE rows (state is typographic, no
//! chips); a comment is its quoted text in the reading ink with the
//! line it stands on, mono, at the right.

use std::sync::Arc;

use editor::location::ResourceLocation;
use hikit::forest::TreeRow;
use hikit::tree_item::{TreeItemCommand, TreeItemView, TreeLabelCommand};
use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult},
    layout::{Layout, LayoutExt as _, LayoutValue},
    store::Store,
    ui::UiCtx,
    View,
};
use skia_safe::Size;

/// What a comment row shows.
#[derive(Clone, Debug)]
pub struct CommentFacts {
    /// The first line of the thread's opening entry.
    pub quote: String,
    /// The 1-based line the comment stands on, when it has a range.
    pub line: Option<u32>,
    /// Entries past the first — the replies.
    pub replies: usize,
    pub resolved: bool,
}

/// The dock's list row: a tree row, or a comment.
#[derive(Clone)]
pub enum CommentsRow {
    Tree(TreeRow),
    Comment(TreeItemView<CommentLine>),
}

impl CommentsRow {
    #[cfg(any(test, feature = "test-support"))]
    pub fn comment(&self) -> Option<&CommentFacts> {
        match self {
            CommentsRow::Tree(_) => None,
            CommentsRow::Comment(row) => Some(&row.inner().facts),
        }
    }
}

/// The forest's row dresser for one refresh: comment keys read their
/// facts, every other key stays the tree row the forest made.
pub(crate) fn dress(
    comments: Arc<std::collections::HashMap<ResourceLocation, CommentFacts>>,
) -> hikit::forest::Wrap<ResourceLocation, CommentsRow> {
    Arc::new(
        move |row: TreeRow, key: &ResourceLocation| match comments.get(key) {
            Some(facts) => CommentsRow::Comment(TreeItemView::leaf(
                CommentLine {
                    facts: facts.clone(),
                },
                row.depth(),
            )),
            None => CommentsRow::Tree(row),
        },
    )
}

impl View for CommentsRow {
    type Command = TreeItemCommand<TreeLabelCommand>;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match self {
            CommentsRow::Tree(row) => row.perform(store, ui, command, fx),
            CommentsRow::Comment(row) => row.perform(store, ui, command, fx),
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        let row: imba::layout::LayoutBox<'a, Self::Command> = match self {
            CommentsRow::Tree(row) => {
                imba::layout::LayoutBox::new(arena, row.display(arena, store, ui))
            }
            CommentsRow::Comment(row) => {
                imba::layout::LayoutBox::new(arena, row.display(arena, store, ui))
            }
        };
        // The column's text edge stands past the tree frame's own
        // inset (the Files tab's lead).
        let theme = editor::env::Themes::of(store);
        let lead = (theme
            .part(::editor::theme::WindowChrome::KEY)
            .side_text_edge
            - theme.part(::hikit::chrome::TreeChrome::KEY).text_x)
            .max(0.0);
        row.pad_insets(imba::layout::Insets {
            left: lead,
            ..Default::default()
        })
    }
}

/// A comment: the quote in the reading ink (dim once resolved), the
/// reply count and the line, mono, at the right.
#[derive(Clone)]
pub struct CommentLine {
    facts: CommentFacts,
}

impl View for CommentLine {
    type Command = TreeLabelCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: Self::Command,
        _fx: &mut Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        let theme = editor::env::Themes::of(store);
        let size = theme.part(::hikit::chrome::TreeChrome::KEY).font_size;
        let quote_style = match self.facts.resolved {
            true => hikit::ui::brief(store, ui).sized(size),
            false => hikit::ui::reading(store, ui).sized(size),
        };
        let brief = hikit::ui::brief(store, ui);
        let margin = hikit::ui::margin(store, ui);
        let mut trail = imba::layout::Row::new(arena).gap(hikit::ui::space::S);
        if self.facts.replies > 0 {
            trail = trail
                .child_by_baseline(hikit::ui::text(&brief, format!("+{}", self.facts.replies)));
        }
        if let Some(line) = self.facts.line {
            trail = trail.child_by_baseline(hikit::ui::text(&margin, line.to_string()));
        }
        QuoteLine {
            quote: hikit::ui::text(&quote_style, self.facts.quote.clone()),
            trail,
            gap: hikit::ui::space::L,
        }
        .pad_insets(imba::layout::Insets {
            top: hikit::ui::space::S,
            right: theme.part(::editor::theme::WindowChrome::KEY).side_trailing,
            bottom: hikit::ui::space::S,
            ..Default::default()
        })
        // The row body consumes nothing: an unclaimed click is the
        // LIST's to answer — `Select` then `Activate(Click)`
        // (docs/ui/list-keyboard.md §2).
        .on_event(
            move |_arena: &Arena, event: &Event<'_>, _size| match event {
                Event::MouseDown { button, mods, .. }
                    if matches!(button, imba::event::MouseButton::Right)
                        || (matches!(button, imba::event::MouseButton::Left) && mods.control) =>
                {
                    EventResult::command(TreeLabelCommand::Context)
                }
                Event::MouseDown { .. } => EventResult::handled(),
                _ => EventResult::Ignored,
            },
        )
    }
}

/// A quote and its trailing words on one baseline: the trail is
/// reserved FIRST and pinned right, so a long quote ellipsizes
/// instead of pushing the line number off the row.
struct QuoteLine<'a, Command> {
    quote: imba::layout::Text,
    trail: imba::layout::Row<'a, Command>,
    gap: f32,
}

impl<Command> LayoutValue for QuoteLine<'_, Command> {}

impl<'a, Command: 'a> Layout<'a, Command> for QuoteLine<'a, Command> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, Command> {
        let QuoteLine { quote, trail, gap } = self;
        let loose = Constraints {
            min: Size::default(),
            max: constraints.max,
        };
        let trail = trail.layout(arena, loose);
        let reserved = imba::Thunk::size(&trail).width + gap;
        let bounded = constraints.max.width < f32::MAX / 2.0;
        let quote = Layout::<Command>::layout(
            quote,
            arena,
            Constraints {
                min: Size::default(),
                max: Size::new(
                    match bounded {
                        true => (constraints.max.width - reserved).max(0.0),
                        false => f32::MAX,
                    },
                    constraints.max.height,
                ),
            },
        );
        imba::layout::Row::new(arena)
            .child_by_baseline(imba::layout::fixed(quote))
            .weighted(1.0, imba::layout::Fill::new().height(0.0))
            .child_by_baseline(imba::layout::fixed(trail).pad_insets(imba::layout::Insets {
                left: gap,
                ..Default::default()
            }))
            .layout(arena, loose)
    }
}
