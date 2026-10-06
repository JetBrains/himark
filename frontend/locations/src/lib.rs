// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The location-lists COLLECTION (docs/entities.md,
//! docs/ui/location-list.md): the resolved shape of a result stream
//! and the session's standing feed rows, addressed by
//! `(Id<LocationLists>, FeedId)`. Pure model — the stream pump lives
//! with the drivers (the shell's wire lanes); the faces — the
//! search tab, the peek card — and the washes are this crate's too
//! (`search`, `peek`, `views`). No
//! document is fetched here and no URI parsed: locations arrive
//! resolved.

pub mod peek;
pub mod search;
pub mod views;

use editor::location::ResourceLocation;
use imba::store::Store;

/// One streamed location with its URI resolved at the route edge
/// into a real location (the model never parses URIs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundLocation {
    pub location: ResourceLocation,
    /// 0-based line.
    pub line: u32,
    /// 0-based byte column within the line.
    pub column: u32,
    /// Match length in bytes, clamped for display.
    pub length: u32,
    pub context: String,
    pub context_column_start: u32,
}

impl FoundLocation {
    /// The navigation target: the match's span as line/col — the
    /// door clamps against live text, so staleness degrades to the
    /// nearest sane position.
    pub fn target(&self) -> std::ops::Range<documents::text_ext::LineCol> {
        let start = documents::text_ext::LineCol {
            line: self.line,
            col: self.column,
        };
        let end = documents::text_ext::LineCol {
            line: self.line,
            col: self.column.saturating_add(self.length),
        };
        start..end
    }
}

/// Tree rows address by domain key — no id minting
/// (docs/ui/list-tree.md): a directory or file row IS its location;
/// an occurrence row its (location, line, column).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum LocationKey {
    Node(ResourceLocation),
    Hit(ResourceLocation, u32, u32),
}

/// A location list's identity — every result set (a search query, a
/// references ask) is one feed, minted here and addressed by id, the
/// session-row pattern: surfaces (the Search dock tab, the go-to
/// peek) are FACES over a feed; the feed and its stream outlive any
/// face, so promoting a peek into the dock reuses the feed instead
/// of asking again.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FeedId(u64);

impl FeedId {
    pub fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// One feed: the accumulated results and the stream's resolution
/// flags — the live stream end itself (channel, poll token) is the
/// DRIVER's, in the session's wire row. `generation` bumps on every
/// fold so faces refresh on paint.
#[derive(Clone, Default)]
#[doc(hidden)]
pub struct LocationsFeedRow {
    pub title: String,
    /// Seeds the query input. Empty for LSP result sets.
    pub query: String,
    pub generation: u64,
    pub locations: rpds::VectorSync<FoundLocation>,
    pub done: bool,
    pub truncated: bool,
    /// The find-results washes this feed installed on opened
    /// documents: markup id plus the ranges last pushed (the change
    /// set a removal brings, the find-bar discipline).
    pub washes: rpds::HashTrieMapSync<
        documents::DocumentId,
        (editor::markup::MarkupId, rpds::VectorSync<(u32, u32)>),
    >,
}

/// What a face may ask of the lists — model vocabulary; the wire
/// driver's lane turns an ask into traffic.
#[derive(Clone)]
pub enum LocationsAsk {
    /// Stream a content search into a feed.
    Search { feed: FeedId, query: String },

    /// Stream an LSP location answer into a feed.
    Lsp {
        feed: FeedId,
        kind: LspKind,
        location: ResourceLocation,
        position: documents::text_ext::LineCol,
    },

    /// Stop a feed's stream, keeping what landed.
    Stop(FeedId),

    /// Dispose a feed: stream, washes and row.
    Dispose(FeedId),
}

/// The location-answering ask flavors — a mirror; the wire's own
/// enum lives with the effects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LspKind {
    References,
    Implementations,
}

/// The session's location lists — a COLLECTION (docs/entities.md
/// law 1): the feed rows are its private schema, addressed by
/// `(Id<LocationLists>, FeedId)`, wired to the session's documents at
/// the mint. The Search dock front and the pending wash notes are
/// collection state too, not store components.
#[derive(Clone)]
pub struct LocationLists {
    /// The documents the washes land in — wired at the session mint
    /// (law 4): the wash and dispose roads read it instead of
    /// re-deriving scope from a window that may have moved on.
    documents: imba::store::Id<documents::OpenDocuments>,

    feeds: rpds::HashTrieMapSync<FeedId, LocationsFeedRow>,

    /// Which feed fronts the Search dock tab.
    search: Option<FeedId>,

    /// Search-originated opens awaiting registration: the pick notes
    /// the location; the wash hook converts it into a wash when the
    /// open lands.
    pending_washes: rpds::HashTrieMapSync<ResourceLocation, FeedId>,

    /// The session folders a content search spans — stamped by the
    /// shell as the catalog grants them.
    folders: rpds::VectorSync<ResourceLocation>,

    /// Outbound intents the faces NOTED — the wire lane drains them.
    asks: Vec<LocationsAsk>,
}

impl LocationLists {
    /// A collection wired to the documents its washes land in —
    /// minted by the session ceremony.
    pub fn wired(documents: imba::store::Id<documents::OpenDocuments>) -> Self {
        Self {
            documents,
            feeds: rpds::HashTrieMapSync::new_sync(),
            search: None,
            pending_washes: rpds::HashTrieMapSync::new_sync(),
            folders: rpds::VectorSync::new_sync(),
            asks: Vec::new(),
        }
    }

    /// The folders a content search spans.
    pub fn folders(store: &Store, lists: imba::store::Id<LocationLists>) -> Vec<ResourceLocation> {
        store
            .entity::<LocationLists>(lists)
            .map(|held| held.folders.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The shell's stamp: adopt folders not yet held, keeping order.
    pub fn adopt_folders(
        store: &mut Store,
        lists: imba::store::Id<LocationLists>,
        folders: &[ResourceLocation],
    ) {
        let fresh: Vec<ResourceLocation> = {
            let Some(held) = store.entity::<LocationLists>(lists) else {
                return;
            };
            folders
                .iter()
                .filter(|folder| !held.folders.iter().any(|known| known == *folder))
                .cloned()
                .collect()
        };
        if fresh.is_empty() {
            return;
        }
        Self::update(store, lists, |held| {
            for folder in &fresh {
                held.folders.push_back_mut(folder.clone());
            }
        });
    }

    /// Note an ask for the wire lane — the faces' door.
    pub fn ask(store: &mut Store, lists: imba::store::Id<LocationLists>, ask: LocationsAsk) {
        Self::update(store, lists, |held| held.asks.push(ask));
    }

    pub fn owes_asks(store: &Store, lists: imba::store::Id<LocationLists>) -> bool {
        store
            .entity::<LocationLists>(lists)
            .is_some_and(|held| !held.asks.is_empty())
    }

    pub fn take_asks(
        store: &mut Store,
        lists: imba::store::Id<LocationLists>,
    ) -> Vec<LocationsAsk> {
        let Some(mut held) = store.entity::<LocationLists>(lists).cloned() else {
            return Vec::new();
        };
        let asks = std::mem::take(&mut held.asks);
        store.put_entity(lists, held);
        asks
    }

    pub fn row(
        store: &Store,
        lists: imba::store::Id<LocationLists>,
        feed: FeedId,
    ) -> Option<LocationsFeedRow> {
        Self::row_ref(store, lists, feed).cloned()
    }

    pub fn row_ref(
        store: &Store,
        lists: imba::store::Id<LocationLists>,
        feed: FeedId,
    ) -> Option<&LocationsFeedRow> {
        store.entity::<LocationLists>(lists)?.feeds.get(&feed)
    }

    pub fn put(
        store: &mut Store,
        lists: imba::store::Id<LocationLists>,
        feed: FeedId,
        row: LocationsFeedRow,
    ) {
        Self::update(store, lists, |held| {
            held.feeds.insert_mut(feed, row);
        });
    }

    pub fn remove(store: &mut Store, lists: imba::store::Id<LocationLists>, feed: FeedId) {
        Self::update(store, lists, |held| {
            held.feeds.remove_mut(&feed);
        });
    }

    /// The feed fronting the Search dock tab.
    pub fn search(store: &Store, lists: imba::store::Id<LocationLists>) -> Option<FeedId> {
        store.entity::<LocationLists>(lists)?.search
    }

    pub fn set_search(store: &mut Store, lists: imba::store::Id<LocationLists>, feed: FeedId) {
        Self::update(store, lists, |held| held.search = Some(feed));
    }

    /// Fold a resolved batch into the feed: results append, the
    /// stream flags OR in, the generation moves so faces refresh.
    pub fn fold_locations(
        store: &mut Store,
        lists: imba::store::Id<LocationLists>,
        feed: FeedId,
        locations: Vec<FoundLocation>,
        done: bool,
        truncated: bool,
    ) {
        Self::update_row(store, lists, feed, |row| {
            for found in locations {
                row.locations.push_back_mut(found);
            }
            row.done |= done;
            row.truncated |= truncated;
            row.generation += 1;
        });
    }

    /// Resolve a feed cut-off in plain sight: a failed ask, a failed
    /// batch, or a deliberate stop — a still-running row reads done
    /// and truncated after; a finished one only repaints.
    pub fn mark_cut(store: &mut Store, lists: imba::store::Id<LocationLists>, feed: FeedId) {
        Self::update_row(store, lists, feed, |row| {
            if !row.done {
                row.done = true;
                row.truncated = true;
            }
            row.generation += 1;
        });
    }

    /// Mutate one feed row in place; a gone row takes no write.
    fn update_row(
        store: &mut Store,
        lists: imba::store::Id<LocationLists>,
        feed: FeedId,
        mutate: impl FnOnce(&mut LocationsFeedRow),
    ) {
        Self::update(store, lists, |held| {
            if let Some(mut row) = held.feeds.get(&feed).cloned() {
                mutate(&mut row);
                held.feeds.insert_mut(feed, row);
            }
        });
    }

    /// The documents collection this one's washes land in.
    pub fn documents_of(
        store: &Store,
        lists: imba::store::Id<LocationLists>,
    ) -> Option<imba::store::Id<documents::OpenDocuments>> {
        Some(store.entity::<LocationLists>(lists)?.documents)
    }

    pub fn is_empty(&self) -> bool {
        self.feeds.is_empty()
            && self.search.is_none()
            && self.pending_washes.is_empty()
            && self.asks.is_empty()
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn note_wash(
        store: &mut Store,
        lists: imba::store::Id<LocationLists>,
        location: ResourceLocation,
        feed: FeedId,
    ) {
        Self::update(store, lists, |held| {
            held.pending_washes.insert_mut(location, feed);
        });
    }

    /// Claim the pending wash noted for a location, if any — the
    /// wash hook's half of the pick → open → wash chain.
    pub(crate) fn take_wash(
        store: &mut Store,
        lists: imba::store::Id<LocationLists>,
        location: &ResourceLocation,
    ) -> Option<FeedId> {
        let feed = store
            .entity::<LocationLists>(lists)?
            .pending_washes
            .get(location)
            .copied();
        if feed.is_some() {
            Self::update(store, lists, |held| {
                held.pending_washes.remove_mut(location);
            });
        }
        feed
    }

    /// Drop every pending wash note a feed left — disposal's sweep.
    pub(crate) fn sweep_washes(
        store: &mut Store,
        lists: imba::store::Id<LocationLists>,
        feed: FeedId,
    ) {
        Self::update(store, lists, |held| {
            let stale: Vec<ResourceLocation> = held
                .pending_washes
                .iter()
                .filter(|(_, held)| **held == feed)
                .map(|(location, _)| location.clone())
                .collect();
            for location in stale {
                held.pending_washes.remove_mut(&location);
            }
        });
    }

    /// Mutate in place; a gone collection takes no write — never
    /// minted from `Default` (a dangling id must not resurrect).
    fn update(
        store: &mut Store,
        lists: imba::store::Id<LocationLists>,
        mutate: impl FnOnce(&mut LocationLists),
    ) {
        let Some(mut held) = store.entity::<LocationLists>(lists).cloned() else {
            return;
        };
        mutate(&mut held);
        store.put_entity(lists, held);
    }
}

/// Open a feed row in "searching…" state — the surface shows
/// IMMEDIATELY; the stream attaches when the ask lands. A failed ask
/// resolves the row cut-off in plain sight instead of a silent
/// no-op.
pub fn open_feed(
    store: &mut Store,
    lists: imba::store::Id<LocationLists>,
    feed: FeedId,
    title: String,
    query: String,
) {
    LocationLists::put(
        store,
        lists,
        feed,
        LocationsFeedRow {
            title,
            query,
            generation: 1,
            ..LocationsFeedRow::default()
        },
    );
}
