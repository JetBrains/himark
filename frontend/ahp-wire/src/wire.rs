// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use ahp_types::actions::{RootConfigChangedAction, StateAction};
use ahp_types::commands::{
    ContentEncoding, CreateChatParams, CreateResourceWatchParams, CreateResourceWatchResult,
    DisposeSessionParams, FetchTurnsParams, ListSessionsParams, ListSessionsResult,
    ResourceDeleteParams, ResourceListParams, ResourceListResult, ResourceMoveParams,
    ResourceReadParams, ResourceWriteParams, SubscribeResult,
};
use ahp_types::common::Uri;
use ahp_types::state::{
    AgentInfo, ChatState, Message, MessageKind, MessageOrigin, ModelSelection, SessionState,
    SnapshotState,
};

use crate::client::{
    AnnotationsClient, ChangesClient, ChatClient, DocumentsClient, HistoryClient, LocationsClient, LspClient,
    ResourceClient, RootInfo, ClientFuture, ServerEvent, SessionClient, SessionsPage, TerminalClient,
};
use crate::client::{FileEditContents, TurnsPage};

const ROOT: &str = "ahp-root://";

fn annotations_channel(session: &str) -> Uri {
    format!("{session}/annotations")
}

pub enum Discovery {
    Explicit(String),

    VsCode,

    /// The embedder's own way to find — or start — its agent host,
    /// injected at construction: the wire never reads lockfiles or
    /// spawns daemons itself.
    Resolver(Arc<dyn Fn() -> Result<String, String> + Send + Sync>),
}

impl std::fmt::Debug for Discovery {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Discovery::Explicit(url) => out.debug_tuple("Explicit").field(url).finish(),
            Discovery::VsCode => out.write_str("VsCode"),
            Discovery::Resolver(_) => out.write_str("Resolver"),
        }
    }
}

#[cfg(not(target_os = "emscripten"))]
#[doc(hidden)]
pub fn test_runtime() -> tokio::runtime::Handle {
    static HANDLE: std::sync::OnceLock<tokio::runtime::Handle> = std::sync::OnceLock::new();
    HANDLE
        .get_or_init(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .thread_name("himark-runtime")
                .build()
                .expect("the test runtime");
            let handle = runtime.handle().clone();
            std::mem::forget(runtime);
            handle
        })
        .clone()
}

pub struct WireHost {
    active: Arc<Mutex<Option<Arc<Active>>>>,

    /// Held for the whole of a reconnect. `active` is released while
    /// the new connection dials (a dial blocks for seconds), and every
    /// ask that lands meanwhile must WAIT for that one connection —
    /// not dial its own: two dials are two connections, the second
    /// carries no feeds, and whichever stores last orphans the
    /// other's subscriptions.
    reconnecting: Mutex<()>,

    /// Reconnect attempts, counted as they finish, with the last
    /// one's failure. An ask that waited out an attempt which then
    /// failed takes that failure instead of dialing again: a host
    /// that is down answers every waiter at once, not one dial
    /// (and one connect timeout) per waiter in a row.
    attempts: Mutex<Attempts>,

    /// Rung when a connection becomes ACTIVE. Polls that found no
    /// connection wait on this (or a backoff) before re-arming.
    connected: Arc<tokio::sync::Notify>,
    next_turn: AtomicU64,
    discovery: Discovery,

    runtime: tokio::runtime::Handle,

    tag: String,

    client_id: String,

    last_seen: Arc<std::sync::atomic::AtomicI64>,

    connector: Arc<dyn crate::transport::Connector>,
}

#[derive(Default)]
struct Attempts {
    finished: u64,
    failed: Option<String>,
}

/// How long a poll waits, with no connection to park on, before it
/// re-arms and the re-arm dials again.
const RECONNECT_BACKOFF: std::time::Duration = std::time::Duration::from_secs(3);

struct Active {
    client: ahp::Client,

    dead: Arc<std::sync::atomic::AtomicBool>,

    /// Rung once `dead` is set — by whoever set it, or by the death
    /// watch (`watch_death`) for the transport, which only has the
    /// flag. Parked polls and asks wait on this, not on a timer each.
    died: tokio::sync::Notify,

    feeds: Mutex<HashMap<Uri, Arc<Feed>>>,

    root: Mutex<Option<Arc<RootFeed>>>,

    agents: Mutex<Vec<AgentInfo>>,
}

impl Active {
    fn die(&self) {
        self.dead.store(true, std::sync::atomic::Ordering::Relaxed);
        self.died.notify_waiters();
    }

    /// Resolves once this connection is dead.
    async fn died(self: Arc<Self>) {
        loop {
            let notified = self.died.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.dead.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            notified.await;
        }
    }
}

impl WireHost {
    pub fn new(
        runtime: tokio::runtime::Handle,
        connector: Arc<dyn crate::transport::Connector>,
    ) -> Self {
        Self::with(Discovery::VsCode, runtime, connector)
    }

    pub fn at(
        runtime: tokio::runtime::Handle,
        connector: Arc<dyn crate::transport::Connector>,
        url: impl Into<String>,
    ) -> Self {
        Self::with(Discovery::Explicit(url.into()), runtime, connector)
    }

    pub fn discovered(
        runtime: tokio::runtime::Handle,
        connector: Arc<dyn crate::transport::Connector>,
        resolver: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    ) -> Self {
        Self::with(Discovery::Resolver(resolver), runtime, connector)
    }

    fn with(
        discovery: Discovery,
        runtime: tokio::runtime::Handle,
        connector: Arc<dyn crate::transport::Connector>,
    ) -> Self {
        static CLIENT: AtomicU64 = AtomicU64::new(1);
        let tag = format!("client#{}", CLIENT.fetch_add(1, Ordering::Relaxed));
        tracing::info!(target: "ahp_wire", client = %tag, ?discovery, "client opened");
        Self {
            active: Arc::new(Mutex::new(None)),
            reconnecting: Mutex::new(()),
            attempts: Mutex::new(Attempts::default()),
            connected: Arc::new(tokio::sync::Notify::new()),
            next_turn: AtomicU64::new(1),
            discovery,
            runtime,
            tag,
            client_id: uuid_v4(),
            last_seen: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            connector,
        }
    }

    fn discover_url(&self) -> Result<String, String> {
        match &self.discovery {
            Discovery::Explicit(url) => Ok(match url.split_once("://") {
                Some(("http", rest)) => format!("ws://{rest}"),
                Some(("https", rest)) => format!("wss://{rest}"),
                _ => url.clone(),
            }),
            Discovery::VsCode => {
                if let Ok(url) = std::env::var("HIMARK_AHP_URL") {
                    return Ok(url);
                }
                Self::discover_vscode_url()
            }
            Discovery::Resolver(resolve) => resolve(),
        }
    }

    fn discover_vscode_url() -> Result<String, String> {
        let home = std::env::var("HOME").map_err(|_| "no HOME".to_owned())?;
        let lock = format!("{home}/.vscode-server/cli/agent-host-stable.lock");
        let raw = std::fs::read_to_string(&lock).map_err(|error| {
            format!("no agent host lockfile at {lock} ({error}) — start one with `code agent host`")
        })?;
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Lockfile {
            host: String,
            port: u16,
            connection_token: Option<String>,
        }
        let lockfile: Lockfile =
            serde_json::from_str(&raw).map_err(|error| format!("bad lockfile: {error}"))?;
        let token = lockfile
            .connection_token
            .map(|token| format!("/?tkn={token}"))
            .unwrap_or_default();
        Ok(format!("ws://{}:{}{}", lockfile.host, lockfile.port, token))
    }

    fn run<T, F>(&self, work: impl FnOnce(Arc<Active>) -> F + Send + 'static) -> RunFuture<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, String>> + Send + 'static,
    {
        let slot = OneShot::new();
        let filler = slot.clone();
        let active = match self.ensure_active() {
            Ok(active) => active,
            Err(error) => {
                filler.fill(Err(error));
                return RunFuture { slot };
            }
        };
        let tag = self.tag.clone();
        self.runtime.spawn(async move {
            let outcome = work(active).await;
            if let Err(error) = &outcome {
                tracing::warn!(target: "ahp_wire", client = %tag, %error, "client call failed");
            }
            filler.fill(outcome);
        });
        RunFuture { slot }
    }

    fn run_ask<T, F>(&self, work: impl FnOnce(Arc<Active>) -> F + Send + 'static) -> RunFuture<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, String>> + Send + 'static,
    {
        let tag = self.tag.clone();
        self.run(move |active| async move {
            let died = Arc::clone(&active).died();
            tokio::select! {
                outcome = work(active) => outcome,
                () = died => {
                    tracing::warn!(target: "ahp_wire", client = %tag, "ask abandoned: the connection died under it");
                    Err("the agent host connection died".to_owned())
                }
            }
        })
    }

    /// A fire-and-forget ask that rides the LIVE connection or is
    /// DROPPED. These come straight from view perform code — a
    /// keystroke into a terminal, a resize mid-paint — and the UI
    /// thread must never wait out a reconnect, let alone dial one
    /// (15 seconds at best; the 2026-10-05 deadlock at worst). A
    /// gesture against a dead host asks nothing anyway — the
    /// reconnect's resume re-subscribes the channel whole.
    fn run_live<T, F>(&self, work: impl FnOnce(Arc<Active>) -> F + Send + 'static)
    where
        T: Send + 'static,
        F: Future<Output = Result<T, String>> + Send + 'static,
    {
        let Some(active) = self.try_active() else {
            tracing::warn!(target: "ahp_wire", client = %self.tag, "ask DROPPED: connection down");
            return;
        };
        let tag = self.tag.clone();
        self.runtime.spawn(async move {
            let died = Arc::clone(&active).died();
            tokio::select! {
                outcome = work(active) => {
                    if let Err(error) = outcome {
                        tracing::warn!(target: "ahp_wire", client = %tag, %error, "live ask failed");
                    }
                }
                () = died => {}
            }
        });
    }

    /// `run_ask` for a SUBSCRIBE: an ask the host answers the same
    /// however often it is made (one subscription per channel per
    /// connection). Died under a reconnect, it is made once more on
    /// the connection that comes up — a chat opened at the moment the
    /// keepalive gave up opens, instead of failing for the user to
    /// retry. The wait is bounded; nothing reconnects on this road.
    fn run_subscribe<T, F>(
        &self,
        work: impl Fn(Arc<Active>) -> F + Send + Sync + 'static,
    ) -> RunFuture<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, String>> + Send + 'static,
    {
        let tag = self.tag.clone();
        let connected = Arc::clone(&self.connected);
        let slot = Arc::clone(&self.active);
        self.run(move |active| async move {
            let died = Arc::clone(&active).died();
            let first = tokio::select! {
                outcome = work(active) => return outcome,
                () = died => Err("the agent host connection died".to_owned()),
            };
            tracing::warn!(target: "ahp_wire", client = %tag, "subscribe died under a reconnect: asking again on the next connection");
            let up = connected.notified();
            tokio::pin!(up);
            up.as_mut().enable();
            let next = {
                let held = slot.lock().expect("wire active").clone();
                held.filter(|active| !active.dead.load(std::sync::atomic::Ordering::Relaxed))
            };
            let active = match next {
                Some(active) => active,
                None => {
                    let waited = tokio::time::timeout(RECONNECT_BACKOFF * 4, up).await;
                    if waited.is_err() {
                        return first;
                    }
                    match slot.lock().expect("wire active").clone() {
                        Some(active) => active,
                        None => return first,
                    }
                }
            };
            let died = Arc::clone(&active).died();
            tokio::select! {
                outcome = work(active) => outcome,
                () = died => first,
            }
        })
    }

    /// The LIVE connection or nothing — no reconnect, no waiting.
    /// This is the only road a caller that must not block may take:
    /// view perform code on the UI thread, anything on the runtime.
    fn try_active(&self) -> Option<Arc<Active>> {
        self.active
            .lock()
            .expect("wire active")
            .as_ref()
            .filter(|active| !active.dead.load(std::sync::atomic::Ordering::Relaxed))
            .cloned()
    }

    fn ensure_active(&self) -> Result<Arc<Active>, String> {
        let live = |active: &Arc<Active>| !active.dead.load(std::sync::atomic::Ordering::Relaxed);
        if let Some(active) = self.try_active() {
            return Ok(active);
        }
        // A RUNTIME WORKER stops here: the dial below — and its
        // timeout — are driven by this very runtime (one worker).
        // Parking that worker on `reconnecting` freezes the reactor,
        // the in-flight dial never resolves and never times out, and
        // every other caller wedges behind the lock — the sampled
        // 2026-10-05 deadlock (main thread included). The pollers all
        // have an unconnected road; errors here ride the normal lanes.
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err("connection down — reconnect pending".to_owned());
        }
        // Dead or absent: ONE reconnect at a time. Whoever waited here
        // re-reads `active` — the reconnect they waited for is theirs,
        // its failure too.
        let seen = self.attempts.lock().expect("wire attempts").finished;
        let _reconnecting = self.reconnecting.lock().expect("wire reconnecting");
        let previous = {
            let mut held = self.active.lock().expect("wire active");
            if let Some(active) = held.as_ref().filter(|active| live(active)) {
                return Ok(Arc::clone(active));
            }
            // An attempt finished while this ask waited for the lock
            // and the connection is still not live: that attempt
            // failed, and this ask waited for it.
            let attempts = self.attempts.lock().expect("wire attempts");
            if attempts.finished > seen {
                if let Some(error) = attempts.failed.clone() {
                    return Err(error);
                }
            }
            drop(attempts);
            match held.take() {
                Some(active) => {
                    tracing::warn!(target: "ahp_wire", client = %self.tag, "connection DEAD — reconnecting");
                    Some(active)
                }
                None => None,
            }
        };
        let outcome = self.connect_fresh(&previous);
        {
            let mut attempts = self.attempts.lock().expect("wire attempts");
            attempts.finished += 1;
            attempts.failed = outcome.as_ref().err().cloned();
        }
        match outcome {
            Ok(active) => {
                if let Some(previous) = previous {
                    self.retire(previous);
                }
                self.connected.notify_waiters();
                Ok(active)
            }
            Err(error) => {
                if let Some(previous) = previous {
                    *self.active.lock().expect("wire active") = Some(previous);
                }
                Err(error)
            }
        }
    }

    /// A poll with no connection to park on: wait for one to come up
    /// (some ask reconnected) or for the backoff, then answer EMPTY so
    /// the re-arm dials. Parked for good, the channel would stay
    /// frozen after the host came back.
    fn poll_unconnected<T: Send + 'static>(&self) -> ClientFuture<Vec<T>> {
        let connected = Arc::clone(&self.connected);
        let backoff = self
            .runtime
            .spawn(async { tokio::time::sleep(RECONNECT_BACKOFF).await });
        Box::pin(async move {
            let up = connected.notified();
            tokio::select! {
                () = up => {}
                _ = backoff => {}
            }
            Vec::new()
        })
    }

    /// Drop a channel's feed and, on a LIVE connection, its subscription
    /// on the host. A dead connection takes its rows with it when it
    /// closes, and the reconnect carries only the feeds it finds: no
    /// dial is made just to unsubscribe (a host that is down would
    /// hold the caller for the whole connect timeout).
    fn unsubscribe_channel(&self, channel: Uri) -> RunFuture<()> {
        let slot = OneShot::new();
        let live = {
            let held = self.active.lock().expect("wire active");
            match held.as_ref() {
                Some(active) if !active.dead.load(std::sync::atomic::Ordering::Relaxed) => {
                    Some(Arc::clone(active))
                }
                Some(active) => {
                    active.feeds.lock().expect("wire feeds").remove(&channel);
                    None
                }
                None => None,
            }
        };
        let Some(active) = live else {
            slot.fill(Ok(()));
            return RunFuture { slot };
        };
        let filler = slot.clone();
        let tag = self.tag.clone();
        self.runtime.spawn(async move {
            active.feeds.lock().expect("wire feeds").remove(&channel);
            let outcome = active
                .client
                .unsubscribe(channel.clone())
                .await
                .map_err(|error| format!("unsubscribe {channel}: {error}"));
            if let Err(error) = &outcome {
                tracing::warn!(target: "ahp_wire", client = %tag, %error, "unsubscribe failed");
            }
            filler.fill(outcome);
        });
        RunFuture { slot }
    }

    /// Close a connection that was declared dead. The declaration is
    /// the client's (a deaf host, a failed ping) — the socket itself may
    /// well be open, and while it is the host keeps every subscription
    /// on it and broadcasts to BOTH connections: each action arrives
    /// twice and every chat delta is appended twice. The pumps stop
    /// pushing the moment `dead` is set (`pump_channel`); this ends the
    /// transport, so the host drops the old rows. Polls parked on the
    /// old `Active` are released by its death (`poll_channel`) and
    /// re-arm on the new one; the feeds are shared, nothing is lost.
    fn retire(&self, previous: Arc<Active>) {
        let tag = self.tag.clone();
        self.runtime.spawn(async move {
            previous.client.shutdown().await;
            tracing::info!(target: "ahp_wire", client = %tag, "dead connection closed");
        });
    }

    /// TEST SUPPORT: declare the live connection dead, the way a timed
    /// out keepalive does — the next ask reconnects.
    #[doc(hidden)]
    pub fn mark_dead(&self) {
        if let Some(active) = self.active.lock().expect("wire active").as_ref() {
            active.die();
        }
    }

    fn connect_fresh(&self, previous: &Option<Arc<Active>>) -> Result<Arc<Active>, String> {
        let url = match self.discover_url() {
            Ok(url) => url,
            Err(error) => {
                tracing::error!(target: "ahp_wire", client = %self.tag, %error, "discover failed");
                return Err(error);
            }
        };
        tracing::info!(target: "ahp_wire", client = %self.tag, %url, "connecting");

        if tokio::runtime::Handle::try_current().is_ok() {
            tracing::error!(target: "ahp_wire", client = %self.tag, "connect refused: requested from the runtime itself");
            return Err("wire connect requested from the runtime itself".to_owned());
        }

        let tag = self.tag.clone();
        let dead = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let transport_dead = Arc::clone(&dead);

        let connect_deadline = std::time::Duration::from_secs(
            std::env::var("HIHOST_CONNECT_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(15),
        );
        let connector = Arc::clone(&self.connector);
        let dial_url = url.clone();
        let dial_tag = tag.clone();
        let last_seen = Arc::clone(&self.last_seen);
        let client = self.runtime.block_on(async {
            tokio::time::timeout(connect_deadline, async {
                let transport = connector
                    .dial(dial_url, dial_tag.clone(), transport_dead)
                    .await?;
                let client = ahp::Client::connect(transport, ahp::ClientConfig::default())
                    .await
                    .map_err(|error| format!("connect: {error}"))?;
                let initialized = client
                    .initialize(
                        "himark".to_owned(),
                        vec![ahp_types::version::PROTOCOL_VERSION.to_owned()],
                        Vec::new(),
                    )
                    .await
                    .map_err(|error| format!("initialize: {error}"))?;
                // The host's seq is at or past anything this client has
                // seen — unless the host RESTARTED and counts from zero
                // again. A cursor from the old count would tell the new
                // host every gap is covered and lose it; follow the host.
                let behind = last_seen.load(std::sync::atomic::Ordering::Relaxed);
                if initialized.server_seq < behind {
                    tracing::warn!(
                        target: "ahp_wire",
                        client = %dial_tag,
                        host_seq = initialized.server_seq,
                        last_seen = behind,
                        "the host counts from before this client's cursor — a restart; following it"
                    );
                    last_seen.store(initialized.server_seq, std::sync::atomic::Ordering::Relaxed);
                }

                let mut config = ahp_types::common::JsonObject::new();
                config.insert("claudeUseCopilotProxy".to_owned(), serde_json::json!(false));
                config.insert(
                    "allowSignedOutWhenUsable".to_owned(),
                    serde_json::json!(true),
                );
                client
                    .dispatch(
                        ROOT.to_owned(),
                        StateAction::RootConfigChanged(RootConfigChangedAction {
                            config,
                            replace: None,
                        }),
                    )
                    .await
                    .map_err(|error| format!("configChanged: {error}"))?;
                Ok::<_, String>(client)
            })
            .await
        });
        let client = match client {
            Ok(outcome) => outcome?,
            Err(_) => {
                tracing::error!(
                    target: "ahp_wire",
                    client = %self.tag,
                    deadline = ?connect_deadline,
                    "connect TIMED OUT — the host is deaf"
                );
                return Err(format!(
                    "agent host did not answer within {connect_deadline:?}"
                ));
            }
        };

        let (feeds, root, agents) = match &previous {
            Some(previous) => (
                {
                    let feeds = previous.feeds.lock().expect("wire feeds").clone();
                    for feed in feeds.values() {
                        feed.carried();
                    }
                    feeds
                },
                previous.root.lock().expect("wire root").clone(),
                previous.agents.lock().expect("wire agents").clone(),
            ),
            None => (HashMap::new(), None, Vec::new()),
        };
        let active = Arc::new(Active {
            client,
            dead,
            died: tokio::sync::Notify::new(),
            feeds: Mutex::new(feeds),
            root: Mutex::new(root),
            agents: Mutex::new(agents),
        });
        if previous.is_some() {
            match self.runtime.block_on(async {
                tokio::time::timeout(connect_deadline, self.resume(&active)).await
            }) {
                Ok(outcome) => outcome?,
                Err(_) => {
                    tracing::error!(
                        target: "ahp_wire",
                        client = %self.tag,
                        deadline = ?connect_deadline,
                        "resume TIMED OUT"
                    );
                    return Err(format!(
                        "reconnect replay did not answer within {connect_deadline:?}"
                    ));
                }
            }
        }
        self.spawn_keepalive(&active);
        self.watch_death(&active);
        tracing::info!(
            target: "ahp_wire",
            client = %self.tag,
            channels = active.feeds.lock().expect("wire feeds").len(),
            "connection ACTIVE"
        );
        *self.active.lock().expect("wire active") = Some(Arc::clone(&active));
        Ok(active)
    }

    async fn resume(&self, active: &Arc<Active>) -> Result<(), String> {
        let channels: Vec<Uri> = active
            .feeds
            .lock()
            .expect("wire feeds")
            .keys()
            .cloned()
            .collect();
        let had_root = active.root.lock().expect("wire root").is_some();
        let mut subscriptions = channels.clone();
        if had_root {
            subscriptions.push(ROOT.to_owned());
        }
        if subscriptions.is_empty() {
            return Ok(());
        }

        // The local ends are attached BEFORE the reconnect request, so
        // nothing the host broadcasts after re-subscribing is missed —
        // but they are pumped only once the replay has landed: the
        // replay holds every action up to the host's subscribe, the
        // live ends everything after, and the feed must see them in
        // that order. Until then the live ends buffer.
        let mut attached = Vec::with_capacity(channels.len());
        for channel in &channels {
            let sub = active.client.attach_subscription(channel).await;
            let feed = active
                .feeds
                .lock()
                .expect("wire feeds")
                .get(channel)
                .cloned()
                .expect("a carried channel keeps its feed");
            attached.push((sub, feed));
        }
        let root_attached = if had_root {
            let sub = active.client.attach_subscription(ROOT).await;
            let feed = active
                .root
                .lock()
                .expect("wire root")
                .clone()
                .expect("had_root");
            Some((sub, feed))
        } else {
            None
        };
        // The dead connection's pumps push under their feed's lock
        // (`Feed::push_live`), and `dead` was set before this reconnect
        // began: passing every lock here lets a push already past its
        // dead check finish — and bump `last_seen` — before the replay
        // point is read, and refuses every push after. Without the
        // pass, that one straddling action comes back in the replay.
        for channel in &channels {
            if let Some(feed) = active
                .feeds
                .lock()
                .expect("wire feeds")
                .get(channel)
                .cloned()
            {
                feed.settle();
            }
        }
        let last_seen = self.last_seen.load(std::sync::atomic::Ordering::Relaxed);
        tracing::info!(
            target: "ahp_wire",
            client = %self.tag,
            subscriptions = subscriptions.len(),
            last_seen,
            "reconnecting subscriptions"
        );
        let result = active
            .client
            .reconnect(self.client_id.clone(), last_seen, subscriptions)
            .await
            .map_err(|error| format!("reconnect: {error}"))?;
        match result {
            ahp_types::commands::ReconnectResult::Replay(replay) => {
                tracing::info!(
                    target: "ahp_wire",
                    client = %self.tag,
                    actions = replay.actions.len(),
                    missing = replay.missing.len(),
                    "reconnect replay"
                );
                for envelope in replay.actions {
                    self.last_seen.fetch_max(
                        envelope.server_seq as i64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    if envelope.channel == ROOT {
                        if let StateAction::RootAgentsChanged(changed) = envelope.action {
                            if let Some(feed) = active.root.lock().expect("wire root").clone() {
                                feed.push(ServerEvent::AgentsChanged(changed.agents));
                            }
                        }

                        continue;
                    }
                    let feed = active
                        .feeds
                        .lock()
                        .expect("wire feeds")
                        .get(&envelope.channel)
                        .cloned();
                    if let Some(feed) = feed {
                        feed.push_replayed(envelope.server_seq, envelope.action);
                    }
                }
                for channel in replay.missing {
                    tracing::warn!(
                        target: "ahp_wire",
                        client = %self.tag,
                        %channel,
                        "channel is GONE on the host — dropping its feed"
                    );
                    active.feeds.lock().expect("wire feeds").remove(&channel);
                }
            }
            ahp_types::commands::ReconnectResult::Snapshot(snapshot) => {
                tracing::warn!(
                    target: "ahp_wire",
                    client = %self.tag,
                    snapshots = snapshot.snapshots.len(),
                    "reconnect replay TOO OLD — fresh snapshots, the gap is lost"
                );
            }
        }
        for (sub, feed) in attached {
            pump_channel(
                sub,
                feed,
                Arc::clone(&self.last_seen),
                self.tag.clone(),
                Arc::clone(&active.dead),
            );
        }
        if let Some((sub, feed)) = root_attached {
            pump_root(
                sub,
                feed,
                Arc::clone(&self.last_seen),
                self.tag.clone(),
                Arc::clone(&active.dead),
            );
        }
        Ok(())
    }

    fn spawn_keepalive(&self, active: &Arc<Active>) {
        let weak = Arc::downgrade(active);
        let tag = self.tag.clone();
        let every = std::env::var("HIHOST_PING_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(15);
        self.runtime.spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(every)).await;
                let Some(active) = weak.upgrade() else {
                    break;
                };
                if active.dead.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let answered = tokio::time::timeout(
                    std::time::Duration::from_secs(10.min(every)),
                    active.client.ping(),
                )
                .await;
                match answered {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::error!(target: "ahp_wire", client = %tag, %error, "keepalive ping failed — marking dead");
                        active.die();
                        break;
                    }
                    Err(_) => {
                        tracing::error!(target: "ahp_wire", client = %tag, "keepalive ping TIMED OUT — the host is DEAF; marking dead");
                        active.die();
                        break;
                    }
                }
            }
        });
    }

    /// The transport latches `dead` on a failed read or write and
    /// holds nothing else: ONE task per connection turns that flag
    /// into the `died` ring, so every waiter wakes within a tick
    /// instead of each running a timer of its own.
    fn watch_death(&self, active: &Arc<Active>) {
        let weak = Arc::downgrade(active);
        self.runtime.spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                let Some(active) = weak.upgrade() else {
                    break;
                };
                if active.dead.load(std::sync::atomic::Ordering::Relaxed) {
                    active.died.notify_waiters();
                    break;
                }
            }
        });
    }

    /// The extension channels' subscribe (documents, history,
    /// locations): the same road as `subscribe_pumped`, answering the
    /// raw result. The local end is attached BEFORE the request so no
    /// action slips between the snapshot and the pump, the feed is
    /// the channel's existing one if it has one (a second tap, or a
    /// poll already waiting on it), and the pump advances `last_seen`
    /// like every other — a bespoke pump that did not left the cursor
    /// behind, and the reconnect re-requested edits already applied:
    /// character doubling after a reconnect.
    async fn subscribe_ext(
        active: &Arc<Active>,
        channel: Uri,
        last_seen: Arc<std::sync::atomic::AtomicI64>,
        client: String,
    ) -> Result<serde_json::Value, String> {
        let sub = active.client.attach_subscription(&channel).await;
        let feed = Arc::clone(
            active
                .feeds
                .lock()
                .expect("wire feeds")
                .entry(channel.clone())
                .or_default(),
        );
        pump_channel(sub, feed, last_seen, client, Arc::clone(&active.dead));
        let result: serde_json::Value = active
            .client
            .request("subscribe", serde_json::json!({ "channel": channel }))
            .await
            .map_err(|error| format!("subscribe {channel}: {error}"))?;
        if active.dead.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(format!("subscribe {channel}: the connection died under it"));
        }
        Ok(result)
    }

    async fn subscribe_pumped(
        active: &Arc<Active>,
        channel: Uri,
        last_seen: Arc<std::sync::atomic::AtomicI64>,
        client: String,
    ) -> Result<SubscribeResult, String> {
        let (result, sub) = active
            .client
            .subscribe(channel.clone())
            .await
            .map_err(|error| format!("subscribe {channel}: {error}"))?;
        // Answered — by a connection declared dead meanwhile? Its
        // rows live on the host side of a socket about to close, and
        // the reconnect that replaced it carried the feeds it found
        // BEFORE this answer: a feed filed now would be pumped by
        // nobody. Refuse; the caller subscribes again on the live one.
        if active.dead.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(format!("subscribe {channel}: the connection died under it"));
        }

        // The channel's existing feed if it has one — a second tap on
        // the channel, or a poll already waiting on it — else a new
        // one; looked up and filed under the one lock, so two
        // subscribes landing together share a feed instead of the
        // second replacing the first's.
        let feed = Arc::clone(
            active
                .feeds
                .lock()
                .expect("wire feeds")
                .entry(channel)
                .or_default(),
        );
        pump_channel(sub, feed, last_seen, client, Arc::clone(&active.dead));
        Ok(result)
    }
}

/// Pump one channel's events into its feed — for as long as the
/// connection is TRUSTED. A connection declared dead keeps receiving
/// until its socket is closed; nothing it hears after that is pushed:
/// the reconnect replays the gap and the new connection delivers the
/// rest, so pushing here would deliver every action twice.
fn pump_channel(
    mut sub: ahp::SessionSubscription,
    feed: Arc<Feed>,
    last_seen: Arc<std::sync::atomic::AtomicI64>,
    client: String,
    dead: Arc<std::sync::atomic::AtomicBool>,
) {
    let channel = sub.uri().to_owned();
    tokio::spawn(async move {
        while let Some(event) = sub.recv().await {
            if let ahp::SubscriptionEvent::Action(envelope) = event {
                if !feed.push_live(&dead, &last_seen, envelope.server_seq, envelope.action) {
                    break;
                }
            } else if dead.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
        }
        tracing::info!(target: "ahp_wire", client = %client, %channel, "pump ended");
    });
}

fn pump_root(
    mut sub: ahp::SessionSubscription,
    feed: Arc<RootFeed>,
    last_seen: Arc<std::sync::atomic::AtomicI64>,
    client: String,
    dead: Arc<std::sync::atomic::AtomicBool>,
) {
    tokio::spawn(async move {
        while let Some(event) = sub.recv().await {
            if dead.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            match event {
                ahp::SubscriptionEvent::SessionAdded(params) => {
                    feed.push(ServerEvent::SessionAdded(params.summary));
                }
                ahp::SubscriptionEvent::SessionRemoved(params) => {
                    feed.push(ServerEvent::SessionRemoved(crate::client::SessionUri::new(
                        params.session,
                    )));
                }
                ahp::SubscriptionEvent::SessionSummaryChanged(params) => {
                    feed.push(ServerEvent::SessionChanged {
                        session: crate::client::SessionUri::new(params.session),
                        changes: params.changes,
                    });
                }
                ahp::SubscriptionEvent::Action(envelope) => {
                    last_seen.fetch_max(
                        envelope.server_seq as i64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    if let StateAction::RootAgentsChanged(changed) = envelope.action {
                        feed.push(ServerEvent::AgentsChanged(changed.agents));
                    }
                }
                _ => {}
            }
        }
        tracing::info!(target: "ahp_wire", client = %client, channel = ROOT, "pump ended");
    });
}

fn session_state(result: SubscribeResult) -> Result<SessionState, String> {
    match result.snapshot.map(|snapshot| snapshot.state) {
        Some(SnapshotState::Session(state)) => Ok(*state),
        _ => Err("the subscribe answered no session snapshot".to_owned()),
    }
}

fn chat_state(result: SubscribeResult) -> Result<ChatState, String> {
    match result.snapshot.map(|snapshot| snapshot.state) {
        Some(SnapshotState::Chat(state)) => Ok(*state),
        _ => Err("the subscribe answered no chat snapshot".to_owned()),
    }
}

impl SessionClient for WireHost {
    fn connect(&self) -> ClientFuture<Result<RootInfo, String>> {
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_ask(|active| async move {
            {
                let root = active.root.lock().expect("wire root");
                if root.is_some() {
                    return Ok(RootInfo {
                        agents: active.agents.lock().expect("wire agents").clone(),
                    });
                }
            }
            let (result, sub) = active
                .client
                .subscribe(ROOT.to_owned())
                .await
                .map_err(|error| format!("subscribe root: {error}"))?;
            let feed = Arc::new(RootFeed::default());
            *active.root.lock().expect("wire root") = Some(Arc::clone(&feed));
            pump_root(sub, feed, last_seen, tag, Arc::clone(&active.dead));
            let agents = match result.snapshot.map(|snapshot| snapshot.state) {
                Some(SnapshotState::Root(root)) => root.agents,
                _ => return Err("the subscribe answered no root snapshot".to_owned()),
            };
            *active.agents.lock().expect("wire agents") = agents.clone();
            Ok(RootInfo { agents })
        }))
    }

    fn list_sessions(&self, cursor: Option<String>) -> ClientFuture<Result<SessionsPage, String>> {
        Box::pin(self.run_ask(|active| async move {
            let result: ListSessionsResult = active
                .client
                .request(
                    "listSessions",
                    ListSessionsParams {
                        meta: None,
                        channel: ROOT.to_owned(),
                        limit: None,
                        cursor,
                    },
                )
                .await
                .map_err(|error| format!("listSessions: {error}"))?;
            Ok(SessionsPage {
                sessions: result.items,
                next_cursor: result.next_cursor,
            })
        }))
    }

    fn poll_root(&self) -> ClientFuture<Vec<ServerEvent>> {
        let found = self.ensure_active().ok().and_then(|active| {
            let feed = active.root.lock().expect("wire root").clone()?;
            Some((active, feed))
        });
        let Some((active, feed)) = found else {
            tracing::info!(target: "ahp_wire", client = %self.tag, "root poll waiting: not connected");
            return self.poll_unconnected();
        };
        // As `poll_channel`: released empty when the connection dies,
        // so the re-armed poll reconnects; driven by the caller.
        let tag = self.tag.clone();
        Box::pin(async move {
            tokio::select! {
                events = PollRootFeed { feed } => events,
                () = active.died() => {
                    tracing::warn!(target: "ahp_wire", client = %tag, "root poll released: the connection died under it");
                    Vec::new()
                }
            }
        })
    }

    fn create_session(
        &self,
        working_directories: Vec<Uri>,
        options: crate::client::SessionOptions,
    ) -> ClientFuture<Result<crate::client::SessionUri, String>> {
        let vscode = matches!(self.discovery, Discovery::VsCode);
        Box::pin(self.run_ask(move |active| async move {
            let uuid = uuid_v4();
            let requested = format!("ahp-session:/{uuid}");
            let session = if vscode {
                format!("claude:/{uuid}")
            } else {
                requested.clone()
            };
            #[derive(serde::Serialize)]
            #[serde(rename_all = "camelCase")]
            struct CreateSession {
                channel: Uri,
                provider: String,
                #[serde(skip_serializing_if = "Option::is_none")]
                working_directories: Option<Vec<Uri>>,
                config: serde_json::Value,
                model: serde_json::Value,
            }

            let mut config = serde_json::Map::new();
            if !working_directories.is_empty() {
                config.insert("isolation".to_owned(), serde_json::json!("folder"));
            }
            if let Some(overlay) = options.config {
                for (key, value) in overlay {
                    config.insert(key, value);
                }
            }

            let provider = options.provider.unwrap_or_else(|| "claude".to_owned());
            let default_model = match provider.as_str() {
                "codex" => "@provider=openai:default",
                _ => "@provider=anthropic:default",
            };
            let model = match options.model {
                Some(selection) => serde_json::to_value(selection)
                    .unwrap_or_else(|_| serde_json::json!({ "id": default_model })),
                None => serde_json::json!({ "id": default_model }),
            };
            let _: serde_json::Value = active
                .client
                .request(
                    "createSession",
                    CreateSession {
                        channel: requested,
                        provider,
                        working_directories: (!working_directories.is_empty())
                            .then_some(working_directories),
                        config: serde_json::Value::Object(config),
                        model,
                    },
                )
                .await
                .map_err(|error| format!("createSession: {error}"))?;
            Ok(crate::client::SessionUri::new(session))
        }))
    }

    fn resolve_session_config(
        &self,
        working_directory: Option<Uri>,
        config: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> ClientFuture<Result<ahp_types::commands::ResolveSessionConfigResult, String>> {
        Box::pin(self.run_ask(move |active| async move {
            #[derive(serde::Serialize)]
            #[serde(rename_all = "camelCase")]
            struct Resolve {
                channel: Uri,
                provider: String,
                #[serde(skip_serializing_if = "Option::is_none")]
                working_directory: Option<Uri>,
                #[serde(skip_serializing_if = "Option::is_none")]
                config: Option<serde_json::Map<String, serde_json::Value>>,
            }
            let result: ahp_types::commands::ResolveSessionConfigResult = active
                .client
                .request(
                    "resolveSessionConfig",
                    Resolve {
                        channel: format!("ahp-session:/{}", uuid_v4()),
                        provider: "claude".to_owned(),
                        working_directory,
                        config,
                    },
                )
                .await
                .map_err(|error| format!("resolveSessionConfig: {error}"))?;
            Ok(result)
        }))
    }

    fn dispose_session(
        &self,
        session: crate::client::SessionUri,
    ) -> ClientFuture<Result<(), String>> {
        let session = session.into_string();
        Box::pin(self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "disposeSession",
                    DisposeSessionParams {
                        meta: None,
                        channel: session,
                    },
                )
                .await
                .map_err(|error| format!("disposeSession: {error}"))?;
            Ok(())
        }))
    }

    fn subscribe_session(
        &self,
        session: crate::client::SessionUri,
    ) -> ClientFuture<Result<SessionState, String>> {
        let session = session.into_string();
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_subscribe(move |active| {
            let (session, last_seen, tag) = (session.clone(), Arc::clone(&last_seen), tag.clone());
            async move {
                let result = WireHost::subscribe_pumped(&active, session, last_seen, tag).await?;
                session_state(result)
            }
        }))
    }

    fn poll_session(&self, session: crate::client::SessionUri) -> ClientFuture<Vec<StateAction>> {
        let session = session.into_string();
        self.poll_channel(session)
    }

    fn http_serve(&self) -> ClientFuture<Result<String, String>> {
        Box::pin(self.run_ask(move |active| async move {
            let answer: serde_json::Value = active
                .client
                .request("httpServe", serde_json::json!({ "enabled": true }))
                .await
                .map_err(|error| format!("httpServe: {error}"))?;
            answer
                .get("url")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| "httpServe answered no url".to_owned())
        }))
    }

    fn dispatch_action(
        &self,
        channel: crate::client::ChannelUri,
        action: StateAction,
    ) -> ClientFuture<Result<(), String>> {
        let channel = channel.into_string();
        Box::pin(self.run_ask(move |active| async move {
            let _ = active
                .client
                .dispatch(channel, action)
                .await
                .map_err(|error| format!("dispatch: {error}"))?;
            Ok(())
        }))
    }
}

impl ChatClient for WireHost {
    fn create_chat(
        &self,
        session: crate::client::SessionUri,
    ) -> ClientFuture<Result<crate::client::ChatUri, String>> {
        let session = session.into_string();
        Box::pin(self.run_ask(move |active| async move {
            let chat = format!("ahp-chat:/{}", uuid_v4());
            let _: serde_json::Value = active
                .client
                .request(
                    "createChat",
                    CreateChatParams {
                        meta: None,
                        channel: session,
                        chat: chat.clone(),
                        initial_message: None,
                        source: None,
                        working_directories: None,
                    },
                )
                .await
                .map_err(|error| format!("createChat: {error}"))?;
            Ok(crate::client::ChatUri::new(chat))
        }))
    }

    fn subscribe_chat(
        &self,
        chat: crate::client::ChatUri,
    ) -> ClientFuture<Result<ChatState, String>> {
        let chat = chat.into_string();
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_subscribe(move |active| {
            let (chat, last_seen, tag) = (chat.clone(), Arc::clone(&last_seen), tag.clone());
            async move {
                let result = WireHost::subscribe_pumped(&active, chat, last_seen, tag).await?;
                chat_state(result)
            }
        }))
    }

    fn fetch_turns(
        &self,
        chat: crate::client::ChatUri,
        cursor: Option<String>,
    ) -> ClientFuture<Result<TurnsPage, String>> {
        let chat = chat.into_string();
        let feed = self.ensure_active().and_then(|active| {
            active
                .feeds
                .lock()
                .expect("wire feeds")
                .get(&chat)
                .cloned()
                .ok_or_else(|| format!("not subscribed: {chat}"))
        });
        Box::pin(self.run_ask(move |active| async move {
            let feed = feed?;
            let capture = feed.arm_turns_capture();
            let result: Result<serde_json::Value, _> = active
                .client
                .request(
                    "fetchTurns",
                    FetchTurnsParams {
                        meta: None,
                        channel: chat,
                        cursor,
                    },
                )
                .await;
            if let Err(error) = result {
                feed.disarm_turns_capture();
                return Err(format!("fetchTurns: {error}"));
            }

            Ok(capture.await)
        }))
    }

    fn start_turn(
        &self,
        chat: crate::client::ChatUri,
        text: String,
        attachments: Option<Vec<ahp_types::state::MessageAttachment>>,
        model: Option<ModelSelection>,
    ) -> ClientFuture<Result<(), String>> {
        let chat = chat.into_string();
        let turn_id = format!("himark-{}", self.next_turn.fetch_add(1, Ordering::Relaxed));
        Box::pin(self.run_ask(move |active| async move {
            active
                .client
                .dispatch(
                    chat,
                    StateAction::ChatTurnStarted(ahp_types::actions::ChatTurnStartedAction {
                        turn_id,
                        started_at: humantime::format_rfc3339_millis(std::time::SystemTime::now())
                            .to_string(),
                        message: Message {
                            text,
                            origin: MessageOrigin {
                                kind: MessageKind::User,
                            },
                            attachments,

                            model: Some(model.unwrap_or_else(|| ModelSelection {
                                id: "@provider=anthropic:default".to_owned(),
                                config: None,
                            })),
                            agent: None,
                            meta: None,
                        },
                        queued_message_id: None,
                        meta: None,
                    }),
                )
                .await
                .map_err(|error| format!("dispatch turnStarted: {error}"))?;
            Ok(())
        }))
    }

    fn poll_chat(&self, chat: crate::client::ChatUri) -> ClientFuture<Vec<StateAction>> {
        let chat = chat.into_string();
        self.poll_channel(chat)
    }

    fn cancel_turn(
        &self,
        chat: crate::client::ChatUri,
        turn_id: crate::client::TurnId,
    ) -> ClientFuture<()> {
        let chat = chat.into_string();
        let turn_id = turn_id.into_string();
        let dispatched = self.run_ask(move |active| async move {
            active
                .client
                .dispatch(
                    chat,
                    StateAction::ChatTurnCancelled(ahp_types::actions::ChatTurnCancelledAction {
                        turn_id,
                        duration: 0,
                        meta: None,
                    }),
                )
                .await
                .map_err(|error| format!("dispatch turnCancelled: {error}"))
        });
        Box::pin(async move {
            if let Err(error) = dispatched.await {
                eprintln!("[hiahp] cancel: {error}");
            }
        })
    }

    fn read_file_edit(
        &self,
        before: Option<Uri>,
        after: Option<Uri>,
    ) -> ClientFuture<Result<FileEditContents, String>> {
        Box::pin(self.run_ask(move |active| async move {
            let read = |uri: Option<Uri>| {
                let client = active.client.clone();
                async move {
                    let Some(uri) = uri else {
                        return Ok::<Option<String>, String>(None);
                    };
                    let result = client
                        .resource_read(ResourceReadParams {
                            meta: None,
                            channel: String::new(),
                            uri: uri.as_str().to_owned(),
                            encoding: None,
                        })
                        .await
                        .map_err(|error| format!("resourceRead {uri}: {error}"))?;
                    match result.encoding {
                        ahp_types::commands::ContentEncoding::Utf8 => Ok(Some(result.data)),
                        ahp_types::commands::ContentEncoding::Base64 => {
                            Err(format!("binary content not supported yet: {uri}"))
                        }
                    }
                }
            };
            Ok(FileEditContents {
                before: read(before).await?,
                after: read(after).await?,
            })
        }))
    }
}

impl ChangesClient for WireHost {
    fn subscribe_changeset(
        &self,
        channel: crate::client::ChannelUri,
    ) -> ClientFuture<Result<ahp_types::state::ChangesetState, String>> {
        let channel = channel.into_string();
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_subscribe(move |active| {
            let (channel, last_seen, tag) = (channel.clone(), Arc::clone(&last_seen), tag.clone());
            async move {
                let result = WireHost::subscribe_pumped(&active, channel, last_seen, tag).await?;
                match result.snapshot.map(|snapshot| snapshot.state) {
                    Some(SnapshotState::Changeset(state)) => Ok(*state),
                    _ => Err("the subscribe answered no changeset snapshot".to_owned()),
                }
            }
        }))
    }

    fn poll_changeset(&self, channel: crate::client::ChannelUri) -> ClientFuture<Vec<StateAction>> {
        let channel = channel.into_string();
        self.poll_channel(channel)
    }

    fn unsubscribe_changeset(&self, channel: &crate::client::ChannelUri) {
        let _ = self.unsubscribe_channel(channel.as_str().to_owned());
    }
}

impl HistoryClient for WireHost {
    fn subscribe_history(
        &self,
        channel: crate::client::ChannelUri,
    ) -> ClientFuture<Result<himark_ahp_ext_types::history::HistoryState, String>> {
        let channel = channel.into_string();
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_subscribe(move |active| {
            let (channel, last_seen, tag) = (channel.clone(), Arc::clone(&last_seen), tag.clone());
            async move {
                let result = WireHost::subscribe_ext(&active, channel, last_seen, tag).await?;
                serde_json::from_value(result["snapshot"]["state"].clone())
                    .map_err(|error| format!("history snapshot: {error}"))
            }
        }))
    }
}

impl AnnotationsClient for WireHost {
    fn subscribe_annotations(
        &self,
        session: crate::client::SessionUri,
    ) -> ClientFuture<Result<ahp_types::state::AnnotationsState, String>> {
        let channel = annotations_channel(&session.into_string());
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_subscribe(move |active| {
            let (channel, last_seen, tag) = (channel.clone(), Arc::clone(&last_seen), tag.clone());
            async move {
                let result = WireHost::subscribe_pumped(&active, channel, last_seen, tag).await?;
                match result.snapshot.map(|snapshot| snapshot.state) {
                    Some(SnapshotState::Annotations(state)) => Ok(*state),
                    _ => Err("the subscribe answered no annotations snapshot".to_owned()),
                }
            }
        }))
    }

    fn poll_annotations(&self, session: crate::client::SessionUri) -> ClientFuture<Vec<StateAction>> {
        let session = session.into_string();
        self.poll_channel(annotations_channel(&session))
    }

    fn dispatch_annotations(&self, session: &crate::client::SessionUri, action: StateAction) {
        let channel = annotations_channel(session.as_str());
        let _ = self.run_ask(move |active| async move {
            active
                .client
                .dispatch(channel, action)
                .await
                .map(|_| ())
                .map_err(|error| format!("dispatch annotations: {error}"))
        });
    }

    fn unsubscribe_annotations(&self, session: &crate::client::SessionUri) {
        let _ = self.unsubscribe_channel(annotations_channel(session.as_str()));
    }
}

impl DocumentsClient for WireHost {
    fn open_document(
        &self,
        session: crate::client::SessionUri,
        uri: Option<crate::client::ResourceUri>,
        text: Option<String>,
    ) -> ClientFuture<Result<himark_ahp_ext_types::documents::OpenDocumentResult, String>> {
        let session = session.into_string();
        let uri = uri.map(crate::client::ResourceUri::into_string);
        Box::pin(self.run_ask(move |active| async move {
            active
                .client
                .request(
                    "openDocument",
                    himark_ahp_ext_types::documents::OpenDocumentParams {
                        channel: session,
                        uri,
                        text,
                    },
                )
                .await
                .map_err(|error| format!("openDocument: {error}"))
        }))
    }

    fn subscribe_document(
        &self,
        channel: crate::client::ChannelUri,
    ) -> ClientFuture<Result<himark_ahp_ext_types::documents::DocumentState, String>> {
        let channel = channel.into_string();
        // The pump MUST advance `last_seen` (via `pump_channel`, like
        // every other channel) — a bespoke pump that ignored it left
        // the client's cursor behind the doc channel, so a reconnect
        // re-requested already-seen edits and the rebase log applied
        // them a second time as foreign ops: character doubling after
        // a reconnect.
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_subscribe(move |active| {
            let (channel, last_seen, tag) = (channel.clone(), Arc::clone(&last_seen), tag.clone());
            async move {
                let result = WireHost::subscribe_ext(&active, channel, last_seen, tag).await?;
                serde_json::from_value(result["snapshot"]["state"].clone())
                    .map_err(|error| format!("document snapshot: {error}"))
            }
        }))
    }

    fn poll_document(
        &self,
        channel: crate::client::ChannelUri,
    ) -> ClientFuture<Vec<himark_ahp_ext_types::documents::DocumentApplied>> {
        let channel = channel.into_string();
        let poll = self.poll_channel(channel);
        Box::pin(async move {
            poll.await
                .into_iter()
                .filter_map(|action| match action {
                    StateAction::Unknown(value)
                        if value["type"] == himark_ahp_ext_types::documents::DOCUMENT_APPLIED =>
                    {
                        serde_json::from_value(value).ok()
                    }
                    _ => None,
                })
                .collect()
        })
    }

    fn dispatch_document(
        &self,
        channel: &crate::client::ChannelUri,
        action: himark_ahp_ext_types::documents::DocumentApplied,
    ) {
        let channel = channel.as_str().to_owned();
        let _ = self.run_ask(move |active| async move {
            let mut value = serde_json::to_value(&action).expect("an action serializes");
            value["type"] =
                serde_json::Value::String(himark_ahp_ext_types::documents::DOCUMENT_APPLIED.to_owned());
            active
                .client
                .dispatch(channel, StateAction::Unknown(value))
                .await
                .map(|_| ())
                .map_err(|error| format!("dispatch: {error}"))
        });
    }

    fn store_document(
        &self,
        channel: crate::client::ChannelUri,
        uri: crate::client::ResourceUri,
    ) -> ClientFuture<Result<(), String>> {
        let channel = channel.into_string();
        let uri = uri.into_string();
        Box::pin(self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "storeDocument",
                    himark_ahp_ext_types::documents::StoreDocumentParams { channel, uri },
                )
                .await
                .map_err(|error| format!("storeDocument: {error}"))?;
            Ok(())
        }))
    }

    fn unsubscribe_document(&self, channel: &crate::client::ChannelUri) -> ClientFuture<()> {
        let ask = self.unsubscribe_channel(channel.as_str().to_owned());
        Box::pin(async move {
            let _ = ask.await;
        })
    }
}

impl LspClient for WireHost {
    fn lsp(
        &self,
        session: crate::client::SessionUri,
        method: String,
        params: serde_json::Value,
    ) -> ClientFuture<Result<serde_json::Value, String>> {
        let session = session.into_string();
        Box::pin(self.run_ask(move |active| async move {
            active
                .client
                .request(
                    &format!("lsp/{method}"),
                    serde_json::json!({ "channel": session, "params": params }),
                )
                .await
                .map_err(|error| format!("lsp/{method}: {error}"))
        }))
    }
}

impl ResourceClient for WireHost {
    fn resource_read(
        &self,
        session: crate::client::SessionUri,
        uri: crate::client::ResourceUri,
    ) -> ClientFuture<Option<String>> {
        let session = session.into_string();
        let asked = self.run_ask(move |active| async move {
            let result = active
                .client
                .resource_read(ResourceReadParams {
                    meta: None,
                    channel: session,
                    uri: uri.as_str().to_owned(),
                    encoding: None,
                })
                .await
                .map_err(|error| format!("resourceRead {uri}: {error}"))?;
            match result.encoding {
                ContentEncoding::Utf8 => Ok(Some(result.data)),
                ContentEncoding::Base64 => Err(format!("binary content: {uri}")),
            }
        });
        Box::pin(async move {
            asked.await.unwrap_or_else(|error| {
                eprintln!("[hiahp] {error}");
                None
            })
        })
    }

    fn resource_read_bytes(
        &self,
        session: crate::client::SessionUri,
        uri: crate::client::ResourceUri,
    ) -> ClientFuture<Option<Vec<u8>>> {
        let session = session.into_string();
        let asked = self.run_ask(move |active| async move {
            let result = active
                .client
                .resource_read(ResourceReadParams {
                    meta: None,
                    channel: session,
                    uri: uri.as_str().to_owned(),
                    encoding: Some(ContentEncoding::Base64),
                })
                .await
                .map_err(|error| format!("resourceRead {uri}: {error}"))?;
            match result.encoding {
                ContentEncoding::Base64 => {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD
                        .decode(result.data.as_bytes())
                        .map(Some)
                        .map_err(|error| format!("resourceRead {uri}: bad base64: {error}"))
                }
                ContentEncoding::Utf8 => Ok(Some(result.data.into_bytes())),
            }
        });
        Box::pin(async move {
            asked.await.unwrap_or_else(|error| {
                eprintln!("[hiahp] {error}");
                None
            })
        })
    }

    fn resource_write(
        &self,
        session: crate::client::SessionUri,
        uri: crate::client::ResourceUri,
        text: String,
    ) -> ClientFuture<bool> {
        let session = session.into_string();
        let asked = self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "resourceWrite",
                    ResourceWriteParams {
                        meta: None,
                        channel: session,
                        uri: uri.as_str().to_owned(),
                        data: text,
                        encoding: ContentEncoding::Utf8,
                        content_type: None,
                        create_only: None,
                        mode: None,
                        position: None,
                        if_match: None,
                    },
                )
                .await
                .map_err(|error| format!("resourceWrite {uri}: {error}"))?;
            Ok(true)
        });
        Box::pin(async move {
            asked.await.unwrap_or_else(|error| {
                eprintln!("[hiahp] {error}");
                false
            })
        })
    }

    fn resource_create(
        &self,
        session: crate::client::SessionUri,
        uri: crate::client::ResourceUri,
    ) -> ClientFuture<bool> {
        let session = session.into_string();
        let asked = self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "resourceWrite",
                    ResourceWriteParams {
                        meta: None,
                        channel: session,
                        uri: uri.as_str().to_owned(),
                        data: String::new(),
                        encoding: ContentEncoding::Utf8,
                        content_type: None,
                        create_only: Some(true),
                        mode: None,
                        position: None,
                        if_match: None,
                    },
                )
                .await
                .map_err(|error| format!("resourceWrite {uri}: {error}"))?;
            Ok(true)
        });
        Box::pin(async move {
            asked.await.unwrap_or_else(|error| {
                eprintln!("[hiahp] {error}");
                false
            })
        })
    }

    fn resource_delete(
        &self,
        session: crate::client::SessionUri,
        uri: crate::client::ResourceUri,
        recursive: bool,
    ) -> ClientFuture<bool> {
        let session = session.into_string();
        let asked = self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "resourceDelete",
                    ResourceDeleteParams {
                        meta: None,
                        channel: session,
                        uri: uri.as_str().to_owned(),
                        recursive: Some(recursive),
                    },
                )
                .await
                .map_err(|error| format!("resourceDelete {uri}: {error}"))?;
            Ok(true)
        });
        Box::pin(async move {
            asked.await.unwrap_or_else(|error| {
                eprintln!("[hiahp] {error}");
                false
            })
        })
    }

    fn resource_move(
        &self,
        session: crate::client::SessionUri,
        from: crate::client::ResourceUri,
        to: crate::client::ResourceUri,
    ) -> ClientFuture<bool> {
        let session = session.into_string();
        let asked = self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "resourceMove",
                    ResourceMoveParams {
                        meta: None,
                        channel: session,
                        source: from.as_str().to_owned(),
                        destination: to.as_str().to_owned(),
                        fail_if_exists: Some(true),
                    },
                )
                .await
                .map_err(|error| format!("resourceMove {from}: {error}"))?;
            Ok(true)
        });
        Box::pin(async move {
            asked.await.unwrap_or_else(|error| {
                eprintln!("[hiahp] {error}");
                false
            })
        })
    }

    fn resource_list(
        &self,
        session: crate::client::SessionUri,
        uri: crate::client::ResourceUri,
    ) -> ClientFuture<Option<Vec<(String, bool)>>> {
        let session = session.into_string();
        let asked = self.run_ask(move |active| async move {
            let result: ResourceListResult = active
                .client
                .request(
                    "resourceList",
                    ResourceListParams {
                        meta: None,
                        channel: session,
                        uri: uri.as_str().to_owned(),
                    },
                )
                .await
                .map_err(|error| format!("resourceList {uri}: {error}"))?;
            Ok(Some(
                result
                    .entries
                    .into_iter()
                    .map(|entry| {
                        let directory = entry.r#type == "directory";
                        (entry.name, directory)
                    })
                    .collect(),
            ))
        });
        Box::pin(async move {
            asked.await.unwrap_or_else(|error| {
                eprintln!("[hiahp] {error}");
                None
            })
        })
    }

    fn resource_watch(
        &self,
        session: crate::client::SessionUri,
        uri: crate::client::ResourceUri,
        events: Arc<dyn Fn() + Send + Sync>,
    ) -> ClientFuture<Option<crate::client::WatchHandle>> {
        let session = session.into_string();
        let asked = self.run_ask(move |active| async move {
            let result: CreateResourceWatchResult = active
                .client
                .request(
                    "createResourceWatch",
                    CreateResourceWatchParams {
                        meta: None,
                        channel: session,
                        uri: uri.as_str().to_owned(),
                        recursive: None,
                        excludes: None,
                        includes: None,
                    },
                )
                .await
                .map_err(|error| format!("createResourceWatch {uri}: {error}"))?;
            let channel = result.channel;
            let (_, mut sub) = active
                .client
                .subscribe(channel.clone())
                .await
                .map_err(|error| format!("subscribe {channel}: {error}"))?;
            tokio::spawn(async move {
                while let Some(event) = sub.recv().await {
                    if let ahp::SubscriptionEvent::Action(envelope) = event {
                        if matches!(envelope.action, StateAction::ResourceWatchChanged(_)) {
                            events();
                        }
                    }
                }
            });
            Ok(Some(crate::client::WatchHandle {
                channel: crate::client::ChannelUri::new(channel),
            }))
        });
        Box::pin(async move {
            asked.await.unwrap_or_else(|error| {
                eprintln!("[hiahp] {error}");
                None
            })
        })
    }

    fn resource_unwatch(&self, handle: crate::client::WatchHandle) -> ClientFuture<()> {
        let asked = self.run_ask(move |active| async move {
            active
                .client
                .unsubscribe(handle.channel.clone().into_string())
                .await
                .map_err(|error| format!("unsubscribe {}: {error}", handle.channel))
        });
        Box::pin(async move {
            if let Err(error) = asked.await {
                eprintln!("[hiahp] {error}");
            }
        })
    }

    fn search(
        &self,
        session: crate::client::SessionUri,
        ask: crate::client::SearchAsk,
    ) -> ClientFuture<Option<himark_ahp_ext_types::search::SearchResult>> {
        let session = session.into_string();
        let asked = self.run_ask(move |active| async move {
            let params = himark_ahp_ext_types::search::SearchParams {
                channel: session,
                folders: Some(ask.folders),
                query: ask.query,
                kind: ask.kind,
                case_sensitive: ask.case_sensitive,
                target: ask.target,
                limit: Some(ask.limit as u64),
            };
            let result: himark_ahp_ext_types::search::SearchResult = active
                .client
                .request("search", params)
                .await
                .map_err(|error| format!("search: {error}"))?;
            Ok(result)
        });
        Box::pin(async move { asked.await.ok() })
    }
}

impl LocationsClient for WireHost {
    fn search_locations(
        &self,
        session: crate::client::SessionUri,
        ask: crate::client::LocationsAsk,
    ) -> ClientFuture<Result<crate::client::ChannelUri, String>> {
        let session = session.into_string();
        Box::pin(self.run_ask(move |active| async move {
            let params = himark_ahp_ext_types::locations::SearchLocationsParams {
                channel: session,
                folders: Some(ask.folders),
                query: ask.query,
                kind: ask.kind,
                case_sensitive: ask.case_sensitive,
                limit: Some(ask.limit as u64),
            };
            let result: himark_ahp_ext_types::locations::LocationsChannelResult = active
                .client
                .request("searchLocations", params)
                .await
                .map_err(|error| format!("searchLocations: {error}"))?;
            Ok(crate::client::ChannelUri::new(result.channel))
        }))
    }

    fn lsp_locations(
        &self,
        session: crate::client::SessionUri,
        method: String,
        params: serde_json::Value,
    ) -> ClientFuture<Result<crate::client::ChannelUri, String>> {
        let session = session.into_string();
        Box::pin(self.run_ask(move |active| async move {
            let params = himark_ahp_ext_types::locations::LspLocationsParams {
                channel: session,
                method,
                params,
            };
            let result: himark_ahp_ext_types::locations::LocationsChannelResult = active
                .client
                .request("lsp/locations", params)
                .await
                .map_err(|error| format!("lsp/locations: {error}"))?;
            Ok(crate::client::ChannelUri::new(result.channel))
        }))
    }

    fn subscribe_locations(
        &self,
        channel: crate::client::ChannelUri,
    ) -> ClientFuture<Result<himark_ahp_ext_types::locations::LocationList, String>> {
        let channel = channel.into_string();
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_subscribe(move |active| {
            let (channel, last_seen, tag) = (channel.clone(), Arc::clone(&last_seen), tag.clone());
            async move {
                let result = WireHost::subscribe_ext(&active, channel, last_seen, tag).await?;
                serde_json::from_value(result["snapshot"]["state"].clone())
                    .map_err(|error| format!("locations snapshot: {error}"))
            }
        }))
    }

    fn poll_locations(
        &self,
        channel: crate::client::ChannelUri,
    ) -> ClientFuture<Vec<himark_ahp_ext_types::locations::LocationList>> {
        let channel = channel.into_string();
        let polled = self.poll_channel(channel);
        Box::pin(async move {
            polled
                .await
                .into_iter()
                .filter_map(|action| match action {
                    StateAction::Unknown(value)
                        if value["type"] == himark_ahp_ext_types::locations::LOCATIONS_EXTEND =>
                    {
                        serde_json::from_value(value).ok()
                    }
                    _ => None,
                })
                .collect()
        })
    }

    fn unsubscribe_locations(&self, channel: &crate::client::ChannelUri) {
        let _ = self.unsubscribe_channel(channel.as_str().to_owned());
    }
}

impl TerminalClient for WireHost {
    fn terminal_open(
        &self,
        _session: crate::client::SessionUri,
        channel: crate::client::ChannelUri,
        cwd: Option<Uri>,
        cols: u16,
        rows: u16,
        events: Arc<dyn Fn(crate::client::TerminalEvent) + Send + Sync>,
    ) -> ClientFuture<Option<crate::client::TerminalHandle>> {
        let channel = channel.into_string();
        let asked = self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "createTerminal",
                    ahp_types::commands::CreateTerminalParams {
                        meta: None,
                        channel: channel.clone(),
                        claim: ahp_types::state::TerminalClaim::Client(
                            ahp_types::state::TerminalClientClaim {
                                client_id: "himark".to_owned(),
                            },
                        ),
                        name: None,
                        cwd,
                        cols: Some(cols as i64),
                        rows: Some(rows as i64),
                    },
                )
                .await
                .map_err(|error| format!("createTerminal: {error}"))?;
            let (result, mut sub) = active
                .client
                .subscribe(channel.clone())
                .await
                .map_err(|error| format!("subscribe {channel}: {error}"))?;

            if let Some(SnapshotState::Terminal(state)) = result.snapshot.map(|s| s.state) {
                for part in &state.content {
                    match part {
                        ahp_types::state::TerminalContentPart::Unclassified(part) => {
                            events(crate::client::TerminalEvent::Data(part.value.clone()))
                        }
                        ahp_types::state::TerminalContentPart::Command(part) => {
                            events(crate::client::TerminalEvent::Data(part.output.clone()))
                        }
                        _ => {}
                    }
                }
                if let ahp_types::state::TerminalLifecycleState::Exited(exited) = &state.lifecycle {
                    events(crate::client::TerminalEvent::Exited(
                        exited.exit_code.map(|code| code as i32),
                    ));
                }
            }
            tokio::spawn(async move {
                while let Some(event) = sub.recv().await {
                    if let ahp::SubscriptionEvent::Action(envelope) = event {
                        match envelope.action {
                            StateAction::TerminalData(data) => {
                                events(crate::client::TerminalEvent::Data(data.data))
                            }
                            StateAction::TerminalExited(exited) => {
                                events(crate::client::TerminalEvent::Exited(
                                    exited.exit_code.map(|code| code as i32),
                                ))
                            }
                            _ => {}
                        }
                    }
                }
            });
            Ok(Some(crate::client::TerminalHandle {
                channel: crate::client::ChannelUri::new(channel),
            }))
        });
        Box::pin(async move {
            asked.await.unwrap_or_else(|error| {
                eprintln!("[hiahp] {error}");
                None
            })
        })
    }

    fn terminal_input(&self, channel: &crate::client::ChannelUri, data: String) {
        let channel = channel.as_str().to_owned();
        self.run_live(move |active| async move {
            active
                .client
                .dispatch(
                    channel,
                    StateAction::TerminalInput(ahp_types::actions::TerminalInputAction { data }),
                )
                .await
                .map_err(|error| format!("terminal/input: {error}"))
        });
    }

    fn terminal_resize(&self, channel: &crate::client::ChannelUri, cols: u16, rows: u16) {
        let channel = channel.as_str().to_owned();
        self.run_live(move |active| async move {
            active
                .client
                .dispatch(
                    channel,
                    StateAction::TerminalResized(ahp_types::actions::TerminalResizedAction {
                        cols: cols as i64,
                        rows: rows as i64,
                    }),
                )
                .await
                .map_err(|error| format!("terminal/resized: {error}"))
        });
    }

    fn terminal_dispose(&self, channel: &crate::client::ChannelUri) {
        let channel = channel.as_str().to_owned();
        self.run_live(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "disposeTerminal",
                    ahp_types::commands::DisposeTerminalParams {
                        meta: None,
                        channel: channel.clone(),
                    },
                )
                .await
                .map_err(|error| format!("disposeTerminal: {error}"))?;
            active
                .client
                .unsubscribe(channel.clone())
                .await
                .map_err(|error| format!("unsubscribe {channel}: {error}"))?;
            Ok(())
        });
    }
}

impl WireHost {
    /// A poll parks on the channel's feed — and on the connection it
    /// found live. When THAT connection is declared dead the poll
    /// answers an EMPTY batch: the pumps feeding it have stopped, and
    /// the caller's re-armed poll is the ask that reconnects. A poll
    /// that stayed parked would freeze the channel until some other
    /// ask happened by (the chat froze mid-stream after a keepalive
    /// timeout, 2026-10-02).
    ///
    /// The future is the CALLER's to drive, never a task of the
    /// runtime's: callers cancel a poll and arm another (every
    /// relaunch does), and a task parked on the feed would go on to
    /// take the next batch for nobody.
    fn poll_channel(&self, channel: Uri) -> ClientFuture<Vec<StateAction>> {
        let Ok(active) = self.ensure_active() else {
            return self.poll_unconnected();
        };
        let tag = self.tag.clone();
        let runtime = self.runtime.clone();
        Box::pin(async move {
            let died = Arc::clone(&active).died();
            let batch = async {
                let mut waited = false;
                loop {
                    let feed = active
                        .feeds
                        .lock()
                        .expect("wire feeds")
                        .get(&channel)
                        .cloned();
                    match feed {
                        Some(feed) => return PollFeed { feed }.await,
                        None => {
                            // The SUBSCRIBE may still be in flight — the
                            // poll loop re-arms only when this future
                            // resolves, so parking forever here would
                            // orphan the channel mirror for good. WAIT
                            // for the feed instead. (The timer lives on
                            // the runtime; this future is driven off it.)
                            if !waited {
                                waited = true;
                                eprintln!("[hiahp] poll waiting: not subscribed yet: {channel}");
                            }
                            let _ = runtime
                                .spawn(async {
                                    tokio::time::sleep(std::time::Duration::from_millis(50)).await
                                })
                                .await;
                        }
                    }
                }
            };
            tokio::select! {
                batch = batch => batch,
                () = died => {
                    tracing::warn!(target: "ahp_wire", client = %tag, "poll released: the connection died under it");
                    Vec::new()
                }
            }
        })
    }
}

#[derive(Default)]
pub struct Feed {
    state: Mutex<FeedState>,
}

#[derive(Default)]
struct FeedState {
    actions: VecDeque<StateAction>,

    /// The highest server seq landed on this connection. The host
    /// keeps ONE ROW PER TAP on a channel and serves each row (a
    /// client may tap a channel twice, for different reasons and
    /// lifetimes — that is the host's contract, not a bug), so every
    /// action reaches the client once per row, and each copy reaches
    /// every pump of the channel. They fold here: a seq at or below
    /// the mark has landed. Reset with every connection
    /// (`Feed::carried`): the seq space is the host's, and a host
    /// that restarted starts over.
    landed: u64,

    /// EVERY pending waiter, not a single slot. Refetch roads arm a
    /// second poll on a channel that already has one standing (the
    /// changes refresh chip does), and a one-slot waker means the
    /// overwritten waiter is never polled again — a parked chain
    /// once the registered one has drained and gone.
    wakers: Vec<Waker>,

    turns_capture: Option<OneShot<TurnsPage>>,
}

impl Feed {
    /// A live pump's push: the connection's `dead` is read UNDER the
    /// feed lock, together with the `last_seen` bump, so a reconnect
    /// that `settle`s the feed sees either the whole push or none of
    /// it. Returns false when the connection is dead — the pump ends.
    /// Test seam: a bare push, as the pumps land batches.
    #[cfg(test)]
    fn push(&self, action: StateAction) {
        let mut state = self.state.lock().expect("feed state");
        Self::land(&mut state, action);
    }

    fn push_live(
        &self,
        dead: &std::sync::atomic::AtomicBool,
        last_seen: &std::sync::atomic::AtomicI64,
        server_seq: u64,
        action: StateAction,
    ) -> bool {
        let mut state = self.state.lock().expect("feed state");
        if dead.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        last_seen.fetch_max(server_seq as i64, std::sync::atomic::Ordering::Relaxed);
        if server_seq <= state.landed {
            return true;
        }
        state.landed = server_seq;
        Self::land(&mut state, action);
        true
    }

    /// A replayed action: landed unless a copy of it already has.
    fn push_replayed(&self, server_seq: u64, action: StateAction) {
        let mut state = self.state.lock().expect("feed state");
        if server_seq <= state.landed {
            return;
        }
        state.landed = server_seq;
        Self::land(&mut state, action);
    }

    /// Carried onto a new connection: the seq fold starts over.
    fn carried(&self) {
        self.state.lock().expect("feed state").landed = 0;
    }

    /// Wait out a push in flight on this feed (see `push_live`).
    fn settle(&self) {
        drop(self.state.lock().expect("feed state"));
    }


    fn land(state: &mut FeedState, action: StateAction) {
        if let StateAction::ChatTurnsLoaded(loaded) = &action {
            if let Some(capture) = state.turns_capture.take() {
                capture.fill(TurnsPage {
                    turns: loaded.turns.clone(),
                    next_cursor: loaded.turns_next_cursor.clone(),
                });
                return;
            }
        }
        state.actions.push_back(action);
        for waker in state.wakers.drain(..) {
            waker.wake();
        }
    }

    fn arm_turns_capture(&self) -> OneShot<TurnsPage> {
        let capture = OneShot::new();
        self.state.lock().expect("feed state").turns_capture = Some(capture.clone());
        capture
    }

    fn disarm_turns_capture(&self) {
        self.state.lock().expect("feed state").turns_capture = None;
    }
}

pub struct PollFeed {
    feed: Arc<Feed>,
}

impl Future for PollFeed {
    type Output = Vec<StateAction>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Vec<StateAction>> {
        let mut state = self.feed.state.lock().expect("feed state");
        if state.actions.is_empty() {
            let waker = cx.waker();
            if !state.wakers.iter().any(|held| held.will_wake(waker)) {
                state.wakers.push(waker.clone());
            }
            return Poll::Pending;
        }
        Poll::Ready(state.actions.drain(..).collect())
    }
}

#[derive(Default)]
struct RootFeed {
    state: Mutex<RootFeedState>,
}

#[derive(Default)]
struct RootFeedState {
    events: VecDeque<ServerEvent>,
    wakers: Vec<Waker>,
}

impl RootFeed {
    fn push(&self, event: ServerEvent) {
        let mut state = self.state.lock().expect("root feed");
        state.events.push_back(event);
        for waker in state.wakers.drain(..) {
            waker.wake();
        }
    }
}

struct PollRootFeed {
    feed: Arc<RootFeed>,
}

impl Future for PollRootFeed {
    type Output = Vec<ServerEvent>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Vec<ServerEvent>> {
        let mut state = self.feed.state.lock().expect("root feed");
        if state.events.is_empty() {
            let waker = cx.waker();
            if !state.wakers.iter().any(|held| held.will_wake(waker)) {
                state.wakers.push(waker.clone());
            }
            return Poll::Pending;
        }
        Poll::Ready(state.events.drain(..).collect())
    }
}

pub(crate) struct OneShot<T> {
    state: Arc<Mutex<OneShotState<T>>>,
}

struct OneShotState<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

impl<T> Clone for OneShot<T> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl<T> OneShot<T> {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(OneShotState {
                value: None,
                waker: None,
            })),
        }
    }

    pub(crate) fn fill(&self, value: T) {
        let mut state = self.state.lock().expect("oneshot state");
        state.value = Some(value);
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

impl<T> Future for OneShot<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut state = self.state.lock().expect("oneshot state");
        match state.value.take() {
            Some(value) => Poll::Ready(value),
            None => {
                state.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

pub struct RunFuture<T> {
    slot: OneShot<Result<T, String>>,
}

impl<T> Future for RunFuture<T> {
    type Output = Result<T, String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.slot).poll(cx)
    }
}

fn uuid_v4() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e3779b97f4a7c15)
        ^ (std::process::id() as u64).rotate_left(32)
        ^ COUNTER.fetch_add(0x9e3779b97f4a7c15, Ordering::Relaxed);
    let mut next = || {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        seed.wrapping_mul(0x2545F4914F6CDD1D)
    };
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&next().to_le_bytes());
    bytes[8..].copy_from_slice(&next().to_le_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connector that must never be reached: dialing from the wrong
    /// thread IS the bug these tests pin.
    struct NoDial;
    impl crate::transport::Connector for NoDial {
        fn dial(
            &self,
            _url: String,
            _tag: String,
            _dead: Arc<std::sync::atomic::AtomicBool>,
        ) -> crate::transport::Dialing {
            panic!("this road must not dial");
        }
    }

    fn down_host(runtime: &tokio::runtime::Runtime) -> WireHost {
        WireHost::at(runtime.handle().clone(), Arc::new(NoDial), "ws://nowhere:1/")
    }

    /// A RUNTIME WORKER never parks on the reconnect lock and never
    /// dials: the dial and its own timeout are driven by this runtime
    /// (one worker in production), so parking it freezes the reactor
    /// and the in-flight dial never resolves NOR times out — the
    /// 2026-10-05 deadlock: main thread (terminal resize mid-paint),
    /// executor (mid-dial, holding the lock) and the sole worker all
    /// wedged forever.
    #[test]
    fn a_runtime_worker_never_dials_or_parks_for_a_reconnect() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        let host = down_host(&runtime);
        let outcome = runtime.block_on(async { host.ensure_active() });
        assert!(
            outcome.is_err(),
            "on the runtime, a dead connection is an ERROR, not a wait"
        );
    }

    /// The gesture asks — a keystroke into a terminal, a resize
    /// mid-paint — come synchronously from view perform code on the
    /// UI thread. Against a down host they are DROPPED on the spot:
    /// no dial (15 seconds), no parking behind someone else's.
    #[test]
    fn a_gesture_ask_against_a_down_host_is_dropped_not_waited() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        let host = down_host(&runtime);
        let channel = crate::client::ChannelUri::new("ahp-terminal:/t");
        use crate::client::TerminalClient;
        host.terminal_input(&channel, "ls\n".to_owned());
        host.terminal_resize(&channel, 80, 24);
        host.terminal_dispose(&channel);
        // Still here, synchronously: nothing dialed (NoDial panics),
        // nothing blocked.
    }

    #[test]
    fn uuids_are_v4_shaped_and_distinct() {
        let a = uuid_v4();
        let b = uuid_v4();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'4', "version nibble: {a}");
        assert!(
            matches!(a.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
            "variant: {a}"
        );
    }

    #[test]
    fn a_feeds_push_wakes_every_waiter() {
        struct Flag(std::sync::atomic::AtomicBool);
        impl std::task::Wake for Flag {
            fn wake(self: Arc<Self>) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let feed = Arc::new(Feed::default());
        let first = Arc::new(Flag(Default::default()));
        let second = Arc::new(Flag(Default::default()));
        let first_waker = std::task::Waker::from(Arc::clone(&first));
        let second_waker = std::task::Waker::from(Arc::clone(&second));
        let mut poll_first = PollFeed {
            feed: Arc::clone(&feed),
        };
        let mut poll_second = PollFeed {
            feed: Arc::clone(&feed),
        };
        assert!(Pin::new(&mut poll_first)
            .poll(&mut Context::from_waker(&first_waker))
            .is_pending());
        assert!(Pin::new(&mut poll_second)
            .poll(&mut Context::from_waker(&second_waker))
            .is_pending());

        feed.push(StateAction::Unknown(serde_json::Value::Null));
        let woken = std::sync::atomic::Ordering::SeqCst;
        assert!(
            first.0.load(woken) && second.0.load(woken),
            "a push must wake EVERY waiter — an overwritten waker is a \
             permanently parked poll chain (the changes refresh chip arms \
             a second poll on a channel that already has one standing)"
        );
    }

    struct Nowhere;

    impl crate::transport::Connector for Nowhere {
        fn dial(
            &self,
            url: String,
            _tag: String,
            _dead: Arc<std::sync::atomic::AtomicBool>,
        ) -> crate::transport::Dialing {
            Box::pin(async move { Err(format!("nowhere to dial {url}")) })
        }
    }

    #[test]
    fn discovery_is_per_client_and_the_env_override_reaches_vscode() {
        std::env::set_var("HIMARK_AHP_URL", "ws://127.0.0.1:9999/?tkn=t");
        assert_eq!(
            WireHost::new(test_runtime(), Arc::new(Nowhere))
                .discover_url()
                .unwrap(),
            "ws://127.0.0.1:9999/?tkn=t"
        );

        assert_eq!(
            WireHost::at(test_runtime(), Arc::new(Nowhere), "unix:/tmp/x.sock")
                .discover_url()
                .unwrap(),
            "unix:/tmp/x.sock"
        );
        std::env::remove_var("HIMARK_AHP_URL");
    }
}
