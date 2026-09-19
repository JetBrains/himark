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
//! The store's `SyncSeats` holds the per-location state; the spawned
//! life task (`channel_life`) owns the wire: open → subscribe →
//! adopt → pumps → poll. Shutdown is COOPERATIVE (a stop signal,
//! never an abort): every exit past the subscribe UNSUBSCRIBES the
//! channel, awaited, and then posts `Drained` — so a reopen queued
//! on a draining seat connects strictly AFTER the old subscription
//! is gone. One location, one subscription, ever.

mod codec;
mod rules;

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

    stop: mpsc::UnboundedSender<()>,

    attached_at: u64,

    taken: u64,

    applied: Option<EditIdentity>,
}

/// A seat waiting to connect again once the draining life is gone.
#[derive(Clone)]
struct Reopen {
    seat: Arc<dyn AhpServer>,
    session: String,
}

#[derive(Clone)]
enum SeatState {
    Connecting {
        stop: mpsc::UnboundedSender<()>,

        since: u64,
    },
    Live(Seat),
    /// The life was told to stop and is unsubscribing; `Drained`
    /// retires the entry. An open arriving meanwhile parks here and
    /// connects from `Drained` — never beside the dying life.
    Draining {
        reopen: Option<Reopen>,
    },
}

impl SeatState {
    /// COOPERATIVE shutdown, never an abort: the life task owns the
    /// channel subscription and must live long enough to unsubscribe
    /// it — an aborted task cannot, and a leaked subscription doubles
    /// every broadcast the next time the location opens.
    fn stop(&self) {
        match self {
            Self::Connecting { stop, .. } => {
                let _ = stop.send(());
            }
            Self::Live(seat) => {
                let _ = seat.stop.send(());
            }
            Self::Draining { .. } => {}
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
            SeatState::Connecting { .. } | SeatState::Draining { .. } => None,
        }
    }

    fn connecting(
        store: &Store,
        location: &ResourceLocation,
    ) -> Option<(mpsc::UnboundedSender<()>, u64)> {
        match store.get::<SyncSeats>()?.seats.get(location)? {
            SeatState::Connecting { stop, since } => Some((stop.clone(), *since)),
            SeatState::Live(_) | SeatState::Draining { .. } => None,
        }
    }

    fn known(store: &Store, location: &ResourceLocation) -> bool {
        store
            .get::<SyncSeats>()
            .is_some_and(|seats| seats.seats.contains_key(location))
    }

    fn draining(store: &Store, location: &ResourceLocation) -> bool {
        store.get::<SyncSeats>().is_some_and(|seats| {
            matches!(seats.seats.get(location), Some(SeatState::Draining { .. }))
        })
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

    /// The one exit door: stop the life (it will unsubscribe and
    /// post `Drained`) and mark the entry draining. Detaching a
    /// still-draining entry only clears a queued reopen — the
    /// document that wanted back in is itself gone.
    fn detach(store: &mut Store, location: &ResourceLocation) {
        let Some(state) = store
            .get::<SyncSeats>()
            .and_then(|seats| seats.seats.get(location))
        else {
            return;
        };
        state.stop();
        Self::put(
            store,
            location.clone(),
            SeatState::Draining { reopen: None },
        );
    }

    fn queue_reopen(store: &mut Store, location: &ResourceLocation, reopen: Reopen) {
        if Self::draining(store, location) {
            Self::put(
                store,
                location.clone(),
                SeatState::Draining {
                    reopen: Some(reopen),
                },
            );
        }
    }

    fn retire(store: &mut Store, location: &ResourceLocation) -> Option<SeatState> {
        let mut seats = Self::of(store);
        let retired = seats.seats.get(location).cloned();
        seats.seats.remove_mut(location);
        store.put(seats);
        retired
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
    post: Arc<dyn Fn(AppCommand) + Send + Sync>,
    uris: Arc<dyn himark::higent::ResourceUriMap>,
    runtime: tokio::runtime::Handle,

    salt: u128,
    state: std::sync::Mutex<ChannelState>,
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

/// Spawn a fresh life for the location. Callers guarantee no other
/// life exists: `EnsureSync` connects only an unknown location, and
/// `Drained` connects only after the previous life unsubscribed.
fn connect(
    channels: &Arc<DocumentChannels>,
    store: &mut Store,
    location: &ResourceLocation,
    seat: &Arc<dyn AhpServer>,
    session: &str,
) {
    channels.store_connecting(location);
    let since = himark::OpenDocuments::by_location(store, location)
        .and_then(|id| himark::OpenDocuments::document_ref(store, id))
        .map(|document| document.revision())
        .unwrap_or_default();
    let (stop, stopped) = mpsc::unbounded_channel();
    channels.runtime.spawn(channel_life(
        Arc::clone(channels),
        location.clone(),
        Arc::clone(seat),
        session.to_owned(),
        stopped,
    ));
    SyncSeats::put(
        store,
        location.clone(),
        SeatState::Connecting { stop, since },
    );
}

type Seeded = (SyncState, Uid, mpsc::UnboundedReceiver<Local<SyncEdit>>);

async fn channel_life(
    channels: Arc<DocumentChannels>,
    location: ResourceLocation,
    server: Arc<dyn AhpServer>,
    session: String,
    mut stopped: mpsc::UnboundedReceiver<()>,
) {
    life(&channels, &location, server, session, &mut stopped).await;
    // The last word, ALWAYS: whatever road ended the life, the
    // subscription is gone by now, so a queued reopen may connect.
    channels.post(Drained {
        channels: Arc::clone(&channels),
        location,
    });
}

async fn life(
    channels: &Arc<DocumentChannels>,
    location: &ResourceLocation,
    server: Arc<dyn AhpServer>,
    session: String,
    stopped: &mut mpsc::UnboundedReceiver<()>,
) {
    let uri = channels.uris.uri_of(location);
    let opened = tokio::select! {
        biased;
        // Stopped before the subscribe was ever sent: nothing to
        // release — an open at most mints (or re-finds) the channel.
        _ = stopped.recv() => return,
        opened = server.open_document(session, Some(uri), None) => match opened {
            Ok(opened) => opened,
            Err(error) => {
                tracing::warn!(%error, "docsync: could not reach the channel");
                channels.post(GiveUp {
                    channels: Arc::clone(channels),
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
    let snapshot = match server.subscribe_document(opened.document.clone()).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::warn!(%error, "docsync: could not subscribe the channel");
            server.unsubscribe_document(&opened.document).await;
            channels.post(GiveUp {
                channels: Arc::clone(channels),
                location: location.clone(),
            });
            return;
        }
    };
    if stopped.try_recv().is_ok() {
        server.unsubscribe_document(&opened.document).await;
        return;
    }

    let (seeded, mut seed) = mpsc::channel::<Seeded>(1);
    channels.post(AdoptSnapshot {
        channels: Arc::clone(channels),
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
        server.unsubscribe_document(&opened.document).await;
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
        let channels = Arc::clone(channels);
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
        let heard = tokio::select! {
            biased;
            _ = stopped.recv() => {
                server.unsubscribe_document(&opened.document).await;
                return;
            }
            heard = server.poll_document(opened.document.clone()) => heard,
        };
        for action in heard {
            let applied = rebase::Applied {
                id: action.id,
                action: SyncEdit::Theirs {
                    resolve: resolve_wire(action.operation),
                },
            };
            if actions.send(applied).await.is_err() {
                server.unsubscribe_document(&opened.document).await;
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
        if SyncSeats::draining(store, &self.location) {
            SyncSeats::queue_reopen(
                store,
                &self.location,
                Reopen {
                    seat: Arc::clone(&self.seat),
                    session: self.session.clone(),
                },
            );
            return;
        }
        if SyncSeats::known(store, &self.location) {
            return;
        }
        connect(
            &self.channels,
            store,
            &self.location,
            &self.seat,
            &self.session,
        );
    }
}

/// A life ended and its subscription is gone. Retire the seat entry;
/// a reopen that queued behind the drain connects HERE — strictly
/// after the old unsubscribe.
struct Drained {
    channels: Arc<DocumentChannels>,
    location: ResourceLocation,
}

impl himark::DynamicCommand for Drained {
    fn id(&self) -> &'static str {
        "docsync.drained"
    }
    fn name(&self) -> String {
        "Retire Document Channel".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        store: &mut Store,
        _window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        match SyncSeats::retire(store, &self.location) {
            Some(SeatState::Draining {
                reopen: Some(reopen),
            }) => {
                connect(
                    &self.channels,
                    store,
                    &self.location,
                    &reopen.seat,
                    &reopen.session,
                );
            }
            Some(SeatState::Live(_)) => {
                // The life died on its own (the wire went away): fall
                // back to mode two — the client watches and reloads
                // the resource itself.
                self.channels.forget_store(&self.location);
                if let Some(document) = himark::OpenDocuments::by_location(store, &self.location) {
                    himark::OpenDocuments::set_host_synced(store, document, false, fx);
                    himark::sync_document_watches(store, fx);
                    himark::refetch_document(store, document, fx);
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
        let Some((stop, since)) = SyncSeats::connecting(store, &self.location) else {
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
        SyncSeats::put(
            store,
            self.location.clone(),
            SeatState::Live(Seat {
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
