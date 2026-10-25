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

use crate::canvases::Canvases;
use crate::diff_canvas::{
    canvas_files, canvas_generation, totals_of, CanvasFile, CanvasListing, Totals,
};
use crate::rows::{
    body_air, header_band, reserved_body, trace_wrap, CanvasRow, DiffRow, HeaderAction, HeaderRow,
    RowBody, RowCommand,
};
use changesview::hichanges::{CanvasId, CanvasSource, Changes};
use editor::env;
use editor::{location::ResourceLocation, unified_diff::UnifiedDiffCommand};
use imba::effect::{AnyEffect, Effects};
use imba::event::Placement;
use imba::list::{ListCommand, ListSlice, ListView, StickyStyle};
use imba::thunk_ext::ThunkExt;
use imba::{arena::Arena, constraints::Constraints, store::Store, ui::UiCtx, Thunk, View};
use skia_safe::Size;

/// The row key: the file's `new` side names the FILE node (the header
/// row and its cover span — the reveal target), and the diff child
/// keys itself beside it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum CanvasKey {
    File(ResourceLocation),
    Diff(ResourceLocation),
}

type CanvasRows = ListView<CanvasRow, CanvasKey>;
/// BARE rows — the canvas owns no scroll (the descrollering law):
/// the workbench pane wraps it in `hikit::modal::Scrolled`, the
/// mounted road rides the host's scroller, and positioning asks
/// travel as reveals either way.
type RowsCommand = ListCommand<RowCommand>;

#[derive(Clone)]
pub enum CanvasCommand {
    Rows(RowsCommand),

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

    /// The summary pill's press: collapse every open file while any
    /// stands open, expand every parked one after.
    ToggleAll,
}

impl std::fmt::Display for CanvasCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CanvasCommand::Rows(command) => command.fmt(out),
            CanvasCommand::ToRow { command, .. } => command.fmt(out),
            CanvasCommand::ToPending { command, .. } => command.fmt(out),
            CanvasCommand::Landed { .. } => out.write_str("canvas landed"),
            CanvasCommand::ToggleAll => out.write_str("canvas toggle all"),
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
    pub(crate) changes: imba::store::Id<Changes>,
    pub(crate) source: CanvasSource,
    pub(crate) rows: CanvasRows,
    pub(crate) files: rpds::HashTrieMapSync<ResourceLocation, CanvasFile>,

    /// The files' sums, restamped as the listing moves — what a face's
    /// byline and summary read, O(1).
    totals: Totals,

    note: Option<String>,
    seen: Option<u64>,
    pub(crate) populated: bool,
    pub(crate) request: Option<hikit::panel::PanelRequest>,

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
    pub(crate) reveal: Option<ResourceLocation>,

    /// Views standing on this canvas. At zero the canvas retires —
    /// except the WORKING-COPY canvas, which stays once opened.
    pub(crate) refs: u32,

    /// Collapsed files' diff rows, parked with their heights — the
    /// views stay alive (a built diff keeps its editors), the list
    /// just stops holding their rows.
    stash: rpds::HashTrieMapSync<ResourceLocation, (CanvasRow, f32)>,

    /// Built rows by their diff view — the DRESSING sweep names the
    /// views it touched (crate::DressedViews) and the canvas resizes
    /// exactly those rows, O(touched), no scan.
    pub(crate) pairs: rpds::HashTrieMapSync<documents::diffs::DiffViewId, ResourceLocation>,

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
        let theme = env::Themes::of(store);
        let window = theme.part(::editor::theme::WindowChrome::KEY);
        let ink = theme.part(::editor::theme::InkChrome::KEY);
        StickyStyle {
            // The planted band wears the FILE BAND's own fold ground
            // (`FileBand`), ruled off from the scrolling code.
            background: theme
                .part(::hikit::chrome::DiffChrome::KEY)
                .fold_background
                .0,
            divider: ink.rule.0,
            divider_width: window.divider_width,
        }
    })
}

impl Canvas {
    pub(crate) fn fresh(changes: imba::store::Id<Changes>, source: CanvasSource) -> Self {
        Self {
            changes,
            source,
            rows: ListView::empty().with_sticky(sticky_style()),
            files: rpds::HashTrieMapSync::new_sync(),
            totals: Totals::default(),
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
        self.rows.probe_viewport_top()
    }

    /// (title, phase, current diff-row height), in list order;
    /// collapsed files report their parked row at height zero.
    #[doc(hidden)]
    pub fn probe_rows(&self) -> Vec<(String, RowPhase, f32)> {
        let rows = (&self.rows);
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
        let rows = (&self.rows);
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

    /// TEST SUPPORT: every list row's key, in order — catches an
    /// orphaned Diff row a per-file probe would miss.
    #[doc(hidden)]
    pub fn probe_row_keys(&self) -> Vec<String> {
        let rows = (&self.rows);
        (0..rows.len())
            .filter_map(|index| match rows.key_at(index)? {
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
            .row_range(&CanvasKey::File(key.clone()))
            .map(|range| range.len())
    }

    /// TEST SUPPORT: the registered `DiffViewId` backing a built row.
    #[doc(hidden)]
    pub fn probe_pair(&self, key: &ResourceLocation) -> Option<documents::diffs::DiffViewId> {
        let row = self
            .rows
            .row_range(&CanvasKey::Diff(key.clone()))
            .and_then(|range| (&self.rows).view_at(range.start))
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

    fn refresh(&mut self, store: &mut Store, fx: &mut Effects<'_, CanvasCommand>) {
        let (generation, listing) = canvas_files(store, self.changes, &self.source);
        self.adopt(store, generation, listing, fx);
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
        let Some(header_range) = (&self.rows).row_range(&CanvasKey::File(key.clone())) else {
            return;
        };
        let start = header_range.start;
        let expanded = self.rows.row_range(&CanvasKey::Diff(key.clone())).is_some();
        let (ordinal, split) = self
            .header_at(key)
            .map(|header| (header.ordinal, header.split))
            .unwrap_or_default();
        let header = CanvasRow::Header(HeaderRow {
            file,
            collapsed: !expanded,
            built: true,
            ordinal,
            split,
        });
        let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
        slice.push_keyed_sized(CanvasKey::File(key.clone()), header, header_band(&theme));
        if expanded {
            let height = self
                .rows
                .row_range(&CanvasKey::Diff(key.clone()))
                .and_then(|range| (&self.rows).height_at(range.start))
                .unwrap_or(0.0);
            slice.push_keyed_sized(CanvasKey::Diff(key.clone()), diff, height);
            slice.cover(CanvasKey::File(key.clone()), 0..2);
            self.rows.splice_slice(start..start + 2, slice);
            // The successor is dressed: settle to its honest height.
            self.resize_key(key, store, ui, fx);
        } else {
            let height = self.stash.get(key).map(|(_, held)| *held).unwrap_or(0.0);
            slice.cover(CanvasKey::File(key.clone()), 0..1);
            self.stash.insert_mut(key.clone(), (diff, height));
            self.rows.splice_slice(start..start + 1, slice);
        }
        fx.settle();
    }

    pub(crate) fn sync_in_place(
        &mut self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        dressed: &[documents::diffs::DiffViewId],
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        if self.seen != Some(canvas_generation(store, self.changes, &self.source)) {
            self.refresh(store, fx);
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
        let Some(CanvasKey::Diff(key)) = (&self.rows).key_at(index) else {
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
        let Some(range) = (&self.rows).row_range(&CanvasKey::Diff(key)) else {
            return;
        };
        let index = range.start;
        let Some(CanvasRow::Diff(DiffRow {
            body: RowBody::Built { pane },
            ..
        })) = (&self.rows).view_at(index)
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
        // No card margin: the row is the body and the air under it;
        // the next band's hairline closes it.
        let want = body + body_air(&env::Themes::of(store));
        if (want - (&self.rows).height_at(index).unwrap_or(0.0)).abs() > 0.5 {
            let rows = ListCommand::SetHeight(index, want);
            fx.scope(CanvasCommand::Rows, |fx| {
                self.rows.perform(store, ui, rows, fx)
            });
        }
    }

    fn adopt(
        &mut self,
        store: &mut Store,
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
                self.totals = totals_of(files.iter());
                let count = files.len();
                for (at, file) in files.iter().enumerate() {
                    let start = slice.len();
                    slice.push_keyed_sized(
                        CanvasKey::File(file.new.clone()),
                        CanvasRow::Header(HeaderRow {
                            file: file.clone(),
                            collapsed: false,
                            built: false,
                            ordinal: (at + 1, count),
                            split: false,
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
                (&mut self.rows).splice_slice(0..0, slice);
                if let Some(key) = self.reveal.take() {
                    self.rows
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
        let mut anchor = 0;
        for file in fresh {
            let key = file.new.clone();
            match self.files.get(&key).cloned() {
                Some(known) => {
                    if let Some(header) = (&self.rows).row_range(&CanvasKey::File(key.clone())) {
                        anchor = match (&self.rows).row_range(&CanvasKey::Diff(key.clone())) {
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
                            // Restamped below, once the membership
                            // settles.
                            ordinal: (0, 0),
                            split: false,
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
                    (&mut self.rows).splice_slice(anchor..anchor, slice);
                    anchor += 2;
                    self.phases.insert_mut(key.clone(), RowPhase::Placeholder);
                    self.files.insert_mut(key, file);
                    moved = true;
                }
            }
        }
        if moved {
            self.restamp();
            fx.settle();
        }
    }

    /// After a membership or stats move: every header's "file N of M"
    /// and the canvas's totals follow the listing. Linear in the
    /// files, like the reconcile that calls it — and a header only
    /// resplices when its ordinal actually moved.
    fn restamp(&mut self) {
        let keys: Vec<ResourceLocation> = (0..self.rows.len())
            .filter_map(|index| match (&self.rows).key_at(index) {
                Some(CanvasKey::File(key)) => Some(key.clone()),
                _ => None,
            })
            .collect();
        let count = keys.len();
        for (at, key) in keys.iter().enumerate() {
            let ordinal = (at + 1, count);
            if self
                .header_at(key)
                .is_some_and(|header| header.ordinal != ordinal)
            {
                self.resplice_header(key, |header| header.ordinal = ordinal);
            }
        }
        self.totals = totals_of(self.files.values());
    }

    /// The standing header row of a file.
    fn header_at(&self, key: &ResourceLocation) -> Option<HeaderRow> {
        let range = (&self.rows).row_range(&CanvasKey::File(key.clone()))?;
        match (&self.rows).view_at(range.start)? {
            CanvasRow::Header(header) => Some(header),
            _ => None,
        }
    }

    fn retire(&mut self, store: &mut Store, key: &ResourceLocation) {
        self.teardown_row(store, key);
        if let Some(header) = (&self.rows).row_range(&CanvasKey::File(key.clone())) {
            let end = match (&self.rows).row_range(&CanvasKey::Diff(key.clone())) {
                Some(diff) => header.end.max(diff.end),
                None => header.end,
            };
            let empty: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
            self.rows.splice_slice(header.start..end, empty);
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
    pub(crate) fn teardown_row(&mut self, store: &mut Store, key: &ResourceLocation) {
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
            .row_range(&CanvasKey::Diff(key.clone()))
            .and_then(|range| (&self.rows).view_at(range.start))
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
        let _ = band;
        let built = matches!(
            self.phases.get(key),
            Some(RowPhase::Built) | Some(RowPhase::Failed)
        );
        self.resplice_header(key, |header| {
            header.file = file.clone();
            header.built = built;
        });
    }

    /// Resplices one header row in place with `update` applied — same
    /// band height, the FULL cover re-asserted (see `refresh_header`).
    fn resplice_header(&mut self, key: &ResourceLocation, update: impl FnOnce(&mut HeaderRow)) {
        let Some(header) = (&self.rows).row_range(&CanvasKey::File(key.clone())) else {
            return;
        };
        let Some(CanvasRow::Header(mut row)) = (&self.rows).view_at(header.start) else {
            return;
        };
        update(&mut row);
        let band = self.rows.height_at(header.start).unwrap_or(0.0);
        match (&self.rows).row_range(&CanvasKey::Diff(key.clone())) {
            Some(diff_range) => {
                let Some(diff_row) = (&self.rows).view_at(diff_range.start) else {
                    return;
                };
                let diff_height = self.rows.height_at(diff_range.start).unwrap_or(0.0);
                row.collapsed = false;
                let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
                slice.push_keyed_sized(CanvasKey::File(key.clone()), CanvasRow::Header(row), band);
                slice.push_keyed_sized(CanvasKey::Diff(key.clone()), diff_row, diff_height);
                slice.cover(CanvasKey::File(key.clone()), 0..2);
                self.rows
                    .splice_slice(header.start..header.start + 2, slice);
            }
            None => {
                row.collapsed = true;
                let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
                slice.push_keyed_sized(CanvasKey::File(key.clone()), CanvasRow::Header(row), band);
                slice.cover(CanvasKey::File(key.clone()), 0..1);
                self.rows
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
        if trace_wrap() {
            eprintln!("[wrap] launch {} at width {width}", key.name());
        }
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
        let row = match (&self.rows).row_range(&CanvasKey::Diff(key.clone())) {
            Some(range) => (&self.rows).view_at(range.start),
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
                ordinal: (1, 1),
                split: false,
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
        let at = (&self.rows).len();
        (&mut self.rows).splice_slice(at..at, slice);
        self.populated = true;
        self.totals = totals_of(self.files.values());
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
        let Some(CanvasKey::Diff(location)) = (&self.rows).key_at(index).cloned() else {
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
        let rows = (&self.rows);
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
        let chrome = theme.part(::hikit::chrome::ChatChrome::KEY);
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
                            true => height + body_air(&theme),
                            false => reserved_body(&theme, &file),
                        };
                        (RowBody::Built { pane }, height)
                    }
                    None => (
                        RowBody::Failed("could not open the diff".to_owned()),
                        chrome.title_size * 3.0 + body_air(&theme),
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
        let diff_height = body_height;

        if let Some(header_range) = (&self.rows).row_range(&CanvasKey::File(key.clone())) {
            let start = header_range.start;
            let expanded = self.rows.row_range(&CanvasKey::Diff(key.clone())).is_some();
            let (ordinal, split) = self
                .header_at(&key)
                .map(|header| (header.ordinal, header.split))
                .unwrap_or_default();
            let header = CanvasRow::Header(HeaderRow {
                file,
                collapsed: !expanded,
                built: true,
                ordinal,
                split,
            });
            let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
            slice.push_keyed_sized(CanvasKey::File(key.clone()), header, header_band(&theme));
            if expanded {
                slice.push_keyed_sized(CanvasKey::Diff(key.clone()), diff, diff_height);
                slice.cover(CanvasKey::File(key.clone()), 0..2);
                self.rows.splice_slice(start..start + 2, slice);
            } else {
                // Collapsed before the build landed: park the built
                // row; expand splices it in.
                slice.cover(CanvasKey::File(key.clone()), 0..1);
                self.stash.insert_mut(key.clone(), (diff, diff_height));
                self.rows.splice_slice(start..start + 1, slice);
            }
            // The swap is a height mutation like any other: the door
            // noted the anchor, the pulse re-aims before this frame
            // paints. Zero wrong frames (docs/editor/diff-canvas.md §4).
            fx.settle();
        }
    }

    fn toggle_collapse(&mut self, key: &ResourceLocation, fx: &mut Effects<'_, CanvasCommand>) {
        let Some(header_range) = (&self.rows).row_range(&CanvasKey::File(key.clone())) else {
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
        let band = (&self.rows).height_at(start).unwrap_or(64.0);
        let (ordinal, split) = self
            .header_at(key)
            .map(|header| (header.ordinal, header.split))
            .unwrap_or_default();
        match (&self.rows).row_range(&CanvasKey::Diff(key.clone())) {
            Some(diff_range) => {
                let Some(diff) = (&self.rows).view_at(diff_range.start) else {
                    return;
                };
                let height = self.rows.height_at(diff_range.start).unwrap_or(0.0);
                self.stash.insert_mut(key.clone(), (diff, height));
                let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
                slice.push_keyed_sized(
                    CanvasKey::File(key.clone()),
                    CanvasRow::Header(HeaderRow {
                        file,
                        collapsed: true,
                        built,
                        ordinal,
                        split,
                    }),
                    band,
                );
                slice.cover(CanvasKey::File(key.clone()), 0..1);
                self.rows.splice_slice(start..start + 2, slice);
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
                        ordinal,
                        split,
                    }),
                    band,
                );
                slice.push_keyed_sized(CanvasKey::Diff(key.clone()), diff, height);
                slice.cover(CanvasKey::File(key.clone()), 0..2);
                self.rows.splice_slice(start..start + 1, slice);
            }
        }
        fx.settle();
    }

    /// The summary pill's one verb: anything open → collapse all;
    /// everything parked → expand all. Each flip is the header
    /// chevron's own `toggle_collapse`, so the stash discipline (built
    /// views stay alive, placeholders park) holds file by file.
    fn toggle_all(&mut self, fx: &mut Effects<'_, CanvasCommand>) {
        let keys: Vec<ResourceLocation> = self.files.keys().cloned().collect();
        let any_open = keys.iter().any(|key| {
            (&self.rows)
                .row_range(&CanvasKey::Diff(key.clone()))
                .is_some()
        });
        for key in keys {
            let open = (&self.rows)
                .row_range(&CanvasKey::Diff(key.clone()))
                .is_some();
            if any_open == open && (open || self.stash.get(&key).is_some()) {
                self.toggle_collapse(&key, fx);
            }
        }
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
                // The band's face word names the OTHER face: read the
                // one the pane now wears.
                let split = self
                    .row_pane(key)
                    .and_then(|pane| {
                        crate::diff_pane::gathered_view(store, pane.documents(), pane.id())
                    })
                    .map(|view| view.layout == editor::unified_diff::DiffLayout::Split);
                if let Some(split) = split {
                    self.resplice_header(key, |header| header.split = split);
                }
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
            .row_range(&CanvasKey::Diff(key.clone()))
            .and_then(|range| (&self.rows).view_at(range.start))
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
        match (&self.rows).row_range(&CanvasKey::Diff(key.clone())) {
            Some(range) => {
                let index = range.start;
                let rows = ListCommand::Child(index, command);
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
    let inner = command;
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
    let gutter = theme.part(::editor::theme::EditorGutterChrome::KEY).width;
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
    pub(crate) fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, CanvasCommand> {
        self.rows.focus_data(store, ui).map(CanvasCommand::Rows)
    }

    pub(crate) fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: CanvasCommand,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        match command {
            CanvasCommand::Landed { key, prep } => self.land(store, ui, key, prep, fx),
            CanvasCommand::ToggleAll => self.toggle_all(fx),
            CanvasCommand::ToRow { key, command } => self.to_row(key, command, store, ui, fx),
            CanvasCommand::ToPending { key, command } => {
                self.to_pending(key, command, store, ui, fx)
            }
            CanvasCommand::Rows(command) => {
                match row_ask(&command) {
                    Some((index, RowCommand::Arm(width))) => self.launch(store, index, *width, fx),
                    Some((index, RowCommand::Header(action))) => {
                        let key = match (&self.rows).key_at(index) {
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
            use imba::layout::LayoutExt as _;
            let ground = env::Themes::of(store)
                .part(::editor::theme::InkChrome::KEY)
                .ground
                .0;
            match &self.note {
                // The note (pending, empty, failed listing): one brief
                // line centered on the ground.
                Some(note) => imba::layout::Layout::layout(
                    hikit::ui::text(&hikit::ui::brief(store, ui), note.clone())
                        .align(imba::layout::Alignment::Center)
                        .backdrop(hikit::ui::ground(ground)),
                    arena,
                    constraints,
                ),
                // The rows, content-sized under the ceiling the shell
                // gives (docs/mounted-parity.md): the sticky headers
                // are NOT claimed here — they bubble to whoever owns
                // the scroll physics.
                None => {
                    let rows = imba::layout::Layout::layout(
                        self.rows.display(arena, store, ui),
                        arena,
                        constraints,
                    )
                    .map(CanvasCommand::Rows);
                    imba::ThunkBox::new(
                        arena,
                        imba::thunk_ext::ThunkExt::paint_below(rows, hikit::ui::ground(ground)),
                    )
                }
            }
        })
    }
}

/// The canvas VIEW — a REFERENCE view over the store-held canvas,
/// the `PairPane` shape: views hold ids, state lives in `Canvases`,
/// and a second view of the same source costs nothing. A face
/// (`faces`) stands its head over it and is the panel.
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

    pub fn changes(&self) -> imba::store::Id<Changes> {
        self.changes
    }

    /// The canvas's sums — a face's byline and summary read them.
    pub fn totals(&self, store: &Store) -> Totals {
        self.canvas(store)
            .map(|canvas| canvas.totals)
            .unwrap_or_default()
    }

    /// How many files stand collapsed.
    pub fn collapsed(&self, store: &Store) -> usize {
        self.canvas(store)
            .map(|canvas| canvas.stash.size())
            .unwrap_or(0)
    }

    /// The panel ask a row minted (a header's open): the face carries
    /// it out.
    pub fn take_request(&mut self) -> Option<hikit::panel::PanelRequest> {
        self.request.take()
    }

    /// Arm a reveal on the shared canvas — the paint probe picks it
    /// up next frame.
    pub fn arm_reveal(&self, store: &mut Store, key: ResourceLocation) {
        Canvases::set_reveal(store, self.changes, self.id, key);
    }

    /// The face left its slot, by closing or by displacement: the
    /// canvas loses this view's retain and retires at zero — except
    /// the working-copy canvas, which stays once opened.
    pub fn release(&mut self, store: &mut Store) {
        Canvases::release(store, self.changes, self.id);
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

    /// TEST SUPPORT: the armed row reveal, if any.
    #[doc(hidden)]
    pub fn probe_reveal(&self, store: &Store) -> Option<editor::location::ResourceLocation> {
        self.canvas(store)?.reveal.clone()
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

    /// TEST SUPPORT: every Built row's (new-side location, pair id).
    #[doc(hidden)]
    pub fn probe_built_pairs(
        &self,
        store: &Store,
    ) -> Vec<(
        editor::location::ResourceLocation,
        documents::diffs::DiffViewId,
    )> {
        let Some(canvas) = self.canvas(store) else {
            return Vec::new();
        };
        canvas
            .files
            .keys()
            .filter_map(|key| Some((key.clone(), canvas.probe_pair(key)?)))
            .collect()
    }

    /// TEST SUPPORT: drive the REAL listing adoption (the branch the
    /// paint probe and `Canvases::sync` reach through `refresh`).
    #[doc(hidden)]
    pub fn adopt_for_tests(
        &self,
        store: &mut Store,
        generation: u64,
        listing: crate::diff_canvas::CanvasListing,
    ) {
        if let Some(mut canvas) = Canvases::take(store, self.changes, self.id) {
            let mut batch = imba::effect::Batch::new();
            canvas.adopt(store, generation, listing, &mut batch.effects());
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
                if let Some(range) = canvas.rows.row_range(&CanvasKey::Diff(key.clone())) {
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
