// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! How the changes and history docks DRESS their tree rows
//! (docs/Design.md, the Main artboard's right column): the history's
//! branch line and commit rows set their own type, every other row is
//! the hikit tree row hung from the column's text edge — and the
//! history's RAIL, the gutter hairline every commit hangs a node on,
//! is a named modifier over the rows.
//!
//! The forest hands a row its label and depth only, so a row's ROLE
//! (newest, open) is derived from the store at display — the same
//! store facts the row was measured against: a role change is always
//! a landing (a page prepended, a commit's files arrived), and every
//! landing re-slices the view.

use crate::hichanges::{ChangeSet, ChangeSets, Changes, ChangesStatus, Revision};
use crate::hihistory::History;
use editor::location::{ResourceLocation, ResourceType};
use editor::theme::TypeRole;
use hikit::forest::TreeRow;
use hikit::tree_item::{TreeItemCommand, TreeLabelCommand};
use hikit::ui::TextStyle;
use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult},
    layout::{Alignment, Column, Insets, Layout, LayoutExt as _, Row},
    store::Store,
    thunk_ext::ThunkExt as _,
    ui::UiCtx,
    Thunk, ThunkBox, View,
};

pub(crate) const GRAPH_KIND: &str = "history-graph";
pub(crate) const COMMIT_KIND: &str = "history-commit";

/// A dock row: the forest's tree row plus what it stands for.
#[derive(Clone)]
pub struct DockRow {
    row: TreeRow,
    key: ResourceLocation,
    changes: imba::store::Id<ChangeSets>,

    /// The depth whose rows sit AT the column's text edge: a commit's
    /// file tree hangs at its title's edge (history: 2, under the
    /// branch line and the commit), a repository's files at its
    /// name's (changes: 1).
    hang: u16,
}

impl DockRow {
    pub fn new(
        row: TreeRow,
        key: ResourceLocation,
        changes: imba::store::Id<ChangeSets>,
        hang: u16,
    ) -> Self {
        Self {
            row,
            key,
            changes,
            hang,
        }
    }
}

/// The folder a history row's key was minted under — graph and
/// commit keys are `folder.child(kind, name)`.
pub(crate) fn folder_of(key: &ResourceLocation) -> ResourceLocation {
    ResourceLocation::new(
        ResourceType::directory(),
        key.authority().clone(),
        key.path()[..key.path().len().saturating_sub(1)].to_vec(),
    )
}

/// A ramp rung in a color — `TextStyle` from a role the dock's own
/// chrome carries rather than the shared ramp.
fn styled(ui: &UiCtx, role: &TypeRole, color: skia_safe::Color) -> TextStyle {
    TextStyle {
        font: hikit::fonts::role_font(ui, role),
        color,
        tracking: role.tracking,
        shaper: imba::layout::TextShaper::of(ui),
    }
}

/// A commit's files, once they landed — what makes a commit OPEN.
fn landed_files(
    store: &Store,
    changes: imba::store::Id<ChangeSets>,
    folder: &ResourceLocation,
    id: &str,
) -> Option<ChangeSet> {
    Changes::commit_set(store, changes, folder, &Revision::new(id)).filter(|set| {
        set.generation() > 0 && set.status == ChangesStatus::Ready && !set.files.is_empty()
    })
}

/// Greedy word breaks at `measure` — the open title WRAPS. The
/// measure is the column's DESIGNED width, not the laid one: the
/// list measures a row once, width-blind, so a row's height must not
/// hang on the width it is later laid at (a narrower drag ellipsizes
/// each line instead).
fn broken(style: &TextStyle, text: &str, measure: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = match line.is_empty() {
            true => word.to_owned(),
            false => format!("{line} {word}"),
        };
        if !line.is_empty() && style.shaper.advance(&style.font, &candidate) > measure {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Rows press like the tree label: the body consumes nothing the
/// list would answer (`Select`, then `Activate`).
fn quiet_press(
    _arena: &Arena,
    event: &Event<'_>,
    _size: skia_safe::Size,
) -> EventResult<TreeItemCommand<TreeLabelCommand>> {
    match event {
        Event::MouseDown { .. } => EventResult::handled(),
        _ => EventResult::Ignored,
    }
}

impl DockRow {
    /// "main  himark-jb · newest first": the branch strong, the
    /// folder and the order brief, on one baseline.
    fn branch_line<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl Layout<'a, TreeItemCommand<TreeLabelCommand>> + 'a {
        let themes = editor::env::Themes::of(store);
        let theme = themes.ui();
        let chrome = &theme.history;
        let folder = folder_of(&self.key);
        let branch = Changes::of(store, self.changes)
            .and_then(|held| History::folder(store, held.history(), &folder))
            .and_then(|entry| entry.head.branch);
        let (lead, note) = match branch {
            Some(branch) => (branch, format!("{} · newest first", folder.name())),
            None => (folder.name().to_owned(), "newest first".to_owned()),
        };
        Row::new(arena)
            .gap(chrome.header_gap)
            .child_by_baseline(hikit::ui::text(
                &styled(ui, &chrome.branch, theme.ink.ink.0),
                lead,
            ))
            .child_by_baseline(hikit::ui::text(
                &styled(ui, &chrome.folder, theme.ink.ink_3.0),
                note,
            ))
            .pad_insets(Insets {
                left: theme.window.side_text_edge,
                top: chrome.header_top,
                right: theme.window.side_trailing,
                bottom: chrome.header_bottom,
            })
            .on_event(quiet_press)
    }

    /// A commit: its title at the text edge — one ellipsized line,
    /// or, OPEN (standing expanded in the tree), a size up, strong
    /// and wrapping, with the "N files · +a −r" line under it.
    fn commit_row<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl Layout<'a, TreeItemCommand<TreeLabelCommand>> + 'a {
        let themes = editor::env::Themes::of(store);
        let theme = themes.ui();
        let chrome = &theme.history;
        let window = &theme.window;
        let folder = folder_of(&self.key);
        let id = self.key.name();
        // The OPEN commit is the one the tree stands EXPANDED — not
        // the newest, not every commit whose files once landed. Only
        // it wears the strong title and its stat line.
        let open = self.row.expanded() == Some(true);
        let files = landed_files(store, self.changes, &folder, id).filter(|_| open);
        let title = self.row.inner().text();
        let mut block = Column::new(arena);
        let (style, lines) = match open {
            true => {
                let style = styled(ui, &chrome.open_commit, chrome.open_title.0);
                let measure = window.side_width - window.side_text_edge - window.side_trailing;
                let lines = broken(&style, title, measure);
                (style, lines)
            }
            false => (
                styled(ui, &chrome.commit, theme.ink.ink_2.0),
                vec![title.to_owned()],
            ),
        };
        for line in lines {
            block = block.child(
                hikit::ui::text(&style, line)
                    .align(Alignment::CenterStart)
                    .height(chrome.commit_line),
            );
        }
        if let Some(files) = &files {
            let added: i64 = files.files.iter().filter_map(|entry| entry.added).sum();
            let removed: i64 = files.files.iter().filter_map(|entry| entry.removed).sum();
            let count = files.files.len();
            let noun = if count == 1 { "file" } else { "files" };
            let meta = styled(ui, &chrome.meta, theme.ink.ink_3.0);
            let tally = |sign: &str, value: i64, color: skia_safe::Color| {
                let color = if value == 0 { theme.ink.ink_4.0 } else { color };
                hikit::ui::text(&meta.clone().colored(color), format!("{sign}{value}"))
            };
            let space = meta.shaper.advance(&meta.font, " ");
            block = block.child(
                Row::new(arena)
                    .gap(space)
                    .child_by_baseline(hikit::ui::text(&meta, format!("{count} {noun} ·")))
                    .child_by_baseline(tally("+", added, theme.ink.added.0))
                    .child_by_baseline(tally("−", removed, theme.ink.removed.0))
                    .pad_insets(Insets {
                        top: chrome.meta_gap,
                        ..Insets::default()
                    }),
            );
        }
        block
            .pad_insets(Insets {
                left: window.side_text_edge,
                top: chrome.commit_air,
                right: window.side_trailing,
                bottom: match files.is_some() {
                    true => chrome.tree_gap,
                    false => chrome.commit_air,
                },
            })
            .on_event(quiet_press)
    }

    /// Any other row is the hikit tree row, its text hung at the
    /// column's edge plus one indent per level below `hang` — the
    /// frame's own offset is `text_x` plus one indent per level, so
    /// the hang is the difference (negative when the frame sits
    /// deeper than the column wants it).
    fn hung<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl Layout<'a, TreeItemCommand<TreeLabelCommand>> + 'a {
        let themes = editor::env::Themes::of(store);
        let theme = themes.ui();
        let tree = &theme.tree;
        let depth = f32::from(self.row.depth());
        let below = f32::from(self.row.depth().saturating_sub(self.hang));
        let wanted = theme.window.side_text_edge + tree.indent * below;
        let framed = tree.text_x + tree.indent * depth;
        let trail_inset = hikit::ui::RowStyle::drawer(store, ui).trail_inset;
        self.row.display(arena, store, ui).pad_insets(Insets {
            left: wanted - framed,
            right: (theme.window.side_trailing - trail_inset).max(0.0),
            ..Insets::default()
        })
    }
}

impl View for DockRow {
    type Command = TreeItemCommand<TreeLabelCommand>;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        self.row.perform(store, ui, command, fx)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        self.row.destroy(store, fx)
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |arena: &'a Arena, constraints: Constraints| {
            let kind = self.key.kind();
            if *kind == ResourceType::new(GRAPH_KIND) {
                return Layout::layout(self.branch_line(arena, store, ui), arena, constraints);
            }
            if *kind == ResourceType::new(COMMIT_KIND) {
                return Layout::layout(self.commit_row(arena, store, ui), arena, constraints);
            }
            Layout::layout(self.hung(arena, store, ui), arena, constraints)
        })
    }
}

/// The HISTORY RAIL over the dock's rows: a hairline in the gutter
/// down every row but the branch lines, the here-commit's block
/// washed over it, and a ring per commit on top — the newest ringed
/// a grade brighter, the here-commit (the one the cursor rests in)
/// filled live. Paints only the rows the clip shows.
pub(crate) struct Rail<'a> {
    pub list: &'a imba::list::ListView<crate::changes_view::HistoryRow, ResourceLocation>,
    /// Each folder's newest commit row.
    pub newest: Vec<ResourceLocation>,
    pub chrome: editor::theme::HistoryChrome,
    pub ink: editor::theme::InkChrome,
}

/// A row key hangs under a commit when the commit's key is its
/// path prefix (file and directory keys are minted under it).
fn within(key: &ResourceLocation, commit: &ResourceLocation) -> bool {
    key.authority() == commit.authority()
        && key.path().starts_with(commit.path())
        && (key == commit || *key.kind() != ResourceType::new(COMMIT_KIND))
}

impl<'a> Rail<'a> {
    /// The rows the clip shows, as (index, top, height, key).
    fn visible(
        list: &'a imba::list::ListView<crate::changes_view::HistoryRow, ResourceLocation>,
        canvas: &skia_safe::Canvas,
        rect: skia_safe::Rect,
    ) -> Vec<(usize, f32, f32, ResourceLocation)> {
        let Some(clip) = canvas.local_clip_bounds() else {
            return Vec::new();
        };
        let mut index = list.index_at_y((clip.top - rect.top).max(0.0)).unwrap_or(0);
        let mut rows = Vec::new();
        while index < list.len() {
            let Some((top, height)) = list.row_span(index) else {
                break;
            };
            if rect.top + top > clip.bottom {
                break;
            }
            if let Some(key) = list.key_at(index) {
                rows.push((index, top, height, key.clone()));
            }
            index += 1;
        }
        rows
    }

    /// The commit the cursor rests in, if any.
    fn here(
        list: &imba::list::ListView<crate::changes_view::HistoryRow, ResourceLocation>,
        rows: &[(usize, f32, f32, ResourceLocation)],
    ) -> Option<ResourceLocation> {
        let cursor = list.cursor()?;
        rows.iter()
            .map(|(_, _, _, key)| key)
            .filter(|key| *key.kind() == ResourceType::new(COMMIT_KIND))
            .find(|commit| within(cursor, commit))
            .cloned()
    }

    pub fn wrap<Command: 'a>(self, rows: impl Thunk<'a, Command> + 'a) -> impl Thunk<'a, Command> + 'a {
        let Rail {
            list,
            newest,
            chrome,
            ink,
        } = self;
        let below = chrome.clone();
        let below_ink = ink.clone();
        rows.paint_below(move |_arena, canvas, rect| {
            let rows = Self::visible(list, canvas, rect);
            let here = Self::here(list, &rows);
            let mut paint = skia_safe::Paint::default();
            paint.set_color(below_ink.rule_strong.0);
            for (_, top, height, key) in &rows {
                if *key.kind() == ResourceType::new(GRAPH_KIND) {
                    continue;
                }
                canvas.draw_rect(
                    skia_safe::Rect::from_xywh(
                        rect.left + below.rail_x,
                        rect.top + top,
                        below.rail_width,
                        *height,
                    ),
                    &paint,
                );
            }
            // The here-commit's block — its title row through the
            // last row hanging under it — washes across the rail.
            let Some(here) = here else { return };
            let block: Vec<&(usize, f32, f32, ResourceLocation)> = rows
                .iter()
                .skip_while(|(_, _, _, key)| *key != here)
                .take_while(|(_, _, _, key)| within(key, &here))
                .collect();
            if let (Some(first), Some(last)) = (block.first(), block.last()) {
                paint.set_color(below_ink.here.0);
                canvas.draw_rect(
                    skia_safe::Rect::from_ltrb(
                        rect.left,
                        rect.top + first.1,
                        rect.right,
                        rect.top + last.1 + last.2,
                    ),
                    &paint,
                );
            }
        })
        .paint_above(move |_arena, canvas, rect| {
            let rows = Self::visible(list, canvas, rect);
            let here = Self::here(list, &rows);
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            let radius = (chrome.node_size - chrome.node_stroke) * 0.5;
            for (_, top, _, key) in &rows {
                if *key.kind() != ResourceType::new(COMMIT_KIND) {
                    continue;
                }
                let center = (
                    rect.left + chrome.rail_x + chrome.rail_width * 0.5,
                    rect.top + top + chrome.node_top + chrome.node_size * 0.5,
                );
                let (fill, ring) = match (Some(key) == here.as_ref(), newest.contains(key)) {
                    (true, _) => (ink.live.0, ink.live.0),
                    (false, true) => (ink.ground.0, ink.ink_2.0),
                    (false, false) => (ink.ground.0, ink.ink_3.0),
                };
                paint.set_style(skia_safe::paint::Style::Fill);
                paint.set_color(fill);
                canvas.draw_circle(center, radius, &paint);
                paint.set_style(skia_safe::paint::Style::Stroke);
                paint.set_stroke_width(chrome.node_stroke);
                paint.set_color(ring);
                canvas.draw_circle(center, radius, &paint);
            }
        })
    }
}

/// The type-erased rows a `Rail` hands back, for callers that keep a
/// `ThunkBox`.
pub(crate) fn railed<'a, Command: 'a>(
    arena: &'a Arena,
    rail: Rail<'a>,
    rows: impl Thunk<'a, Command> + 'a,
) -> ThunkBox<'a, Command> {
    ThunkBox::new(arena, rail.wrap(rows))
}
