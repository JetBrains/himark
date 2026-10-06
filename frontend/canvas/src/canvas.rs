// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The diff canvas (docs/editor/diff-canvas.md): one huge TREE-shaped list —
//! per changed file a Header-1 parent row (name, counts, the header
//! buttons) and the file's diff as its child row. Diffs build lazily
//! when their row paints, entirely off-thread, and land atomically:
//! the swap and the height move in one perform, and the settle pulse
//! re-aims the viewport before the frame paints. The header rows ride
//! the list's own sticky machinery (`ListView::with_sticky`), so the
//! current file's name — buttons included — stays planted while its
//! diff scrolls; collapsing a file folds its diff row away.

use crate::diff_canvas::{canvas_files, canvas_generation, CanvasFile, CanvasListing};
use editor::env;
use editor::{location::ResourceLocation, unified_diff::UnifiedDiffCommand};
use imba::effect::{AnyEffect, Effects};
use imba::event::{Event, EventResult, Placement};
use imba::list::{ListCommand, ListSlice, ListView, StickyStyle};
use imba::scroll::{ScrollCommand, ScrollView};
use imba::thunk_ext::ThunkExt;
use imba::{arena::Arena, constraints::Constraints, store::Store, ui::UiCtx, Thunk, View, Widget};
use skia_safe::{Paint, Rect, Size};

const MIN_EST_LINES: i64 = 4;
const MAX_EST_LINES: i64 = 60;
const EST_CONTEXT_LINES: i64 = 4;

/// The row key: the file's `new` side names the FILE node (the header
/// row and its cover span — the reveal target), and the diff child
/// keys itself beside it. The banner heads the list and covers
/// nothing, so the sticky machinery never plants it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum CanvasKey {
    Banner,
    File(ResourceLocation),
    Diff(ResourceLocation),
}

type CanvasRows = ListView<CanvasRow, CanvasKey>;
type RowsCommand = ScrollCommand<ListCommand<RowCommand>>;

#[derive(Clone)]
pub enum CanvasCommand {
    Rows(RowsCommand),

    /// An async landing for the commit banner's message box (the
    /// Bounded build's tail, the markdown reparse) — routed to the
    /// banner row wherever it currently sits.
    BannerEditor(editor::editor_view::EditorCommand),

    Landed {
        key: ResourceLocation,
        prep: documents::diff_views::OpenedDiffPair,
    },

    /// A key-addressed row command — effect landings route by KEY,
    /// never by a captured index: collapse/expand splices shift
    /// indices under in-flight work.
    ToRow {
        key: ResourceLocation,
        command: RowCommand,
    },

    /// A key-addressed command for a row's PARKED successor (the
    /// off-row rebuild a succession is dressing) — after the
    /// promotion the same pane rides the row, so a late landing
    /// falls through to `ToRow`.
    ToPending {
        key: ResourceLocation,
        command: RowCommand,
    },
}

impl std::fmt::Display for CanvasCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CanvasCommand::Rows(command) => command.fmt(out),
            CanvasCommand::BannerEditor(command) => command.fmt(out),
            CanvasCommand::ToRow { command, .. } => command.fmt(out),
            CanvasCommand::ToPending { command, .. } => command.fmt(out),
            CanvasCommand::Landed { .. } => out.write_str("canvas landed"),
        }
    }
}

/// The canvas STATE, store-held in `Canvases` (the `OpenDocuments`
/// discipline): panels are `DiffCanvasView` reference views over a
/// `CanvasId`; opening a source that already has a canvas reuses it —
/// nothing is ever rebuilt for a second click.
#[derive(Clone)]
pub struct Canvas {
    /// The collection that owns this canvas's set.
    changes: imba::store::Id<Changes>,
    source: CanvasSource,
    rows: ScrollView<CanvasRows>,
    files: rpds::HashTrieMapSync<ResourceLocation, CanvasFile>,

    note: Option<String>,
    seen: Option<u64>,
    populated: bool,
    request: Option<hikit::panel::PanelRequest>,

    phases: rpds::HashTrieMapSync<ResourceLocation, RowPhase>,

    /// Rows whose build is already IN FLIGHT — the prefetch horizon
    /// launches rows the paint never armed, and the arm that follows
    /// must not launch them twice. Cleared when the build lands (a
    /// later rebuild goes through `relaunch`, not the arm).
    launched: rpds::HashTrieSetSync<ResourceLocation>,

    /// Speculative builds waiting their turn — the horizon the last
    /// arm aimed at, drained ONE at a time from the batch tail. A
    /// flood here was a freeze: the serial worker finished the opens
    /// back to back and their landings clumped into one UI batch,
    /// while the visible row's normalize queued behind every
    /// speculative open.
    prefetch_queue: std::collections::VecDeque<ResourceLocation>,

    /// The width the queued rows will build at (the arming row's — one
    /// column, every row shares it).
    prefetch_width: f32,

    /// The one speculative open in flight. The pump launches nothing
    /// while this stands; the landing clears it.
    prefetching: Option<ResourceLocation>,

    /// An armed reveal: applied at populate, or picked up by the
    /// paint probe when a reuse navigation arms it later.
    reveal: Option<ResourceLocation>,

    /// Views standing on this canvas. At zero the canvas retires —
    /// except the WORKING-COPY canvas, which stays once opened.
    refs: u32,

    /// Collapsed files' diff rows, parked with their heights — the
    /// views stay alive (a built diff keeps its editors), the list
    /// just stops holding their rows.
    stash: rpds::HashTrieMapSync<ResourceLocation, (CanvasRow, f32)>,

    /// Built rows by their diff view — the DRESSING sweep names the
    /// views it touched (crate::DressedViews) and the canvas resizes
    /// exactly those rows, O(touched), no scan.
    pairs: rpds::HashTrieMapSync<documents::diffs::DiffViewId, ResourceLocation>,

    /// Relaunched builds for rows whose diff the user has already
    /// SEEN (`ever_dressed`): the fresh pane dresses off-row while
    /// the old view keeps showing, and the batch-tail sync promotes
    /// it once whole — stub → diff happens exactly once per row,
    /// never again on a relaunch.
    successions: rpds::HashTrieMapSync<ResourceLocation, Succession>,
}

/// One parked rebuild: the successor pane and the width it was
/// built at (it lands into the row's standing geometry).
#[derive(Clone)]
struct Succession {
    pane: crate::diff_pane::PairPane,
    built_width: f32,
}

/// A row's lifecycle, panel-tracked — the test oracle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowPhase {
    Placeholder,
    Built,
    Failed,
}

/// The prefetch horizon past an armed row: at most this many
/// placeholder rows launch speculatively…
const PREFETCH_ROWS: usize = 8;
/// …and their change mass (added+removed lines, the only size signal
/// the listing carries) may sum to at most this much. The walk stops
/// at the first row that would overflow — a huge file loads when the
/// user actually reaches it, never speculatively.
const PREFETCH_LINES: i64 = 4000;
/// A listing entry with no stats weighs this much against the budget.
const PREFETCH_UNKNOWN_LINES: i64 = 400;

fn sticky_style() -> imba::list::StickySource {
    std::sync::Arc::new(|store: &Store| {
        let window = env::Themes::of(store).ui().window.clone();
        StickyStyle {
            background: window.background.0,
            divider: window.divider.0,
            divider_width: window.divider_width,
        }
    })
}

impl Canvas {
    fn fresh(changes: imba::store::Id<Changes>, source: CanvasSource) -> Self {
        Self {
            changes,
            source,
            rows: ScrollView::new(ListView::empty().with_sticky(sticky_style())),
            files: rpds::HashTrieMapSync::new_sync(),
            note: None,
            seen: None,
            populated: false,
            successions: rpds::HashTrieMapSync::new_sync(),
            request: None,
            phases: rpds::HashTrieMapSync::new_sync(),
            launched: rpds::HashTrieSetSync::new_sync(),
            prefetch_queue: std::collections::VecDeque::new(),
            prefetch_width: 0.0,
            prefetching: None,
            reveal: None,
            refs: 0,
            stash: rpds::HashTrieMapSync::new_sync(),
            pairs: rpds::HashTrieMapSync::new_sync(),
        }
    }

    #[doc(hidden)]
    pub fn probe_note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    #[doc(hidden)]
    pub fn probe_scroll_top(&self) -> f32 {
        self.rows.scroll_y()
    }

    /// (title, phase, current diff-row height), in list order;
    /// collapsed files report their parked row at height zero.
    #[doc(hidden)]
    pub fn probe_rows(&self) -> Vec<(String, RowPhase, f32)> {
        let rows = self.rows.content();
        (0..rows.len())
            .filter_map(|index| {
                let key = rows.key_at(index)?;
                let CanvasKey::File(location) = key else {
                    return None;
                };
                let title = self.files.get(location)?.title.clone();
                let phase = self
                    .phases
                    .get(location)
                    .copied()
                    .unwrap_or(RowPhase::Placeholder);
                let height = rows
                    .row_range(&CanvasKey::Diff(location.clone()))
                    .and_then(|range| rows.height_at(range.start))
                    .unwrap_or(0.0);
                Some((title, phase, height))
            })
            .collect()
    }

    fn diff_rows(&self) -> Vec<(String, DiffRow)> {
        let rows = self.rows.content();
        let mut out = Vec::new();
        for index in 0..rows.len() {
            let Some(CanvasKey::File(location)) = rows.key_at(index) else {
                continue;
            };
            let Some(title) = self.files.get(location).map(|file| file.title.clone()) else {
                continue;
            };
            let held = match rows.row_range(&CanvasKey::Diff(location.clone())) {
                Some(range) => rows.view_at(range.start),
                None => self.stash.get(location).map(|(row, _)| row.clone()),
            };
            if let Some(CanvasRow::Diff(diff)) = held {
                out.push((title, diff));
            }
        }
        out
    }

    fn banner_row(&self) -> Option<BannerRow> {
        let range = self.rows.content().row_range(&CanvasKey::Banner)?;
        match self.rows.content().view_at(range.start)? {
            CanvasRow::Banner(banner) => Some(banner),
            _ => None,
        }
    }

    fn composer_text(&self) -> Option<String> {
        match self.banner_row()? {
            BannerRow::Composer { message, .. } => {
                let text = message.document.text();
                let end = text.byte_count().min(u32::MAX as usize) as u32;
                Some(text.view().substring(0..end))
            }
            _ => None,
        }
    }

    /// (focused, text) of the working-copy composer banner.
    #[doc(hidden)]
    pub fn probe_composer(&self) -> Option<(bool, String)> {
        match self.banner_row()? {
            BannerRow::Composer { message, focused } => {
                let text = message.document.text();
                let end = text.byte_count().min(u32::MAX as usize) as u32;
                Some((focused, text.view().substring(0..end)))
            }
            _ => None,
        }
    }

    /// (message, author) of a commit canvas's banner.
    #[doc(hidden)]
    pub fn probe_banner(&self) -> Option<(String, String)> {
        match self.banner_row()? {
            BannerRow::Commit {
                message, author, ..
            } => {
                let text = message.document.text();
                let end = text.byte_count().min(u32::MAX as usize) as u32;
                Some((text.view().substring(0..end), author))
            }
            _ => None,
        }
    }

    /// TEST SUPPORT: every list row's key, in order — catches an
    /// orphaned Diff row a per-file probe would miss.
    #[doc(hidden)]
    pub fn probe_row_keys(&self) -> Vec<String> {
        let rows = self.rows.content();
        (0..rows.len())
            .filter_map(|index| match rows.key_at(index)? {
                CanvasKey::Banner => Some("Banner".to_owned()),
                CanvasKey::File(loc) => Some(format!("File:{}", loc.name())),
                CanvasKey::Diff(loc) => Some(format!("Diff:{}", loc.name())),
            })
            .collect()
    }

    /// TEST SUPPORT: the File key's cover span — the sticky
    /// machinery's input (2 rows expanded, 1 collapsed).
    #[doc(hidden)]
    pub fn probe_cover(&self, key: &ResourceLocation) -> Option<usize> {
        self.rows
            .content()
            .row_range(&CanvasKey::File(key.clone()))
            .map(|range| range.len())
    }

    /// TEST SUPPORT: the registered `DiffViewId` backing a built row.
    #[doc(hidden)]
    pub fn probe_pair(&self, key: &ResourceLocation) -> Option<documents::diffs::DiffViewId> {
        let row = self
            .rows
            .content()
            .row_range(&CanvasKey::Diff(key.clone()))
            .and_then(|range| self.rows.content().view_at(range.start))
            .or_else(|| self.stash.get(key).map(|(row, _)| row.clone()))?;
        match row {
            CanvasRow::Diff(DiffRow {
                body: RowBody::Built { pane },
                ..
            }) => Some(pane.id()),
            _ => None,
        }
    }

    /// Per Built row: (left content height, right content height,
    /// inline content height, left width, right width) — the split
    /// alignment oracle.
    #[doc(hidden)]
    pub fn probe_half_heights(&self, store: &Store) -> Vec<(f32, f32, f32, f32, f32)> {
        self.diff_rows()
            .into_iter()
            .filter_map(|(_, diff)| {
                let RowBody::Built { pane } = &diff.body else {
                    return None;
                };
                let Some(view) =
                    crate::diff_pane::gathered_view(store, pane.documents(), pane.id())
                else {
                    return None;
                };
                let left = &view.split.left;
                let right = &view.split.right;
                Some((
                    left.document.content_height(left.editor),
                    right.document.content_height(right.editor),
                    view.inline_editor
                        .map(|editor| right.document.content_height(editor))
                        .unwrap_or(-1.0),
                    left.document.layout_width(left.editor),
                    right.document.layout_width(right.editor),
                ))
            })
            .collect()
    }

    /// Per Built row (parked ones included): (title, current face).
    #[doc(hidden)]
    pub fn probe_layouts(&self, store: &Store) -> Vec<(String, editor::unified_diff::DiffLayout)> {
        self.diff_rows()
            .into_iter()
            .filter_map(|(title, diff)| {
                let RowBody::Built { pane } = &diff.body else {
                    return None;
                };
                let Some(view) =
                    crate::diff_pane::gathered_view(store, pane.documents(), pane.id())
                else {
                    return None;
                };
                Some((title, view.layout))
            })
            .collect()
    }

    /// Per Built row: (title, host/inline focus, each card's focus).
    #[doc(hidden)]
    pub fn probe_focus(&self, store: &Store) -> Vec<(String, String, Vec<String>)> {
        self.diff_rows()
            .into_iter()
            .filter_map(|(title, diff)| {
                let RowBody::Built { pane } = &diff.body else {
                    return None;
                };
                let Some(view) =
                    crate::diff_pane::gathered_view(store, pane.documents(), pane.id())
                else {
                    return None;
                };
                let inline = view.inline_editor?;
                let host = format!("{:?}", view.split.right.document.focus(inline));
                let cards = view
                    .split
                    .right
                    .document
                    .before_inlay_views(inline)
                    .into_iter()
                    .map(|(_, card)| format!("{:?}", card.card_focus()))
                    .collect();
                Some((title, host, cards))
            })
            .collect()
    }

    fn refresh(
        &mut self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        let (generation, listing) = canvas_files(store, self.changes, &self.source);
        self.adopt(store, ui, generation, listing, fx);
    }

    /// The push-road reconcile, driven from the sync tick with a REAL
    /// routed sink (`Canvases::sync`). Three explicit steps, no paint:
    /// membership follows the feed generation; each BUILT row's stored
    /// height follows its diff's fresh content height (the diff lane
    /// ran just before us — the list's paint SetHeight only corrects
    /// the VISIBLE part on a resize, never a model change like a fold
    /// landing); an armed reveal lands.
    /// Perform a command against a PARKED successor. Promoted (or
    /// torn down) meanwhile: a Diff command falls through to the row
    /// — after a promotion the same pane rides it.
    fn to_pending(
        &mut self,
        key: ResourceLocation,
        command: RowCommand,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        let RowCommand::Diff(command) = command else {
            return;
        };
        match self.successions.get(&key).cloned() {
            Some(mut next) => {
                let route = key.clone();
                fx.scope(
                    move |command| CanvasCommand::ToPending {
                        key: route.clone(),
                        command: RowCommand::Diff(command),
                    },
                    |fx| next.pane.perform(store, ui, command, fx),
                );
                self.successions.insert_mut(key, next);
            }
            None => self.to_row(key, RowCommand::Diff(command), store, ui, fx),
        }
    }

    /// Swap a DRESSED successor into its row — the one visible
    /// transition of a relaunch, diff-for-diff, no skeleton between.
    fn promote(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        key: &ResourceLocation,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        let Some(next) = self.successions.get(key).cloned() else {
            return;
        };
        self.successions.remove_mut(key);
        let Some(file) = self.files.get(key).cloned() else {
            documents::diff_views::teardown_diff_view(store, next.pane.documents(), next.pane.id());
            return;
        };
        // The old pane dies only now — the swap is dressed-for-dressed.
        self.teardown_row(store, key);
        self.pairs.insert_mut(next.pane.id(), key.clone());
        let diff = CanvasRow::Diff(DiffRow {
            file: file.clone(),
            body: RowBody::Built { pane: next.pane },
            rewrap_ask: None,
            built_width: Some(next.built_width),
        });
        let theme = env::Themes::of(store);
        let Some(header_range) = self.rows.content().row_range(&CanvasKey::File(key.clone()))
        else {
            return;
        };
        let start = header_range.start;
        let expanded = self
            .rows
            .content()
            .row_range(&CanvasKey::Diff(key.clone()))
            .is_some();
        let header = CanvasRow::Header(HeaderRow {
            file,
            collapsed: !expanded,
            built: true,
        });
        let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
        slice.push_keyed_sized(CanvasKey::File(key.clone()), header, header_band(&theme));
        if expanded {
            let height = self
                .rows
                .content()
                .row_range(&CanvasKey::Diff(key.clone()))
                .and_then(|range| self.rows.content().height_at(range.start))
                .unwrap_or(0.0);
            slice.push_keyed_sized(CanvasKey::Diff(key.clone()), diff, height);
            slice.cover(CanvasKey::File(key.clone()), 0..2);
            self.rows
                .content_mut()
                .splice_slice(start..start + 2, slice);
            // The successor is dressed: settle to its honest height.
            self.resize_key(key, store, ui, fx);
        } else {
            let height = self.stash.get(key).map(|(_, held)| *held).unwrap_or(0.0);
            slice.cover(CanvasKey::File(key.clone()), 0..1);
            self.stash.insert_mut(key.clone(), (diff, height));
            self.rows
                .content_mut()
                .splice_slice(start..start + 1, slice);
        }
        fx.settle();
    }

    fn sync_in_place(
        &mut self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        dressed: &[documents::diffs::DiffViewId],
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        if self.seen != Some(canvas_generation(store, self.changes, &self.source)) {
            self.refresh(store, ui, fx);
        }
        // A parked successor that answers DRESSED promotes now —
        // checked directly (not off the dressed list) so a
        // succession can never stall on a missed signal.
        let ready: Vec<ResourceLocation> = self
            .successions
            .iter()
            .filter(|(_, next)| {
                crate::diff_pane::gathered_view(store, next.pane.documents(), next.pane.id())
                    .is_some_and(|view| view.dressed())
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in ready {
            self.promote(store, ui, &key, fx);
        }
        // The dressing sweep names the views it re-dressed this batch;
        // resize exactly OUR rows among them — O(touched), no scan.
        for id in dressed {
            let Some(key) = self.pairs.get(id).cloned() else {
                continue;
            };
            self.resize_key(&key, store, ui, fx);
        }
        if self.populated {
            if let Some(key) = self.reveal.take() {
                self.rows
                    .content_mut()
                    .reveal_row(CanvasKey::File(key), Placement::TopLeftAt);
                fx.settle();
            }
        }
        self.pump_prefetch(store, fx);
    }

    /// Resize ONE diff row to its current body height — called right
    /// after a command reaches that row (a repair, a fold toggle, a
    /// face flip, the diff's own Resync), which is exactly when — and
    /// the only time — its height can change. No scan: we know which
    /// row changed because we just routed a command to it. A no-op
    /// unless the row is a built diff whose stored height has drifted.
    fn resize_row(
        &mut self,
        index: usize,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        let Some(CanvasKey::Diff(key)) = self.rows.content().key_at(index) else {
            return;
        };
        // Re-resolve the index by key: an interleaved splice may have
        // shifted it since the command that triggered this.
        let key = key.clone();
        self.resize_key(&key, store, ui, fx);
    }

    /// Resize ONE built row (by key) to its diff's current body height
    /// — a rope point update, the only height writer besides `land`.
    fn resize_key(
        &mut self,
        key: &ResourceLocation,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        let key = key.clone();
        let Some(range) = self.rows.content().row_range(&CanvasKey::Diff(key)) else {
            return;
        };
        let index = range.start;
        let Some(CanvasRow::Diff(DiffRow {
            body: RowBody::Built { pane },
            ..
        })) = self.rows.content().view_at(index)
        else {
            return;
        };
        let Some(view) = crate::diff_pane::gathered_view(store, pane.documents(), pane.id()) else {
            return;
        };
        // The row holds its reserved band under the skeleton until the
        // dressing is whole — the flip to the real height IS the one
        // visible transition.
        if !view.dressed() {
            return;
        }
        let body = match view.layout {
            editor::unified_diff::DiffLayout::Inline => match view
                .inline_editor
                .map(|editor| view.split.right.document.content_height(editor))
            {
                Some(height) => height,
                None => return,
            },
            editor::unified_diff::DiffLayout::Split => {
                let left = view
                    .split
                    .left
                    .document
                    .content_height(view.split.left.editor);
                let right = view
                    .split
                    .right
                    .document
                    .content_height(view.split.right.editor);
                left.max(right)
            }
        };
        let want = body + env::Themes::of(store).ui().chat.gap;
        if (want - self.rows.content().height_at(index).unwrap_or(0.0)).abs() > 0.5 {
            let rows = ScrollCommand::Content(ListCommand::SetHeight(index, want));
            fx.scope(CanvasCommand::Rows, |fx| {
                self.rows.perform(store, ui, rows, fx)
            });
        }
    }

    fn adopt(
        &mut self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        generation: u64,
        listing: CanvasListing,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        match listing {
            // A pending listing is NOT adopted: the stamp stays put, so
            // the canvas keeps probing until the source answers. Stamping
            // here was the desync — a read that saw nothing recorded a
            // generation it never consumed.
            CanvasListing::Pending(text) => {
                if !self.populated {
                    self.note = Some(text);
                }
            }
            CanvasListing::Empty(text) => {
                self.seen = Some(generation);
                // The after-commit state: the set is READY and empty.
                // A populated canvas retires every row (the committed
                // change set is gone — holding it was the bug) and
                // shows the note; an unpopulated one just notes.
                if self.populated {
                    self.reconcile(store, Vec::new(), fx);
                }
                self.note = Some(text);
            }
            CanvasListing::Ready(files) => {
                self.seen = Some(generation);
                if self.populated {
                    self.reconcile(store, files, fx);
                    return;
                }
                self.populated = true;
                self.note = None;
                let theme = env::Themes::of(store);
                let band = header_band(&theme);
                let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
                match crate::diff_canvas::canvas_banner(store, self.changes, &self.source) {
                    Some(crate::diff_canvas::CanvasBanner::Composer { .. }) => {
                        let message = fresh_composer_box(store, ui);
                        let height = composer_band(&theme, Some(&message));
                        slice.push_keyed_sized(
                            CanvasKey::Banner,
                            CanvasRow::Banner(BannerRow::Composer {
                                message,
                                focused: false,
                            }),
                            height,
                        );
                    }
                    Some(crate::diff_canvas::CanvasBanner::Commit { message, author }) => {
                        let message = commit_banner_box(store, ui, &message, fx);
                        let height = commit_band(&theme, &message);
                        slice.push_keyed_sized(
                            CanvasKey::Banner,
                            CanvasRow::Banner(BannerRow::Commit {
                                message,
                                author,
                                focused: false,
                            }),
                            height,
                        );
                    }
                    None => {}
                }
                for file in &files {
                    let start = slice.len();
                    slice.push_keyed_sized(
                        CanvasKey::File(file.new.clone()),
                        CanvasRow::Header(HeaderRow {
                            file: file.clone(),
                            collapsed: false,
                            built: false,
                        }),
                        band,
                    );
                    slice.push_keyed_sized(
                        CanvasKey::Diff(file.new.clone()),
                        CanvasRow::Diff(DiffRow {
                            file: file.clone(),
                            body: RowBody::Placeholder { armed: false },
                            rewrap_ask: None,
                            built_width: None,
                        }),
                        reserved_body(&theme, file),
                    );
                    slice.cover(CanvasKey::File(file.new.clone()), start..start + 2);
                }
                for file in files {
                    self.phases
                        .insert_mut(file.new.clone(), RowPhase::Placeholder);
                    self.files.insert_mut(file.new.clone(), file);
                }
                self.rows.content_mut().splice_slice(0..0, slice);
                if let Some(key) = self.reveal.take() {
                    self.rows
                        .content_mut()
                        .reveal_row(CanvasKey::File(key), Placement::TopLeftAt);
                }
            }
        }
    }

    /// Reconcile a populated canvas with a fresh listing — the unified
    /// gate's second half (docs/editor/diff-canvas.md §7). Removed pairs
    /// retire, added pairs splice in as lazy placeholders, and a pair
    /// the host stamped newer than the row's build relaunches its
    /// build at the standing width — the old view keeps showing until
    /// the landing swaps it, so a refresh never flashes placeholders.
    /// Surviving rows never move: the feed reorders on every touch
    /// (`ChangesetFileSet` re-appends), and rows must not jump.
    fn reconcile(
        &mut self,
        store: &mut Store,
        fresh: Vec<CanvasFile>,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        self.note = None;
        let mut moved = false;
        let incoming: std::collections::HashSet<ResourceLocation> =
            fresh.iter().map(|file| file.new.clone()).collect();
        let retired: Vec<ResourceLocation> = self
            .files
            .keys()
            .filter(|key| !incoming.contains(*key))
            .cloned()
            .collect();
        for key in retired {
            self.retire(store, &key);
            moved = true;
        }

        let theme = env::Themes::of(store);
        let band = header_band(&theme);
        let mut anchor = match self.rows.content().row_range(&CanvasKey::Banner) {
            Some(range) => range.end,
            None => 0,
        };
        for file in fresh {
            let key = file.new.clone();
            match self.files.get(&key).cloned() {
                Some(known) => {
                    if let Some(header) =
                        self.rows.content().row_range(&CanvasKey::File(key.clone()))
                    {
                        anchor = match self.rows.content().row_range(&CanvasKey::Diff(key.clone()))
                        {
                            Some(diff) => header.end.max(diff.end),
                            None => header.end,
                        };
                    }
                    if file.updated > known.updated {
                        self.files.insert_mut(key.clone(), file.clone());
                        self.refresh_header(&key, &file, band);
                        // The stamp moved: relaunch the build NOW,
                        // through the routed sink — no `owed`, no paint.
                        self.relaunch(store, &key, &file, fx);
                        moved = true;
                    } else if file != known {
                        // A value change without a stamp move should
                        // not happen; keep the chrome honest anyway.
                        self.files.insert_mut(key.clone(), file.clone());
                        self.refresh_header(&key, &file, band);
                        moved = true;
                    }
                }
                None => {
                    let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
                    slice.push_keyed_sized(
                        CanvasKey::File(key.clone()),
                        CanvasRow::Header(HeaderRow {
                            file: file.clone(),
                            collapsed: false,
                            built: false,
                        }),
                        band,
                    );
                    slice.push_keyed_sized(
                        CanvasKey::Diff(key.clone()),
                        CanvasRow::Diff(DiffRow {
                            file: file.clone(),
                            body: RowBody::Placeholder { armed: false },
                            rewrap_ask: None,
                            built_width: None,
                        }),
                        reserved_body(&theme, &file),
                    );
                    slice.cover(CanvasKey::File(key.clone()), 0..2);
                    self.rows.content_mut().splice_slice(anchor..anchor, slice);
                    anchor += 2;
                    self.phases.insert_mut(key.clone(), RowPhase::Placeholder);
                    self.files.insert_mut(key, file);
                    moved = true;
                }
            }
        }
        if moved {
            fx.settle();
        }
    }

    fn retire(&mut self, store: &mut Store, key: &ResourceLocation) {
        self.teardown_row(store, key);
        if let Some(header) = self.rows.content().row_range(&CanvasKey::File(key.clone())) {
            let end = match self.rows.content().row_range(&CanvasKey::Diff(key.clone())) {
                Some(diff) => header.end.max(diff.end),
                None => header.end,
            };
            let empty: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
            self.rows
                .content_mut()
                .splice_slice(header.start..end, empty);
        }
        self.files.remove_mut(key);
        self.phases.remove_mut(key);
        self.launched.remove_mut(key);
        if self.prefetching.as_ref() == Some(key) {
            self.prefetching = None;
        }
        self.stash.remove_mut(key);
        if self.reveal.as_ref() == Some(key) {
            self.reveal = None;
        }
    }

    /// Untrack a row's diff and drop its editors from the (shared)
    /// registered documents — the rows no longer die with the canvas
    /// now that they ARE registered documents
    /// (docs/editor/diff-canvas.md §7). Covers the live row and a
    /// collapsed row parked in the stash.
    fn teardown_row(&mut self, store: &mut Store, key: &ResourceLocation) {
        if let Some(next) = self.successions.get(key).cloned() {
            self.successions.remove_mut(key);
            documents::diff_views::teardown_diff_view(store, next.pane.documents(), next.pane.id());
        }
        if let Some(pane) = self.row_pane(key) {
            self.pairs.remove_mut(&pane.id());
            documents::diff_views::teardown_diff_view(store, pane.documents(), pane.id());
        }
    }

    /// The row's standing BUILT pane — live in the list or parked in
    /// the collapse stash.
    fn row_pane(&self, key: &ResourceLocation) -> Option<crate::diff_pane::PairPane> {
        let row = self
            .rows
            .content()
            .row_range(&CanvasKey::Diff(key.clone()))
            .and_then(|range| self.rows.content().view_at(range.start))
            .or_else(|| self.stash.get(key).map(|(row, _)| row.clone()))?;
        match row {
            CanvasRow::Diff(DiffRow {
                body: RowBody::Built { pane },
                ..
            }) => Some(pane),
            _ => None,
        }
    }

    /// Resplices one header row in place — same band, fresh stats.
    /// The File key's structure interval is the COVER (header + diff
    /// child when expanded) and it is what the sticky machinery
    /// reads; a header-only resplice must re-assert the FULL cover,
    /// or the file loses its sticky header (the working-copy canvas
    /// reconciles on every host touch — the commit canvas never does,
    /// which is how this once shipped asymmetrically broken).
    fn refresh_header(&mut self, key: &ResourceLocation, file: &CanvasFile, band: f32) {
        let Some(header) = self.rows.content().row_range(&CanvasKey::File(key.clone())) else {
            return;
        };
        let built = matches!(
            self.phases.get(key),
            Some(RowPhase::Built) | Some(RowPhase::Failed)
        );
        match self.rows.content().row_range(&CanvasKey::Diff(key.clone())) {
            Some(diff_range) => {
                let Some(diff_row) = self.rows.content().view_at(diff_range.start) else {
                    return;
                };
                let diff_height = self
                    .rows
                    .content()
                    .height_at(diff_range.start)
                    .unwrap_or(0.0);
                let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
                slice.push_keyed_sized(
                    CanvasKey::File(key.clone()),
                    CanvasRow::Header(HeaderRow {
                        file: file.clone(),
                        collapsed: false,
                        built,
                    }),
                    band,
                );
                slice.push_keyed_sized(CanvasKey::Diff(key.clone()), diff_row, diff_height);
                slice.cover(CanvasKey::File(key.clone()), 0..2);
                self.rows
                    .content_mut()
                    .splice_slice(header.start..header.start + 2, slice);
            }
            None => {
                let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
                slice.push_keyed_sized(
                    CanvasKey::File(key.clone()),
                    CanvasRow::Header(HeaderRow {
                        file: file.clone(),
                        collapsed: true,
                        built,
                    }),
                    band,
                );
                slice.cover(CanvasKey::File(key.clone()), 0..1);
                self.rows
                    .content_mut()
                    .splice_slice(header.start..header.start + 1, slice);
            }
        }
    }

    /// Launch a row's diff prep: resolve each side on the UI thread (an
    /// open side hands over its live registry snapshot, no fetch/
    /// throwaway; a closed side is fetched, built, and registered at
    /// the landing), run the shared off-thread prep, land a fully
    /// dressed pair (docs/editor/diff-canvas.md §4).
    fn launch_pair(
        store: &Store,
        changes: imba::store::Id<Changes>,
        key: ResourceLocation,
        file: &CanvasFile,
        width: f32,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        let Some(documents) = Changes::of(store, changes).map(|held| held.documents()) else {
            return;
        };
        let old = documents::diff_views::DiffSideInput::resolve(store, documents, file.old.clone());
        let new = documents::diff_views::DiffSideInput::resolve(store, documents, file.new.clone());
        fx.push(
            AnyEffect::new(documents::diff_views::OpenDiffPairEffect { old, new, width }).map(
                move |prep| CanvasCommand::Landed {
                    key: key.clone(),
                    prep,
                },
            ),
        );
    }

    /// Relaunch a stale row's build at its standing width. A row that
    /// never built (placeholder) has no width yet — its paint arm
    /// picks up the fresh pair from `files` on its own.
    fn relaunch(
        &self,
        store: &Store,
        key: &ResourceLocation,
        file: &CanvasFile,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        let Some(width) = self.built_width(key) else {
            return;
        };
        Self::launch_pair(store, self.changes, key.clone(), file, width, fx);
    }

    fn built_width(&self, key: &ResourceLocation) -> Option<f32> {
        let row = match self.rows.content().row_range(&CanvasKey::Diff(key.clone())) {
            Some(range) => self.rows.content().view_at(range.start),
            None => self.stash.get(key).map(|(row, _)| row.clone()),
        }?;
        match row {
            CanvasRow::Diff(DiffRow { built_width, .. }) => built_width,
            _ => None,
        }
    }

    /// TEST SUPPORT: seed one BUILT row directly — the perf harness
    /// measures canvas RENDERING without the changes feed or a host.
    #[doc(hidden)]
    pub(crate) fn seed_built_for_tests(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        file: CanvasFile,
        prep: documents::diff_views::OpenedDiffPair,
    ) {
        let theme = env::Themes::of(store);
        let key = file.new.clone();
        let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
        slice.push_keyed_sized(
            CanvasKey::File(key.clone()),
            CanvasRow::Header(HeaderRow {
                file: file.clone(),
                collapsed: false,
                built: false,
            }),
            header_band(&theme),
        );
        slice.push_keyed_sized(
            CanvasKey::Diff(key.clone()),
            CanvasRow::Diff(DiffRow {
                file: file.clone(),
                body: RowBody::Placeholder { armed: false },
                rewrap_ask: None,
                built_width: None,
            }),
            reserved_body(&theme, &file),
        );
        slice.cover(CanvasKey::File(key.clone()), 0..2);
        self.phases.insert_mut(key.clone(), RowPhase::Placeholder);
        self.files.insert_mut(key.clone(), file);
        let at = self.rows.content().len();
        self.rows.content_mut().splice_slice(at..at, slice);
        self.populated = true;
        let mut throwaway = imba::effect::Batch::new();
        self.land(store, ui, key, prep, &mut throwaway.effects());
    }

    fn launch(
        &mut self,
        store: &Store,
        index: usize,
        width: f32,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        let Some(CanvasKey::Diff(location)) = self.rows.content().key_at(index).cloned() else {
            return;
        };
        let Some(file) = self.files.get(&location).cloned() else {
            return;
        };
        self.launch_once(store, location, &file, width, fx);
        self.prefetch(index, width);
    }

    fn launch_once(
        &mut self,
        store: &Store,
        key: ResourceLocation,
        file: &CanvasFile,
        width: f32,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        if self.launched.contains(&key) {
            return;
        }
        self.launched.insert_mut(key.clone());
        Self::launch_pair(store, self.changes, key, file, width, fx);
    }

    /// The prefetch horizon: an ARMED row names the moment and the
    /// width — the placeholders just past it are AIMED AT, bounded, so
    /// scrolling meets built diffs instead of skeletons. Nothing
    /// launches here: the batch-tail pump drains the queue one open at
    /// a time, each landing admitting the next, so the serial worker
    /// never holds more than one speculative open ahead of visible
    /// work and the landings arrive spread out instead of clumping
    /// into one UI batch.
    fn prefetch(&mut self, index: usize, width: f32) {
        let rows = self.rows.content();
        self.prefetch_queue = (index + 1..rows.len())
            .filter_map(|at| match rows.key_at(at) {
                Some(CanvasKey::Diff(location)) => Some(location.clone()),
                _ => None,
            })
            .filter(|key| {
                !self.launched.contains(key)
                    && matches!(self.phases.get(key), Some(RowPhase::Placeholder))
            })
            .filter_map(|key| {
                let file = self.files.get(&key)?;
                let weight = match (file.added, file.removed) {
                    (None, None) => PREFETCH_UNKNOWN_LINES,
                    (added, removed) => added.unwrap_or(0) + removed.unwrap_or(0),
                };
                Some((key, weight))
            })
            // The mass budget: the scan dies at the first row that
            // would overflow — a wall, not a sieve.
            .scan(PREFETCH_LINES, |left, (key, weight)| {
                *left -= weight;
                (*left >= 0).then_some(key)
            })
            .take(PREFETCH_ROWS)
            .collect();
        self.prefetch_width = width;
    }

    /// Drain ONE speculative open, if none is in flight. Runs at the
    /// batch tail — after the diff lanes — so a landed row's normalize
    /// and repair are already queued ahead of the next speculation.
    fn pump_prefetch(&mut self, store: &Store, fx: &mut Effects<'_, CanvasCommand>) {
        if self
            .prefetching
            .as_ref()
            .is_some_and(|key| self.launched.contains(key))
        {
            return;
        }
        self.prefetching = None;
        while let Some(key) = self.prefetch_queue.pop_front() {
            if self.launched.contains(&key)
                || !matches!(self.phases.get(&key), Some(RowPhase::Placeholder))
            {
                continue;
            }
            let Some(file) = self.files.get(&key).cloned() else {
                continue;
            };
            self.prefetching = Some(key.clone());
            self.launch_once(store, key, &file, self.prefetch_width, fx);
            return;
        }
    }

    fn land(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        key: ResourceLocation,
        prep: documents::diff_views::OpenedDiffPair,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        self.launched.remove_mut(&key);
        if self.prefetching.as_ref() == Some(&key) {
            self.prefetching = None;
        }
        let Some(file) = self.files.get(&key).cloned() else {
            return;
        };
        // A row whose diff the user has SEEN never falls back to the
        // skeleton: the rebuild mounts OFF-ROW, dresses there, and
        // the batch-tail sync promotes it whole (stub → diff is one
        // transition, once). A failed prep falls through — an honest
        // failure face beats a silently stale diff.
        if !prep.failed {
            let shown = self.row_pane(&key).is_some_and(|standing| {
                crate::diff_pane::gathered_view(store, standing.documents(), standing.id())
                    .is_some_and(|view| view.ever_dressed())
            });
            if shown {
                let built_width = prep.width;
                let route = key.clone();
                let parked = fx.scope(
                    move |command: RowCommand| CanvasCommand::ToPending {
                        key: route.clone(),
                        command,
                    },
                    |fx| mounted(store, self.changes, ui, prep, fx),
                );
                if let Some((pane, _)) = parked {
                    if let Some(previous) = self.successions.get(&key).cloned() {
                        documents::diff_views::teardown_diff_view(
                            store,
                            previous.pane.documents(),
                            previous.pane.id(),
                        );
                    }
                    self.successions
                        .insert_mut(key, Succession { pane, built_width });
                }
                return;
            }
        }
        // A relaunch (a stale row rebuilding) replaces a Built row:
        // untrack the standing diff before the fresh mount, or it
        // leaks a tracked pair + editors (docs/editor/diff-canvas.md §7).
        self.teardown_row(store, &key);
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let route = key.clone();
        let built_width = prep.width;
        let (body, body_height) = match prep.failed {
            true => (
                RowBody::Failed(format!("contents unavailable: {}", file.title)),
                chrome.title_size * 3.0,
            ),
            false => {
                let mounted = fx.scope(
                    move |command: RowCommand| CanvasCommand::ToRow {
                        key: route.clone(),
                        command,
                    },
                    |fx| mounted(store, self.changes, ui, prep, fx),
                );
                match mounted {
                    // A fresh mount is a SEED — the row keeps the
                    // skeleton face and its RESERVED band until the
                    // dressing lands whole, so splice at the reserved
                    // height: the only height move is the final one
                    // (`resize_row`, once the view answers dressed).
                    Some((pane, height)) => {
                        self.pairs.insert_mut(pane.id(), key.clone());
                        let dressed =
                            crate::diff_pane::gathered_view(store, pane.documents(), pane.id())
                                .is_none_or(|view| view.dressed());
                        let height = match dressed {
                            true => height,
                            false => (reserved_body(&theme, &file) - chrome.gap).max(0.0),
                        };
                        (RowBody::Built { pane }, height)
                    }
                    None => (
                        RowBody::Failed("could not open the diff".to_owned()),
                        chrome.title_size * 3.0,
                    ),
                }
            }
        };
        self.phases.insert_mut(
            key.clone(),
            match &body {
                RowBody::Failed(_) => RowPhase::Failed,
                _ => RowPhase::Built,
            },
        );
        let diff = CanvasRow::Diff(DiffRow {
            file: file.clone(),
            body,
            rewrap_ask: None,
            built_width: Some(built_width),
        });
        let diff_height = body_height + chrome.gap;

        if let Some(header_range) = self.rows.content().row_range(&CanvasKey::File(key.clone())) {
            let start = header_range.start;
            let expanded = self
                .rows
                .content()
                .row_range(&CanvasKey::Diff(key.clone()))
                .is_some();
            let header = CanvasRow::Header(HeaderRow {
                file,
                collapsed: !expanded,
                built: true,
            });
            let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
            slice.push_keyed_sized(CanvasKey::File(key.clone()), header, header_band(&theme));
            if expanded {
                slice.push_keyed_sized(CanvasKey::Diff(key.clone()), diff, diff_height);
                slice.cover(CanvasKey::File(key.clone()), 0..2);
                self.rows
                    .content_mut()
                    .splice_slice(start..start + 2, slice);
            } else {
                // Collapsed before the build landed: park the built
                // row; expand splices it in.
                slice.cover(CanvasKey::File(key.clone()), 0..1);
                self.stash.insert_mut(key.clone(), (diff, diff_height));
                self.rows
                    .content_mut()
                    .splice_slice(start..start + 1, slice);
            }
            // The swap is a height mutation like any other: the door
            // noted the anchor, the pulse re-aims before this frame
            // paints. Zero wrong frames (docs/editor/diff-canvas.md §4).
            fx.settle();
        }
    }

    fn toggle_collapse(&mut self, key: &ResourceLocation, fx: &mut Effects<'_, CanvasCommand>) {
        let Some(header_range) = self.rows.content().row_range(&CanvasKey::File(key.clone()))
        else {
            return;
        };
        let start = header_range.start;
        let Some(file) = self.files.get(key).cloned() else {
            return;
        };
        let built = matches!(
            self.phases.get(key),
            Some(RowPhase::Built) | Some(RowPhase::Failed)
        );
        let band = self.rows.content().height_at(start).unwrap_or(64.0);
        match self.rows.content().row_range(&CanvasKey::Diff(key.clone())) {
            Some(diff_range) => {
                let Some(diff) = self.rows.content().view_at(diff_range.start) else {
                    return;
                };
                let height = self
                    .rows
                    .content()
                    .height_at(diff_range.start)
                    .unwrap_or(0.0);
                self.stash.insert_mut(key.clone(), (diff, height));
                let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
                slice.push_keyed_sized(
                    CanvasKey::File(key.clone()),
                    CanvasRow::Header(HeaderRow {
                        file,
                        collapsed: true,
                        built,
                    }),
                    band,
                );
                slice.cover(CanvasKey::File(key.clone()), 0..1);
                self.rows
                    .content_mut()
                    .splice_slice(start..start + 2, slice);
            }
            None => {
                let (diff, height) = match self.stash.get(key) {
                    Some((diff, height)) => (diff.clone(), *height),
                    None => return,
                };
                self.stash.remove_mut(key);
                let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
                slice.push_keyed_sized(
                    CanvasKey::File(key.clone()),
                    CanvasRow::Header(HeaderRow {
                        file,
                        collapsed: false,
                        built,
                    }),
                    band,
                );
                slice.push_keyed_sized(CanvasKey::Diff(key.clone()), diff, height);
                slice.cover(CanvasKey::File(key.clone()), 0..2);
                self.rows
                    .content_mut()
                    .splice_slice(start..start + 1, slice);
            }
        }
        fx.settle();
    }

    /// A header ask, wherever it was pressed — the in-flow row or the
    /// planted sticky copy route identically.
    fn header_action(
        &mut self,
        key: &ResourceLocation,
        action: HeaderAction,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        match action {
            HeaderAction::OpenFile => {
                if let Some(file) = self.files.get(key) {
                    // Land on the caret the row's diff editor holds —
                    // cmd-enter continues where the user was reading,
                    // like the standalone split-diff pane. The shell
                    // supplies the window at the drain.
                    self.request = Some(hikit::panel::PanelRequest::OpenAt(
                        file.new.clone(),
                        self.row_caret(store, key),
                    ));
                }
            }
            HeaderAction::OpenPane => {
                if let Some(file) = self.files.get(key) {
                    // The standalone pane for this pair — the shell
                    // supplies the window at the drain.
                    self.request = Some(hikit::panel::PanelRequest::OpenDiff(
                        file.old.clone(),
                        file.new.clone(),
                    ));
                }
            }
            HeaderAction::ToggleCollapse => self.toggle_collapse(key, fx),
            HeaderAction::ToggleFace => {
                let command = RowCommand::Header(HeaderAction::ToggleFace);
                self.to_row(key.clone(), command, store, ui, fx);
            }
        }
    }

    /// The caret position (target-side) of a built row's diff editor,
    /// as a `LineCol` range — the cmd-enter navigation target. `None`
    /// for an unbuilt row (nothing focused yet) or a missing pane.
    fn row_caret(
        &self,
        store: &Store,
        key: &ResourceLocation,
    ) -> Option<std::ops::Range<documents::text_ext::LineCol>> {
        let pane = self
            .rows
            .content()
            .row_range(&CanvasKey::Diff(key.clone()))
            .and_then(|range| self.rows.content().view_at(range.start))
            .or_else(|| self.stash.get(key).map(|(row, _)| row.clone()))?;
        let CanvasRow::Diff(DiffRow {
            body: RowBody::Built { pane },
            ..
        }) = pane
        else {
            return None;
        };
        let view = documents::OpenDocuments::diff_view_ref(store, pane.documents(), pane.id())?;
        let right = view.right;
        // The canvas shows the INLINE face by default, where the
        // user's caret lives on the inline editor; both it and the
        // split-right editor ride the same (target) document.
        let editor = view
            .state
            .as_ref()
            .and_then(|state| state.inline_editor())
            .unwrap_or_else(|| right.editor());
        let document =
            documents::OpenDocuments::document_ref(store, right.documents(), right.document())?;
        let byte = document.caret_byte(editor);
        let mut text = document.text().view();
        let at = documents::text_ext::line_col_at(&mut text, byte as usize);
        Some(at..at)
    }

    /// Route a row command by KEY: to the live diff row, or into the
    /// parked one while its file is collapsed.
    fn to_row(
        &mut self,
        key: ResourceLocation,
        command: RowCommand,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        match self.rows.content().row_range(&CanvasKey::Diff(key.clone())) {
            Some(range) => {
                let index = range.start;
                let rows = ScrollCommand::Content(ListCommand::Child(index, command));
                fx.scope(CanvasCommand::Rows, |fx| {
                    self.rows.perform(store, ui, rows, fx)
                });
                // A face flip / fold toggle routed here changes the
                // row's body height — resize it afterward.
                self.resize_row(index, store, ui, fx);
            }
            None => {
                let Some((mut row, height)) = self.stash.get(&key).cloned() else {
                    return;
                };
                let route = key.clone();
                fx.scope(
                    move |command: RowCommand| CanvasCommand::ToRow {
                        key: route.clone(),
                        command,
                    },
                    |fx| row.perform(store, ui, command, fx),
                );
                self.stash.insert_mut(key, (row, height));
            }
        }
    }
}

/// The whole off-thread build arrives here; this mounts it — bounded
/// editors over the PRE-BUILT documents, the seeded pair road exactly
/// as the chat diff cell mounts (higent/cell.rs `resolve_diff`).
/// A row-level ask inside a rows command, seen from the panel — digs
/// through the list's `Focus(_, Some(..))` press wrapping.
fn row_ask(command: &RowsCommand) -> Option<(usize, &RowCommand)> {
    let ScrollCommand::Content(inner) = command else {
        return None;
    };
    fn dig<C>(list: &ListCommand<C>) -> Option<(usize, &C)> {
        match list {
            ListCommand::Child(index, command) => Some((*index, command)),
            ListCommand::Focus(_, Some(inner)) => dig(inner),
            _ => None,
        }
    }
    dig(inner)
}

/// Mount a row's diff over REGISTERED documents through the SHARED
/// install road (docs/editor/diff-canvas.md §7): `install_opened_pair`
/// reuses each open side and registers each freshly-built one, then
/// `build_diff_view` tracks the pair (rebasing the prepared op to the
/// live pair) and mints a store-held `DiffView`. Exactly the road the
/// split-diff pane runs — the row is just the embedded face.
fn mounted(
    store: &mut Store,
    changes: imba::store::Id<Changes>,
    ui: &UiCtx,
    prep: documents::diff_views::OpenedDiffPair,
    fx: &mut Effects<'_, RowCommand>,
) -> Option<(crate::diff_pane::PairPane, f32)> {
    let theme = env::Themes::of(store);
    let gutter = theme.ui().editor_gutter.width;
    let editor_width = (prep.width - gutter).max(120.0);

    let documents = Changes::of(store, changes)?.documents();
    let id = documents::diff_views::install_opened_pair(store, documents, ui, prep, true)?;
    let mut pane = crate::diff_pane::PairPane::over(documents, id);

    // Default to the inline face.
    fx.scope(RowCommand::Diff, |fx| {
        pane.perform(
            store,
            ui,
            UnifiedDiffCommand::SetLayout(editor::unified_diff::DiffLayout::Inline),
            fx,
        )
    });

    let height = {
        let frame = Arena::default();
        let thunk = imba::layout::Layout::layout(
            pane.display(&frame, store, ui),
            &frame,
            Constraints {
                min: Size::new(editor_width, 0.0),
                max: Size::new(editor_width + gutter, f32::MAX),
            },
        );
        Thunk::size(&thunk).height
    };
    Some((pane, height))
}

impl Canvas {
    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, CanvasCommand> {
        self.rows.focus_data(store, ui).map(CanvasCommand::Rows)
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: CanvasCommand,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        match command {
            CanvasCommand::Landed { key, prep } => self.land(store, ui, key, prep, fx),
            CanvasCommand::ToRow { key, command } => self.to_row(key, command, store, ui, fx),
            CanvasCommand::ToPending { key, command } => {
                self.to_pending(key, command, store, ui, fx)
            }
            CanvasCommand::BannerEditor(command) => {
                if let Some(range) = self.rows.content().row_range(&CanvasKey::Banner) {
                    let rows = ScrollCommand::Content(ListCommand::Child(
                        range.start,
                        RowCommand::Composer(ComposerCommand::Message(command)),
                    ));
                    fx.scope(CanvasCommand::Rows, |fx| {
                        self.rows.perform(store, ui, rows, fx)
                    });
                }
            }
            CanvasCommand::Rows(command) => {
                match row_ask(&command) {
                    Some((index, RowCommand::Arm(width))) => self.launch(store, index, *width, fx),
                    Some((index, RowCommand::Header(action))) => {
                        let key = match self.rows.content().key_at(index) {
                            Some(CanvasKey::File(location)) | Some(CanvasKey::Diff(location)) => {
                                Some(location.clone())
                            }
                            _ => None,
                        };
                        if let Some(key) = key {
                            let action = *action;
                            self.header_action(&key, action, store, ui, fx);
                        }
                    }
                    // The canvas posts the commit ask (it owns the
                    // folder and the request); the text is read HERE,
                    // before the routed command resets the row's box.
                    Some((_, RowCommand::Composer(ComposerCommand::Commit))) => {
                        let text = self.composer_text().unwrap_or_default();
                        let history = Changes::of(store, self.changes).map(|held| held.history());
                        if let (false, Some(history)) = (text.trim().is_empty(), history) {
                            self.request = Some(hikit::panel::PanelRequest::Perform(
                                std::sync::Arc::new(changesview::hihistory::CommitHistory {
                                    history,
                                    folder: self.source.folder().clone(),
                                    message: text,
                                }),
                            ));
                        }
                    }
                    _ => {}
                }
                // A command that reaches a diff row (a repair, a fold
                // toggle, a face flip, the diff's own Resync) may change
                // its body height — resize exactly that row afterward.
                let touched = match row_ask(&command) {
                    Some((index, RowCommand::Diff(_) | RowCommand::Rewrap(_))) => Some(index),
                    _ => None,
                };
                fx.scope(CanvasCommand::Rows, |fx| {
                    self.rows.perform(store, ui, command, fx)
                });
                if let Some(index) = touched {
                    self.resize_row(index, store, ui, fx);
                }
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, CanvasCommand> + imba::layout::LayoutValue + 'a {
        let _ = arena;
        imba::layout::laid(move |arena: &'a Arena, constraints: Constraints| {
            let inner: imba::ThunkBox<'a, CanvasCommand> = match &self.note {
                Some(note) => {
                    let chrome = env::Themes::of(store).ui().chat.clone();
                    let text = note.clone();
                    let font = hikit::fonts::ui_text_font(ui, chrome.title_size);
                    let shaper = imba::layout::TextShaper::of(ui);
                    let color = chrome.loader_color.0;
                    let size = constraints.max;
                    imba::ThunkBox::new(
                        arena,
                        imba::leaf::leaf::<CanvasCommand>(size.width, size.height.min(240.0))
                            .paint_instead(move |_arena, canvas, rect| {
                                let width = shaper.advance(&font, &text);
                                shaper.draw(
                                    canvas,
                                    &font,
                                    &text,
                                    color,
                                    0.0,
                                    rect.left + (rect.width() - width) / 2.0,
                                    rect.top + rect.height() * 0.5,
                                );
                            }),
                    )
                }
                None => imba::ThunkBox::new(
                    arena,
                    imba::layout::Layout::layout(
                        self.rows.display(arena, store, ui),
                        arena,
                        constraints,
                    )
                    .map(CanvasCommand::Rows)
                    // The list plants its sticky headers here —
                    // the canvas IS the pane face, so the band
                    // spans it edge to edge.
                    .overlay_host(imba::list::STICKY_HOST),
                ),
            };
            inner
        })
    }
}

// ------------------------------------------------------------ canvases

use changesview::hichanges::{CanvasId, CanvasSource};

/// Stateless FACADE over the sets' owned canvases
/// (docs/model-view.md: `ChangeSet.canvases`): at most one canvas per
/// source — and a source IS a set now, so reuse is the set lookup.
/// Canvases live while views retain them; the working-copy canvas
/// stays once opened.
pub struct Canvases;

use changesview::hichanges::{ChangeSetId, ChangeSetSource, Changes};

fn set_source(source: &CanvasSource) -> ChangeSetSource {
    match source {
        CanvasSource::WorkingCopy { folder } => ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        },
        CanvasSource::Commit { folder, id } => ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: id.clone(),
        },
    }
}

impl Canvases {
    pub fn by_source(
        store: &Store,
        changes: imba::store::Id<Changes>,
        source: &CanvasSource,
    ) -> Option<CanvasId> {
        let set = Changes::id_for_source(store, changes, &set_source(source))?;
        Changes::set_ref(store, changes, set)?
            .canvases
            .keys()
            .next()
            .copied()
    }

    fn owner_of(
        store: &Store,
        changes: imba::store::Id<Changes>,
        id: CanvasId,
    ) -> Option<ChangeSetId> {
        Changes::canvas_ids(store, changes)
            .into_iter()
            .find(|(_, canvas)| *canvas == id)
            .map(|(set, _)| set)
    }

    fn find_or_create(
        store: &mut Store,
        changes: imba::store::Id<Changes>,
        source: &CanvasSource,
    ) -> CanvasId {
        let set = Changes::ensure_set_for_source(store, changes, &set_source(source));
        if let Some(id) = Changes::set_ref(store, changes, set)
            .and_then(|held| held.canvases.keys().next().copied())
        {
            return id;
        }
        let id = CanvasId::mint();
        Changes::put_canvas(
            store,
            changes,
            set,
            id,
            Canvas::fresh(changes, source.clone()),
        );
        id
    }

    fn get<'a>(
        store: &'a Store,
        changes: imba::store::Id<Changes>,
        id: CanvasId,
    ) -> Option<&'a Canvas> {
        let set = Self::owner_of(store, changes, id)?;
        Changes::canvas_ref(store, changes, set, id)
    }

    fn take(store: &mut Store, changes: imba::store::Id<Changes>, id: CanvasId) -> Option<Canvas> {
        let set = Self::owner_of(store, changes, id)?;
        Changes::take_canvas(store, changes, set, id)
    }

    fn put(store: &mut Store, changes: imba::store::Id<Changes>, id: CanvasId, canvas: Canvas) {
        // A put without a surviving owner re-homes by source (the set
        // always exists — sources mint their sets).
        let set = Self::owner_of(store, changes, id).unwrap_or_else(|| {
            Changes::ensure_set_for_source(store, changes, &set_source(&canvas.source))
        });
        Changes::put_canvas(store, changes, set, id, canvas);
    }

    fn retain(store: &mut Store, changes: imba::store::Id<Changes>, id: CanvasId) {
        if let Some(mut canvas) = Self::take(store, changes, id) {
            canvas.refs += 1;
            Self::put(store, changes, id, canvas);
        }
    }

    fn release(store: &mut Store, changes: imba::store::Id<Changes>, id: CanvasId) {
        let Some(mut canvas) = Self::take(store, changes, id) else {
            return;
        };
        canvas.refs = canvas.refs.saturating_sub(1);
        // View-retained lifetime — except the WORKING-COPY canvas,
        // which stays around once opened (its diffs keep serving the
        // next open for free).
        if canvas.refs > 0 || matches!(canvas.source, CanvasSource::WorkingCopy { .. }) {
            Self::put(store, changes, id, canvas);
            return;
        }
        // The last view is gone. The rows' substance — documents,
        // editors, tracked diffs — lives in the session's shared
        // OpenDocuments, not in this struct: dropping the canvas alone
        // leaks all of it for the session's life. Tear every row down:
        // live in the list, parked in the collapse stash, and the
        // off-row successors alike.
        let keys: Vec<ResourceLocation> = canvas.files.keys().cloned().collect();
        for key in keys {
            canvas.teardown_row(store, &key);
        }
    }

    pub(crate) fn set_reveal(
        store: &mut Store,
        changes: imba::store::Id<Changes>,
        id: CanvasId,
        key: ResourceLocation,
    ) {
        if let Some(mut canvas) = Self::take(store, changes, id) {
            canvas.reveal = Some(key);
            Self::put(store, changes, id, canvas);
        }
    }
}

/// The canvases' own At address — a stateless ROUTER row minted by
/// the session ceremony beside the collection: canvas commands land
/// on canvas code here, and the model (which stores canvases as
/// SLOTS) never calls up into a face.
#[derive(Clone)]
pub struct CanvasRouter {
    changes: imba::store::Id<Changes>,
}

impl CanvasRouter {
    pub fn wired(changes: imba::store::Id<Changes>) -> Self {
        Self { changes }
    }

    pub fn is_empty(&self) -> bool {
        true
    }
}

#[derive(Clone)]
pub enum CanvasRouted {
    Canvas(ChangeSetId, CanvasId, Box<CanvasCommand>),
}

impl std::fmt::Display for CanvasRouted {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CanvasRouted::Canvas(_, _, command) => command.fmt(out),
        }
    }
}

impl imba::store::Entity for CanvasRouter {
    type Command = CanvasRouted;

    fn perform(
        &mut self,
        _id: imba::store::Id<Self>,
        command: CanvasRouted,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        fx: &mut imba::effect::Effects<'_, CanvasRouted>,
    ) {
        // The router row is stateless: the canvases live on the SETS
        // (slot doors), so no lease escape is owed here.
        match command {
            CanvasRouted::Canvas(set, canvas, command) => {
                perform_canvas(store, ui, self.changes, set, canvas, *command, fx);
            }
        }
    }

    fn destroy(&mut self, _store: &mut Store) {}
}

/// The batch-tail canvas sweep — a DIRECT lane now (the sets own their
/// canvases; no plugin observer, no window): reconcile every canvas in
/// the gathered session, launching builds through the canvases' own At
/// address (`CanvasRouted`, the DiffView shape).
pub fn sync_canvases(
    store: &mut Store,
    router: imba::store::Id<CanvasRouter>,
    ui: &UiCtx,
    dressed: &[documents::diffs::DiffViewId],
    fx: &mut imba::command::Fx<'_>,
) {
    let Some(changes) = store.entity::<CanvasRouter>(router).map(|row| row.changes) else {
        return;
    };
    fx.scope(
        move |command| imba::command::Verb::at(router, command),
        |fx| {
            for (set, id) in Changes::canvas_ids(store, changes) {
                let Some(mut canvas) = Changes::take_canvas::<Canvas>(store, changes, set, id)
                else {
                    continue;
                };
                let route = route_canvas(set, id);
                fx.scope(route, |fx| canvas.sync_in_place(store, ui, &dressed, fx));
                Changes::put_canvas(store, changes, set, id, canvas);
            }
        },
    );
}

/// Map a canvas's commands home BY IDS — the set and the canvas; the
/// router is the At address wrapping outside (no window, no landing
/// box, no session).
fn route_canvas(
    set: ChangeSetId,
    canvas: CanvasId,
) -> impl Fn(CanvasCommand) -> CanvasRouted + Clone {
    move |command| CanvasRouted::Canvas(set, canvas, Box::new(command))
}

/// Perform one command against a SET-OWNED canvas — the panel-free
/// road (`perform_diff_view`'s twin), reached through the router.
pub(crate) fn perform_canvas(
    store: &mut Store,
    ui: &UiCtx,
    changes: imba::store::Id<Changes>,
    set: ChangeSetId,
    id: CanvasId,
    command: CanvasCommand,
    fx: &mut imba::effect::Effects<'_, CanvasRouted>,
) {
    let Some(mut canvas) = Changes::take_canvas::<Canvas>(store, changes, set, id) else {
        return;
    };
    let route = route_canvas(set, id);
    fx.scope(route, |fx| canvas.perform(store, ui, command, fx));
    Changes::put_canvas(store, changes, set, id, canvas);
}

/// The canvas PANEL — a REFERENCE view over the store-held canvas,
/// the `PairPane` shape: panes hold ids, state lives in `Canvases`,
/// and a second view of the same source costs nothing.
#[derive(Clone)]
pub struct DiffCanvasView {
    id: CanvasId,
    /// The collection that owns this canvas's set.
    changes: imba::store::Id<Changes>,
    source: CanvasSource,
    request: Option<hikit::panel::PanelRequest>,
}

impl DiffCanvasView {
    pub fn over(
        store: &mut Store,
        changes: imba::store::Id<Changes>,
        source: CanvasSource,
    ) -> Self {
        let id = Canvases::find_or_create(store, changes, &source);
        Canvases::retain(store, changes, id);
        Self {
            id,
            changes,
            source,
            request: None,
        }
    }

    pub fn id(&self) -> CanvasId {
        self.id
    }

    pub fn source(&self) -> &CanvasSource {
        &self.source
    }

    fn canvas<'a>(&self, store: &'a Store) -> Option<&'a Canvas> {
        Canvases::get(store, self.changes, self.id)
    }

    /// TEST SUPPORT: a view over a canvas seeded with one BUILT row.
    #[doc(hidden)]
    pub fn seeded_for_tests(
        store: &mut Store,
        ui: &UiCtx,
        changes: imba::store::Id<Changes>,
        source: CanvasSource,
        file: CanvasFile,
        prep: documents::diff_views::OpenedDiffPair,
    ) -> Self {
        let view = Self::over(store, changes, source);
        if let Some(mut canvas) = Canvases::take(store, view.changes, view.id) {
            canvas.seed_built_for_tests(store, ui, file, prep);
            Canvases::put(store, view.changes, view.id, canvas);
        }
        view
    }

    /// TEST SUPPORT: the registered `DiffViewId` backing a built row.
    #[doc(hidden)]
    pub fn probe_pair(
        &self,
        store: &Store,
        key: &editor::location::ResourceLocation,
    ) -> Option<documents::diffs::DiffViewId> {
        self.canvas(store)?.probe_pair(key)
    }

    #[doc(hidden)]
    pub fn probe_cover(
        &self,
        store: &Store,
        key: &editor::location::ResourceLocation,
    ) -> Option<usize> {
        self.canvas(store)?.probe_cover(key)
    }

    #[doc(hidden)]
    pub fn probe_row_keys(&self, store: &Store) -> Vec<String> {
        self.canvas(store)
            .map(|c| c.probe_row_keys())
            .unwrap_or_default()
    }

    /// TEST SUPPORT: drive the REAL listing adoption (the branch the
    /// paint probe and `Canvases::sync` reach through `refresh`).
    #[doc(hidden)]
    pub fn adopt_for_tests(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        generation: u64,
        listing: crate::diff_canvas::CanvasListing,
    ) {
        if let Some(mut canvas) = Canvases::take(store, self.changes, self.id) {
            let mut batch = imba::effect::Batch::new();
            canvas.adopt(store, ui, generation, listing, &mut batch.effects());
            Canvases::put(store, self.changes, self.id, canvas);
        }
    }

    /// TEST SUPPORT: feed a fresh listing straight into the reconcile
    /// (bypassing the Changes feed); the reconcile relaunches stamp-
    /// moved rows through the sink itself now, so this just counts the
    /// builds it launched.
    #[doc(hidden)]
    pub fn reconcile_for_tests(&self, store: &mut Store, files: Vec<CanvasFile>) -> usize {
        let mut launched = 0;
        if let Some(mut canvas) = Canvases::take(store, self.changes, self.id) {
            let mut batch = imba::effect::Batch::new();
            {
                let mut fx = batch.effects();
                canvas.reconcile(store, files, &mut fx);
            }
            // The settle pulse rides the same channel — strip it, count
            // only real builds.
            let _ = batch.take_settle();
            launched = batch
                .drain()
                .into_iter()
                .filter(|message| {
                    matches!(
                        message,
                        imba::effect::Message::Launch(..) | imba::effect::Message::Relaunch(..)
                    )
                })
                .count();
            Canvases::put(store, self.changes, self.id, canvas);
        }
        launched
    }

    /// TEST SUPPORT: arm one row by its file key at a width, as its
    /// first paint would — returns how many builds the arm launched
    /// (the row itself plus the prefetch horizon behind it).
    #[doc(hidden)]
    pub fn arm_for_tests(&self, store: &mut Store, key: &ResourceLocation, width: f32) -> usize {
        let mut launched = 0;
        if let Some(mut canvas) = Canvases::take(store, self.changes, self.id) {
            let mut batch = imba::effect::Batch::new();
            {
                let mut fx = batch.effects();
                if let Some(range) = canvas
                    .rows
                    .content()
                    .row_range(&CanvasKey::Diff(key.clone()))
                {
                    canvas.launch(store, range.start, width, &mut fx);
                }
            }
            let _ = batch.take_settle();
            launched = batch
                .drain()
                .into_iter()
                .filter(|message| {
                    matches!(
                        message,
                        imba::effect::Message::Launch(..) | imba::effect::Message::Relaunch(..)
                    )
                })
                .count();
            Canvases::put(store, self.changes, self.id, canvas);
        }
        launched
    }

    /// TEST SUPPORT: run the batch-tail prefetch pump once — returns
    /// how many speculative opens it launched (0 or 1).
    #[doc(hidden)]
    pub fn pump_for_tests(&self, store: &mut Store) -> usize {
        let mut launched = 0;
        if let Some(mut canvas) = Canvases::take(store, self.changes, self.id) {
            let mut batch = imba::effect::Batch::new();
            {
                let mut fx = batch.effects();
                canvas.pump_prefetch(store, &mut fx);
            }
            let _ = batch.take_settle();
            launched = batch
                .drain()
                .into_iter()
                .filter(|message| {
                    matches!(
                        message,
                        imba::effect::Message::Launch(..) | imba::effect::Message::Relaunch(..)
                    )
                })
                .count();
            Canvases::put(store, self.changes, self.id, canvas);
        }
        launched
    }

    /// TEST SUPPORT: the queued prefetch horizon, in drain order.
    #[doc(hidden)]
    pub fn probe_prefetch_queue(&self, store: &Store) -> Vec<editor::location::ResourceLocation> {
        self.canvas(store)
            .map(|canvas| canvas.prefetch_queue.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// TEST SUPPORT: land a build for a key, as the effect would.
    #[doc(hidden)]
    pub fn land_for_tests(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        key: editor::location::ResourceLocation,
        prep: documents::diff_views::OpenedDiffPair,
    ) {
        if let Some(mut canvas) = Canvases::take(store, self.changes, self.id) {
            let mut batch = imba::effect::Batch::new();
            canvas.land(store, ui, key, prep, &mut batch.effects());
            Canvases::put(store, self.changes, self.id, canvas);
        }
    }

    #[doc(hidden)]
    pub fn probe_note(&self, store: &Store) -> Option<String> {
        self.canvas(store)?.probe_note().map(str::to_owned)
    }

    #[doc(hidden)]
    pub fn probe_scroll_top(&self, store: &Store) -> f32 {
        self.canvas(store)
            .map(|canvas| canvas.probe_scroll_top())
            .unwrap_or(0.0)
    }

    #[doc(hidden)]
    pub fn probe_rows(&self, store: &Store) -> Vec<(String, RowPhase, f32)> {
        self.canvas(store)
            .map(|canvas| canvas.probe_rows())
            .unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn probe_composer(&self, store: &Store) -> Option<(bool, String)> {
        self.canvas(store)?.probe_composer()
    }

    #[doc(hidden)]
    pub fn probe_banner(&self, store: &Store) -> Option<(String, String)> {
        self.canvas(store)?.probe_banner()
    }

    #[doc(hidden)]
    pub fn probe_layouts(&self, store: &Store) -> Vec<(String, editor::unified_diff::DiffLayout)> {
        self.canvas(store)
            .map(|canvas| canvas.probe_layouts(store))
            .unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn probe_half_heights(&self, store: &Store) -> Vec<(f32, f32, f32, f32, f32)> {
        self.canvas(store)
            .map(|canvas| canvas.probe_half_heights(store))
            .unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn probe_focus(&self, store: &Store) -> Vec<(String, String, Vec<String>)> {
        self.canvas(store)
            .map(|canvas| canvas.probe_focus(store))
            .unwrap_or_default()
    }
}

impl View for DiffCanvasView {
    type Command = CanvasCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, CanvasCommand> {
        match self.canvas(store) {
            Some(canvas) => canvas.focus_data(store, ui),
            None => imba::focus::FocusData::default(),
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        let Some(mut canvas) = Canvases::take(store, self.changes, self.id) else {
            return;
        };
        canvas.perform(store, ui, command, fx);
        // Requests are the PANEL's ask (`take_request` has no store):
        // pull what the canvas minted into the view.
        if let Some(request) = canvas.request.take() {
            self.request = Some(request);
        }
        Canvases::put(store, self.changes, self.id, canvas);
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |arena: &'a Arena, constraints: Constraints| {
            match self.canvas(store) {
                Some(canvas) => imba::layout::Layout::layout(
                    canvas.display(arena, store, ui),
                    arena,
                    constraints,
                ),
                None => imba::ThunkBox::new(
                    arena,
                    imba::leaf::leaf::<CanvasCommand>(constraints.max.width.max(1.0), 1.0),
                ),
            }
        })
    }
}

// ---------------------------------------------------------------- rows

/// What a header row's face offers — pressed on the in-flow row or on
/// its planted sticky copy alike.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeaderAction {
    /// The header body (and cmd-enter): open the live file in a pane.
    OpenFile,

    /// The side-by-side button: the standalone diff pane.
    OpenPane,

    /// The layout button: inline ⇄ split for this file's diff.
    ToggleFace,

    /// The chevron: fold the diff row away / bring it back.
    ToggleCollapse,
}

#[derive(Clone)]
pub enum RowCommand {
    /// The placeholder painted un-armed: build me, at this width.
    Arm(f32),

    Header(HeaderAction),

    Diff(UnifiedDiffCommand),

    Composer(ComposerCommand),

    Rewrap(f32),
}

impl std::fmt::Display for RowCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RowCommand::Diff(command) => command.fmt(out),
            RowCommand::Composer(command) => command.fmt(out),
            RowCommand::Arm(_) => out.write_str("row arm"),
            RowCommand::Header(_) => out.write_str("row header"),
            RowCommand::Rewrap(_) => out.write_str("row rewrap"),
        }
    }
}

#[derive(Clone)]
pub enum ComposerCommand {
    Message(editor::editor_view::EditorCommand),

    /// A press on the well outside the editor's own face.
    Focus,

    /// ⌘⏎ or the COMMIT button. The CANVAS posts the CommitHistory
    /// ask (it owns the source folder and the panel request); the row
    /// then resets its box.
    Commit,
}

impl std::fmt::Display for ComposerCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposerCommand::Message(command) => command.fmt(out),
            ComposerCommand::Focus => out.write_str("composer focus"),
            ComposerCommand::Commit => out.write_str("composer commit"),
        }
    }
}

#[derive(Clone)]
enum RowBody {
    Placeholder { armed: bool },
    Failed(String),
    Built { pane: crate::diff_pane::PairPane },
}

#[derive(Clone)]
pub enum CanvasRow {
    Banner(BannerRow),
    Header(HeaderRow),
    Diff(DiffRow),
}

/// The canvas's first row (docs/editor/diff-canvas.md): the commit's message
/// and author on a commit canvas, the commit composer on the
/// working-copy canvas.
#[derive(Clone)]
pub enum BannerRow {
    Commit {
        message: editor::editor_view::EditorView,
        author: String,
        focused: bool,
    },
    Composer {
        message: editor::editor_view::EditorView,
        focused: bool,
    },
}

#[derive(Clone)]
pub struct HeaderRow {
    file: CanvasFile,
    collapsed: bool,

    /// Whether a diff stands behind this header — the face toggle
    /// only shows then.
    built: bool,
}

#[derive(Clone)]
pub struct DiffRow {
    file: CanvasFile,
    body: RowBody,

    /// The last rewrap target seen. A live window drag changes the
    /// width EVERY frame; resizing three editors per row per frame
    /// (plus the SetHeight/settle churn each resize drags in) is the
    /// resize storm. A rewrap only runs once the same target arrives
    /// twice — i.e. the width held still for a frame.
    rewrap_ask: Option<f32>,

    /// The width the standing build ran at — the reconcile relaunches
    /// a stale row at this width so the fresh build lands into the
    /// same geometry (the old view keeps showing until it does).
    built_width: Option<f32>,
}

fn header_band(theme: &editor::theme::Theme) -> f32 {
    let h1 = theme.resolve([editor::theme::StyleId::Header(1)]);
    h1.font_size.unwrap_or(48.0) + h1.block_gap.unwrap_or(24.0)
}

/// The composer banner's height — the CHAT COMPOSER's whole footprint
/// (higent/chat.rs): the input band (one chat line at rest, `box_pad`
/// = pad × 0.75 above and below) and the TOOLBAR row under it, ruled
/// off. The box grows UNBOUNDED with the message — a commit message
/// is as long as its author wants it; the canvas just scrolls.
fn composer_band(
    theme: &editor::theme::Theme,
    message: Option<&editor::editor_view::EditorView>,
) -> f32 {
    let chat = theme.ui().chat.clone();
    let one_line = chat.title_size * 1.6;
    let grown = message
        .map(|editor| editor.content_height())
        .unwrap_or(one_line)
        .max(one_line);
    grown + chat.pad * 1.5 + theme.ui().toolbar.height
}

/// A fresh commit box — the CHAT composer's input recipe
/// (higent/composer.rs `fresh_input`): a markdown document, the
/// placeholder the editor's own.
fn fresh_composer_box(store: &Store, ui: &imba::ui::UiCtx) -> editor::editor_view::EditorView {
    let fonts = env::Fonts::of(store)();
    let theme = env::Themes::of(store);
    let document = editor::document::Document::new(
        text::text::Text::from_string_exact(""),
        editor::markup::Markup::new(),
    )
    .with_syntax(
        editor::markup::Syntax::new("markdown", None, editor::markup::Markup::new()),
        &[],
    );
    let mut view =
        editor::editor_view::EditorView::of_document(document, 600.0, store, ui, &fonts, &theme);
    view.set_placeholder("Commit message", &fonts, &theme);
    view
}

/// The commit banner's message box — the CHAT CELL's markdown recipe
/// (higent/cell.rs `build_text`): a markdown document over the exact
/// message, a Bounded build whose tail repairs and reparse land over
/// the `BannerEditor` road. A normal editor: it focuses on click and
/// takes the keyboard like the composer's box.
fn commit_banner_box(
    store: &Store,
    ui: &imba::ui::UiCtx,
    message: &str,
    fx: &mut Effects<'_, CanvasCommand>,
) -> editor::editor_view::EditorView {
    let fonts = env::Fonts::of(store)();
    let theme = env::Themes::of(store);
    let mut document = editor::document::Document::new(
        text::text::Text::from_string_exact(message),
        editor::markup::Markup::new(),
    )
    .with_syntax(
        editor::markup::Syntax::new("markdown", None, editor::markup::Markup::new()),
        &[],
    );
    fx.scope(CanvasCommand::BannerEditor, |fx| {
        let editor = document.add_editor(
            600.0,
            None,
            editor::document::EditorBuild::Bounded,
            &[],
            store,
            ui,
            &fonts,
            &theme,
            fx,
        );
        if let Some(parsers) = env::Parsers::of(store) {
            document.launch_reparse(parsers, fx);
        }
        editor::editor_view::EditorView {
            document,
            editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        }
    })
}

fn commit_band(theme: &editor::theme::Theme, message: &editor::editor_view::EditorView) -> f32 {
    let chat = theme.ui().chat.clone();
    let line = chat.title_size * 1.5;
    // The full message and the author line under it — no truncation;
    // the canvas just scrolls.
    message.content_height().max(line) + line + chat.pad * 2.0
}

fn reserved_body(theme: &editor::theme::Theme, file: &CanvasFile) -> f32 {
    let chrome = theme.ui().chat.clone();
    let line = chrome.title_size * 1.5;
    let known = file.added.is_some() || file.removed.is_some();
    let est = match known {
        true => (file.added.unwrap_or(0) + file.removed.unwrap_or(0) + EST_CONTEXT_LINES)
            .clamp(MIN_EST_LINES, MAX_EST_LINES),
        false => 12,
    };
    chrome.gap + est as f32 * line
}

fn open_commands(location: &ResourceLocation) -> Vec<imba::PresentableCommand<RowCommand>> {
    let _ = location;
    vec![
        imba::PresentableCommand::new(
            "workbench.open-in-full",
            "Open File in Full",
            RowCommand::Header(HeaderAction::OpenFile),
        ),
        imba::PresentableCommand::new(
            "diff.open-pane",
            "Diff: Open in Side-by-Side Panel",
            RowCommand::Header(HeaderAction::OpenPane),
        ),
    ]
}

impl View for CanvasRow {
    type Command = RowCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, RowCommand> {
        match self {
            CanvasRow::Banner(BannerRow::Composer { message, .. }) => {
                // ⌘⏎ commits from anywhere in the box; text and keys
                // ride the editor's own focus data.
                let own = imba::focus::FocusData {
                    on_key: Some(Box::new(|key, mods| match key {
                        imba::event::Key::Enter if mods.command => {
                            EventResult::Command(RowCommand::Composer(ComposerCommand::Commit))
                        }
                        _ => EventResult::Ignored,
                    })),
                    ..imba::focus::FocusData::default()
                };
                own.merge_under(
                    message
                        .focus_data(store, ui)
                        .map(|command| RowCommand::Composer(ComposerCommand::Message(command))),
                )
            }
            CanvasRow::Banner(BannerRow::Commit { message, .. }) => message
                .focus_data(store, ui)
                .map(|command| RowCommand::Composer(ComposerCommand::Message(command))),
            CanvasRow::Header(header) => {
                imba::focus::FocusData::of_commands(open_commands(&header.file.new))
            }
            CanvasRow::Diff(diff) => {
                let own = imba::focus::FocusData::of_commands(open_commands(&diff.file.new));
                match &diff.body {
                    RowBody::Built { pane } => pane
                        .focus_data(store, ui)
                        .map(RowCommand::Diff)
                        .merge_under(own),
                    _ => own,
                }
            }
        }
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        let _ = fx;
        if let CanvasRow::Diff(DiffRow {
            body: RowBody::Built { pane },
            ..
        }) = self
        {
            documents::diff_views::teardown_diff_view(store, pane.documents(), pane.id());
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        let diff = match self {
            // Header rows carry no state of their own: header actions
            // are the CANVAS's (it owns the splices and requests).
            CanvasRow::Header(_) => return,
            CanvasRow::Banner(banner) => {
                let (message, focused, composer) = match banner {
                    BannerRow::Composer { message, focused } => (message, focused, true),
                    BannerRow::Commit {
                        message, focused, ..
                    } => (message, focused, false),
                };
                match command {
                    RowCommand::Composer(ComposerCommand::Message(command)) => {
                        if matches!(command, editor::editor_view::EditorCommand::Click { .. })
                            && !*focused
                        {
                            *focused = true;
                            message.focus_text();
                        }
                        fx.scope(
                            |command| RowCommand::Composer(ComposerCommand::Message(command)),
                            |fx| message.perform(store, ui, command, fx),
                        );
                    }
                    RowCommand::Composer(ComposerCommand::Focus) => {
                        *focused = true;
                        message.focus_text();
                    }
                    // The canvas already posted the ask (reading the
                    // text first) — the row just resets its box.
                    RowCommand::Composer(ComposerCommand::Commit) if composer => {
                        *message = fresh_composer_box(store, ui);
                        *focused = false;
                    }
                    // The paint probe saw the box wrapped at the
                    // wrong width (the chat composer's Rewrap ride).
                    RowCommand::Rewrap(width) => {
                        let fonts = env::Fonts::of(store)();
                        let theme = env::Themes::of(store);
                        let editor = message.editor;
                        fx.scope(
                            |command| RowCommand::Composer(ComposerCommand::Message(command)),
                            |fx| {
                                message
                                    .document
                                    .resize(editor, width, 0, store, ui, &fonts, &theme, fx)
                            },
                        );
                    }
                    _ => {}
                }
                return;
            }
            CanvasRow::Diff(diff) => diff,
        };
        match command {
            RowCommand::Header(HeaderAction::ToggleFace) => {
                let RowBody::Built { pane } = &diff.body else {
                    return;
                };
                let Some(view) =
                    crate::diff_pane::gathered_view(store, pane.documents(), pane.id())
                else {
                    return;
                };
                let next = view.layout.other();
                let mut pane = *pane;
                fx.scope(RowCommand::Diff, |fx| {
                    pane.perform(store, ui, UnifiedDiffCommand::SetLayout(next), fx)
                });
            }
            RowCommand::Header(_) | RowCommand::Composer(_) => {}
            RowCommand::Arm(_) => {
                if let RowBody::Placeholder { armed } = &mut diff.body {
                    *armed = true;
                }
            }
            RowCommand::Diff(command) => {
                let RowBody::Built { pane } = &diff.body else {
                    return;
                };
                let mut pane = *pane;
                fx.scope(RowCommand::Diff, |fx| pane.perform(store, ui, command, fx));
            }
            RowCommand::Rewrap(width) => {
                if diff.rewrap_ask != Some(width) {
                    diff.rewrap_ask = Some(width);
                    return;
                }
                let RowBody::Built { pane } = &diff.body else {
                    return;
                };
                let id = pane.id();
                let documents = pane.documents();
                fx.scope(RowCommand::Diff, |fx| {
                    documents::diff_views::rewrap_pair(store, documents, ui, id, width, fx)
                });
            }
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        RowFrame {
            row: self,
            store,
            ui,
        }
    }
}

struct RowFrame<'a> {
    row: &'a CanvasRow,
    store: &'a Store,
    ui: &'a UiCtx,
}

impl imba::layout::LayoutValue for RowFrame<'_> {}

impl<'a> imba::layout::Layout<'a, RowCommand> for RowFrame<'a> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, RowCommand> {
        use imba::layout::LayoutExt as _;
        let RowFrame { row, store, ui } = self;
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let width = constraints.max.width.max(1.0);
        let gutter = theme.ui().editor_gutter.width;

        match row {
            CanvasRow::Banner(BannerRow::Commit {
                message,
                author,
                focused,
            }) => {
                // The MESSAGE rides its own markdown box (the chat
                // cell's dress: headers, emphasis, code — the works),
                // the dim author byline under it. The box wraps at
                // the canvas width over the composer's Rewrap ride.
                let band = commit_band(&theme, message);
                let line = chrome.title_size * 1.5;
                let inset = chrome.pad;
                let body_font = hikit::fonts::ui_text_font(ui, chrome.title_size * 0.9);
                let dim = chrome.loader_color.0;
                let editor_w = (width - inset * 2.0).max(120.0);
                let content = message.content_height().max(line);
                let mut face = imba::container::container(arena, Size::new(width, band));
                // Bottom-most: a press anywhere on the band focuses
                // the box (the editor and the byline sit on top).
                face.place(
                    0.0,
                    0.0,
                    imba::leaf::leaf::<RowCommand>(width, band).event(|_arena, event, _size| {
                        match event {
                            Event::MouseDown {
                                button: imba::event::MouseButton::Left,
                                ..
                            } => EventResult::Command(RowCommand::Composer(ComposerCommand::Focus)),
                            _ => EventResult::Ignored,
                        }
                    }),
                );
                face.place(
                    inset,
                    inset,
                    imba::layout::Layout::layout(
                        message.display(arena, store, ui),
                        arena,
                        Constraints {
                            min: Size::new(editor_w, content),
                            max: Size::new(editor_w, content),
                        },
                    )
                    .map(|command| RowCommand::Composer(ComposerCommand::Message(command)))
                    .focus_scope(*focused),
                );
                let author = author.clone();
                let ascent = -body_font.metrics().1.ascent;
                let shaper = imba::layout::TextShaper::of(ui);
                face.place(
                    0.0,
                    inset + content,
                    imba::leaf::leaf::<RowCommand>(width, line).paint_instead(
                        move |_arena, canvas, rect| {
                            shaper.draw(
                                canvas,
                                &body_font,
                                &author,
                                dim,
                                0.0,
                                rect.left + inset,
                                rect.top + ascent + (line - ascent) * 0.5,
                            );
                        },
                    ),
                );
                let rewrap = ((message.layout_width() - editor_w).abs() > 1.0).then_some(editor_w);
                imba::ThunkBox::new(
                    arena,
                    face.wrap_realized(move |inner| RewrapOnPaint { inner, rewrap }),
                )
            }
            CanvasRow::Banner(BannerRow::Composer { message, focused }) => {
                // The CHAT COMPOSER's layout, copied whole: the bare
                // input band (the placeholder is the editor's own,
                // growing UNBOUNDED with the message), a hairline,
                // and the TOOLBAR row under it — empty for now — with
                // the COMMIT cell flush right at the SEND cell's
                // fixed height and dress: tracked caps + the ⌘⏎ hint
                // over an accent fill with a 1px accent left rule.
                let band = composer_band(&theme, Some(message));
                let pad = chrome.pad;
                let box_pad = pad * 0.75;
                let ui_theme = theme.ui();
                let toolbar_h = ui_theme.toolbar.height;
                let editor_h = band - toolbar_h - box_pad * 2.0;

                let combo = ui_theme.combo.clone();
                let caps_font = hikit::fonts::ui_font(ui, combo.label_size * 1.1);
                let key_font = hikit::fonts::ui_text_font(ui, ui_theme.peeker.hint_size * 0.95);
                let label = "COMMIT";
                let cell_width = label
                    .chars()
                    .map(|ch| caps_font.measure_str(ch.to_string(), None).0 + 1.5)
                    .sum::<f32>()
                    + imba::layout::text_advance(ui, &key_font, "⌘⏎")
                    + combo.gap
                    + combo.pad * 2.0;
                let sendable = message.document.text().byte_count() > 0;
                let accent = chrome.accent.0;
                let on_accent = chrome.on_accent.0;
                let accent_soft = ui_theme.peeker.dim_text.0;
                let cell_h = toolbar_h - 1.0;
                let mid = cell_h * 0.5;
                let caps_ascent = -caps_font.metrics().1.ascent;
                let key_ascent = -key_font.metrics().1.ascent;
                let cell = imba::layout::Row::new(arena)
                    .gap(combo.gap)
                    .child(
                        imba::layout::text(ui, label, caps_font.clone(), on_accent)
                            .tracking(1.5)
                            .pad_insets(imba::layout::Insets {
                                left: 0.0,
                                top: (mid + caps_font.size() * 0.35 - caps_ascent).max(0.0),
                                right: 0.0,
                                bottom: 0.0,
                            }),
                    )
                    .child(
                        imba::layout::text(ui, "⌘⏎", key_font.clone(), accent_soft).pad_insets(
                            imba::layout::Insets {
                                left: 0.0,
                                top: (mid + key_font.size() * 0.35 - key_ascent).max(0.0),
                                right: 0.0,
                                bottom: 0.0,
                            },
                        ),
                    )
                    .pad_insets(imba::layout::Insets {
                        left: combo.pad,
                        top: 0.0,
                        right: 0.0,
                        bottom: 0.0,
                    })
                    .sized(cell_width, cell_h)
                    .backdrop(
                        move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: Rect| {
                            let mut paint = Paint::default();
                            let mut fill = accent;
                            if !sendable {
                                fill = fill.with_a(0x50);
                            }
                            paint.set_color(fill.with_a(fill.a() / 3));
                            canvas.draw_rect(rect, &paint);
                            paint.set_anti_alias(false);
                            paint.set_color(fill);
                            canvas.draw_rect(
                                Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                                &paint,
                            );
                        },
                    )
                    .on_click(|| RowCommand::Composer(ComposerCommand::Commit))
                    .layout(arena, Constraints::tight(Size::new(cell_width, cell_h)));

                let editor_w = (width - pad * 2.0).max(120.0);
                let input_band = band - toolbar_h;
                let mut face = imba::container::container(arena, Size::new(width, band));
                // Bottom-most: a press anywhere on the INPUT band
                // focuses the box (the editor sits on top; the
                // toolbar row is its own zone).
                face.place(
                    0.0,
                    0.0,
                    imba::leaf::leaf::<RowCommand>(width, input_band).event(
                        |_arena, event, _size| match event {
                            Event::MouseDown {
                                button: imba::event::MouseButton::Left,
                                ..
                            } => EventResult::Command(RowCommand::Composer(ComposerCommand::Focus)),
                            _ => EventResult::Ignored,
                        },
                    ),
                );
                face.place(
                    pad,
                    box_pad,
                    imba::layout::Layout::layout(
                        message.display(arena, store, ui),
                        arena,
                        Constraints {
                            min: Size::new(editor_w, editor_h),
                            max: Size::new(editor_w, editor_h),
                        },
                    )
                    .map(|command| RowCommand::Composer(ComposerCommand::Message(command)))
                    .focus_scope(*focused),
                );
                // The toolbar row: ruled off above, the COMMIT cell
                // flush right — the chat footer's shape, awaiting its
                // cells.
                let rule = ui_theme.toolbar.rule.0;
                face.place(
                    0.0,
                    input_band,
                    imba::leaf::leaf::<RowCommand>(width, 1.0).paint_instead(
                        move |_arena, canvas, rect| {
                            let mut paint = Paint::default();
                            paint.set_anti_alias(false);
                            paint.set_color(rule);
                            canvas.draw_rect(rect, &paint);
                        },
                    ),
                );
                face.place_boxed(width - cell_width, input_band + 1.0, cell);
                let rewrap = ((message.layout_width() - editor_w).abs() > 1.0).then_some(editor_w);
                imba::ThunkBox::new(
                    arena,
                    face.wrap_realized(move |inner| RewrapOnPaint { inner, rewrap }),
                )
            }
            CanvasRow::Header(header) => {
                use crate::diff_header::{DiffHeaderFace, DiffHeaderPress, DiffHeaderSpec};
                let band = header_band(&theme);
                let h1 = theme.resolve([editor::theme::StyleId::Header(1)]);
                let size = h1.font_size.unwrap_or(48.0);
                let mut buttons = vec![DiffHeaderPress::OpenPane, DiffHeaderPress::OpenFile];
                if header.built {
                    buttons.push(DiffHeaderPress::ToggleFace);
                }
                let face = DiffHeaderFace::new(
                    store,
                    ui,
                    DiffHeaderSpec {
                        title: header.file.title.clone(),
                        added: header.file.added,
                        removed: header.file.removed,
                        chevron: Some(header.collapsed),
                        buttons,
                        primary: Some(DiffHeaderPress::OpenFile),
                        title_size: size,
                        bold: h1.bold,
                        title_color: h1.color,
                        baseline: size,
                    },
                    band,
                    width,
                );
                let widget = face.widget(|press| {
                    Some(RowCommand::Header(match press {
                        DiffHeaderPress::OpenFile => HeaderAction::OpenFile,
                        DiffHeaderPress::OpenPane => HeaderAction::OpenPane,
                        DiffHeaderPress::ToggleFace => HeaderAction::ToggleFace,
                        DiffHeaderPress::ToggleCollapse => HeaderAction::ToggleCollapse,
                    }))
                });
                imba::ThunkBox::new(arena, imba::eager(widget))
            }
            CanvasRow::Diff(diff) => match &diff.body {
                RowBody::Placeholder { armed } => {
                    let body_height = (reserved_body(&theme, &diff.file) - chrome.gap).max(0.0);
                    let skeleton = skeleton_layout(arena, &theme, &diff.file, width, body_height);
                    let card = imba::layout::ZBox::new(arena)
                        .child(imba::layout::spacer(width, body_height))
                        .child(imba::layout::fixed(skeleton))
                        .pad_insets(imba::layout::Insets {
                            left: 0.0,
                            top: 0.0,
                            right: 0.0,
                            bottom: chrome.gap,
                        })
                        .layout(arena, constraints);
                    let armed = *armed;
                    imba::ThunkBox::new(
                        arena,
                        card.wrap(move |inner| ArmedPlaceholder { inner, armed }),
                    )
                }
                RowBody::Failed(error) => {
                    let text = format!("{} — {error}", diff.file.title);
                    let font = hikit::fonts::ui_text_font(ui, chrome.title_size * 0.85);
                    let shaper = imba::layout::TextShaper::of(ui);
                    let color = chrome.loader_color.0;
                    let body = imba::leaf::leaf::<RowCommand>(width, chrome.title_size * 3.0)
                        .paint_instead(move |_arena, canvas, rect| {
                            shaper.draw(
                                canvas,
                                &font,
                                &text,
                                color,
                                0.0,
                                rect.left + 16.0,
                                rect.top + rect.height() * 0.5,
                            );
                        });
                    imba::layout::ZBox::new(arena)
                        .child(imba::layout::spacer(width, chrome.title_size * 3.0))
                        .child(imba::layout::fixed(body))
                        .pad_insets(imba::layout::Insets {
                            left: 0.0,
                            top: 0.0,
                            right: 0.0,
                            bottom: chrome.gap,
                        })
                        .layout(arena, constraints)
                }
                RowBody::Built { pane } => {
                    let editor_target = (width - gutter).max(120.0);
                    let body = imba::layout::Layout::layout(
                        pane.display(arena, store, ui),
                        arena,
                        Constraints {
                            min: Size::new(editor_target, 0.0),
                            max: Size::new(editor_target + gutter, f32::MAX),
                        },
                    )
                    .map(RowCommand::Diff);
                    // A fresh landing is a SEED. The pane mounts and
                    // paints underneath (its own probes drive the
                    // dressing lanes), but the ROW keeps the skeleton
                    // face and its reserved band until the view answers
                    // DRESSED — loader → diff is one swap, not a
                    // striptease of markup, folds and heights arriving
                    // separately. ONE swap only: an edit undresses the
                    // face for a beat while its marks re-land, and a
                    // row that has shown its diff keeps showing it
                    // through the re-dress (`ever_dressed`) — no blink
                    // back to the skeleton under the user's caret.
                    // The list clips the row, and the container
                    // reports the reserved size, so the taller
                    // undressed body neither bleeds nor fights the
                    // list's visible-resize measure.
                    let presentable =
                        crate::diff_pane::gathered_view(store, pane.documents(), pane.id())
                            .is_none_or(|view| view.dressed() || view.ever_dressed());
                    if !presentable {
                        let body_height = (reserved_body(&theme, &diff.file) - chrome.gap).max(0.0);
                        let mut face = imba::container::container(
                            arena,
                            Size::new(width, body_height + chrome.gap),
                        );
                        face.place_boxed(0.0, 0.0, imba::ThunkBox::new(arena, body));
                        face.place_boxed(
                            0.0,
                            0.0,
                            skeleton_layout(arena, &theme, &diff.file, width, body_height),
                        );
                        return imba::ThunkBox::new(arena, face);
                    }
                    let body_height = Thunk::size(&body).height;
                    // The row-level rewrap governs the INLINE face
                    // only. On the split face the halves report their
                    // own painted geometry (`reports_geometry`) and
                    // resize themselves to the half-pane width — a
                    // row-level rewrap to the full width would fight
                    // them every frame.
                    let rewrap =
                        match crate::diff_pane::gathered_view(store, pane.documents(), pane.id()) {
                            Some(view) => match view.layout {
                                editor::unified_diff::DiffLayout::Split => None,
                                editor::unified_diff::DiffLayout::Inline => {
                                    let laid = view
                                        .split
                                        .right
                                        .document
                                        .layout_width(view.split.right.editor);
                                    ((laid - editor_target).abs() > 1.0).then_some(editor_target)
                                }
                            },
                            None => None,
                        };
                    let card = imba::layout::ZBox::new(arena)
                        .child(imba::layout::spacer(width, body_height))
                        .child(imba::layout::fixed(body))
                        .pad_insets(imba::layout::Insets {
                            left: 0.0,
                            top: 0.0,
                            right: 0.0,
                            bottom: chrome.gap,
                        })
                        .layout(arena, constraints);
                    imba::ThunkBox::new(
                        arena,
                        card.wrap(move |inner| RewrapOnPaint { inner, rewrap }),
                    )
                }
            },
        }
    }
}

// ------------------------------------------------------------- header

/// The skeleton under a pending header: rounded bars at the line
/// rhythm, tinted from the diff palette, the mix proportioned to the
/// entry's added:removed ratio — it already READS like the diff it
/// stands for.
fn skeleton_layout<'a>(
    arena: &'a Arena,
    theme: &editor::theme::Theme,
    file: &CanvasFile,
    width: f32,
    height: f32,
) -> imba::ThunkBox<'a, RowCommand> {
    let chrome = theme.ui().chat.clone();
    let line = chrome.title_size * 1.5;
    let added = file.added.unwrap_or(0).max(0) as f32;
    let removed = file.removed.unwrap_or(0).max(0) as f32;
    let green_share = match added + removed > 0.0 {
        true => added / (added + removed),
        false => 0.6,
    };
    let tint =
        |color: skia_safe::Color| skia_safe::Color::from_argb(40, color.r(), color.g(), color.b());
    let (added_color, removed_color) = (tint(chrome.added_color.0), tint(chrome.removed_color.0));
    let inset = chrome.pad;
    let thunk =
        imba::leaf::leaf::<RowCommand>(width, height).paint_instead(move |_arena, canvas, rect| {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            let bars = ((rect.height() / line).floor() as usize).max(1);
            let widths = [0.72f32, 0.48, 0.63, 0.55, 0.42, 0.68];
            let green_bars = (bars as f32 * green_share).round() as usize;
            for bar in 0..bars {
                let top = rect.top + bar as f32 * line + line * 0.2;
                let bar_width = (rect.width() - inset * 2.0) * widths[bar % widths.len()];
                paint.set_color(match bar < green_bars {
                    true => added_color,
                    false => removed_color,
                });
                let bar_rect = Rect::from_xywh(rect.left + inset, top, bar_width, line * 0.55);
                canvas.draw_round_rect(bar_rect, 4.0, 4.0, &paint);
            }
        });
    imba::ThunkBox::new(arena, thunk)
}

/// The lazy trigger: a placeholder that PAINTED is a placeholder the
/// user can see (the list culls everything else) — the first unarmed
/// paint asks for the build at the row's real width.
struct ArmedPlaceholder<Inner> {
    inner: Inner,
    armed: bool,
}

impl<'a, Inner: Widget<'a, RowCommand>> Widget<'a, RowCommand> for ArmedPlaceholder<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, RowCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<RowCommand> {
        let result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) && !self.armed {
            return result.merge(EventResult::Command(RowCommand::Arm(
                self.inner.size().width,
            )));
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, RowCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

struct RewrapOnPaint<Inner> {
    inner: Inner,
    rewrap: Option<f32>,
}

impl<'a, Inner: Widget<'a, RowCommand>> Widget<'a, RowCommand> for RewrapOnPaint<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, RowCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<RowCommand> {
        let result = self.inner.handle_event(arena, event, viewport);
        if let (Event::Paint { .. }, Some(width)) = (event, self.rewrap) {
            return result.merge(EventResult::Command(RowCommand::Rewrap(width)));
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, RowCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

// ---------------------------------------------------------------- panel

use crate::diff_canvas::CanvasPlace;

/// Answers `CanvasPlace` navigations: find — or create — the source's
/// store-held canvas and hand back a REFERENCE view of it. Reuse is
/// the lookup; nothing is ever rebuilt for a second open.
pub struct CanvasNavigator;

impl hikit::navigation::Navigator for CanvasNavigator {
    type Place = CanvasPlace;

    fn navigate(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        place: &CanvasPlace,
        _fx: &mut imba::command::Fx<'_>,
    ) -> Option<Box<dyn hikit::panel::DynPanelView>> {
        let view = DiffCanvasView::over(store, place.changes, place.source.clone());
        if let Some(key) = &place.reveal {
            Canvases::set_reveal(store, view.changes, view.id(), key.clone());
        }
        Some(Box::new(view))
    }
}

impl hikit::panel::PanelView for DiffCanvasView {
    type Place = CanvasPlace;

    fn pane_row(&self) -> Option<hikit::pane_row::PaneRow> {
        Some(hikit::pane_row::PaneRow::new(crate::CanvasRow(
            self.source.clone(),
        )))
    }

    fn navigation_location(&self, _store: &Store) -> Option<CanvasPlace> {
        Some(CanvasPlace {
            changes: self.changes,
            source: self.source.clone(),
            reveal: None,
        })
    }

    fn navigate_to(
        &mut self,
        store: &mut Store,
        place: &CanvasPlace,
        _fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        if place.source != self.source {
            return false;
        }
        // Reuse IN PLACE: arm the reveal on the shared canvas — the
        // paint probe picks it up next frame.
        if let Some(key) = &place.reveal {
            Canvases::set_reveal(store, self.changes, self.id, key.clone());
        }
        true
    }

    fn title(&self, store: &Store) -> String {
        self.source.title(store, self.changes)
    }

    fn take_request(&mut self) -> Option<hikit::panel::PanelRequest> {
        self.request.take()
    }

    fn dismantle(&mut self, store: &mut Store) {
        Canvases::release(store, self.changes, self.id);
    }

    /// Displacement ends this view as surely as closing does — the
    /// slot walked elsewhere and the instance is dropped right after
    /// (the walk back mints a fresh view through the navigator, which
    /// retains again). Without this, every navigation away left a
    /// permanent +1 and the canvas — rows, documents, editors — could
    /// never retire.
    fn displaced(&mut self, store: &mut Store) {
        Canvases::release(store, self.changes, self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
