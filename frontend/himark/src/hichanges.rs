// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{Authority, ForestNode, ResourceLocation, ResourceType};
use imba::{effect::AnyEffect, store::Store};

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

impl ChangeEntry {
    /// The driver's construction door — digestion builds entries
    /// outside this module; the stamp starts at zero and is the
    /// adopt road's to move.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn assembled(
        id: String,
        rel: Vec<String>,
        working: ResourceLocation,
        before: Option<ResourceLocation>,
        after: Option<ResourceLocation>,
        added: Option<i64>,
        removed: Option<i64>,
    ) -> Self {
        Self {
            id,
            rel,
            working,
            before,
            after,
            added,
            removed,
            updated: 0,
        }
    }
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

/// A changeset snapshot digested OFF the UI thread: the wire `Value`
/// trees are parsed and the uris resolved inside the effect's landing
/// map (the background runner's thread), so the UI-thread adopt only
/// stamps and swaps finished entries (docs/perf-issue.md §2).
#[derive(Clone)]
pub struct DigestedChangeset {
    pub(crate) status: ChangesStatus,
    pub(crate) entries: Vec<ChangeEntry>,
}

/// A polled wire action digested OFF the UI thread — `entry_of` has
/// already run; the UI-thread fold only splices finished entries.
#[derive(Clone)]
pub enum ChangeAction {
    Content(Vec<ChangeEntry>),
    Status(ChangesStatus),
    /// `entry` is `None` when the wire file resolves outside the
    /// folder — the standing entry under `id` still leaves.
    FileSet {
        id: String,
        entry: Option<ChangeEntry>,
    },
    FileRemoved(String),
    Cleared,
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
#[derive(Clone)]
pub struct ChangeSet {
    pub(crate) source: ChangeSetSource,

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

/// The session's change sets, keyed by minted id
/// (docs/model-view.md): one record per working copy, one per commit.
/// Session-family state — the AHP changeset/history channels are
/// session-level (docs/ahp/vcs.md), so the collection gathers with
/// its session.
#[derive(Clone)]
pub struct ChangeSets {
    /// Siblings, wired at the family mint (docs/entities.md law 4):
    /// the documents whose stripes this collection's bases serve, and
    /// the history whose commits mint sets here.
    documents: imba::store::Id<crate::OpenDocuments>,
    history: imba::store::Id<crate::hihistory::History>,

    sets: rpds::HashTrieMapSync<ChangeSetId, ChangeSet>,

    /// Source → set: the reuse lookup for BOTH flavors.
    pub(crate) by_source: rpds::HashTrieMapSync<ChangeSetSource, ChangeSetId>,

    /// Set → the serial of ITS one standing poll loop. A relaunch
    /// bumps it; a `Polled` landing re-arms only when it carries the
    /// current serial, so a re-subscribe (refetch, catalog re-route)
    /// supersedes the old loop instead of multiplying it.

    /// A landing's pending stripe-base re-asks (the folders whose
    /// bases must re-resolve) — the collection's own note to its
    /// `after_route` tail, drained the same batch. Private schema,
    /// not a store component.
    rearms: Vec<ResourceLocation>,

    /// Refetch intents the views NOTED (None = every folder) — the
    /// wire driver's lane drains them; no view carries a wire.
    refetch_asks: Vec<Option<ResourceLocation>>,

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

/// What the collection answers to behind its `At` address
/// (docs/entities.md law 5).
pub enum ChangesCommand {
    /// A command for a SET-OWNED canvas — the DiffView shape: routed
    /// by the collection, the set and the canvas; no window, no
    /// session.
    Canvas(
        ChangeSetId,
        crate::diff_canvas::canvas::CanvasId,
        Box<crate::diff_canvas::canvas::CanvasCommand>,
    ),
}

impl std::fmt::Display for ChangesCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangesCommand::Canvas(_, _, command) => command.fmt(out),
        }
    }
}

impl imba::store::Entity for ChangeSets {
    type Command = ChangesCommand;

    fn perform(
        &mut self,
        id: imba::store::Id<Self>,
        command: ChangesCommand,
        store: &mut Store,
        ui: &imba::UiCtx,
        fx: &mut imba::effect::Effects<'_, ChangesCommand>,
    ) {
        match command {
            ChangesCommand::Canvas(set, canvas, command) => {
                // The canvas perform legitimately reopens this
                // collection (set rows, sibling documents) — give the
                // row back for the duration and pick up the fresh
                // state after, the documents-entity escape. The
                // stand-in is an EMPTY wired row, never Default: the
                // sibling ids stay real.
                let stand_in = Self::wired(self.documents, self.history);
                store.unlease(id, std::mem::replace(self, stand_in));
                crate::diff_canvas::canvas::perform_canvas(
                    store, ui, id, set, canvas, *command, fx,
                );
                if let Some(fresh) = store.lease(id) {
                    *self = fresh;
                }
            }
        }
    }

    fn destroy(&mut self, _store: &mut Store) {
        // The records are the collection's private schema; the family
        // ceremony owns the retract.
    }
}

impl Changes {
    /// A collection wired to its siblings — minted by the family
    /// ceremony, and by tests that stand one up alone.
    pub fn wired(
        documents: imba::store::Id<crate::OpenDocuments>,
        history: imba::store::Id<crate::hihistory::History>,
    ) -> Self {
        Self {
            documents,
            history,
            sets: rpds::HashTrieMapSync::new_sync(),
            by_source: rpds::HashTrieMapSync::new_sync(),
            rearms: Vec::new(),
            refetch_asks: Vec::new(),
            views: rpds::HashTrieMapSync::new_sync(),
            viewers: rpds::HashTrieMapSync::new_sync(),
            stale: rpds::HashTrieSetSync::new_sync(),
        }
    }

    pub fn documents(&self) -> imba::store::Id<crate::OpenDocuments> {
        self.documents
    }

    pub fn history(&self) -> imba::store::Id<crate::hihistory::History> {
        self.history
    }

    pub(crate) fn of(store: &Store, changes: imba::store::Id<ChangeSets>) -> Option<&ChangeSets> {
        store.entity(changes)
    }

    /// Mutate the collection in place. A gone collection takes no
    /// write — there is no row to mint from: siblings are wired at
    /// the ceremony, never defaulted.
    pub(crate) fn update(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        mutate: impl FnOnce(&mut ChangeSets),
    ) {
        let Some(mut row) = store.entity(changes).cloned() else {
            return;
        };
        mutate(&mut row);
        store.put_entity(changes, row);
    }

    fn folder_set_id(&self, folder: &ResourceLocation) -> Option<ChangeSetId> {
        let source = ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        };
        self.by_source.get(&source).copied()
    }

    /// Leave the stripe-base note on the row: the re-asks run behind
    /// the lease, so the work waits for `after_route`.
    fn note_rearm(&mut self, folder: &ResourceLocation) {
        self.rearms.push(folder.clone());
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sets.is_empty() && self.refetch_asks.is_empty()
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
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
        revision: &Revision,
    ) -> ChangeSetId {
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        if let Some(id) =
            Self::of(store, changes).and_then(|changes| changes.by_source.get(&source).copied())
        {
            return id;
        }
        let id = ChangeSetId::mint();
        Self::update(store, changes, |changes| {
            changes.sets.insert_mut(
                id,
                ChangeSet {
                    source: source.clone(),
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
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
        revision: &Revision,
    ) -> Option<ChangeSet> {
        let changes = Self::of(store, changes)?;
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        changes.sets.get(changes.by_source.get(&source)?).cloned()
    }

    /// The per-SET staleness cue for a commit canvas.
    pub fn commit_generation(
        store: &Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
        revision: &Revision,
    ) -> u64 {
        Self::commit_set(store, changes, folder, revision)
            .map(|set| set.generation)
            .unwrap_or(0)
    }

    /// Mint — or find — the set for a source, WITHOUT a feed when the
    /// feeds have not routed it yet (a canvas may open first; the
    /// feed attaches at `ensure_folder`). Never bumps generations.
    pub(crate) fn ensure_set_for_source(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        source: &ChangeSetSource,
    ) -> ChangeSetId {
        if let Some(id) =
            Self::of(store, changes).and_then(|changes| changes.by_source.get(source).copied())
        {
            return id;
        }
        let id = ChangeSetId::mint();
        let source = source.clone();
        Self::update(store, changes, |changes| {
            changes.sets.insert_mut(
                id,
                ChangeSet {
                    source: source.clone(),
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
        changes: imba::store::Id<ChangeSets>,
        source: &ChangeSetSource,
    ) -> Option<ChangeSetId> {
        Self::of(store, changes)?.by_source.get(source).copied()
    }

    /// The SET owns its canvases; these reach one by (set, canvas) —
    /// canvas mutations never touch the set's generation.
    pub(crate) fn canvas_ref<'a>(
        store: &'a Store,
        changes: imba::store::Id<ChangeSets>,
        set: ChangeSetId,
        canvas: crate::diff_canvas::canvas::CanvasId,
    ) -> Option<&'a crate::diff_canvas::canvas::Canvas> {
        Self::of(store, changes)?
            .sets
            .get(&set)?
            .canvases
            .get(&canvas)
    }

    pub(crate) fn take_canvas(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        set: ChangeSetId,
        canvas: crate::diff_canvas::canvas::CanvasId,
    ) -> Option<crate::diff_canvas::canvas::Canvas> {
        let held = Self::canvas_ref(store, changes, set, canvas)?.clone();
        Self::update(store, changes, |changes| {
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
        changes: imba::store::Id<ChangeSets>,
        set: ChangeSetId,
        canvas: crate::diff_canvas::canvas::CanvasId,
        held: crate::diff_canvas::canvas::Canvas,
    ) {
        Self::update(store, changes, |changes| {
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
        changes: imba::store::Id<ChangeSets>,
    ) -> Vec<(ChangeSetId, crate::diff_canvas::canvas::CanvasId)> {
        Self::of(store, changes)
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
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
        revision: &Revision,
    ) {
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        Self::update(store, changes, |changes| {
            changes.update_set_by_source(&source, |set| {
                set.status = ChangesStatus::Computing;
                set.files = rpds::VectorSync::new_sync();
            });
        });
        if let Some(id) = Self::id_for_source(store, changes, &source) {
            Self::nudge_set(store, changes, id);
        }
    }

    /// A commit set's content snapshot landed (the changeset channel
    /// for `?commit=<sha>`): status + files onto the SET.
    pub(crate) fn adopt_commit_state(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
        revision: &Revision,
        result: &Result<DigestedChangeset, String>,
    ) {
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        Self::update(store, changes, |changes| {
            changes.update_set_by_source(&source, |set| match result {
                Ok(digested) => {
                    set.status = digested.status.clone();
                    set.files = digested.entries.iter().cloned().collect();
                }
                Err(error) => {
                    set.status = ChangesStatus::Error(error.clone());
                    set.files = rpds::VectorSync::new_sync();
                }
            });
        });
        if let Some(id) = Self::id_for_source(store, changes, &source) {
            Self::nudge_set(store, changes, id);
        }
    }

    /// Streamed updates for a commit set's changeset channel.
    pub(crate) fn fold_commit_actions(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
        revision: &Revision,
        actions: Vec<ChangeAction>,
    ) {
        let source = ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: revision.to_owned(),
        };
        Self::update(store, changes, |changes| {
            changes.update_set_by_source(&source, |set| {
                for action in &actions {
                    match action {
                        ChangeAction::Content(entries) => {
                            set.files = entries.iter().cloned().collect();
                        }
                        ChangeAction::Status(status) => {
                            set.status = status.clone();
                        }
                        _ => {}
                    }
                }
            });
        });
        if let Some(id) = Self::id_for_source(store, changes, &source) {
            Self::nudge_set(store, changes, id);
        }
    }

    pub fn set_ref<'a>(
        store: &'a Store,
        changes: imba::store::Id<ChangeSets>,
        id: ChangeSetId,
    ) -> Option<&'a ChangeSet> {
        Self::of(store, changes)?.sets.get(&id)
    }

    pub fn id_for_folder(
        store: &Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
    ) -> Option<ChangeSetId> {
        let source = ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        };
        Self::of(store, changes)?.by_source.get(&source).copied()
    }

    /// The per-SET staleness cue for a working copy — 0 while absent
    /// (the canvas keeps probing until the set exists).
    pub fn folder_generation(
        store: &Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
    ) -> u64 {
        Self::of(store, changes)
            .and_then(|changes| changes.folder_set(folder))
            .map(|set| set.generation)
            .unwrap_or(0)
    }

    pub fn folder(
        store: &Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
    ) -> Option<ChangeSet> {
        Self::of(store, changes)?.folder_set(folder).cloned()
    }

    /// The driver's attach door (docs/entities.md: collections are
    /// passive models — the wire driver calls this): the working-copy
    /// set exists after it. Idempotent — a canvas may have opened the
    /// set detached already, and the door keeps it.
    pub(crate) fn ensure_working_set(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
    ) {
        let source = ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        };
        if Self::of(store, changes).is_some_and(|held| held.by_source.contains_key(&source)) {
            return;
        }
        Self::update(store, changes, |changes| {
            let id = ChangeSetId::mint();
            changes.sets.insert_mut(
                id,
                ChangeSet {
                    source: source.clone(),
                    status: ChangesStatus::Computing,
                    files: rpds::VectorSync::new_sync(),
                    generation: 0,
                    bases: rpds::HashTrieMapSync::new_sync(),
                    canvases: rpds::HashTrieMapSync::new_sync(),
                },
            );
            changes.by_source.insert_mut(source, id);
        });
    }

    /// A snapshot answered for one folder's set — already digested on
    /// the effect worker; this is value adoption only.
    pub(crate) fn adopt_snapshot(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
        result: Result<DigestedChangeset, String>,
    ) {
        Self::update(store, changes, |held| {
            match result {
                Ok(digested) => held.adopt(folder, digested),
                Err(error) => held.adopt_error(folder, error),
            }
            held.nudge_folder_in_place(folder);
            held.note_rearm(folder);
        });
    }

    /// A polled batch folded into one folder's set — already digested.
    pub(crate) fn fold_folder(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
        actions: Vec<ChangeAction>,
    ) {
        Self::update(store, changes, |held| {
            held.fold(folder, actions);
            held.nudge_folder_in_place(folder);
            held.note_rearm(folder);
        });
    }

    /// One folder's set resolves errored (the session feed failed).
    pub(crate) fn fold_error(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
        error: &str,
    ) {
        Self::update(store, changes, |held| {
            held.adopt_error(folder, error.to_owned());
            held.nudge_folder_in_place(folder);
        });
    }

    /// The refetch road's mark: the folder's set shows computing
    /// while the fresh snapshot rides.
    pub(crate) fn mark_folder_computing(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
    ) {
        Self::update(store, changes, |held| {
            held.update_folder_set(folder, |set| {
                set.status = ChangesStatus::Computing;
            });
        });
    }

    /// Drain the stripe-base re-ask note the mutations left — the
    /// driver runs the asks with the effects the model does not hold.
    /// Note a refetch ask for the driver's lane (None = all).
    pub fn ask_refetch(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        only: Option<ResourceLocation>,
    ) {
        Self::update(store, changes, |held| held.refetch_asks.push(only));
    }

    pub(crate) fn owes_refetch(store: &Store, changes: imba::store::Id<ChangeSets>) -> bool {
        Self::of(store, changes).is_some_and(|held| !held.refetch_asks.is_empty())
    }

    pub(crate) fn take_refetch_asks(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
    ) -> Vec<Option<ResourceLocation>> {
        let Some(mut held) = store.entity::<ChangeSets>(changes).cloned() else {
            return Vec::new();
        };
        let asks = std::mem::take(&mut held.refetch_asks);
        store.put_entity(changes, held);
        asks
    }

    pub(crate) fn take_rearms(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
    ) -> Option<(imba::store::Id<crate::OpenDocuments>, Vec<ResourceLocation>)> {
        let mut row = store.entity::<ChangeSets>(changes).cloned()?;
        let rearms = std::mem::take(&mut row.rearms);
        let documents = row.documents;
        store.put_entity(changes, row);
        Some((documents, rearms))
    }

    pub fn script_summary(store: &Store, changes: imba::store::Id<ChangeSets>) -> Option<String> {
        let changes = Self::of(store, changes)?;
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

    /// Adopt a snapshot the effect worker already digested: stamp the
    /// finished entries against the standing ones and swap — the only
    /// UI-thread work left is value compares (docs/perf-issue.md §2).
    fn adopt(&mut self, folder: &ResourceLocation, digested: DigestedChangeset) {
        self.update_folder_set(folder, |set| {
            set.status = digested.status;
            let mut fresh = digested.entries;
            stamp_entries(set.files.iter(), fresh.iter_mut(), set.generation + 1);
            set.files = fresh.into_iter().collect();
            set.note_bases();
        });
    }

    fn adopt_error(&mut self, folder: &ResourceLocation, error: String) {
        self.update_folder_set(folder, |set| {
            set.status = ChangesStatus::Error(error);
            set.files = rpds::VectorSync::new_sync();
            set.note_bases();
        });
    }

    /// Fold a polled batch the effect worker already digested — the
    /// wire parsing happened there, and the superseded file mutations
    /// (everything a later full snapshot overwrites) never arrive
    /// (docs/perf-issue.md §2, §4 measure 3).
    fn fold(&mut self, folder: &ResourceLocation, actions: Vec<ChangeAction>) {
        self.update_folder_set(folder, |set| {
            let entry = set;
            let stamp = entry.generation + 1;
            let mut files: Vec<Option<ChangeEntry>> =
                entry.files.iter().cloned().map(Some).collect();
            let mut by_id: std::collections::HashMap<String, usize> = files
                .iter()
                .enumerate()
                .filter_map(|(at, slot)| slot.as_ref().map(|file| (file.id.clone(), at)))
                .collect();
            for action in actions {
                match action {
                    ChangeAction::Content(mut fresh) => {
                        // At most one per batch (the digest dropped the
                        // superseded ones), so the standing `files` ARE
                        // the previous list to stamp against.
                        stamp_entries(
                            files.iter().filter_map(|slot| slot.as_ref()),
                            fresh.iter_mut(),
                            stamp,
                        );
                        files = fresh.into_iter().map(Some).collect();
                        by_id = files
                            .iter()
                            .enumerate()
                            .filter_map(|(at, slot)| {
                                slot.as_ref().map(|file| (file.id.clone(), at))
                            })
                            .collect();
                    }
                    ChangeAction::Status(status) => {
                        entry.status = status;
                    }
                    ChangeAction::FileSet { id, entry: fresh } => {
                        if let Some(at) = by_id.remove(&id) {
                            files[at] = None;
                        }
                        if let Some(mut fresh) = fresh {
                            // The host resends a file exactly when it
                            // changed — stamp unconditionally; value
                            // equality cannot see a same-stats edit.
                            fresh.updated = stamp;
                            by_id.insert(fresh.id.clone(), files.len());
                            files.push(Some(fresh));
                        }
                    }
                    ChangeAction::FileRemoved(id) => {
                        if let Some(at) = by_id.remove(&id) {
                            files[at] = None;
                        }
                    }
                    ChangeAction::Cleared => {
                        files.clear();
                        by_id.clear();
                    }
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
        changes: imba::store::Id<ChangeSets>,
        abs_path: &str,
    ) -> Option<ResourceLocation> {
        Self::of(store, changes)?
            .sets
            .values()
            .find_map(|set| set.bases.get(abs_path).cloned())
    }
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
        let documents = crate::Windows::session_family(store, window)
            .expect("a diff opens from a window with a session")
            .documents();
        let old = crate::DiffSideInput::resolve(store, documents, self.old.clone());
        let new = crate::DiffSideInput::resolve(store, documents, self.new.clone());
        let _ = fx.push(AnyEffect::new(crate::OpenDiffByLocationsEffect {
            window,
            documents,
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
        // The pane closes over the window's family: its change sets
        // by id, the session only as the catalog's name for the folders.
        let workspace = entity.current_session();
        let changes = entity.family().changes();
        let wire = entity.family().changes_wire();
        let folders = crate::higent::session_folders(store, &workspace);
        crate::drivers::changes::ensure(store, window, wire, folders, fx);

        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let view = Changes::mint_view(
            store,
            changes,
            crate::changes_view::ChangesView::open(
                store,
                &_app.ui_ctx(),
                window,
                changes,
                wire,
                workspace,
                crate::changes_view::ViewSets::WorkingCopies,
            ),
        );
        let owner = self.id();
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| {
                entity.show_dock(
                    store,
                    Box::new(crate::changes_view::ChangesPane::new(changes, view)),
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
    /// The wire to refetch through. `None` means "the window's
    /// family's", resolved when the command performs — a command
    /// registered into the palette holds no id at registration.
    pub wire: Option<imba::store::Id<crate::drivers::changes::ChangesWire>>,
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
        let _ = fx;
        let Some(changes) = crate::Windows::session_family(store, window)
            .map(|family| family.changes())
            .or_else(|| {
                // An addressed refetch (the view's chip) names its
                // wire's collection directly.
                self.wire
                    .and_then(|wire| crate::drivers::changes::changes_of(store, wire))
            })
        else {
            return;
        };
        Changes::ask_refetch(store, changes, self.folder.clone());
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
