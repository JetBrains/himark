// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::Document;
use imba::store::Store;

pub mod diff_views;
pub mod diffs;
mod dynamic;
mod entity_view;
pub mod hover;
mod lifecycle;
pub mod scroll_stripes;
pub mod sync;
pub mod text_ext;
pub mod lanes;
pub mod watch;

pub use diffs::{
    rearm_base_asks, DiffHandle, DiffNormalizeEffect, DiffNormalizeHandler, DiffView, DiffViewId,
    Normalized, StripeBaseResolver, StripeBases,
};
pub use dynamic::{DocumentCommand, DocumentCommands};
pub use entity_view::EditorIdView;
pub use lifecycle::{close_editor, deliver, mount_editor};
pub use text_ext::{line_col_at, offset_at, LineCol};
pub use watch::{
    FileChanged, FilesChanged, SubscribeEffect, Subscription, UnsubscribeEffect, Watching,
};

pub struct FetchDocumentEffect {
    pub location: editor::ResourceLocation,
}

impl std::fmt::Display for FetchDocumentEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "fetch document /{}", self.location.path().join("/"))
    }
}

impl imba::effect::Effect for FetchDocumentEffect {
    type Result = Option<String>;
}

pub struct FetchResourceBytesEffect {
    pub origin: editor::ResourceLocation,

    pub reference: String,
}

impl std::fmt::Display for FetchResourceBytesEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "fetch resource {}", self.reference)
    }
}

impl imba::effect::Effect for FetchResourceBytesEffect {
    type Result = Option<Vec<u8>>;
}

/// Build a Document from fetched text, off the UI thread — the base
/// chain's second leg (BaseFetched -> BaseBuilt).
pub struct BuildDocumentEffect {
    pub location: editor::ResourceLocation,
    pub text: String,
}

impl std::fmt::Display for BuildDocumentEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "build document /{}", self.location.path().join("/"))
    }
}

impl imba::effect::Effect for BuildDocumentEffect {
    type Result = BuiltDocument;
}

#[derive(Clone)]
pub struct BuiltDocument {
    pub document: Document,
}

#[derive(Clone, Default)]
pub struct ScratchMint(u64);

impl ScratchMint {
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
}

pub fn next_scratch_location(
    store: &mut Store,
    scratch_names: imba::store::Id<ScratchMint>,
) -> editor::ResourceLocation {
    let mut minted = 0;
    store.update_entity(scratch_names, |mint: &mut ScratchMint| {
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
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
        location: editor::ResourceLocation,
    ) {
        store.update_entity(documents, |documents| {
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
            documents.note_write(document);
        });
    }
}

/// What the documents collection answers to, behind its `At` address
/// (docs/entities.md law 5): the editor road plus every
/// document-addressed landing. The variants carry the collection's
/// PRIVATE keys (`DocumentId`); the table never sees them.
#[derive(Clone)]
pub enum DocumentsCommand {
    Editor(DocumentId, editor::EditorCommand),

    /// A command for a STORE-HELD diff view, routed by the
    /// collection and the view id — the dressing's own road
    /// (docs/model-view.md step 1): marks-job landings and resyncs
    /// reach the view with no panel involved.
    DiffView(crate::diffs::DiffViewId, Box<editor::UnifiedDiffCommand>),

    BaseLocated {
        document: DocumentId,
        base: Option<editor::ResourceLocation>,
    },

    BaseFetched {
        document: DocumentId,
        base: editor::ResourceLocation,
        text: Option<String>,
    },

    BaseBuilt {
        document: DocumentId,
        base: editor::ResourceLocation,
        built: Document,
    },

    /// A Save All store came home for one document.
    Stored {
        document: DocumentId,
        revision: u64,
        snapshot: editor::Text,
        stored: bool,
    },

    Watched(DocumentId, Option<crate::Subscription>),

    Refetched {
        document: DocumentId,

        serial: u64,
        text: Option<String>,
    },

    RefetchDiffed {
        document: DocumentId,
        base_revision: u64,
        serial: u64,
        rebase: crate::watch::RefetchRebase,
    },

    /// A normalize lane came home: the minimal diff for a tracked
    /// pair, stamped with its collection at launch.
    Normalized {
        diff: editor::diff::DiffId,
        operation: operation::Operation,
        markup: editor::Markup,
        changed: Vec<std::ops::Range<u32>>,
        base_revision: u64,
        target_revision: u64,
    },
}

// The leaf labels are grepped in reconcile traces — keep them stable.
impl std::fmt::Display for DocumentsCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DocumentsCommand::Editor(_, command) => command.fmt(out),
            DocumentsCommand::DiffView(_, command) => command.fmt(out),
            DocumentsCommand::BaseLocated { .. } => out.write_str("base located"),
            DocumentsCommand::BaseFetched { .. } => out.write_str("base fetched"),
            DocumentsCommand::BaseBuilt { .. } => out.write_str("base built"),
            DocumentsCommand::Stored { .. } => out.write_str("document stored"),
            DocumentsCommand::Watched(..) => out.write_str("watched"),
            DocumentsCommand::Refetched { .. } => out.write_str("refetched"),
            DocumentsCommand::RefetchDiffed { .. } => out.write_str("refetch-diffed"),
            DocumentsCommand::Normalized { .. } => out.write_str("diff normalized"),
        }
    }
}

/// The one command road (docs/entities.md law 5): the router leases
/// the row and hands it effects already scoped to `DocumentsCommand`
/// — every push and every nested scope lands back at this collection
/// without naming the application's command type.
impl imba::store::Entity for OpenDocuments {
    type Command = DocumentsCommand;

    fn perform(
        &mut self,
        id: imba::store::Id<Self>,
        command: DocumentsCommand,
        store: &mut Store,
        ui: &imba::UiCtx,
        fx: &mut imba::effect::Effects<'_, DocumentsCommand>,
    ) {
        // The arms that reach a PLUGIN BOUNDARY (editor performs,
        // markup destroys, enricher installs — code that legitimately
        // opens and releases SIBLINGS in this collection) give the row
        // BACK for the duration and pick up the fresh state after; the
        // lease marker guards only the collection's own bookkeeping.
        macro_rules! with_row_home {
            ($body:expr) => {{
                store.unlease(id, std::mem::take(self));
                let out = $body;
                *self = store.lease(id).unwrap_or_default();
                out
            }};
        }
        match command {
            DocumentsCommand::Editor(document, command) => {
                if self.contains_id(document) {
                    with_row_home!(fx.scope(
                        move |command| DocumentsCommand::Editor(document, command),
                        |fx| crate::deliver(store, id, ui, document, command, fx),
                    ));
                }
            }
            DocumentsCommand::DiffView(view, command) => {
                with_row_home!(crate::diff_views::perform_diff_view(
                    store, id, ui, view, *command, fx
                ));
            }
            DocumentsCommand::BaseLocated { document, base } => {
                with_row_home!(crate::diffs::land_base_located(
                    store, id, ui, document, base, fx
                ));
            }
            DocumentsCommand::BaseFetched {
                document,
                base,
                text,
            } => {
                let Some(text) = text else {
                    return;
                };
                if !self.contains_id(document) {
                    return;
                }
                let _ = fx.push(
                    imba::effect::AnyEffect::new(BuildDocumentEffect {
                        location: base.clone(),
                        text,
                    })
                    .map(move |built| DocumentsCommand::BaseBuilt {
                        document,
                        base,
                        built: built.document,
                    }),
                );
            }
            DocumentsCommand::BaseBuilt {
                document,
                base,
                built,
            } => {
                with_row_home!(crate::diffs::land_base_built(
                    store, id, ui, document, base, built, fx
                ));
            }
            DocumentsCommand::Stored {
                document,
                revision,
                snapshot,
                stored,
            } => match stored {
                true => self.mark_saved_row(document, revision, snapshot),
                false => eprintln!("[documents] store failed for an open document"),
            },
            DocumentsCommand::Watched(document, subscription) => {
                // The channel may have gone live while the subscribe
                // was in flight: mode one holds — the host watches
                // the file, this subscription is surplus.
                if self.host_synced_row(document) {
                    if let Some(subscription) = subscription {
                        let _ = fx.push(imba::effect::AnyEffect::notification(
                            crate::watch::UnsubscribeEffect { subscription },
                        ));
                    }
                    return;
                }
                self.set_watch_row(document, subscription);
            }
            DocumentsCommand::Refetched {
                document,
                serial,
                text,
            } => {
                self.apply_refetched_row(store, document, serial, text, fx);
            }
            DocumentsCommand::RefetchDiffed {
                document,
                base_revision,
                serial,
                rebase,
            } => {
                let crate::watch::RefetchRebase {
                    operation,
                    fetched,
                    fetched_source,
                    synced,
                    ..
                } = rebase;
                // The merge EDITS the document (change sinks, repair
                // tails) — a plugin boundary like any editor perform.
                let retry = with_row_home!(fx.scope(
                    move |command| DocumentsCommand::Editor(document, command),
                    |fx| {
                        Self::absorb_refetched(
                            store,
                            id,
                            ui,
                            document,
                            base_revision,
                            serial,
                            &operation,
                            fetched,
                            synced,
                            fx,
                        )
                    },
                ));
                if retry {
                    self.rediff_row(store, document, serial, fetched_source, fx);
                }
            }
            DocumentsCommand::Normalized {
                diff,
                operation,
                markup,
                changed,
                base_revision,
                target_revision,
            } => {
                if self.land_normalized_row(diff, operation, base_revision, target_revision) {
                    if let Some(handle) = self.diff_handle_row(diff) {
                        let document = handle.target;
                        // The markup swap destroys replaced inlays —
                        // a plugin boundary.
                        with_row_home!(fx.scope(
                            move |command| DocumentsCommand::Editor(document, command),
                            |fx| {
                                crate::diffs::land_diff_markup(
                                    store,
                                    id,
                                    ui,
                                    diff,
                                    markup,
                                    changed,
                                    target_revision,
                                    fx,
                                )
                            },
                        ));
                    }
                    // No push here: every visible diff face notices the
                    // landed generation itself, on its next paint (the
                    // staleness probe in hidiff's GatheredSplit).
                }
            }
        }
    }

    fn destroy(&mut self, _store: &mut Store) {
        // The records are the collection's PRIVATE schema, not table
        // entities — nothing to retract; watches and editors die with
        // the drop.
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

/// Registration/release observers. They receive the FACTS (the
/// document's location) rather than reading the collection back by
/// id — hooks run inside the collection's own doors, where a row
/// read would be lease reentrancy (docs/entities.md law 5).
pub trait DocumentHook: Send + Sync {
    fn opened(
        &self,
        store: &mut imba::store::Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
        location: Option<&editor::ResourceLocation>,
    );
    fn closing(
        &self,
        store: &mut imba::store::Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
        location: Option<&editor::ResourceLocation>,
        doc: &Document,
    );
}

#[derive(Clone, Default)]
pub struct OpenDocuments {
    pub(crate) entries: rpds::HashTrieMapSync<DocumentId, OpenDocument>,

    by_location: rpds::HashTrieMapSync<editor::ResourceLocation, DocumentId>,

    by_watch: rpds::HashTrieMapSync<crate::Subscription, rpds::VectorSync<DocumentId>>,

    pub(crate) diffs: crate::diffs::Diffs,

    pub(crate) pending: PendingSweeps,
}

/// The batch-tail lanes' work queues: every entry write enqueues the
/// document for each lane, and a lane drains ITS queue when it runs —
/// O(touched since the last sweep), never O(all documents ever)
/// (docs/perf-issue.md). Three queues, one per lane: the lanes run at
/// different tail positions, so a shared queue drained by the first
/// would starve the rest. `theme` is the stripe lane's last-swept
/// theme — stripes derive from theme colors, so a switch re-queues
/// every document once.
#[derive(Clone)]
pub(crate) struct PendingSweeps {
    pub(crate) diff_lanes: rpds::HashTrieSetSync<DocumentId>,
    pub(crate) stripes: rpds::HashTrieSetSync<DocumentId>,
    pub(crate) dressing: rpds::HashTrieSetSync<DocumentId>,
    pub(crate) theme: Option<String>,

    /// The diff views the dressing touched this batch — written by
    /// the sweep and the id-routed landings, taken by the canvas
    /// lane (`take_dressed`), which resizes exactly these rows.
    pub(crate) dressed: Vec<crate::DiffViewId>,
}

impl Default for PendingSweeps {
    fn default() -> Self {
        Self {
            diff_lanes: rpds::HashTrieSetSync::new_sync(),
            stripes: rpds::HashTrieSetSync::new_sync(),
            dressing: rpds::HashTrieSetSync::new_sync(),
            theme: None,
            dressed: Vec::new(),
        }
    }
}

#[derive(Clone, Default)]
struct DocumentHooks {
    global: rpds::VectorSync<std::sync::Arc<dyn DocumentHook>>,

    /// Hooks WIRED to one collection — installed by the session
    /// ceremony with their sibling ids in hand (docs/entities.md
    /// law 4), retired with the collection.
    scoped: rpds::HashTrieMapSync<
        imba::store::Id<OpenDocuments>,
        rpds::VectorSync<std::sync::Arc<dyn DocumentHook>>,
    >,
}

/// The ONE closed home for this crate's registries (docs/entities.md
/// law 3): commands, hooks, the stripe-base resolver and the watch
/// capability are NAMED fields behind their modules' doors — never
/// anonymous components grabbed from the store by type.
#[derive(Clone, Default)]
pub(crate) struct Registry {
    pub(crate) commands: crate::DocumentCommands,
    pub(crate) hooks: DocumentHooks,
    pub(crate) stripe_bases: Option<std::sync::Arc<dyn crate::diffs::StripeBaseResolver>>,
    pub(crate) watching: bool,
}

impl Registry {
    pub(crate) fn of(store: &Store) -> Option<&Registry> {
        store.get::<Registry>()
    }

    pub(crate) fn update(store: &mut Store, mutate: impl FnOnce(&mut Registry)) {
        store.update::<Registry>(mutate);
    }
}

impl OpenDocuments {
    pub fn register(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        document: Document,
        location: Option<editor::ResourceLocation>,
        title: String,
        saved_revision: u64,
    ) -> DocumentId {
        // The outside door mints the row lazily, as it always has; a
        // leased perform calls `register_row` on the row it holds.
        let mut rows = store.entity(documents).cloned().unwrap_or_default();
        let id = rows.register_row(store, documents, document, location, title, saved_revision);
        store.put_entity(documents, rows);
        id
    }

    pub fn register_row(
        &mut self,
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        document: Document,
        location: Option<editor::ResourceLocation>,
        title: String,
        saved_revision: u64,
    ) -> DocumentId {
        // Monotonic and never reused — the `Id::mint` pattern.
        static MINT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = DocumentId(MINT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let stamp = Self::next_stamp(self);
        if let Some(location) = &location {
            self.by_location.insert_mut(location.clone(), id);
        }
        let hook_location = location.clone();
        let baseline = document.text().clone();
        self.entries.insert_mut(
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
        self.note_write(id);
        for hook in Self::hooks(store, documents) {
            hook.opened(store, documents, id, hook_location.as_ref());
        }
        id
    }

    pub fn install_hook(store: &mut Store, hook: std::sync::Arc<dyn DocumentHook>) {
        Registry::update(store, |registry| registry.hooks.global.push_back_mut(hook));
    }

    pub fn install_scoped_hook(
        store: &mut Store,
        scope: imba::store::Id<OpenDocuments>,
        hook: std::sync::Arc<dyn DocumentHook>,
    ) {
        Registry::update(store, |registry| {
            let mut entries = registry
                .hooks
                .scoped
                .get(&scope)
                .cloned()
                .unwrap_or_default();
            entries.push_back_mut(hook);
            registry.hooks.scoped.insert_mut(scope, entries);
        });
    }

    /// Retire everything the ceremony wired to this collection: its
    /// scoped hooks and scoped commands. The owner calls this from
    /// its `destroy` — teardown cascades by ownership (law 6).
    pub fn retire_scope(store: &mut Store, scope: imba::store::Id<OpenDocuments>) {
        Registry::update(store, |registry| {
            registry.hooks.scoped.remove_mut(&scope);
        });
        crate::DocumentCommands::retire_scope(store, scope);
    }

    fn hooks(
        store: &Store,
        scope: imba::store::Id<OpenDocuments>,
    ) -> Vec<std::sync::Arc<dyn DocumentHook>> {
        let Some(hooks) = Registry::of(store).map(|registry| &registry.hooks) else {
            return Vec::new();
        };
        hooks
            .scoped
            .get(&scope)
            .into_iter()
            .flat_map(|entries| entries.iter())
            .chain(hooks.global.iter())
            .cloned()
            .collect()
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

    pub fn document(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        id: DocumentId,
    ) -> Option<Document> {
        Self::document_ref(store, documents, id).cloned()
    }

    pub fn document_ref(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        id: DocumentId,
    ) -> Option<&Document> {
        store.entity(documents)?.document_row(id)
    }

    /// The row forms (`*_row`): the collection speaking for itself —
    /// what a leased perform uses, where a store read of the own id
    /// would be reentrancy (docs/entities.md law 5). The `(store, id)`
    /// statics stay as the outside doors and delegate here.
    pub fn document_row(&self, id: DocumentId) -> Option<&Document> {
        self.entries.get(&id).map(|entity| &entity.document)
    }

    pub fn put_document(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        id: DocumentId,
        document: Document,
    ) {
        Self::update_entity(store, documents, id, |entity| entity.document = document);
    }

    pub fn put_document_row(&mut self, id: DocumentId, document: Document) {
        self.update_row(id, |entity| entity.document = document);
    }

    pub(crate) fn update_row(&mut self, id: DocumentId, mutate: impl FnOnce(&mut OpenDocument)) {
        let Some(mut entity) = self.entries.get(&id).cloned() else {
            return;
        };
        mutate(&mut entity);
        self.entries.insert_mut(id, entity);
        self.note_write(id);
    }

    /// An entry write the batch-tail lanes care about: queue the
    /// document for each lane's next sweep. Every door that replaces
    /// an entry calls this — the lanes see exactly the writes, so
    /// their sweeps stay O(touched) (docs/perf-issue.md).
    pub(crate) fn note_write(&mut self, id: DocumentId) {
        self.pending.diff_lanes.insert_mut(id);
        self.pending.stripes.insert_mut(id);
        self.pending.dressing.insert_mut(id);
    }

    pub(crate) fn take_diff_lane_pending(&mut self) -> rpds::HashTrieSetSync<DocumentId> {
        std::mem::take(&mut self.pending.diff_lanes)
    }

    /// Drain the stripe lane's queue. A theme switch — and the very
    /// first sweep — re-queues EVERY document once: the stripes derive
    /// from theme colors, which no entry write announces.
    pub(crate) fn take_stripe_pending(&mut self, theme: &str) -> Vec<DocumentId> {
        if self.pending.theme.as_deref() != Some(theme) {
            self.pending.theme = Some(theme.to_owned());
            self.pending.stripes = rpds::HashTrieSetSync::new_sync();
            return self.entries.keys().copied().collect();
        }
        let drained = std::mem::take(&mut self.pending.stripes);
        drained.iter().copied().collect()
    }

    pub fn contains(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        id: DocumentId,
    ) -> bool {
        Self::document_ref(store, documents, id).is_some()
    }

    pub fn list(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
    ) -> Vec<(DocumentId, OpenDocument)> {
        let mut entries: Vec<(DocumentId, OpenDocument)> = store
            .entity(documents)
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

    pub fn list_recent(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
    ) -> Vec<(DocumentId, OpenDocument)> {
        let mut entries = Self::list(store, documents);
        entries.sort_by(|(_, a), (_, b)| b.last_opened.cmp(&a.last_opened));
        entries
    }

    pub fn name(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
    ) -> Option<String> {
        Self::entity(store, documents, document).map(|entity| entity.name())
    }

    pub fn location(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
    ) -> Option<editor::ResourceLocation> {
        store.entity(documents)?.location_row(document)
    }

    pub fn location_row(&self, document: DocumentId) -> Option<editor::ResourceLocation> {
        self.entries.get(&document)?.location.clone()
    }

    pub fn entity(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
    ) -> Option<OpenDocument> {
        store.entity(documents)?.entity_row(document)
    }

    pub fn entity_row(&self, document: DocumentId) -> Option<OpenDocument> {
        self.entries.get(&document).cloned()
    }

    pub fn by_location(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        location: &editor::ResourceLocation,
    ) -> Option<DocumentId> {
        store.entity(documents)?.by_location_row(location)
    }

    pub fn by_location_row(&self, location: &editor::ResourceLocation) -> Option<DocumentId> {
        self.by_location.get(location).copied()
    }

    pub fn touch(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
    ) {
        let Some(rows_ref) = store.entity(documents) else {
            return;
        };
        let stamp = Self::next_stamp(rows_ref);
        Self::update_entity(store, documents, document, |entity| {
            entity.last_opened = stamp
        });
    }

    pub fn set_save_token(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
        token: Option<imba::effect::CancellationToken>,
    ) {
        Self::update_entity(store, documents, document, |entity| {
            entity.save_token = token
        });
    }

    /// The document channel went live (or died): while live, the
    /// HOST owns disk reloads and this client must not watch the
    /// file; on fallback the watch machinery re-arms and a refetch
    /// resyncs from disk.
    pub fn set_host_synced<R: 'static>(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
        synced: bool,
        fx: &mut imba::effect::Effects<'_, R>,
    ) {
        let Some(entity) = Self::entity(store, documents, document) else {
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
        Self::update_entity(store, documents, document, |entity| {
            entity.host_synced = synced;
            if synced {
                entity.watch = None;
            }
            // Either way the sweep decides afresh.
            entity.watch_requested = false;
        });
    }

    pub fn host_synced(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
    ) -> bool {
        store
            .entity(documents)
            .is_some_and(|rows| rows.host_synced_row(document))
    }

    pub fn host_synced_row(&self, document: DocumentId) -> bool {
        self.entries
            .get(&document)
            .is_some_and(|entity| entity.host_synced)
    }

    pub fn set_watch_requested(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
    ) {
        Self::update_entity(store, documents, document, |entity| {
            entity.watch_requested = true
        });
    }

    pub fn set_base_requested(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
    ) {
        Self::update_entity(store, documents, document, |entity| {
            entity.base_requested = true
        });
    }

    pub fn set_base_requested_row(&mut self, document: DocumentId) {
        self.update_row(document, |entity| entity.base_requested = true);
    }

    pub fn set_watch(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
        watch: Option<crate::Subscription>,
    ) {
        store.update_entity(documents, |documents| {
            documents.set_watch_row(document, watch)
        });
    }

    pub fn set_watch_row(&mut self, document: DocumentId, watch: Option<crate::Subscription>) {
        let Some(entity) = self.entries.get(&document) else {
            return;
        };
        let mut entity = entity.clone();
        let old = entity.watch;
        entity.watch = watch;
        self.entries.insert_mut(document, entity);
        self.note_write(document);
        if old != watch {
            if let Some(old) = old {
                self.unindex_watch(document, old);
            }
            if let Some(new) = watch {
                self.index_watch(document, new);
            }
        }
    }

    pub fn mark_saved(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
        revision: u64,
        stored: editor::Text,
    ) {
        Self::update_entity(store, documents, document, |entity| {
            entity.saved_revision = revision;
            entity.baseline = stored;
        });
    }

    pub fn mark_saved_row(&mut self, document: DocumentId, revision: u64, stored: editor::Text) {
        self.update_row(document, |entity| {
            entity.saved_revision = revision;
            entity.baseline = stored;
        });
    }

    pub fn edit_external(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        ui: &imba::UiCtx,
        document_id: DocumentId,
        base_revision: u64,
        operation: &operation::Operation,
        fx: &mut imba::effect::Effects<'_, editor::EditorCommand>,
    ) {
        let Some(mut document) = Self::document(store, documents, document_id) else {
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
        if let Some(location) = Self::location(store, documents, document_id) {
            for sink in ::editor::InstalledChangeSink::of(store) {
                sink.changed(store, &document, &location, base_revision, &text_before, fx);
            }
        }

        let revision = document.revision();
        let stored = document.text().clone();
        Self::put_document(store, documents, document_id, document);
        Self::mark_saved(store, documents, document_id, revision, stored);
    }

    pub fn edit_shared(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        ui: &imba::UiCtx,
        document_id: DocumentId,
        identity: ::editor::EditIdentity,
        base_revision: u64,
        operation: &operation::Operation,
        fx: &mut imba::effect::Effects<'_, editor::EditorCommand>,
    ) -> bool {
        let Some(mut document) = Self::document(store, documents, document_id) else {
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
        if let Some(location) = Self::location(store, documents, document_id) {
            for sink in ::editor::InstalledChangeSink::of(store) {
                sink.changed(store, &document, &location, base_revision, &text_before, fx);
            }
        }
        Self::put_document(store, documents, document_id, document);
        true
    }

    pub fn stamp_refetch(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
    ) -> u64 {
        let mut stamped = 0;
        Self::update_entity(store, documents, document, |entity| {
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
        documents: imba::store::Id<OpenDocuments>,
        ui: &imba::UiCtx,
        document_id: DocumentId,
        base_revision: u64,
        serial: u64,
        operation: &operation::Operation,
        fetched: editor::Text,
        synced: bool,
        fx: &mut imba::effect::Effects<'_, editor::EditorCommand>,
    ) -> bool {
        let Some(entity) = Self::entity(store, documents, document_id) else {
            return false;
        };
        // The channel went live while this landing was in flight: the
        // HOST owns disk truth now; a client-side merge would fight
        // its broadcasts.
        if entity.host_synced || entity.refetch_serial != serial {
            return false;
        }
        let Some(mut document) = Self::document(store, documents, document_id) else {
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
            if let Some(location) = Self::location(store, documents, document_id) {
                for sink in ::editor::InstalledChangeSink::of(store) {
                    sink.changed(store, &document, &location, base_revision, &text_before, fx);
                }
            }
        }
        let revision = document.revision();
        Self::put_document(store, documents, document_id, document);
        // `synced` came from the WORKER (the merge target equals the
        // disk text): the landing brought the buffer exactly to the
        // disk — a save, whatever the diff route was. Never compare
        // texts here; this is the UI thread.
        match synced {
            true => Self::mark_saved(store, documents, document_id, revision, fetched),
            false => Self::update_entity(store, documents, document_id, |entity| {
                entity.baseline = fetched
            }),
        }
        false
    }

    pub fn remove_if_editorless<R: 'static>(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        ui: &imba::UiCtx,
        document: DocumentId,
        fx: &mut imba::effect::Effects<'_, R>,
    ) {
        Self::release_editorless(store, documents, ui, document, fx, true)
    }

    pub fn remove_on_close<R: 'static>(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        ui: &imba::UiCtx,
        document: DocumentId,
        fx: &mut imba::effect::Effects<'_, R>,
    ) {
        Self::release_editorless(store, documents, ui, document, fx, false)
    }

    fn release_editorless<R: 'static>(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        ui: &imba::UiCtx,
        document: DocumentId,
        fx: &mut imba::effect::Effects<'_, R>,
        spare_scratch: bool,
    ) {
        let Some(entity) = Self::entity(store, documents, document) else {
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

        if Self::untrack_stripes(store, documents, ui, document, fx) {
            return;
        }

        if store
            .entity(documents)
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

        for hook in Self::hooks(store, documents) {
            hook.closing(
                store,
                documents,
                document,
                entity.location.as_ref(),
                &entity.document,
            );
        }
        store.update_entity(documents, |documents| {
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
            documents.note_write(document);
        });
    }

    pub fn update_entity(
        store: &mut Store,
        documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
        mutate: impl FnOnce(&mut OpenDocument),
    ) {
        store.update_entity(documents, |documents| {
            documents.update_row(document, mutate);
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

// The workspace file operations the shells route to their hosts
// (fsroute): plain location-addressed asks, no window anywhere.

pub struct StoreDocumentEffect {
    pub location: editor::ResourceLocation,
    pub text: String,
}

impl std::fmt::Display for StoreDocumentEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "store document /{}", self.location.path().join("/"))
    }
}

impl imba::effect::Effect for StoreDocumentEffect {
    type Result = bool;
}

pub struct ListDirectoryEffect {
    pub location: editor::ResourceLocation,
}

impl std::fmt::Display for ListDirectoryEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "list directory /{}", self.location.path().join("/"))
    }
}

impl imba::effect::Effect for ListDirectoryEffect {
    type Result = Option<Vec<editor::ResourceLocation>>;
}

/// Creates an empty file; never overwrites — false when the
/// location already exists.
pub struct CreateDocumentEffect {
    pub location: editor::ResourceLocation,
}

impl std::fmt::Display for CreateDocumentEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "create document /{}", self.location.path().join("/"))
    }
}

impl imba::effect::Effect for CreateDocumentEffect {
    type Result = bool;
}

pub struct DeleteResourceEffect {
    pub location: editor::ResourceLocation,
    pub recursive: bool,
}

impl std::fmt::Display for DeleteResourceEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "delete resource /{}", self.location.path().join("/"))
    }
}

impl imba::effect::Effect for DeleteResourceEffect {
    type Result = bool;
}

/// A rename: fails when the destination exists.
pub struct MoveResourceEffect {
    pub from: editor::ResourceLocation,
    pub to: editor::ResourceLocation,
}

impl std::fmt::Display for MoveResourceEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "move resource /{} -> /{}",
            self.from.path().join("/"),
            self.to.path().join("/")
        )
    }
}

impl imba::effect::Effect for MoveResourceEffect {
    type Result = bool;
}

pub struct PickSaveEffect {
    pub suggested: String,
}

impl std::fmt::Display for PickSaveEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "pick save {}", self.suggested)
    }
}

impl imba::effect::Effect for PickSaveEffect {
    type Result = Option<editor::ResourceLocation>;
}
