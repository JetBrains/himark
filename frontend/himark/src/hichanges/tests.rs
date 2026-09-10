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
        unsubscribe_document(channel: &String) -> ();
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

fn mirror() -> (Changes, ChangeRefs, ResourceLocation) {
    let mut changes = Changes::default();
    let refs = ChangeRefs::default();
    changes.uris = Some(Arc::new(FileUris));
    changes.folders.insert_mut(
        folder(),
        FolderChanges {
            seat: Arc::new(InertSeat),
            session: "hihost-fs:/local".to_owned(),
            channel: Some("hihost-changes://tmp/repo".to_owned()),
            status: ChangesStatus::Computing,
            files: rpds::VectorSync::new_sync(),
        },
    );
    (changes, refs, folder())
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
    let (mut changes, _refs, folder) = mirror();
    let mut pending = changes.folders.get(&folder).unwrap().clone();
    pending.channel = None;
    changes.folders.insert_mut(folder.clone(), pending);
    changes.session = Some(SessionFeed {
        uri: "hihost-fs:/local".to_owned(),
        seat: Arc::new(InertSeat),
        catalog: rpds::VectorSync::new_sync(),
    });
    let fresh = changes.adopt_catalog(
        "hihost-fs:/local",
        vec![
            CatalogEntry {
                uri: "hihost-changes://somewhere/else".to_owned(),
                description: Some("/somewhere/else".to_owned()),
                kind: "uncommitted".to_owned(),
            },
            CatalogEntry {
                uri: "hihost-changes://tmp/repo".to_owned(),
                description: Some("/tmp/repo".to_owned()),
                kind: "uncommitted".to_owned(),
            },
        ],
    );
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].0, folder);
    assert_eq!(fresh[0].2, "hihost-changes://tmp/repo");
    assert_eq!(
        changes.folders.get(&folder).unwrap().channel.as_deref(),
        Some("hihost-changes://tmp/repo")
    );

    let again = changes.adopt_catalog(
        "hihost-fs:/local",
        vec![CatalogEntry {
            uri: "hihost-changes://tmp/repo".to_owned(),
            description: Some("/tmp/repo".to_owned()),
            kind: "uncommitted".to_owned(),
        }],
    );
    assert!(again.is_empty());
}

#[test]
fn a_lone_folder_takes_a_lone_foreign_entry() {
    let (mut changes, _refs, folder) = mirror();
    let mut pending = changes.folders.get(&folder).unwrap().clone();
    pending.channel = None;
    changes.folders.insert_mut(folder.clone(), pending);
    changes.session = Some(SessionFeed {
        uri: "hihost-fs:/local".to_owned(),
        seat: Arc::new(InertSeat),
        catalog: rpds::VectorSync::new_sync(),
    });
    let fresh = changes.adopt_catalog(
        "hihost-fs:/local",
        vec![CatalogEntry {
            uri: "vscode-changes:/session".to_owned(),
            description: None,
            kind: "uncommitted".to_owned(),
        }],
    );
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].2, "vscode-changes:/session");
}

#[test]
fn a_snapshot_adopts_into_digested_entries_and_refs() {
    let (mut changes, refs, folder) = mirror();
    changes.adopt(
        &refs,
        &folder,
        &ready(vec![
            wire_file("src/notes.md", Some("hihost-git:/one"), false, (2, 1)),
            wire_file("added.md", None, false, (3, 0)),
            wire_file("gone.md", Some("hihost-git:/two"), true, (0, 5)),
        ]),
    );
    let entry = changes.folders.get(&folder).expect("the mirror entry");
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

    assert!(refs.lookup("/tmp/repo/src/notes.md").is_some());
    assert!(refs.lookup("/tmp/repo/gone.md").is_some());
    assert!(
        refs.lookup("/tmp/repo/added.md").is_none(),
        "no before, no ref"
    );
    assert_eq!(changes.generation, 1, "the landing bumped the generation");
}

#[test]
fn the_fold_mirrors_the_official_reducers() {
    let (mut changes, refs, folder) = mirror();
    changes.adopt(
        &refs,
        &folder,
        &ready(vec![wire_file("a.md", Some("r1"), false, (1, 1))]),
    );

    changes.fold(
        &refs,
        &folder,
        &[StateAction::ChangesetContentChanged(Box::new(
            ChangesetContentChangedAction {
                files: vec![
                    wire_file("b.md", Some("r2"), false, (2, 2)),
                    wire_file("c.md", None, false, (1, 0)),
                ],
                operations: None,
                error: None,
            },
        ))],
    );
    let entry = changes.folders.get(&folder).unwrap();
    assert_eq!(entry.files.len(), 2);
    assert_eq!(entry.status, ChangesStatus::Ready);
    assert!(
        refs.lookup("/tmp/repo/a.md").is_none(),
        "the replaced file's ref is gone"
    );
    assert!(refs.lookup("/tmp/repo/b.md").is_some());

    changes.fold(
        &refs,
        &folder,
        &[StateAction::ChangesetFileSet(ChangesetFileSetAction {
            file: wire_file("b.md", Some("r3"), false, (9, 9)),
        })],
    );
    let entry = changes.folders.get(&folder).unwrap();
    assert_eq!(entry.files.len(), 2, "an upsert replaces, never duplicates");
    let b = entry
        .files
        .iter()
        .find(|change| change.working.name() == "b.md")
        .unwrap();
    assert_eq!(b.added, Some(9));
    changes.fold(
        &refs,
        &folder,
        &[StateAction::ChangesetFileRemoved(
            ChangesetFileRemovedAction {
                file_id: "file:///tmp/repo/c.md".to_owned(),
            },
        )],
    );
    assert_eq!(changes.folders.get(&folder).unwrap().files.len(), 1);

    changes.fold(
        &refs,
        &folder,
        &[StateAction::ChangesetStatusChanged(
            ChangesetStatusChangedAction {
                status: ChangesetStatus::Computing,
                error: None,
            },
        )],
    );
    assert_eq!(
        changes.folders.get(&folder).unwrap().status,
        ChangesStatus::Computing
    );
    changes.fold(
        &refs,
        &folder,
        &[StateAction::ChangesetCleared(ChangesetClearedAction {})],
    );
    let entry = changes.folders.get(&folder).unwrap();
    assert!(entry.files.is_empty());
    assert!(
        refs.lookup("/tmp/repo/b.md").is_none(),
        "cleared files clear their refs"
    );
}

#[test]
fn a_failed_subscribe_reports_on_the_row() {
    let (mut changes, refs, folder) = mirror();
    changes.adopt_error(&refs, &folder, "not a repository".to_owned());
    let entry = changes.folders.get(&folder).unwrap();
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
    let (mut changes, refs, folder) = mirror();
    changes.adopt(
        &refs,
        &folder,
        &ready(vec![
            wire_file("top.md", Some("r0"), false, (1, 0)),
            wire_file("src/deep/inner/one.md", Some("r1"), false, (2, 1)),
            wire_file("src/deep/inner/two.md", None, false, (4, 0)),
            wire_file("docs/guide.md", Some("r2"), false, (0, 3)),
        ]),
    );
    let entry = changes.folders.get(&folder).unwrap();
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
    let (mut changes, refs, folder) = mirror();
    changes.adopt(
        &refs,
        &folder,
        &ready(vec![
            wire_file("mod.md", Some("hihost-git:/base"), false, (1, 1)),
            wire_file("new.md", None, false, (2, 0)),
        ]),
    );
    let entry = changes.folders.get(&folder).unwrap();
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
        Some(RowItem::File {
            old: Some(old),
            new,
        }) => {
            assert_eq!(raw_ref(old).expect("a before ref").1, "hihost-git:/base");
            assert_eq!(new, &key("mod.md"), "the new side IS the working copy");
        }
        other => panic!("expected a paired file row, got {:?}", other.is_some()),
    }
    match items.get(&key("new.md")) {
        Some(RowItem::File { old: None, .. }) => {}
        other => panic!("an add carries no old side, got {:?}", other.is_some()),
    }
}
