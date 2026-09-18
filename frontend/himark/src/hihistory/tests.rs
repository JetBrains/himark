// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::higent::ahp_types::state::{ChangesetFile, ChangesetState, ChangesetStatus, FileEdit};
use crate::Authority;

struct InertSeat;

macro_rules! unreached {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $out:ty;)*) => {
        $(fn $name(&self, $($arg: $ty),*) -> $out {
            $(let _ = $arg;)*
            unreachable!("the mirror tests never reach the seat")
        })*
    };
}

impl crate::higent::AhpServer for InertSeat {
    unreached! {
        connect() -> crate::higent::SeatFuture<Result<crate::higent::RootInfo, String>>;
        list_sessions(cursor: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::SessionsPage, String>>;
        poll_root() -> crate::higent::SeatFuture<Vec<crate::higent::ServerEvent>>;
        create_session(dirs: Vec<String>, options: crate::higent::SessionOptions) -> crate::higent::SeatFuture<Result<String, String>>;
        resolve_session_config(working_directory: Option<String>, config: Option<serde_json::Map<String, serde_json::Value>>) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::commands::ResolveSessionConfigResult, String>>;
        dispose_session(session: String) -> crate::higent::SeatFuture<Result<(), String>>;
        subscribe_session(session: String) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::SessionState, String>>;
        poll_session(session: String) -> crate::higent::SeatFuture<Vec<StateAction>>;
        create_chat(session: String) -> crate::higent::SeatFuture<Result<String, String>>;
        subscribe_chat(chat: String) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::ChatState, String>>;
        fetch_turns(chat: String, cursor: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::TurnsPage, String>>;
        start_turn(chat: String, text: String, attachments: Option<Vec<crate::higent::ahp_types::state::MessageAttachment>>, model: Option<crate::higent::ahp_types::state::ModelSelection>) -> crate::higent::SeatFuture<Result<(), String>>;
        poll_chat(chat: String) -> crate::higent::SeatFuture<Vec<StateAction>>;
        cancel_turn(chat: String, turn: String) -> crate::higent::SeatFuture<()>;
        dispatch_action(chat: String, action: StateAction) -> crate::higent::SeatFuture<Result<(), String>>;
        read_file_edit(before: Option<String>, after: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::FileEditContents, String>>;
        resource_read(session: String, uri: crate::higent::ResourceUri) -> crate::higent::SeatFuture<Option<String>>;
        resource_write(session: String, uri: crate::higent::ResourceUri, text: String) -> crate::higent::SeatFuture<bool>;
        resource_list(session: String, uri: crate::higent::ResourceUri) -> crate::higent::SeatFuture<Option<Vec<(String, bool)>>>;
        resource_watch(session: String, uri: crate::higent::ResourceUri, events: Arc<dyn Fn() + Send + Sync>) -> crate::higent::SeatFuture<Option<crate::higent::WatchHandle>>;
        resource_unwatch(handle: crate::higent::WatchHandle) -> crate::higent::SeatFuture<()>;
        search(session: String, ask: crate::higent::SearchAsk) -> crate::higent::SeatFuture<Option<crate::higent::SearchResult>>;
        terminal_input(channel: &String, data: String) -> ();
        terminal_resize(channel: &String, cols: u16, rows: u16) -> ();
        terminal_dispose(channel: &String) -> ();
        subscribe_changeset(channel: String) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::ChangesetState, String>>;
        poll_changeset(channel: String) -> crate::higent::SeatFuture<Vec<StateAction>>;
        unsubscribe_changeset(channel: &String) -> ();
        subscribe_annotations(session: String) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::AnnotationsState, String>>;
        poll_annotations(session: String) -> crate::higent::SeatFuture<Vec<StateAction>>;
        dispatch_annotations(session: &String, action: StateAction) -> ();
        unsubscribe_annotations(session: &String) -> ();
        open_document(session: String, uri: Option<crate::higent::ResourceUri>, text: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::seat::OpenDocumentResult, String>>;
        subscribe_document(channel: String) -> crate::higent::SeatFuture<Result<crate::higent::seat::DocumentState, String>>;
        poll_document(channel: String) -> crate::higent::SeatFuture<Vec<crate::higent::seat::DocumentApplied>>;
        dispatch_document(channel: &String, action: crate::higent::seat::DocumentApplied) -> ();
        unsubscribe_document(channel: &String) -> crate::higent::SeatFuture<()>;
        lsp(session: String, method: String, params: serde_json::Value) -> crate::higent::SeatFuture<Result<serde_json::Value, String>>;
    }

    fn terminal_open(
        &self,
        _session: String,
        _channel: String,
        _cwd: Option<String>,
        _cols: u16,
        _rows: u16,
        _events: Arc<dyn Fn(crate::higent::TerminalEvent) + Send + Sync>,
    ) -> crate::higent::SeatFuture<Option<crate::higent::TerminalHandle>> {
        unreachable!("the mirror tests never reach the seat")
    }
}

struct FileUris;

impl crate::higent::ResourceUriMap for FileUris {
    fn uri_of(&self, location: &ResourceLocation) -> crate::higent::ResourceUri {
        crate::higent::ResourceUri::new(format!("file:///{}", location.path().join("/")))
    }

    fn location_of(
        &self,
        uri: &crate::higent::ResourceUri,
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

fn history_mirror() -> (History, ResourceLocation) {
    let mut history = History::default();
    let folder = folder();
    history.folders.insert_mut(
        folder.clone(),
        FolderHistory {
            seat: Arc::new(InertSeat),
            session: "hihost-fs:/local".to_owned(),
            channel: Some("hihost-history://tmp/repo".to_owned()),
            status: ChangesStatus::Computing,
            head: history_wire::HistoryHead::default(),
            commits: rpds::VectorSync::new_sync(),
            more: None,
            commit_files: rpds::HashTrieMapSync::new_sync(),
        },
    );
    (history, folder)
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
    let (mut history, folder) = history_mirror();
    history.adopt(
        &folder,
        history_window(
            vec![wire_commit("b", "second", &[("main", "branch")], true)],
            Some("1"),
        ),
    );
    let entry = history.folders.get(&folder).unwrap();
    assert_eq!(entry.status, ChangesStatus::Ready);
    assert_eq!(entry.head.branch.as_deref(), Some("main"));
    assert_eq!(entry.commits.len(), 1);
    assert_eq!(entry.more.as_deref(), Some("1"));

    history.fold(
        &folder,
        &[StateAction::Unknown(history_wire::action_value(
            history_wire::HISTORY_APPENDED,
            &history_wire::HistoryAppended {
                commits: vec![wire_commit("a", "first", &[], false)],
                more: None,
            },
        ))],
    );
    let entry = history.folders.get(&folder).unwrap();
    assert_eq!(entry.commits.len(), 2);
    assert_eq!(entry.commits.iter().last().unwrap().summary, "first");
    assert_eq!(entry.more, None);

    history.fold(
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
    let entry = history.folders.get(&folder).unwrap();
    assert_eq!(entry.commits.len(), 3);
    assert_eq!(entry.commits.first().unwrap().summary, "third");
    assert_eq!(entry.head.ahead, Some(2));

    history.fold(
        &folder,
        &[StateAction::Unknown(history_wire::action_value(
            history_wire::HISTORY_RESET,
            &history_wire::HistoryReset {
                state: history_window(vec![wire_commit("d", "rewritten", &[], false)], None),
            },
        ))],
    );
    let entry = history.folders.get(&folder).unwrap();
    assert_eq!(entry.commits.len(), 1);
    assert_eq!(entry.commits.first().unwrap().summary, "rewritten");
}

#[test]
fn the_graph_lists_commits_refs_outgoing_and_paging() {
    let (mut history, folder) = history_mirror();
    history.adopt(
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
        &folder,
        history.folders.get(&folder),
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
        Some(RowItem::Commit { id, .. }) if id == "b"
    ));
    let graph_key = folder.child(ResourceType::new("history-graph"), "graph");
    let more_key = graph_key.child(ResourceType::new("history-more"), "more");
    assert!(matches!(items.get(&more_key), Some(RowItem::More { .. })));
}

#[test]
fn fetched_commit_files_expand_with_pinned_sides() {
    let (mut history, folder) = history_mirror();
    history.adopt(
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
    history.adopt_commit_files(&FileUris, &folder, "b", &Ok(ready(vec![file])));
    let mut items = rpds::HashTrieMapSync::new_sync();
    let node = graph_node(
        &folder,
        history.folders.get(&folder),
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
    let Some(RowItem::File {
        folder: row_folder,
        commit,
        new,
    }) = items.get(&file_key)
    else {
        panic!("a file row under the commit");
    };
    assert_eq!(row_folder, &folder, "the row names its canvas source");
    assert_eq!(commit, "b");
    let (_, new_raw) = crate::hichanges::raw_ref(new).expect("an after ref");
    assert_eq!(new_raw, "hihost-git:/a-commit-ref");

    // The pinned old side rides the canvas feed now.
    let mut store = imba::store::Store::new();
    store.put(history);
    let (_, listing) = crate::diff_canvas::canvas_files(
        &store,
        &crate::diff_canvas::CanvasSource::Commit {
            folder: folder.clone(),
            id: "b".to_owned(),
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
    let tip = CommitTip::of(&commit);
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
