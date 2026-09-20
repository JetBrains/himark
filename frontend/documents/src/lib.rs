// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::Document;
use imba::store::Store;

pub mod change;
pub mod diffs;
mod entity_view;
mod lifecycle;
pub mod scroll_stripes;
pub mod sync;
pub mod watch;

pub use change::{
    line_col_at, notify_change, offset_at, ChangeObserver, DocumentChangeEffect, FolderSource,
    LineCol, TextChange,
};
pub use diffs::{
    rearm_base_asks, DiffChanged, DiffHandle, DiffNormalizeEffect, DiffNormalizeHandler, DiffView,
    DiffViewId, FetchBaseEffect, Normalized, StripeBases,
};
pub use entity_view::EditorIdView;
pub use lifecycle::{close_editor, deliver, mount_editor};
pub use watch::{
    FileChanged, FilesChanged, SubscribeEffect, Subscription, UnsubscribeEffect, Watching,
};

pub struct FetchDocumentEffect {
    pub location: editor::ResourceLocation,
}

impl imba::effect::Effect for FetchDocumentEffect {
    type Result = Option<String>;
}

pub struct FetchResourceBytesEffect {
    pub origin: editor::ResourceLocation,

    pub reference: String,
}

impl imba::effect::Effect for FetchResourceBytesEffect {
    type Result = Option<Vec<u8>>;
}

#[derive(Clone, Default)]
pub struct ScratchMint(u64);

impl ScratchMint {
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
}

pub fn next_scratch_location(store: &mut Store) -> editor::ResourceLocation {
    let mut minted = 0;
    store.update::<ScratchMint>(|mint| {
        mint.0 += 1;
        minted = mint.0;
    });
    let name = match minted {
        1 => "scratch".to_owned(),
        n => format!("scratch {n}"),
    };
    editor::ResourceLocation::new(
        editor::ResourceType::document(),
        editor::Authority::new("scratch"),
        vec![name],
    )
}

pub fn is_scratch(location: &editor::ResourceLocation) -> bool {
    location.authority().as_str() == "scratch"
}

pub fn is_synthetic(location: &editor::ResourceLocation) -> bool {
    location.is_synthetic()
}

impl OpenDocuments {
    pub fn set_location(
        store: &mut Store,
        document: DocumentId,
        location: editor::ResourceLocation,
    ) {
        store.update::<OpenDocuments>(|documents| {
            let Some(entity) = documents.entries.get(&document) else {
                return;
            };
            let mut entity = entity.clone();
            if let Some(old) = entity.watch {
                documents.unindex_watch(document, old);
            }
            if let Some(old) = &entity.location {
                documents.by_location.remove_mut(old);
            }
            documents.by_location.insert_mut(location.clone(), document);
            entity.location = Some(location);
            entity.watch = None;
            entity.watch_requested = false;
            documents.entries.insert_mut(document, entity);
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DocumentId(u64);

impl DocumentId {
    #[doc(hidden)]
    pub fn raw(self) -> u64 {
        self.0
    }

    #[doc(hidden)]
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Clone)]
pub struct OpenDocument {
    pub(crate) document: Document,

    pub(crate) location: Option<editor::ResourceLocation>,

    pub(crate) title: String,

    pub(crate) registered: u64,

    pub(crate) last_opened: u64,

    pub(crate) saved_revision: u64,

    pub(crate) baseline: editor::Text,

    pub(crate) refetch_serial: u64,

    pub(crate) save_token: Option<imba::effect::CancellationToken>,

    pub(crate) watch: Option<crate::Subscription>,

    pub(crate) watch_requested: bool,

    /// A live document channel makes the HOST the source of truth:
    /// it reloads the file itself and broadcasts its own edits. While
    /// set, this client neither watches the file nor absorbs its own
    /// refetches (docs: agents edit files, clients edit documents).
    pub(crate) host_synced: bool,

    pub(crate) base_requested: bool,
}

impl OpenDocument {
    pub fn name(&self) -> String {
        match &self.location {
            Some(location) => location.name().to_owned(),
            None => self.title.clone(),
        }
    }

    pub fn location(&self) -> Option<&editor::ResourceLocation> {
        self.location.as_ref()
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn saved_revision(&self) -> u64 {
        self.saved_revision
    }

    /// Whether edits stand that no store has landed for.
    pub fn modified(&self) -> bool {
        self.document.revision() != self.saved_revision
    }

    pub fn baseline(&self) -> &editor::Text {
        &self.baseline
    }

    pub fn refetch_serial(&self) -> u64 {
        self.refetch_serial
    }

    pub fn watch(&self) -> Option<crate::Subscription> {
        self.watch
    }

    pub fn watch_requested(&self) -> bool {
        self.watch_requested
    }

    pub fn base_requested(&self) -> bool {
        self.base_requested
    }

    pub fn save_token(&self) -> Option<imba::effect::CancellationToken> {
        self.save_token
    }
}

pub trait DocumentHook: Send + Sync {
    fn opened(&self, store: &mut imba::store::Store, document: DocumentId);
    fn closing(&self, store: &mut imba::store::Store, document: DocumentId);
}

#[derive(Clone, Default)]
pub struct OpenDocuments {
    pub(crate) entries: rpds::HashTrieMapSync<DocumentId, OpenDocument>,

    by_location: rpds::HashTrieMapSync<editor::ResourceLocation, DocumentId>,

    by_watch: rpds::HashTrieMapSync<crate::Subscription, rpds::VectorSync<DocumentId>>,

    pub(crate) diffs: crate::diffs::Diffs,
}

#[derive(Clone, Default)]
pub(crate) struct DocumentMint(u64);

#[derive(Clone, Default)]
struct DocumentHooks(rpds::VectorSync<std::sync::Arc<dyn DocumentHook>>);

impl OpenDocuments {
    pub fn register(
        store: &mut Store,
        document: Document,
        location: Option<editor::ResourceLocation>,
        title: String,
        saved_revision: u64,
    ) -> DocumentId {
        let mut minted = 0;
        store.update::<DocumentMint>(|mint| {
            mint.0 += 1;
            minted = mint.0;
        });
        let id = DocumentId(minted);
        let mut documents = store.get::<OpenDocuments>().cloned().unwrap_or_default();
        let stamp = Self::next_stamp(&documents);
        if let Some(location) = &location {
            documents.by_location.insert_mut(location.clone(), id);
        }
        let baseline = document.text().clone();
        documents.entries.insert_mut(
            id,
            OpenDocument {
                document,
                location,
                title,
                registered: stamp,
                last_opened: stamp,
                saved_revision,
                baseline,
                refetch_serial: 0,
                save_token: None,
                watch: None,
                watch_requested: false,
                host_synced: false,
                base_requested: false,
            },
        );
        store.put(documents);
        for hook in Self::hooks(store).iter() {
            hook.opened(store, id);
        }
        id
    }

    pub fn install_hook(store: &mut Store, hook: std::sync::Arc<dyn DocumentHook>) {
        let mut hooks = store.get::<DocumentHooks>().cloned().unwrap_or_default();
        hooks.0.push_back_mut(hook);
        store.put(hooks);
    }

    fn hooks(store: &Store) -> rpds::VectorSync<std::sync::Arc<dyn DocumentHook>> {
        store
            .get::<DocumentHooks>()
            .map(|hooks| hooks.0.clone())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.diffs.is_empty()
    }

    pub fn contains_id(&self, document: DocumentId) -> bool {
        self.entries.contains_key(&document)
    }

    pub fn rides_watch(&self, subscription: crate::Subscription) -> bool {
        self.by_watch
            .get(&subscription)
            .is_some_and(|riders| !riders.is_empty())
    }

    pub fn watch_riders(&self, subscription: crate::Subscription) -> Vec<DocumentId> {
        self.by_watch
            .get(&subscription)
            .map(|riders| riders.iter().copied().collect())
            .unwrap_or_default()
    }

    fn index_watch(&mut self, document: DocumentId, subscription: crate::Subscription) {
        let mut riders = self
            .by_watch
            .get(&subscription)
            .cloned()
            .unwrap_or_default();
        if !riders.iter().any(|rider| *rider == document) {
            riders.push_back_mut(document);
        }
        self.by_watch.insert_mut(subscription, riders);
    }

    fn unindex_watch(&mut self, document: DocumentId, subscription: crate::Subscription) {
        let Some(riders) = self.by_watch.get(&subscription) else {
            return;
        };
        let kept: rpds::VectorSync<DocumentId> = riders
            .iter()
            .copied()
            .filter(|rider| *rider != document)
            .collect();
        match kept.is_empty() {
            true => {
                self.by_watch.remove_mut(&subscription);
            }
            false => {
                self.by_watch.insert_mut(subscription, kept);
            }
        }
    }

    pub fn tracks_diff(&self, diff: ::editor::diff::DiffId) -> bool {
        self.diffs.record(diff).is_some()
    }

    pub fn document(store: &Store, id: DocumentId) -> Option<Document> {
        Self::document_ref(store, id).cloned()
    }

    pub fn document_ref(store: &Store, id: DocumentId) -> Option<&Document> {
        store
            .get::<OpenDocuments>()?
            .entries
            .get(&id)
            .map(|entity| &entity.document)
    }

    pub fn put_document(store: &mut Store, id: DocumentId, document: Document) {
        Self::update_entity(store, id, |entity| entity.document = document);
    }

    pub fn contains(store: &Store, id: DocumentId) -> bool {
        Self::document_ref(store, id).is_some()
    }

    pub fn list(store: &Store) -> Vec<(DocumentId, OpenDocument)> {
        let mut entries: Vec<(DocumentId, OpenDocument)> = store
            .get::<OpenDocuments>()
            .map(|documents| {
                documents
                    .entries
                    .iter()
                    .map(|(id, entity)| (*id, entity.clone()))
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by_key(|(_, entity)| entity.registered);
        entries
    }

    pub fn list_recent(store: &Store) -> Vec<(DocumentId, OpenDocument)> {
        let mut entries = Self::list(store);
        entries.sort_by(|(_, a), (_, b)| b.last_opened.cmp(&a.last_opened));
        entries
    }

    pub fn name(store: &Store, document: DocumentId) -> Option<String> {
        Self::entity(store, document).map(|entity| entity.name())
    }

    pub fn location(store: &Store, document: DocumentId) -> Option<editor::ResourceLocation> {
        store
            .get::<OpenDocuments>()?
            .entries
            .get(&document)?
            .location
            .clone()
    }

    pub fn entity(store: &Store, document: DocumentId) -> Option<OpenDocument> {
        store
            .get::<OpenDocuments>()?
            .entries
            .get(&document)
            .cloned()
    }

    pub fn by_location(store: &Store, location: &editor::ResourceLocation) -> Option<DocumentId> {
        store
            .get::<OpenDocuments>()?
            .by_location
            .get(location)
            .copied()
    }

    pub fn touch(store: &mut Store, document: DocumentId) {
        let Some(documents) = store.get::<OpenDocuments>() else {
            return;
        };
        let stamp = Self::next_stamp(documents);
        Self::update_entity(store, document, |entity| entity.last_opened = stamp);
    }

    pub fn set_save_token(
        store: &mut Store,
        document: DocumentId,
        token: Option<imba::effect::CancellationToken>,
    ) {
        Self::update_entity(store, document, |entity| entity.save_token = token);
    }

    /// The document channel went live (or died): while live, the
    /// HOST owns disk reloads and this client must not watch the
    /// file; on fallback the watch machinery re-arms and a refetch
    /// resyncs from disk.
    pub fn set_host_synced<R: 'static>(
        store: &mut Store,
        document: DocumentId,
        synced: bool,
        fx: &mut imba::effect::Effects<'_, R>,
    ) {
        let Some(entity) = Self::entity(store, document) else {
            return;
        };
        if entity.host_synced == synced {
            return;
        }
        if synced {
            if let Some(subscription) = entity.watch {
                let _ = fx.push(imba::effect::AnyEffect::notification(
                    crate::watch::UnsubscribeEffect { subscription },
                ));
            }
        }
        Self::update_entity(store, document, |entity| {
            entity.host_synced = synced;
            if synced {
                entity.watch = None;
            }
            // Either way the sweep decides afresh.
            entity.watch_requested = false;
        });
    }

    pub fn host_synced(store: &Store, document: DocumentId) -> bool {
        Self::entity(store, document).is_some_and(|entity| entity.host_synced)
    }

    pub fn set_watch_requested(store: &mut Store, document: DocumentId) {
        Self::update_entity(store, document, |entity| entity.watch_requested = true);
    }

    pub fn set_base_requested(store: &mut Store, document: DocumentId) {
        Self::update_entity(store, document, |entity| entity.base_requested = true);
    }

    pub fn set_watch(store: &mut Store, document: DocumentId, watch: Option<crate::Subscription>) {
        store.update::<OpenDocuments>(|documents| {
            let Some(entity) = documents.entries.get(&document) else {
                return;
            };
            let mut entity = entity.clone();
            let old = entity.watch;
            entity.watch = watch;
            documents.entries.insert_mut(document, entity);
            if old != watch {
                if let Some(old) = old {
                    documents.unindex_watch(document, old);
                }
                if let Some(new) = watch {
                    documents.index_watch(document, new);
                }
            }
        });
    }

    pub fn mark_saved(
        store: &mut Store,
        document: DocumentId,
        revision: u64,
        stored: editor::Text,
    ) {
        Self::update_entity(store, document, |entity| {
            entity.saved_revision = revision;
            entity.baseline = stored;
        });
    }

    pub fn edit_external(
        store: &mut Store,
        ui: &imba::UiCtx,
        document_id: DocumentId,
        base_revision: u64,
        operation: &operation::Operation,
        fx: &mut imba::effect::Effects<'_, editor::EditorCommand>,
    ) {
        let Some(mut document) = Self::document(store, document_id) else {
            return;
        };
        if document.revision() != base_revision {
            return;
        }
        if operation
            .iter()
            .all(|op| matches!(op, operation::Op::Retain(_)))
        {
            return;
        }
        let text_before = document.text().clone();
        let fonts = ::editor::env::Fonts::of(store)();
        let theme = ::editor::env::Themes::of(store);
        document.edit(operation, store, ui, &fonts, &theme, fx);
        if let Some(parsers) = ::editor::env::Parsers::of(store) {
            document.launch_reparse(parsers, fx);
        }
        document.clear_undo_history();
        if let Some(location) = Self::location(store, document_id) {
            for sink in ::editor::InstalledChangeSink::of(store) {
                sink.changed(store, &document, &location, base_revision, &text_before, fx);
            }
        }

        let revision = document.revision();
        let stored = document.text().clone();
        Self::put_document(store, document_id, document);
        Self::mark_saved(store, document_id, revision, stored);
    }

    pub fn edit_shared(
        store: &mut Store,
        ui: &imba::UiCtx,
        document_id: DocumentId,
        identity: ::editor::EditIdentity,
        base_revision: u64,
        operation: &operation::Operation,
        fx: &mut imba::effect::Effects<'_, editor::EditorCommand>,
    ) -> bool {
        let Some(mut document) = Self::document(store, document_id) else {
            return false;
        };
        if document.revision() != base_revision {
            return false;
        }
        if operation
            .iter()
            .all(|op| matches!(op, operation::Op::Retain(_)))
        {
            return false;
        }
        let text_before = document.text().clone();
        let fonts = ::editor::env::Fonts::of(store)();
        let theme = ::editor::env::Themes::of(store);
        document.edit_shared(identity, operation, store, ui, &fonts, &theme, fx);
        if let Some(parsers) = ::editor::env::Parsers::of(store) {
            document.launch_reparse(parsers, fx);
        }
        if let Some(location) = Self::location(store, document_id) {
            for sink in ::editor::InstalledChangeSink::of(store) {
                sink.changed(store, &document, &location, base_revision, &text_before, fx);
            }
        }
        Self::put_document(store, document_id, document);
        true
    }

    pub fn stamp_refetch(store: &mut Store, document: DocumentId) -> u64 {
        let mut stamped = 0;
        Self::update_entity(store, document, |entity| {
            entity.refetch_serial += 1;
            stamped = entity.refetch_serial;
        });
        stamped
    }

    /// Land a refetch's computed rebase. `true` back means the
    /// landing raced a fresher revision and the caller must RE-DIFF
    /// the kept disk text against the new buffer
    /// (`watch::rediff`, with the rebase's `fetched_source`) —
    /// dropping it silently would leave the document stale against
    /// the disk with nothing left to retry (the reload loop must
    /// converge, not give up).
    #[must_use]
    pub fn absorb_refetched(
        store: &mut Store,
        ui: &imba::UiCtx,
        document_id: DocumentId,
        base_revision: u64,
        serial: u64,
        operation: &operation::Operation,
        fetched: editor::Text,
        synced: bool,
        fx: &mut imba::effect::Effects<'_, editor::EditorCommand>,
    ) -> bool {
        let Some(entity) = Self::entity(store, document_id) else {
            return false;
        };
        // The channel went live while this landing was in flight: the
        // HOST owns disk truth now; a client-side merge would fight
        // its broadcasts.
        if entity.host_synced || entity.refetch_serial != serial {
            return false;
        }
        let Some(mut document) = Self::document(store, document_id) else {
            return false;
        };
        if document.revision() != base_revision {
            return true;
        }
        let moves = !operation
            .iter()
            .all(|op| matches!(op, operation::Op::Retain(_)));
        if moves {
            let text_before = document.text().clone();
            let fonts = ::editor::env::Fonts::of(store)();
            let theme = ::editor::env::Themes::of(store);
            document.edit(operation, store, ui, &fonts, &theme, fx);
            if let Some(parsers) = ::editor::env::Parsers::of(store) {
                document.launch_reparse(parsers, fx);
            }
            document.clear_undo_history();
            if let Some(location) = Self::location(store, document_id) {
                for sink in ::editor::InstalledChangeSink::of(store) {
                    sink.changed(store, &document, &location, base_revision, &text_before, fx);
                }
            }
        }
        let revision = document.revision();
        Self::put_document(store, document_id, document);
        // `synced` came from the WORKER (the merge target equals the
        // disk text): the landing brought the buffer exactly to the
        // disk — a save, whatever the diff route was. Never compare
        // texts here; this is the UI thread.
        match synced {
            true => Self::mark_saved(store, document_id, revision, fetched),
            false => Self::update_entity(store, document_id, |entity| entity.baseline = fetched),
        }
        false
    }

    pub fn remove_if_editorless<R: 'static>(
        store: &mut Store,
        ui: &imba::UiCtx,
        document: DocumentId,
        fx: &mut imba::effect::Effects<'_, R>,
    ) {
        Self::release_editorless(store, ui, document, fx, true)
    }

    pub fn remove_on_close<R: 'static>(
        store: &mut Store,
        ui: &imba::UiCtx,
        document: DocumentId,
        fx: &mut imba::effect::Effects<'_, R>,
    ) {
        Self::release_editorless(store, ui, document, fx, false)
    }

    fn release_editorless<R: 'static>(
        store: &mut Store,
        ui: &imba::UiCtx,
        document: DocumentId,
        fx: &mut imba::effect::Effects<'_, R>,
        spare_scratch: bool,
    ) {
        let Some(entity) = Self::entity(store, document) else {
            return;
        };
        if entity.document.editor_ids().next().is_some() {
            return;
        }
        if entity.modified() {
            return;
        }
        if spare_scratch && entity.location.as_ref().is_some_and(is_scratch) {
            return;
        }

        if Self::untrack_stripes(store, ui, document, fx) {
            return;
        }

        if store
            .get::<OpenDocuments>()
            .is_some_and(|docs| docs.diffs.touches(document))
        {
            return;
        }
        if let Some(subscription) = entity.watch {
            fx.notify(crate::UnsubscribeEffect { subscription });
        }

        {
            let mut document = entity.document.clone();
            document.release_enrichment(store);
        }

        for hook in Self::hooks(store).iter() {
            hook.closing(store, document);
        }
        store.update::<OpenDocuments>(|documents| {
            let (watch, location) = match documents.entries.get(&document) {
                Some(entity) => (entity.watch, entity.location.clone()),
                None => (None, None),
            };
            if let Some(subscription) = watch {
                documents.unindex_watch(document, subscription);
            }
            if let Some(location) = location {
                if documents.by_location.get(&location) == Some(&document) {
                    documents.by_location.remove_mut(&location);
                }
            }
            documents.entries.remove_mut(&document);
        });
    }

    pub fn update_entity(
        store: &mut Store,
        document: DocumentId,
        mutate: impl FnOnce(&mut OpenDocument),
    ) {
        store.update::<OpenDocuments>(|documents| {
            let Some(mut entity) = documents.entries.get(&document).cloned() else {
                return;
            };
            mutate(&mut entity);
            documents.entries.insert_mut(document, entity);
        });
    }

    fn next_stamp(documents: &OpenDocuments) -> u64 {
        documents
            .entries
            .values()
            .map(|entity| entity.registered.max(entity.last_opened))
            .max()
            .unwrap_or(0)
            + 1
    }
}

#[cfg(test)]
mod tests;
