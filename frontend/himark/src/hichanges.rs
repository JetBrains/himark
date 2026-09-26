// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::higent::ahp_types::actions::StateAction;
use crate::higent::ahp_types::state::{ChangesetFile, ChangesetState, ChangesetStatus};
use crate::higent::{AhpServer, PollChangesetEffect, SubscribeChangesetEffect};
use crate::{
    AppCommand, Authority, ForestList, ForestNode, ForestSearcher, ModalRequest, ModalView,
    ActivateTrigger, ListKeyCommand, ListKeyboardController, ResourceLocation, ResourceType,
    TreeListCommand,
};
use imba::list::ListOps;
use himark_ahp_ext_types::history as history_wire;
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::{AnyEffect, Effects},
    event::{Event, EventResult, Key as InputKey},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View,
};
use skia_safe::Size;

const NOTE_KIND: &str = "changes-note";

const PANEL_PAD: f32 = 6.0;

const REF_PREFIX: &str = "ahpref\u{1f}";

const EMPTY_AUTHORITY: &str = "changes-empty";

pub fn before_ref_location(
    origin_authority: &str,
    raw_uri: &str,
    path: Vec<String>,
) -> ResourceLocation {
    ResourceLocation::new(
        ResourceType::document(),
        Authority::new(format!("{REF_PREFIX}{origin_authority}\u{1f}{raw_uri}")),
        path,
    )
}

pub fn scoped(location: &ResourceLocation) -> bool {
    location.authority().as_str().starts_with(REF_PREFIX)
}

pub fn raw_ref(location: &ResourceLocation) -> Option<(String, String)> {
    let body = location.authority().as_str().strip_prefix(REF_PREFIX)?;
    let (origin, raw) = body.split_once('\u{1f}')?;
    Some((origin.to_owned(), raw.to_owned()))
}

pub fn working_copy(location: &ResourceLocation) -> Option<ResourceLocation> {
    let (origin, _) = raw_ref(location)?;
    Some(ResourceLocation::new(
        ResourceType::document(),
        Authority::new(origin),
        location.path().to_vec(),
    ))
}

pub(crate) fn empty_side(of: &ResourceLocation) -> ResourceLocation {
    ResourceLocation::new(
        ResourceType::document(),
        Authority::new(EMPTY_AUTHORITY),
        of.path().to_vec(),
    )
}

#[derive(serde::Deserialize)]
struct WireSide {
    uri: String,
    #[serde(default)]
    content: Option<WireContent>,
}

#[derive(serde::Deserialize)]
struct WireContent {
    uri: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct WireCounts {
    #[serde(default)]
    added: Option<i64>,
    #[serde(default)]
    removed: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeEntry {
    id: String,

    pub(crate) rel: Vec<String>,

    pub working: ResourceLocation,

    pub before: Option<ResourceLocation>,

    pub after: Option<ResourceLocation>,
    pub added: Option<i64>,
    pub removed: Option<i64>,
    /// The `Changes` generation at which the host last touched this
    /// entry — the canvas's per-row staleness probe. `ChangesetFileSet`
    /// stamps unconditionally (the host resends a file only when it
    /// changed — value equality cannot see a same-stats content edit);
    /// full-snapshot replaces stamp by value comparison against the
    /// previous entry with the same id.
    pub(crate) updated: u64,
}

pub(crate) fn entry_of(
    uris: &dyn crate::higent::ResourceUriMap,
    folder: &ResourceLocation,
    file: &ChangesetFile,
) -> Option<ChangeEntry> {
    let side = |value: &Option<serde_json::Value>| -> Option<WireSide> {
        value
            .as_ref()
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    };
    let before = side(&file.edit.before);
    let after = side(&file.edit.after);
    let working = after.as_ref().or(before.as_ref()).and_then(|side| {
        uris.location_of(
            &crate::higent::ResourceUri::new(side.uri.as_str()),
            ResourceType::document(),
            folder.authority(),
        )
    })?;
    if !working.path().starts_with(folder.path()) {
        return None;
    }
    let rel: Vec<String> = working.path()[folder.path().len()..].to_vec();
    if rel.is_empty() {
        return None;
    }
    let path: Vec<String> = working.path().to_vec();
    let after_ref = after.and_then(|side| side.content).map(|content| {
        before_ref_location(folder.authority().as_str(), &content.uri, path.clone())
    });
    let before = before.and_then(|side| side.content).map(|content| {
        before_ref_location(folder.authority().as_str(), &content.uri, path.clone())
    });
    let counts: WireCounts = file
        .edit
        .diff
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    Some(ChangeEntry {
        id: file.id.clone(),
        rel,
        working,
        before,
        after: after_ref,
        added: counts.added,
        removed: counts.removed,
        updated: 0,
    })
}

/// Value identity for stamping — everything but the stamp itself.
fn same_entry(a: &ChangeEntry, b: &ChangeEntry) -> bool {
    a.id == b.id
        && a.rel == b.rel
        && a.working == b.working
        && a.before == b.before
        && a.after == b.after
        && a.added == b.added
        && a.removed == b.removed
}

/// Carries stamps across a full-list replace: an entry value-equal to
/// its predecessor (same id) keeps the old stamp; new or changed
/// entries take `stamp`.
fn stamp_entries<'a>(
    previous: impl Iterator<Item = &'a ChangeEntry>,
    fresh: impl Iterator<Item = &'a mut ChangeEntry>,
    stamp: u64,
) {
    let by_id: std::collections::HashMap<&str, &ChangeEntry> =
        previous.map(|entry| (entry.id.as_str(), entry)).collect();
    for entry in fresh {
        entry.updated = match by_id.get(entry.id.as_str()) {
            Some(old) if same_entry(old, entry) => old.updated,
            _ => stamp,
        };
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangesStatus {
    Computing,
    Ready,
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChangeSetId(u64);

impl ChangeSetId {
    fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// What a change set IS: one folder's working copy, or one commit —
/// `CanvasSource`'s shape, made the model's first-class identity.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ChangeSetSource {
    WorkingCopy {
        folder: ResourceLocation,
    },
    /// Minted eagerly (and light) when a history window lands; the
    /// file entries land lazily on first ask. (Stage B of the
    /// migration constructs these.)
    #[allow(dead_code)]
    Commit {
        folder: ResourceLocation,
        revision: String,
    },
}

/// ONE set of changes (docs/model-view.md): the model payload — files,
/// status, the feed channel — plus everything the set OWNS: its own
/// generation, its base refs, and (per the hierarchy) its views.
/// A set's WIRE side: the seat serving it, the owning AHP session and
/// the claimed changeset channel. Optional on the set — a canvas may
/// open a set the feeds have not routed yet (a DETACHED set);
/// `ensure_folder` attaches the feed when the route exists.
#[derive(Clone)]
pub(crate) struct SetFeed {
    pub(crate) seat: Arc<dyn AhpServer>,
    pub(crate) session: crate::higent::SessionUri,
    pub(crate) channel: Option<crate::higent::ChannelUri>,
}

#[derive(Clone)]
pub struct ChangeSet {
    pub(crate) source: ChangeSetSource,

    pub(crate) feed: Option<SetFeed>,

    pub status: ChangesStatus,
    pub files: rpds::VectorSync<ChangeEntry>,

    /// The SET's generation — bumped by every mutation of this set,
    /// compared by this set's views (tree sections, canvases) and by
    /// its entries' `updated` stamps. Never session-wide again.
    generation: u64,

    /// Working file (absolute path) → its BASE ref, denormalized from
    /// THIS set's entries (replaced whole per landing). Read at effect
    /// launch on the UI thread. (This replaced a shared
    /// Arc<Mutex<HashMap>>; never bring that back.)
    bases: rpds::HashTrieMapSync<String, ResourceLocation>,

    /// The set OWNS its canvases (docs/model-view.md hierarchy) —
    /// canvas mutations never touch the set's `generation`.
    pub(crate) canvases: rpds::HashTrieMapSync<crate::diff_canvas::canvas::CanvasId, crate::diff_canvas::canvas::Canvas>,
}

impl ChangeSet {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Re-derive THIS set's base refs from its entries, whole — the
    /// set owns its slice, no cross-set scrubbing.
    fn note_bases(&mut self) {
        let mut bases = rpds::HashTrieMapSync::new_sync();
        for change in self.files.iter() {
            let Some(before) = change.before.clone() else {
                continue;
            };
            bases.insert_mut(format!("/{}", change.working.path().join("/")), before);
        }
        self.bases = bases;
    }
}

/// Transitional alias — the tests and older call sites named the
/// per-folder record `FolderChanges`; it IS the working-copy
/// `ChangeSet` now.
pub type FolderChanges = ChangeSet;

impl ChangesStatus {
    pub(crate) fn of_wire(status: &ChangesetStatus, error: Option<&str>) -> ChangesStatus {
        match status {
            ChangesetStatus::Computing => ChangesStatus::Computing,
            ChangesetStatus::Ready => ChangesStatus::Ready,
            ChangesetStatus::Error => {
                ChangesStatus::Error(error.unwrap_or("changeset error").to_owned())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogEntry {
    pub(crate) uri: crate::higent::ChannelUri,
    pub(crate) description: Option<String>,

    pub(crate) kind: String,
}

#[derive(Clone)]
struct SessionFeed {
    uri: crate::higent::SessionUri,
    seat: Arc<dyn AhpServer>,
    catalog: rpds::VectorSync<CatalogEntry>,
}

fn digest_catalog(changesets: &[crate::higent::ahp_types::state::Changeset]) -> Vec<CatalogEntry> {
    changesets
        .iter()
        .filter(|entry| {
            (entry.change_kind == "uncommitted"
                || entry.change_kind == history_wire::HISTORY_CHANGE_KIND)
                && !entry.uri_template.contains('{')
        })
        .map(|entry| CatalogEntry {
            uri: crate::higent::ChannelUri::new(entry.uri_template.clone()),
            description: entry.description.clone(),
            kind: entry.change_kind.clone(),
        })
        .collect()
}

pub(crate) fn entry_serves(folder: &ResourceLocation, entry: &CatalogEntry) -> bool {
    let abs = format!("/{}", folder.path().join("/"));
    entry.description.as_deref() == Some(abs.as_str()) || entry.uri.as_str().ends_with(&abs)
}

/// The session's change sets, keyed by minted id
/// (docs/model-view.md): one record per working copy, one per commit.
/// Session-family state — the AHP changeset/history channels are
/// session-level (docs/ahp/vcs.md), so the collection gathers with
/// its session.
#[derive(Clone, Default)]
pub struct ChangeSets {
    sets: rpds::HashTrieMapSync<ChangeSetId, ChangeSet>,

    /// Source → set: the reuse lookup for BOTH flavors.
    by_source: rpds::HashTrieMapSync<ChangeSetSource, ChangeSetId>,

    session: Option<SessionFeed>,

    pub(crate) uris: Option<Arc<dyn crate::higent::ResourceUriMap>>,

    /// The COLLECTION tick: membership moves and any set's mutation
    /// bump it — the union tree view's staleness cue. Per-set
    /// staleness reads the set's own generation.
    generation: u64,

    /// The union tree views over the session's working-copy sets.
    /// TRANSITIONAL home (stage A2 splits the tree into per-set
    /// sections owned by their sets, per the hierarchy).
    views: rpds::HashTrieMapSync<ChangesViewId, ChangesView>,
}

/// Transitional alias — the store slot and the wide call-site surface
/// named the collection `Changes`.
pub type Changes = ChangeSets;

impl Changes {
    fn feed_for(&self, session: &crate::higent::SessionUri) -> Option<&SessionFeed> {
        self.session.as_ref().filter(|feed| feed.uri == *session)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sets.is_empty() && self.session.is_none()
    }

    fn folder_set(&self, folder: &ResourceLocation) -> Option<&ChangeSet> {
        let source = ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        };
        self.sets.get(self.by_source.get(&source)?)
    }

    /// Every working-copy set with its folder, iteration order
    /// unspecified.
    fn working_copies(&self) -> impl Iterator<Item = (&ResourceLocation, &ChangeSet)> {
        self.by_source.iter().filter_map(|(source, id)| {
            let ChangeSetSource::WorkingCopy { folder } = source else {
                return None;
            };
            Some((folder, self.sets.get(id)?))
        })
    }

    /// Mutate one folder's set through clone-modify-insert, bumping
    /// BOTH generations: the set's own (its views' cue) and the
    /// collection tick (the union tree's cue).
    fn update_folder_set(
        &mut self,
        folder: &ResourceLocation,
        mutate: impl FnOnce(&mut ChangeSet),
    ) -> bool {
        let source = ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        };
        self.update_set_by_source(&source, mutate)
    }

    fn update_set_by_source(
        &mut self,
        source: &ChangeSetSource,
        mutate: impl FnOnce(&mut ChangeSet),
    ) -> bool {
        let Some(id) = self.by_source.get(source).copied() else {
            return false;
        };
        let Some(mut set) = self.sets.get(&id).cloned() else {
            return false;
        };
        mutate(&mut set);
        set.generation += 1;
        self.sets.insert_mut(id, set);
        self.generation += 1;
        true
    }

    /// Mint — or find — the COMMIT-flavored set (eager and light: the
    /// history row references it from birth; its files land lazily on
    /// first ask). Never bumps the commit set's generation.
    pub(crate) fn ensure_commit_set(
        store: &mut Store,
        folder: &ResourceLocation,
        revision: &str,
        seat: &Arc<dyn AhpServer>,
        session: &crate::higent::SessionUri,
    ) -> ChangeSetId {
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        if let Some(id) = store
            .get::<ChangeSets>()
            .and_then(|changes| changes.by_source.get(&source).copied())
        {
            return id;
        }
        let id = ChangeSetId::mint();
        let seat = seat.clone();
        let session = session.to_owned();
        store.update::<ChangeSets>(|changes| {
            changes.sets.insert_mut(
                id,
                ChangeSet {
                    source: source.clone(),
                    feed: Some(SetFeed {
                        seat,
                        session,
                        channel: None,
                    }),
                    status: ChangesStatus::Computing,
                    files: rpds::VectorSync::new_sync(),
                    generation: 0,
                    bases: rpds::HashTrieMapSync::new_sync(),
                    canvases: rpds::HashTrieMapSync::new_sync(),
                },
            );
            changes.by_source.insert_mut(source, id);
        });
        id
    }

    pub fn commit_set(
        store: &Store,
        folder: &ResourceLocation,
        revision: &str,
    ) -> Option<ChangeSet> {
        let changes = store.get::<ChangeSets>()?;
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        changes.sets.get(changes.by_source.get(&source)?).cloned()
    }

    /// The per-SET staleness cue for a commit canvas.
    pub fn commit_generation(store: &Store, folder: &ResourceLocation, revision: &str) -> u64 {
        Self::commit_set(store, folder, revision)
            .map(|set| set.generation)
            .unwrap_or(0)
    }


    /// THE UPDATE RULE (docs/model-view.md): a commit set's mutation
    /// must reach every uniting HistoryView — their trees render the
    /// sets' content. The History generation is their staleness cue;
    /// the sync lane rolls the stale views at the batch tail.
    fn nudge_uniting_histories(store: &mut Store) {
        store.update::<crate::hihistory::History>(|history| history.nudge());
    }

    /// Mint — or find — the set for a source, WITHOUT a feed when the
    /// feeds have not routed it yet (a canvas may open first; the
    /// feed attaches at `ensure_folder`). Never bumps generations.
    pub(crate) fn ensure_set_for_source(
        store: &mut Store,
        source: &ChangeSetSource,
    ) -> ChangeSetId {
        if let Some(id) = store
            .get::<ChangeSets>()
            .and_then(|changes| changes.by_source.get(source).copied())
        {
            return id;
        }
        let id = ChangeSetId::mint();
        let source = source.clone();
        store.update::<ChangeSets>(|changes| {
            changes.sets.insert_mut(
                id,
                ChangeSet {
                    source: source.clone(),
                    feed: None,
                    status: ChangesStatus::Computing,
                    files: rpds::VectorSync::new_sync(),
                    generation: 0,
                    bases: rpds::HashTrieMapSync::new_sync(),
                    canvases: rpds::HashTrieMapSync::new_sync(),
                },
            );
            changes.by_source.insert_mut(source, id);
        });
        id
    }

    pub(crate) fn id_for_source(
        store: &Store,
        source: &ChangeSetSource,
    ) -> Option<ChangeSetId> {
        store.get::<ChangeSets>()?.by_source.get(source).copied()
    }

    /// The SET owns its canvases; these reach one by (set, canvas) —
    /// canvas mutations never touch the set's generation.
    pub(crate) fn canvas_ref(
        store: &Store,
        set: ChangeSetId,
        canvas: crate::diff_canvas::canvas::CanvasId,
    ) -> Option<&crate::diff_canvas::canvas::Canvas> {
        store.get::<ChangeSets>()?.sets.get(&set)?.canvases.get(&canvas)
    }

    pub(crate) fn take_canvas(
        store: &mut Store,
        set: ChangeSetId,
        canvas: crate::diff_canvas::canvas::CanvasId,
    ) -> Option<crate::diff_canvas::canvas::Canvas> {
        let held = Self::canvas_ref(store, set, canvas)?.clone();
        store.update::<ChangeSets>(|changes| {
            let Some(mut owner) = changes.sets.get(&set).cloned() else {
                return;
            };
            owner.canvases.remove_mut(&canvas);
            changes.sets.insert_mut(set, owner);
        });
        Some(held)
    }

    pub(crate) fn put_canvas(
        store: &mut Store,
        set: ChangeSetId,
        canvas: crate::diff_canvas::canvas::CanvasId,
        held: crate::diff_canvas::canvas::Canvas,
    ) {
        store.update::<ChangeSets>(|changes| {
            let Some(mut owner) = changes.sets.get(&set).cloned() else {
                return;
            };
            owner.canvases.insert_mut(canvas, held);
            changes.sets.insert_mut(set, owner);
        });
    }

    /// Every canvas in the gathered session, with its owning set —
    /// the batch-tail sweep's domain.
    pub(crate) fn canvas_ids(
        store: &Store,
    ) -> Vec<(ChangeSetId, crate::diff_canvas::canvas::CanvasId)> {
        store
            .get::<ChangeSets>()
            .map(|changes| {
                changes
                    .sets
                    .iter()
                    .flat_map(|(set, held)| {
                        held.canvases.keys().map(|canvas| (*set, *canvas))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The fetch road armed: the set is COMPUTING until its snapshot
    /// lands.
    pub(crate) fn mark_commit_computing(
        store: &mut Store,
        folder: &ResourceLocation,
        revision: &str,
    ) {
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        store.update::<ChangeSets>(|changes| {
            changes.update_set_by_source(&source, |set| {
                set.status = ChangesStatus::Computing;
                set.files = rpds::VectorSync::new_sync();
            });
        });
        Self::nudge_uniting_histories(store);
    }

    /// A commit set's content snapshot landed (the changeset channel
    /// for `?commit=<sha>`): status + files onto the SET.
    pub(crate) fn adopt_commit_state(
        store: &mut Store,
        folder: &ResourceLocation,
        revision: &str,
        result: &Result<ChangesetState, String>,
    ) {
        let Some(uris) = store
            .get::<ChangeSets>()
            .and_then(|changes| changes.uris.clone())
        else {
            return;
        };
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        store.update::<ChangeSets>(|changes| {
            changes.update_set_by_source(&source, |set| match result {
                Ok(state) => {
                    set.status = ChangesStatus::of_wire(
                        &state.status,
                        state.error.as_ref().map(|error| error.message.as_str()),
                    );
                    set.files = state
                        .files
                        .iter()
                        .filter_map(|file| entry_of(&*uris, folder, file))
                        .collect();
                }
                Err(error) => {
                    set.status = ChangesStatus::Error(error.clone());
                    set.files = rpds::VectorSync::new_sync();
                }
            });
        });
        Self::nudge_uniting_histories(store);
    }

    /// Streamed updates for a commit set's changeset channel.
    pub(crate) fn fold_commit_actions(
        store: &mut Store,
        folder: &ResourceLocation,
        revision: &str,
        actions: &[StateAction],
    ) {
        let Some(uris) = store
            .get::<ChangeSets>()
            .and_then(|changes| changes.uris.clone())
        else {
            return;
        };
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        store.update::<ChangeSets>(|changes| {
            changes.update_set_by_source(&source, |set| {
                for action in actions {
                    match action {
                        StateAction::ChangesetContentChanged(content) => {
                            set.files = content
                                .files
                                .iter()
                                .filter_map(|file| entry_of(&*uris, folder, file))
                                .collect();
                        }
                        StateAction::ChangesetStatusChanged(status) => {
                            set.status = ChangesStatus::of_wire(
                                &status.status,
                                status.error.as_ref().map(|error| error.message.as_str()),
                            );
                        }
                        _ => {}
                    }
                }
            });
        });
        Self::nudge_uniting_histories(store);
    }

    pub fn set_ref(store: &Store, id: ChangeSetId) -> Option<&ChangeSet> {
        store.get::<ChangeSets>()?.sets.get(&id)
    }

    pub fn id_for_folder(store: &Store, folder: &ResourceLocation) -> Option<ChangeSetId> {
        let source = ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        };
        store.get::<ChangeSets>()?.by_source.get(&source).copied()
    }

    /// The per-SET staleness cue for a working copy — 0 while absent
    /// (the canvas keeps probing until the set exists).
    pub fn folder_generation(store: &Store, folder: &ResourceLocation) -> u64 {
        store
            .get::<ChangeSets>()
            .and_then(|changes| changes.folder_set(folder))
            .map(|set| set.generation)
            .unwrap_or(0)
    }

    pub fn generation(store: &Store) -> u64 {
        store
            .get::<Changes>()
            .map(|changes| changes.generation)
            .unwrap_or(0)
    }

    pub fn folder(store: &Store, folder: &ResourceLocation) -> Option<ChangeSet> {
        store.get::<Changes>()?.folder_set(folder).cloned()
    }

    pub(crate) fn uris(store: &Store) -> Option<Arc<dyn crate::higent::ResourceUriMap>> {
        store.get::<Changes>()?.uris.clone()
    }

    pub fn ensure(
        store: &mut Store,
        window: crate::WindowId,
        workspace: crate::SessionId,
        fx: &mut crate::AppFx<'_>,
    ) {
        for folder in crate::higent::session_folders(store, &workspace) {
            Self::ensure_folder(store, window, folder, fx);
        }
    }

    pub fn ensure_folder(
        store: &mut Store,
        window: crate::WindowId,
        folder: ResourceLocation,
        fx: &mut crate::AppFx<'_>,
    ) {
        let known = store.get::<Changes>().is_some_and(|changes| {
            changes
                .folder_set(&folder)
                .is_some_and(|set| set.feed.is_some())
        });
        if known {
            return;
        }
        let detached = store.get::<Changes>().is_some_and(|changes| {
            changes.by_source.contains_key(&ChangeSetSource::WorkingCopy {
                folder: folder.clone(),
            })
        });
        let Some((host, seat, session)) =
            crate::higent::seat::route_seat(store, folder.authority().as_str())
        else {
            return;
        };
        let Some(uris) = crate::higent::Hosts::uris(store, host) else {
            return;
        };
        store.update::<Changes>(|changes| changes.uris = Some(uris.clone()));
        let scope = crate::SessionId {
            host,
            session: session.clone(),
        };
        store.update::<Changes>(|changes| {
            if detached {
                // The canvas opened this set before the feeds routed:
                // attach the feed, keep the set (and its canvases).
                changes.update_folder_set(&folder, |set| {
                    set.feed = Some(SetFeed {
                        seat: seat.clone(),
                        session: session.clone(),
                        channel: None,
                    });
                });
                return;
            }
            let id = ChangeSetId::mint();
            changes.sets.insert_mut(
                id,
                ChangeSet {
                    source: ChangeSetSource::WorkingCopy {
                        folder: folder.clone(),
                    },
                    feed: Some(SetFeed {
                        seat: seat.clone(),
                        session: session.clone(),
                        channel: None,
                    }),
                    status: ChangesStatus::Computing,
                    files: rpds::VectorSync::new_sync(),
                    generation: 0,
                    bases: rpds::HashTrieMapSync::new_sync(),
                    canvases: rpds::HashTrieMapSync::new_sync(),
                },
            );
            changes.by_source.insert_mut(
                ChangeSetSource::WorkingCopy {
                    folder: folder.clone(),
                },
                id,
            );
            changes.generation += 1;
        });
        crate::hihistory::History::ensure_folder(store, &folder, &seat, &session);

        let directory = uris.uri_of(&folder).into_string();
        fx.push(
            AnyEffect::new(crate::higent::DispatchChatActionEffect {
                seat: seat.clone(),
                channel: session.as_channel(),
                action: StateAction::SessionWorkingDirectorySet(
                    crate::higent::ahp_types::actions::SessionWorkingDirectorySetAction {
                        directory,
                    },
                ),
            })
            .map(move |result| AppCommand::Dynamic(window, Arc::new(Dispatched { result }))),
        );
        let feed_known = store
            .get::<Changes>()
            .is_some_and(|changes| changes.feed_for(&session).is_some());
        if !feed_known {
            store.update::<Changes>(|changes| {
                changes.session = Some(SessionFeed {
                    uri: session.clone(),
                    seat: seat.clone(),
                    catalog: rpds::VectorSync::new_sync(),
                });
            });
            let landing = session.clone();
            fx.push(
                AnyEffect::new(crate::higent::SubscribeSessionEffect { seat, session }).map(
                    move |result| {
                        AppCommand::dynamic_in(
                            scope.clone(),
                            window,
                            Arc::new(SessionLanded {
                                session: landing.clone(),
                                result,
                            }),
                        )
                    },
                ),
            );
        }
    }

    pub fn script_summary(store: &Store) -> Option<String> {
        let changes = store.get::<Changes>()?;
        let mut lines = Vec::new();
        for (folder, entry) in changes.working_copies() {
            for file in entry.files.iter() {
                let status = match file.before.is_some() {
                    true => "M",
                    false => "A",
                };
                let counts = match (file.added, file.removed) {
                    (Some(added), Some(removed)) => format!(" (+{added} -{removed})"),
                    _ => String::new(),
                };
                lines.push(format!(
                    "{status} {}/{}{counts}",
                    folder.name(),
                    file.rel.join("/"),
                ));
            }
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    pub fn refetch(
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
        only: Option<&ResourceLocation>,
    ) {
        let Some(changes) = store.get::<Changes>() else {
            return;
        };
        let riding: Vec<(ResourceLocation, Arc<dyn AhpServer>, crate::higent::ChannelUri)> = changes
            .working_copies()
            .filter(|(folder, _)| only.is_none_or(|only| *folder == only))
            .filter_map(|(folder, entry)| {
                let feed = entry.feed.as_ref()?;
                let channel = feed.channel.clone()?;
                Some((folder.clone(), feed.seat.clone(), channel))
            })
            .collect();
        if riding.is_empty() {
            return;
        }
        store.update::<Changes>(|changes| {
            for (folder, _, _) in &riding {
                changes.update_folder_set(folder, |set| {
                    set.status = ChangesStatus::Computing;
                });
            }
        });
        for (folder, seat, channel) in riding {
            let landing = folder.clone();
            let scope = folder_scope(&folder);
            fx.push(
                AnyEffect::new(SubscribeChangesetEffect { seat, channel }).map(move |result| {
                    let landed = Arc::new(SnapshotLanded {
                        folder: landing.clone(),
                        result,
                    });
                    match scope.clone() {
                        Some(scope) => AppCommand::dynamic_in(scope, window, landed),
                        None => AppCommand::Dynamic(window, landed),
                    }
                }),
            );
        }
    }

    fn adopt_catalog(
        &mut self,
        session: &crate::higent::SessionUri,
        entries: Vec<CatalogEntry>,
    ) -> Vec<(ResourceLocation, Arc<dyn AhpServer>, crate::higent::ChannelUri)> {
        let Some(mut feed) = self.feed_for(session).cloned() else {
            return Vec::new();
        };
        feed.catalog = entries.iter().cloned().collect();
        self.session = Some(feed);
        let changesets: Vec<CatalogEntry> = entries
            .iter()
            .filter(|entry| entry.kind == "uncommitted")
            .cloned()
            .collect();
        let mut fresh = Vec::new();
        let lone_folder = self
            .working_copies()
            .filter(|(_, entry)| {
                entry
                    .feed
                    .as_ref()
                    .is_some_and(|feed| feed.session == *session)
            })
            .count()
            == 1;
        let riding: Vec<(ResourceLocation, ChangeSet)> = self
            .working_copies()
            .map(|(folder, entry)| (folder.clone(), entry.clone()))
            .collect();
        for (folder, entry) in riding {
            let Some(feed) = entry.feed.clone() else {
                continue;
            };
            if feed.session != *session || feed.channel.is_some() {
                continue;
            }
            let matched = changesets
                .iter()
                .find(|candidate| entry_serves(&folder, candidate))
                .or_else(|| (lone_folder && changesets.len() == 1).then(|| &changesets[0]));
            let Some(matched) = matched else {
                continue;
            };
            let seat = feed.seat.clone();
            let channel = matched.uri.clone();
            let claimed = matched.uri.clone();
            self.update_folder_set(&folder, |set| {
                if let Some(feed) = &mut set.feed {
                    feed.channel = Some(claimed);
                }
            });
            fresh.push((folder, seat, channel));
        }
        self.generation += 1;
        fresh
    }

    fn adopt(&mut self, folder: &ResourceLocation, state: &ChangesetState) {
        let Some(uris) = self.uris.clone() else {
            return;
        };
        self.update_folder_set(folder, |set| {
            set.status = ChangesStatus::of_wire(
                &state.status,
                state.error.as_ref().map(|error| error.message.as_str()),
            );
            let mut fresh: Vec<ChangeEntry> = state
                .files
                .iter()
                .filter_map(|file| entry_of(&*uris, folder, file))
                .collect();
            stamp_entries(set.files.iter(), fresh.iter_mut(), set.generation + 1);
            set.files = fresh.into_iter().collect();
            set.note_bases();
        });
    }

    fn session_failed(&mut self, session: &crate::higent::SessionUri, error: &str) {
        let riding: Vec<ResourceLocation> = self
            .working_copies()
            .filter(|(_, entry)| {
                entry
                    .feed
                    .as_ref()
                    .is_some_and(|feed| feed.session == *session)
            })
            .map(|(folder, _)| folder.clone())
            .collect();
        for folder in riding {
            self.adopt_error(&folder, error.to_owned());
        }
    }

    fn adopt_error(&mut self, folder: &ResourceLocation, error: String) {
        self.update_folder_set(folder, |set| {
            set.status = ChangesStatus::Error(error);
            set.files = rpds::VectorSync::new_sync();
            set.note_bases();
        });
    }

    fn fold(&mut self, folder: &ResourceLocation, actions: &[StateAction]) {
        let Some(uris) = self.uris.clone() else {
            return;
        };
        self.update_folder_set(folder, |set| {
        let entry = set;
        let stamp = entry.generation + 1;
        let previous: Vec<ChangeEntry> = entry.files.iter().cloned().collect();
        let mut files: Vec<Option<ChangeEntry>> = entry.files.iter().cloned().map(Some).collect();
        let mut by_id: std::collections::HashMap<String, usize> = files
            .iter()
            .enumerate()
            .filter_map(|(at, slot)| slot.as_ref().map(|file| (file.id.clone(), at)))
            .collect();
        for action in actions {
            match action {
                StateAction::ChangesetContentChanged(content) => {
                    let mut fresh: Vec<ChangeEntry> = content
                        .files
                        .iter()
                        .filter_map(|file| entry_of(&*uris, folder, file))
                        .collect();
                    stamp_entries(previous.iter(), fresh.iter_mut(), stamp);
                    files = fresh.into_iter().map(Some).collect();
                    by_id = files
                        .iter()
                        .enumerate()
                        .filter_map(|(at, slot)| slot.as_ref().map(|file| (file.id.clone(), at)))
                        .collect();
                }
                StateAction::ChangesetStatusChanged(status) => {
                    entry.status = ChangesStatus::of_wire(
                        &status.status,
                        status.error.as_ref().map(|error| error.message.as_str()),
                    );
                }
                StateAction::ChangesetFileSet(set) => {
                    if let Some(at) = by_id.remove(&set.file.id) {
                        files[at] = None;
                    }
                    if let Some(mut fresh) = entry_of(&*uris, folder, &set.file) {
                        // The host resends a file exactly when it
                        // changed — stamp unconditionally; value
                        // equality cannot see a same-stats edit.
                        fresh.updated = stamp;
                        by_id.insert(fresh.id.clone(), files.len());
                        files.push(Some(fresh));
                    }
                }
                StateAction::ChangesetFileRemoved(removed) => {
                    if let Some(at) = by_id.remove(&removed.file_id) {
                        files[at] = None;
                    }
                }
                StateAction::ChangesetCleared(_) => {
                    files.clear();
                    by_id.clear();
                }
                _ => {}
            }
        }
        entry.files = files.into_iter().flatten().collect();
        entry.note_bases();
        });
    }

    #[cfg(test)]
    pub(crate) fn base_lookup(&self, abs_path: &str) -> Option<ResourceLocation> {
        self.sets
            .values()
            .find_map(|set| set.bases.get(abs_path).cloned())
    }

    /// The base ref for a working file, by absolute path — read at
    /// effect launch (UI thread, store in hand). Each working-copy
    /// set owns its slice; the sets are few.
    pub fn base_ref(store: &Store, abs_path: &str) -> Option<ResourceLocation> {
        store
            .get::<Changes>()?
            .sets
            .values()
            .find_map(|set| set.bases.get(abs_path).cloned())
    }
}

pub(crate) struct Dispatched {
    pub(crate) result: Result<(), String>,
}

impl crate::DynamicCommand for Dispatched {
    fn id(&self) -> &'static str {
        "changes.dispatched"
    }
    fn name(&self) -> String {
        "Changes Dispatch".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut crate::AppFx<'_>,
    ) {
        if let Err(error) = &self.result {
            eprintln!("[hichanges] workingDirectorySet failed: {error}");
        }
    }
}

struct SessionLanded {
    session: crate::higent::SessionUri,
    result: Result<crate::higent::ahp_types::state::SessionState, String>,
}

impl crate::DynamicCommand for SessionLanded {
    fn id(&self) -> &'static str {
        "changes.session-landed"
    }
    fn name(&self) -> String {
        "Changes Catalog".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        match &self.result {
            Ok(state) => {
                let entries = digest_catalog(state.changesets.as_deref().unwrap_or_default());
                subscribe_fresh(store, window, &self.session, entries, fx);
                relaunch_session_poll(store, window, &self.session, fx);
            }
            Err(error) => {
                eprintln!("[hichanges] session subscribe failed: {error}");
                store.update::<Changes>(|changes| {
                    changes.session_failed(&self.session, error);
                });
                crate::hihistory::History::session_failed(store, &self.session, error);
            }
        }
    }
}

struct SessionPolled {
    session: crate::higent::SessionUri,
    actions: Vec<StateAction>,
}

impl crate::DynamicCommand for SessionPolled {
    fn id(&self) -> &'static str {
        "changes.session-polled"
    }
    fn name(&self) -> String {
        "Changes Catalog Update".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        for action in &self.actions {
            if let StateAction::SessionChangesetsChanged(changed) = action {
                let entries = digest_catalog(changed.changesets.as_deref().unwrap_or_default());
                subscribe_fresh(store, window, &self.session, entries, fx);
            }
        }
        relaunch_session_poll(store, window, &self.session, fx);
    }
}

fn subscribe_fresh(
    store: &mut Store,
    window: crate::WindowId,
    session: &crate::higent::SessionUri,
    entries: Vec<CatalogEntry>,
    fx: &mut crate::AppFx<'_>,
) {
    let mut fresh = Vec::new();
    store.update::<Changes>(|changes| {
        fresh = changes.adopt_catalog(session, entries.clone());
    });

    crate::hihistory::subscribe_fresh(store, window, session, &entries, fx);
    for (folder, seat, channel) in fresh {
        let landing = folder.clone();
        let scope = folder_scope(&folder);
        fx.push(
            AnyEffect::new(SubscribeChangesetEffect { seat, channel }).map(move |result| {
                let landed = Arc::new(SnapshotLanded {
                    folder: landing.clone(),
                    result,
                });
                match scope.clone() {
                    Some(scope) => AppCommand::dynamic_in(scope, window, landed),
                    None => AppCommand::Dynamic(window, landed),
                }
            }),
        );
    }
}

pub(crate) fn folder_scope(folder: &ResourceLocation) -> Option<crate::SessionId> {
    let (host, session) = crate::higent::seat::parse(folder.authority().as_str())?;
    Some(crate::SessionId { host, session })
}

fn relaunch_session_poll(
    store: &Store,
    window: crate::WindowId,
    session: &crate::higent::SessionUri,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(feed) = store
        .get::<Changes>()
        .and_then(|changes| changes.feed_for(session).cloned())
    else {
        return;
    };
    let landing = session.to_owned();

    let scope = crate::Gathered::scope(store).cloned();
    fx.push(
        AnyEffect::new(crate::higent::PollSessionEffect {
            seat: feed.seat,
            session: session.to_owned(),
        })
        .map(move |actions| {
            let polled = Arc::new(SessionPolled {
                session: landing.clone(),
                actions,
            });
            match scope.clone() {
                Some(scope) => AppCommand::dynamic_in(scope, window, polled),
                None => AppCommand::Dynamic(window, polled),
            }
        }),
    );
}

struct SnapshotLanded {
    folder: ResourceLocation,
    result: Result<ChangesetState, String>,
}

impl crate::DynamicCommand for SnapshotLanded {
    fn id(&self) -> &'static str {
        "changes.snapshot-landed"
    }
    fn name(&self) -> String {
        "Changes Snapshot".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        store.update::<Changes>(|changes| match &self.result {
            Ok(state) => changes.adopt(&self.folder, state),
            Err(error) => changes.adopt_error(&self.folder, error.clone()),
        });
        rearm_stripes(store, &_app.ui_ctx(), &self.folder, fx);
        if self.result.is_ok() {
            relaunch_poll(store, window, &self.folder, fx);
        }
    }
}

struct Polled {
    folder: ResourceLocation,
    actions: Vec<StateAction>,
}

impl crate::DynamicCommand for Polled {
    fn id(&self) -> &'static str {
        "changes.polled"
    }
    fn name(&self) -> String {
        "Changes Update".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        store.update::<Changes>(|changes| changes.fold(&self.folder, &self.actions));
        rearm_stripes(store, &_app.ui_ctx(), &self.folder, fx);
        relaunch_poll(store, window, &self.folder, fx);
    }
}

fn rearm_stripes(
    store: &mut Store,
    ui: &UiCtx,
    folder: &ResourceLocation,
    fx: &mut crate::AppFx<'_>,
) {
    let authority = folder.authority().clone();
    let prefix = format!("/{}/", folder.path().join("/"));
    crate::rearm_base_asks(store, &|location| {
        location.authority() == &authority
            && format!("/{}", location.path().join("/")).starts_with(&prefix)
    });
    crate::sync_stripe_bases(store, ui, fx);
}

fn relaunch_poll(
    store: &Store,
    window: crate::WindowId,
    folder: &ResourceLocation,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(entry) = Changes::folder(store, folder) else {
        return;
    };
    let Some(feed) = entry.feed else {
        return;
    };
    let Some(channel) = feed.channel else {
        return;
    };
    let landing = folder.clone();
    let scope = folder_scope(folder);
    fx.push(
        AnyEffect::new(PollChangesetEffect {
            seat: feed.seat,
            channel,
        })
        .map(move |actions| {
            let polled = Arc::new(Polled {
                folder: landing.clone(),
                actions,
            });
            match scope.clone() {
                Some(scope) => AppCommand::dynamic_in(scope, window, polled),
                None => AppCommand::Dynamic(window, polled),
            }
        }),
    );
}

pub struct OpenDiffForPair {
    pub old: ResourceLocation,
    pub new: ResourceLocation,
}

impl crate::DynamicCommand for OpenDiffForPair {
    fn id(&self) -> &'static str {
        "changes.open-diff"
    }
    fn name(&self) -> String {
        "Open Diff".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        // Resolve both sides on the UI thread — an open side hands over
        // its live registry snapshot, so the diff is against the live
        // buffer and rebases if it moves (docs/no-diff-on-ui-thread).
        let old = crate::DiffSideInput::resolve(store, self.old.clone());
        let new = crate::DiffSideInput::resolve(store, self.new.clone());
        let _ = fx.push(AnyEffect::new(crate::OpenDiffByLocationsEffect {
            window,
            old,
            new,
        }));
    }
}

#[derive(Clone)]
enum RowItem {
    Branch,

    File { new: ResourceLocation },

    Note,
}

#[derive(Default)]
pub(crate) struct DirTrie {
    dirs: BTreeMap<String, DirTrie>,
    files: Vec<ChangeEntry>,
}

impl DirTrie {
    pub(crate) fn insert(&mut self, entry: ChangeEntry) {
        let mut node = self;
        for segment in &entry.rel[..entry.rel.len() - 1] {
            node = node.dirs.entry(segment.clone()).or_default();
        }
        node.files.push(entry);
    }
}

fn folder_node(
    folder: &ResourceLocation,
    changes: Option<&FolderChanges>,
    items: &mut rpds::HashTrieMapSync<ResourceLocation, RowItem>,
    counts: (skia_safe::Color, skia_safe::Color),
) -> ForestNode<ResourceLocation> {
    items.insert_mut(folder.clone(), RowItem::Branch);
    let note = |text: &str, items: &mut rpds::HashTrieMapSync<ResourceLocation, RowItem>| {
        let key = folder.child(ResourceType::new(NOTE_KIND), text);
        items.insert_mut(key.clone(), RowItem::Note);
        vec![ForestNode {
            key,
            label: text.to_owned(),
            pick: false,
            dim: true,
            trail: Vec::new(),
            tint: crate::TreeTint::Label,
            action: None,
            children: Vec::new(),
        }]
    };
    let children = match changes {
        None => note("no changes source", items),
        Some(changes) => match (&changes.status, changes.files.is_empty()) {
            (ChangesStatus::Error(message), _) => note(message, items),
            (ChangesStatus::Computing, true) => note("computing…", items),
            (ChangesStatus::Ready, true) => note("no changes", items),
            _ => {
                let mut trie = DirTrie::default();
                for entry in changes.files.iter() {
                    trie.insert(entry.clone());
                }
                struct Sink<'a> {
                    items: &'a mut rpds::HashTrieMapSync<ResourceLocation, RowItem>,
                }
                impl DirSink for Sink<'_> {
                    fn branch(&mut self, key: &ResourceLocation) {
                        self.items.insert_mut(key.clone(), RowItem::Branch);
                    }

                    fn file_key(
                        &self,
                        entry: &ChangeEntry,
                        _at: &ResourceLocation,
                    ) -> ResourceLocation {
                        entry.working.clone()
                    }
                    fn file(&mut self, entry: &ChangeEntry, key: &ResourceLocation) {
                        self.items.insert_mut(
                            key.clone(),
                            RowItem::File {
                                new: entry.working.clone(),
                            },
                        );
                    }
                }
                dir_forest(folder, trie, counts, &mut Sink { items })
            }
        },
    };
    ForestNode {
        key: folder.clone(),
        label: folder.name().to_owned(),
        pick: false,
        dim: false,
        trail: Vec::new(),
        tint: crate::TreeTint::Directory,
        // Refetch is PER REPOSITORY — the chip rides its root row.
        action: Some("REFRESH".to_owned()),
        children,
    }
}

pub(crate) trait DirSink {
    fn branch(&mut self, key: &ResourceLocation);
    fn file_key(&self, entry: &ChangeEntry, at: &ResourceLocation) -> ResourceLocation;
    fn file(&mut self, entry: &ChangeEntry, key: &ResourceLocation);
}

pub(crate) fn dir_forest(
    at: &ResourceLocation,
    trie: DirTrie,
    counts: (skia_safe::Color, skia_safe::Color),
    sink: &mut dyn DirSink,
) -> Vec<ForestNode<ResourceLocation>> {
    let mut children = Vec::new();
    for (name, sub) in trie.dirs {
        let mut label = name.clone();
        let mut location = at.child(ResourceType::directory(), &name);
        let mut sub = sub;
        while sub.files.is_empty() && sub.dirs.len() == 1 {
            let (name, inner) = sub.dirs.into_iter().next().expect("the single child");
            label.push('/');
            label.push_str(&name);
            location = location.child(ResourceType::directory(), &name);
            sub = inner;
        }
        sink.branch(&location);
        let nested = dir_forest(&location, sub, counts, sink);
        children.push(ForestNode {
            key: location,
            label,
            pick: false,
            dim: false,
            trail: Vec::new(),
            tint: crate::TreeTint::Directory,
            action: None,
            children: nested,
        });
    }
    let mut files = trie.files;
    files.sort_by(|a, b| a.working.name().cmp(b.working.name()));
    for entry in files {
        let label = entry.working.name().to_owned();

        let mut trail = Vec::new();
        if entry.added.is_some() || entry.removed.is_some() {
            trail.push((format!("+{}", entry.added.unwrap_or(0)), counts.0));
            trail.push((format!("−{}", entry.removed.unwrap_or(0)), counts.1));
        }
        let key = sink.file_key(&entry, at);
        sink.file(&entry, &key);
        children.push(ForestNode {
            key,
            label,
            pick: true,
            dim: false,
            trail,
            tint: crate::TreeTint::File,
            action: None,
            children: Vec::new(),
        });
    }
    children
}

type Rows = ListKeyboardController<ForestList<ResourceLocation>, ForestSearcher<ResourceLocation>>;

pub enum ChangesCommand {
    Rows(ListKeyCommand<TreeListCommand>),

    Refetch(ResourceLocation),


    Dismiss,
}

pub struct ChangesView {
    list: Rows,
    items: rpds::HashTrieMapSync<ResourceLocation, RowItem>,

    workspace: crate::SessionId,

    window: crate::WindowId,

    seen: u64,
    request: Option<ModalRequest>,
}

impl Clone for ChangesView {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            items: self.items.clone(),
            workspace: self.workspace.clone(),
            window: self.window,
            seen: self.seen,

            request: None,
        }
    }
}

impl ChangesView {
    pub fn open(
        store: &Store,
        ui: &UiCtx,
        window: crate::WindowId,
        workspace: crate::SessionId,
    ) -> Self {
        let mut panel = Self {
            list: ListKeyboardController::searchable(
                ForestList::new(store),
                ForestSearcher::default(),
                store,
                ui,
                crate::env::Fonts::of(store),
            )
            .with_folds(),
            items: rpds::HashTrieMapSync::new_sync(),
            workspace,
            window,
            seen: 0,
            request: None,
        };
        panel.refresh(store, ui);
        panel
    }

    pub fn row_count(&self) -> usize {
        self.list.inner().list().len()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rows(&self) -> Vec<(u8, String, bool)> {
        self.list.inner().forest.rows_trailed()
    }

    fn refresh(&mut self, store: &Store, ui: &UiCtx) {
        self.seen = Changes::generation(store);
        let mut items = rpds::HashTrieMapSync::new_sync();
        let chat = crate::env::Themes::of(store).ui().chat.clone();
        let counts = (chat.added_color.0, chat.removed_color.0);
        let nodes: Vec<ForestNode<ResourceLocation>> =
            crate::higent::session_folders(store, &self.workspace)
                .iter()
                .map(|folder| {
                    folder_node(
                        folder,
                        Changes::folder(store, folder).as_ref(),
                        &mut items,
                        counts,
                    )
                })
                .collect();
        self.items = items;
        self.list.inner_mut().set(&nodes, store, ui);
    }

    pub fn activate(&mut self, index: usize, store: &Store, ui: &UiCtx) {
        let Some(key) = self.list.inner().list().key_at(index).cloned() else {
            return;
        };
        self.activate_key(&key, store, ui);
    }

    fn activate_key(&mut self, key: &ResourceLocation, store: &Store, ui: &UiCtx) {
        match self.items.get(key).cloned() {
            Some(RowItem::Branch) => {
                // The workspace folder ROOT opens the diff canvas
                // (docs/editor/diff-canvas.md §6); inner directories keep
                // toggling. The chevron expands either way.
                if crate::higent::session_folders(store, &self.workspace).contains(key) {
                    self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                        self.window,
                        Arc::new(crate::diff_canvas::OpenDiffCanvas {
                            source: crate::diff_canvas::CanvasSource::WorkingCopy {
                                folder: key.clone(),
                            },
                            reveal: None,
                        }),
                    )));
                    return;
                }
                self.list.inner_mut().toggle(key, store, ui)
            }
            Some(RowItem::File { new }) => {
                self.list.inner_mut().list_mut().select_only(key.clone());

                // A file row REVEALS itself in the folder's canvas —
                // the canvas row keys are these same locations.
                let folder = crate::higent::session_folders(store, &self.workspace)
                    .into_iter()
                    .find(|folder| {
                        folder.authority() == new.authority()
                            && new.path().starts_with(folder.path())
                    });
                let Some(folder) = folder else {
                    return;
                };
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(crate::diff_canvas::OpenDiffCanvas {
                        source: crate::diff_canvas::CanvasSource::WorkingCopy { folder },
                        reveal: Some(new),
                    }),
                )));
            }
            Some(RowItem::Note) | None => {}
        }
    }
}

impl View for ChangesView {
    type Command = ChangesCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ChangesCommand> {
        use imba::focus::FocusData;
        // The key table is the controller's; the surface keeps only
        // its own dismissal.
        let searching = self.list.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => EventResult::Command(ChangesCommand::Dismiss),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(self.list.focus_data(store, ui).map(ChangesCommand::Rows))
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        // Teardown-only: `View::destroy` carries no UiCtx.
        let ui = &imba::UiCtx::dont_use_too_slow();
        fx.scope(ChangesCommand::Rows, |fx| self.list.clear(store, ui, fx));
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            ChangesCommand::Rows(command) => {
                match &command {
                    ListKeyCommand::Fold { expand, .. } => {
                        return self.list.inner_mut().fold_cursor(*expand, store, ui);
                    }
                    ListKeyCommand::Inner(inner) => {
                        if let Some(index) = crate::tree_action(inner) {
                            if let Some(folder) = self.list.inner().list().key_at(index).cloned() {
                                return self.perform(
                                    store,
                                    ui,
                                    ChangesCommand::Refetch(folder),
                                    fx,
                                );
                            }
                        }
                        if let Some(index) = crate::tree_toggle(inner) {
                            return self.activate(index, store, ui);
                        }
                    }
                    _ => {}
                }
                if let Some((index, trigger)) = Rows::activated(&command) {
                    let searching = self.list.searching();
                    self.activate(index, store, ui);
                    match trigger {
                        // The deliberate pick ends the search in the
                        // same stroke.
                        ActivateTrigger::Enter if searching => {
                            return self.perform(
                                store,
                                ui,
                                ChangesCommand::Rows(ListKeyCommand::Clear),
                                fx,
                            );
                        }
                        ActivateTrigger::Enter | ActivateTrigger::Click => {}
                    }
                }
                fx.scope(ChangesCommand::Rows, |fx| {
                    self.list.perform(store, ui, command, fx)
                });
            }

            ChangesCommand::Refetch(folder) => {
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(RefetchChanges {
                        folder: Some(folder),
                    }),
                )));
            }
            ChangesCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let mut overlay = container(arena, size);

            // The commit composer lives in the diff canvas's first
            // row and REFRESH rides each repository's root row — the
            // dock is just the tree.
            let band = PANEL_PAD;
            let rows = imba::Layout::layout(
                self.list.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(size.width, size.height - band)),
            )
            .map(ChangesCommand::Rows);
            overlay.place(0.0, band, rows);

            // The key table lives in the controller's own overlay;
            // the surface keeps only its dismissal.
            let searching = self.list.searching();
            let keymap = leaf::<ChangesCommand>(size.width, size.height).event(
                move |_arena, event, _size| match event {
                    Event::KeyDown {
                        key: InputKey::Escape,
                        ..
                    } if !searching => EventResult::Command(ChangesCommand::Dismiss),
                    _ => EventResult::Ignored,
                },
            );
            overlay.place(0.0, 0.0, keymap);
            overlay
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChangesViewId(u64);

impl ChangesViewId {
    fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

impl Changes {
    fn mint_view(store: &mut Store, view: ChangesView) -> ChangesViewId {
        let id = ChangesViewId::mint();
        store.update::<Changes>(|changes| {
            changes.views.insert_mut(id, view);
        });
        id
    }

    pub fn view_ref(store: &Store, id: ChangesViewId) -> Option<&ChangesView> {
        store.get::<Changes>()?.views.get(&id)
    }

    fn take_view(store: &mut Store, id: ChangesViewId) -> Option<ChangesView> {
        let view = Self::view_ref(store, id)?.clone();
        store.update::<Changes>(|changes| {
            changes.views.remove_mut(&id);
        });
        Some(view)
    }

    fn put_view(store: &mut Store, id: ChangesViewId, view: ChangesView) {
        store.update::<Changes>(|changes| {
            changes.views.insert_mut(id, view);
        });
    }

    fn remove_view(store: &mut Store, id: ChangesViewId) {
        store.update::<Changes>(|changes| {
            changes.views.remove_mut(&id);
        });
    }

    fn view_ids(store: &Store) -> Vec<ChangesViewId> {
        store
            .get::<Changes>()
            .map(|changes| changes.views.keys().copied().collect())
            .unwrap_or_default()
    }
}

/// The dock's REFERENCE view over a store-held changes record — holds
/// only the id (and the request it pulled out, `take_request` having
/// no store). Its death removes the record: tree rows are pure
/// derivation, nothing is lost on close.
pub struct ChangesPane {
    view: ChangesViewId,
    request: Option<ModalRequest>,
}

impl ChangesPane {
    pub fn view(&self) -> ChangesViewId {
        self.view
    }
}

impl Clone for ChangesPane {
    fn clone(&self) -> Self {
        Self {
            view: self.view,
            request: None,
        }
    }
}

impl View for ChangesPane {
    type Command = ChangesCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ChangesCommand> {
        match Changes::view_ref(store, self.view) {
            Some(view) => view.focus_data(store, ui),
            None => imba::focus::FocusData::default(),
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        let Some(mut view) = Changes::take_view(store, self.view) else {
            return;
        };
        view.perform(store, ui, command, fx);
        // Requests are the PANE's ask (`take_request` has no store):
        // pull what the record minted into the reference view.
        if let Some(request) = view.request.take() {
            self.request = Some(request);
        }
        Changes::put_view(store, self.view, view);
    }

    fn destroy(
        &mut self,
        store: &mut Store,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        Changes::remove_view(store, self.view);
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a Arena, constraints: imba::constraints::Constraints| {
                let widget: imba::ThunkBox<'a, ChangesCommand> =
                    match Changes::view_ref(store, self.view) {
                        Some(view) => imba::ThunkBox::new(
                            arena,
                            imba::Layout::layout(view.display(arena, store, ui), arena, constraints),
                        ),
                        None => imba::ThunkBox::new(
                            arena,
                            leaf(constraints.max.width, constraints.max.height),
                        ),
                    };
                widget
            },
        )
    }
}

impl ModalView for ChangesPane {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The PUSH lane for the changes dock: at the batch tail, refresh any
/// window's mounted `ChangesView` whose feed generation moved — the
/// same batch the feed landed in, no paint probe. Scope-guarded: only
/// a view of the GATHERED session compares against its generation.
pub(crate) fn sync_changes_docks(store: &mut Store, ui: &UiCtx) {
    let Some(scope) = crate::Gathered::scope(store).cloned() else {
        return;
    };
    let generation = Changes::generation(store);
    for id in Changes::view_ids(store) {
        let stale = Changes::view_ref(store, id)
            .is_some_and(|view| view.workspace == scope && view.seen != generation);
        if !stale {
            continue;
        }
        if let Some(mut view) = Changes::take_view(store, id) {
            view.refresh(store, ui);
            Changes::put_view(store, id, view);
        }
    }
}

pub struct ToggleChangesView;

impl crate::DynamicCommand for ToggleChangesView {
    fn id(&self) -> &'static str {
        "changes.view"
    }
    fn name(&self) -> String {
        "Changes".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            crate::Windows::put(store, window, entity);
            return;
        }
        let workspace = entity.current_session();
        Changes::ensure(store, window, workspace.clone(), fx);

        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let view = Changes::mint_view(store, ChangesView::open(store, &_app.ui_ctx(), window, workspace));
        let owner = self.id();
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| {
                entity.show_dock(
                    store,
                    Box::new(ChangesPane {
                        view,
                        request: None,
                    }),
                    owner,
                    fx,
                )
            },
        );
        crate::Windows::put(store, window, entity);
    }
}

/// Refetch changesets — one repository's when `folder` names it, every
/// riding folder's otherwise (the palette / test road).
#[derive(Default)]
pub struct RefetchChanges {
    pub folder: Option<ResourceLocation>,
}

impl crate::DynamicCommand for RefetchChanges {
    fn id(&self) -> &'static str {
        "changes.refetch"
    }
    fn name(&self) -> String {
        "Refresh Changes".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        Changes::refetch(store, window, fx, self.folder.as_ref());
    }
}

pub fn toolbar_button() -> crate::ToolbarButton {
    crate::ToolbarButton {
        command: "changes.view",
        order: 1.0,
        side: crate::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());
            let mut path = skia_safe::PathBuilder::new();

            let (px, py, arm) = (l + w * 0.32, t + h * 0.32, w * 0.17);
            path.move_to((px - arm, py));
            path.line_to((px + arm, py));
            path.move_to((px, py - arm));
            path.line_to((px, py + arm));

            let (mx, my) = (l + w * 0.68, t + h * 0.74);
            path.move_to((mx - arm, my));
            path.line_to((mx + arm, my));
            canvas.draw_path(&path.detach(), &paint);
        }),
    }
}

#[cfg(test)]
mod tests;
