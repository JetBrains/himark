// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::higent::ahp_types::actions::StateAction;
use crate::higent::ahp_types::state::{ChangesetFile, ChangesetState, ChangesetStatus};
use crate::higent::{AhpServer, PollChangesetEffect, SubscribeChangesetEffect};
use crate::{AppCommand, Authority, ForestNode, ResourceLocation, ResourceType};
use himark_ahp_ext_types::history as history_wire;
use imba::{effect::AnyEffect, store::Store, UiCtx};

const NOTE_KIND: &str = "changes-note";

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

/// A commit's identity in its repository — the revision/sha string
/// off the wire, wrapped so it cannot be confused with the paths,
/// uris and messages it travels beside.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct Revision(String);

impl Revision {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for Revision {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}

impl From<String> for Revision {
    fn from(raw: String) -> Self {
        Self(raw)
    }
}

impl From<&str> for Revision {
    fn from(raw: &str) -> Self {
        Self(raw.to_owned())
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
        revision: Revision,
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
    pub(crate) canvases: rpds::HashTrieMapSync<
        crate::diff_canvas::canvas::CanvasId,
        crate::diff_canvas::canvas::Canvas,
    >,
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

    /// The unified tree VIEWS over this collection's sets — one per
    /// mounted dock, changes- or history-flavored only by the data
    /// they derive rows from (crate::changes_view). The collection
    /// is the views' point of gravity: a uniting view spans many
    /// sets, so the views cannot live inside one.
    pub(crate) views:
        rpds::HashTrieMapSync<crate::changes_view::ChangesViewId, crate::changes_view::ChangesView>,

    /// THE UPDATE RULE's index — the many-to-many `ChangeSetId ↔
    /// ChangesViewId` join: a set mutation marks exactly its viewers
    /// stale; the batch-tail lane rolls them.
    pub(crate) viewers: rpds::HashTrieMapSync<
        ChangeSetId,
        rpds::HashTrieSetSync<crate::changes_view::ChangesViewId>,
    >,

    pub(crate) stale: rpds::HashTrieSetSync<crate::changes_view::ChangesViewId>,
}

/// Transitional alias — the store slot and the wide call-site surface
/// named the collection `Changes`.
pub type Changes = ChangeSets;

impl Changes {
    /// The change sets of one session, addressed by it — the family is
    /// never gathered as a component, so nothing can reach the wrong
    /// session's sets or lose a write to a scopeless batch.
    pub(crate) fn of<'a>(store: &'a Store, home: &crate::SessionId) -> Option<&'a ChangeSets> {
        crate::higent::Hosts::family(store, home).map(|family| &family.changes)
    }

    pub(crate) fn update(
        store: &mut Store,
        home: &crate::SessionId,
        mutate: impl FnOnce(&mut ChangeSets),
    ) {
        crate::higent::Hosts::update_family(store, home, |family| mutate(&mut family.changes));
    }

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
        true
    }

    /// Mint — or find — the COMMIT-flavored set (eager and light: the
    /// history row references it from birth; its files land lazily on
    /// first ask). Never bumps the commit set's generation.
    pub(crate) fn ensure_commit_set(
        store: &mut Store,
        home: &crate::SessionId,
        folder: &ResourceLocation,
        revision: &Revision,
        seat: &Arc<dyn AhpServer>,
        session: &crate::higent::SessionUri,
    ) -> ChangeSetId {
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        if let Some(id) =
            Self::of(store, home).and_then(|changes| changes.by_source.get(&source).copied())
        {
            return id;
        }
        let id = ChangeSetId::mint();
        let seat = seat.clone();
        let session = session.to_owned();
        Self::update(store, home, |changes| {
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
        home: &crate::SessionId,
        folder: &ResourceLocation,
        revision: &Revision,
    ) -> Option<ChangeSet> {
        let changes = Self::of(store, home)?;
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        changes.sets.get(changes.by_source.get(&source)?).cloned()
    }

    /// The per-SET staleness cue for a commit canvas.
    pub fn commit_generation(
        store: &Store,
        home: &crate::SessionId,
        folder: &ResourceLocation,
        revision: &Revision,
    ) -> u64 {
        Self::commit_set(store, home, folder, revision)
            .map(|set| set.generation)
            .unwrap_or(0)
    }

    /// Mint — or find — the set for a source, WITHOUT a feed when the
    /// feeds have not routed it yet (a canvas may open first; the
    /// feed attaches at `ensure_folder`). Never bumps generations.
    pub(crate) fn ensure_set_for_source(
        store: &mut Store,
        home: &crate::SessionId,
        source: &ChangeSetSource,
    ) -> ChangeSetId {
        if let Some(id) =
            Self::of(store, home).and_then(|changes| changes.by_source.get(source).copied())
        {
            return id;
        }
        let id = ChangeSetId::mint();
        let source = source.clone();
        Self::update(store, home, |changes| {
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
        home: &crate::SessionId,
        source: &ChangeSetSource,
    ) -> Option<ChangeSetId> {
        Self::of(store, home)?.by_source.get(source).copied()
    }

    /// The SET owns its canvases; these reach one by (set, canvas) —
    /// canvas mutations never touch the set's generation.
    pub(crate) fn canvas_ref<'a>(
        store: &'a Store,
        home: &crate::SessionId,
        set: ChangeSetId,
        canvas: crate::diff_canvas::canvas::CanvasId,
    ) -> Option<&'a crate::diff_canvas::canvas::Canvas> {
        Self::of(store, home)?.sets.get(&set)?.canvases.get(&canvas)
    }

    pub(crate) fn take_canvas(
        store: &mut Store,
        home: &crate::SessionId,
        set: ChangeSetId,
        canvas: crate::diff_canvas::canvas::CanvasId,
    ) -> Option<crate::diff_canvas::canvas::Canvas> {
        let held = Self::canvas_ref(store, home, set, canvas)?.clone();
        Self::update(store, home, |changes| {
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
        home: &crate::SessionId,
        set: ChangeSetId,
        canvas: crate::diff_canvas::canvas::CanvasId,
        held: crate::diff_canvas::canvas::Canvas,
    ) {
        Self::update(store, home, |changes| {
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
        home: &crate::SessionId,
    ) -> Vec<(ChangeSetId, crate::diff_canvas::canvas::CanvasId)> {
        Self::of(store, home)
            .map(|changes| {
                changes
                    .sets
                    .iter()
                    .flat_map(|(set, held)| held.canvases.keys().map(|canvas| (*set, *canvas)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The fetch road armed: the set is COMPUTING until its snapshot
    /// lands.
    pub(crate) fn mark_commit_computing(
        store: &mut Store,
        home: &crate::SessionId,
        folder: &ResourceLocation,
        revision: &Revision,
    ) {
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        Self::update(store, home, |changes| {
            changes.update_set_by_source(&source, |set| {
                set.status = ChangesStatus::Computing;
                set.files = rpds::VectorSync::new_sync();
            });
        });
        if let Some(id) = Self::id_for_source(store, home, &source) {
            Self::nudge_set(store, home, id);
        }
    }

    /// A commit set's content snapshot landed (the changeset channel
    /// for `?commit=<sha>`): status + files onto the SET.
    pub(crate) fn adopt_commit_state(
        store: &mut Store,
        home: &crate::SessionId,
        folder: &ResourceLocation,
        revision: &Revision,
        result: &Result<ChangesetState, String>,
    ) {
        let Some(uris) = Self::of(store, home).and_then(|changes| changes.uris.clone()) else {
            return;
        };
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        Self::update(store, home, |changes| {
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
        if let Some(id) = Self::id_for_source(store, home, &source) {
            Self::nudge_set(store, home, id);
        }
    }

    /// Streamed updates for a commit set's changeset channel.
    pub(crate) fn fold_commit_actions(
        store: &mut Store,
        home: &crate::SessionId,
        folder: &ResourceLocation,
        revision: &Revision,
        actions: &[StateAction],
    ) {
        let Some(uris) = Self::of(store, home).and_then(|changes| changes.uris.clone()) else {
            return;
        };
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        Self::update(store, home, |changes| {
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
        if let Some(id) = Self::id_for_source(store, home, &source) {
            Self::nudge_set(store, home, id);
        }
    }

    pub fn set_ref<'a>(
        store: &'a Store,
        home: &crate::SessionId,
        id: ChangeSetId,
    ) -> Option<&'a ChangeSet> {
        Self::of(store, home)?.sets.get(&id)
    }

    pub fn id_for_folder(
        store: &Store,
        home: &crate::SessionId,
        folder: &ResourceLocation,
    ) -> Option<ChangeSetId> {
        let source = ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        };
        Self::of(store, home)?.by_source.get(&source).copied()
    }

    /// The per-SET staleness cue for a working copy — 0 while absent
    /// (the canvas keeps probing until the set exists).
    pub fn folder_generation(
        store: &Store,
        home: &crate::SessionId,
        folder: &ResourceLocation,
    ) -> u64 {
        Self::of(store, home)
            .and_then(|changes| changes.folder_set(folder))
            .map(|set| set.generation)
            .unwrap_or(0)
    }

    pub fn folder(
        store: &Store,
        home: &crate::SessionId,
        folder: &ResourceLocation,
    ) -> Option<ChangeSet> {
        Self::of(store, home)?.folder_set(folder).cloned()
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
        // The folder's own authority names the session that serves it:
        // that session's family is where its change set lives.
        let Some((host, seat, session)) =
            crate::higent::seat::route_seat(store, folder.authority().as_str())
        else {
            return;
        };
        let scope = crate::SessionId {
            host,
            session: session.clone(),
        };
        let known = Self::of(store, &scope).is_some_and(|changes| {
            changes
                .folder_set(&folder)
                .is_some_and(|set| set.feed.is_some())
        });
        if known {
            return;
        }
        let detached = Self::of(store, &scope).is_some_and(|changes| {
            changes
                .by_source
                .contains_key(&ChangeSetSource::WorkingCopy {
                    folder: folder.clone(),
                })
        });
        let Some(uris) = crate::higent::Hosts::uris(store, host) else {
            return;
        };
        Self::update(store, &scope, |changes| changes.uris = Some(uris.clone()));
        Self::update(store, &scope, |changes| {
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
        });
        crate::hihistory::History::ensure_folder(store, &scope, &folder, &seat);
        Changes::nudge_folder(store, &scope, &folder);

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
        let feed_known =
            Self::of(store, &scope).is_some_and(|changes| changes.feed_for(&session).is_some());
        if !feed_known {
            Self::update(store, &scope, |changes| {
                changes.session = Some(SessionFeed {
                    uri: session.clone(),
                    seat: seat.clone(),
                    catalog: rpds::VectorSync::new_sync(),
                });
            });
            let landing = scope.clone();
            fx.push(
                AnyEffect::new(crate::higent::SubscribeSessionEffect { seat, session }).map(
                    move |result| {
                        AppCommand::dynamic_in(
                            scope.clone(),
                            window,
                            Arc::new(SessionLanded {
                                home: landing.clone(),
                                result,
                            }),
                        )
                    },
                ),
            );
        }
    }

    pub fn script_summary(store: &Store, home: &crate::SessionId) -> Option<String> {
        let changes = Self::of(store, home)?;
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
        home: &crate::SessionId,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
        only: Option<&ResourceLocation>,
    ) {
        let Some(changes) = Self::of(store, home) else {
            return;
        };
        let riding: Vec<(
            ResourceLocation,
            Arc<dyn AhpServer>,
            crate::higent::ChannelUri,
        )> = changes
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
        Self::update(store, home, |changes| {
            for (folder, _, _) in &riding {
                changes.update_folder_set(folder, |set| {
                    set.status = ChangesStatus::Computing;
                });
            }
        });
        for (folder, _, _) in &riding {
            Changes::nudge_folder(store, home, folder);
        }
        for (folder, seat, channel) in riding {
            let landing = folder.clone();
            let scope = home.clone();
            fx.push(
                AnyEffect::new(SubscribeChangesetEffect { seat, channel }).map(move |result| {
                    let landed = Arc::new(SnapshotLanded {
                        home: scope.clone(),
                        folder: landing.clone(),
                        result,
                    });
                    AppCommand::dynamic_in(scope.clone(), window, landed)
                }),
            );
        }
    }

    fn adopt_catalog(
        &mut self,
        session: &crate::higent::SessionUri,
        entries: Vec<CatalogEntry>,
    ) -> Vec<(
        ResourceLocation,
        Arc<dyn AhpServer>,
        crate::higent::ChannelUri,
    )> {
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
            let mut files: Vec<Option<ChangeEntry>> =
                entry.files.iter().cloned().map(Some).collect();
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
                            .filter_map(|(at, slot)| {
                                slot.as_ref().map(|file| (file.id.clone(), at))
                            })
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
    pub fn base_ref(
        store: &Store,
        home: &crate::SessionId,
        abs_path: &str,
    ) -> Option<ResourceLocation> {
        Self::of(store, home)?
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
    home: crate::SessionId,
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
                subscribe_fresh(store, window, &self.home, entries, fx);
                relaunch_session_poll(store, window, &self.home, fx);
            }
            Err(error) => {
                eprintln!("[hichanges] session subscribe failed: {error}");
                let session = self.home.session.clone();
                Changes::update(store, &self.home, |changes| {
                    changes.session_failed(&session, error);
                });
                Changes::nudge_all(store, &self.home);
                crate::hihistory::History::session_failed(store, &self.home, error);
            }
        }
    }
}

struct SessionPolled {
    home: crate::SessionId,
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
        // This poller and higent's drain the SAME wire feed; the
        // winner takes the whole batch, so hand every action kind
        // to the shared application, not just the changeset ones.
        crate::higent::apply_channel_actions(store, window, &self.home, &self.actions, fx);
        relaunch_session_poll(store, window, &self.home, fx);
    }
}

/// The session-channel catalog action, routed here from whichever
/// poller drained it.
pub(crate) fn adopt_session_catalog(
    store: &mut Store,
    window: crate::WindowId,
    home: &crate::SessionId,
    changed: &crate::higent::ahp_types::actions::SessionChangesetsChangedAction,
    fx: &mut crate::AppFx<'_>,
) {
    let entries = digest_catalog(changed.changesets.as_deref().unwrap_or_default());
    subscribe_fresh(store, window, home, entries, fx);
}

fn subscribe_fresh(
    store: &mut Store,
    window: crate::WindowId,
    home: &crate::SessionId,
    entries: Vec<CatalogEntry>,
    fx: &mut crate::AppFx<'_>,
) {
    let session = home.session.clone();
    let mut fresh = Vec::new();
    Changes::update(store, home, |changes| {
        fresh = changes.adopt_catalog(&session, entries.clone());
    });
    Changes::nudge_all(store, home);

    crate::hihistory::subscribe_fresh(store, window, home, &entries, fx);
    for (folder, seat, channel) in fresh {
        let landing = folder.clone();
        let scope = home.clone();
        fx.push(
            AnyEffect::new(SubscribeChangesetEffect { seat, channel }).map(move |result| {
                let landed = Arc::new(SnapshotLanded {
                    home: scope.clone(),
                    folder: landing.clone(),
                    result,
                });
                match Some(scope.clone()) {
                    Some(scope) => AppCommand::dynamic_in(scope, window, landed),
                    None => AppCommand::Dynamic(window, landed),
                }
            }),
        );
    }
}

fn relaunch_session_poll(
    store: &Store,
    window: crate::WindowId,
    home: &crate::SessionId,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(feed) =
        Changes::of(store, home).and_then(|changes| changes.feed_for(&home.session).cloned())
    else {
        return;
    };
    let landing = home.clone();
    let scope = home.clone();
    fx.push(
        AnyEffect::new(crate::higent::PollSessionEffect {
            seat: feed.seat,
            session: home.session.clone(),
        })
        .map(move |actions| {
            AppCommand::dynamic_in(
                scope.clone(),
                window,
                Arc::new(SessionPolled {
                    home: landing.clone(),
                    actions,
                }),
            )
        }),
    );
}

struct SnapshotLanded {
    home: crate::SessionId,
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
        Changes::update(store, &self.home, |changes| match &self.result {
            Ok(state) => changes.adopt(&self.folder, state),
            Err(error) => changes.adopt_error(&self.folder, error.clone()),
        });
        Changes::nudge_folder(store, &self.home, &self.folder);
        rearm_stripes(store, &_app.ui_ctx(), &self.folder, fx);
        if self.result.is_ok() {
            relaunch_poll(store, window, &self.home, &self.folder, fx);
        }
    }
}

struct Polled {
    home: crate::SessionId,
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
        Changes::update(store, &self.home, |changes| {
            changes.fold(&self.folder, &self.actions)
        });
        Changes::nudge_folder(store, &self.home, &self.folder);
        rearm_stripes(store, &_app.ui_ctx(), &self.folder, fx);
        relaunch_poll(store, window, &self.home, &self.folder, fx);
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
    home: &crate::SessionId,
    folder: &ResourceLocation,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(entry) = Changes::folder(store, home, folder) else {
        return;
    };
    let Some(feed) = entry.feed else {
        return;
    };
    let Some(channel) = feed.channel else {
        return;
    };
    let landing = folder.clone();
    let scope = home.clone();
    fx.push(
        AnyEffect::new(PollChangesetEffect {
            seat: feed.seat,
            channel,
        })
        .map(move |actions| {
            AppCommand::dynamic_in(
                scope.clone(),
                window,
                Arc::new(Polled {
                    home: scope.clone(),
                    folder: landing.clone(),
                    actions,
                }),
            )
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

use crate::changes_view::RowItem;

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

pub(crate) fn folder_node(
    folder: &ResourceLocation,
    changes: Option<&FolderChanges>,
    items: &mut rpds::HashTrieMapSync<ResourceLocation, RowItem>,
    counts: (skia_safe::Color, skia_safe::Color),
) -> ForestNode<ResourceLocation> {
    // The workspace folder ROOT opens the diff canvas
    // (docs/editor/diff-canvas.md §6); the chevron expands either way.
    items.insert_mut(
        folder.clone(),
        RowItem::Open {
            source: crate::diff_canvas::CanvasSource::WorkingCopy {
                folder: folder.clone(),
            },
            reveal: None,
            toggle: false,
            select: false,
        },
    );
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
                    folder: &'a ResourceLocation,
                }
                impl DirSink for Sink<'_> {
                    fn branch(&mut self, key: &ResourceLocation) {
                        self.items
                            .insert_mut(key.clone(), RowItem::Branch { select: false });
                    }

                    fn file_key(
                        &self,
                        entry: &ChangeEntry,
                        _at: &ResourceLocation,
                    ) -> ResourceLocation {
                        entry.working.clone()
                    }
                    fn file(&mut self, entry: &ChangeEntry, key: &ResourceLocation) {
                        // A file row REVEALS itself in the folder's
                        // canvas — the canvas row keys are these
                        // same locations.
                        self.items.insert_mut(
                            key.clone(),
                            RowItem::Open {
                                source: crate::diff_canvas::CanvasSource::WorkingCopy {
                                    folder: self.folder.clone(),
                                },
                                reveal: Some(entry.working.clone()),
                                toggle: false,
                                select: true,
                            },
                        );
                    }
                }
                dir_forest(folder, trie, counts, &mut Sink { items, folder })
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
        let view = Changes::mint_view(
            store,
            &workspace.clone(),
            crate::changes_view::ChangesView::open(
                store,
                &_app.ui_ctx(),
                window,
                workspace.clone(),
                crate::changes_view::ViewSets::WorkingCopies,
            ),
        );
        let owner = self.id();
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| {
                entity.show_dock(
                    store,
                    Box::new(crate::changes_view::ChangesPane::new(workspace, view)),
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
    /// The session to refetch in. `None` means "the one this window is
    /// working in", resolved when the command performs — a command
    /// registered into the palette has no session at registration.
    pub home: Option<crate::SessionId>,
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
        let home = self.home.clone().unwrap_or_else(|| {
            crate::Windows::window_ref(store, window)
                .map(|entity| entity.current_session())
                .unwrap_or_else(|| crate::SessionId::working(store))
        });
        Changes::refetch(store, &home, window, fx, self.folder.as_ref());
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
