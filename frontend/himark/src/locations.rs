// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Location lists, client-side (docs/ui/location-list.md): the
//! resolved shape of an `ahp-locations:/…` stream, the session's
//! standing feed row, and the dirs → files → occurrences forest the
//! search surfaces render. No document is fetched here — a row
//! renders from the location's own context.

use std::collections::BTreeMap;

use imba::store::Store;

use crate::forest::ForestNode;
use crate::{ResourceLocation, ResourceType};

/// One streamed location with its URI resolved at the route edge
/// into a real location (himark never parses URIs).
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
    pub fn target(&self) -> std::ops::Range<crate::LineCol> {
        let start = crate::LineCol {
            line: self.line,
            col: self.column,
        };
        let end = crate::LineCol {
            line: self.line,
            col: self.column.saturating_add(self.length),
        };
        start..end
    }
}

/// Resolve a wire batch against its channel's route — locations
/// whose URI the route cannot place are dropped.
pub fn resolve_batch(
    channel: &crate::LocationsChannel,
    batch: himark_ahp_ext_types::LocationList,
) -> Vec<FoundLocation> {
    batch
        .locations
        .into_iter()
        .filter_map(|location| {
            Some(FoundLocation {
                location: (channel.resolve)(&location.uri)?,
                line: location.line,
                column: location.column,
                length: location.length,
                context: location.context,
                context_column_start: location.context_column_start,
            })
        })
        .collect()
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
/// family-row pattern: surfaces (the Search dock tab, the go-to
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

/// One feed: the accumulated results plus the live stream end (the
/// channel rides behind Arcs, the Terminals precedent; the poll
/// token is a Copy id for cancellation). `generation` bumps on every
/// fold so faces refresh on paint.
#[derive(Clone, Default)]
pub struct LocationsFeedRow {
    pub title: String,
    /// Seeds the query input. Empty for LSP result sets.
    pub query: String,
    pub generation: u64,
    pub locations: rpds::VectorSync<FoundLocation>,
    pub done: bool,
    pub truncated: bool,
    pub channel: Option<crate::LocationsChannel>,
    pub poll: Option<imba::effect::CancellationToken>,
    /// The find-results washes this feed installed on opened
    /// documents: markup id plus the ranges last pushed (the change
    /// set a removal brings, the find-bar discipline).
    pub washes:
        rpds::HashTrieMapSync<crate::DocumentId, (crate::MarkupId, rpds::VectorSync<(u32, u32)>)>,
}

/// The session's location lists — a COLLECTION (docs/entities.md
/// law 1): the feed rows are its private schema, addressed by
/// `(Id<LocationLists>, FeedId)`, wired to the family's documents at
/// the mint. The Search dock front and the pending wash notes are
/// collection state too, not store components.
#[derive(Clone)]
pub struct LocationLists {
    /// The documents the washes land in — wired at the family mint
    /// (law 4): the wash and dispose roads read it instead of
    /// re-deriving scope from a window that may have moved on.
    documents: imba::store::Id<crate::OpenDocuments>,

    feeds: rpds::HashTrieMapSync<FeedId, LocationsFeedRow>,

    /// Which feed fronts the Search dock tab.
    search: Option<FeedId>,

    /// Search-originated opens awaiting registration: the pick notes
    /// the location; the wash hook converts it into a wash when the
    /// open lands.
    pending_washes: rpds::HashTrieMapSync<ResourceLocation, FeedId>,
}

impl LocationLists {
    /// A collection wired to the documents its washes land in —
    /// minted by the family ceremony.
    pub fn wired(documents: imba::store::Id<crate::OpenDocuments>) -> Self {
        Self {
            documents,
            feeds: rpds::HashTrieMapSync::new_sync(),
            search: None,
            pending_washes: rpds::HashTrieMapSync::new_sync(),
        }
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

    /// The documents collection this one's washes land in.
    pub(crate) fn documents_of(
        store: &Store,
        lists: imba::store::Id<LocationLists>,
    ) -> Option<imba::store::Id<crate::OpenDocuments>> {
        Some(store.entity::<LocationLists>(lists)?.documents)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.feeds.is_empty() && self.search.is_none() && self.pending_washes.is_empty()
    }

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

    fn take_wash(
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

    fn sweep_washes(store: &mut Store, lists: imba::store::Id<LocationLists>, feed: FeedId) {
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
/// IMMEDIATELY; the stream attaches when the ask lands
/// (`AttachFeedStream`). A failed ask resolves the row cut-off in
/// plain sight instead of a silent no-op.
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

/// Phase two of every ask: the channel landed — subscribe and start
/// the feed's own pump, view-independent. Pushed through
/// `AppRequests` by whichever surface asked.
pub struct AttachFeedStream {
    pub lists: imba::store::Id<LocationLists>,
    pub feed: FeedId,
    pub outcome: Result<crate::LocationsChannel, String>,
}

impl crate::DynamicCommand for AttachFeedStream {
    fn id(&self) -> &'static str {
        "locations.attach-stream"
    }

    fn name(&self) -> String {
        "Attach Location Stream".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(mut row) = LocationLists::row(store, self.lists, self.feed) else {
            return;
        };
        match self.outcome.clone() {
            Err(_) => {
                row.done = true;
                row.truncated = true;
                row.generation += 1;
                LocationLists::put(store, self.lists, self.feed, row);
            }
            Ok(channel) => {
                row.channel = Some(channel.clone());
                LocationLists::put(store, self.lists, self.feed, row);
                let (lists, feed) = (self.lists, self.feed);
                let _ = fx.push(
                    imba::effect::AnyEffect::new(crate::higent::SubscribeLocationsEffect {
                        seat: channel.seat,
                        channel: channel.channel,
                    })
                    .map(move |outcome| {
                        crate::AppCommand::Landing(
                            window,
                            Box::new(FeedBatch {
                                lists,
                                feed,
                                batches: outcome.map(|snapshot| vec![snapshot]),
                            }),
                        )
                    }),
                );
            }
        }
    }
}

/// The pump's landing: fold, then poll again while the stream runs.
struct FeedBatch {
    lists: imba::store::Id<LocationLists>,
    feed: FeedId,
    batches: Result<Vec<himark_ahp_ext_types::LocationList>, String>,
}

impl crate::LandingCommand for FeedBatch {
    fn perform(
        self: Box<Self>,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(mut row) = LocationLists::row(store, self.lists, self.feed) else {
            return; // disposed while in flight — the unsubscribe ran
        };
        let Some(channel) = row.channel.clone() else {
            return;
        };
        match self.batches {
            Err(_) => {
                row.done = true;
                row.truncated = true;
            }
            Ok(batches) => {
                for batch in batches {
                    row.done |= batch.done;
                    row.truncated |= batch.truncated;
                    for found in resolve_batch(&channel, batch) {
                        row.locations.push_back_mut(found);
                    }
                }
            }
        }
        row.generation += 1;
        let running = !row.done;
        let (lists, feed) = (self.lists, self.feed);
        if running {
            let token = fx.push(
                imba::effect::AnyEffect::new(crate::higent::PollLocationsEffect {
                    seat: std::sync::Arc::clone(&channel.seat),
                    channel: channel.channel.clone(),
                })
                .map(move |batches| {
                    crate::AppCommand::Landing(
                        window,
                        Box::new(FeedBatch {
                            lists,
                            feed,
                            batches: Ok(batches),
                        }),
                    )
                }),
            );
            row.poll = Some(token);
        } else {
            row.poll = None;
        }
        LocationLists::put(store, self.lists, self.feed, row);
    }
}

/// The registration hook: a search-picked document opened — wash it.
/// WIRED: minted by the family ceremony with the lists collection in
/// hand, installed SCOPED to the family's documents (docs/entities.md
/// law 4) — fires only for its own collection, dies with it.
pub struct LocationsWashHook {
    pub lists: imba::store::Id<LocationLists>,
}

impl crate::DocumentHook for LocationsWashHook {
    fn opened(
        &self,
        store: &mut Store,
        _documents: imba::store::Id<crate::OpenDocuments>,
        document: crate::DocumentId,
        location: Option<&crate::ResourceLocation>,
    ) {
        let Some(location) = location else {
            return;
        };
        if let Some(feed) = LocationLists::take_wash(store, self.lists, location) {
            let lists = self.lists;
            crate::AppRequests::push(
                store,
                std::sync::Arc::new(WashDocument {
                    lists,
                    feed,
                    document,
                }),
            );
        }
    }

    fn closing(
        &self,
        _store: &mut Store,
        _documents: imba::store::Id<crate::OpenDocuments>,
        _document: crate::DocumentId,
        _location: Option<&crate::ResourceLocation>,
        _doc: &crate::Document,
    ) {
    }
}

/// Install the feed's find-results markup on an opened document:
/// every occurrence of this feed in the file, washed
/// `StyleId::Match`, Document-scoped so every editor of the file —
/// current and future panes — shows it. Ranges resolve against the
/// LIVE text and shift with edits like all markup; the wash leaves
/// with the feed (`DisposeFeed`).
pub struct WashDocument {
    pub lists: imba::store::Id<LocationLists>,
    pub feed: FeedId,
    pub document: crate::DocumentId,
}

impl crate::DynamicCommand for WashDocument {
    fn id(&self) -> &'static str {
        "locations.wash-document"
    }

    fn name(&self) -> String {
        "Highlight Found Results".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        _window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let Some(mut row) = LocationLists::row(store, self.lists, self.feed) else {
            return;
        };
        if row.washes.contains_key(&self.document) {
            return;
        }
        // The collection's wired sibling, not the window's current
        // family — the window may have moved on since the pick.
        let Some(documents) = LocationLists::documents_of(store, self.lists) else {
            return;
        };
        let Some(location) = crate::OpenDocuments::location(store, documents, self.document) else {
            return;
        };
        let Some(mut document) = crate::OpenDocuments::document(store, documents, self.document)
        else {
            return;
        };

        let mut ranges: Vec<std::ops::Range<u32>> = {
            let mut view = document.text().view();
            row.locations
                .iter()
                .filter(|found| found.location == location)
                .map(|found| {
                    let target = found.target();
                    let start = crate::offset_at(&mut view, target.start) as u32;
                    start..(start + found.length.max(1))
                })
                .collect()
        };
        if ranges.is_empty() {
            crate::OpenDocuments::put_document(store, documents, self.document, document);
            return;
        }
        ranges.sort_by_key(|range| range.start);
        ranges.dedup();

        let fonts = crate::env::Fonts::of(store)();
        let theme = crate::env::Themes::of(store);
        let markup = crate::MarkupId::mint();
        document.ensure_document_markup(markup);
        let mut tints = crate::Markup::new();
        for range in &ranges {
            tints.push_styled(range.clone(), crate::theme::StyleId::Match);
        }
        let entity = self.document;
        fx.scope(
            move |command| {
                crate::AppCommand::at(
                    documents,
                    crate::app::DocumentsCommand::Editor(entity, command),
                )
            },
            |fx| document.replace_markup(markup, tints, &ranges, store, ui, &fonts, &theme, fx),
        );
        crate::OpenDocuments::put_document(store, documents, self.document, document);

        row.washes.insert_mut(
            self.document,
            (
                markup,
                ranges
                    .into_iter()
                    .map(|range| (range.start, range.end))
                    .collect(),
            ),
        );
        LocationLists::put(store, self.lists, self.feed, row);
    }
}

fn remove_washes(
    store: &mut Store,
    documents: imba::store::Id<crate::OpenDocuments>,
    ui: &imba::UiCtx,
    row: &LocationsFeedRow,
    fx: &mut crate::app::AppFx<'_>,
) {
    let fonts = crate::env::Fonts::of(store)();
    let theme = crate::env::Themes::of(store);
    for (id, (markup, pushed)) in row.washes.iter() {
        let Some(mut document) = crate::OpenDocuments::document(store, documents, *id) else {
            continue; // closed — the markup died with it
        };
        let changed: Vec<std::ops::Range<u32>> =
            pushed.iter().map(|(start, end)| *start..*end).collect();
        let entity = *id;
        fx.scope(
            move |command| {
                crate::AppCommand::at(
                    documents,
                    crate::app::DocumentsCommand::Editor(entity, command),
                )
            },
            |fx| document.remove_markup(*markup, &changed, store, ui, &fonts, &theme, fx),
        );
        crate::OpenDocuments::put_document(store, documents, *id, document);
    }
}

/// Stop a feed's stream, keeping what landed: cancel the pump,
/// unsubscribe (the host-side cancel), resolve the row cut-off.
pub struct StopFeed {
    pub lists: imba::store::Id<LocationLists>,
    pub feed: FeedId,
}

impl crate::DynamicCommand for StopFeed {
    fn id(&self) -> &'static str {
        "locations.stop-feed"
    }

    fn name(&self) -> String {
        "Stop Location Stream".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(mut row) = LocationLists::row(store, self.lists, self.feed) else {
            return;
        };
        if let Some(token) = row.poll.take() {
            fx.cancel(token);
        }
        if let Some(channel) = row.channel.take() {
            let _ = fx.push(
                imba::effect::AnyEffect::new(crate::higent::UnsubscribeLocationsEffect {
                    seat: channel.seat,
                    channel: channel.channel,
                })
                .map(move |()| crate::AppCommand::Landing(window, Box::new(NothingLanded))),
            );
        }
        if !row.done {
            row.done = true;
            row.truncated = true;
        }
        row.generation += 1;
        LocationLists::put(store, self.lists, self.feed, row);
    }
}

/// Dispose a feed: cancel its pump, unsubscribe its channel (the
/// host-side cancel), drop the row. Pushed through `AppRequests`.
pub struct DisposeFeed {
    pub lists: imba::store::Id<LocationLists>,
    pub feed: FeedId,
}

impl crate::DynamicCommand for DisposeFeed {
    fn id(&self) -> &'static str {
        "locations.dispose-feed"
    }

    fn name(&self) -> String {
        "Dispose Location Feed".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let Some(row) = LocationLists::row(store, self.lists, self.feed) else {
            return;
        };
        // The collection's wired sibling, not the window's current
        // family — the window may have moved on since the feed opened.
        if let Some(documents) = LocationLists::documents_of(store, self.lists) {
            remove_washes(store, documents, ui, &row, fx);
        }
        LocationLists::sweep_washes(store, self.lists, self.feed);
        if let Some(token) = row.poll {
            fx.cancel(token);
        }
        if let Some(channel) = row.channel {
            let _ = fx.push(
                imba::effect::AnyEffect::new(crate::higent::UnsubscribeLocationsEffect {
                    seat: channel.seat,
                    channel: channel.channel,
                })
                .map(move |()| crate::AppCommand::Landing(window, Box::new(NothingLanded))),
            );
        }
        LocationLists::remove(store, self.lists, self.feed);
    }
}

struct NothingLanded;

impl crate::LandingCommand for NothingLanded {
    fn perform(
        self: Box<Self>,
        _app: &mut crate::Application,
        _store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
    }
}

/// The peek's master forest: locations grouped by FILE (no directory
/// nesting — the card is compact), every occurrence a pickable leaf.
pub fn files_forest<'a>(
    store: &Store,
    rows: impl IntoIterator<Item = &'a FoundLocation>,
) -> Vec<ForestNode<LocationKey>> {
    let tree = crate::env::Themes::of(store).ui().tree.clone();
    let chip = tree.directory.0;
    let mut order: Vec<ResourceLocation> = Vec::new();
    let mut grouped: std::collections::HashMap<ResourceLocation, Vec<&FoundLocation>> =
        std::collections::HashMap::new();
    for found in rows {
        if !grouped.contains_key(&found.location) {
            order.push(found.location.clone());
        }
        grouped
            .entry(found.location.clone())
            .or_default()
            .push(found);
    }
    order
        .into_iter()
        .map(|location| {
            let hits = grouped.remove(&location).unwrap_or_default();
            ForestNode {
                key: LocationKey::Node(location.clone()),
                label: location.name().to_owned(),
                pick: true,
                dim: false,
                trail: vec![(format!("{}", hits.len()), chip)],
                tint: crate::TreeTint::File,
                action: None,
                children: hits
                    .into_iter()
                    .map(|found| ForestNode {
                        key: LocationKey::Hit(found.location.clone(), found.line, found.column),
                        label: found.context.trim().to_owned(),
                        pick: true,
                        dim: false,
                        trail: vec![(format!("{}", found.line + 1), chip)],
                        tint: crate::TreeTint::Label,
                        action: None,
                        children: Vec::new(),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Fold a location list into the dirs → files → occurrences forest.
/// Sorted by (authority, path, line, column) whatever order batches
/// landed in; single-child directory chains join into one row (the
/// TOC recipe); every occurrence is its own pickable leaf.
pub fn locations_forest<'a>(
    store: &Store,
    rows: impl IntoIterator<Item = &'a FoundLocation>,
) -> Vec<ForestNode<LocationKey>> {
    let tree = crate::env::Themes::of(store).ui().tree.clone();
    let position_color = tree.directory.0;
    let count_color = tree.directory.0;

    let mut sorted: Vec<&FoundLocation> = rows.into_iter().collect();
    sorted.sort_by(|a, b| {
        (
            a.location.authority().as_str(),
            a.location.path(),
            a.line,
            a.column,
        )
            .cmp(&(
                b.location.authority().as_str(),
                b.location.path(),
                b.line,
                b.column,
            ))
    });
    sorted.dedup_by(|a, b| a.location == b.location && a.line == b.line && a.column == b.column);

    #[derive(Default)]
    struct Trie<'a> {
        dirs: BTreeMap<String, Trie<'a>>,
        files: Vec<(ResourceLocation, Vec<&'a FoundLocation>)>,
    }

    let mut root = Trie::default();
    for found in sorted {
        let path = found.location.path();
        let mut level = &mut root;
        for segment in &path[..path.len().saturating_sub(1)] {
            level = level.dirs.entry(segment.clone()).or_default();
        }
        match level.files.last_mut() {
            Some((location, hits)) if *location == found.location => hits.push(found),
            _ => level.files.push((found.location.clone(), vec![found])),
        }
    }

    struct Emit {
        position_color: skia_safe::Color,
        count_color: skia_safe::Color,
    }

    impl Emit {
        fn dir(
            &self,
            trie: Trie<'_>,
            mut label: String,
            location: ResourceLocation,
        ) -> ForestNode<LocationKey> {
            let mut trie = trie;
            let mut location = location;
            while trie.files.is_empty() && trie.dirs.len() == 1 {
                let (segment, child) = trie.dirs.pop_first().expect("one child");
                label.push('/');
                label.push_str(&segment);
                location = location.child(ResourceType::directory(), &segment);
                trie = child;
            }
            ForestNode {
                key: LocationKey::Node(location.clone()),
                label,
                pick: false,
                dim: true,
                trail: Vec::new(),
                tint: crate::TreeTint::Directory,
                action: None,
                children: self.children(trie, &location),
            }
        }

        fn children(&self, trie: Trie<'_>, at: &ResourceLocation) -> Vec<ForestNode<LocationKey>> {
            let mut children = Vec::new();
            for (segment, child) in trie.dirs {
                let location = at.child(ResourceType::directory(), &segment);
                children.push(self.dir(child, segment, location));
            }
            for (location, hits) in trie.files {
                children.push(self.file(location, hits));
            }
            children
        }

        fn file(
            &self,
            location: ResourceLocation,
            hits: Vec<&FoundLocation>,
        ) -> ForestNode<LocationKey> {
            let leaves = hits
                .iter()
                .map(|found| ForestNode {
                    key: LocationKey::Hit(found.location.clone(), found.line, found.column),
                    label: found.context.trim().to_owned(),
                    pick: true,
                    dim: false,
                    trail: vec![(
                        format!("{}:{}", found.line + 1, found.column + 1),
                        self.position_color,
                    )],
                    tint: crate::TreeTint::Label,
                    action: None,
                    children: Vec::new(),
                })
                .collect();
            ForestNode {
                key: LocationKey::Node(location.clone()),
                label: location.name().to_owned(),
                pick: true,
                dim: false,
                trail: vec![(format!("{}", hits.len()), self.count_color)],
                tint: crate::TreeTint::File,
                action: None,
                children: leaves,
            }
        }
    }

    let emit = Emit {
        position_color,
        count_color,
    };
    // Authorities rarely mix; when they do, each gets its own root
    // ordering by the sort above. The root level emits every
    // top-level dir and any root-level files.
    let mut nodes = Vec::new();
    for (segment, child) in root.dirs {
        // The root dir's location needs the authority — take it from
        // the first file reachable in the subtree.
        fn first_authority<'a>(trie: &'a Trie<'a>) -> Option<&'a ResourceLocation> {
            trie.files
                .first()
                .map(|(location, _)| location)
                .or_else(|| trie.dirs.values().find_map(first_authority))
        }
        let Some(sample) = first_authority(&child) else {
            continue;
        };
        let location = ResourceLocation::new(
            ResourceType::directory(),
            sample.authority().clone(),
            vec![segment.clone()],
        );
        nodes.push(emit.dir(child, segment, location));
    }
    for (location, hits) in root.files {
        nodes.push(emit.file(location, hits));
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(path: &[&str], line: u32, column: u32, context: &str) -> FoundLocation {
        FoundLocation {
            location: ResourceLocation::new(
                crate::ResourceType::document(),
                crate::Authority::new("local"),
                path.iter()
                    .map(|segment| segment.to_string())
                    .collect::<Vec<String>>(),
            ),
            line,
            column,
            length: 4,
            context: context.to_owned(),
            context_column_start: 0,
        }
    }

    fn shape(nodes: &[ForestNode<LocationKey>], depth: usize, out: &mut Vec<(usize, String)>) {
        for node in nodes {
            out.push((depth, node.label.clone()));
            shape(&node.children, depth + 1, out);
        }
    }

    #[test]
    fn the_forest_nests_dirs_files_and_occurrences() {
        let store = Store::default();
        let rows = vec![
            found(&["work", "src", "b.rs"], 3, 0, "  beta"),
            found(&["work", "src", "a.rs"], 1, 2, "alpha one"),
            found(&["work", "src", "a.rs"], 0, 0, "alpha zero"),
            found(&["work", "README.md"], 5, 1, "readme hit"),
        ];
        let forest = locations_forest(&store, &rows);

        let mut rendered = Vec::new();
        shape(&forest, 0, &mut rendered);
        assert_eq!(
            rendered,
            [
                (0, "work".to_owned()),
                (1, "src".to_owned()),
                (2, "a.rs".to_owned()),
                (3, "alpha zero".to_owned()),
                (3, "alpha one".to_owned()),
                (2, "b.rs".to_owned()),
                (3, "beta".to_owned()),
                (1, "README.md".to_owned()),
                (2, "readme hit".to_owned()),
            ],
            "sorted whatever order batches landed in, contexts trimmed"
        );

        let src = &forest[0].children[0];
        assert!(matches!(&src.key, LocationKey::Node(location)
            if location.path().join("/") == "work/src" && location.kind().is_directory()));
        let hit = &src.children[0].children[0];
        assert!(matches!(&hit.key, LocationKey::Hit(_, 0, 0)));
        assert!(hit.pick && !src.children[0].children.is_empty());
        assert_eq!(src.children[0].trail[0].0, "2", "occurrence count chip");
        assert_eq!(hit.trail[0].0, "1:1", "1-based position chip");
    }

    #[test]
    fn single_child_dir_chains_join() {
        let store = Store::default();
        let rows = vec![found(&["deep", "one", "two", "leaf.rs"], 0, 0, "x")];
        let forest = locations_forest(&store, &rows);
        assert_eq!(forest.len(), 1);
        assert_eq!(forest[0].label, "deep/one/two");
        assert!(matches!(&forest[0].key, LocationKey::Node(location)
            if location.path().join("/") == "deep/one/two"));
        assert_eq!(forest[0].children[0].label, "leaf.rs");
    }

    #[test]
    fn duplicate_hits_fold_away_and_targets_span_the_match() {
        let store = Store::default();
        let rows = vec![
            found(&["a.rs"], 2, 7, "same"),
            found(&["a.rs"], 2, 7, "same"),
        ];
        let forest = locations_forest(&store, &rows);
        assert_eq!(forest[0].children.len(), 1);

        let target = rows[0].target();
        assert_eq!((target.start.line, target.start.col), (2, 7));
        assert_eq!(target.end.col, 11, "start + length");
    }
}
