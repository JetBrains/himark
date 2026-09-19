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

use himark::diff_canvas::{
    canvas_files, canvas_generation, CanvasFile, CanvasListing, CanvasSource,
};
use himark::{env, ResourceLocation, UnifiedDiffCommand};
use imba::effect::{AnyEffect, Effects};
use imba::event::{Event, EventResult, Placement};
use imba::list::{ListCommand, ListSlice, ListView, StickyStyle};
use imba::scroll::{ScrollCommand, ScrollView};
use imba::thunk_ext::ThunkExt;
use imba::{arena::Arena, constraints::Constraints, store::Store, Thunk, UiCtx, View, Widget};
use skia_safe::{Paint, Rect, Size};

const MIN_EST_LINES: i64 = 4;
const MAX_EST_LINES: i64 = 60;
const EST_CONTEXT_LINES: i64 = 4;

/// The row key: the file's `new` side names the FILE node (the header
/// row and its cover span — the reveal target), and the diff child
/// keys itself beside it. The banner heads the list and covers
/// nothing, so the sticky machinery never plants it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum CanvasKey {
    Banner,
    File(ResourceLocation),
    Diff(ResourceLocation),
}

type CanvasRows = ListView<CanvasRow, CanvasKey>;
type RowsCommand = ScrollCommand<ListCommand<RowCommand>>;

pub enum CanvasCommand {
    Rows(RowsCommand),

    /// The paint probe saw the feed's generation move.
    Refresh,

    /// The paint probe saw an armed reveal on an already-populated
    /// canvas (a reuse navigation delivered it).
    PickupReveal,

    Landed {
        key: ResourceLocation,
        built: himark::BuiltFileDiff,
    },

    /// A key-addressed row command — effect landings route by KEY,
    /// never by a captured index: collapse/expand splices shift
    /// indices under in-flight work.
    ToRow {
        key: ResourceLocation,
        command: RowCommand,
    },
}

/// The canvas STATE, store-held in `Canvases` (the `OpenDocuments`
/// discipline): panels are `DiffCanvasView` reference views over a
/// `CanvasId`; opening a source that already has a canvas reuses it —
/// nothing is ever rebuilt for a second click.
#[derive(Clone)]
pub struct Canvas {
    source: CanvasSource,
    rows: ScrollView<CanvasRows>,
    files: rpds::HashTrieMapSync<ResourceLocation, CanvasFile>,

    note: Option<String>,
    seen: Option<u64>,
    populated: bool,
    request: Option<himark::PanelRequest>,

    phases: rpds::HashTrieMapSync<ResourceLocation, RowPhase>,

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
}

/// A row's lifecycle, panel-tracked — the test oracle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowPhase {
    Placeholder,
    Built,
    Failed,
}

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
    fn fresh(source: CanvasSource) -> Self {
        Self {
            source,
            rows: ScrollView::new(ListView::empty().with_sticky(sticky_style())),
            files: rpds::HashTrieMapSync::new_sync(),
            note: None,
            seen: None,
            populated: false,
            request: None,
            phases: rpds::HashTrieMapSync::new_sync(),
            reveal: None,
            refs: 0,
            stash: rpds::HashTrieMapSync::new_sync(),
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

    #[doc(hidden)]
    pub fn probe_focused_row(&self) -> Option<usize> {
        self.rows.content().focused()
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
            BannerRow::Commit { message, author } => Some((message, author)),
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
    pub fn probe_pair(&self, key: &ResourceLocation) -> Option<himark::DiffViewId> {
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
                let Some(view) = crate::gathered_view(store, pane.id()) else {
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
    pub fn probe_layouts(&self, store: &Store) -> Vec<(String, himark::DiffLayout)> {
        self.diff_rows()
            .into_iter()
            .filter_map(|(title, diff)| {
                let RowBody::Built { pane } = &diff.body else {
                    return None;
                };
                let Some(view) = crate::gathered_view(store, pane.id()) else {
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
                let Some(view) = crate::gathered_view(store, pane.id()) else {
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

    /// Per Built row: (host text head, each card's text head).
    #[doc(hidden)]
    pub fn probe_texts(&self, store: &Store) -> Vec<(String, Vec<String>)> {
        self.diff_rows()
            .into_iter()
            .filter_map(|(_, diff)| {
                let RowBody::Built { pane } = &diff.body else {
                    return None;
                };
                let Some(view) = crate::gathered_view(store, pane.id()) else {
                    return None;
                };
                let inline = view.inline_editor?;
                let host = {
                    let text = view.split.right.document.text();
                    let end = text.byte_count().min(120) as u32;
                    text.view().substring(0..end)
                };
                let cards = view
                    .split
                    .right
                    .document
                    .before_inlay_views(inline)
                    .into_iter()
                    .map(|(_, card)| card.shown_text())
                    .collect();
                Some((host, cards))
            })
            .collect()
    }

    /// Geometry oracle: (content_height, [(anchor_byte, y_of_anchor)]).
    #[doc(hidden)]
    pub fn probe_geometry(&self, store: &Store) -> Vec<(f32, Vec<(u32, f32)>)> {
        self.diff_rows()
            .into_iter()
            .filter_map(|(_, diff)| {
                let RowBody::Built { pane } = &diff.body else {
                    return None;
                };
                let Some(view) = crate::gathered_view(store, pane.id()) else {
                    return None;
                };
                let inline = view.inline_editor?;
                let document = &view.split.right.document;
                let content = document.content_height(inline);
                let anchors = document
                    .before_inlays(inline)
                    .into_iter()
                    .map(|(range, _, _)| (range.start, document.height_before(inline, range.start)))
                    .collect();
                Some((content, anchors))
            })
            .collect()
    }

    fn refresh(&mut self, store: &mut Store, fx: &mut Effects<'_, CanvasCommand>) {
        let (generation, listing) = canvas_files(store, &self.source);
        self.adopt(store, generation, listing, fx);
    }

    fn adopt(
        &mut self,
        store: &mut Store,
        generation: u64,
        listing: CanvasListing,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        self.seen = Some(generation);
        match listing {
            CanvasListing::Pending(text) => {
                if !self.populated {
                    self.note = Some(text);
                }
            }
            CanvasListing::Empty(text) => {
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
                if self.populated {
                    self.reconcile(store, files, fx);
                    return;
                }
                self.populated = true;
                self.note = None;
                let theme = env::Themes::of(store);
                let band = header_band(&theme);
                let mut slice: ListSlice<CanvasRow, CanvasKey> = ListSlice::new();
                match himark::diff_canvas::canvas_banner(store, &self.source) {
                    Some(himark::diff_canvas::CanvasBanner::Composer { .. }) => {
                        let message = fresh_composer_box(store);
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
                    Some(himark::diff_canvas::CanvasBanner::Commit { message, author }) => {
                        let height = commit_band(&theme, &message);
                        slice.push_keyed_sized(
                            CanvasKey::Banner,
                            CanvasRow::Banner(BannerRow::Commit { message, author }),
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
                        self.relaunch(&key, &file, fx);
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
    fn teardown_row(&self, store: &mut Store, key: &ResourceLocation) {
        let pane = self
            .rows
            .content()
            .row_range(&CanvasKey::Diff(key.clone()))
            .and_then(|range| self.rows.content().view_at(range.start))
            .or_else(|| self.stash.get(key).map(|(row, _)| row.clone()));
        if let Some(CanvasRow::Diff(DiffRow {
            body: RowBody::Built { pane },
            ..
        })) = pane
        {
            crate::teardown_diff_view(store, pane.id());
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

    /// Relaunch a stale row's build at its standing width. A row that
    /// never built (placeholder) has no width yet — its paint arm
    /// picks up the fresh pair from `files` on its own.
    fn relaunch(
        &self,
        key: &ResourceLocation,
        file: &CanvasFile,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        let Some(width) = self.built_width(key) else {
            return;
        };
        let location = key.clone();
        fx.push(
            AnyEffect::new(himark::BuildFileDiffEffect {
                old: file.old.clone(),
                new: file.new.clone(),
                width,
            })
            .map(move |built| CanvasCommand::Landed {
                key: location.clone(),
                built,
            }),
        );
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
    pub fn seed_built_for_tests(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        file: CanvasFile,
        built: himark::BuiltFileDiff,
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
        self.land(store, ui, key, built, &mut throwaway.effects());
    }

    fn launch(&self, index: usize, width: f32, fx: &mut Effects<'_, CanvasCommand>) {
        let Some(CanvasKey::Diff(location)) = self.rows.content().key_at(index).cloned() else {
            return;
        };
        let Some(file) = self.files.get(&location) else {
            return;
        };
        fx.push(
            AnyEffect::new(himark::BuildFileDiffEffect {
                old: file.old.clone(),
                new: file.new.clone(),
                width,
            })
            .map(move |built| CanvasCommand::Landed {
                key: location.clone(),
                built,
            }),
        );
    }

    fn land(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        key: ResourceLocation,
        built: himark::BuiltFileDiff,
        fx: &mut Effects<'_, CanvasCommand>,
    ) {
        let Some(file) = self.files.get(&key).cloned() else {
            return;
        };
        // A relaunch (a stale row rebuilding) replaces a Built row:
        // untrack the standing diff before the fresh mount, or it
        // leaks a tracked pair + editors (docs/editor/diff-canvas.md §7).
        self.teardown_row(store, &key);
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let route = key.clone();
        let built_width = built.width;
        let (body, body_height) = match built.failed.clone() {
            Some(error) => (RowBody::Failed(error), chrome.title_size * 3.0),
            None => {
                let mounted = fx.scope(
                    move |command: RowCommand| CanvasCommand::ToRow {
                        key: route.clone(),
                        command,
                    },
                    |fx| mounted(store, ui, &file, built, fx),
                );
                match mounted {
                    Some((pane, height)) => (RowBody::Built { pane }, height),
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
                    self.request = Some(himark::PanelRequest::Perform(std::sync::Arc::new(
                        himark::diff_canvas::OpenCanvasFile {
                            location: file.new.clone(),
                            // Land on the caret the row's diff editor
                            // holds — cmd-enter continues where the
                            // user was reading, like the standalone
                            // split-diff pane.
                            target: self.row_caret(store, key),
                        },
                    )));
                }
            }
            HeaderAction::OpenPane => {
                if let Some(file) = self.files.get(key) {
                    self.request = Some(himark::PanelRequest::Perform(std::sync::Arc::new(
                        himark::hichanges::OpenDiffForPair {
                            old: file.old.clone(),
                            new: file.new.clone(),
                        },
                    )));
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
    ) -> Option<std::ops::Range<himark::LineCol>> {
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
        let view = himark::OpenDocuments::diff_view_ref(store, pane.id())?;
        let right = view.right;
        // The canvas shows the INLINE face by default, where the
        // user's caret lives on the inline editor; both it and the
        // split-right editor ride the same (target) document.
        let editor = view
            .state
            .as_ref()
            .and_then(|state| state.inline_editor())
            .unwrap_or_else(|| right.editor());
        let document = himark::OpenDocuments::document_ref(store, right.document())?;
        let byte = document.caret_byte(editor);
        let mut text = document.text().view();
        let at = himark::line_col_at(&mut text, byte as usize);
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

/// Mount a row's diff over REGISTERED documents tracked by the Diffs
/// subsystem (docs/editor/diff-canvas.md §7): reuse the open
/// document for each side's location — or register the freshly-fetched
/// build — then `build_diff_view` tracks the pair (so the normalize
/// lane runs and edits compose) and mints a store-held `DiffView`. The
/// row holds only the `PairPane` id; its documents live in
/// `OpenDocuments`, exactly like any pane.
fn mounted(
    store: &mut Store,
    ui: &UiCtx,
    file: &CanvasFile,
    built: himark::BuiltFileDiff,
    fx: &mut Effects<'_, RowCommand>,
) -> Option<(crate::PairPane, f32)> {
    let theme = env::Themes::of(store);
    let gutter = theme.ui().editor_gutter.width;
    let editor_width = (built.width - gutter).max(120.0);

    let old_id = register_side(store, &file.old, built.old);
    let new_id = register_side(store, &file.new, built.new);

    let prep = crate::DiffPrep {
        operation: built.operation,
        marks: built.marks,
    };
    let id = crate::build_diff_view(store, old_id, new_id, Some(prep), editor_width)?;
    let mut pane = crate::PairPane::over(id);

    // Default to the inline face.
    fx.scope(RowCommand::Diff, |fx| {
        pane.perform(
            store,
            ui,
            UnifiedDiffCommand::SetLayout(himark::DiffLayout::Inline),
            fx,
        )
    });

    let height = {
        let frame = Arena::default();
        let thunk = imba::Layout::layout(
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

/// A document keyed to a `ResourceLocation` must BE the registered
/// `OpenDocuments` document for it (docs/editor/diff-canvas.md §7).
fn register_side(
    store: &mut Store,
    location: &ResourceLocation,
    document: himark::Document,
) -> himark::DocumentId {
    match himark::OpenDocuments::by_location(store, location) {
        Some(id) => id,
        None => {
            let revision = document.revision();
            himark::OpenDocuments::register(
                store,
                document,
                Some(location.clone()),
                location.name().to_owned(),
                revision,
            )
        }
    }
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
            CanvasCommand::Refresh => self.refresh(store, fx),
            CanvasCommand::PickupReveal => {
                if let Some(key) = self.reveal.take() {
                    self.rows
                        .content_mut()
                        .reveal_row(CanvasKey::File(key), Placement::TopLeftAt);
                    fx.settle();
                }
            }
            CanvasCommand::Landed { key, built } => self.land(store, ui, key, built, fx),
            CanvasCommand::ToRow { key, command } => self.to_row(key, command, store, ui, fx),
            CanvasCommand::Rows(command) => {
                match row_ask(&command) {
                    Some((index, RowCommand::Arm(width))) => self.launch(index, *width, fx),
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
                        if !text.trim().is_empty() {
                            self.request = Some(himark::PanelRequest::Perform(
                                std::sync::Arc::new(himark::hihistory::CommitHistory {
                                    folder: self.source.folder().clone(),
                                    message: text,
                                }),
                            ));
                        }
                    }
                    _ => {}
                }
                fx.scope(CanvasCommand::Rows, |fx| {
                    self.rows.perform(store, ui, command, fx)
                });
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, CanvasCommand> + imba::LayoutValue + 'a {
        let _ = arena;
        imba::laid(move |arena: &'a Arena, constraints: Constraints| {
            let refresh = self.seen != Some(canvas_generation(store, &self.source));
            let reveal = self.populated && self.reveal.is_some();
            let inner: imba::ThunkBox<'a, CanvasCommand> = match &self.note {
                Some(note) => {
                    let chrome = env::Themes::of(store).ui().chat.clone();
                    let text = note.clone();
                    let font = himark::fonts::ui_text_font(ui, chrome.title_size);
                    let color = chrome.loader_color.0;
                    let size = constraints.max;
                    imba::ThunkBox::new(
                        arena,
                        imba::leaf::leaf::<CanvasCommand>(size.width, size.height.min(240.0))
                            .paint_instead(move |_arena, canvas, rect| {
                                let mut paint = Paint::default();
                                paint.set_anti_alias(true);
                                paint.set_color(color);
                                let width = font.measure_str(&text, None).0;
                                canvas.draw_str(
                                    &text,
                                    (
                                        rect.left + (rect.width() - width) / 2.0,
                                        rect.top + rect.height() * 0.5,
                                    ),
                                    &font,
                                    &paint,
                                );
                            }),
                    )
                }
                None => imba::ThunkBox::new(
                    arena,
                    imba::Layout::layout(self.rows.display(arena, store, ui), arena, constraints)
                        .map(CanvasCommand::Rows)
                        // The list plants its sticky headers here —
                        // the canvas IS the pane face, so the band
                        // spans it edge to edge.
                        .overlay_host(imba::list::STICKY_HOST),
                ),
            };
            inner.wrap(move |widget| CanvasProbe {
                inner: widget,
                refresh,
                reveal,
            })
        })
    }
}

/// The ReconcileShell of the canvas: paint compares retained state
/// against the store and answers with commands — mutation stays in
/// perform.
struct CanvasProbe<Inner> {
    inner: Inner,
    refresh: bool,
    reveal: bool,
}

impl<'a, Inner: Widget<'a, CanvasCommand>> Widget<'a, CanvasCommand> for CanvasProbe<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, CanvasCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<CanvasCommand> {
        let mut result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) {
            if self.refresh {
                result = result.merge(EventResult::Command(CanvasCommand::Refresh));
            }
            if self.reveal {
                result = result.merge(EventResult::Command(CanvasCommand::PickupReveal));
            }
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, CanvasCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

// ------------------------------------------------------------ canvases

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CanvasId(u64);

impl CanvasId {
    fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// The store-held canvases (the `OpenDocuments` discipline): at most
/// one canvas per source, found — never rebuilt — on every open.
/// Canvases live while views retain them; the working-copy canvas
/// stays once opened.
#[derive(Clone, Default)]
pub struct Canvases(rpds::HashTrieMapSync<CanvasId, Canvas>);

impl Canvases {
    pub fn by_source(store: &Store, source: &CanvasSource) -> Option<CanvasId> {
        store
            .get::<Canvases>()?
            .0
            .iter()
            .find(|(_, canvas)| canvas.source == *source)
            .map(|(id, _)| *id)
    }

    fn find_or_create(store: &mut Store, source: &CanvasSource) -> CanvasId {
        if let Some(id) = Self::by_source(store, source) {
            return id;
        }
        let id = CanvasId::mint();
        let canvas = Canvas::fresh(source.clone());
        store.update::<Canvases>(|held| {
            held.0.insert_mut(id, canvas);
        });
        id
    }

    fn get<'a>(store: &'a Store, id: CanvasId) -> Option<&'a Canvas> {
        store.get::<Canvases>()?.0.get(&id)
    }

    fn take(store: &mut Store, id: CanvasId) -> Option<Canvas> {
        let canvas = Self::get(store, id)?.clone();
        store.update::<Canvases>(|held| {
            held.0.remove_mut(&id);
        });
        Some(canvas)
    }

    fn put(store: &mut Store, id: CanvasId, canvas: Canvas) {
        store.update::<Canvases>(|held| {
            held.0.insert_mut(id, canvas);
        });
    }

    fn retain(store: &mut Store, id: CanvasId) {
        if let Some(mut canvas) = Self::take(store, id) {
            canvas.refs += 1;
            Self::put(store, id, canvas);
        }
    }

    fn release(store: &mut Store, id: CanvasId) {
        let Some(mut canvas) = Self::take(store, id) else {
            return;
        };
        canvas.refs = canvas.refs.saturating_sub(1);
        // View-retained lifetime — except the WORKING-COPY canvas,
        // which stays around once opened (its diffs keep serving the
        // next open for free).
        if canvas.refs > 0 || matches!(canvas.source, CanvasSource::WorkingCopy { .. }) {
            Self::put(store, id, canvas);
        }
    }

    pub fn set_reveal(store: &mut Store, id: CanvasId, key: ResourceLocation) {
        if let Some(mut canvas) = Self::take(store, id) {
            canvas.reveal = Some(key);
            Self::put(store, id, canvas);
        }
    }

    /// Reconcile every store-held canvas against the current change set
    /// / commit history — driven from the app's sync tick, so a
    /// canvas's file list stays current even when no panel is painting
    /// it. This is why `Canvases` is store state: clicking a file in
    /// the changes view reveals it because the row is already there
    /// (docs/editor/diff-canvas.md §7).
    pub fn sync(store: &mut Store) {
        let ids: Vec<CanvasId> = match store.get::<Canvases>() {
            Some(canvases) => canvases.0.keys().copied().collect(),
            None => return,
        };
        for id in ids {
            let stale = match Self::get(store, id) {
                Some(canvas) => canvas.seen != Some(canvas_generation(store, &canvas.source)),
                None => false,
            };
            if !stale {
                continue;
            }
            if let Some(mut canvas) = Self::take(store, id) {
                // A headless sync: the row-list membership (placeholders
                // for new files, retirement of removed) is pure state.
                // Any builds relaunched here land when the panel next
                // paints; the throwaway effects are dropped.
                let mut batch = imba::effect::Batch::new();
                canvas.refresh(store, &mut batch.effects());
                Self::put(store, id, canvas);
            }
        }
    }
}

/// The `himark::SyncObserver` that keeps `Canvases` current on the sync
/// tick. Registered at the edge alongside the row minter and navigator.
pub fn canvas_sync_observer() -> std::sync::Arc<himark::SyncObserver> {
    std::sync::Arc::new(|store: &mut Store| Canvases::sync(store))
}

/// The canvas PANEL — a REFERENCE view over the store-held canvas,
/// the `PairPane` shape: panes hold ids, state lives in `Canvases`,
/// and a second view of the same source costs nothing.
#[derive(Clone)]
pub struct DiffCanvasView {
    id: CanvasId,
    source: CanvasSource,
    request: Option<himark::PanelRequest>,
}

impl DiffCanvasView {
    pub fn over(store: &mut Store, source: CanvasSource) -> Self {
        let id = Canvases::find_or_create(store, &source);
        Canvases::retain(store, id);
        Self {
            id,
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
        Canvases::get(store, self.id)
    }

    /// TEST SUPPORT: a view over a canvas seeded with one BUILT row.
    #[doc(hidden)]
    pub fn seeded_for_tests(
        store: &mut Store,
        ui: &UiCtx,
        source: CanvasSource,
        file: CanvasFile,
        built: himark::BuiltFileDiff,
    ) -> Self {
        let view = Self::over(store, source);
        if let Some(mut canvas) = Canvases::take(store, view.id) {
            canvas.seed_built_for_tests(store, ui, file, built);
            Canvases::put(store, view.id, canvas);
        }
        view
    }

    /// TEST SUPPORT: the registered `DiffViewId` backing a built row.
    #[doc(hidden)]
    pub fn probe_pair(
        &self,
        store: &Store,
        key: &himark::ResourceLocation,
    ) -> Option<himark::DiffViewId> {
        self.canvas(store)?.probe_pair(key)
    }

    #[doc(hidden)]
    pub fn probe_cover(&self, store: &Store, key: &himark::ResourceLocation) -> Option<usize> {
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
        generation: u64,
        listing: himark::diff_canvas::CanvasListing,
    ) {
        if let Some(mut canvas) = Canvases::take(store, self.id) {
            let mut batch = imba::effect::Batch::new();
            canvas.adopt(store, generation, listing, &mut batch.effects());
            Canvases::put(store, self.id, canvas);
        }
    }

    /// TEST SUPPORT: feed a fresh listing straight into the reconcile
    /// (bypassing the Changes feed); returns how many builds it
    /// relaunched.
    #[doc(hidden)]
    pub fn reconcile_for_tests(&self, store: &mut Store, files: Vec<CanvasFile>) -> usize {
        let mut launched = 0;
        if let Some(mut canvas) = Canvases::take(store, self.id) {
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
            Canvases::put(store, self.id, canvas);
        }
        launched
    }

    /// TEST SUPPORT: land a build for a key, as the effect would.
    #[doc(hidden)]
    pub fn land_for_tests(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        key: himark::ResourceLocation,
        built: himark::BuiltFileDiff,
    ) {
        if let Some(mut canvas) = Canvases::take(store, self.id) {
            let mut batch = imba::effect::Batch::new();
            canvas.land(store, ui, key, built, &mut batch.effects());
            Canvases::put(store, self.id, canvas);
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
    pub fn probe_focused_row(&self, store: &Store) -> Option<usize> {
        self.canvas(store)?.probe_focused_row()
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
    pub fn probe_layouts(&self, store: &Store) -> Vec<(String, himark::DiffLayout)> {
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

    #[doc(hidden)]
    pub fn probe_texts(&self, store: &Store) -> Vec<(String, Vec<String>)> {
        self.canvas(store)
            .map(|canvas| canvas.probe_texts(store))
            .unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn probe_geometry(&self, store: &Store) -> Vec<(f32, Vec<(u32, f32)>)> {
        self.canvas(store)
            .map(|canvas| canvas.probe_geometry(store))
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
        let Some(mut canvas) = Canvases::take(store, self.id) else {
            return;
        };
        canvas.perform(store, ui, command, fx);
        // Requests are the PANEL's ask (`take_request` has no store):
        // pull what the canvas minted into the view.
        if let Some(request) = canvas.request.take() {
            self.request = Some(request);
        }
        Canvases::put(store, self.id, canvas);
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |arena: &'a Arena, constraints: Constraints| match self.canvas(store) {
                Some(canvas) => {
                    imba::Layout::layout(canvas.display(arena, store, ui), arena, constraints)
                }
                None => imba::ThunkBox::new(
                    arena,
                    imba::leaf::leaf::<CanvasCommand>(constraints.max.width.max(1.0), 1.0),
                ),
            },
        )
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

pub enum RowCommand {
    /// The placeholder painted un-armed: build me, at this width.
    Arm(f32),

    Header(HeaderAction),

    Diff(UnifiedDiffCommand),

    Composer(ComposerCommand),

    Rewrap(f32),
}

pub enum ComposerCommand {
    Message(himark::EditorCommand),

    /// A press on the well outside the editor's own face.
    Focus,

    /// ⌘⏎ or the COMMIT button. The CANVAS posts the CommitHistory
    /// ask (it owns the source folder and the panel request); the row
    /// then resets its box.
    Commit,
}

#[derive(Clone)]
enum RowBody {
    Placeholder { armed: bool },
    Failed(String),
    Built { pane: crate::PairPane },
}

#[derive(Clone)]
pub(crate) enum CanvasRow {
    Banner(BannerRow),
    Header(HeaderRow),
    Diff(DiffRow),
}

/// The canvas's first row (docs/editor/diff-canvas.md): the commit's message
/// and author on a commit canvas, the commit composer on the
/// working-copy canvas.
#[derive(Clone)]
pub(crate) enum BannerRow {
    Commit {
        message: String,
        author: String,
    },
    Composer {
        message: himark::EditorView,
        focused: bool,
    },
}

#[derive(Clone)]
pub(crate) struct HeaderRow {
    file: CanvasFile,
    collapsed: bool,

    /// Whether a diff stands behind this header — the face toggle
    /// only shows then.
    built: bool,
}

#[derive(Clone)]
pub(crate) struct DiffRow {
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

fn header_band(theme: &himark::Theme) -> f32 {
    let h1 = theme.resolve([himark::StyleId::Header(1)]);
    h1.font_size.unwrap_or(48.0) + h1.block_gap.unwrap_or(24.0)
}

/// The composer banner's height — the CHAT COMPOSER's whole footprint
/// (higent/chat.rs): the input band (one chat line at rest, `box_pad`
/// = pad × 0.75 above and below) and the TOOLBAR row under it, ruled
/// off. The box grows UNBOUNDED with the message — a commit message
/// is as long as its author wants it; the canvas just scrolls.
fn composer_band(theme: &himark::Theme, message: Option<&himark::EditorView>) -> f32 {
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
fn fresh_composer_box(store: &Store) -> himark::EditorView {
    let fonts = env::Fonts::of(store)();
    let theme = env::Themes::of(store);
    let document =
        himark::Document::new(himark::Text::from_string_exact(""), himark::Markup::new())
            .with_syntax(
                himark::Syntax::new("markdown", None, himark::Markup::new()),
                &[],
            );
    let mut view = himark::EditorView::of_document(document, 600.0, &fonts, &theme);
    view.set_placeholder("Commit message", &fonts, &theme);
    view
}

const BANNER_MESSAGE_LINES: usize = 12;

fn commit_band(theme: &himark::Theme, message: &str) -> f32 {
    let chat = theme.ui().chat.clone();
    let line = chat.title_size * 1.5;
    let lines = message.lines().take(BANNER_MESSAGE_LINES).count().max(1) + 1; // + the author line
    lines as f32 * line + chat.pad * 2.0
}

fn reserved_body(theme: &himark::Theme, file: &CanvasFile) -> f32 {
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
            CanvasRow::Banner(_) => imba::focus::FocusData::default(),
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
            crate::teardown_diff_view(store, pane.id());
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
                let BannerRow::Composer { message, focused } = banner else {
                    return;
                };
                match command {
                    RowCommand::Composer(ComposerCommand::Message(command)) => {
                        if matches!(command, himark::EditorCommand::Click { .. }) && !*focused {
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
                    RowCommand::Composer(ComposerCommand::Commit) => {
                        *message = fresh_composer_box(store);
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
                                    .resize(editor, width, 0, &fonts, &theme, fx)
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
                let Some(view) = crate::gathered_view(store, pane.id()) else {
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
                fx.scope(RowCommand::Diff, |fx| {
                    crate::rewrap_pair(store, ui, id, width, fx)
                });
            }
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
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

impl imba::LayoutValue for RowFrame<'_> {}

impl<'a> imba::Layout<'a, RowCommand> for RowFrame<'a> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, RowCommand> {
        use imba::LayoutExt as _;
        let RowFrame { row, store, ui } = self;
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let width = constraints.max.width.max(1.0);
        let gutter = theme.ui().editor_gutter.width;

        match row {
            CanvasRow::Banner(BannerRow::Commit { message, author }) => {
                let band = commit_band(&theme, message);
                let line = chrome.title_size * 1.5;
                let inset = chrome.pad;
                let title_font = himark::fonts::ui_text_font(ui, chrome.title_size);
                let body_font = himark::fonts::ui_text_font(ui, chrome.title_size * 0.9);
                let text_color = chrome.text_color.0;
                let dim = chrome.loader_color.0;
                let title_size = chrome.title_size;
                let lines: Vec<String> = message
                    .lines()
                    .take(BANNER_MESSAGE_LINES)
                    .map(str::to_owned)
                    .collect();
                let author = author.clone();
                let face = imba::leaf::leaf::<RowCommand>(width, band).paint_instead(
                    move |_arena, canvas, rect| {
                        let mut paint = Paint::default();
                        paint.set_anti_alias(true);
                        let mut y = rect.top + inset + title_size;
                        paint.set_color(text_color);
                        for (n, text) in lines.iter().enumerate() {
                            let font = match n {
                                0 => &title_font,
                                _ => &body_font,
                            };
                            canvas.draw_str(text, (rect.left + inset, y), font, &paint);
                            y += line;
                        }
                        paint.set_color(dim);
                        canvas.draw_str(&author, (rect.left + inset, y), &body_font, &paint);
                    },
                );
                imba::ThunkBox::new(arena, face)
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
                let caps_font = himark::fonts::ui_font(ui, combo.label_size * 1.1);
                let key_font = himark::fonts::ui_text_font(ui, ui_theme.peeker.hint_size * 0.95);
                let label = "COMMIT";
                let cell_width = label
                    .chars()
                    .map(|ch| caps_font.measure_str(ch.to_string(), None).0 + 1.5)
                    .sum::<f32>()
                    + key_font.measure_str("⌘⏎", None).0
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
                let cell = imba::Row::new(arena)
                    .gap(combo.gap)
                    .child(
                        imba::text(label, caps_font.clone(), on_accent)
                            .tracking(1.5)
                            .pad_insets(imba::Insets {
                                left: 0.0,
                                top: (mid + caps_font.size() * 0.35 - caps_ascent).max(0.0),
                                right: 0.0,
                                bottom: 0.0,
                            }),
                    )
                    .child(imba::text("⌘⏎", key_font.clone(), accent_soft).pad_insets(
                        imba::Insets {
                            left: 0.0,
                            top: (mid + key_font.size() * 0.35 - key_ascent).max(0.0),
                            right: 0.0,
                            bottom: 0.0,
                        },
                    ))
                    .pad_insets(imba::Insets {
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
                    imba::Layout::layout(
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
                let band = header_band(&theme);
                let face = HeaderFace::new(store, ui, header, band, width);
                imba::ThunkBox::new(arena, imba::eager(HeaderWidget { face }))
            }
            CanvasRow::Diff(diff) => match &diff.body {
                RowBody::Placeholder { armed } => {
                    let body_height = (reserved_body(&theme, &diff.file) - chrome.gap).max(0.0);
                    let skeleton = skeleton_layout(arena, &theme, &diff.file, width, body_height);
                    let card = imba::ZBox::new(arena)
                        .child(imba::spacer(width, body_height))
                        .child(imba::fixed(skeleton))
                        .pad_insets(imba::Insets {
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
                    let font = himark::fonts::ui_text_font(ui, chrome.title_size * 0.85);
                    let color = chrome.loader_color.0;
                    let body = imba::leaf::leaf::<RowCommand>(width, chrome.title_size * 3.0)
                        .paint_instead(move |_arena, canvas, rect| {
                            let mut paint = Paint::default();
                            paint.set_anti_alias(true);
                            paint.set_color(color);
                            canvas.draw_str(
                                &text,
                                (rect.left + 16.0, rect.top + rect.height() * 0.5),
                                &font,
                                &paint,
                            );
                        });
                    imba::ZBox::new(arena)
                        .child(imba::spacer(width, chrome.title_size * 3.0))
                        .child(imba::fixed(body))
                        .pad_insets(imba::Insets {
                            left: 0.0,
                            top: 0.0,
                            right: 0.0,
                            bottom: chrome.gap,
                        })
                        .layout(arena, constraints)
                }
                RowBody::Built { pane } => {
                    let editor_target = (width - gutter).max(120.0);
                    let body = imba::Layout::layout(
                        pane.display(arena, store, ui),
                        arena,
                        Constraints {
                            min: Size::new(editor_target, 0.0),
                            max: Size::new(editor_target + gutter, f32::MAX),
                        },
                    )
                    .map(RowCommand::Diff);
                    let body_height = Thunk::size(&body).height;
                    // The row-level rewrap governs the INLINE face
                    // only. On the split face the halves report their
                    // own painted geometry (`reports_geometry`) and
                    // resize themselves to the half-pane width — a
                    // row-level rewrap to the full width would fight
                    // them every frame.
                    let rewrap = match crate::gathered_view(store, pane.id()) {
                        Some(view) => match view.layout {
                            himark::DiffLayout::Split => None,
                            himark::DiffLayout::Inline => {
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
                    let card = imba::ZBox::new(arena)
                        .child(imba::spacer(width, body_height))
                        .child(imba::fixed(body))
                        .pad_insets(imba::Insets {
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

/// The Header-1 band, with its affordances: the collapse chevron and
/// the `+N −M` trail at the left, the file name right-aligned before
/// the three buttons at the right edge — toggle layout, open file,
/// open the side-by-side pane. The body of the band opens the file.
struct HeaderFace {
    title: String,
    added: Option<i64>,
    removed: Option<i64>,
    collapsed: bool,

    band: f32,
    width: f32,
    inset: f32,

    title_font: skia_safe::Font,
    title_size: f32,
    title_color: skia_safe::Color,
    trail_font: skia_safe::Font,
    added_color: skia_safe::Color,
    removed_color: skia_safe::Color,
    affordance_color: skia_safe::Color,

    chevron: Rect,
    buttons: Vec<(HeaderAction, Rect)>,
}

impl HeaderFace {
    fn new(store: &Store, ui: &UiCtx, header: &HeaderRow, band: f32, width: f32) -> Self {
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let h1 = theme.resolve([himark::StyleId::Header(1)]);
        let size = h1.font_size.unwrap_or(48.0);
        let mut title_font = himark::fonts::ui_text_font(ui, size);
        if h1.bold {
            title_font.set_embolden(true);
        }
        let inset = chrome.pad;
        let glyph = chrome.title_size * 1.2;
        let zone = glyph + chrome.title_size;

        // Button zones, right edge inward: [toggle] [open] [pane].
        let mut buttons = Vec::new();
        let mut right = width - inset;
        for action in [
            HeaderAction::OpenPane,
            HeaderAction::OpenFile,
            HeaderAction::ToggleFace,
        ] {
            if action == HeaderAction::ToggleFace && !header.built {
                continue;
            }
            buttons.push((action, Rect::from_xywh(right - zone, 0.0, zone, band)));
            right -= zone;
        }

        Self {
            title: header.file.title.clone(),
            added: header.file.added.filter(|n| *n > 0),
            removed: header.file.removed.filter(|n| *n > 0),
            collapsed: header.collapsed,
            band,
            width,
            inset,
            title_font,
            title_size: size,
            title_color: h1.color.unwrap_or(chrome.text_color.0),
            trail_font: himark::fonts::ui_text_font(ui, chrome.title_size),
            added_color: chrome.added_color.0,
            removed_color: chrome.removed_color.0,
            affordance_color: chrome.loader_color.0,
            chevron: Rect::from_xywh(0.0, 0.0, inset + chrome.title_size, band),
            buttons,
        }
    }

    fn action_at(&self, x: f32) -> HeaderAction {
        if self.chevron.right > x {
            return HeaderAction::ToggleCollapse;
        }
        for (action, zone) in &self.buttons {
            if x >= zone.left && x < zone.right {
                return *action;
            }
        }
        HeaderAction::OpenFile
    }

    fn paint(&self, canvas: &skia_safe::Canvas, rect: Rect) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        let baseline = rect.top + self.title_size;

        // The chevron: right-pointing when collapsed, down when open.
        let glyph = self.trail_font.size() * 0.5;
        let center = (
            rect.left + self.chevron.left + self.inset * 0.5 + glyph * 0.5,
            baseline - glyph * 0.6,
        );
        paint.set_style(skia_safe::paint::Style::Stroke);
        paint.set_stroke_width((glyph * 0.22).max(1.0));
        paint.set_stroke_cap(skia_safe::paint::Cap::Round);
        paint.set_color(self.affordance_color);
        let mut path = skia_safe::PathBuilder::new();
        if self.collapsed {
            path.move_to((center.0 - glyph * 0.25, center.1 - glyph * 0.5));
            path.line_to((center.0 + glyph * 0.35, center.1));
            path.line_to((center.0 - glyph * 0.25, center.1 + glyph * 0.5));
        } else {
            path.move_to((center.0 - glyph * 0.5, center.1 - glyph * 0.25));
            path.line_to((center.0, center.1 + glyph * 0.35));
            path.line_to((center.0 + glyph * 0.5, center.1 - glyph * 0.25));
        }
        canvas.draw_path(&path.detach(), &paint);
        paint.set_style(skia_safe::paint::Style::Fill);

        // The +N −M trail after the chevron.
        let mut x = rect.left + self.chevron.right;
        if let Some(added) = self.added {
            paint.set_color(self.added_color);
            let label = format!("+{added}");
            canvas.draw_str(&label, (x, baseline), &self.trail_font, &paint);
            x += self.trail_font.measure_str(&label, None).0 + 8.0;
        }
        if let Some(removed) = self.removed {
            paint.set_color(self.removed_color);
            canvas.draw_str(
                &format!("−{removed}"),
                (x, baseline),
                &self.trail_font,
                &paint,
            );
        }

        // The buttons, hairline glyphs on the affordance color.
        paint.set_style(skia_safe::paint::Style::Stroke);
        paint.set_color(self.affordance_color);
        let side = self.trail_font.size() * 1.05;
        paint.set_stroke_width((side * 0.11).max(1.0));
        for (action, zone) in &self.buttons {
            let center = (
                rect.left + zone.left + zone.width() * 0.5,
                baseline - side * 0.42,
            );
            let half = side * 0.5;
            let frame = Rect::from_xywh(center.0 - half, center.1 - half, side, side);
            match action {
                HeaderAction::ToggleFace => {
                    // Two columns — switch the diff's face.
                    canvas.draw_round_rect(frame, 2.0, 2.0, &paint);
                    canvas.draw_line((center.0, frame.top), (center.0, frame.bottom), &paint);
                }
                HeaderAction::OpenFile => {
                    // A corner arrow leaving the box.
                    let inset = side * 0.22;
                    let mut path = skia_safe::PathBuilder::new();
                    path.move_to((frame.left + side * 0.5, frame.top + inset));
                    path.line_to((frame.left + inset, frame.top + inset));
                    path.line_to((frame.left + inset, frame.bottom - inset));
                    path.line_to((frame.right - inset, frame.bottom - inset));
                    path.line_to((frame.right - inset, frame.top + side * 0.5));
                    canvas.draw_path(&path.detach(), &paint);
                    let mut arrow = skia_safe::PathBuilder::new();
                    arrow.move_to((center.0 + side * 0.05, center.1 - side * 0.05));
                    arrow.line_to((frame.right, frame.top));
                    arrow.move_to((frame.right - side * 0.32, frame.top));
                    arrow.line_to((frame.right, frame.top));
                    arrow.line_to((frame.right, frame.top + side * 0.32));
                    canvas.draw_path(&arrow.detach(), &paint);
                }
                HeaderAction::OpenPane => {
                    // A framed pair of panes — the standalone
                    // side-by-side diff.
                    canvas.draw_round_rect(frame, 2.0, 2.0, &paint);
                    let third = frame.left + frame.width() * 0.5;
                    canvas.draw_line((third, frame.top), (third, frame.bottom), &paint);
                    let mid = frame.top + frame.height() * 0.5;
                    canvas.draw_line((frame.left, mid), (third, mid), &paint);
                }
                _ => {}
            }
        }
        paint.set_style(skia_safe::paint::Style::Fill);

        // The file name, right-aligned before the buttons.
        let buttons_left = self
            .buttons
            .last()
            .map(|(_, zone)| zone.left)
            .unwrap_or(self.width - self.inset);
        paint.set_color(self.title_color);
        let title_width = self.title_font.measure_str(&self.title, None).0;
        canvas.draw_str(
            &self.title,
            (
                (rect.left + buttons_left - self.inset - title_width)
                    .max(rect.left + self.chevron.right),
                baseline,
            ),
            &self.title_font,
            &paint,
        );
    }
}

struct HeaderWidget {
    face: HeaderFace,
}

impl<'a> Widget<'a, RowCommand> for HeaderWidget {
    fn size(&self) -> Size {
        Size::new(self.face.width, self.face.band)
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<RowCommand> {
        match event {
            Event::Paint { canvas, .. } => {
                self.face.paint(canvas, Rect::from_size(self.size()));
                EventResult::Handled
            }
            Event::MouseDown {
                button: imba::event::MouseButton::Left,
                point,
                ..
            } => EventResult::Command(RowCommand::Header(self.face.action_at(point.x))),
            _ => EventResult::Ignored,
        }
    }
}

/// The skeleton under a pending header: rounded bars at the line
/// rhythm, tinted from the diff palette, the mix proportioned to the
/// entry's added:removed ratio — it already READS like the diff it
/// stands for.
fn skeleton_layout<'a>(
    arena: &'a Arena,
    theme: &himark::Theme,
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

use himark::diff_canvas::CanvasPlace;

/// Answers `CanvasPlace` navigations: find — or create — the source's
/// store-held canvas and hand back a REFERENCE view of it. Reuse is
/// the lookup; nothing is ever rebuilt for a second open.
pub struct CanvasNavigator;

impl himark::Navigator for CanvasNavigator {
    type Place = CanvasPlace;

    fn navigate(
        &self,
        store: &mut Store,
        _window: himark::WindowId,
        place: &CanvasPlace,
        _fx: &mut himark::AppFx<'_>,
    ) -> Option<himark::Panel> {
        let view = DiffCanvasView::over(store, place.source.clone());
        if let Some(key) = &place.reveal {
            Canvases::set_reveal(store, view.id(), key.clone());
        }
        Some(himark::Panel::Plugin(Box::new(view)))
    }
}

impl himark::PanelView for DiffCanvasView {
    type Place = CanvasPlace;

    fn family_row(&self) -> Option<himark::FamilyRow> {
        Some(himark::FamilyRow::Canvas(self.source.clone()))
    }

    fn navigation_location(&self, _store: &Store) -> Option<CanvasPlace> {
        Some(CanvasPlace {
            source: self.source.clone(),
            reveal: None,
        })
    }

    fn navigate_to(
        &mut self,
        store: &mut Store,
        place: &CanvasPlace,
        _fx: &mut himark::AppFx<'_>,
    ) -> bool {
        if place.source != self.source {
            return false;
        }
        // Reuse IN PLACE: arm the reveal on the shared canvas — the
        // paint probe picks it up next frame.
        if let Some(key) = &place.reveal {
            Canvases::set_reveal(store, self.id, key.clone());
        }
        true
    }

    fn title(&self, store: &Store) -> String {
        self.source.title(store)
    }

    fn take_request(&mut self) -> Option<himark::PanelRequest> {
        self.request.take()
    }

    fn dismantle(&mut self, store: &mut Store) {
        // Row documents and editors are ROW-owned — they die with the
        // canvas when the collection lets go of it.
        Canvases::release(store, self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
