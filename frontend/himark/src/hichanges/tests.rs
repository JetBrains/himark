// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::higent::ahp_types::actions::{
    ChangesetClearedAction, ChangesetContentChangedAction, ChangesetFileRemovedAction,
    ChangesetFileSetAction, ChangesetStatusChangedAction, StateAction,
};
use crate::higent::ahp_types::state::{ChangesetFile, ChangesetState, ChangesetStatus, FileEdit};
use serde_json::json;

use super::*;

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
        create_session(dirs: Vec<String>, options: crate::higent::SessionOptions) -> crate::higent::SeatFuture<Result<crate::higent::SessionUri, String>>;
        resolve_session_config(working_directory: Option<String>, config: Option<serde_json::Map<String, serde_json::Value>>) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::commands::ResolveSessionConfigResult, String>>;
        dispose_session(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<(), String>>;
        subscribe_session(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::SessionState, String>>;
        poll_session(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Vec<StateAction>>;
        create_chat(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<crate::higent::ChatUri, String>>;
        subscribe_chat(chat: crate::higent::ChatUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::ChatState, String>>;
        fetch_turns(chat: crate::higent::ChatUri, cursor: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::TurnsPage, String>>;
        start_turn(chat: crate::higent::ChatUri, text: String, attachments: Option<Vec<crate::higent::ahp_types::state::MessageAttachment>>, model: Option<crate::higent::ahp_types::state::ModelSelection>) -> crate::higent::SeatFuture<Result<(), String>>;
        poll_chat(chat: crate::higent::ChatUri) -> crate::higent::SeatFuture<Vec<StateAction>>;
        cancel_turn(chat: crate::higent::ChatUri, turn: crate::higent::TurnId) -> crate::higent::SeatFuture<()>;
        dispatch_action(chat: crate::higent::ChannelUri, action: StateAction) -> crate::higent::SeatFuture<Result<(), String>>;
        read_file_edit(before: Option<String>, after: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::FileEditContents, String>>;
        resource_read(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri) -> crate::higent::SeatFuture<Option<String>>;
        resource_write(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri, text: String) -> crate::higent::SeatFuture<bool>;
        resource_list(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri) -> crate::higent::SeatFuture<Option<Vec<(String, bool)>>>;
        resource_watch(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri, events: Arc<dyn Fn() + Send + Sync>) -> crate::higent::SeatFuture<Option<crate::higent::WatchHandle>>;
        resource_unwatch(handle: crate::higent::WatchHandle) -> crate::higent::SeatFuture<()>;
        search(session: crate::higent::SessionUri, ask: crate::higent::SearchAsk) -> crate::higent::SeatFuture<Option<crate::higent::SearchResult>>;
        terminal_input(channel: &crate::higent::ChannelUri, data: String) -> ();
        terminal_resize(channel: &crate::higent::ChannelUri, cols: u16, rows: u16) -> ();
        terminal_dispose(channel: &crate::higent::ChannelUri) -> ();
        subscribe_changeset(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::ChangesetState, String>>;
        poll_changeset(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Vec<StateAction>>;
        unsubscribe_changeset(channel: &crate::higent::ChannelUri) -> ();
        subscribe_annotations(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::AnnotationsState, String>>;
        poll_annotations(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Vec<StateAction>>;
        dispatch_annotations(session: &crate::higent::SessionUri, action: StateAction) -> ();
        unsubscribe_annotations(session: &crate::higent::SessionUri) -> ();
        open_document(session: crate::higent::SessionUri, uri: Option<crate::higent::ResourceUri>, text: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::seat::OpenDocumentResult, String>>;
        subscribe_document(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Result<crate::higent::seat::DocumentState, String>>;
        poll_document(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Vec<crate::higent::seat::DocumentApplied>>;
        dispatch_document(channel: &crate::higent::ChannelUri, action: crate::higent::seat::DocumentApplied) -> ();
        unsubscribe_document(channel: &crate::higent::ChannelUri) -> crate::higent::SeatFuture<()>;
        lsp(session: crate::higent::SessionUri, method: String, params: serde_json::Value) -> crate::higent::SeatFuture<Result<serde_json::Value, String>>;
    }

    fn terminal_open(
        &self,
        _session: crate::higent::SessionUri,
        _channel: crate::higent::ChannelUri,
        _cwd: Option<String>,
        _cols: u16,
        _rows: u16,
        _events: Arc<dyn Fn(crate::higent::TerminalEvent) + Send + Sync>,
    ) -> crate::higent::SeatFuture<Option<crate::higent::TerminalHandle>> {
        unreachable!("the mirror tests never reach the seat")
    }
}

/// The collection the test's sets live in — wired to sibling ids
/// nothing here resolves, the way the ceremony would wire them.
fn changes_id() -> imba::store::Id<Changes> {
    static ID: std::sync::OnceLock<imba::store::Id<Changes>> = std::sync::OnceLock::new();
    *ID.get_or_init(imba::store::Id::mint)
}

fn wired() -> Changes {
    Changes::wired(imba::store::Id::mint(), imba::store::Id::mint(), None)
}

fn folder() -> ResourceLocation {
    ResourceLocation::new(
        ResourceType::directory(),
        Authority::new("local"),
        vec!["tmp".to_owned(), "repo".to_owned()],
    )
}

fn wire_file(
    rel: &str,
    before_ref: Option<&str>,
    deleted: bool,
    counts: (i64, i64),
) -> ChangesetFile {
    let abs = format!("/tmp/repo/{rel}");
    ChangesetFile {
        id: format!("file://{abs}"),
        edit: FileEdit {
            before: before_ref.map(
                |reference| json!({"uri": format!("file://{abs}"), "content": {"uri": reference}}),
            ),
            after: (!deleted).then(|| json!({"uri": format!("file://{abs}")})),
            diff: Some(json!({"added": counts.0, "removed": counts.1})),
        },
        reviewed: None,
        meta: None,
    }
}

fn ready(files: Vec<ChangesetFile>) -> ChangesetState {
    ChangesetState {
        status: ChangesetStatus::Ready,
        error: None,
        files,
        operations: None,
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
        authority: &crate::Authority,
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

/// The wire roads, the way the landings travel them now: digest on
/// one side (the effect worker's job), adopt/fold the finished
/// entries on the other.
fn adopt_wire(changes: &mut Changes, folder: &ResourceLocation, state: &ChangesetState) {
    changes.adopt(folder, digest_state(&FileUris, folder, state));
}

fn fold_wire(changes: &mut Changes, folder: &ResourceLocation, actions: &[StateAction]) {
    changes.fold(folder, digest_actions(&FileUris, folder, actions));
}

fn mirror() -> (Changes, ResourceLocation) {
    let mut changes = wired();
    changes.uris = Some(Arc::new(FileUris));
    let id = ChangeSetId::mint();
    changes.sets.insert_mut(
        id,
        ChangeSet {
            source: ChangeSetSource::WorkingCopy { folder: folder() },
            feed: Some(SetFeed {
                seat: Arc::new(InertSeat),
                session: crate::higent::SessionUri::new("hihost-fs:/local"),
                channel: Some(crate::higent::ChannelUri::new("hihost-changes://tmp/repo")),
            }),
            status: ChangesStatus::Computing,
            files: rpds::VectorSync::new_sync(),
            generation: 0,
            bases: rpds::HashTrieMapSync::new_sync(),
            canvases: rpds::HashTrieMapSync::new_sync(),
        },
    );
    changes
        .by_source
        .insert_mut(ChangeSetSource::WorkingCopy { folder: folder() }, id);
    (changes, folder())
}

#[test]
fn the_ref_codec_round_trips() {
    let path = vec!["tmp".to_owned(), "repo".to_owned(), "a.md".to_owned()];
    let before = before_ref_location("local", "hihost-git:/sha\u{1f}/tmp/repo\u{1f}a.md", path);
    assert!(scoped(&before));
    assert!(!scoped(&folder()));
    assert_eq!(before.name(), "a.md", "the name-path shows the file");
    let (origin, raw) = raw_ref(&before).expect("decodes");
    assert_eq!(origin, "local");
    assert_eq!(raw, "hihost-git:/sha\u{1f}/tmp/repo\u{1f}a.md");
    let working = working_copy(&before).expect("the working copy");
    assert_eq!(working.authority().as_str(), "local");
    assert_eq!(working.path(), before.path());
    assert!(working.kind().is_document());
}

#[test]
fn the_catalog_names_the_folders_channel() {
    let (mut changes, folder) = mirror();
    let mut pending = changes.folder_set(&folder).unwrap().clone();
    pending.feed.as_mut().unwrap().channel = None;
    let id = *changes
        .by_source
        .get(&ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        })
        .unwrap();
    changes.sets.insert_mut(id, pending);
    changes.session = Some(SessionFeed {
        uri: crate::higent::SessionUri::new("hihost-fs:/local"),
        seat: Arc::new(InertSeat),
        catalog: rpds::VectorSync::new_sync(),
    });
    let fresh = changes.adopt_catalog(
        &crate::higent::SessionUri::new("hihost-fs:/local"),
        vec![
            CatalogEntry {
                uri: crate::higent::ChannelUri::new("hihost-changes://somewhere/else"),
                description: Some("/somewhere/else".to_owned()),
                kind: "uncommitted".to_owned(),
            },
            CatalogEntry {
                uri: crate::higent::ChannelUri::new("hihost-changes://tmp/repo"),
                description: Some("/tmp/repo".to_owned()),
                kind: "uncommitted".to_owned(),
            },
        ],
    );
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].0, folder);
    assert_eq!(fresh[0].2.as_str(), "hihost-changes://tmp/repo");
    assert_eq!(
        changes
            .folder_set(&folder)
            .unwrap()
            .feed
            .as_ref()
            .unwrap()
            .channel
            .as_ref()
            .map(|c| c.as_str()),
        Some("hihost-changes://tmp/repo")
    );

    let again = changes.adopt_catalog(
        &crate::higent::SessionUri::new("hihost-fs:/local"),
        vec![CatalogEntry {
            uri: crate::higent::ChannelUri::new("hihost-changes://tmp/repo"),
            description: Some("/tmp/repo".to_owned()),
            kind: "uncommitted".to_owned(),
        }],
    );
    assert!(again.is_empty());
}

#[test]
fn a_lone_folder_takes_a_lone_foreign_entry() {
    let (mut changes, folder) = mirror();
    let mut pending = changes.folder_set(&folder).unwrap().clone();
    pending.feed.as_mut().unwrap().channel = None;
    let id = *changes
        .by_source
        .get(&ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        })
        .unwrap();
    changes.sets.insert_mut(id, pending);
    changes.session = Some(SessionFeed {
        uri: crate::higent::SessionUri::new("hihost-fs:/local"),
        seat: Arc::new(InertSeat),
        catalog: rpds::VectorSync::new_sync(),
    });
    let fresh = changes.adopt_catalog(
        &crate::higent::SessionUri::new("hihost-fs:/local"),
        vec![CatalogEntry {
            uri: crate::higent::ChannelUri::new("vscode-changes:/session"),
            description: None,
            kind: "uncommitted".to_owned(),
        }],
    );
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].2.as_str(), "vscode-changes:/session");
}

#[test]
fn a_snapshot_adopts_into_digested_entries_and_refs() {
    let (mut changes, folder) = mirror();
    adopt_wire(
        &mut changes,
        &folder,
        &ready(vec![
            wire_file("src/notes.md", Some("hihost-git:/one"), false, (2, 1)),
            wire_file("added.md", None, false, (3, 0)),
            wire_file("gone.md", Some("hihost-git:/two"), true, (0, 5)),
        ]),
    );
    let entry = changes.folder_set(&folder).expect("the mirror entry");
    assert_eq!(entry.status, ChangesStatus::Ready);
    assert_eq!(entry.files.len(), 3);
    let modified = entry
        .files
        .iter()
        .find(|change| change.working.name() == "notes.md")
        .expect("the modified file");
    let before = modified.before.as_ref().expect("a tracked before side");
    assert_eq!(
        raw_ref(before).expect("decodes").1,
        "hihost-git:/one",
        "the before ref carries the host's content uri"
    );
    assert_eq!((modified.added, modified.removed), (Some(2), Some(1)));
    let added = entry
        .files
        .iter()
        .find(|change| change.working.name() == "added.md")
        .expect("the added file");
    assert!(added.before.is_none(), "an add has no old text");

    assert!(changes.base_lookup("/tmp/repo/src/notes.md").is_some());
    assert!(changes.base_lookup("/tmp/repo/gone.md").is_some());
    assert!(
        changes.base_lookup("/tmp/repo/added.md").is_none(),
        "no before, no ref"
    );
    assert_eq!(
        changes.folder_set(&folder).expect("the set").generation(),
        1,
        "the landing bumped the set's generation"
    );
}

#[test]
fn the_fold_mirrors_the_official_reducers() {
    let (mut changes, folder) = mirror();
    adopt_wire(
        &mut changes,
        &folder,
        &ready(vec![wire_file("a.md", Some("r1"), false, (1, 1))]),
    );

    fold_wire(
        &mut changes,
        &folder,
        &[StateAction::ChangesetContentChanged(Box::new(
            ChangesetContentChangedAction {
                files: vec![
                    wire_file("b.md", Some("r2"), false, (2, 2)),
                    wire_file("c.md", None, false, (1, 0)),
                ],
                operations: None,
            },
        ))],
    );
    let entry = changes.folder_set(&folder).unwrap();
    assert_eq!(entry.files.len(), 2);
    assert_eq!(entry.status, ChangesStatus::Ready);
    assert!(
        changes.base_lookup("/tmp/repo/a.md").is_none(),
        "the replaced file's ref is gone"
    );
    assert!(changes.base_lookup("/tmp/repo/b.md").is_some());

    fold_wire(
        &mut changes,
        &folder,
        &[StateAction::ChangesetFileSet(ChangesetFileSetAction {
            file: wire_file("b.md", Some("r3"), false, (9, 9)),
        })],
    );
    let entry = changes.folder_set(&folder).unwrap();
    assert_eq!(entry.files.len(), 2, "an upsert replaces, never duplicates");
    let b = entry
        .files
        .iter()
        .find(|change| change.working.name() == "b.md")
        .unwrap();
    assert_eq!(b.added, Some(9));
    fold_wire(
        &mut changes,
        &folder,
        &[StateAction::ChangesetFileRemoved(
            ChangesetFileRemovedAction {
                file_id: "file:///tmp/repo/c.md".to_owned(),
            },
        )],
    );
    assert_eq!(changes.folder_set(&folder).unwrap().files.len(), 1);

    fold_wire(
        &mut changes,
        &folder,
        &[StateAction::ChangesetStatusChanged(
            ChangesetStatusChangedAction {
                status: ChangesetStatus::Computing,
                error: None,
            },
        )],
    );
    assert_eq!(
        changes.folder_set(&folder).unwrap().status,
        ChangesStatus::Computing
    );
    fold_wire(
        &mut changes,
        &folder,
        &[StateAction::ChangesetCleared(ChangesetClearedAction {})],
    );
    let entry = changes.folder_set(&folder).unwrap();
    assert!(entry.files.is_empty());
    assert!(
        changes.base_lookup("/tmp/repo/b.md").is_none(),
        "cleared files clear their refs"
    );
}

#[test]
fn a_failed_subscribe_reports_on_the_row() {
    let (mut changes, folder) = mirror();
    changes.adopt_error(&folder, "not a repository".to_owned());
    let entry = changes.folder_set(&folder).unwrap();
    assert_eq!(
        entry.status,
        ChangesStatus::Error("not a repository".to_owned())
    );
    let mut items = rpds::HashTrieMapSync::new_sync();
    let node = folder_node(
        &folder,
        Some(entry),
        &mut items,
        (skia_safe::Color::GREEN, skia_safe::Color::RED),
    );
    assert_eq!(
        rows_of(&node),
        vec![
            (0, "repo".to_owned(), false),
            (1, "not a repository".to_owned(), false),
        ]
    );
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
fn the_tree_nests_dirs_first_and_compacts_chains() {
    let (mut changes, folder) = mirror();
    adopt_wire(
        &mut changes,
        &folder,
        &ready(vec![
            wire_file("top.md", Some("r0"), false, (1, 0)),
            wire_file("src/deep/inner/one.md", Some("r1"), false, (2, 1)),
            wire_file("src/deep/inner/two.md", None, false, (4, 0)),
            wire_file("docs/guide.md", Some("r2"), false, (0, 3)),
        ]),
    );
    let entry = changes.folder_set(&folder).unwrap();
    let mut items = rpds::HashTrieMapSync::new_sync();
    let node = folder_node(
        &folder,
        Some(entry),
        &mut items,
        (skia_safe::Color::GREEN, skia_safe::Color::RED),
    );
    assert_eq!(
        rows_of(&node),
        vec![
            (0, "repo".to_owned(), false),
            (1, "docs".to_owned(), false),
            (2, "guide.md".to_owned(), true),
            (1, "src/deep/inner".to_owned(), false),
            (2, "one.md".to_owned(), true),
            (2, "two.md".to_owned(), true),
            (1, "top.md".to_owned(), true),
        ]
    );
}

#[test]
fn activation_pairs_carry_the_exact_locations() {
    let (mut changes, folder) = mirror();
    adopt_wire(
        &mut changes,
        &folder,
        &ready(vec![
            wire_file("mod.md", Some("hihost-git:/base"), false, (1, 1)),
            wire_file("new.md", None, false, (2, 0)),
        ]),
    );
    let entry = changes.folder_set(&folder).unwrap();
    let mut items = rpds::HashTrieMapSync::new_sync();
    let _ = folder_node(
        &folder,
        Some(entry),
        &mut items,
        (skia_safe::Color::GREEN, skia_safe::Color::RED),
    );
    let key = |name: &str| {
        ResourceLocation::new(
            ResourceType::document(),
            Authority::new("local"),
            vec!["tmp".to_owned(), "repo".to_owned(), name.to_owned()],
        )
    };
    match items.get(&key("mod.md")) {
        Some(RowItem::Open {
            reveal: Some(new), ..
        }) => {
            assert_eq!(new, &key("mod.md"), "the new side IS the working copy");
        }
        other => panic!("expected a paired file row, got {:?}", other.is_some()),
    }

    // The PAIR itself now rides the canvas feed — the same entries,
    // normalized the way activation consumes them.
    let mut store = imba::store::Store::new();
    store.put_entity(changes_id(), changes);
    let (_, listing) = crate::diff_canvas::canvas_files(
        &store,
        changes_id(),
        &crate::diff_canvas::CanvasSource::WorkingCopy {
            folder: folder.clone(),
        },
    );
    let crate::diff_canvas::CanvasListing::Ready(files) = listing else {
        panic!("a ready listing");
    };
    let by_key = |name: &str| {
        files
            .iter()
            .find(|file| file.new == key(name))
            .expect("a listed file")
    };
    assert_eq!(
        raw_ref(&by_key("mod.md").old).expect("a before ref").1,
        "hihost-git:/base"
    );
    assert_eq!(
        by_key("new.md").old.authority().as_str(),
        super::EMPTY_AUTHORITY,
        "an add's old side is the empty authority"
    );
}

#[test]
fn a_later_snapshot_supersedes_earlier_file_mutations_in_the_batch() {
    let (mut changes, folder) = mirror();
    // Three full snapshots and an interleaved upsert in ONE batch —
    // only the last snapshot (and what follows it) may cost a
    // conversion or a fold.
    let batch = vec![
        StateAction::ChangesetContentChanged(Box::new(ChangesetContentChangedAction {
            files: vec![wire_file("stale-one.md", None, false, (1, 0))],
            operations: None,
        })),
        StateAction::ChangesetFileSet(ChangesetFileSetAction {
            file: wire_file("stale-upsert.md", None, false, (1, 0)),
        }),
        StateAction::ChangesetContentChanged(Box::new(ChangesetContentChangedAction {
            files: vec![wire_file("stale-two.md", None, false, (1, 0))],
            operations: None,
        })),
        StateAction::ChangesetStatusChanged(ChangesetStatusChangedAction {
            status: ChangesetStatus::Ready,
            error: None,
        }),
        StateAction::ChangesetContentChanged(Box::new(ChangesetContentChangedAction {
            files: vec![wire_file("final.md", None, false, (2, 0))],
            operations: None,
        })),
        StateAction::ChangesetFileSet(ChangesetFileSetAction {
            file: wire_file("after.md", None, false, (3, 0)),
        }),
    ];
    let digested = digest_actions(&FileUris, &folder, &batch);
    let contents = digested
        .iter()
        .filter(|action| matches!(action, ChangeAction::Content(_)))
        .count();
    assert_eq!(contents, 1, "superseded snapshots never convert");
    changes.fold(&folder, digested);
    let entry = changes.folder_set(&folder).unwrap();
    assert_eq!(entry.status, ChangesStatus::Ready, "statuses all apply");
    let mut names: Vec<&str> = entry.files.iter().map(|file| file.working.name()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["after.md", "final.md"],
        "the last snapshot plus the mutations after it"
    );
}

#[test]
fn a_superseded_poll_folds_its_batch_but_never_rearms() {
    use imba::store::Entity as _;

    let (mut changes, folder) = mirror();
    let mut store = imba::store::Store::new();
    let ui = imba::UiCtx::dont_use_too_slow();
    let launches = |batch: imba::effect::Batch<ChangesCommand>| {
        batch
            .drain()
            .into_iter()
            .filter(|message| matches!(message, imba::effect::Message::Launch(..)))
            .count()
    };

    // The subscribe landing arms the ONE standing loop.
    let mut batch = imba::effect::Batch::<ChangesCommand>::new();
    changes.perform(
        changes_id(),
        ChangesCommand::Snapshot {
            folder: folder.clone(),
            result: Ok(digest_state(&FileUris, &folder, &ready(vec![]))),
        },
        &mut store,
        &ui,
        &mut batch.effects(),
    );
    assert_eq!(launches(batch), 1, "the snapshot arms the poll");
    let armed = changes.poll_serial(&folder).expect("a serial stands");

    // A re-subscribe supersedes the loop: its landing bumps the serial.
    let mut batch = imba::effect::Batch::<ChangesCommand>::new();
    changes.perform(
        changes_id(),
        ChangesCommand::Snapshot {
            folder: folder.clone(),
            result: Ok(digest_state(&FileUris, &folder, &ready(vec![]))),
        },
        &mut store,
        &ui,
        &mut batch.effects(),
    );
    assert_eq!(launches(batch), 1);
    let current = changes.poll_serial(&folder).expect("a serial stands");
    assert_ne!(armed, current);

    // The OLD loop's landing: the batch folds, the loop dies.
    let mut batch = imba::effect::Batch::<ChangesCommand>::new();
    changes.perform(
        changes_id(),
        ChangesCommand::Polled {
            folder: folder.clone(),
            serial: armed,
            actions: digest_actions(
                &FileUris,
                &folder,
                &[StateAction::ChangesetFileSet(ChangesetFileSetAction {
                    file: wire_file("late.md", None, false, (1, 0)),
                })],
            ),
        },
        &mut store,
        &ui,
        &mut batch.effects(),
    );
    assert_eq!(launches(batch), 0, "a stale landing never rearms");
    assert_eq!(
        changes.folder_set(&folder).unwrap().files.len(),
        1,
        "its drained actions still fold"
    );

    // The CURRENT loop's landing re-arms as ever.
    let mut batch = imba::effect::Batch::<ChangesCommand>::new();
    changes.perform(
        changes_id(),
        ChangesCommand::Polled {
            folder: folder.clone(),
            serial: current,
            actions: Vec::new(),
        },
        &mut store,
        &ui,
        &mut batch.effects(),
    );
    assert_eq!(launches(batch), 1, "the standing loop keeps polling");
}

/// PERF REGRESSION (docs/perf-issue.md §2): a poll batch carrying
/// SEVERAL full snapshots of a skia-sized changeset must digest at
/// the cost of ONE — the superseded ones are dropped before any wire
/// `Value` is parsed. Before the coalesce (and with `entry_of`
/// deep-cloning every payload) a backlogged batch cost O(snapshots ×
/// files × payload): the multi-second drawer freezes.
#[test]
fn digesting_a_backlogged_skia_sized_batch_costs_one_snapshot() {
    let files = 1500usize;
    // The wire payloads carry arbitrary host JSON — give each side
    // real bulk so a hidden deep-clone would surface in the ratio.
    let bulk: Vec<String> = (0..40).map(|n| format!("hunk payload line {n}")).collect();
    let snapshot = |salt: usize| -> StateAction {
        let files = (0..files)
            .map(|n| {
                let abs = format!("/tmp/repo/src/file{n}.md");
                ChangesetFile {
                    id: format!("file://{abs}"),
                    edit: FileEdit {
                        before: Some(json!({
                            "uri": format!("file://{abs}"),
                            "content": {"uri": format!("hihost-git:/r{salt}")},
                            "bulk": bulk,
                        })),
                        after: Some(json!({"uri": format!("file://{abs}"), "bulk": bulk})),
                        diff: Some(json!({"added": salt as i64, "removed": 1, "bulk": bulk})),
                    },
                    reviewed: None,
                    meta: None,
                }
            })
            .collect();
        StateAction::ChangesetContentChanged(Box::new(ChangesetContentChangedAction {
            files,
            operations: None,
        }))
    };

    let (mut changes, folder) = mirror();
    let median_ms = |actions: &[StateAction], folder: &ResourceLocation| -> f64 {
        let mut times = Vec::new();
        for _ in 0..5 {
            let started = std::time::Instant::now();
            let digested = digest_actions(&FileUris, folder, actions);
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(
                digested
                    .iter()
                    .filter(|action| matches!(action, ChangeAction::Content(_)))
                    .count(),
                1,
                "one surviving snapshot per batch"
            );
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        times[times.len() / 2]
    };

    let lone = [snapshot(7)];
    let backlog: Vec<StateAction> = (0..8).map(snapshot).collect();
    let one = median_ms(&lone, &folder);
    let eight = median_ms(&backlog, &folder);
    eprintln!(
        "[perf] digest median over {files} files: 1 snapshot {one:.2}ms, 8 snapshots {eight:.2}ms"
    );
    assert!(
        eight < (one * 3.0).max(2.0),
        "superseded snapshots must never convert: 1x {one:.2}ms vs 8x {eight:.2}ms"
    );

    // And the UI-thread share — the fold of finished entries — stays
    // a fraction of the digestion it was freed from.
    let digested = digest_actions(&FileUris, &folder, &backlog);
    let started = std::time::Instant::now();
    changes.fold(&folder, digested);
    let fold_ms = started.elapsed().as_secs_f64() * 1000.0;
    eprintln!("[perf] fold median over {files} files: {fold_ms:.2}ms");
    assert_eq!(changes.folder_set(&folder).unwrap().files.len(), files);
}
