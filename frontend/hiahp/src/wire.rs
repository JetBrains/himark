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
    ResourceListParams, ResourceListResult, ResourceReadParams, ResourceWriteParams,
    SubscribeResult,
};
use ahp_types::common::Uri;
use ahp_types::state::{
    AgentInfo, ChatState, Message, MessageKind, MessageOrigin, ModelSelection, SessionState,
    SnapshotState,
};

use himark::higent::{AhpServer, RootInfo, SeatFuture, ServerEvent, SessionsPage};
use himark::higent::{FileEditContents, TurnsPage};

const ROOT: &str = "ahp-root://";

fn annotations_channel(session: &Uri) -> Uri {
    format!("{session}/annotations")
}

#[derive(Debug)]
pub enum Discovery {
    Explicit(String),

    VsCode,

    HimarkHost,
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
    active: Mutex<Option<Arc<Active>>>,
    next_turn: AtomicU64,
    discovery: Discovery,

    runtime: tokio::runtime::Handle,

    tag: String,

    client_id: String,

    last_seen: Arc<std::sync::atomic::AtomicI64>,

    connector: Arc<dyn crate::transport::Connector>,
}

struct Active {
    client: ahp::Client,

    dead: Arc<std::sync::atomic::AtomicBool>,

    feeds: Mutex<HashMap<Uri, Arc<Feed>>>,

    root: Mutex<Option<Arc<RootFeed>>>,

    agents: Mutex<Vec<AgentInfo>>,
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

    pub fn himark_host(
        runtime: tokio::runtime::Handle,
        connector: Arc<dyn crate::transport::Connector>,
    ) -> Self {
        Self::with(Discovery::HimarkHost, runtime, connector)
    }

    fn with(
        discovery: Discovery,
        runtime: tokio::runtime::Handle,
        connector: Arc<dyn crate::transport::Connector>,
    ) -> Self {
        static SEAT: AtomicU64 = AtomicU64::new(1);
        host_discovery::logging::init("app");
        let tag = format!("seat#{}", SEAT.fetch_add(1, Ordering::Relaxed));
        tracing::info!(target: "ahp_wire", seat = %tag, ?discovery, "seat opened");
        Self {
            active: Mutex::new(None),
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
            Discovery::HimarkHost => {
                if let Some(lock) = host_discovery::default_dir()
                    .as_deref()
                    .and_then(host_discovery::read_live)
                {
                    return Ok(format!("unix:{}", lock.socket.display()));
                }
                Self::autostart().ok_or_else(|| {
                    "no himark agent host running — start one with `himark-agent-host`".to_owned()
                })
            }
        }
    }

    fn autostart() -> Option<String> {
        let gate = std::env::var("HIMARK_HOST_AUTOSTART").ok()?;
        let dir = host_discovery::default_dir()?;
        let binary = match gate.as_str() {
            "1" => host_discovery::resolve_daemon_binary()?,
            path => std::path::PathBuf::from(path),
        };
        std::process::Command::new(binary)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;

        for _ in 0..100 {
            if let Some(lock) = host_discovery::read_live(&dir) {
                return Some(format!("unix:{}", lock.socket.display()));
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        None
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
                tracing::warn!(target: "ahp_wire", seat = %tag, %error, "seat call failed");
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
            let dead = Arc::clone(&active.dead);
            let died = async move {
                loop {
                    if dead.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
            };
            tokio::select! {
                outcome = work(active) => outcome,
                () = died => {
                    tracing::warn!(target: "ahp_wire", seat = %tag, "ask abandoned: the connection died under it");
                    Err("the agent host connection died".to_owned())
                }
            }
        })
    }

    fn ensure_active(&self) -> Result<Arc<Active>, String> {
        let previous = {
            let mut held = self.active.lock().expect("wire active");
            match held.clone() {
                Some(active) if !active.dead.load(std::sync::atomic::Ordering::Relaxed) => {
                    return Ok(active);
                }
                Some(active) => {
                    tracing::warn!(target: "ahp_wire", seat = %self.tag, "connection DEAD — reconnecting");
                    *held = None;
                    Some(active)
                }
                None => None,
            }
        };
        match self.connect_fresh(&previous) {
            Ok(active) => Ok(active),
            Err(error) => {
                if let Some(previous) = previous {
                    *self.active.lock().expect("wire active") = Some(previous);
                }
                Err(error)
            }
        }
    }

    fn connect_fresh(&self, previous: &Option<Arc<Active>>) -> Result<Arc<Active>, String> {
        let url = match self.discover_url() {
            Ok(url) => url,
            Err(error) => {
                tracing::error!(target: "ahp_wire", seat = %self.tag, %error, "discover failed");
                return Err(error);
            }
        };
        tracing::info!(target: "ahp_wire", seat = %self.tag, %url, "connecting");

        if tokio::runtime::Handle::try_current().is_ok() {
            tracing::error!(target: "ahp_wire", seat = %self.tag, "connect refused: requested from the runtime itself");
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
        let client = self.runtime.block_on(async {
            tokio::time::timeout(connect_deadline, async {
                let transport = connector.dial(dial_url, dial_tag, transport_dead).await?;
                let client = ahp::Client::connect(transport, ahp::ClientConfig::default())
                    .await
                    .map_err(|error| format!("connect: {error}"))?;
                client
                    .initialize(
                        "himark".to_owned(),
                        vec!["0.8.0".to_owned(), "0.7.0".to_owned()],
                        Vec::new(),
                    )
                    .await
                    .map_err(|error| format!("initialize: {error}"))?;

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
                    seat = %self.tag,
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
                previous.feeds.lock().expect("wire feeds").clone(),
                previous.root.lock().expect("wire root").clone(),
                previous.agents.lock().expect("wire agents").clone(),
            ),
            None => (HashMap::new(), None, Vec::new()),
        };
        let active = Arc::new(Active {
            client,
            dead,
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
                        seat = %self.tag,
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
        tracing::info!(
            target: "ahp_wire",
            seat = %self.tag,
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

        for channel in &channels {
            let sub = active.client.attach_subscription(channel).await;
            let feed = active
                .feeds
                .lock()
                .expect("wire feeds")
                .get(channel)
                .cloned()
                .expect("a carried channel keeps its feed");
            pump_channel(sub, feed, Arc::clone(&self.last_seen), self.tag.clone());
        }
        if had_root {
            let sub = active.client.attach_subscription(ROOT).await;
            let feed = active
                .root
                .lock()
                .expect("wire root")
                .clone()
                .expect("had_root");
            pump_root(sub, feed, Arc::clone(&self.last_seen), self.tag.clone());
        }
        let last_seen = self.last_seen.load(std::sync::atomic::Ordering::Relaxed);
        tracing::info!(
            target: "ahp_wire",
            seat = %self.tag,
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
                    seat = %self.tag,
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
                        feed.push(envelope.action);
                    }
                }
                for channel in replay.missing {
                    tracing::warn!(
                        target: "ahp_wire",
                        seat = %self.tag,
                        %channel,
                        "channel is GONE on the host — dropping its feed"
                    );
                    active.feeds.lock().expect("wire feeds").remove(&channel);
                }
            }
            ahp_types::commands::ReconnectResult::Snapshot(snapshot) => {
                tracing::warn!(
                    target: "ahp_wire",
                    seat = %self.tag,
                    snapshots = snapshot.snapshots.len(),
                    "reconnect replay TOO OLD — fresh snapshots, the gap is lost"
                );
            }
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
                        tracing::error!(target: "ahp_wire", seat = %tag, %error, "keepalive ping failed — marking dead");
                        active.dead.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    Err(_) => {
                        tracing::error!(target: "ahp_wire", seat = %tag, "keepalive ping TIMED OUT — the host is DEAF; marking dead");
                        active.dead.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                }
            }
        });
    }

    async fn subscribe_pumped(
        active: &Arc<Active>,
        channel: Uri,
        last_seen: Arc<std::sync::atomic::AtomicI64>,
        seat: String,
    ) -> Result<SubscribeResult, String> {
        let (result, sub) = active
            .client
            .subscribe(channel.clone())
            .await
            .map_err(|error| format!("subscribe {channel}: {error}"))?;

        let feed = active
            .feeds
            .lock()
            .expect("wire feeds")
            .get(&channel)
            .cloned()
            .unwrap_or_default();
        active
            .feeds
            .lock()
            .expect("wire feeds")
            .insert(channel, Arc::clone(&feed));
        pump_channel(sub, feed, last_seen, seat);
        Ok(result)
    }
}

fn pump_channel(
    mut sub: ahp::SessionSubscription,
    feed: Arc<Feed>,
    last_seen: Arc<std::sync::atomic::AtomicI64>,
    seat: String,
) {
    let channel = sub.uri().to_owned();
    tokio::spawn(async move {
        while let Some(event) = sub.recv().await {
            if let ahp::SubscriptionEvent::Action(envelope) = event {
                last_seen.fetch_max(
                    envelope.server_seq as i64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                feed.push(envelope.action);
            }
        }
        tracing::info!(target: "ahp_wire", seat = %seat, %channel, "pump ended");
    });
}

fn pump_root(
    mut sub: ahp::SessionSubscription,
    feed: Arc<RootFeed>,
    last_seen: Arc<std::sync::atomic::AtomicI64>,
    seat: String,
) {
    tokio::spawn(async move {
        while let Some(event) = sub.recv().await {
            match event {
                ahp::SubscriptionEvent::SessionAdded(params) => {
                    feed.push(ServerEvent::SessionAdded(params.summary));
                }
                ahp::SubscriptionEvent::SessionRemoved(params) => {
                    feed.push(ServerEvent::SessionRemoved(params.session));
                }
                ahp::SubscriptionEvent::SessionSummaryChanged(params) => {
                    feed.push(ServerEvent::SessionChanged {
                        session: params.session,
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
        tracing::info!(target: "ahp_wire", seat = %seat, channel = ROOT, "pump ended");
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

impl AhpServer for WireHost {
    fn connect(&self) -> SeatFuture<Result<RootInfo, String>> {
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
            pump_root(sub, feed, last_seen, tag);
            let agents = match result.snapshot.map(|snapshot| snapshot.state) {
                Some(SnapshotState::Root(root)) => root.agents,
                _ => return Err("the subscribe answered no root snapshot".to_owned()),
            };
            *active.agents.lock().expect("wire agents") = agents.clone();
            Ok(RootInfo { agents })
        }))
    }

    fn list_sessions(&self, cursor: Option<String>) -> SeatFuture<Result<SessionsPage, String>> {
        Box::pin(self.run_ask(|active| async move {
            let result: ListSessionsResult = active
                .client
                .request(
                    "listSessions",
                    ListSessionsParams {
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

    fn poll_root(&self) -> SeatFuture<Vec<ServerEvent>> {
        let feed = self
            .ensure_active()
            .ok()
            .and_then(|active| active.root.lock().expect("wire root").clone());
        match feed {
            Some(feed) => Box::pin(PollRootFeed { feed }),
            None => {
                tracing::info!(target: "ahp_wire", seat = %self.tag, "root poll parked: not connected");
                eprintln!("[hiahp] root poll parked: not connected");
                Box::pin(std::future::pending())
            }
        }
    }

    fn create_session(
        &self,
        working_directories: Vec<Uri>,
        options: himark::higent::SessionOptions,
    ) -> SeatFuture<Result<Uri, String>> {
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
            Ok(session)
        }))
    }

    fn resolve_session_config(
        &self,
        working_directory: Option<Uri>,
        config: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> SeatFuture<Result<ahp_types::commands::ResolveSessionConfigResult, String>> {
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

    fn dispose_session(&self, session: Uri) -> SeatFuture<Result<(), String>> {
        Box::pin(self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request("disposeSession", DisposeSessionParams { channel: session })
                .await
                .map_err(|error| format!("disposeSession: {error}"))?;
            Ok(())
        }))
    }

    fn subscribe_session(&self, session: Uri) -> SeatFuture<Result<SessionState, String>> {
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_ask(move |active| async move {
            let result = WireHost::subscribe_pumped(&active, session, last_seen, tag).await?;
            session_state(result)
        }))
    }

    fn poll_session(&self, session: Uri) -> SeatFuture<Vec<StateAction>> {
        self.poll_channel(session)
    }

    fn create_chat(&self, session: Uri) -> SeatFuture<Result<Uri, String>> {
        Box::pin(self.run_ask(move |active| async move {
            let chat = format!("ahp-chat:/{}", uuid_v4());
            let _: serde_json::Value = active
                .client
                .request(
                    "createChat",
                    CreateChatParams {
                        channel: session,
                        chat: chat.clone(),
                        initial_message: None,
                        source: None,
                        working_directories: None,
                    },
                )
                .await
                .map_err(|error| format!("createChat: {error}"))?;
            Ok(chat)
        }))
    }

    fn subscribe_chat(&self, chat: Uri) -> SeatFuture<Result<ChatState, String>> {
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_ask(move |active| async move {
            let result = WireHost::subscribe_pumped(&active, chat, last_seen, tag).await?;
            chat_state(result)
        }))
    }

    fn fetch_turns(
        &self,
        chat: Uri,
        cursor: Option<String>,
    ) -> SeatFuture<Result<TurnsPage, String>> {
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
        chat: Uri,
        text: String,
        attachments: Option<Vec<ahp_types::state::MessageAttachment>>,
        model: Option<ModelSelection>,
    ) -> SeatFuture<Result<(), String>> {
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

    fn poll_chat(&self, chat: Uri) -> SeatFuture<Vec<StateAction>> {
        self.poll_channel(chat)
    }

    fn subscribe_changeset(
        &self,
        channel: Uri,
    ) -> SeatFuture<Result<ahp_types::state::ChangesetState, String>> {
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_ask(move |active| async move {
            let result = WireHost::subscribe_pumped(&active, channel, last_seen, tag).await?;
            match result.snapshot.map(|snapshot| snapshot.state) {
                Some(SnapshotState::Changeset(state)) => Ok(*state),
                _ => Err("the subscribe answered no changeset snapshot".to_owned()),
            }
        }))
    }

    fn poll_changeset(&self, channel: Uri) -> SeatFuture<Vec<StateAction>> {
        self.poll_channel(channel)
    }

    fn unsubscribe_changeset(&self, channel: &Uri) {
        let channel = channel.clone();
        let _ = self.run_ask(move |active| async move {
            active.feeds.lock().expect("wire feeds").remove(&channel);
            active
                .client
                .unsubscribe(channel.clone())
                .await
                .map_err(|error| format!("unsubscribe {channel}: {error}"))
        });
    }

    fn subscribe_history(
        &self,
        channel: Uri,
    ) -> SeatFuture<Result<himark_ahp_ext_types::history::HistoryState, String>> {
        Box::pin(self.run_ask(move |active| async move {
            let mut sub = active.client.attach_subscription(&channel).await;
            let feed = Arc::new(Feed::default());
            active
                .feeds
                .lock()
                .expect("wire feeds")
                .insert(channel.clone(), Arc::clone(&feed));
            tokio::spawn(async move {
                while let Some(event) = sub.recv().await {
                    if let ahp::SubscriptionEvent::Action(envelope) = event {
                        feed.push(envelope.action);
                    }
                }
            });
            let result: serde_json::Value = active
                .client
                .request("subscribe", serde_json::json!({ "channel": channel }))
                .await
                .map_err(|error| format!("subscribe {channel}: {error}"))?;
            serde_json::from_value(result["snapshot"]["state"].clone())
                .map_err(|error| format!("history snapshot: {error}"))
        }))
    }

    fn subscribe_annotations(
        &self,
        session: Uri,
    ) -> SeatFuture<Result<ahp_types::state::AnnotationsState, String>> {
        let last_seen = Arc::clone(&self.last_seen);
        let tag = self.tag.clone();
        Box::pin(self.run_ask(move |active| async move {
            let channel = annotations_channel(&session);
            let result = WireHost::subscribe_pumped(&active, channel, last_seen, tag).await?;
            match result.snapshot.map(|snapshot| snapshot.state) {
                Some(SnapshotState::Annotations(state)) => Ok(*state),
                _ => Err("the subscribe answered no annotations snapshot".to_owned()),
            }
        }))
    }

    fn poll_annotations(&self, session: Uri) -> SeatFuture<Vec<StateAction>> {
        self.poll_channel(annotations_channel(&session))
    }

    fn dispatch_annotations(&self, session: &Uri, action: StateAction) {
        let channel = annotations_channel(session);
        let _ = self.run_ask(move |active| async move {
            active
                .client
                .dispatch(channel, action)
                .await
                .map(|_| ())
                .map_err(|error| format!("dispatch annotations: {error}"))
        });
    }

    fn unsubscribe_annotations(&self, session: &Uri) {
        let channel = annotations_channel(session);
        let _ = self.run_ask(move |active| async move {
            active.feeds.lock().expect("wire feeds").remove(&channel);
            active
                .client
                .unsubscribe(channel.clone())
                .await
                .map_err(|error| format!("unsubscribe {channel}: {error}"))
        });
    }

    fn open_document(
        &self,
        session: Uri,
        uri: Option<himark::higent::seat::ResourceUri>,
        text: Option<String>,
    ) -> SeatFuture<Result<himark_ahp_ext_types::OpenDocumentResult, String>> {
        let uri = uri.map(himark::higent::seat::ResourceUri::into_string);
        Box::pin(self.run_ask(move |active| async move {
            active
                .client
                .request(
                    "openDocument",
                    himark_ahp_ext_types::OpenDocumentParams {
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
        channel: Uri,
    ) -> SeatFuture<Result<himark_ahp_ext_types::DocumentState, String>> {
        Box::pin(self.run_ask(move |active| async move {
            let mut sub = active.client.attach_subscription(&channel).await;
            let feed = Arc::new(Feed::default());
            active
                .feeds
                .lock()
                .expect("wire feeds")
                .insert(channel.clone(), Arc::clone(&feed));
            tokio::spawn(async move {
                while let Some(event) = sub.recv().await {
                    if let ahp::SubscriptionEvent::Action(envelope) = event {
                        feed.push(envelope.action);
                    }
                }
            });
            let result: serde_json::Value = active
                .client
                .request("subscribe", serde_json::json!({ "channel": channel }))
                .await
                .map_err(|error| format!("subscribe {channel}: {error}"))?;
            serde_json::from_value(result["snapshot"]["state"].clone())
                .map_err(|error| format!("document snapshot: {error}"))
        }))
    }

    fn poll_document(
        &self,
        channel: Uri,
    ) -> SeatFuture<Vec<himark_ahp_ext_types::DocumentApplied>> {
        let poll = self.poll_channel(channel);
        Box::pin(async move {
            poll.await
                .into_iter()
                .filter_map(|action| match action {
                    StateAction::Unknown(value)
                        if value["type"] == himark_ahp_ext_types::DOCUMENT_APPLIED =>
                    {
                        serde_json::from_value(value).ok()
                    }
                    _ => None,
                })
                .collect()
        })
    }

    fn dispatch_document(&self, channel: &Uri, action: himark_ahp_ext_types::DocumentApplied) {
        let channel = channel.clone();
        let _ = self.run_ask(move |active| async move {
            let mut value = serde_json::to_value(&action).expect("an action serializes");
            value["type"] =
                serde_json::Value::String(himark_ahp_ext_types::DOCUMENT_APPLIED.to_owned());
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
        channel: Uri,
        uri: himark::higent::seat::ResourceUri,
    ) -> SeatFuture<Result<(), String>> {
        let uri = uri.into_string();
        Box::pin(self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "storeDocument",
                    himark_ahp_ext_types::StoreDocumentParams { channel, uri },
                )
                .await
                .map_err(|error| format!("storeDocument: {error}"))?;
            Ok(())
        }))
    }

    fn unsubscribe_document(&self, channel: &Uri) {
        let channel = channel.clone();
        let _ = self.run_ask(move |active| async move {
            active.feeds.lock().expect("wire feeds").remove(&channel);
            active
                .client
                .unsubscribe(channel.clone())
                .await
                .map_err(|error| format!("unsubscribe {channel}: {error}"))
        });
    }

    fn lsp(
        &self,
        session: Uri,
        method: String,
        params: serde_json::Value,
    ) -> SeatFuture<Result<serde_json::Value, String>> {
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

    fn http_serve(&self) -> SeatFuture<Result<String, String>> {
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

    fn dispatch_action(&self, channel: Uri, action: StateAction) -> SeatFuture<Result<(), String>> {
        Box::pin(self.run_ask(move |active| async move {
            let _ = active
                .client
                .dispatch(channel, action)
                .await
                .map_err(|error| format!("dispatch: {error}"))?;
            Ok(())
        }))
    }

    fn cancel_turn(&self, chat: Uri, turn_id: String) -> SeatFuture<()> {
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

    fn resource_read(
        &self,
        session: Uri,
        uri: himark::higent::seat::ResourceUri,
    ) -> SeatFuture<Option<String>> {
        let asked = self.run_ask(move |active| async move {
            let result = active
                .client
                .resource_read(ResourceReadParams {
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
        session: Uri,
        uri: himark::higent::seat::ResourceUri,
    ) -> SeatFuture<Option<Vec<u8>>> {
        let asked = self.run_ask(move |active| async move {
            let result = active
                .client
                .resource_read(ResourceReadParams {
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
        session: Uri,
        uri: himark::higent::seat::ResourceUri,
        text: String,
    ) -> SeatFuture<bool> {
        let asked = self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "resourceWrite",
                    ResourceWriteParams {
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

    fn resource_list(
        &self,
        session: Uri,
        uri: himark::higent::seat::ResourceUri,
    ) -> SeatFuture<Option<Vec<(String, bool)>>> {
        let asked = self.run_ask(move |active| async move {
            let result: ResourceListResult = active
                .client
                .request(
                    "resourceList",
                    ResourceListParams {
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
        session: Uri,
        uri: himark::higent::seat::ResourceUri,
        events: Arc<dyn Fn() + Send + Sync>,
    ) -> SeatFuture<Option<himark::higent::WatchHandle>> {
        let asked = self.run_ask(move |active| async move {
            let result: CreateResourceWatchResult = active
                .client
                .request(
                    "createResourceWatch",
                    CreateResourceWatchParams {
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
            Ok(Some(himark::higent::WatchHandle { channel }))
        });
        Box::pin(async move {
            asked.await.unwrap_or_else(|error| {
                eprintln!("[hiahp] {error}");
                None
            })
        })
    }

    fn search(
        &self,
        session: Uri,
        ask: himark::higent::SearchAsk,
    ) -> SeatFuture<Option<himark::higent::SearchResult>> {
        let asked = self.run_ask(move |active| async move {
            let params = himark_ahp_ext_types::SearchParams {
                channel: session,
                folders: Some(ask.folders),
                query: ask.query,
                kind: ask.kind,
                case_sensitive: ask.case_sensitive,
                target: ask.target,
                limit: Some(ask.limit as u64),
            };
            let result: himark::higent::SearchResult = active
                .client
                .request("search", params)
                .await
                .map_err(|error| format!("search: {error}"))?;
            Ok(result)
        });
        Box::pin(async move { asked.await.ok() })
    }

    fn terminal_open(
        &self,
        _session: Uri,
        channel: Uri,
        cwd: Option<Uri>,
        cols: u16,
        rows: u16,
        events: Arc<dyn Fn(himark::higent::TerminalEvent) + Send + Sync>,
    ) -> SeatFuture<Option<himark::higent::TerminalHandle>> {
        let asked = self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "createTerminal",
                    ahp_types::commands::CreateTerminalParams {
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
                            events(himark::higent::TerminalEvent::Data(part.value.clone()))
                        }
                        ahp_types::state::TerminalContentPart::Command(part) => {
                            events(himark::higent::TerminalEvent::Data(part.output.clone()))
                        }
                        _ => {}
                    }
                }
                if let Some(code) = state.exit_code {
                    events(himark::higent::TerminalEvent::Exited(Some(code as i32)));
                }
            }
            tokio::spawn(async move {
                while let Some(event) = sub.recv().await {
                    if let ahp::SubscriptionEvent::Action(envelope) = event {
                        match envelope.action {
                            StateAction::TerminalData(data) => {
                                events(himark::higent::TerminalEvent::Data(data.data))
                            }
                            StateAction::TerminalExited(exited) => {
                                events(himark::higent::TerminalEvent::Exited(
                                    exited.exit_code.map(|code| code as i32),
                                ))
                            }
                            _ => {}
                        }
                    }
                }
            });
            Ok(Some(himark::higent::TerminalHandle { channel }))
        });
        Box::pin(async move {
            asked.await.unwrap_or_else(|error| {
                eprintln!("[hiahp] {error}");
                None
            })
        })
    }

    fn terminal_input(&self, channel: &Uri, data: String) {
        let channel = channel.clone();
        let _ = self.run_ask(move |active| async move {
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

    fn terminal_resize(&self, channel: &Uri, cols: u16, rows: u16) {
        let channel = channel.clone();
        let _ = self.run_ask(move |active| async move {
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

    fn terminal_dispose(&self, channel: &Uri) {
        let channel = channel.clone();
        let _ = self.run_ask(move |active| async move {
            let _: serde_json::Value = active
                .client
                .request(
                    "disposeTerminal",
                    ahp_types::commands::DisposeTerminalParams {
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

    fn resource_unwatch(&self, handle: himark::higent::WatchHandle) -> SeatFuture<()> {
        let asked = self.run_ask(move |active| async move {
            active
                .client
                .unsubscribe(handle.channel.clone())
                .await
                .map_err(|error| format!("unsubscribe {}: {error}", handle.channel))
        });
        Box::pin(async move {
            if let Err(error) = asked.await {
                eprintln!("[hiahp] {error}");
            }
        })
    }

    fn read_file_edit(
        &self,
        before: Option<Uri>,
        after: Option<Uri>,
    ) -> SeatFuture<Result<FileEditContents, String>> {
        Box::pin(self.run_ask(move |active| async move {
            let read = |uri: Option<Uri>| {
                let client = active.client.clone();
                async move {
                    let Some(uri) = uri else {
                        return Ok::<Option<String>, String>(None);
                    };
                    let result = client
                        .resource_read(ResourceReadParams {
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

impl WireHost {
    fn poll_channel(&self, channel: Uri) -> SeatFuture<Vec<StateAction>> {
        let active = self.ensure_active().ok();
        Box::pin(async move {
            let Some(active) = active else {
                // Not connected: park. Reconnect re-subscribes and
                // re-arms the poll from scratch.
                return std::future::pending().await;
            };
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
                        // for the feed instead.
                        if !waited {
                            waited = true;
                            eprintln!("[hiahp] poll waiting: not subscribed yet: {channel}");
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
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
    waker: Option<Waker>,

    turns_capture: Option<OneShot<TurnsPage>>,
}

impl Feed {
    fn push(&self, action: StateAction) {
        let mut state = self.state.lock().expect("feed state");
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
        if let Some(waker) = state.waker.take() {
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
            state.waker = Some(cx.waker().clone());
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
    waker: Option<Waker>,
}

impl RootFeed {
    fn push(&self, event: ServerEvent) {
        let mut state = self.state.lock().expect("root feed");
        state.events.push_back(event);
        if let Some(waker) = state.waker.take() {
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
            state.waker = Some(cx.waker().clone());
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
    fn discovery_is_per_seat_and_the_env_override_reaches_vscode() {
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
