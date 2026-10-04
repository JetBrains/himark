// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::diff::DiffId;
use imba::effect::{CancellationToken, Effect, EffectHandler};
use imba::store::Store;
use operation::Operation;

use crate::{DocumentId, OpenDocuments};

fn probe() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_DIFF").is_some())
}

/// base -> target as ONE replacement: the only correct-by-construction
/// operation over two texts that costs no diffing — the seed for a
/// tracking whose minimal diff the normalize lane still owes.
fn whole_replace(base: &editor::Text, target: &editor::Text) -> Operation {
    let mut ops = Vec::with_capacity(2);
    let base_len = base.byte_count();
    if base_len > 0 {
        ops.push(operation::Op::Delete(base.view().byte_string(0, base_len)));
    }
    let target_len = target.byte_count();
    if target_len > 0 {
        ops.push(operation::Op::Insert(
            target.view().byte_string(0, target_len),
        ));
    }
    Operation::from_ops(ops)
}

#[derive(Clone)]
pub(crate) struct DiffRecord {
    pub(crate) base: DocumentId,
    pub(crate) target: DocumentId,

    pub(crate) base_markup: editor::MarkupId,

    pub(crate) refs: u32,

    pub(crate) stripes: bool,

    pub(crate) normalize_token: Option<CancellationToken>,

    pub(crate) normalized: Option<(u64, u64)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DiffViewId(u64);

impl DiffViewId {
    pub fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[derive(Clone)]
pub struct DiffView {
    pub left: crate::EditorIdView,
    pub right: crate::EditorIdView,
    pub diff: DiffId,
    /// A pair living INSIDE a diff canvas row — it fronts with its
    /// canvas, never as its own session row (the peeker once listed
    /// every canvas row as a bare "Diff").
    pub embedded: bool,
    /// The pane's own right-half extras entry (word tints + fold
    /// strips) — editor-owned, dying with the right half's editor.
    /// THE diff markup (hunk washes) is the entry's own
    /// (`Diff::markup`), never the pane's to write.
    pub right_extras: editor::MarkupId,
    pub state: Option<editor::DiffViewState>,
}

#[derive(Clone, Default)]
pub(crate) struct Diffs {
    records: rpds::HashTrieMapSync<DiffId, DiffRecord>,

    diff_views: rpds::HashTrieMapSync<u64, DiffView>,

    by_target: rpds::HashTrieMapSync<DocumentId, Vec<DiffId>>,

    by_base: rpds::HashTrieMapSync<DocumentId, Vec<DiffId>>,

    /// Side document → the views it fronts in — the dressing sweep's
    /// reverse index: a document write names exactly the views whose
    /// state can lag, so the sweep never walks `diff_views` whole
    /// (docs/perf-issue.md §1b).
    views_by_document: rpds::HashTrieMapSync<DocumentId, Vec<u64>>,
}

impl Diffs {
    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty() && self.diff_views.is_empty()
    }

    pub(crate) fn record(&self, id: DiffId) -> Option<&DiffRecord> {
        self.records.get(&id)
    }

    pub(crate) fn touches(&self, document: DocumentId) -> bool {
        self.by_target
            .get(&document)
            .is_some_and(|ids| !ids.is_empty())
            || self
                .by_base
                .get(&document)
                .is_some_and(|ids| !ids.is_empty())
    }

    pub(crate) fn by_pair(&self, base: DocumentId, target: DocumentId) -> Option<DiffId> {
        self.by_target.get(&target)?.iter().copied().find(|id| {
            self.records
                .get(id)
                .is_some_and(|record| record.base == base)
        })
    }

    pub(crate) fn stripe_of(&self, target: DocumentId) -> Option<DiffId> {
        self.by_target
            .get(&target)?
            .iter()
            .copied()
            .find(|id| self.records.get(id).is_some_and(|record| record.stripes))
    }

    pub(crate) fn insert(&mut self, id: DiffId, record: DiffRecord) {
        let join = |index: &mut rpds::HashTrieMapSync<DocumentId, Vec<DiffId>>,
                    document: DocumentId| {
            let mut ids = index.get(&document).cloned().unwrap_or_default();
            if !ids.contains(&id) {
                ids.push(id);
            }
            index.insert_mut(document, ids);
        };
        join(&mut self.by_target, record.target);
        join(&mut self.by_base, record.base);
        self.records.insert_mut(id, record);
    }

    pub(crate) fn put(&mut self, id: DiffId, record: DiffRecord) {
        debug_assert!(self.records.get(&id).is_some_and(|standing| {
            standing.base == record.base && standing.target == record.target
        }));
        self.records.insert_mut(id, record);
    }

    /// BOTH side documents' diffs — the diff-lane sweep's candidates
    /// for one touched document.
    pub(crate) fn of_document(&self, document: DocumentId) -> impl Iterator<Item = DiffId> + '_ {
        self.by_base
            .get(&document)
            .into_iter()
            .chain(self.by_target.get(&document))
            .flatten()
            .copied()
    }

    pub(crate) fn join_view(&mut self, id: u64, view: &DiffView) {
        let mut join = |document: DocumentId| {
            let mut ids = self
                .views_by_document
                .get(&document)
                .cloned()
                .unwrap_or_default();
            if !ids.contains(&id) {
                ids.push(id);
            }
            self.views_by_document.insert_mut(document, ids);
        };
        join(view.left.document());
        join(view.right.document());
    }

    pub(crate) fn leave_view(&mut self, id: u64) {
        let Some(view) = self.diff_views.get(&id) else {
            return;
        };
        let sides = [view.left.document(), view.right.document()];
        for document in sides {
            let Some(mut ids) = self.views_by_document.get(&document).cloned() else {
                continue;
            };
            ids.retain(|other| *other != id);
            match ids.is_empty() {
                true => {
                    self.views_by_document.remove_mut(&document);
                }
                false => self.views_by_document.insert_mut(document, ids),
            }
        }
    }

    pub(crate) fn views_of(&self, document: DocumentId) -> impl Iterator<Item = DiffViewId> + '_ {
        self.views_by_document
            .get(&document)
            .into_iter()
            .flatten()
            .map(|id| DiffViewId(*id))
    }

    pub(crate) fn remove(&mut self, id: DiffId) -> Option<DiffRecord> {
        let record = self.records.get(&id).cloned()?;
        let leave = |index: &mut rpds::HashTrieMapSync<DocumentId, Vec<DiffId>>,
                     document: DocumentId| {
            let Some(mut ids) = index.get(&document).cloned() else {
                return;
            };
            ids.retain(|other| *other != id);
            match ids.is_empty() {
                true => {
                    index.remove_mut(&document);
                }
                false => index.insert_mut(document, ids),
            }
        };
        leave(&mut self.by_target, record.target);
        leave(&mut self.by_base, record.base);
        self.records.remove_mut(&id);
        Some(record)
    }
}

#[derive(Clone, Copy)]
pub struct DiffHandle {
    pub id: DiffId,
    pub base: DocumentId,
    pub target: DocumentId,
    pub base_markup: editor::MarkupId,
}

impl OpenDocuments {
    pub fn put_diff_view(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        id: DiffViewId,
        pair: DiffView,
    ) {
        store.update_entity(documents, |docs| {
            docs.diffs.join_view(id.0, &pair);
            docs.diffs.diff_views.insert_mut(id.0, pair);
        });
    }

    /// Note a view the dressing touched this batch — the collection's
    /// own tail queue (the `PendingSweeps` shape): the canvas lane
    /// takes exactly these at the batch tail and resizes their rows.
    pub fn note_dressed(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        id: DiffViewId,
    ) {
        store.update_entity(documents, |docs: &mut OpenDocuments| {
            docs.pending.dressed.push(id);
        });
    }

    /// Drain the dressed-views queue. Read-take-put, never minting:
    /// a gone collection answers empty instead of resurrecting.
    pub fn take_dressed(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
    ) -> Vec<DiffViewId> {
        let Some(mut docs) = store.entity::<OpenDocuments>(documents).cloned() else {
            return Vec::new();
        };
        let dressed = std::mem::take(&mut docs.pending.dressed);
        store.put_entity(documents, docs);
        dressed
    }

    pub fn take_diff_view(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        id: DiffViewId,
    ) -> Option<DiffView> {
        let pair = Self::diff_view_ref(store, documents, id).cloned();
        if pair.is_some() {
            Self::remove_diff_view(store, documents, id);
        }
        pair
    }

    pub fn diff_view_ref(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        id: DiffViewId,
    ) -> Option<&DiffView> {
        store
            .entity(documents)
            .and_then(|docs| docs.diffs.diff_views.get(&id.0))
    }

    pub fn remove_diff_view(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        id: DiffViewId,
    ) {
        store.update_entity(documents, |docs| {
            docs.diffs.leave_view(id.0);
            docs.diffs.diff_views.remove_mut(&id.0);
        });
    }

    /// Drain the dressing sweep's queue: the tracked views whose SIDE
    /// documents were written since the last sweep — the only views
    /// whose state can lag. O(touched views), never O(all views ever)
    /// (docs/perf-issue.md §1b).
    pub fn take_stale_view_candidates(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
    ) -> Vec<DiffViewId> {
        let mut candidates = Vec::new();
        store.update_entity(documents, |docs| {
            let touched = std::mem::take(&mut docs.pending.dressing);
            for document in touched.iter() {
                for view in docs.diffs.views_of(*document) {
                    if !candidates.contains(&view) {
                        candidates.push(view);
                    }
                }
            }
        });
        candidates
    }

    /// The STANDALONE pairs — the session rows a peeker can front.
    /// Canvas-embedded pairs stay with their canvas.
    /// Every tracked diff view — the dressing sweep's domain.
    pub fn diff_view_ids(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
    ) -> Vec<DiffViewId> {
        store
            .entity(documents)
            .map(|docs| {
                docs.diffs
                    .diff_views
                    .keys()
                    .map(|id| DiffViewId(*id))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn pair_ids(store: &Store, documents: imba::store::Id<OpenDocuments>) -> Vec<DiffViewId> {
        store
            .entity(documents)
            .map(|docs| {
                docs.diffs
                    .diff_views
                    .iter()
                    .filter(|(_, pair)| !pair.embedded)
                    .map(|(id, _)| DiffViewId(*id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// TEST SUPPORT: every held pair, embedded or not.
    #[doc(hidden)]
    pub fn diff_view_count(store: &Store, documents: imba::store::Id<OpenDocuments>) -> usize {
        store
            .entity(documents)
            .map(|docs| docs.diffs.diff_views.size())
            .unwrap_or(0)
    }

    pub fn holds_diff_view(&self, id: DiffViewId) -> bool {
        self.diffs.diff_views.contains_key(&id.0)
    }

    pub fn pair_tracked(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        base: DocumentId,
        target: DocumentId,
    ) -> bool {
        store
            .entity(documents)
            .is_some_and(|docs| docs.diffs.by_pair(base, target).is_some())
    }

    /// Track a diff between two ALREADY-REGISTERED documents and mint
    /// its entry. The entry is SEEDED with the whole-replace (an exact,
    /// content-blind delete-all/insert-all — no diffing on this thread)
    /// and the batch-tail sweep's normalize lane computes the real
    /// minimal diff from the live documents (structural, from their
    /// parses) and dresses it. There is no prepared operation and no
    /// snapshot: the documents ARE the truth (docs/no-diff-on-ui-thread,
    /// docs/editor/diff-canvas.md §7).
    pub fn track_diff(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        base: DocumentId,
        target: DocumentId,
        stripes: bool,
    ) -> Option<DiffId> {
        let mut result = None;
        store.update_entity(documents, |docs| {
            result = docs.track_diff_row(base, target, stripes);
        });
        result
    }

    pub fn track_diff_row(
        &mut self,
        base: DocumentId,
        target: DocumentId,
        stripes: bool,
    ) -> Option<DiffId> {
        if let Some(existing) = self.diffs.by_pair(base, target) {
            let mut record = self.diffs.record(existing).expect("indexed").clone();
            record.refs += 1;
            // The pair takes the stripes role it didn't have:
            // its markup steps onto the enabled tracks.
            if stripes && !record.stripes {
                if let Some(target_entity) = self.entries.get(&target) {
                    if let Some(markup) = target_entity
                        .document
                        .diff(existing)
                        .map(|entry| entry.markup())
                    {
                        let mut target_document = target_entity.document.clone();
                        target_document.mark_scroll_stripes_on_enabled(markup);
                        let mut entity = target_entity.clone();
                        entity.document = target_document;
                        self.entries.insert_mut(target, entity);
                        self.note_write(target);
                    }
                }
            }
            record.stripes |= stripes;
            self.diffs.put(existing, record);
            return Some(existing);
        }
        let Some(base_entity) = self.entries.get(&base) else {
            return None;
        };
        let base_text = base_entity.document.text().clone();
        let base_revision = base_entity.document.revision();
        let Some(target_entity) = self.entries.get(&target) else {
            return None;
        };
        // The seed: exact by construction, zero diffing. The
        // normalize lane owes the minimal diff.
        let operation = whole_replace(&base_text, target_entity.document.text());
        let mut target_document = target_entity.document.clone();
        let id = target_document.add_diff(operation, base_revision);
        // THE stripes diff registers on the enabled tracks; a
        // panel's diff (stripes=false) stays off them.
        if stripes {
            if let Some(markup) = target_document.diff(id).map(|entry| entry.markup()) {
                target_document.mark_scroll_stripes_on_enabled(markup);
            }
        }
        let mut entity = target_entity.clone();
        entity.document = target_document;
        self.entries.insert_mut(target, entity);
        self.note_write(target);

        let base_entity = self.entries.get(&base).expect("checked above");
        let mut base_document = base_entity.document.clone();
        let base_markup = base_document.add_markup();
        let mut entity = base_entity.clone();
        entity.document = base_document;
        self.entries.insert_mut(base, entity);
        self.note_write(base);

        self.diffs.insert(
            id,
            DiffRecord {
                base,
                target,
                base_markup,
                refs: 1,
                stripes,
                normalize_token: None,
                // Seeded, never normalized at birth — the sweep owes it.
                normalized: None,
            },
        );
        if probe() {
            eprintln!("[diffs] tracked {id:?}: {base:?} -> {target:?} stripes={stripes}");
        }
        Some(id)
    }

    /// A PLUGIN BOUNDARY: `remove_diff`/`remove_markup` destroy
    /// inlays whose views resolve this collection by id (fence
    /// embeds), so the row stays IN the table and the spans that
    /// touch it stay narrow.
    pub fn untrack_diff<R: 'static>(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        ui: &imba::UiCtx,
        id: DiffId,
        fx: &mut imba::effect::Effects<'_, R>,
    ) {
        let fonts = editor::env::Fonts::of(store)();
        let theme = editor::env::Themes::of(store);
        let mut released: Option<DiffRecord> = None;
        store.update_entity(documents, |docs| {
            let Some(mut record) = docs.diffs.record(id).cloned() else {
                return;
            };
            record.refs = record.refs.saturating_sub(1);
            if record.refs > 0 {
                docs.diffs.put(id, record);
                return;
            }
            released = docs.diffs.remove(id);
        });
        let Some(record) = released else {
            return;
        };
        if let Some(token) = record.normalize_token {
            fx.cancel(token);
        }

        if let Some(mut document) = Self::document(store, documents, record.target) {
            document.remove_diff(
                id,
                &[],
                store,
                ui,
                &fonts,
                &theme,
                &mut imba::effect::Batch::new().effects(),
            );
            Self::put_document(store, documents, record.target, document);
        }
        if let Some(mut document) = Self::document(store, documents, record.base) {
            document.remove_markup(
                record.base_markup,
                &[],
                store,
                ui,
                &fonts,
                &theme,
                &mut imba::effect::Batch::new().effects(),
            );
            Self::put_document(store, documents, record.base, document);
        }
        Self::remove_if_editorless(store, documents, ui, record.base, fx);
        Self::remove_if_editorless(store, documents, ui, record.target, fx);
    }

    pub fn diff_handle(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        id: DiffId,
    ) -> Option<DiffHandle> {
        store.entity(documents)?.diff_handle_row(id)
    }

    pub fn diff_handle_row(&self, id: DiffId) -> Option<DiffHandle> {
        let record = self.diffs.record(id)?;
        Some(DiffHandle {
            id,
            base: record.base,
            target: record.target,
            base_markup: record.base_markup,
        })
    }

    pub fn stripe_diff(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        target: DocumentId,
    ) -> Option<DiffHandle> {
        store.entity(documents)?.stripe_diff_row(target)
    }

    pub fn stripe_diff_row(&self, target: DocumentId) -> Option<DiffHandle> {
        self.diff_handle_row(self.diffs.stripe_of(target)?)
    }

    pub(crate) fn untrack_stripes<R: 'static>(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        ui: &imba::UiCtx,
        document: DocumentId,
        fx: &mut imba::effect::Effects<'_, R>,
    ) -> bool {
        let Some(id) = store
            .entity(documents)
            .and_then(|docs| docs.diffs.stripe_of(document))
        else {
            return false;
        };
        store.update_entity(documents, |docs| {
            if let Some(mut record) = docs.diffs.record(id).cloned() {
                record.stripes = false;
                docs.diffs.put(id, record);
            }
            // The role ends NOW, even if a panel's ref keeps the diff
            // itself alive: the markup leaves every track.
            if let Some(target_entity) = docs.entries.get(&document) {
                if let Some(markup) = target_entity.document.diff(id).map(|entry| entry.markup()) {
                    let mut target_document = target_entity.document.clone();
                    target_document.unmark_scroll_stripes(markup);
                    let mut entity = target_entity.clone();
                    entity.document = target_document;
                    docs.entries.insert_mut(document, entity);
                    docs.note_write(document);
                }
            }
        });
        Self::untrack_diff(store, documents, ui, id, fx);
        true
    }

    #[doc(hidden)]
    pub fn diff_refs(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        id: DiffId,
    ) -> Option<u32> {
        Some(store.entity(documents)?.diffs.record(id)?.refs)
    }
}

pub fn sync_diff_lanes<R: 'static>(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    fx: &mut imba::effect::Effects<'_, R>,
    wrap: impl Fn(Normalized) -> R + Send + Clone + 'static,
) {
    if store
        .entity(documents)
        .is_none_or(|docs| docs.diffs.is_empty())
    {
        return;
    }
    let policy = editor::env::Differ::of(store);
    store.update_entity(documents, |docs| {
        // O(touched): only a diff whose SIDE document was written since
        // the last sweep can owe a rebase or a normalize — the write
        // doors queue exactly those (docs/perf-issue.md §1a). Everything
        // else never enters the loop.
        let touched = docs.take_diff_lane_pending();
        if touched.is_empty() {
            return;
        }
        let mut lanes: Vec<DiffId> = Vec::new();
        for document in touched.iter() {
            for id in docs.diffs.of_document(*document) {
                if !lanes.contains(&id) {
                    lanes.push(id);
                }
            }
        }
        for id in lanes {
            let Some(mut record) = docs.diffs.record(id).cloned() else {
                continue;
            };
            let Some(base_entity) = docs.entries.get(&record.base) else {
                continue;
            };
            let base_revision = base_entity.document.revision();
            let Some(target_entity) = docs.entries.get(&record.target) else {
                continue;
            };

            let cursor = target_entity
                .document
                .diff(id)
                .map(|entry| entry.base_revision());
            let mut force_normalize = false;
            if cursor.is_some_and(|cursor| cursor != base_revision) {
                let base_log = base_entity.document.log().clone();
                let mut document = target_entity.document.clone();
                match document.apply_diff_base_edits(id, &base_log) {
                    true => {
                        let mut entity = target_entity.clone();
                        entity.document = document;
                        docs.entries.insert_mut(record.target, entity);
                        docs.note_write(record.target);
                    }

                    false => force_normalize = true,
                }
            }

            let target_entity = docs.entries.get(&record.target).expect("present above");
            let target_revision = target_entity.document.revision();
            let now = (base_revision, target_revision);
            if force_normalize || record.normalized != Some(now) {
                if probe() {
                    eprintln!("[diffs] normalize {id:?} at {now:?}");
                }
                // The captures — text, log and above all the SYNTAX
                // TREE copies — live BEHIND the staleness gate: an
                // already-normalized diff costs two map reads here,
                // never a ts_tree_copy (docs/perf-issue.md §1a).
                let base_entity = docs.entries.get(&record.base).expect("present above");
                let base_text = base_entity.document.text().clone();
                let base_syntax = syntax_snapshot(&base_entity.document);
                let target_syntax = syntax_snapshot(&target_entity.document);
                let language = target_syntax
                    .as_ref()
                    .map(|snapshot| snapshot.language.clone());
                let base_tree = base_syntax
                    .filter(|snapshot| Some(&snapshot.language) == language.as_ref())
                    .map(|snapshot| (snapshot.tree, snapshot.fresh));
                let effect = DiffNormalizeEffect {
                    diff: id,
                    base_text,
                    target_text: target_entity.document.text().clone(),
                    previous: target_entity
                        .document
                        .diff(id)
                        .and_then(|entry| target_entity.document.feature_markup(entry.markup()))
                        .cloned(),
                    base_revision,
                    target_revision,
                    language,
                    base_tree,
                    target_tree: target_syntax.map(|snapshot| (snapshot.tree, snapshot.fresh)),
                    policy: policy.clone(),
                };
                fx.relaunch_erased(
                    &mut record.normalize_token,
                    imba::effect::AnyEffect::new(effect).map(wrap.clone()),
                );
                record.normalized = Some(now);
                docs.diffs.put(id, record);
            }
        }
    });
}

pub fn land_normalized(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    id: DiffId,
    minimal: Operation,
    base_revision: u64,
    target_revision: u64,
) -> bool {
    let mut landed = false;
    store.update_entity(documents, |docs| {
        landed = docs.land_normalized_row(id, minimal.clone(), base_revision, target_revision);
    });
    landed
}

impl OpenDocuments {
    pub fn land_normalized_row(
        &mut self,
        id: DiffId,
        minimal: Operation,
        base_revision: u64,
        target_revision: u64,
    ) -> bool {
        let mut landed = false;
        if let Some(record) = self.diffs.record(id).cloned() {
            'land: {
                let Some(base_entity) = self.entries.get(&record.base) else {
                    break 'land;
                };
                let a = base_entity.document.log().compose_since(base_revision);
                let base_now = base_entity.document.revision();
                let Some(target_entity) = self.entries.get(&record.target) else {
                    break 'land;
                };
                let b = target_entity.document.log().compose_since(target_revision);
                let mut rebased = minimal;
                if let Some(a) = a {
                    rebased = a.invert().compose(&rebased);
                }
                if let Some(b) = &b {
                    rebased = rebased.compose(b);
                }
                let mut document = target_entity.document.clone();
                if document.install_normalized_diff(id, rebased, base_now) {
                    let mut entity = target_entity.clone();
                    entity.document = document;
                    self.entries.insert_mut(record.target, entity);
                    self.note_write(record.target);
                    landed = true;
                }
            }
        }
        if probe() {
            eprintln!("[diffs] normalization landed={landed} for {id:?}");
        }
        landed
    }
}

/// Lands a normalize run's freshly derived diff markup on the target
/// document — through the entity's own effects scope, so the swap's
/// repair tails route home like any landing's. A PLUGIN BOUNDARY:
/// the markup swap destroys replaced inlays.
pub fn land_diff_markup(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::UiCtx,
    id: DiffId,
    markup: editor::Markup,
    changed: Vec<std::ops::Range<u32>>,
    derived_at: u64,
    fx: &mut editor::EditorEffects<'_>,
) {
    let Some(record) = store
        .entity(documents)
        .and_then(|docs| docs.diffs.record(id).cloned())
    else {
        return;
    };
    let fonts = editor::env::Fonts::of(store)();
    let theme = editor::env::Themes::of(store);
    let Some(mut document) = OpenDocuments::document(store, documents, record.target) else {
        return;
    };
    document.install_diff_markup(
        id, markup, changed, derived_at, store, ui, &fonts, &theme, fx,
    );
    OpenDocuments::put_document(store, documents, record.target, document);
}

/// The edge-installed BASE RESOLVER: working location → its base ref,
/// answered synchronously from store truth at effect launch (the UI
/// thread has the store; a worker-side handler does not — the old
/// async effect smuggled the data through an Arc<Mutex<HashMap>>,
/// which is exactly the mutable shared state this codebase bans).
#[derive(Clone)]
pub struct StripeBases(pub std::sync::Arc<dyn StripeBaseResolver>);

/// The base resolver's shape: it is handed the documents collection
/// the ask is for, so an edge that keeps bases next to that collection
/// reaches them by id (docs/entities.md law 3).
pub trait StripeBaseResolver: Send + Sync {
    fn resolve(
        &self,
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        location: &editor::ResourceLocation,
    ) -> Option<editor::ResourceLocation>;
}

impl<F> StripeBaseResolver for F
where
    F: Fn(
            &Store,
            imba::store::Id<OpenDocuments>,
            &editor::ResourceLocation,
        ) -> Option<editor::ResourceLocation>
        + Send
        + Sync,
{
    fn resolve(
        &self,
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        location: &editor::ResourceLocation,
    ) -> Option<editor::ResourceLocation> {
        self(store, documents, location)
    }
}

impl StripeBases {
    pub fn install(store: &mut Store, resolve: std::sync::Arc<dyn StripeBaseResolver>) {
        crate::Registry::update(store, |registry| registry.stripe_bases = Some(resolve));
    }
}

pub fn sync_stripe_bases<R: 'static>(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    fx: &mut imba::effect::Effects<'_, R>,
    mut land: impl FnMut(
        &mut Store,
        DocumentId,
        Option<editor::ResourceLocation>,
        &mut imba::effect::Effects<'_, R>,
    ),
) {
    let Some(resolve) =
        crate::Registry::of(store).and_then(|registry| registry.stripe_bases.clone())
    else {
        return;
    };
    let asks: Vec<(DocumentId, editor::ResourceLocation)> = OpenDocuments::list(store, documents)
        .into_iter()
        .filter(|(_, entity)| !entity.base_requested())
        .filter_map(|(document, entity)| {
            let location = entity.location()?.clone();
            (!crate::is_synthetic(&location)).then_some((document, location))
        })
        .collect();
    for (document, location) in asks {
        OpenDocuments::set_base_requested(store, documents, document);
        if probe() {
            eprintln!("[diffs] base ask for /{}", location.path().join("/"));
        }
        let base = resolve.resolve(store, documents, &location);
        land(store, document, base, fx);
    }
}

pub fn rearm_base_asks(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    matches: &dyn Fn(&editor::ResourceLocation) -> bool,
) {
    let rearm: Vec<crate::DocumentId> = OpenDocuments::list(store, documents)
        .into_iter()
        .filter(|(_, entity)| {
            entity.base_requested() && entity.location().is_some_and(|location| matches(location))
        })
        .map(|(document, _)| document)
        .collect();
    for document in rearm {
        OpenDocuments::update_entity(store, documents, document, |entity| {
            entity.base_requested = false
        });
    }
}

pub fn adopt_base_location<R: 'static>(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::UiCtx,
    document: crate::DocumentId,
    base: Option<editor::ResourceLocation>,
    fx: &mut imba::effect::Effects<'_, R>,
) -> Option<editor::ResourceLocation> {
    if !OpenDocuments::contains(store, documents, document) {
        return None;
    }

    if let Some(handle) = OpenDocuments::stripe_diff(store, documents, document) {
        if OpenDocuments::location(store, documents, handle.base).as_ref() == base.as_ref() {
            return None;
        }
        OpenDocuments::untrack_stripes(store, documents, ui, document, fx);

        if !OpenDocuments::contains(store, documents, document) {
            return None;
        }
    }
    let base = base?;
    if let Some(base_id) = OpenDocuments::by_location(store, documents, &base) {
        let _ = OpenDocuments::track_diff(store, documents, base_id, document, true);
        return None;
    }
    Some(base)
}

/// The BaseLocated landing: adopt the resolved base and, if its text
/// is still owed, launch the fetch — the landing comes home as
/// `BaseFetched` on the collection's own command type (the caller
/// scopes the fx to its address).
pub fn land_base_located(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::UiCtx,
    document: crate::DocumentId,
    base: Option<editor::ResourceLocation>,
    fx: &mut imba::effect::Effects<'_, crate::DocumentsCommand>,
) {
    let Some(base) = adopt_base_location(store, documents, ui, document, base, fx) else {
        return;
    };
    let _ = fx.push(
        imba::effect::AnyEffect::new(crate::FetchDocumentEffect {
            location: base.clone(),
        })
        .map(move |text| crate::DocumentsCommand::BaseFetched {
            document,
            base,
            text,
        }),
    );
}

pub fn land_base_built(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::UiCtx,
    document: crate::DocumentId,
    base: editor::ResourceLocation,
    built: editor::Document,
    fx: &mut imba::effect::Effects<'_, crate::DocumentsCommand>,
) {
    if !OpenDocuments::contains(store, documents, document) {
        return;
    }
    if let Some(handle) = OpenDocuments::stripe_diff(store, documents, document) {
        if OpenDocuments::location(store, documents, handle.base).as_ref() == Some(&base) {
            return;
        }
        OpenDocuments::untrack_stripes(store, documents, ui, document, fx);
        if !OpenDocuments::contains(store, documents, document) {
            return;
        }
    }
    let base_id = match OpenDocuments::by_location(store, documents, &base) {
        Some(existing) => existing,
        None => {
            let saved = built.revision();
            let name = base.name().to_owned();
            let id = OpenDocuments::register(store, documents, built, Some(base), name, saved);

            OpenDocuments::set_base_requested(store, documents, id);
            id
        }
    };
    let tracked = OpenDocuments::track_diff(store, documents, base_id, document, true);
    if probe() {
        eprintln!("[diffs] stripes tracked={tracked:?} for {document:?}");
    }
}

pub struct DiffNormalizeEffect {
    pub(crate) diff: DiffId,
    pub(crate) base_text: editor::Text,
    pub(crate) target_text: editor::Text,
    /// The standing diff markup at capture (O(1) persistent clone) —
    /// the worker set-diffs the fresh derivation against it, so the
    /// changed set is the producer's and the landing never walks a
    /// markup (docs/editor/scroll-stripe.md §7).
    pub(crate) previous: Option<editor::Markup>,
    pub(crate) base_revision: u64,
    pub(crate) target_revision: u64,
    /// Target's language at capture — the policy's cue to try
    /// structural alignment.
    pub(crate) language: Option<String>,
    /// Side trees with their freshness — a stale one is edit-adjusted
    /// and rides along for the policy's incremental catch-up parse.
    pub(crate) base_tree: Option<(Box<dyn editor::SyntaxTree>, bool)>,
    pub(crate) target_tree: Option<(Box<dyn editor::SyntaxTree>, bool)>,
    /// The edge-installed policy (`editor::env::Differ`), captured at
    /// launch so the handler needs no store access.
    pub(crate) policy: std::sync::Arc<dyn editor::diff::DiffPolicy>,
}

/// Language + tree clone + freshness. FRESH means the tree matches the
/// text exactly (no edits since the last completed reparse) and the
/// policy may align on it directly. A STALE tree is still handed over:
/// the edit door keeps it edit-adjusted, so it is exactly the `old`
/// tree-sitter's incremental parse wants — the policy catches it up
/// for pennies instead of parsing the whole file cold. Only alignment
/// on a stale tree misaligns; catch-up parsing on it does not.
fn syntax_snapshot(document: &editor::Document) -> Option<TreeSnapshot> {
    let syntax = document.syntax()?;
    let tree = syntax.tree.as_ref()?.clone_tree();
    Some(TreeSnapshot {
        language: syntax.language.clone(),
        tree,
        fresh: document.edited_since_parse().is_empty(),
    })
}

pub(crate) struct TreeSnapshot {
    pub(crate) language: String,
    pub(crate) tree: Box<dyn editor::SyntaxTree>,
    pub(crate) fresh: bool,
}

pub struct Normalized {
    pub diff: DiffId,
    pub operation: Operation,
    /// THE diff markup, derived FROM the fresh operation
    /// (`diff::hunk_markup`) — hunks against
    /// `target_text`@`target_revision`; the landing shifts it home
    /// (docs/editor/scroll-stripe.md §7).
    pub markup: editor::Markup,
    /// The damage the swap owes, worker-computed: the set difference
    /// against the markup standing at capture.
    pub changed: Vec<std::ops::Range<u32>>,
    pub base_revision: u64,
    pub target_revision: u64,
}

impl std::fmt::Display for DiffNormalizeEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "normalize diff {:?} @ ({}, {})",
            self.diff, self.base_revision, self.target_revision
        )
    }
}

impl Effect for DiffNormalizeEffect {
    type Result = Normalized;
}

pub struct DiffNormalizeHandler;

impl EffectHandler<DiffNormalizeEffect> for DiffNormalizeHandler {
    async fn handle(&self, effect: DiffNormalizeEffect) -> Normalized {
        let syntax = effect
            .language
            .as_deref()
            .map(|language| editor::diff::DiffSyntax {
                language,
                base: effect
                    .base_tree
                    .as_ref()
                    .map(|(tree, fresh)| editor::diff::DiffTree {
                        tree: tree.as_ref(),
                        fresh: *fresh,
                    }),
                target: effect
                    .target_tree
                    .as_ref()
                    .map(|(tree, fresh)| editor::diff::DiffTree {
                        tree: tree.as_ref(),
                        fresh: *fresh,
                    }),
            });
        let operation = effect
            .policy
            .diff(&effect.base_text, &effect.target_text, syntax.as_ref());
        let markup = editor::diff::hunk_markup(&operation, &effect.target_text);
        let changed = editor::set_diff(effect.previous.as_ref(), &markup);
        Normalized {
            diff: effect.diff,
            operation,
            markup,
            changed,
            base_revision: effect.base_revision,
            target_revision: effect.target_revision,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::test_document::plain_document;

    fn located(name: &str) -> editor::ResourceLocation {
        editor::ResourceLocation::new(
            editor::ResourceType::document(),
            editor::Authority::new("local"),
            vec![name.to_owned()],
        )
    }

    #[allow(dead_code)]
    enum Landed {
        Located(DocumentId, Option<editor::ResourceLocation>),
        Normalized(Normalized),
    }

    fn launches(batch: imba::effect::Batch<Landed>) -> usize {
        batch
            .drain()
            .into_iter()
            .filter(|message| {
                matches!(
                    message,
                    imba::effect::Message::Launch(..) | imba::effect::Message::Relaunch(..)
                )
            })
            .count()
    }

    #[test]
    fn the_base_chain_tracks_and_the_release_unwinds() {
        let ui = ::editor::test_document::test_ui();
        let mut store = Store::new();
        let documents = imba::store::Id::mint();
        let target = OpenDocuments::register(
            &mut store,
            documents,
            plain_document("one\nTWO\n"),
            Some(located("work.md")),
            "work.md".to_owned(),
            0,
        );

        let mut landed: Vec<(DocumentId, Option<editor::ResourceLocation>)> = Vec::new();
        let mut quiet = imba::effect::Batch::<Landed>::new();
        sync_stripe_bases(
            &mut store,
            documents,
            &mut quiet.effects(),
            |_, document, base, _| landed.push((document, base)),
        );
        assert_eq!(landed.len(), 0, "no resolver installed, no ask");
        StripeBases::install(
            &mut store,
            std::sync::Arc::new(
                |_: &Store,
                 _: imba::store::Id<OpenDocuments>,
                 location: &editor::ResourceLocation| {
                    Some(editor::ResourceLocation::new(
                        editor::ResourceType::document(),
                        location.authority().clone(),
                        vec![format!("{}@abc123", location.path().join("/"))],
                    ))
                },
            ),
        );
        let mut first = imba::effect::Batch::<Landed>::new();
        sync_stripe_bases(
            &mut store,
            documents,
            &mut first.effects(),
            |_, document, base, _| landed.push((document, base)),
        );
        assert_eq!(landed.len(), 1, "one ask for the located document");
        assert_eq!(
            landed[0],
            (target, Some(located("work.md@abc123"))),
            "the resolver answered at launch, synchronously"
        );
        let mut again = imba::effect::Batch::<Landed>::new();
        sync_stripe_bases(
            &mut store,
            documents,
            &mut again.effects(),
            |_, document, base, _| landed.push((document, base)),
        );
        assert_eq!(landed.len(), 1, "asked once per open");

        let base_location = located("work.md@abc123");
        assert_eq!(
            adopt_base_location(
                &mut store,
                documents,
                ui,
                target,
                Some(base_location.clone()),
                &mut imba::effect::Batch::<crate::DocumentsCommand>::new().effects()
            ),
            Some(base_location.clone()),
            "the cold base still needs its text fetched"
        );

        land_base_built(
            &mut store,
            documents,
            ui,
            target,
            base_location.clone(),
            plain_document("one\ntwo\n"),
            &mut imba::effect::Batch::<crate::DocumentsCommand>::new().effects(),
        );
        let handle = OpenDocuments::stripe_diff(&store, documents, target).expect("tracked");
        let base_id = handle.base;
        assert_eq!(
            OpenDocuments::location(&store, documents, base_id),
            Some(base_location.clone())
        );
        let base_entity = OpenDocuments::entity(&store, documents, base_id).expect("registered");
        assert_eq!(
            base_entity.saved_revision(),
            base_entity.document().revision(),
            "born clean"
        );
        assert!(
            base_entity.base_requested(),
            "a base is never asked for a base"
        );
        assert!(
            OpenDocuments::document_ref(&store, documents, target)
                .and_then(|document| document.diff(handle.id))
                .is_some(),
            "the entry rides the target"
        );

        assert_eq!(
            adopt_base_location(
                &mut store,
                documents,
                ui,
                target,
                Some(base_location.clone()),
                &mut imba::effect::Batch::<crate::DocumentsCommand>::new().effects()
            ),
            None,
            "nothing owed while the track stands"
        );
        assert_eq!(
            OpenDocuments::diff_refs(&store, documents, handle.id),
            Some(1)
        );

        let mut lanes = imba::effect::Batch::new();
        sync_diff_lanes(
            &mut store,
            documents,
            &mut lanes.effects(),
            Landed::Normalized,
        );
        assert_eq!(launches(lanes), 1, "the first normalization launches");

        OpenDocuments::remove_if_editorless(
            &mut store,
            documents,
            ui,
            target,
            &mut imba::effect::Batch::<crate::DocumentsCommand>::new().effects(),
        );
        assert!(
            !OpenDocuments::contains(&store, documents, target),
            "the target released"
        );
        assert!(
            !OpenDocuments::contains(&store, documents, base_id),
            "the base released with the record"
        );
        assert!(OpenDocuments::diff_handle(&store, documents, handle.id).is_none());
    }
}

#[cfg(test)]
mod perf_tests {
    use super::*;
    use editor::test_document::plain_document;

    /// PERF REGRESSION (docs/perf-issue.md §1): the batch-tail sweeps
    /// run per input event, so their cost must scale with the
    /// documents TOUCHED that batch — one, on a scroll tick — never
    /// with how many diffs/documents the registry tracks (a skia-sized
    /// status diff accumulates thousands). Before the dirty queues the
    /// diff lane walked every record and copied syntax trees per diff
    /// per tick: 30 fps.
    #[test]
    fn batch_tail_sweep_cost_is_tracked_count_independent() {
        let sweep_median_ms = |pairs: usize| -> f64 {
            let mut store = Store::new();
            let documents = imba::store::Id::mint();
            let mut targets = Vec::new();
            for n in 0..pairs {
                let base = OpenDocuments::register(
                    &mut store,
                    documents,
                    plain_document(&format!("base {n}\nsame\n")),
                    Some(editor::ResourceLocation::new(
                        editor::ResourceType::document(),
                        editor::Authority::new("local"),
                        vec![format!("file{n}.md.base")],
                    )),
                    format!("file{n}.md.base"),
                    0,
                );
                let target = OpenDocuments::register(
                    &mut store,
                    documents,
                    plain_document(&format!("target {n}\nsame\n")),
                    Some(editor::ResourceLocation::new(
                        editor::ResourceType::document(),
                        editor::Authority::new("local"),
                        vec![format!("file{n}.md")],
                    )),
                    format!("file{n}.md"),
                    0,
                );
                let tracked = OpenDocuments::track_diff(&mut store, documents, base, target, false);
                assert!(tracked.is_some());
                targets.push(target);
            }

            // The landing sweep: every registration is queued, every
            // diff normalizes once — drained here, off the clock.
            let mut warmup = imba::effect::Batch::<()>::new();
            sync_diff_lanes(&mut store, documents, &mut warmup.effects(), |_| ());
            crate::scroll_stripes::sync_scroll_stripe_lanes(
                &mut store,
                documents,
                &mut warmup.effects(),
                |_, _| (),
            );
            let _ = OpenDocuments::take_stale_view_candidates(&mut store, documents);

            // The per-tick shape: ONE document written, then the tail.
            let touched = targets[pairs / 2];
            let mut times = Vec::new();
            for _ in 0..30 {
                OpenDocuments::update_entity(&mut store, documents, touched, |_| {});
                let mut batch = imba::effect::Batch::<()>::new();
                let started = std::time::Instant::now();
                sync_diff_lanes(&mut store, documents, &mut batch.effects(), |_| ());
                crate::scroll_stripes::sync_scroll_stripe_lanes(
                    &mut store,
                    documents,
                    &mut batch.effects(),
                    |_, _| (),
                );
                let _ = OpenDocuments::take_stale_view_candidates(&mut store, documents);
                times.push(started.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            times[times.len() / 2]
        };

        let small = sweep_median_ms(50);
        let large = sweep_median_ms(3000);
        eprintln!("[perf] batch-tail sweep median: 50 pairs {small:.4}ms, 3000 pairs {large:.4}ms");
        // The old sweep walked every record per tick — 30x the pairs
        // measured way past this bound; machine speed cancels out.
        assert!(
            large < (small * 3.0).max(0.05),
            "the batch-tail sweeps must cost O(touched), not O(tracked): \
             50 pairs {small:.4}ms vs 3000 pairs {large:.4}ms"
        );
    }
}
