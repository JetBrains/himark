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
    /// The pane's own right-half extras entry (word tints + fold
    /// strips) — editor-owned, dying with the right half's editor.
    /// THE diff markup (hunk washes) is the entry's own
    /// (`Diff::markup`), never the pane's to write.
    pub right_extras: editor::MarkupId,
    pub state: Option<editor::DiffState>,
}

#[derive(Clone, Default)]
pub(crate) struct Diffs {
    records: rpds::HashTrieMapSync<DiffId, DiffRecord>,

    diff_views: rpds::HashTrieMapSync<u64, DiffView>,

    by_target: rpds::HashTrieMapSync<DocumentId, Vec<DiffId>>,

    by_base: rpds::HashTrieMapSync<DocumentId, Vec<DiffId>>,
}

impl Diffs {
    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty() && self.diff_views.is_empty()
    }

    pub(crate) fn record(&self, id: DiffId) -> Option<&DiffRecord> {
        self.records.get(&id)
    }

    pub(crate) fn ids(&self) -> Vec<DiffId> {
        self.records.keys().copied().collect()
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
    pub fn put_diff_view(store: &mut Store, id: DiffViewId, pair: DiffView) {
        store.update::<OpenDocuments>(|docs| {
            docs.diffs.diff_views.insert_mut(id.0, pair);
        });
    }

    pub fn take_diff_view(store: &mut Store, id: DiffViewId) -> Option<DiffView> {
        let pair = Self::diff_view_ref(store, id).cloned();
        if pair.is_some() {
            Self::remove_diff_view(store, id);
        }
        pair
    }

    pub fn diff_view_ref(store: &Store, id: DiffViewId) -> Option<&DiffView> {
        store
            .get::<OpenDocuments>()
            .and_then(|docs| docs.diffs.diff_views.get(&id.0))
    }

    pub fn remove_diff_view(store: &mut Store, id: DiffViewId) {
        store.update::<OpenDocuments>(|docs| {
            docs.diffs.diff_views.remove_mut(&id.0);
        });
    }

    pub fn pair_ids(store: &Store) -> Vec<DiffViewId> {
        store
            .get::<OpenDocuments>()
            .map(|docs| {
                docs.diffs
                    .diff_views
                    .keys()
                    .map(|id| DiffViewId(*id))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn pair_tracked(store: &Store, base: DocumentId, target: DocumentId) -> bool {
        store
            .get::<OpenDocuments>()
            .is_some_and(|docs| docs.diffs.by_pair(base, target).is_some())
    }

    pub fn track_diff(
        store: &mut Store,
        base: DocumentId,
        target: DocumentId,
        stripes: bool,
        prepared: Option<Operation>,
    ) -> Option<DiffId> {
        let mut result = None;
        let policy = editor::env::Differ::of(store);
        store.update::<OpenDocuments>(|docs| {
            if let Some(existing) = docs.diffs.by_pair(base, target) {
                let mut record = docs.diffs.record(existing).expect("indexed").clone();
                record.refs += 1;
                // The pair takes the stripes role it didn't have:
                // its markup steps onto the enabled tracks.
                if stripes && !record.stripes {
                    if let Some(target_entity) = docs.entries.get(&target) {
                        if let Some(markup) = target_entity
                            .document
                            .diff(existing)
                            .map(|entry| entry.markup())
                        {
                            let mut target_document = target_entity.document.clone();
                            target_document.mark_scroll_stripes_on_enabled(markup);
                            let mut entity = target_entity.clone();
                            entity.document = target_document;
                            docs.entries.insert_mut(target, entity);
                        }
                    }
                }
                record.stripes |= stripes;
                docs.diffs.put(existing, record);
                result = Some(existing);
                return;
            }
            let Some(base_entity) = docs.entries.get(&base) else {
                return;
            };
            let base_text = base_entity.document.text().clone();
            let base_revision = base_entity.document.revision();
            let Some(target_entity) = docs.entries.get(&target) else {
                return;
            };
            let target_revision = target_entity.document.revision();
            if let Some(operation) = &prepared {
                debug_assert_eq!(operation.old_len() as usize, base_text.byte_count());
            }
            let operation = match prepared.clone() {
                Some(prepared) => prepared,
                None => policy.diff(&base_text, target_entity.document.text(), None),
            };
            let normalized_at_birth = prepared.is_some();
            let mut target_document = target_entity.document.clone();
            let id = target_document.add_diff(operation.clone(), base_revision);
            if normalized_at_birth {
                target_document.install_normalized_diff(id, operation, base_revision);
            }
            // THE stripes diff registers on the enabled tracks; a
            // panel's diff (stripes=false) stays off them.
            if stripes {
                if let Some(markup) = target_document.diff(id).map(|entry| entry.markup()) {
                    target_document.mark_scroll_stripes_on_enabled(markup);
                }
            }
            let mut entity = target_entity.clone();
            entity.document = target_document;
            docs.entries.insert_mut(target, entity);

            let base_entity = docs.entries.get(&base).expect("checked above");
            let mut base_document = base_entity.document.clone();
            let base_markup = base_document.add_markup();
            let mut entity = base_entity.clone();
            entity.document = base_document;
            docs.entries.insert_mut(base, entity);

            docs.diffs.insert(
                id,
                DiffRecord {
                    base,
                    target,
                    base_markup,
                    refs: 1,
                    stripes,
                    normalize_token: None,
                    normalized: prepared
                        .is_some()
                        .then_some((base_revision, target_revision)),
                },
            );
            if probe() {
                eprintln!("[diffs] tracked {id:?}: {base:?} -> {target:?} stripes={stripes}");
            }
            result = Some(id);
        });
        result
    }

    pub fn untrack_diff<R: 'static>(
        store: &mut Store,
        ui: &imba::UiCtx,
        id: DiffId,
        fx: &mut imba::effect::Effects<'_, R>,
    ) {
        let fonts = editor::env::Fonts::of(store)();
        let theme = editor::env::Themes::of(store);
        let mut released: Option<DiffRecord> = None;
        store.update::<OpenDocuments>(|docs| {
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

        if let Some(mut document) = Self::document(store, record.target) {
            document.remove_diff(
                id,
                &[],
                store,
                ui,
                &fonts,
                &theme,
                &mut imba::effect::Batch::new().effects(),
            );
            Self::put_document(store, record.target, document);
        }
        if let Some(mut document) = Self::document(store, record.base) {
            document.remove_markup(
                record.base_markup,
                &[],
                store,
                ui,
                &fonts,
                &theme,
                &mut imba::effect::Batch::new().effects(),
            );
            Self::put_document(store, record.base, document);
        }
        Self::remove_if_editorless(store, ui, record.base, fx);
        Self::remove_if_editorless(store, ui, record.target, fx);
    }

    pub fn diff_handle(store: &Store, id: DiffId) -> Option<DiffHandle> {
        let docs = store.get::<OpenDocuments>()?;
        let record = docs.diffs.record(id)?;
        Some(DiffHandle {
            id,
            base: record.base,
            target: record.target,
            base_markup: record.base_markup,
        })
    }

    pub fn stripe_diff(store: &Store, target: DocumentId) -> Option<DiffHandle> {
        let docs = store.get::<OpenDocuments>()?;
        Self::diff_handle(store, docs.diffs.stripe_of(target)?)
    }

    pub(crate) fn untrack_stripes<R: 'static>(
        store: &mut Store,
        ui: &imba::UiCtx,
        document: DocumentId,
        fx: &mut imba::effect::Effects<'_, R>,
    ) -> bool {
        let Some(id) = store
            .get::<OpenDocuments>()
            .and_then(|docs| docs.diffs.stripe_of(document))
        else {
            return false;
        };
        store.update::<OpenDocuments>(|docs| {
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
                }
            }
        });
        Self::untrack_diff(store, ui, id, fx);
        true
    }

    #[doc(hidden)]
    pub fn diff_refs(store: &Store, id: DiffId) -> Option<u32> {
        Some(store.get::<OpenDocuments>()?.diffs.record(id)?.refs)
    }
}

pub fn sync_diff_lanes<R: 'static>(
    store: &mut Store,
    fx: &mut imba::effect::Effects<'_, R>,
    wrap: impl Fn(Normalized) -> R + Send + Clone + 'static,
) {
    if store
        .get::<OpenDocuments>()
        .is_none_or(|docs| docs.diffs.is_empty())
    {
        return;
    }
    let policy = editor::env::Differ::of(store);
    store.update::<OpenDocuments>(|docs| {
        for id in docs.diffs.ids() {
            let Some(mut record) = docs.diffs.record(id).cloned() else {
                continue;
            };
            let Some(base_entity) = docs.entries.get(&record.base) else {
                continue;
            };
            let base_revision = base_entity.document.revision();
            let base_log = base_entity.document.log().clone();
            let base_text = base_entity.document.text().clone();
            let base_syntax = syntax_snapshot(&base_entity.document);
            let Some(target_entity) = docs.entries.get(&record.target) else {
                continue;
            };

            let cursor = target_entity
                .document
                .diff(id)
                .map(|entry| entry.base_revision());
            let mut force_normalize = false;
            if cursor.is_some_and(|cursor| cursor != base_revision) {
                let mut document = target_entity.document.clone();
                match document.apply_diff_base_edits(id, &base_log) {
                    true => {
                        let mut entity = target_entity.clone();
                        entity.document = document;
                        docs.entries.insert_mut(record.target, entity);
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
                let target_syntax = syntax_snapshot(&target_entity.document);
                let language = target_syntax.as_ref().map(|(language, _)| language.clone());
                let base_tree = base_syntax
                    .filter(|(base_language, _)| Some(base_language) == language.as_ref())
                    .map(|(_, tree)| tree);
                let effect = DiffNormalizeEffect {
                    diff: id,
                    base_text: base_text.clone(),
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
                    target_tree: target_syntax.map(|(_, tree)| tree),
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
    id: DiffId,
    minimal: Operation,
    base_revision: u64,
    target_revision: u64,
) -> bool {
    let mut landed = false;
    store.update::<OpenDocuments>(|docs| {
        let Some(record) = docs.diffs.record(id).cloned() else {
            return;
        };
        let Some(base_entity) = docs.entries.get(&record.base) else {
            return;
        };
        let a = base_entity.document.log().compose_since(base_revision);
        let base_now = base_entity.document.revision();
        let Some(target_entity) = docs.entries.get(&record.target) else {
            return;
        };
        let b = target_entity.document.log().compose_since(target_revision);
        let mut rebased = minimal.clone();
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
            docs.entries.insert_mut(record.target, entity);
            landed = true;
        }
    });
    if probe() {
        eprintln!("[diffs] normalization landed={landed} for {id:?}");
    }
    landed
}

/// Lands a normalize run's freshly derived diff markup on the target
/// document — through the entity's own effects scope, so the swap's
/// repair tails route home like any landing's.
pub fn land_diff_markup(
    store: &mut Store,
    ui: &imba::UiCtx,
    id: DiffId,
    markup: editor::Markup,
    changed: Vec<std::ops::Range<u32>>,
    derived_at: u64,
    fx: &mut editor::EditorEffects<'_>,
) {
    let Some(record) = store
        .get::<OpenDocuments>()
        .and_then(|docs| docs.diffs.record(id).cloned())
    else {
        return;
    };
    let fonts = editor::env::Fonts::of(store)();
    let theme = editor::env::Themes::of(store);
    let Some(mut document) = OpenDocuments::document(store, record.target) else {
        return;
    };
    document.install_diff_markup(
        id, markup, changed, derived_at, store, ui, &fonts, &theme, fx,
    );
    OpenDocuments::put_document(store, record.target, document);
}

pub struct DiffChanged {
    pub diff: DiffId,
}

pub struct FetchBaseEffect {
    pub location: editor::ResourceLocation,
}

impl Effect for FetchBaseEffect {
    type Result = Option<editor::ResourceLocation>;
}

#[derive(Clone, Default)]
pub struct StripeBases;

impl StripeBases {
    pub fn install(store: &mut Store) {
        store.put(StripeBases);
    }

    pub fn installed(store: &Store) -> bool {
        store.get::<StripeBases>().is_some()
    }
}

pub fn sync_stripe_bases<R: 'static>(
    store: &mut Store,
    fx: &mut imba::effect::Effects<'_, R>,
    wrap: impl Fn(DocumentId, Option<editor::ResourceLocation>) -> R + Send + Clone + 'static,
) {
    if !StripeBases::installed(store) {
        return;
    }
    for (document, entity) in OpenDocuments::list(store) {
        if entity.base_requested() {
            continue;
        }
        let Some(location) = entity.location().cloned() else {
            continue;
        };
        if crate::is_synthetic(&location) {
            continue;
        }
        OpenDocuments::set_base_requested(store, document);
        if probe() {
            eprintln!("[diffs] base ask for /{}", location.path().join("/"));
        }
        let wrap = wrap.clone();
        let _ = fx.push(
            imba::effect::AnyEffect::new(FetchBaseEffect { location })
                .map(move |base| wrap(document, base)),
        );
    }
}

pub fn rearm_base_asks(store: &mut Store, matches: &dyn Fn(&editor::ResourceLocation) -> bool) {
    let rearm: Vec<crate::DocumentId> = OpenDocuments::list(store)
        .into_iter()
        .filter(|(_, entity)| {
            entity.base_requested() && entity.location().is_some_and(|location| matches(location))
        })
        .map(|(document, _)| document)
        .collect();
    for document in rearm {
        OpenDocuments::update_entity(store, document, |entity| entity.base_requested = false);
    }
}

pub fn adopt_base_location<R: 'static>(
    store: &mut Store,
    ui: &imba::UiCtx,
    document: crate::DocumentId,
    base: Option<editor::ResourceLocation>,
    fx: &mut imba::effect::Effects<'_, R>,
) -> Option<editor::ResourceLocation> {
    if !OpenDocuments::contains(store, document) {
        return None;
    }

    if let Some(handle) = OpenDocuments::stripe_diff(store, document) {
        if OpenDocuments::location(store, handle.base).as_ref() == base.as_ref() {
            return None;
        }
        OpenDocuments::untrack_stripes(store, ui, document, fx);

        if !OpenDocuments::contains(store, document) {
            return None;
        }
    }
    let base = base?;
    if let Some(base_id) = OpenDocuments::by_location(store, &base) {
        let _ = OpenDocuments::track_diff(store, base_id, document, true, None);
        return None;
    }
    Some(base)
}

pub fn land_base_built<R: 'static>(
    store: &mut Store,
    ui: &imba::UiCtx,
    document: crate::DocumentId,
    base: editor::ResourceLocation,
    built: editor::Document,
    fx: &mut imba::effect::Effects<'_, R>,
) {
    if !OpenDocuments::contains(store, document) {
        return;
    }
    if let Some(handle) = OpenDocuments::stripe_diff(store, document) {
        if OpenDocuments::location(store, handle.base).as_ref() == Some(&base) {
            return;
        }
        OpenDocuments::untrack_stripes(store, ui, document, fx);
        if !OpenDocuments::contains(store, document) {
            return;
        }
    }
    let base_id = match OpenDocuments::by_location(store, &base) {
        Some(existing) => existing,
        None => {
            let saved = built.revision();
            let name = base.name().to_owned();
            let id = OpenDocuments::register(store, built, Some(base), name, saved);

            OpenDocuments::set_base_requested(store, id);
            id
        }
    };
    let tracked = OpenDocuments::track_diff(store, base_id, document, true, None);
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
    /// Target's language when its tree was fresh at capture — the
    /// policy's cue to try structural alignment.
    pub(crate) language: Option<String>,
    pub(crate) base_tree: Option<Box<dyn editor::SyntaxTree>>,
    pub(crate) target_tree: Option<Box<dyn editor::SyntaxTree>>,
    /// The edge-installed policy (`editor::env::Differ`), captured at
    /// launch so the handler needs no store access.
    pub(crate) policy: std::sync::Arc<dyn editor::diff::DiffPolicy>,
}

/// Language + deep tree clone, only when the tree matches the text
/// exactly (no edits since the last completed reparse) — a stale tree
/// would misalign the structural pass into pure fallback work.
fn syntax_snapshot(document: &editor::Document) -> Option<(String, Box<dyn editor::SyntaxTree>)> {
    if !document.edited_since_parse().is_empty() {
        return None;
    }
    let syntax = document.syntax()?;
    let tree = syntax.tree.as_ref()?.clone_tree();
    Some((syntax.language.clone(), tree))
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
                base_tree: effect.base_tree.as_deref(),
                target_tree: effect.target_tree.as_deref(),
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
        let ui = &imba::UiCtx::dont_use_too_slow();
        let mut store = Store::new();
        let target = OpenDocuments::register(
            &mut store,
            plain_document("one\nTWO\n"),
            Some(located("work.md")),
            "work.md".to_owned(),
            0,
        );

        let mut quiet = imba::effect::Batch::new();
        sync_stripe_bases(&mut store, &mut quiet.effects(), Landed::Located);
        assert_eq!(launches(quiet), 0, "no marker, no ask");
        StripeBases::install(&mut store);
        let mut first = imba::effect::Batch::new();
        sync_stripe_bases(&mut store, &mut first.effects(), Landed::Located);
        assert_eq!(launches(first), 1, "one ask for the located document");
        let mut again = imba::effect::Batch::new();
        sync_stripe_bases(&mut store, &mut again.effects(), Landed::Located);
        assert_eq!(launches(again), 0, "asked once per open");

        let base_location = located("work.md@abc123");
        assert_eq!(
            adopt_base_location(
                &mut store,
                ui,
                target,
                Some(base_location.clone()),
                &mut imba::effect::Batch::<()>::new().effects()
            ),
            Some(base_location.clone()),
            "the cold base still needs its text fetched"
        );

        land_base_built(
            &mut store,
            ui,
            target,
            base_location.clone(),
            plain_document("one\ntwo\n"),
            &mut imba::effect::Batch::<()>::new().effects(),
        );
        let handle = OpenDocuments::stripe_diff(&store, target).expect("tracked");
        let base_id = handle.base;
        assert_eq!(
            OpenDocuments::location(&store, base_id),
            Some(base_location.clone())
        );
        let base_entity = OpenDocuments::entity(&store, base_id).expect("registered");
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
            OpenDocuments::document_ref(&store, target)
                .and_then(|document| document.diff(handle.id))
                .is_some(),
            "the entry rides the target"
        );

        assert_eq!(
            adopt_base_location(
                &mut store,
                ui,
                target,
                Some(base_location.clone()),
                &mut imba::effect::Batch::<()>::new().effects()
            ),
            None,
            "nothing owed while the track stands"
        );
        assert_eq!(OpenDocuments::diff_refs(&store, handle.id), Some(1));

        let mut lanes = imba::effect::Batch::new();
        sync_diff_lanes(&mut store, &mut lanes.effects(), Landed::Normalized);
        assert_eq!(launches(lanes), 1, "the first normalization launches");

        OpenDocuments::remove_if_editorless(
            &mut store,
            ui,
            target,
            &mut imba::effect::Batch::<()>::new().effects(),
        );
        assert!(
            !OpenDocuments::contains(&store, target),
            "the target released"
        );
        assert!(
            !OpenDocuments::contains(&store, base_id),
            "the base released with the record"
        );
        assert!(OpenDocuments::diff_handle(&store, handle.id).is_none());
    }

    #[test]
    fn a_prepared_track_is_normalized_at_birth() {
        let ui = &imba::UiCtx::dont_use_too_slow();
        let mut store = Store::new();
        let base_id = OpenDocuments::register(
            &mut store,
            plain_document("one\ntwo\n"),
            None,
            "base".to_owned(),
            0,
        );
        let target_id = OpenDocuments::register(
            &mut store,
            plain_document("one\nTWO\n"),
            None,
            "target".to_owned(),
            0,
        );
        let prepared = {
            let base = OpenDocuments::document_ref(&store, base_id).expect("registered");
            let target = OpenDocuments::document_ref(&store, target_id).expect("registered");
            myersdiff::diff(base.text(), target.text())
        };
        let id = OpenDocuments::track_diff(&mut store, base_id, target_id, false, Some(prepared))
            .expect("both registered");

        let generation = OpenDocuments::document_ref(&store, target_id)
            .and_then(|document| document.diff(id).map(|entry| entry.generation()))
            .expect("the entry rides the target");
        assert_eq!(generation, 1, "normalized at birth");

        let mut lanes = imba::effect::Batch::new();
        sync_diff_lanes(&mut store, &mut lanes.effects(), Landed::Normalized);
        assert_eq!(launches(lanes), 0, "nothing owed at birth");

        let fonts = ::editor::embedded_fonts::source()();
        let theme = ::editor::theme::Theme::embedded();
        let mut document = OpenDocuments::document(&store, target_id).expect("registered");
        let editor = document.add_editor(
            400.0,
            None,
            ::editor::EditorBuild::Complete,
            &[],
            &store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
        document.insert(
            editor,
            "typed",
            &store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
        OpenDocuments::put_document(&mut store, target_id, document);
        let mut lanes = imba::effect::Batch::new();
        sync_diff_lanes(&mut store, &mut lanes.effects(), Landed::Normalized);
        assert_eq!(
            launches(lanes),
            1,
            "the edit owes exactly one normalization"
        );
    }
}
