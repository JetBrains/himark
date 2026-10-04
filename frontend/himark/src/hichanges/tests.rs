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
use crate::changes_view::RowItem;
use crate::drivers::changes::{digest_actions, digest_state, CatalogEntry};
use crate::{ForestNode};
use editor::location::Authority;
use editor::location::ResourceLocation;
use editor::location::ResourceType;


/// The collection the test's sets live in — wired to sibling ids
/// nothing here resolves, the way the ceremony would wire them.
fn changes_id() -> imba::store::Id<Changes> {
    static ID: std::sync::OnceLock<imba::store::Id<Changes>> = std::sync::OnceLock::new();
    *ID.get_or_init(imba::store::Id::mint)
}

fn wired() -> Changes {
    Changes::wired(imba::store::Id::mint(), imba::store::Id::mint())
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
        authority: &editor::location::Authority,
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
    changes.seed_working_set_for_tests(&folder());
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
    use crate::drivers::changes::{claim_channels, FolderWire};
    let session = crate::higent::SessionUri::new("hihost-fs:/local");
    let entry = |description: Option<&str>, uri: &str| CatalogEntry {
        uri: crate::higent::ChannelUri::new(uri),
        description: description.map(str::to_owned),
        kind: "uncommitted".to_owned(),
    };
    let mut folders = rpds::HashTrieMapSync::new_sync();
    folders.insert_mut(
        folder(),
        FolderWire {
            client: crate::higent::client::inert(),
            session: session.clone(),
            channel: None,
            serial: 0,
        },
    );
    let fresh = claim_channels(
        &folders,
        &session,
        &[
            entry(Some("/somewhere/else"), "hihost-changes://somewhere/else"),
            entry(Some("/tmp/repo"), "hihost-changes://tmp/repo"),
        ],
    );
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].0, folder());
    assert_eq!(fresh[0].2.as_str(), "hihost-changes://tmp/repo");

    // Claimed: the channel stands, a re-announced catalog claims
    // nothing fresh.
    folders.insert_mut(
        folder(),
        FolderWire {
            client: crate::higent::client::inert(),
            session: session.clone(),
            channel: Some(crate::higent::ChannelUri::new("hihost-changes://tmp/repo")),
            serial: 0,
        },
    );
    let again = claim_channels(
        &folders,
        &session,
        &[entry(Some("/tmp/repo"), "hihost-changes://tmp/repo")],
    );
    assert!(again.is_empty());
}

#[test]
fn a_lone_folder_takes_a_lone_foreign_entry() {
    use crate::drivers::changes::{claim_channels, FolderWire};
    let session = crate::higent::SessionUri::new("hihost-fs:/local");
    let mut folders = rpds::HashTrieMapSync::new_sync();
    folders.insert_mut(
        folder(),
        FolderWire {
            client: crate::higent::client::inert(),
            session: session.clone(),
            channel: None,
            serial: 0,
        },
    );
    let fresh = claim_channels(
        &folders,
        &session,
        &[CatalogEntry {
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
        EMPTY_AUTHORITY,
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
    use crate::drivers::changes::{apply_poll, apply_snapshot, ChangesWire, FolderWire};

    let mut store = imba::store::Store::new();
    let ui = imba::ui::UiCtx::dont_use_too_slow();
    let _window = crate::WindowId::from_raw(7);
    let wire_id: imba::store::Id<ChangesWire> = imba::store::Id::mint();
    let history_wire: imba::store::Id<crate::drivers::history::HistoryWire> =
        imba::store::Id::mint();
    store.put_entity(changes_id(), wired());
    store.put_entity(
        wire_id,
        ChangesWire::wired(changes_id(), history_wire, Some(Arc::new(FileUris))),
    );
    ChangesWire::seed_folder_for_tests(
        &mut store,
        wire_id,
        folder(),
        FolderWire {
            client: crate::higent::client::inert(),
            session: crate::higent::SessionUri::new("hihost-fs:/local"),
            channel: Some(crate::higent::ChannelUri::new("hihost-changes://tmp/repo")),
            serial: 0,
        },
    );
    crate::hichanges::Changes::ensure_working_set(&mut store, changes_id(), &folder());
    let launches = |batch: imba::effect::Batch<imba::command::Verb>| {
        batch
            .drain()
            .into_iter()
            .filter(|message| matches!(message, imba::effect::Message::Launch(..)))
            .count()
    };

    // The subscribe landing arms the ONE standing loop (serial 1).
    let mut batch = imba::effect::Batch::new();
    apply_snapshot(
        &mut store,
        &ui,
        wire_id,
        &folder(),
        Ok(digest_state(&FileUris, &folder(), &ready(vec![]))),
        &mut batch.effects(),
    );
    assert_eq!(launches(batch), 1, "the snapshot arms the poll");

    // A re-subscribe supersedes the loop: its landing bumps the
    // serial (to 2).
    let mut batch = imba::effect::Batch::new();
    apply_snapshot(
        &mut store,
        &ui,
        wire_id,
        &folder(),
        Ok(digest_state(&FileUris, &folder(), &ready(vec![]))),
        &mut batch.effects(),
    );
    assert_eq!(launches(batch), 1);

    // The OLD loop's landing: the batch folds, the loop dies.
    let mut batch = imba::effect::Batch::new();
    apply_poll(
        &mut store,
        &ui,
        wire_id,
        &folder(),
        1,
        digest_actions(
            &FileUris,
            &folder(),
            &[StateAction::ChangesetFileSet(ChangesetFileSetAction {
                file: wire_file("late.md", None, false, (1, 0)),
            })],
        ),
        &mut batch.effects(),
    );
    assert_eq!(launches(batch), 0, "a stale landing never rearms");
    assert_eq!(
        crate::hichanges::Changes::of(&store, changes_id())
            .unwrap()
            .folder_set(&folder())
            .unwrap()
            .files
            .len(),
        1,
        "its drained actions still fold"
    );

    // The CURRENT loop keeps polling.
    let mut batch = imba::effect::Batch::new();
    apply_poll(
        &mut store,
        &ui,
        wire_id,
        &folder(),
        2,
        Vec::new(),
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
