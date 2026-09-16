// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

mod codec;
mod rules;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use himark::higent::AhpServer;
use himark::{AppCommand, ResourceLocation};
use himark_ahp_ext_types::{DocumentApplied, Uid};
use imba::store::Store;
use rebase::{Local, Offer, RebaseLog};
use tokio::sync::{mpsc, oneshot, watch};

use documents::sync::{SyncEdit, SyncState};
use editor::EditIdentity;

#[doc(hidden)]
pub use codec::{resolve_wire, wire_operation};

#[derive(Clone)]
struct Seat {
    edits: mpsc::UnboundedSender<Local<SyncEdit>>,

    abort: tokio::task::AbortHandle,

    attached_at: u64,

    taken: u64,

    applied: Option<EditIdentity>,
}

#[derive(Clone)]
enum SeatState {
    Connecting {
        abort: tokio::task::AbortHandle,

        since: u64,
    },
    Live(Seat),
}

impl SeatState {
    fn abort(&self) {
        match self {
            Self::Connecting { abort, .. } => abort.abort(),
            Self::Live(seat) => seat.abort.abort(),
        }
    }
}

#[derive(Clone, Default)]
pub struct SyncSeats {
    seats: rpds::HashTrieMapSync<ResourceLocation, SeatState>,
}

impl SyncSeats {
    fn of(store: &Store) -> Self {
        store
            .get::<SyncSeats>()
            .map(|seats| seats.clone())
            .unwrap_or_default()
    }

    fn seat(store: &Store, location: &ResourceLocation) -> Option<Seat> {
        match store.get::<SyncSeats>()?.seats.get(location)? {
            SeatState::Live(seat) => Some(seat.clone()),
            SeatState::Connecting { .. } => None,
        }
    }

    fn connecting(
        store: &Store,
        location: &ResourceLocation,
    ) -> Option<(tokio::task::AbortHandle, u64)> {
        match store.get::<SyncSeats>()?.seats.get(location)? {
            SeatState::Connecting { abort, since } => Some((abort.clone(), *since)),
            SeatState::Live(_) => None,
        }
    }

    fn known(store: &Store, location: &ResourceLocation) -> bool {
        store
            .get::<SyncSeats>()
            .is_some_and(|seats| seats.seats.contains_key(location))
    }

    fn put(store: &mut Store, location: ResourceLocation, state: SeatState) {
        let mut seats = Self::of(store);
        seats.seats.insert_mut(location, state);
        store.put(seats);
    }

    fn expect(store: &mut Store, location: &ResourceLocation, identity: EditIdentity) {
        let Some(seat) = Self::seat(store, location) else {
            return;
        };
        Self::put(
            store,
            location.clone(),
            SeatState::Live(Seat {
                applied: Some(identity),
                ..seat
            }),
        );
    }

    fn took(store: &mut Store, location: &ResourceLocation) {
        let Some(seat) = Self::seat(store, location) else {
            return;
        };
        Self::put(
            store,
            location.clone(),
            SeatState::Live(Seat {
                taken: seat.taken + 1,
                ..seat
            }),
        );
    }

    fn detach(store: &mut Store, location: &ResourceLocation) {
        let mut seats = Self::of(store);
        if let Some(state) = seats.seats.get(location) {
            state.abort();
        }
        seats.seats.remove_mut(location);
        store.put(seats);
    }

    #[doc(hidden)]
    pub fn count(store: &Store) -> usize {
        Self::of(store).seats.size()
    }
}

/// The save lane of a live document channel: the flush marker rides
/// the same queue as the edits, so the host's dump lands only after
/// every edit enqueued before the save is committed.
#[derive(Clone)]
pub struct StoreHandle {
    edits: mpsc::UnboundedSender<Local<SyncEdit>>,
    server: Arc<dyn AhpServer>,
    channel: himark_ahp_ext_types::Uri,
}

impl StoreHandle {
    pub async fn store(self, uri: himark::higent::seat::ResourceUri) -> bool {
        let (done, landed) = oneshot::channel();
        if self.edits.send(Local::Flush(done)).is_err() {
            return false;
        }
        if landed.await.is_err() {
            return false;
        }
        match self.server.store_document(self.channel, uri).await {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "docsync: the host could not store the document");
                false
            }
        }
    }
}

pub struct DocumentChannels {
    post: Arc<dyn Fn(AppCommand) + Send + Sync>,
    uris: Arc<dyn himark::higent::ResourceUriMap>,
    runtime: tokio::runtime::Handle,

    salt: u128,
    minted: AtomicU64,

    stores: std::sync::Mutex<
        std::collections::HashMap<ResourceLocation, watch::Sender<Option<StoreHandle>>>,
    >,
}

impl DocumentChannels {
    pub fn new(
        runtime: tokio::runtime::Handle,
        post: Arc<dyn Fn(AppCommand) + Send + Sync>,
        uris: Arc<dyn himark::higent::ResourceUriMap>,
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
            minted: AtomicU64::new(1),
            stores: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    fn store_connecting(&self, location: &ResourceLocation) {
        let (route, _) = watch::channel(None);
        self.stores
            .lock()
            .expect("store routes")
            .insert(location.clone(), route);
    }

    fn store_ready(&self, location: &ResourceLocation, handle: StoreHandle) {
        if let Some(route) = self.stores.lock().expect("store routes").get(location) {
            // send_replace: a plain send is DROPPED while no save is
            // subscribed, and the handle must outwait its receivers.
            route.send_replace(Some(handle));
        }
    }

    pub(crate) fn forget_store(&self, location: &ResourceLocation) {
        self.stores.lock().expect("store routes").remove(location);
    }

    /// `None`: no document channel — the resource itself is the
    /// truth, store it directly. `Some`: the host mirrors this
    /// document and the save must flow through the channel. Waits
    /// out the connecting window so a save cannot slip UNDER a
    /// channel being adopted.
    pub async fn store_handle(&self, location: &ResourceLocation) -> Option<StoreHandle> {
        let mut route = {
            let stores = self.stores.lock().expect("store routes");
            stores.get(location)?.subscribe()
        };
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
        Uid(self.salt ^ u128::from(self.minted.fetch_add(1, Ordering::SeqCst)))
    }

    pub fn ensure(
        self: &Arc<Self>,
        location: ResourceLocation,
        seat: Arc<dyn AhpServer>,
        session: String,
    ) {
        self.post(EnsureSync {
            channels: Arc::clone(self),
            location,
            seat,
            session,
        });
    }

    fn post(&self, command: impl himark::DynamicCommand + 'static) {
        (self.post)(AppCommand::Dynamic(
            himark::WindowId::from_raw(0),
            Arc::new(command),
        ));
    }
}

type Seeded = (SyncState, Uid, mpsc::UnboundedReceiver<Local<SyncEdit>>);

async fn channel_life(
    channels: Arc<DocumentChannels>,
    location: ResourceLocation,
    server: Arc<dyn AhpServer>,
    session: String,
) {
    let uri = channels.uris.uri_of(&location);
    let subscribed = async {
        let opened = server
            .open_document(session, Some(uri), None)
            .await
            .map_err(|error| format!("openDocument: {error}"))?;
        let snapshot = server
            .subscribe_document(opened.document.clone())
            .await
            .map_err(|error| format!("subscribe: {error}"))?;
        Ok::<_, String>((opened, snapshot))
    };
    let (opened, snapshot) = match subscribed.await {
        Ok(subscribed) => subscribed,
        Err(error) => {
            tracing::warn!(%error, "docsync: could not reach the channel");
            channels.post(GiveUp {
                channels: Arc::clone(&channels),
                location: location.clone(),
            });
            return;
        }
    };

    let (seeded, mut seed) = mpsc::channel::<Seeded>(1);
    channels.post(AdoptSnapshot {
        channels: Arc::clone(&channels),
        server: Arc::clone(&server),
        document: opened.document.clone(),
        location: location.clone(),
        snapshot: snapshot.text.clone(),
        version: snapshot.version,
        seeded,
    });
    let Some((state, version, edits)) = seed.recv().await else {
        // Adoption declined (no registered document): release the
        // channel — a leaked subscription re-subscribes later and
        // every broadcast arrives twice (the 2026-09-15 doubling).
        server.unsubscribe_document(&opened.document);
        return;
    };

    let (actions, actions_rx) = mpsc::channel(64);
    let (wire, mut wire_rx) = mpsc::channel(64);
    let (offers, mut offers_rx) = mpsc::channel(8);
    let mint = {
        let channels = Arc::clone(&channels);
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
        let channel = opened.document.clone();
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
        let channels = Arc::clone(&channels);
        let location = location.clone();
        let runtime = channels.runtime.clone();
        runtime.spawn(async move {
            while let Some(offer) = offers_rx.recv().await {
                channels.post(ApplyOffer {
                    location: location.clone(),
                    offer,
                });
            }
        });
    }

    loop {
        let heard = server.poll_document(opened.document.clone()).await;
        for action in heard {
            let applied = rebase::Applied {
                id: action.id,
                action: SyncEdit::Theirs {
                    resolve: resolve_wire(action.operation),
                },
            };
            if actions.send(applied).await.is_err() {
                return;
            }
        }
    }
}

struct EnsureSync {
    channels: Arc<DocumentChannels>,
    location: ResourceLocation,
    seat: Arc<dyn AhpServer>,
    session: String,
}

impl himark::DynamicCommand for EnsureSync {
    fn id(&self) -> &'static str {
        "docsync.ensure"
    }
    fn name(&self) -> String {
        "Sync Document".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        store: &mut Store,
        _window: himark::WindowId,
        _fx: &mut himark::AppFx<'_>,
    ) {
        if SyncSeats::known(store, &self.location) {
            return;
        }
        self.channels.store_connecting(&self.location);

        let since = himark::OpenDocuments::by_location(store, &self.location)
            .and_then(|id| himark::OpenDocuments::document_ref(store, id))
            .map(|document| document.revision())
            .unwrap_or_default();
        let task = self.channels.runtime.spawn(channel_life(
            Arc::clone(&self.channels),
            self.location.clone(),
            Arc::clone(&self.seat),
            self.session.clone(),
        ));
        SyncSeats::put(
            store,
            self.location.clone(),
            SeatState::Connecting {
                abort: task.abort_handle(),
                since,
            },
        );
    }
}

struct AdoptSnapshot {
    channels: Arc<DocumentChannels>,
    server: Arc<dyn AhpServer>,
    document: himark_ahp_ext_types::Uri,
    location: ResourceLocation,
    snapshot: String,
    version: Uid,
    seeded: mpsc::Sender<Seeded>,
}

impl himark::DynamicCommand for AdoptSnapshot {
    fn id(&self) -> &'static str {
        "docsync.adopt"
    }
    fn name(&self) -> String {
        "Adopt Document Channel".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        store: &mut Store,
        _window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        let Some((abort, since)) = SyncSeats::connecting(store, &self.location) else {
            return;
        };
        let Some(document_id) = himark::OpenDocuments::by_location(store, &self.location) else {
            SyncSeats::detach(store, &self.location);
            self.channels.forget_store(&self.location);
            return;
        };
        let Some(document) = himark::OpenDocuments::document_ref(store, document_id).cloned()
        else {
            SyncSeats::detach(store, &self.location);
            self.channels.forget_store(&self.location);
            return;
        };
        let log = document.log();
        let since = since.min(log.revision());

        let at_open = match log.compose_since(since) {
            Some(meanwhile) => document.text().edit(&meanwhile.invert()),
            None => document.text().clone(),
        };
        let snapshot = himark::Text::from_string_exact(&self.snapshot);
        let mut history = log.as_of(since);
        let adopt = himark::diff::diff(&at_open, &snapshot);
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
        SyncSeats::put(
            store,
            self.location.clone(),
            SeatState::Live(Seat {
                edits: edits.clone(),
                abort,
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
        himark::OpenDocuments::set_host_synced(store, document_id, true, fx);

        if adopted {
            let shown = document.text().byte_count().min(u32::MAX as usize) as u32;
            let base_revision = document.revision();
            let landing = committed
                .slice_from(document.log())
                .filter(|slice| !rules::is_identity(slice) && slice.old_len() == shown)
                .and_then(|slice| Some((committed.log.head()?, slice)));
            if let Some((identity, slice)) = landing {
                SyncSeats::expect(store, &self.location, identity);
                let applied = himark::entity_scope(document_id, fx, |fx| {
                    himark::OpenDocuments::edit_shared(
                        store,
                        document_id,
                        identity,
                        base_revision,
                        &slice,
                        fx,
                    )
                });
                if applied {
                    SyncSeats::took(store, &self.location);
                }
            }
        }

        let _ = self.seeded.try_send((committed, self.version, edits_rx));
    }
}

struct GiveUp {
    channels: Arc<DocumentChannels>,
    location: ResourceLocation,
}

impl himark::DynamicCommand for GiveUp {
    fn id(&self) -> &'static str {
        "docsync.give-up"
    }
    fn name(&self) -> String {
        "Release Document Channel".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        store: &mut Store,
        _window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        if SyncSeats::connecting(store, &self.location).is_some() {
            SyncSeats::detach(store, &self.location);
        }
        self.channels.forget_store(&self.location);
        // Mode two: no document channel — this client subscribes to
        // the resource itself and reloads on its own.
        if let Some(document) = himark::OpenDocuments::by_location(store, &self.location) {
            himark::OpenDocuments::set_host_synced(store, document, false, fx);
            himark::sync_document_watches(store, fx);
            himark::refetch_document(store, document, fx);
        }
    }
}

struct ApplyOffer {
    location: ResourceLocation,
    offer: Offer<SyncState>,
}

impl himark::DynamicCommand for ApplyOffer {
    fn id(&self) -> &'static str {
        "docsync.offer"
    }
    fn name(&self) -> String {
        "Apply Document Offer".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        store: &mut Store,
        _window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        let Some(seat) = SyncSeats::seat(store, &self.location) else {
            return;
        };
        let Some(document_id) = himark::OpenDocuments::by_location(store, &self.location) else {
            return;
        };
        let Some(document) = himark::OpenDocuments::document_ref(store, document_id) else {
            return;
        };
        let shown = document.text().byte_count().min(u32::MAX as usize) as u32;
        if self.offer.seen_local != rules::sent(document.revision(), seat.attached_at, seat.taken) {
            return;
        }

        let _ = seat.edits.send(Local::Took {
            seen_local: self.offer.seen_local,
        });
        let Some((identity, slice)) = rules::offer_landing(
            document.log(),
            document.revision(),
            seat.attached_at,
            seat.taken,
            shown,
            &self.offer,
        ) else {
            return;
        };
        let base_revision = document.revision();

        SyncSeats::expect(store, &self.location, identity);
        let applied = himark::entity_scope(document_id, fx, |fx| {
            himark::OpenDocuments::edit_shared(
                store,
                document_id,
                identity,
                base_revision,
                &slice,
                fx,
            )
        });
        if applied {
            SyncSeats::took(store, &self.location);
        }
    }
}

pub struct SyncSink;

impl editor::ChangeSink for SyncSink {
    fn changed(
        &self,
        store: &Store,
        document: &editor::Document,
        location: &editor::ResourceLocation,
        base_revision: u64,
        _text_before: &himark::Text,
        _fx: &mut editor::EditorEffects<'_>,
    ) {
        let Some(seat) = SyncSeats::seat(store, location) else {
            return;
        };
        let Some(edit) = rules::seam_edit(document.log(), base_revision, seat.applied) else {
            return;
        };
        let _ = seat.edits.send(Local::Edit(edit));
    }
}

pub struct DocsyncHook {
    pub channels: Arc<DocumentChannels>,
    pub directory: Arc<crate::fs::SeatDirectory>,
}

impl himark::DocumentHook for DocsyncHook {
    fn opened(&self, store: &mut Store, document: himark::DocumentId) {
        // The invariant (2026-09-15): every located document that
        // talks to the outside world is registered in OpenDocuments —
        // so registration is where its document channel gets ensured.
        let Some(location) = himark::OpenDocuments::location(store, document) else {
            return;
        };
        if himark::is_synthetic(&location) || !location.kind().is_document() {
            return;
        }
        let Some((seat, session)) = crate::fsroute::seat_of(&self.directory, &location) else {
            return;
        };
        DocumentChannels::ensure(&self.channels, location, seat, session);
    }

    fn closing(&self, store: &mut Store, document: himark::DocumentId) {
        if let Some(location) = himark::OpenDocuments::location(store, document) {
            SyncSeats::detach(store, &location);
            self.channels.forget_store(&location);
        }
        let mut throwaway: imba::effect::Batch<imba::DynCommand> = imba::effect::Batch::new();
        himark::OpenDocuments::set_host_synced(store, document, false, &mut throwaway.effects());
    }
}
