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

/// The session's standing search feed — the SUBSTANCE in the
/// peeker's sense: accumulated results survive the surface. The
/// live channel and its poll loop ride the surface and die with it
/// (closing the surface IS the cancel); reopening shows what stood.
#[derive(Clone, Default)]
pub struct LocationsFeedRow {
    pub title: String,
    /// Seeds the query input on reopen. Empty for LSP result sets.
    pub query: String,
    /// The stream epoch — landings carry it and stale ones discard.
    pub generation: u64,
    pub locations: rpds::VectorSync<FoundLocation>,
    pub done: bool,
    pub truncated: bool,
}

#[derive(Clone, Default)]
pub struct LocationsFeeds(rpds::HashTrieMapSync<crate::SessionId, LocationsFeedRow>);

impl LocationsFeeds {
    pub fn row(store: &Store, session: &crate::SessionId) -> Option<LocationsFeedRow> {
        store
            .get::<LocationsFeeds>()
            .and_then(|feeds| feeds.0.get(session).cloned())
    }

    pub fn put(store: &mut Store, session: crate::SessionId, row: LocationsFeedRow) {
        store.update::<LocationsFeeds>(|feeds| {
            feeds.0.insert_mut(session, row);
        });
    }

    pub fn remove(store: &mut Store, session: &crate::SessionId) {
        store.update::<LocationsFeeds>(|feeds| {
            feeds.0.remove_mut(session);
        });
    }
}

/// Fold a location list into the dirs → files → occurrences forest.
/// Sorted by (authority, path, line, column) whatever order batches
/// landed in; single-child directory chains join into one row (the
/// TOC recipe); every occurrence is its own pickable leaf.
pub fn locations_forest(
    store: &Store,
    rows: &[FoundLocation],
) -> Vec<ForestNode<LocationKey>> {
    let tree = crate::env::Themes::of(store).ui().tree.clone();
    let position_color = tree.directory.0;
    let count_color = tree.directory.0;

    let mut sorted: Vec<&FoundLocation> = rows.iter().collect();
    sorted.sort_by(|a, b| {
        (a.location.authority().as_str(), a.location.path(), a.line, a.column).cmp(&(
            b.location.authority().as_str(),
            b.location.path(),
            b.line,
            b.column,
        ))
    });
    sorted.dedup_by(|a, b| {
        a.location == b.location && a.line == b.line && a.column == b.column
    });

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

        fn children(
            &self,
            trie: Trie<'_>,
            at: &ResourceLocation,
        ) -> Vec<ForestNode<LocationKey>> {
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
