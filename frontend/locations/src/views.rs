// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The collection's document-facing half: the dirs → files →
//! occurrences forests the search surfaces render, the find-results
//! washes picked results install on opened documents, and the feed
//! disposal that takes the washes back out. The stream pump stays
//! the shell's (`drivers::locations`).

use std::collections::BTreeMap;

use imba::store::Store;

use crate::{FeedId, FoundLocation, LocationKey, LocationLists, LocationsFeedRow};
use editor::{ResourceLocation, ResourceType};
use hikit::ForestNode;

/// The registration hook: a search-picked document opened — wash it.
/// WIRED: minted by the family ceremony with the lists collection in
/// hand, installed SCOPED to the family's documents (docs/entities.md
/// law 4) — fires only for its own collection, dies with it.
pub struct LocationsWashHook {
    pub lists: imba::store::Id<LocationLists>,
}

impl documents::DocumentHook for LocationsWashHook {
    fn opened(
        &self,
        store: &mut Store,
        _documents: imba::store::Id<documents::OpenDocuments>,
        document: documents::DocumentId,
        location: Option<&editor::ResourceLocation>,
    ) {
        let Some(location) = location else {
            return;
        };
        if let Some(feed) = LocationLists::take_wash(store, self.lists, location) {
            let lists = self.lists;
            imba::command::Requests::push(
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
        _documents: imba::store::Id<documents::OpenDocuments>,
        _document: documents::DocumentId,
        _location: Option<&editor::ResourceLocation>,
        _doc: &editor::Document,
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
    pub document: documents::DocumentId,
}

impl imba::command::DynamicCommand for WashDocument {
    fn id(&self) -> &'static str {
        "locations.wash-document"
    }

    fn name(&self) -> String {
        "Highlight Found Results".to_owned()
    }

    fn perform(&self, store: &mut Store, ui: &imba::UiCtx, fx: &mut imba::command::Fx<'_>) {
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
        let Some(location) = documents::OpenDocuments::location(store, documents, self.document)
        else {
            return;
        };
        let Some(mut document) =
            documents::OpenDocuments::document(store, documents, self.document)
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
                    let start = documents::offset_at(&mut view, target.start) as u32;
                    start..(start + found.length.max(1))
                })
                .collect()
        };
        if ranges.is_empty() {
            documents::OpenDocuments::put_document(store, documents, self.document, document);
            return;
        }
        ranges.sort_by_key(|range| range.start);
        ranges.dedup();

        let fonts = editor::env::Fonts::of(store)();
        let theme = editor::env::Themes::of(store);
        let markup = editor::MarkupId::mint();
        document.ensure_document_markup(markup);
        let mut tints = editor::Markup::new();
        for range in &ranges {
            tints.push_styled(range.clone(), editor::theme::StyleId::Match);
        }
        let entity = self.document;
        fx.scope(
            move |command| {
                imba::command::Verb::at(
                    documents,
                    documents::DocumentsCommand::Editor(entity, command),
                )
            },
            |fx| document.replace_markup(markup, tints, &ranges, store, ui, &fonts, &theme, fx),
        );
        documents::OpenDocuments::put_document(store, documents, self.document, document);

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
    documents: imba::store::Id<documents::OpenDocuments>,
    ui: &imba::UiCtx,
    row: &LocationsFeedRow,
    fx: &mut imba::command::Fx<'_>,
) {
    let fonts = editor::env::Fonts::of(store)();
    let theme = editor::env::Themes::of(store);
    for (id, (markup, pushed)) in row.washes.iter() {
        let Some(mut document) = documents::OpenDocuments::document(store, documents, *id) else {
            continue; // closed — the markup died with it
        };
        let changed: Vec<std::ops::Range<u32>> =
            pushed.iter().map(|(start, end)| *start..*end).collect();
        let entity = *id;
        fx.scope(
            move |command| {
                imba::command::Verb::at(
                    documents,
                    documents::DocumentsCommand::Editor(entity, command),
                )
            },
            |fx| document.remove_markup(*markup, &changed, store, ui, &fonts, &theme, fx),
        );
        documents::OpenDocuments::put_document(store, documents, *id, document);
    }
}

/// The model half of a feed's disposal: the washes leave the
/// documents, the pending notes sweep, the row drops. The wire
/// teardown — the poll cancel, the unsubscribe — is the driver's
/// (`drivers::locations::DisposeFeed`), which calls here after.
pub fn dispose_feed(
    store: &mut Store,
    ui: &imba::UiCtx,
    lists: imba::store::Id<LocationLists>,
    feed: FeedId,
    fx: &mut imba::command::Fx<'_>,
) {
    let Some(row) = LocationLists::row(store, lists, feed) else {
        return;
    };
    // The collection's wired sibling, not the window's current
    // family — the window may have moved on since the feed opened.
    if let Some(documents) = LocationLists::documents_of(store, lists) {
        remove_washes(store, documents, ui, &row, fx);
    }
    LocationLists::sweep_washes(store, lists, feed);
    LocationLists::remove(store, lists, feed);
}

/// The peek's master forest: locations grouped by FILE (no directory
/// nesting — the card is compact), every occurrence a pickable leaf.
pub fn files_forest<'a>(
    store: &Store,
    rows: impl IntoIterator<Item = &'a FoundLocation>,
) -> Vec<ForestNode<LocationKey>> {
    let tree = editor::env::Themes::of(store).ui().tree.clone();
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
                tint: hikit::TreeTint::File,
                action: None,
                children: hits
                    .into_iter()
                    .map(|found| ForestNode {
                        key: LocationKey::Hit(found.location.clone(), found.line, found.column),
                        label: found.context.trim().to_owned(),
                        pick: true,
                        dim: false,
                        trail: vec![(format!("{}", found.line + 1), chip)],
                        tint: hikit::TreeTint::Label,
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
    let tree = editor::env::Themes::of(store).ui().tree.clone();
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
                tint: hikit::TreeTint::Directory,
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
                    tint: hikit::TreeTint::Label,
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
                tint: hikit::TreeTint::File,
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
                editor::ResourceType::document(),
                editor::Authority::new("local"),
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
