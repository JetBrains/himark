// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Document-channel sync: one LIFE per location.
//!
//! ```text
//! (no entry) ──ensure──▶ Connecting ──adopt──▶ Live
//!                            │                  │
//!                       detach/decline        detach
//!                            ▼                  ▼
//!                        Draining { reopen } ◀──┘
//!                            │
//!                         Drained ──reopen queued?──▶ Connecting …
//! ```
//!
//! The store's `SyncClients` holds the per-location state; the spawned
//! life task (`channel_life`) owns the wire: open → subscribe →
//! adopt → pumps → poll. Shutdown is COOPERATIVE (a stop signal,
//! never an abort): every exit past the subscribe UNSUBSCRIBES the
//! channel, awaited, and then posts `Drained` — so a reopen queued
//! on a draining client connects strictly AFTER the old subscription
//! is gone. One location, one subscription, ever.

mod codec;
mod rules;

use std::sync::Arc;

use ahp_wire::client::DocumentsClient;
use editor::location::ResourceLocation;
use himark_ahp_ext_types::{DocumentApplied, Uid};
use imba::command::Verb;
use imba::store::Store;
use rebase::{Local, Offer, RebaseLog};
use tokio::sync::{mpsc, oneshot, watch};

use documents::sync::{SyncEdit, SyncState};
use editor::edit_log::EditIdentity;

#[doc(hidden)]
pub use codec::{resolve_wire, wire_operation};

#[derive(Clone)]
struct Client {
    edits: mpsc::UnboundedSender<Local<SyncEdit>>,

    stop: mpsc::UnboundedSender<()>,

    attached_at: u64,

    taken: u64,

    applied: Option<EditIdentity>,
}

/// A client waiting to connect again once the draining life is gone.
#[derive(Clone)]
struct Reopen {
    client: Arc<dyn DocumentsClient>,
    session: String,
}

#[derive(Clone)]
enum ClientState {
    Connecting {
        stop: mpsc::UnboundedSender<()>,

        since: u64,
    },
    Live(Client),
    /// The life was told to stop and is unsubscribing; `Drained`
    /// retires the entry. An open arriving meanwhile parks here and
    /// connects from `Drained` — never beside the dying life.
    Draining {
        reopen: Option<Reopen>,
    },
}

impl ClientState {
    /// COOPERATIVE shutdown, never an abort: the life task owns the
    /// channel subscription and must live long enough to unsubscribe
    /// it — an aborted task cannot, and a leaked subscription doubles
    /// every broadcast the next time the location opens.
    fn stop(&self) {
        match self {
            Self::Connecting { stop, .. } => {
                let _ = stop.send(());
            }
            Self::Live(client) => {
                let _ = client.stop.send(());
            }
            Self::Draining { .. } => {}
        }
    }
}

#[derive(Clone, Default)]
pub struct SyncClients {
    clients: rpds::HashTrieMapSync<ResourceLocation, ClientState>,
}

impl SyncClients {
    fn of(store: &Store) -> Self {
        store
            .get::<SyncClients>()
            .map(|clients| clients.clone())
            .unwrap_or_default()
    }

    fn client(store: &Store, location: &ResourceLocation) -> Option<Client> {
        match store.get::<SyncClients>()?.clients.get(location)? {
            ClientState::Live(client) => Some(client.clone()),
            ClientState::Connecting { .. } | ClientState::Draining { .. } => None,
        }
    }

    fn connecting(
        store: &Store,
        location: &ResourceLocation,
    ) -> Option<(mpsc::UnboundedSender<()>, u64)> {
        match store.get::<SyncClients>()?.clients.get(location)? {
            ClientState::Connecting { stop, since } => Some((stop.clone(), *since)),
            ClientState::Live(_) | ClientState::Draining { .. } => None,
        }
    }

    fn known(store: &Store, location: &ResourceLocation) -> bool {
        store
            .get::<SyncClients>()
            .is_some_and(|clients| clients.clients.contains_key(location))
    }

    fn draining(store: &Store, location: &ResourceLocation) -> bool {
        store.get::<SyncClients>().is_some_and(|clients| {
            matches!(clients.clients.get(location), Some(ClientState::Draining { .. }))
        })
    }

    fn put(store: &mut Store, location: ResourceLocation, state: ClientState) {
        let mut clients = Self::of(store);
        clients.clients.insert_mut(location, state);
        store.put(clients);
    }

    fn expect(store: &mut Store, location: &ResourceLocation, identity: EditIdentity) {
        let Some(client) = Self::client(store, location) else {
            return;
        };
        Self::put(
            store,
            location.clone(),
            ClientState::Live(Client {
                applied: Some(identity),
                ..client
            }),
        );
    }

    fn took(store: &mut Store, location: &ResourceLocation) {
        let Some(client) = Self::client(store, location) else {
            return;
        };
        Self::put(
            store,
            location.clone(),
            ClientState::Live(Client {
                taken: client.taken + 1,
                ..client
            }),
        );
    }

    /// The one exit door: stop the life (it will unsubscribe and
    /// post `Drained`) and mark the entry draining. Detaching a
    /// still-draining entry only clears a queued reopen — the
    /// document that wanted back in is itself gone.
    fn detach(store: &mut Store, location: &ResourceLocation) {
        let Some(state) = store
            .get::<SyncClients>()
            .and_then(|clients| clients.clients.get(location))
        else {
            return;
        };
        state.stop();
        Self::put(
            store,
            location.clone(),
            ClientState::Draining { reopen: None },
        );
    }

    fn queue_reopen(store: &mut Store, location: &ResourceLocation, reopen: Reopen) {
        if Self::draining(store, location) {
            Self::put(
                store,
                location.clone(),
                ClientState::Draining {
                    reopen: Some(reopen),
                },
            );
        }
    }

    fn retire(store: &mut Store, location: &ResourceLocation) -> Option<ClientState> {
        let mut clients = Self::of(store);
        let retired = clients.clients.get(location).cloned();
        clients.clients.remove_mut(location);
        store.put(clients);
        retired
    }

    #[doc(hidden)]
    pub fn count(store: &Store) -> usize {
        Self::of(store).clients.size()
    }
}

/// The save lane of a live document channel: the flush marker rides
/// the same queue as the edits, so the host's dump lands only after
/// every edit enqueued before the save is committed.
#[derive(Clone)]
pub struct StoreHandle {
    edits: mpsc::UnboundedSender<Local<SyncEdit>>,
    server: Arc<dyn DocumentsClient>,
    channel: himark_ahp_ext_types::Uri,
}

impl StoreHandle {
    pub async fn store(self, uri: ahp_wire::client::ResourceUri) -> bool {
        let (done, landed) = oneshot::channel();
        if self.edits.send(Local::Flush(done)).is_err() {
            return false;
        }
        if landed.await.is_err() {
            return false;
        }
        match self
            .server
            .store_document(ahp_wire::client::ChannelUri::new(self.channel), uri)
            .await
        {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "docsync: the host could not store the document");
                false
            }
        }
    }
}

/// Everything the channels object tracks, as ONE persistent value
/// behind one lock — the lock is a swap latch, never a region to
/// think inside.
#[derive(Clone, Default)]
struct ChannelState {
    /// The action-id mint (salted at construction).
    minted: u64,

    /// The save routes: a watch per location so a save can outwait
    /// the connecting window; `Some(handle)` once the channel
    /// adopted.
    stores: rpds::HashTrieMapSync<ResourceLocation, watch::Sender<Option<StoreHandle>>>,
}

pub struct DocumentChannels {
    post: Arc<dyn Fn(Verb) + Send + Sync>,
    uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
    runtime: tokio::runtime::Handle,

    salt: u128,
    state: std::sync::Mutex<ChannelState>,
}

impl DocumentChannels {
    pub fn new(
        runtime: tokio::runtime::Handle,
        post: Arc<dyn Fn(Verb) + Send + Sync>,
        uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
    ) -> Arc<Self> {
        let salt = {
            use std::hash::{BuildHasher, Hasher};
            let high = std::collections::hash_map::RandomState::new()
                .build_hasher()
                .finish();
            let low = std::collections::hash_map::RandomState::new()
                .build_hasher()
                .finish();
            (u128::from(high) << 64) | u128::from(low)
        };
        Arc::new(Self {
            post,
            uris,
            runtime,
            salt,
            state: std::sync::Mutex::new(ChannelState::default()),
        })
    }

    fn update<R>(&self, mutate: impl FnOnce(&mut ChannelState) -> R) -> R {
        let mut state = self.state.lock().expect("docsync channel state");
        mutate(&mut state)
    }

    fn store_connecting(&self, location: &ResourceLocation) {
        let (route, _) = watch::channel(None);
        self.update(|state| state.stores.insert_mut(location.clone(), route));
    }

    fn store_ready(&self, location: &ResourceLocation, handle: StoreHandle) {
        if let Some(route) = self.update(|state| state.stores.get(location).cloned()) {
            // send_replace: a plain send is DROPPED while no save is
            // subscribed, and the handle must outwait its receivers.
            route.send_replace(Some(handle));
        }
    }

    pub(crate) fn forget_store(&self, location: &ResourceLocation) {
        self.update(|state| {
            state.stores.remove_mut(location);
        });
    }

    /// `None`: no document channel — the resource itself is the
    /// truth, store it directly. `Some`: the host mirrors this
    /// document and the save must flow through the channel. Waits
    /// out the connecting window so a save cannot slip UNDER a
    /// channel being adopted.
    pub async fn store_handle(&self, location: &ResourceLocation) -> Option<StoreHandle> {
        let mut route = self
            .update(|state| state.stores.get(location).cloned())?
            .subscribe();
        loop {
            if let Some(handle) = route.borrow().clone() {
                return Some(handle);
            }
            if route.changed().await.is_err() {
                return None;
            }
        }
    }

    fn mint(&self) -> Uid {
        let minted = self.update(|state| {
            state.minted += 1;
            state.minted
        });
        Uid(self.salt ^ u128::from(minted))
    }

    /// Ensure the channel for a document the hook just registered —
    /// the hook holds the collection, so every landing of this life
    /// carries it (docs/entities.md law 3).
    pub fn ensure(
        self: &Arc<Self>,
        documents: imba::store::Id<documents::OpenDocuments>,
        location: ResourceLocation,
        client: Arc<dyn DocumentsClient>,
        session: String,
    ) {
        self.post(EnsureSync {
            channels: Arc::clone(self),
            documents,
            location,
            client,
            session,
        });
    }

    fn post(&self, command: impl imba::command::DynamicCommand + 'static) {
        (self.post)(Verb::Dynamic(Arc::new(command)));
    }
}

/// Spawn a fresh life for the location. Callers guarantee no other
/// life exists: `EnsureSync` connects only an unknown location, and
/// `Drained` connects only after the previous life unsubscribed.
fn connect(
    channels: &Arc<DocumentChannels>,
    store: &mut Store,
    documents: imba::store::Id<documents::OpenDocuments>,
    location: &ResourceLocation,
    client: &Arc<dyn DocumentsClient>,
    session: &str,
) {
    channels.store_connecting(location);
    let since = Some(documents)
        .and_then(|documents| {
            documents::OpenDocuments::by_location(store, documents, location)
                .and_then(|id| documents::OpenDocuments::document_ref(store, documents, id))
        })
        .map(|document| document.revision())
        .unwrap_or_default();
    let (stop, stopped) = mpsc::unbounded_channel();
    channels.runtime.spawn(channel_life(
        Arc::clone(channels),
        documents,
        location.clone(),
        Arc::clone(client),
        session.to_owned(),
        stopped,
    ));
    SyncClients::put(
        store,
        location.clone(),
        ClientState::Connecting { stop, since },
    );
}

type Seeded = (SyncState, Uid, mpsc::UnboundedReceiver<Local<SyncEdit>>);

async fn channel_life(
    channels: Arc<DocumentChannels>,
    documents: imba::store::Id<documents::OpenDocuments>,
    location: ResourceLocation,
    server: Arc<dyn DocumentsClient>,
    session: String,
    mut stopped: mpsc::UnboundedReceiver<()>,
) {
    life(
        &channels,
        documents,
        &location,
        server,
        session,
        &mut stopped,
    )
    .await;
    // The last word, ALWAYS: whatever road ended the life, the
    // subscription is gone by now, so a queued reopen may connect.
    channels.post(Drained {
        channels: Arc::clone(&channels),
        documents,
        location,
    });
}

async fn life(
    channels: &Arc<DocumentChannels>,
    documents: imba::store::Id<documents::OpenDocuments>,
    location: &ResourceLocation,
    server: Arc<dyn DocumentsClient>,
    session: String,
    stopped: &mut mpsc::UnboundedReceiver<()>,
) {
    let uri = channels.uris.uri_of(location);
    let opened = tokio::select! {
        biased;
        // Stopped before the subscribe was ever sent: nothing to
        // release — an open at most mints (or re-finds) the channel.
        _ = stopped.recv() => return,
        opened = server.open_document(ahp_wire::client::SessionUri::new(session), Some(uri), None) => match opened {
            Ok(opened) => opened,
            Err(error) => {
                tracing::warn!(%error, "docsync: could not reach the channel");
                channels.post(GiveUp {
                    channels: Arc::clone(channels),
                    documents,
                    location: location.clone(),
                });
                return;
            }
        },
    };
    // From here the subscription exists (or may): EVERY exit below
    // unsubscribes, awaited — the one subscription dies with the life
    // that made it. A leaked subscription re-subscribes later and
    // every broadcast arrives twice: the character-doubling bug.
    let snapshot = match server
        .subscribe_document(ahp_wire::client::ChannelUri::new(opened.document.clone()))
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::warn!(%error, "docsync: could not subscribe the channel");
            server
                .unsubscribe_document(&ahp_wire::client::ChannelUri::new(opened.document.clone()))
                .await;
            channels.post(GiveUp {
                channels: Arc::clone(channels),
                documents,
                location: location.clone(),
            });
            return;
        }
    };
    if stopped.try_recv().is_ok() {
        server
            .unsubscribe_document(&ahp_wire::client::ChannelUri::new(opened.document.clone()))
            .await;
        return;
    }

    let (seeded, mut seed) = mpsc::channel::<Seeded>(1);
    channels.post(AdoptSnapshot {
        channels: Arc::clone(channels),
        documents,
        server: Arc::clone(&server),
        document: opened.document.clone(),
        location: location.clone(),
        snapshot: snapshot.text.clone(),
        version: snapshot.version,
        seeded,
    });
    let adopted = tokio::select! {
        biased;
        _ = stopped.recv() => None,
        seeded = seed.recv() => seeded,
    };
    let Some((state, version, edits)) = adopted else {
        // Adoption declined (no registered document), or the document
        // closed while we were connecting: release the channel.
        server
            .unsubscribe_document(&ahp_wire::client::ChannelUri::new(opened.document.clone()))
            .await;
        return;
    };

    let (actions, actions_rx) = mpsc::channel(64);
    let (wire, mut wire_rx) = mpsc::channel(64);
    let (offers, mut offers_rx) = mpsc::channel(8);
    let mint = {
        let channels = Arc::clone(channels);
        move || channels.mint()
    };
    channels.runtime.spawn(rebase::run(
        RebaseLog::new(state, version),
        edits,
        actions_rx,
        wire,
        offers,
        mint,
    ));

    {
        let server = Arc::clone(&server);
        let channel = ahp_wire::client::ChannelUri::new(opened.document.clone());
        channels.runtime.spawn(async move {
            while let Some(dispatch) = wire_rx.recv().await {
                let Some(operation) = dispatch.action.operation() else {
                    continue;
                };
                server.dispatch_document(
                    &channel,
                    DocumentApplied {
                        base: dispatch.base,
                        operation: wire_operation(&dispatch.before.text, operation),
                        id: dispatch.id,
                        origin: None,
                    },
                );
            }
        });
    }

    {
        let channels = Arc::clone(channels);
        let location = location.clone();
        let runtime = channels.runtime.clone();
        runtime.spawn(async move {
            while let Some(offer) = offers_rx.recv().await {
                channels.post(ApplyOffer {
                    documents,
                    location: location.clone(),
                    offer,
                });
            }
        });
    }

    loop {
        let heard = tokio::select! {
            biased;
            _ = stopped.recv() => {
                server.unsubscribe_document(&ahp_wire::client::ChannelUri::new(opened.document.clone())).await;
                return;
            }
            heard = server.poll_document(ahp_wire::client::ChannelUri::new(opened.document.clone())) => heard,
        };
        for action in heard {
            let applied = rebase::Applied {
                id: action.id,
                action: SyncEdit::Theirs {
                    resolve: resolve_wire(action.operation),
                },
            };
            if actions.send(applied).await.is_err() {
                server
                    .unsubscribe_document(&ahp_wire::client::ChannelUri::new(opened.document.clone()))
                    .await;
                return;
            }
        }
    }
}

struct EnsureSync {
    channels: Arc<DocumentChannels>,
    documents: imba::store::Id<documents::OpenDocuments>,
    location: ResourceLocation,
    client: Arc<dyn DocumentsClient>,
    session: String,
}

impl imba::command::DynamicCommand for EnsureSync {
    fn id(&self) -> &'static str {
        "docsync.ensure"
    }
    fn name(&self) -> String {
        "Sync Document".to_owned()
    }
    fn perform(&self, store: &mut Store, ui: &imba::ui::UiCtx, _fx: &mut imba::command::Fx<'_>) {
        if SyncClients::draining(store, &self.location) {
            SyncClients::queue_reopen(
                store,
                &self.location,
                Reopen {
                    client: Arc::clone(&self.client),
                    session: self.session.clone(),
                },
            );
            return;
        }
        if SyncClients::known(store, &self.location) {
            return;
        }
        connect(
            &self.channels,
            store,
            self.documents,
            &self.location,
            &self.client,
            &self.session,
        );
    }
}

/// A life ended and its subscription is gone. Retire the client entry;
/// a reopen that queued behind the drain connects HERE — strictly
/// after the old unsubscribe.
struct Drained {
    channels: Arc<DocumentChannels>,
    documents: imba::store::Id<documents::OpenDocuments>,
    location: ResourceLocation,
}

impl imba::command::DynamicCommand for Drained {
    fn id(&self) -> &'static str {
        "docsync.drained"
    }
    fn name(&self) -> String {
        "Retire Document Channel".to_owned()
    }
    fn perform(&self, store: &mut Store, ui: &imba::ui::UiCtx, fx: &mut imba::command::Fx<'_>) {
        match SyncClients::retire(store, &self.location) {
            Some(ClientState::Draining {
                reopen: Some(reopen),
            }) => {
                connect(
                    &self.channels,
                    store,
                    self.documents,
                    &self.location,
                    &reopen.client,
                    &reopen.session,
                );
            }
            Some(ClientState::Live(_)) => {
                // The life died on its own (the wire went away): fall
                // back to mode two — the client watches and reloads
                // the resource itself.
                self.channels.forget_store(&self.location);
                {
                    let documents = self.documents;
                    if let Some(document) =
                        documents::OpenDocuments::by_location(store, documents, &self.location)
                    {
                        documents::OpenDocuments::set_host_synced(
                            store, documents, document, false, fx,
                        );
                        documents::lanes::sync_document_watches(store, documents, fx);
                        documents::lanes::refetch_document(store, documents, document, fx);
                    }
                }
            }
            _ => {
                self.channels.forget_store(&self.location);
            }
        }
    }
}

struct AdoptSnapshot {
    channels: Arc<DocumentChannels>,
    documents: imba::store::Id<documents::OpenDocuments>,
    server: Arc<dyn DocumentsClient>,
    document: himark_ahp_ext_types::Uri,
    location: ResourceLocation,
    snapshot: String,
    version: Uid,
    seeded: mpsc::Sender<Seeded>,
}

impl imba::command::DynamicCommand for AdoptSnapshot {
    fn id(&self) -> &'static str {
        "docsync.adopt"
    }
    fn name(&self) -> String {
        "Adopt Document Channel".to_owned()
    }
    fn perform(&self, store: &mut Store, ui: &imba::ui::UiCtx, fx: &mut imba::command::Fx<'_>) {
        let Some((stop, since)) = SyncClients::connecting(store, &self.location) else {
            return;
        };
        let documents = self.documents;
        let Some(document_id) =
            documents::OpenDocuments::by_location(store, documents, &self.location)
        else {
            SyncClients::detach(store, &self.location);
            self.channels.forget_store(&self.location);
            return;
        };
        let Some(document) =
            documents::OpenDocuments::document_ref(store, documents, document_id).cloned()
        else {
            SyncClients::detach(store, &self.location);
            self.channels.forget_store(&self.location);
            return;
        };
        let log = document.log();
        let since = since.min(log.revision());

        let at_open = match log.compose_since(since) {
            Some(meanwhile) => document.text().edit(&meanwhile.invert()),
            None => document.text().clone(),
        };
        let snapshot = text::text::Text::from_string_exact(&self.snapshot);
        let mut history = log.as_of(since);
        // A patch, not a picture: the minimal exact edit
        // (docs/editor/structural-diff.md, decision 3).
        let adopt = myersdiff::diff(&at_open, &snapshot);
        let adopted = !rules::is_identity(&adopt);
        if adopted {
            let old_len = at_open.byte_count().min(u32::MAX as usize) as u32;
            history.record(&adopt, old_len);
        }
        let committed = SyncState::new(snapshot, history);

        let (edits, edits_rx) = mpsc::unbounded_channel();
        for (revision, (identity, op)) in (since..).zip(log.entries_since(since)) {
            let _ = edits.send(Local::Edit(SyncEdit::captured(
                log.as_of(revision),
                op.clone(),
                identity,
            )));
        }
        SyncClients::put(
            store,
            self.location.clone(),
            ClientState::Live(Client {
                edits: edits.clone(),
                stop,
                attached_at: since,
                taken: 0,
                applied: None,
            }),
        );
        self.channels.store_ready(
            &self.location,
            StoreHandle {
                edits,
                server: Arc::clone(&self.server),
                channel: self.document.clone(),
            },
        );
        // The host is the source of truth from here: it watches the
        // file and broadcasts reloads as its own edits; this client
        // stops watching (docs: agents edit files, clients edit
        // documents).
        documents::OpenDocuments::set_host_synced(store, documents, document_id, true, fx);

        if adopted {
            let shown = document.text().byte_count().min(u32::MAX as usize) as u32;
            let base_revision = document.revision();
            let landing = committed
                .slice_from(document.log())
                .filter(|slice| !rules::is_identity(slice) && slice.old_len() == shown)
                .and_then(|slice| Some((committed.log.head()?, slice)));
            if let Some((identity, slice)) = landing {
                SyncClients::expect(store, &self.location, identity);
                let applied = fx.scope(
                    move |command| {
                        Verb::at(
                            documents,
                            documents::DocumentsCommand::Editor(document_id, command),
                        )
                    },
                    |fx| {
                        documents::OpenDocuments::edit_shared(
                            store,
                            documents,
                            ui,
                            document_id,
                            identity,
                            base_revision,
                            &slice,
                            fx,
                        )
                    },
                );
                if applied {
                    SyncClients::took(store, &self.location);
                }
            }
        }

        let _ = self.seeded.try_send((committed, self.version, edits_rx));
    }
}

struct GiveUp {
    channels: Arc<DocumentChannels>,
    documents: imba::store::Id<documents::OpenDocuments>,
    location: ResourceLocation,
}

impl imba::command::DynamicCommand for GiveUp {
    fn id(&self) -> &'static str {
        "docsync.give-up"
    }
    fn name(&self) -> String {
        "Release Document Channel".to_owned()
    }
    fn perform(&self, store: &mut Store, ui: &imba::ui::UiCtx, fx: &mut imba::command::Fx<'_>) {
        if SyncClients::connecting(store, &self.location).is_some() {
            SyncClients::detach(store, &self.location);
        }
        self.channels.forget_store(&self.location);
        // Mode two: no document channel — this client subscribes to
        // the resource itself and reloads on its own.
        {
            let documents = self.documents;
            if let Some(document) =
                documents::OpenDocuments::by_location(store, documents, &self.location)
            {
                documents::OpenDocuments::set_host_synced(store, documents, document, false, fx);
                documents::lanes::sync_document_watches(store, documents, fx);
                documents::lanes::refetch_document(store, documents, document, fx);
            }
        }
    }
}

struct ApplyOffer {
    documents: imba::store::Id<documents::OpenDocuments>,
    location: ResourceLocation,
    offer: Offer<SyncState>,
}

impl imba::command::DynamicCommand for ApplyOffer {
    fn id(&self) -> &'static str {
        "docsync.offer"
    }
    fn name(&self) -> String {
        "Apply Document Offer".to_owned()
    }
    fn perform(&self, store: &mut Store, ui: &imba::ui::UiCtx, fx: &mut imba::command::Fx<'_>) {
        let Some(client) = SyncClients::client(store, &self.location) else {
            return;
        };
        let documents = self.documents;
        let Some(document_id) =
            documents::OpenDocuments::by_location(store, documents, &self.location)
        else {
            return;
        };
        let Some(document) = documents::OpenDocuments::document_ref(store, documents, document_id)
        else {
            return;
        };
        let shown = document.text().byte_count().min(u32::MAX as usize) as u32;
        if self.offer.seen_local != rules::sent(document.revision(), client.attached_at, client.taken) {
            return;
        }

        let _ = client.edits.send(Local::Took {
            seen_local: self.offer.seen_local,
        });
        let Some((identity, slice)) = rules::offer_landing(
            document.log(),
            document.revision(),
            client.attached_at,
            client.taken,
            shown,
            &self.offer,
        ) else {
            return;
        };
        let base_revision = document.revision();

        SyncClients::expect(store, &self.location, identity);
        let applied = fx.scope(
            move |command| {
                Verb::at(
                    documents,
                    documents::DocumentsCommand::Editor(document_id, command),
                )
            },
            |fx| {
                documents::OpenDocuments::edit_shared(
                    store,
                    documents,
                    ui,
                    document_id,
                    identity,
                    base_revision,
                    &slice,
                    fx,
                )
            },
        );
        if applied {
            SyncClients::took(store, &self.location);
        }
    }
}

pub struct SyncSink;

impl editor::change_sink::ChangeSink for SyncSink {
    fn changed(
        &self,
        store: &Store,
        document: &editor::document::Document,
        location: &editor::location::ResourceLocation,
        base_revision: u64,
        _text_before: &text::text::Text,
        _fx: &mut editor::editor::EditorEffects<'_>,
    ) {
        let Some(client) = SyncClients::client(store, location) else {
            return;
        };
        let Some(edit) = rules::seam_edit(document.log(), base_revision, client.applied) else {
            return;
        };
        let _ = client.edits.send(Local::Edit(edit));
    }
}

pub struct DocsyncHook {
    pub channels: Arc<DocumentChannels>,
    pub directory: Arc<ahp_wire::fs::ClientDirectory>,
}

impl documents::DocumentHook for DocsyncHook {
    fn opened(
        &self,
        _store: &mut Store,
        documents: imba::store::Id<documents::OpenDocuments>,
        _document: documents::DocumentId,
        location: Option<&editor::location::ResourceLocation>,
    ) {
        // The invariant (2026-09-15): every located document that
        // talks to the outside world is registered in OpenDocuments —
        // so registration is where its document channel gets ensured.
        let Some(location) = location else {
            return;
        };
        if documents::is_synthetic(location) || !location.kind().is_document() {
            return;
        }
        let Some((client, session)) = ahp_wire::fs::client_of(&self.directory, location) else {
            return;
        };
        DocumentChannels::ensure(
            &self.channels,
            documents,
            location.clone(),
            client.documents,
            session.into_string(),
        );
    }

    fn closing(
        &self,
        store: &mut Store,
        _documents: imba::store::Id<documents::OpenDocuments>,
        _document: documents::DocumentId,
        location: Option<&editor::location::ResourceLocation>,
        _doc: &editor::document::Document,
    ) {
        // The row leaves the collection right after this hook — no
        // flag writes back into it; the release already unsubscribes
        // its watch.
        if let Some(location) = location {
            SyncClients::detach(store, location);
            self.channels.forget_store(location);
        }
    }
}
