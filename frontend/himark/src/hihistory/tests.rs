// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::changes_view::RowItem;
use ahp_changes::history::{digest_deltas, digest_snapshot};
use crate::hichanges::ChangesStatus;
use ahp_types::actions::StateAction;
use ahp_types::state::{ChangesetFile, ChangesetState, ChangesetStatus, FileEdit};
use editor::location::Authority;
use crate::{ForestNode};
use editor::location::ResourceLocation;
use editor::location::ResourceType;
use himark_ahp_ext_types::history as history_wire;


struct FileUris;

impl ahp_wire::client::ResourceUriMap for FileUris {
    fn uri_of(&self, location: &ResourceLocation) -> ahp_wire::client::ResourceUri {
        ahp_wire::client::ResourceUri::new(format!("file:///{}", location.path().join("/")))
    }

    fn location_of(
        &self,
        uri: &ahp_wire::client::ResourceUri,
        kind: ResourceType,
        authority: &Authority,
    ) -> Option<ResourceLocation> {
        let path = uri.as_str().strip_prefix("file://")?;
        let segments: Vec<String> = path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();
        Some(ResourceLocation::new(kind, authority.clone(), segments))
    }
}

fn folder() -> ResourceLocation {
    ResourceLocation::new(
        ResourceType::directory(),
        Authority::new("local"),
        vec!["tmp".to_owned(), "repo".to_owned()],
    )
}

fn ready(files: Vec<ChangesetFile>) -> ChangesetState {
    ChangesetState {
        status: ChangesetStatus::Ready,
        error: None,
        files,
        operations: None,
    }
}

fn wire_commit(
    id: &str,
    summary: &str,
    refs: &[(&str, &str)],
    outgoing: bool,
) -> history_wire::Commit {
    history_wire::Commit {
        id: id.to_owned(),
        parents: Vec::new(),
        summary: summary.to_owned(),
        message: None,
        author: history_wire::CommitAuthor {
            name: "Test".to_owned(),
            email: None,
            timestamp: 1,
        },
        refs: refs
            .iter()
            .map(|(name, kind)| history_wire::CommitRef {
                name: (*name).to_owned(),
                kind: (*kind).to_owned(),
            })
            .collect(),
        outgoing,
        changeset: format!("hihost-changes://tmp/repo?commit={id}"),
    }
}

fn history_window(
    commits: Vec<history_wire::Commit>,
    more: Option<&str>,
) -> history_wire::HistoryState {
    history_wire::HistoryState {
        status: history_wire::HistoryStatus::Ready,
        error: None,
        head: history_wire::HistoryHead {
            branch: Some("main".to_owned()),
            upstream: Some("origin/main".to_owned()),
            ahead: Some(1),
            behind: None,
        },
        commits,
        more: more.map(str::to_owned),
    }
}

/// The two sibling collections the mirror stands up — wired to each
/// other the way the ceremony wires them.
fn history_id() -> imba::store::Id<History> {
    static ID: std::sync::OnceLock<imba::store::Id<History>> = std::sync::OnceLock::new();
    *ID.get_or_init(imba::store::Id::mint)
}

fn changes_id() -> imba::store::Id<crate::hichanges::Changes> {
    static ID: std::sync::OnceLock<imba::store::Id<crate::hichanges::Changes>> =
        std::sync::OnceLock::new();
    *ID.get_or_init(imba::store::Id::mint)
}

/// The wire → mirror road the driver's landings travel: digest on
/// the worker side, fold the mirrors through the model's doors.
fn land(
    store: &mut imba::store::Store,
    folder: &ResourceLocation,
    state: history_wire::HistoryState,
) {
    let (snapshot, _harvest) = digest_snapshot(state);
    History::land_snapshot(store, history_id(), folder, snapshot);
}

fn fold(store: &mut imba::store::Store, folder: &ResourceLocation, actions: &[StateAction]) {
    let (deltas, _harvest) = digest_deltas(actions);
    History::fold_deltas(store, history_id(), folder, deltas);
}

fn history_mirror() -> (imba::store::Store, ResourceLocation) {
    let mut store = imba::store::Store::new();
    store.put_entity(history_id(), History::wired(changes_id()));
    let changes = crate::hichanges::Changes::wired(imba::store::Id::mint(), history_id());
    store.put_entity(changes_id(), changes);
    let folder = folder();
    History::ensure_folder(&mut store, history_id(), &folder);
    (store, folder)
}

fn folder_entry(store: &imba::store::Store, folder: &ResourceLocation) -> FolderHistory {
    History::folder(store, history_id(), folder).expect("the mirror entry")
}

fn rows_of(node: &ForestNode<ResourceLocation>) -> Vec<(u8, String, bool)> {
    fn walk(node: &ForestNode<ResourceLocation>, depth: u8, out: &mut Vec<(u8, String, bool)>) {
        out.push((depth, node.label.clone(), node.pick));
        for child in &node.children {
            walk(child, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(node, 0, &mut out);
    out
}

#[test]
fn the_history_folds_reset_appended_and_prepended() {
    let (mut store, folder) = history_mirror();
    land(
        &mut store,
        &folder,
        history_window(
            vec![wire_commit("b", "second", &[("main", "branch")], true)],
            Some("1"),
        ),
    );
    let entry = folder_entry(&store, &folder);
    assert_eq!(entry.status, ChangesStatus::Ready);
    assert_eq!(entry.head.branch.as_deref(), Some("main"));
    assert_eq!(entry.commits.len(), 1);
    assert_eq!(entry.more.as_deref(), Some("1"));

    fold(
        &mut store,
        &folder,
        &[StateAction::Unknown(history_wire::action_value(
            history_wire::HISTORY_APPENDED,
            &history_wire::HistoryAppended {
                commits: vec![wire_commit("a", "first", &[], false)],
                more: None,
            },
        ))],
    );
    let entry = folder_entry(&store, &folder);
    assert_eq!(entry.commits.len(), 2);
    assert_eq!(entry.commits.iter().last().unwrap().summary, "first");
    assert_eq!(entry.more, None);

    fold(
        &mut store,
        &folder,
        &[StateAction::Unknown(history_wire::action_value(
            history_wire::HISTORY_PREPENDED,
            &history_wire::HistoryPrepended {
                commits: vec![wire_commit("c", "third", &[("main", "branch")], true)],
                head: history_wire::HistoryHead {
                    branch: Some("main".to_owned()),
                    upstream: None,
                    ahead: Some(2),
                    behind: None,
                },
            },
        ))],
    );
    let entry = folder_entry(&store, &folder);
    assert_eq!(entry.commits.len(), 3);
    assert_eq!(entry.commits.first().unwrap().summary, "third");
    assert_eq!(entry.head.ahead, Some(2));

    fold(
        &mut store,
        &folder,
        &[StateAction::Unknown(history_wire::action_value(
            history_wire::HISTORY_RESET,
            &history_wire::HistoryReset {
                state: history_window(vec![wire_commit("d", "rewritten", &[], false)], None),
            },
        ))],
    );
    let entry = folder_entry(&store, &folder);
    assert_eq!(entry.commits.len(), 1);
    assert_eq!(entry.commits.first().unwrap().summary, "rewritten");
}

#[test]
fn the_graph_lists_commits_refs_outgoing_and_paging() {
    let (mut store, folder) = history_mirror();
    land(
        &mut store,
        &folder,
        history_window(
            vec![
                wire_commit("b", "fix: the second", &[("main", "branch")], true),
                wire_commit("a", "the first", &[], false),
            ],
            Some("2"),
        ),
    );
    let mut items = rpds::HashTrieMapSync::new_sync();
    let node = graph_node(
        &store,
        history_id(),
        &folder,
        History::folder(&store, history_id(), &folder).as_ref(),
        &mut items,
        (skia_safe::Color::GREEN, skia_safe::Color::RED),
    );

    assert_eq!(node.label, "repo — main");
    let labels: Vec<(u8, String, bool)> = rows_of(&node);

    assert_eq!(labels[0], (0, "repo — main".to_owned(), false));
    assert_eq!(labels[1], (1, "fix: the second".to_owned(), true));
    assert_eq!(labels[2], (2, "…".to_owned(), false));
    assert_eq!(labels[3], (1, "the first".to_owned(), true));

    assert_eq!(
        labels[5],
        (1, "· · ·  loading older commits  · · ·".to_owned(), false)
    );

    let commit_key = folder.child(ResourceType::new("history-commit"), "b");
    assert!(matches!(
        items.get(&commit_key),
        Some(RowItem::Open {
            source: crate::diff_canvas::CanvasSource::Commit { id, .. },
            reveal: None,
            ..
        }) if id.as_str() == "b"
    ));
    let graph_key = folder.child(ResourceType::new("history-graph"), "graph");
    let more_key = graph_key.child(ResourceType::new("history-more"), "more");
    assert!(matches!(items.get(&more_key), Some(RowItem::Grow { .. })));
}

#[test]
fn fetched_commit_files_expand_with_pinned_sides() {
    let (mut store, folder) = history_mirror();
    land(
        &mut store,
        &folder,
        history_window(vec![wire_commit("b", "second", &[], false)], None),
    );

    let abs = "/tmp/repo/src/lib.rs";
    let file = ChangesetFile {
        id: format!("file://{abs}"),
        edit: FileEdit {
            before: Some(
                json!({"uri": format!("file://{abs}"), "content": {"uri": "hihost-git:/a-parent-ref"}}),
            ),
            after: Some(
                json!({"uri": format!("file://{abs}"), "content": {"uri": "hihost-git:/a-commit-ref"}}),
            ),
            diff: Some(json!({"added": 3, "removed": 1})),
        },
        reviewed: None,
        meta: None,
    };
    crate::hichanges::Changes::adopt_commit_state(
        &mut store,
        changes_id(),
        &folder,
        &crate::hichanges::Revision::new("b"),
        &Ok(ahp_changes::changes::digest_state(
            &FileUris,
            &folder,
            &ready(vec![file]),
        )),
    );
    let mut items = rpds::HashTrieMapSync::new_sync();
    let node = graph_node(
        &store,
        history_id(),
        &folder,
        History::folder(&store, history_id(), &folder).as_ref(),
        &mut items,
        (skia_safe::Color::GREEN, skia_safe::Color::RED),
    );
    let labels = rows_of(&node);
    assert_eq!(labels[1], (1, "second".to_owned(), true));

    assert_eq!(labels[2], (2, "src".to_owned(), false));
    assert_eq!(labels[3], (3, "lib.rs".to_owned(), true));

    let commit_key = folder.child(ResourceType::new("history-commit"), "b");
    let file_key = commit_key
        .child(ResourceType::directory(), "src")
        .child(ResourceType::document(), "lib.rs");
    let Some(RowItem::Open {
        source:
            crate::diff_canvas::CanvasSource::Commit {
                folder: row_folder,
                id: commit,
            },
        reveal: Some(new),
        ..
    }) = items.get(&file_key)
    else {
        panic!("a file row under the commit");
    };
    assert_eq!(row_folder, &folder, "the row names its canvas source");
    assert_eq!(commit.as_str(), "b");
    let (_, new_raw) = crate::hichanges::raw_ref(new).expect("an after ref");
    assert_eq!(new_raw, "hihost-git:/a-commit-ref");

    // The pinned old side rides the canvas feed now.
    let (_, listing) = crate::diff_canvas::canvas_files(
        &store,
        changes_id(),
        &crate::diff_canvas::CanvasSource::Commit {
            folder: folder.clone(),
            id: crate::hichanges::Revision::new("b"),
        },
    );
    let crate::diff_canvas::CanvasListing::Ready(files) = listing else {
        panic!("a ready listing");
    };
    let (_, old_raw) = crate::hichanges::raw_ref(&files[0].old).expect("a before ref");
    assert_eq!(old_raw, "hihost-git:/a-parent-ref");
}

#[test]
fn the_commit_tip_carries_message_author_and_branches() {
    let commit = himark_ahp_ext_types::history::Commit {
        id: "abc".to_owned(),
        parents: vec![],
        summary: "fix: the head".to_owned(),
        message: Some("fix: the head\n\nA body line.".to_owned()),
        author: himark_ahp_ext_types::history::CommitAuthor {
            name: "Andrey Zaytsev".to_owned(),
            email: Some("a@z.dev".to_owned()),
            timestamp: 0,
        },
        refs: vec![
            himark_ahp_ext_types::history::CommitRef {
                name: "main".to_owned(),
                kind: "branch".to_owned(),
            },
            himark_ahp_ext_types::history::CommitRef {
                name: "origin/main".to_owned(),
                kind: "remote".to_owned(),
            },
        ],
        outgoing: false,
        changeset: "cs:abc".to_owned(),
    };
    let tip = CommitTip::of(&ahp_changes::history::digest_commit(commit));
    let lines: Vec<&str> = tip.lines().iter().map(|(line, _)| line.as_str()).collect();
    assert_eq!(
        lines,
        vec![
            "fix: the head",
            "",
            "A body line.",
            "",
            "Andrey Zaytsev <a@z.dev>",
            "main  ·  origin/main",
        ]
    );

    assert!(!tip.lines()[0].1);
    assert!(tip.lines()[4].1 && tip.lines()[5].1);
}
