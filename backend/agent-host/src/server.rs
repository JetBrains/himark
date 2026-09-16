// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ahp_types::actions::{
    ChangesetContentChangedAction, ChangesetStatusChangedAction, ChatPendingMessageRemovedAction,
    ChatTurnStartedAction, RootTerminalsChangedAction, StateAction, TerminalDataAction,
    TerminalExitedAction,
};
use ahp_types::commands::{
    CreateTerminalParams, DisposeTerminalParams, Implementation, InitializeParams,
    InitializeResult, ListSessionsParams, ListSessionsResult, SubscribeParams, SubscribeResult,
    UnsubscribeParams,
};
use ahp_types::common::Uri;
use ahp_types::messages::{JsonRpcMessage, JsonRpcRequest};
use ahp_types::notifications::PartialSessionSummary;
use ahp_types::state::{
    AgentInfo, Annotation, AnnotationsState, AnnotationsSummary, ChangesetState, ChangesetStatus,
    ChatOrigin, ChatState, ChatSummary, ErrorInfo, MessageAttachment, PendingMessageKind,
    RootState, SessionLifecycle, SessionState, SessionSummary, Snapshot, SnapshotState,
    TerminalContentPart, TerminalInfo, TerminalState,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::rpc;
use crate::store::{Manifest, Store};

pub(crate) const ROOT: &str = "ahp-root://";

const ANNOTATIONS_SUFFIX: &str = "/annotations";

fn annotations_session(channel: &Uri) -> Option<Uri> {
    channel.strip_suffix(ANNOTATIONS_SUFFIX).map(str::to_owned)
}

fn annotations_channel(session: &Uri) -> Uri {
    format!("{session}{ANNOTATIONS_SUFFIX}")
}

fn annotations_summary(session: &Uri, annotations: &[Annotation]) -> AnnotationsSummary {
    AnnotationsSummary {
        resource: annotations_channel(session),
        annotation_count: annotations.len() as i64,
        entry_count: annotations
            .iter()
            .map(|annotation| annotation.entries.len() as i64)
            .sum(),
    }
}

const TURN_TAIL: usize = 10;
const TURN_PAGE: usize = 10;

const REPLAY_DEPTH: usize = 512;

const NO_SUCH_CHANNEL: i32 = -32001;
const VERSION_MISMATCH: i32 = -32005;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL: i32 = -32603;

#[derive(Clone)]
pub struct HostConfig {
    pub agents: Vec<AgentInfo>,

    pub data_dir: PathBuf,

    pub claude_binary: String,

    pub codex_binary: String,

    pub claude_home: PathBuf,

    pub codex_home: PathBuf,

    pub shell: String,

    pub language_servers: Vec<LanguageServer>,
}

#[derive(Clone, Debug)]
pub struct LanguageServer {
    pub extensions: Vec<String>,

    pub command: String,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            agents: vec![claude_card(), codex_card()],
            data_dir: crate::lock::default_dir()
                .unwrap_or_else(|| PathBuf::from(".himark-agent-host")),
            claude_binary: crate::claude::discover_binary(),
            codex_binary: crate::codex::discover_binary(),
            claude_home: std::env::var("HIMARK_CLAUDE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_default();
                    PathBuf::from(home).join(".claude")
                }),
            codex_home: std::env::var("HIMARK_CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_default();
                    PathBuf::from(home).join(".codex")
                }),
            shell: std::env::var("HIMARK_SHELL")
                .or_else(|_| std::env::var("SHELL"))
                .unwrap_or_else(|_| "/bin/sh".to_owned()),
            language_servers: vec![LanguageServer {
                extensions: vec!["rs".to_owned()],
                command: crate::lsp::discover_rust_analyzer(),
            }],
        }
    }
}

fn claude_card() -> AgentInfo {
    AgentInfo {
        provider: "claude".to_owned(),
        display_name: "Claude".to_owned(),
        description: "Claude Code on your own login, driven natively by the himark agent host"
            .to_owned(),
        models: claude_models(),
        protected_resources: None,
        customizations: None,
        capabilities: None,
    }
}

fn claude_models() -> Vec<ahp_types::state::SessionModelInfo> {
    [
        ("claude-fable-5", "Fable"),
        ("claude-opus-5", "Opus"),
        ("claude-sonnet-5", "Sonnet"),
        ("claude-haiku-4-5-20251001", "Haiku"),
    ]
    .into_iter()
    .map(|(id, name)| ahp_types::state::SessionModelInfo {
        id: id.to_owned(),
        provider: "claude".to_owned(),
        name: name.to_owned(),
        max_context_window: None,
        max_output_tokens: None,
        max_prompt_tokens: None,
        supports_vision: Some(true),
        policy_state: None,
        config_schema: Some(thinking_schema(
            &["low", "medium", "high", "max"],
            &["Low", "Medium", "High", "Max"],
            "high",
        )),
        meta: None,
    })
    .collect()
}

fn codex_card() -> AgentInfo {
    AgentInfo {
        provider: "codex".to_owned(),
        display_name: "Codex".to_owned(),
        description: "Codex on your own login, driven natively by the himark agent host".to_owned(),
        models: codex_models(),
        protected_resources: None,
        customizations: None,
        capabilities: None,
    }
}

fn codex_models() -> Vec<ahp_types::state::SessionModelInfo> {
    [
        ("gpt-6-astra", "GPT-6 Astra", true),
        ("gpt-5.6-sol", "GPT-5.6 Sol", true),
        ("gpt-5.6-terra", "GPT-5.6 Terra", true),
        ("gpt-5.6-luna", "GPT-5.6 Luna", false),
    ]
    .into_iter()
    .map(|(id, name, ultra)| {
        let levels: &[&str] = if ultra {
            &["low", "medium", "high", "xhigh", "max", "ultra"]
        } else {
            &["low", "medium", "high", "xhigh", "max"]
        };
        let labels: Vec<String> = levels
            .iter()
            .map(|level| match *level {
                "xhigh" => "Extra high".to_owned(),
                other => {
                    let mut chars = other.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                        .unwrap_or_default()
                }
            })
            .collect();
        ahp_types::state::SessionModelInfo {
            id: id.to_owned(),
            provider: "codex".to_owned(),
            name: name.to_owned(),
            max_context_window: None,
            max_output_tokens: None,
            max_prompt_tokens: None,
            supports_vision: Some(true),
            policy_state: None,
            config_schema: Some(thinking_schema_owned(levels, &labels, "medium")),
            meta: None,
        }
    })
    .collect()
}

fn thinking_schema(
    levels: &[&str],
    labels: &[&str],
    default: &str,
) -> ahp_types::state::ConfigSchema {
    let labels: Vec<String> = labels.iter().map(|label| (*label).to_owned()).collect();
    thinking_schema_owned(levels, &labels, default)
}

fn thinking_schema_owned(
    levels: &[&str],
    labels: &[String],
    default: &str,
) -> ahp_types::state::ConfigSchema {
    use ahp_types::state::ConfigPropertySchema;
    let mut properties = std::collections::HashMap::new();
    properties.insert(
        "thinkingLevel".to_owned(),
        ConfigPropertySchema {
            r#type: "string".to_owned(),
            title: "Effort".to_owned(),
            description: Some("How hard the model thinks before answering.".to_owned()),
            default: Some(serde_json::json!(default)),
            r#enum: Some(
                levels
                    .iter()
                    .map(|level| serde_json::json!(level))
                    .collect(),
            ),
            enum_labels: Some(labels.to_vec()),
            enum_descriptions: None,
            read_only: None,
            items: None,
            properties: None,
            required: None,
            additional_properties: None,
        },
    );
    ahp_types::state::ConfigSchema {
        r#type: "object".to_owned(),
        properties,
        required: None,
    }
}

fn session_config_schema() -> ahp_types::state::SessionConfigSchema {
    use ahp_types::state::SessionConfigPropertySchema;
    fn property(
        kind: &str,
        title: &str,
        default: serde_json::Value,
        options: Option<(&[&str], &[&str])>,
    ) -> SessionConfigPropertySchema {
        SessionConfigPropertySchema {
            r#type: kind.to_owned(),
            title: title.to_owned(),
            description: None,
            default: Some(default),
            r#enum: options.map(|(values, _)| {
                values
                    .iter()
                    .map(|value| serde_json::json!(value))
                    .collect()
            }),
            enum_labels: options
                .map(|(_, labels)| labels.iter().map(|label| (*label).to_owned()).collect()),
            enum_descriptions: None,
            read_only: None,
            items: None,
            properties: None,
            required: None,
            additional_properties: None,
            enum_dynamic: None,
            session_mutable: None,
        }
    }
    let mut properties = std::collections::HashMap::new();
    properties.insert(
        "mode".to_owned(),
        property(
            "string",
            "Mode",
            serde_json::json!("agent"),
            Some((&["agent"], &["Agent"])),
        ),
    );
    properties.insert(
        "permissionMode".to_owned(),
        SessionConfigPropertySchema {
            session_mutable: Some(true),
            ..property(
                "string",
                "Edits",
                serde_json::json!("default"),
                Some((
                    &["default", "acceptEdits", "bypassPermissions", "plan"],
                    &["Ask first", "Accept edits", "Auto", "Plan"],
                )),
            )
        },
    );
    properties.insert(
        "worktree".to_owned(),
        property("boolean", "New worktree", serde_json::json!(false), None),
    );
    ahp_types::state::SessionConfigSchema {
        r#type: "object".to_owned(),
        properties,
        required: None,
    }
}

fn session_config_values(
    overlay: Option<&serde_json::Map<String, serde_json::Value>>,
) -> serde_json::Map<String, serde_json::Value> {
    let schema = session_config_schema();
    let mut values = serde_json::Map::new();
    for (key, property) in &schema.properties {
        let mut value = property.default.clone().unwrap_or(serde_json::Value::Null);
        if let Some(candidate) = overlay.and_then(|overlay| overlay.get(key)) {
            let allowed = property
                .r#enum
                .as_ref()
                .map(|options| options.contains(candidate))
                .unwrap_or(candidate.is_boolean());
            if allowed {
                value = candidate.clone();
            }
        }
        values.insert(key.clone(), value);
    }
    values
}

type Outbox = tokio::sync::mpsc::UnboundedSender<Vec<u8>>;

fn subscribe_outbox(state: &mut State, channel: &Uri, connection: u64, outbox: &Outbox) {
    let mut rows = state
        .subscribers
        .get(channel)
        .cloned()
        .unwrap_or_else(rpds::VectorSync::new_sync);
    rows.push_back_mut((connection, outbox.clone()));
    state.subscribers.insert_mut(channel.clone(), rows);
}

fn drop_subscriber(
    subscribers: &rpds::VectorSync<(u64, Outbox)>,
    connection: u64,
) -> rpds::VectorSync<(u64, Outbox)> {
    let mut kept = rpds::VectorSync::new_sync();
    for row in subscribers.iter() {
        if row.0 != connection {
            kept.push_back_mut(row.clone());
        }
    }
    kept
}

#[derive(Clone)]
struct SessionEntry {
    state: SessionState,
    manifest: Manifest,

    documents: rpds::HashTrieMapSync<Uri, crate::documents::Document>,

    mirrors: rpds::HashTrieMapSync<Uri, Uri>,

    /// The disk text each mirrored channel last reconciled with —
    /// the three-way base when the file changes under unflushed
    /// client edits (documents/reload.rs).
    disk_texts: rpds::HashTrieMapSync<Uri, String>,

    annotations: AnnotationsState,
}

#[derive(Clone)]
struct ChatEntry {
    state: ChatState,
    session: Uri,

    native_id: String,

    provider_session_id: Option<String>,
    agent: Option<LiveAgent>,

    spoken: bool,

    retire_after_turn: bool,
}

#[derive(Clone)]
enum LiveAgent {
    Claude(Arc<crate::claude::ClaudeAgent>),
    Codex(Arc<crate::codex::CodexAgent>),
}

impl LiveAgent {
    fn is_dead(&self) -> bool {
        match self {
            Self::Claude(agent) => agent.is_dead(),
            Self::Codex(agent) => agent.is_dead(),
        }
    }

    async fn prompt(&self, turn_id: String, text: String) -> Result<(), String> {
        match self {
            Self::Claude(agent) => agent.prompt(turn_id, text).await,
            Self::Codex(agent) => agent.prompt(turn_id, text).await,
        }
    }

    async fn answer(&self, tool_call_id: &str, approved: bool) -> Result<(), String> {
        match self {
            Self::Claude(agent) => agent.answer(tool_call_id, approved).await,
            Self::Codex(agent) => agent.answer(tool_call_id, approved).await,
        }
    }

    async fn interrupt(&self) -> Result<(), String> {
        match self {
            Self::Claude(agent) => agent.interrupt().await,
            Self::Codex(agent) => agent.interrupt().await,
        }
    }

    fn idle(&self) -> bool {
        match self {
            Self::Claude(agent) => agent.idle(),
            Self::Codex(agent) => agent.idle(),
        }
    }

    fn shutdown(&self) {
        match self {
            Self::Claude(agent) => agent.shutdown(),
            Self::Codex(agent) => agent.shutdown(),
        }
    }
}

#[derive(Clone)]
struct TerminalEntry {
    state: TerminalState,
    pty: Option<PtyHalves>,
}

#[derive(Clone)]
struct PtyHalves {
    writer: Arc<std::fs::File>,
    controller: Arc<std::os::fd::OwnedFd>,
    child: Arc<Mutex<std::process::Child>>,
}

#[derive(Clone)]
struct LspDiagnostics {
    session: Uri,
    items: rpds::HashTrieMapSync<String, (Option<String>, Value)>,
}

#[derive(Clone)]
struct ChangesetEntry {
    state: ChangesetState,
    folder: PathBuf,

    _watch: Option<Arc<notify::PollWatcher>>,
}

#[derive(Clone)]
struct HistoryEntry {
    state: himark_ahp_ext_types::history::HistoryState,
    folder: PathBuf,
    _watch: Option<Arc<notify::PollWatcher>>,
}

#[derive(Clone)]
struct State {
    server_seq: i64,
    root: RootState,
    sessions: rpds::HashTrieMapSync<Uri, SessionEntry>,
    chats: rpds::HashTrieMapSync<Uri, ChatEntry>,
    subscribers: rpds::HashTrieMapSync<Uri, rpds::VectorSync<(u64, Outbox)>>,
    terminals: rpds::HashTrieMapSync<Uri, TerminalEntry>,
    changesets: rpds::HashTrieMapSync<Uri, ChangesetEntry>,

    histories: rpds::HashTrieMapSync<Uri, HistoryEntry>,

    document_seq: u64,

    lsp_diagnostics: rpds::HashTrieMapSync<Uri, LspDiagnostics>,

    watches: rpds::HashTrieMapSync<Uri, WatchEntry>,

    /// One file watcher per mirrored document channel: the host owns
    /// disk reloads for mirrors (documents/reload.rs) and broadcasts
    /// them as its own edits.
    mirror_watches: rpds::HashTrieMapSync<Uri, WatchEntry>,

    contents: rpds::HashTrieMapSync<Uri, String>,

    replay: rpds::QueueSync<ahp_types::actions::ActionEnvelope>,
    next_connection: u64,

    searches: rpds::HashTrieMapSync<u64, Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Clone)]
struct WatchEntry {
    _watcher: Arc<notify::PollWatcher>,
    root: Uri,
}

pub struct Host {
    state: Mutex<Arc<State>>,
    store: Store,
    config: HostConfig,

    lsp: crate::lsp::Pool,

    lsp_inflight: Mutex<HashMap<(u64, u64), (Arc<crate::lsp::Server>, i64)>>,

    lsp_cancelled: Mutex<std::collections::HashSet<(u64, u64)>>,

    pub(crate) trace: Arc<crate::trace::HostTrace>,

    pub(crate) http: crate::http::HttpServer,

    pub(crate) web_root: std::sync::Mutex<Option<std::path::PathBuf>>,
}

impl Host {
    pub fn new(config: HostConfig) -> Arc<Self> {
        let store = Store::new(config.data_dir.clone());
        let mut sessions = rpds::HashTrieMapSync::new_sync();
        let mut chats = rpds::HashTrieMapSync::new_sync();

        let mut local_annotations = Vec::new();
        for manifest in store.manifests() {
            if manifest.session == host_discovery::LOCAL_FS_SESSION {
                local_annotations = manifest.annotations;
                continue;
            }
            let mut chat_state = empty_chat(&manifest.default_chat.clone(), &manifest.title);
            for action in store.replay(&manifest.native_id, &manifest.default_chat) {
                let _ = ahp::reducers::apply_action_to_chat(&mut chat_state, &action);
            }
            let spoken = !chat_state.turns.is_empty();
            let mut manifest = manifest;
            if !manifest.listed {
                if spoken {
                    manifest.listed = true;
                    let _ = store.write_manifest(&manifest);
                } else {
                    let _ = store.remove_session(&manifest.native_id);
                    continue;
                }
            }
            chats.insert_mut(
                manifest.default_chat.clone(),
                ChatEntry {
                    state: chat_state,
                    session: manifest.session.clone(),
                    native_id: manifest.native_id.clone(),
                    provider_session_id: manifest.provider_session_id.clone().or_else(|| {
                        (manifest.provider == "claude").then(|| manifest.native_id.clone())
                    }),
                    agent: None,
                    spoken,
                    retire_after_turn: false,
                },
            );
            sessions.insert_mut(
                manifest.session.clone(),
                SessionEntry {
                    state: session_state(&manifest),
                    annotations: AnnotationsState {
                        annotations: manifest.annotations.clone(),
                    },
                    manifest,
                    documents: rpds::HashTrieMapSync::new_sync(),
                    mirrors: rpds::HashTrieMapSync::new_sync(),
                    disk_texts: rpds::HashTrieMapSync::new_sync(),
                },
            );
        }
        let active = sessions.size() as i64;

        let local = Manifest {
            session: host_discovery::LOCAL_FS_SESSION.to_owned(),
            native_id: "local-fs".to_owned(),
            provider_session_id: None,
            provider: "hihost".to_owned(),
            working_directories: Vec::new(),
            primary: None,
            default_chat: String::new(),
            title: "Local Files".to_owned(),
            created_at: String::new(),
            annotations: local_annotations,
            model: None,
            thinking_level: None,
            permission_mode: None,
            worktree: false,
            listed: true,
        };
        sessions.insert_mut(
            local.session.clone(),
            SessionEntry {
                state: SessionState {
                    chats: Vec::new(),
                    default_chat: None,
                    changesets: Some(Vec::new()),
                    ..session_state(&local)
                },
                annotations: AnnotationsState {
                    annotations: local.annotations.clone(),
                },
                manifest: local,
                documents: rpds::HashTrieMapSync::new_sync(),
                mirrors: rpds::HashTrieMapSync::new_sync(),
                disk_texts: rpds::HashTrieMapSync::new_sync(),
            },
        );
        Arc::new_cyclic(|weak: &std::sync::Weak<Self>| {
            let events_host = weak.clone();
            let lsp = crate::lsp::Pool::new(Arc::new(move |server, event| {
                if let Some(host) = events_host.upgrade() {
                    host.ls_event(server, event);
                }
            }));
            Self {
                lsp,
                lsp_inflight: Mutex::new(HashMap::new()),
                lsp_cancelled: Mutex::new(std::collections::HashSet::new()),
                trace: crate::trace::HostTrace::new(),
                http: crate::http::HttpServer::new(),
                web_root: std::sync::Mutex::new(None),
                state: Mutex::new(Arc::new(State {
                    server_seq: 0,
                    root: RootState {
                        agents: config.agents.clone(),
                        active_sessions: Some(active),
                        terminals: None,
                        config: None,
                        meta: None,
                    },
                    sessions,
                    chats,
                    subscribers: rpds::HashTrieMapSync::new_sync(),
                    terminals: rpds::HashTrieMapSync::new_sync(),
                    changesets: rpds::HashTrieMapSync::new_sync(),
                    histories: rpds::HashTrieMapSync::new_sync(),
                    document_seq: 0,
                    lsp_diagnostics: rpds::HashTrieMapSync::new_sync(),
                    watches: rpds::HashTrieMapSync::new_sync(),
                    mirror_watches: rpds::HashTrieMapSync::new_sync(),
                    contents: rpds::HashTrieMapSync::new_sync(),
                    replay: rpds::QueueSync::new_sync(),
                    next_connection: 0,
                    searches: rpds::HashTrieMapSync::new_sync(),
                })),
                store,
                config,
            }
        })
    }

    fn snapshot(&self) -> Arc<State> {
        Arc::clone(
            &self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn update<R>(&self, mutate: impl FnOnce(&mut State) -> R) -> R {
        let mut held = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut next = State::clone(&held);
        let result = mutate(&mut next);
        *held = Arc::new(next);
        result
    }

    pub async fn bind(self: Arc<Self>, socket: &std::path::Path) -> std::io::Result<()> {
        let _ = std::fs::remove_file(socket);
        if let Some(parent) = socket.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let listener = UnixListener::bind(socket)?;
        self.trace.ensure_watchdog();
        tracing::info!(
            target: "ahp_host",
            host = %self.trace.tag,
            socket = %socket.display(),
            "listening"
        );
        loop {
            let accepted = listener.accept().await;
            if let Err(error) = &accepted {
                tracing::error!(target: "ahp_host", host = %self.trace.tag, %error, "accept error");
            }
            let (stream, _addr) = accepted?;
            tracing::info!(target: "ahp_host", host = %self.trace.tag, "accepted a connection");
            let host = Arc::clone(&self);
            tokio::spawn(async move {
                let tag = host.trace.tag.clone();
                let outcome = host.serve_stream(stream).await;
                tracing::info!(target: "ahp_host", host = %tag, ?outcome, "connection task ended");
            });
        }
    }

    pub async fn serve_stream(self: Arc<Self>, stream: UnixStream) -> std::io::Result<()> {
        let (read, mut write) = stream.into_split();
        let (outbox, mut inbox) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let connection = self.open_connection();

        let writer = tokio::spawn(async move {
            while let Some(line) = inbox.recv().await {
                if write.write_all(&line).await.is_err() {
                    break;
                }
                if write.flush().await.is_err() {
                    break;
                }
            }
        });
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            self.handle_line(connection, &outbox, &line).await;
        }
        self.close_connection(connection);
        writer.abort();
        tracing::info!(target: "ahp_host", host = %self.trace.tag, connection, "connection closed");
        Ok(())
    }

    pub(crate) async fn serve_ws(
        self: Arc<Self>,
        stream: axum::extract::ws::WebSocket,
    ) -> std::io::Result<()> {
        use axum::extract::ws::Message;
        use futures_util::{SinkExt, StreamExt};
        let (mut sink, mut source) = stream.split();
        let (outbox, mut inbox) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let connection = self.open_connection();
        let writer = tokio::spawn(async move {
            while let Some(line) = inbox.recv().await {
                let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        });
        while let Some(Ok(message)) = source.next().await {
            match message {
                Message::Text(text) => {
                    self.handle_line(connection, &outbox, text.as_str()).await;
                }
                Message::Close(_) => break,

                _ => {}
            }
        }
        self.close_connection(connection);
        writer.abort();
        tracing::info!(target: "ahp_host", host = %self.trace.tag, connection, "connection closed");
        Ok(())
    }

    pub(crate) fn open_connection(&self) -> u64 {
        let connection = self.update(|state| {
            state.next_connection += 1;
            state.next_connection
        });
        self.trace.ensure_watchdog();
        tracing::info!(target: "ahp_host", host = %self.trace.tag, connection, "connection open");
        connection
    }

    pub(crate) fn handle_line<'a>(
        self: &'a Arc<Self>,
        connection: u64,
        outbox: &'a Outbox,
        line: &'a str,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            if line.trim().is_empty() {
                return;
            }
            let Ok(message) = serde_json::from_str::<JsonRpcMessage>(line) else {
                return;
            };
            match message {
                JsonRpcMessage::Request(request) => {
                    tracing::info!(
                        target: "ahp_host",
                        host = %self.trace.tag,
                        connection,
                        method = %request.method,
                        id = request.id,
                        line = %host_discovery::logging::brief(&line),
                        "request"
                    );

                    let host = Arc::clone(&self);
                    let outbox = outbox.clone();

                    let answering: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                        Box::pin(async move {
                            let seq = host.trace.begin(connection, request.id, &request.method);
                            let method = request.method.clone();
                            let id = request.id;
                            let answer = host.answer(connection, &outbox, request).await;
                            let took = host.trace.end(seq).unwrap_or_default();
                            let delivered = outbox.send(rpc::line(&answer)).is_ok();
                            tracing::info!(
                                target: "ahp_host",
                                host = %host.trace.tag,
                                connection,
                                method = %method,
                                id,
                                took_ms = took.as_secs_f64() * 1000.0,
                                delivered,
                                "answered"
                            );
                        });
                    tokio::spawn(answering);
                }
                JsonRpcMessage::Notification(notification) => {
                    tracing::info!(
                        target: "ahp_host",
                        host = %self.trace.tag,
                        connection,
                        method = %notification.method,
                        line = %host_discovery::logging::brief(&line),
                        "notification"
                    );
                    let params = notification.params.unwrap_or(Value::Null);
                    if notification.method == "dispatchAction" {
                        self.dispatch(params).await;
                    } else if notification.method == "lsp/$/cancelRequest" {
                        self.lsp_cancel(connection, &params);
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn close_connection(&self, connection: u64) {
        let search = self.update(|state| {
            let channels: Vec<Uri> = state.subscribers.keys().cloned().collect();
            for channel in channels {
                let kept = drop_subscriber(&state.subscribers[&channel], connection);
                state.subscribers.insert_mut(channel, kept);
            }
            let search = state.searches.get(&connection).cloned();
            state.searches.remove_mut(&connection);
            search
        });
        if let Some(search) = search {
            search.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    async fn http_serve(self: &Arc<Self>, id: u64, params: Value) -> JsonRpcMessage {
        let enabled = params
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if !enabled {
            self.http.stop();
            host_discovery::update_http(None);
            return rpc::success(id, serde_json::json!({}));
        }

        if params.get("bind").is_none() && params.get("webRoot").is_none() {
            if let Some(url) = self.http.url() {
                return rpc::success(id, serde_json::json!({ "url": url }));
            }
        }

        let bind = params
            .get("bind")
            .and_then(Value::as_str)
            .unwrap_or("0.0.0.0:0")
            .to_owned();

        if let Some(root) = params.get("webRoot").and_then(Value::as_str) {
            if let Ok(mut held) = self.web_root.lock() {
                *held = Some(std::path::PathBuf::from(root));
            }
        }
        let web_root = self.web_root.lock().ok().and_then(|held| held.clone());
        match crate::http::start(self, &bind, web_root).await {
            Ok(url) => {
                host_discovery::update_http(Some(url.clone()));
                rpc::success(id, serde_json::json!({ "url": url }))
            }
            Err(error) => rpc::failure(id, INTERNAL, format!("http bind failed: {error}")),
        }
    }

    async fn answer(
        self: &Arc<Self>,
        connection: u64,
        outbox: &Outbox,
        request: JsonRpcRequest,
    ) -> JsonRpcMessage {
        let id = request.id;
        let params = request.params.unwrap_or(Value::Null);
        match request.method.as_str() {
            "initialize" => match serde_json::from_value::<InitializeParams>(params) {
                Ok(params) => self.initialize(connection, outbox, id, params),
                Err(error) => rpc::failure(id, INVALID_PARAMS, error.to_string()),
            },
            "subscribe" => match serde_json::from_value::<SubscribeParams>(params) {
                Ok(params) => {
                    self.subscribe(connection, outbox, id, &params.channel)
                        .await
                }
                Err(error) => rpc::failure(id, INVALID_PARAMS, error.to_string()),
            },
            "unsubscribe" => match serde_json::from_value::<UnsubscribeParams>(params) {
                Ok(params) => {
                    self.update(|state| {
                        let empty = match state.subscribers.get(&params.channel) {
                            Some(subscribers) => {
                                let kept = drop_subscriber(subscribers, connection);
                                let empty = kept.is_empty();
                                state.subscribers.insert_mut(params.channel.clone(), kept);
                                empty
                            }
                            None => true,
                        };

                        if empty {
                            state.watches.remove_mut(&params.channel);
                            state.changesets.remove_mut(&params.channel);

                            let owners: Vec<Uri> = state
                                .sessions
                                .iter()
                                .filter(|(_, session)| {
                                    session.documents.contains_key(&params.channel)
                                })
                                .map(|(uri, _)| uri.clone())
                                .collect();
                            state.mirror_watches.remove_mut(&params.channel);
                            for uri in owners {
                                let mut session = state.sessions[&uri].clone();
                                session.documents.remove_mut(&params.channel);
                                session.disk_texts.remove_mut(&params.channel);
                                let mirrors: Vec<Uri> = session
                                    .mirrors
                                    .iter()
                                    .filter(|(_, held)| *held == &params.channel)
                                    .map(|(resource, _)| resource.clone())
                                    .collect();
                                for resource in mirrors {
                                    session.mirrors.remove_mut(&resource);
                                }
                                state.sessions.insert_mut(uri, session);
                            }
                        }
                    });
                    rpc::success(id, Value::Null)
                }
                Err(error) => rpc::failure(id, INVALID_PARAMS, error.to_string()),
            },
            "ping" => rpc::success(id, Value::Null),
            "listSessions" => match serde_json::from_value::<ListSessionsParams>(params) {
                Ok(_) => {
                    let mut cli = crate::catalog::scan(&self.config.claude_home, 20);
                    cli.extend(crate::catalog::scan_codex(&self.config.codex_home, 20));
                    let state = self.snapshot();
                    let known: std::collections::HashSet<(String, String)> = state
                        .sessions
                        .values()
                        .filter_map(|entry| {
                            let manifest = &entry.manifest;
                            let native = manifest.provider_session_id.clone().or_else(|| {
                                (manifest.provider == "claude").then(|| manifest.native_id.clone())
                            })?;
                            Some((manifest.provider.clone(), native))
                        })
                        .collect();
                    let mut items: Vec<SessionSummary> = state
                        .sessions
                        .values()
                        .filter(|entry| entry.manifest.session != host_discovery::LOCAL_FS_SESSION)
                        .filter(|entry| entry.manifest.listed)
                        .map(|entry| summary(&entry.manifest, &entry.state))
                        .collect();
                    for session in cli {
                        if known.contains(&(session.provider.clone(), session.native_id.clone())) {
                            continue;
                        }
                        items.push(cli_summary(&session));
                    }
                    items.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
                    rpc::success(
                        id,
                        ListSessionsResult {
                            items,
                            next_cursor: None,
                        },
                    )
                }
                Err(error) => rpc::failure(id, INVALID_PARAMS, error.to_string()),
            },
            "reconnect" => self.reconnect(connection, outbox, id, params),
            "openDocument" => self.open_document(id, params),
            "storeDocument" => self.store_document(id, params),
            method if method.starts_with("lsp/") => {
                self.handle_lsp(connection, id, method, params).await
            }
            "createSession" => self.create_session(id, params),
            "resolveSessionConfig" => self.resolve_session_config(id, params),
            "sessionConfigCompletions" => self.session_config_completions(id),
            "createChat" => self.create_chat(id, params),
            "disposeSession" => self.dispose_session(id, params),
            "fetchTurns" => self.fetch_turns(id, params),
            "createResourceWatch" => self.create_watch(id, params),
            "resourceRead" => {
                if std::env::var("HIHOST_TRACE").is_ok() {
                    eprintln!("[hihost] resourceRead {}", params["uri"]);
                }
                self.resource_read(id, params)
            }
            "resourceWrite" => self.resource_write(id, params),
            "resourceList" => {
                if std::env::var("HIHOST_TRACE").is_ok() {
                    eprintln!("[hihost] resourceList {}", params["uri"]);
                }
                self.resource_list(id, params)
            }
            "search" => self.search(connection, id, params).await,
            "httpServe" => self.http_serve(id, params).await,
            "createTerminal" => self.create_terminal(id, params),
            "disposeTerminal" => self.dispose_terminal(id, params),
            other => rpc::failure(id, METHOD_NOT_FOUND, format!("Method not found: {other}")),
        }
    }

    fn initialize(
        self: &Arc<Self>,
        connection: u64,
        outbox: &Outbox,
        id: u64,
        params: InitializeParams,
    ) -> JsonRpcMessage {
        if !params
            .protocol_versions
            .iter()
            .any(|version| version == crate::PROTOCOL_VERSION)
        {
            return rpc::failure(
                id,
                VERSION_MISMATCH,
                format!(
                    "Client offered protocol versions {:?}, none of which are compatible with \
                     this server's version {} (server accepts ^{}).",
                    params.protocol_versions,
                    crate::PROTOCOL_VERSION,
                    crate::PROTOCOL_VERSION
                ),
            );
        }
        let mut snapshots = Vec::new();
        for channel in params.initial_subscriptions.into_iter().flatten() {
            self.materialize_cli_session(&channel);
            if let Some(snapshot) = self.subscribe_channel(connection, outbox, &channel) {
                snapshots.push(snapshot);
            }
        }
        let state = self.snapshot();
        rpc::success(
            id,
            InitializeResult {
                protocol_version: crate::PROTOCOL_VERSION.to_owned(),
                server_seq: state.server_seq,
                server_info: Some(Implementation {
                    name: crate::SERVER_NAME.to_owned(),
                    version: Some(crate::SERVER_VERSION.to_owned()),
                    title: None,
                }),
                snapshots,
                default_directory: None,
                completion_trigger_characters: None,
                terminal_command_prefix: None,
                telemetry: None,
            },
        )
    }

    fn reconnect(
        self: &Arc<Self>,
        connection: u64,
        outbox: &Outbox,
        id: u64,
        params: Value,
    ) -> JsonRpcMessage {
        let last_seen = params["lastSeenServerSeq"].as_i64().unwrap_or(0);
        let channels: Vec<Uri> = params["subscriptions"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        for channel in &channels {
            self.materialize_cli_session(channel);
        }
        let covered = {
            let state = self.snapshot();
            state
                .replay
                .peek()
                .map(|oldest| oldest.server_seq as i64 <= last_seen + 1)
                .unwrap_or(state.server_seq <= last_seen)
        };
        if covered {
            let mut missing = Vec::new();
            let mut actions = Vec::new();
            self.update(|state| {
                for channel in &channels {
                    let known = channel == ROOT
                        || state.sessions.contains_key(channel)
                        || state.chats.contains_key(channel)
                        || state.terminals.contains_key(channel)
                        || state.changesets.contains_key(channel)
                        || state.lsp_diagnostics.contains_key(channel)
                        || state.watches.contains_key(channel)
                        || annotations_session(channel)
                            .is_some_and(|session| state.sessions.contains_key(&session));
                    if !known {
                        missing.push(channel.clone());
                        continue;
                    }
                    subscribe_outbox(state, channel, connection, outbox);
                }
                actions.extend(
                    state
                        .replay
                        .iter()
                        .filter(|envelope| {
                            envelope.server_seq as i64 > last_seen
                                && channels.contains(&envelope.channel)
                        })
                        .cloned(),
                );
            });
            return rpc::success(
                id,
                ahp_types::commands::ReconnectResult::Replay(
                    ahp_types::commands::ReconnectReplayResult { actions, missing },
                ),
            );
        }

        let mut snapshots = Vec::new();
        for channel in &channels {
            if let Some(snapshot) = self.subscribe_channel(connection, outbox, channel) {
                snapshots.push(snapshot);
            }
        }
        rpc::success(
            id,
            ahp_types::commands::ReconnectResult::Snapshot(
                ahp_types::commands::ReconnectSnapshotResult { snapshots },
            ),
        )
    }

    async fn subscribe(
        self: &Arc<Self>,
        connection: u64,
        outbox: &Outbox,
        id: u64,
        channel: &Uri,
    ) -> JsonRpcMessage {
        self.materialize_cli_session(channel);
        self.ensure_changeset(channel);

        if crate::history::channel_folder(channel).is_some() {
            return self.subscribe_history(connection, outbox, id, channel);
        }

        if channel.starts_with(crate::documents::CHANNEL_PREFIX) {
            return self.subscribe_document(connection, outbox, id, channel);
        }
        if channel.starts_with(LSP_DIAGNOSTICS_PREFIX) {
            return self.subscribe_diagnostics(connection, outbox, id, channel);
        }
        let answered = self.subscribe_channel(connection, outbox, channel);

        if answered.is_some()
            && (crate::changes::channel_folder(channel).is_some()
                || crate::changes::channel_commit(channel).is_some())
        {
            self.recompute_changes(channel);
        }
        match answered {
            Some(snapshot) => rpc::success(
                id,
                SubscribeResult {
                    snapshot: Some(snapshot),
                },
            ),
            None => rpc::failure(id, NO_SUCH_CHANNEL, format!("no channel {channel}")),
        }
    }

    fn subscribe_channel(
        &self,
        connection: u64,
        outbox: &Outbox,
        channel: &Uri,
    ) -> Option<Snapshot> {
        self.update(|state| {
            let snapshot_state = if channel == ROOT {
                SnapshotState::Root(Box::new(state.root.clone()))
            } else if let Some(entry) = state.sessions.get(channel) {
                SnapshotState::Session(Box::new(entry.state.clone()))
            } else if let Some(entry) = state.chats.get(channel) {
                SnapshotState::Chat(Box::new(tail_window(&entry.state)))
            } else if let Some(entry) = state.terminals.get(channel) {
                SnapshotState::Terminal(Box::new(entry.state.clone()))
            } else if let Some(entry) = state.changesets.get(channel) {
                SnapshotState::Changeset(Box::new(entry.state.clone()))
            } else if let Some(entry) = state.watches.get(channel) {
                SnapshotState::ResourceWatch(Box::new(ahp_types::state::ResourceWatchState {
                    root: entry.root.clone(),
                    recursive: false,
                    excludes: None,
                    includes: None,
                }))
            } else if let Some(entry) =
                annotations_session(channel).and_then(|session| state.sessions.get(&session))
            {
                SnapshotState::Annotations(Box::new(entry.annotations.clone()))
            } else {
                return None;
            };
            let from_seq = state.server_seq;
            subscribe_outbox(state, channel, connection, outbox);
            Some(Snapshot {
                resource: channel.clone(),
                state: snapshot_state,
                from_seq,
            })
        })
    }

    fn materialize_cli_session(self: &Arc<Self>, channel: &Uri) {
        let session_uri = annotations_session(channel).unwrap_or_else(|| channel.clone());
        let channel = &session_uri;
        let storage_id = match channel.strip_prefix("ahp-session:/") {
            Some(storage_id) => storage_id.to_owned(),
            None => return,
        };
        {
            let state = self.snapshot();
            if state.sessions.contains_key(channel) {
                return;
            }
        }
        let mut cli = crate::catalog::scan(&self.config.claude_home, 100);
        cli.extend(crate::catalog::scan_codex(&self.config.codex_home, 100));
        let Some(session) = cli.into_iter().find(|held| held.storage_id == storage_id) else {
            return;
        };
        let default_chat = format!("ahp-chat:/{storage_id}");
        let manifest = Manifest {
            session: channel.clone(),
            native_id: storage_id.clone(),
            provider_session_id: Some(session.native_id.clone()),
            provider: session.provider.clone(),
            working_directories: session
                .cwd
                .iter()
                .map(|cwd| crate::uris::file_uri(std::path::Path::new(&cwd)))
                .collect(),

            primary: session
                .cwd
                .iter()
                .map(|cwd| crate::uris::file_uri(std::path::Path::new(&cwd)))
                .next(),
            default_chat: default_chat.clone(),
            title: session.title.clone(),
            created_at: now_rfc3339(),
            annotations: Vec::new(),
            model: None,
            thinking_level: None,
            permission_mode: None,
            worktree: false,
            listed: true,
        };
        let mut chat_state = empty_chat(&default_chat, &session.title);
        for action in crate::catalog::session_backfill_actions(&session) {
            let _ = ahp::reducers::apply_action_to_chat(&mut chat_state, &action);
        }
        let _ = self.store.write_manifest(&manifest);
        self.update(|state| {
            state.chats.insert_mut(
                default_chat,
                ChatEntry {
                    state: chat_state,
                    session: channel.clone(),
                    native_id: storage_id,
                    provider_session_id: manifest.provider_session_id.clone(),
                    agent: None,
                    spoken: true,
                    retire_after_turn: false,
                },
            );
            state.sessions.insert_mut(
                channel.clone(),
                SessionEntry {
                    state: session_state(&manifest),
                    manifest,
                    documents: rpds::HashTrieMapSync::new_sync(),
                    mirrors: rpds::HashTrieMapSync::new_sync(),
                    disk_texts: rpds::HashTrieMapSync::new_sync(),
                    annotations: AnnotationsState {
                        annotations: Vec::new(),
                    },
                },
            );
            state.root.active_sessions = Some(state.sessions.size() as i64);
        });
    }

    fn fetch_turns(&self, id: u64, params: Value) -> JsonRpcMessage {
        let channel: Uri = params["channel"].as_str().unwrap_or_default().to_owned();
        let cursor = params["cursor"]
            .as_str()
            .and_then(|cursor| cursor.parse::<usize>().ok());
        let page = {
            let state = self.snapshot();
            let Some(entry) = state.chats.get(&channel) else {
                return rpc::failure(id, NO_SUCH_CHANNEL, format!("no chat {channel}"));
            };
            let turns = &entry.state.turns;

            let end = cursor.unwrap_or(0).min(turns.len());
            let start = end.saturating_sub(TURN_PAGE);
            ahp_types::actions::ChatTurnsLoadedAction {
                turns: turns[start..end].to_vec(),
                turns_next_cursor: (start > 0).then(|| start.to_string()),
            }
        };
        self.broadcast_unfolded(&channel, StateAction::ChatTurnsLoaded(page));
        rpc::success(id, serde_json::json!({}))
    }

    fn apply(&self, channel: &Uri, action: StateAction) {
        self.update(|state| {
            state.server_seq += 1;
            let server_seq = state.server_seq as u64;
            if channel == ROOT {
                let mut root = state.root.clone();
                let _ = ahp::reducers::apply_action_to_root(&mut root, &action);
                state.root = root;
            } else if let Some(entry) = state.sessions.get(channel) {
                let mut entry = entry.clone();
                let _ = ahp::reducers::apply_action_to_session(&mut entry.state, &action);
                state.sessions.insert_mut(channel.clone(), entry);
            } else if let Some(entry) = state.chats.get(channel) {
                let mut entry = entry.clone();
                let _ = ahp::reducers::apply_action_to_chat(&mut entry.state, &action);
                self.store.append(&entry.native_id, channel, &action);
                state.chats.insert_mut(channel.clone(), entry);
            } else if let Some(entry) = state.terminals.get(channel) {
                let mut entry = entry.clone();
                let _ = ahp::reducers::apply_action_to_terminal(&mut entry.state, &action);
                trim_scrollback(&mut entry.state);
                state.terminals.insert_mut(channel.clone(), entry);
            } else if let Some(entry) = state.changesets.get(channel) {
                let mut entry = entry.clone();
                let _ = ahp::reducers::apply_action_to_changeset(&mut entry.state, &action);
                state.changesets.insert_mut(channel.clone(), entry);
            } else if let Some((session, entry)) =
                annotations_session(channel).and_then(|session| {
                    state
                        .sessions
                        .get(&session)
                        .cloned()
                        .map(|entry| (session, entry))
                })
            {
                let mut entry = entry;
                let _ = ahp::reducers::apply_action_to_annotations(&mut entry.annotations, &action);

                entry.manifest.annotations = entry.annotations.annotations.clone();
                let _ = self.store.write_manifest(&entry.manifest);
                state.sessions.insert_mut(session, entry);
            }
            let envelope = ahp_types::actions::ActionEnvelope {
                channel: channel.clone(),
                action,
                server_seq,
                origin: None,
                rejection_reason: None,
            };
            let line = rpc::line(&rpc::notification("action", &envelope));
            if let Some(subscribers) = state.subscribers.get(channel) {
                for (_, outbox) in subscribers.iter() {
                    let _ = outbox.send(line.clone());
                }
            }
            state.replay.enqueue_mut(envelope);
            if state.replay.len() > REPLAY_DEPTH {
                state.replay.dequeue_mut();
            }
        });
    }

    fn broadcast_unfolded(&self, channel: &Uri, action: StateAction) {
        self.update(|state| {
            state.server_seq += 1;
            let server_seq = state.server_seq as u64;
            let envelope = ahp_types::actions::ActionEnvelope {
                channel: channel.clone(),
                action,
                server_seq,
                origin: None,
                rejection_reason: None,
            };
            let line = rpc::line(&rpc::notification("action", &envelope));
            if let Some(subscribers) = state.subscribers.get(channel) {
                for (_, outbox) in subscribers.iter() {
                    let _ = outbox.send(line.clone());
                }
            }
        });
    }

    fn notify_root(&self, method: &str, params: impl serde::Serialize) {
        let state = self.snapshot();
        let line = rpc::line(&rpc::notification(method, params));
        if let Some(subscribers) = state.subscribers.get(ROOT) {
            for (_, outbox) in subscribers {
                let _ = outbox.send(line.clone());
            }
        }
    }

    fn resolve_session_config(&self, id: u64, params: Value) -> JsonRpcMessage {
        let overlay = params["config"].as_object().cloned();
        let result = ahp_types::commands::ResolveSessionConfigResult {
            schema: session_config_schema(),
            values: session_config_values(overlay.as_ref()),
        };
        match serde_json::to_value(result) {
            Ok(value) => rpc::success(id, value),
            Err(error) => rpc::failure(id, INTERNAL, error.to_string()),
        }
    }

    fn session_config_completions(&self, id: u64) -> JsonRpcMessage {
        let result = ahp_types::commands::SessionConfigCompletionsResult { items: Vec::new() };
        match serde_json::to_value(result) {
            Ok(value) => rpc::success(id, value),
            Err(error) => rpc::failure(id, INTERNAL, error.to_string()),
        }
    }

    fn create_session(&self, id: u64, params: Value) -> JsonRpcMessage {
        let requested = params["channel"].as_str().unwrap_or_default().to_owned();
        if requested.is_empty() {
            return rpc::failure(id, INVALID_PARAMS, "no channel");
        }
        let provider = params["provider"].as_str().unwrap_or("claude").to_owned();
        if !matches!(provider.as_str(), "claude" | "codex") {
            return rpc::failure(
                id,
                INVALID_PARAMS,
                format!("unsupported agent provider: {provider}"),
            );
        }
        let working_directories: Vec<Uri> = params["workingDirectories"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let native_id = crate::uuid_v4();
        let default_chat = format!("ahp-chat:/{}", crate::uuid_v4());

        let primary = Some(working_directories.first().cloned().unwrap_or_default());
        let manifest = Manifest {
            session: requested.clone(),
            native_id: native_id.clone(),
            provider_session_id: (provider == "claude").then(|| native_id.clone()),
            provider,
            working_directories,
            primary,
            default_chat: default_chat.clone(),
            title: "New Session".to_owned(),
            created_at: now_rfc3339(),
            annotations: Vec::new(),
            model: params["model"]["id"].as_str().map(str::to_owned),
            thinking_level: params["model"]["config"]["thinkingLevel"]
                .as_str()
                .map(str::to_owned),
            permission_mode: params["config"]["permissionMode"]
                .as_str()
                .map(str::to_owned),
            worktree: params["config"]["worktree"].as_bool().unwrap_or(false),

            listed: !params["config"]["unlisted"].as_bool().unwrap_or(false),
        };
        if let Err(error) = self.store.write_manifest(&manifest) {
            return rpc::failure(id, INTERNAL, error.to_string());
        }
        let exists = self.update(|state| {
            if state.sessions.contains_key(&requested) {
                return true;
            }
            state.chats.insert_mut(
                default_chat.clone(),
                ChatEntry {
                    state: empty_chat(&manifest.default_chat.clone(), &manifest.title),
                    session: requested.clone(),
                    native_id: native_id.clone(),
                    provider_session_id: manifest.provider_session_id.clone(),
                    agent: None,
                    spoken: false,
                    retire_after_turn: false,
                },
            );
            state.sessions.insert_mut(
                requested.clone(),
                SessionEntry {
                    state: session_state(&manifest),
                    manifest: manifest.clone(),
                    documents: rpds::HashTrieMapSync::new_sync(),
                    mirrors: rpds::HashTrieMapSync::new_sync(),
                    disk_texts: rpds::HashTrieMapSync::new_sync(),
                    annotations: AnnotationsState {
                        annotations: Vec::new(),
                    },
                },
            );
            state.root.active_sessions = Some(state.sessions.size() as i64);
            false
        });
        if exists {
            return rpc::failure(id, -32003, format!("session exists: {requested}"));
        }

        if manifest.listed {
            self.notify_root(
                "root/sessionAdded",
                serde_json::json!({
                    "channel": ROOT,
                    "summary": summary_of(&manifest),
                }),
            );
        }
        rpc::success(id, Value::Null)
    }

    fn create_chat(&self, id: u64, params: Value) -> JsonRpcMessage {
        let session: Uri = params["channel"].as_str().unwrap_or_default().to_owned();
        let chat: Uri = params["chat"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("ahp-chat:/{}", crate::uuid_v4()));
        let summary = self.update(|state| {
            let entry = state.sessions.get(&session)?;
            let native_id = crate::uuid_v4();
            let provider_session_id =
                (entry.manifest.provider == "claude").then(|| native_id.clone());
            let summary = ChatSummary {
                resource: chat.clone(),
                title: entry.manifest.title.clone(),
                status: 1,
                activity: None,
                modified_at: now_rfc3339(),
                origin: Some(ChatOrigin::User),
                interactivity: None,
                working_directories: None,
            };
            state.chats.insert_mut(
                chat.clone(),
                ChatEntry {
                    state: empty_chat(&chat, "New Chat"),
                    session: session.clone(),
                    native_id,
                    provider_session_id,
                    agent: None,
                    spoken: false,
                    retire_after_turn: false,
                },
            );
            Some(summary)
        });
        let Some(summary) = summary else {
            return rpc::failure(id, NO_SUCH_CHANNEL, format!("no session {session}"));
        };
        self.apply(
            &session,
            StateAction::SessionChatAdded(ahp_types::actions::SessionChatAddedAction { summary }),
        );
        rpc::success(id, Value::Null)
    }

    fn dispose_session(&self, id: u64, params: Value) -> JsonRpcMessage {
        let session: Uri = params["channel"].as_str().unwrap_or_default().to_owned();
        let removed = self.update(|state| {
            let entry = state.sessions.get(&session).cloned()?;
            state.sessions.remove_mut(&session);
            let chats: Vec<(Uri, ChatEntry)> = state
                .chats
                .iter()
                .filter(|(_, chat)| chat.session == session)
                .map(|(uri, chat)| (uri.clone(), chat.clone()))
                .collect();
            for (uri, chat) in chats {
                state.chats.remove_mut(&uri);
                if let Some(agent) = chat.agent {
                    agent.shutdown();
                }
            }
            state.root.active_sessions = Some(state.sessions.size() as i64);
            Some(entry)
        });
        let Some(removed) = removed else {
            return rpc::failure(id, NO_SUCH_CHANNEL, format!("no session {session}"));
        };
        let _ = self.store.remove_session(&removed.manifest.native_id);
        self.notify_root(
            "root/sessionRemoved",
            serde_json::json!({"channel": ROOT, "session": session}),
        );
        rpc::success(id, Value::Null)
    }

    async fn dispatch(self: &Arc<Self>, params: Value) {
        let trace = std::env::var("HIHOST_TRACE").is_ok();
        let envelope = match serde_json::from_value::<DispatchEnvelope>(params) {
            Ok(envelope) => envelope,
            Err(error) => {
                if trace {
                    eprintln!("[hihost] dispatch parse failed: {error}");
                }
                return;
            }
        };
        if trace {
            eprintln!(
                "[hihost] dispatch {:?} on {}",
                std::mem::discriminant(&envelope.action),
                envelope.channel
            );
        }
        let channel = envelope.channel;

        if channel.starts_with(crate::documents::CHANNEL_PREFIX) {
            if let StateAction::Unknown(value) = envelope.action {
                self.document_dispatch(&channel, value);
            }
            return;
        }

        if annotations_session(&channel).is_some() {
            self.annotations_dispatch(&channel, envelope.action);
            return;
        }

        if crate::history::channel_folder(&channel).is_some() {
            if let StateAction::Unknown(value) = envelope.action {
                self.history_dispatch(&channel, value);
            }
            return;
        }
        match &envelope.action {
            StateAction::ChatTurnStarted(started) => {
                self.honor_message_model(&channel, &started.message);
                self.apply(&channel, envelope.action.clone());
                self.retitle_on_first_prompt(&channel, started);
                self.list_on_first_turn(&channel);
                let text = self.expanded_prompt(started);
                self.prompt(&channel, started.turn_id.clone(), text).await;
            }
            StateAction::ChatPendingMessageSet(_) | StateAction::ChatPendingMessageRemoved(_) => {
                self.apply(&channel, envelope.action);
            }
            StateAction::ChatToolCallConfirmed(confirmed) => {
                let approved = confirmed.approved;
                let tool = confirmed.tool_call_id.clone();
                self.apply(&channel, envelope.action);
                let agent = self.agent_of(&channel);
                if let Some(agent) = agent {
                    let _ = agent.answer(&tool, approved).await;
                }
            }
            StateAction::ChatTurnCancelled(_) => {
                self.apply(&channel, envelope.action);
                if let Some(agent) = self.agent_of(&channel) {
                    let _ = agent.interrupt().await;
                }
            }

            StateAction::SessionWorkingDirectorySet(set) => {
                if crate::uris::file_path(&set.directory).is_none() {
                    return;
                }
                let directory = set.directory.clone();
                self.apply(&channel, envelope.action);
                self.mutate_session(&channel, |manifest| {
                    if !manifest.working_directories.contains(&directory) {
                        manifest.working_directories.push(directory);
                    }
                });
                self.republish_session_catalog(&channel);
            }
            StateAction::SessionWorkingDirectoryRemoved(removed) => {
                let directory = removed.directory.clone();
                self.apply(&channel, envelope.action);
                self.mutate_session(&channel, |manifest| {
                    manifest
                        .working_directories
                        .retain(|held| *held != directory);
                });
                self.republish_session_catalog(&channel);
            }

            StateAction::SessionConfigChanged(changed) => {
                let mode = string_of(&changed.config, "permissionMode");
                let model = string_of(&changed.config, "model");
                let thinking = string_of(&changed.config, "thinkingLevel");
                self.apply(&channel, envelope.action);
                if mode.is_some() || model.is_some() || thinking.is_some() {
                    self.mutate_session(&channel, |manifest| {
                        if let Some(mode) = mode {
                            manifest.permission_mode = Some(mode);
                        }
                        if let Some(model) = model {
                            manifest.model = Some(model);
                        }
                        if let Some(thinking) = thinking {
                            manifest.thinking_level = Some(thinking);
                        }
                    });
                }
            }

            StateAction::TerminalInput(input) => {
                self.terminal_write(&channel, input.data.clone());
            }
            StateAction::TerminalResized(resized) => {
                let (cols, rows) = (resized.cols, resized.rows);
                self.apply(&channel, envelope.action);
                self.terminal_winsize(&channel, cols as u16, rows as u16);
            }

            _ => self.apply(&channel, envelope.action),
        }
    }

    fn annotations_dispatch(&self, channel: &Uri, action: StateAction) {
        let valid = {
            let state = self.snapshot();
            let Some(entry) =
                annotations_session(channel).and_then(|session| state.sessions.get(&session))
            else {
                return;
            };
            match &action {
                StateAction::AnnotationsSet(set) => !set.annotation.entries.is_empty(),
                StateAction::AnnotationsEntryRemoved(removed) => entry
                    .annotations
                    .annotations
                    .iter()
                    .find(|held| held.id == removed.annotation_id)
                    .map_or(true, |held| {
                        held.entries.len() > 1
                            || !held.entries.iter().any(|held| held.id == removed.entry_id)
                    }),
                StateAction::AnnotationsUpdated(_)
                | StateAction::AnnotationsRemoved(_)
                | StateAction::AnnotationsEntrySet(_) => true,
                _ => false,
            }
        };
        if !valid {
            if std::env::var("HIHOST_TRACE").is_ok() {
                eprintln!("[hihost] annotations dispatch rejected on {channel}");
            }
            return;
        }
        self.apply(channel, action);
        self.publish_annotations_summary(channel);
    }

    fn publish_annotations_summary(&self, channel: &Uri) {
        let Some(session) = annotations_session(channel) else {
            return;
        };
        let summary = self.update(|state| {
            let mut entry = state.sessions.get(&session)?.clone();
            let summary = annotations_summary(&session, &entry.annotations.annotations);
            entry.state.annotations = Some(summary.clone());
            state.sessions.insert_mut(session.clone(), entry);
            Some(summary)
        });
        let Some(summary) = summary else {
            return;
        };
        self.notify_root(
            "root/sessionSummaryChanged",
            serde_json::json!({
                "channel": ROOT,
                "session": session,
                "changes": {"annotations": summary},
            }),
        );
    }

    fn agent_of(&self, chat: &Uri) -> Option<LiveAgent> {
        let state = self.snapshot();
        state.chats.get(chat).and_then(|entry| entry.agent.clone())
    }

    async fn prompt(self: &Arc<Self>, chat: &Uri, turn_id: String, text: String) {
        let trace = std::env::var("HIHOST_TRACE").is_ok();
        if trace {
            eprintln!("[hihost] prompt {turn_id} on {chat}");
        }
        let agent = match self.ensure_agent(chat).await {
            Ok(agent) => agent,
            Err(error) => {
                if trace {
                    eprintln!("[hihost] ensure_agent failed: {error}");
                }
                self.turn_failed(chat, &turn_id, &error);
                return;
            }
        };
        if trace {
            eprintln!("[hihost] agent up, sending prompt");
        }
        if let Err(error) = agent.prompt(turn_id.clone(), text).await {
            self.turn_failed(chat, &turn_id, &error);
        }
    }

    async fn ensure_agent(self: &Arc<Self>, chat: &Uri) -> Result<LiveAgent, String> {
        let (
            provider,
            storage_id,
            provider_session_id,
            spoken,
            cwd,
            add_dirs,
            model,
            permission_mode,
            thinking_level,
        ) = {
            let state = self.snapshot();
            let entry = state
                .chats
                .get(chat)
                .ok_or_else(|| format!("no chat {chat}"))?;
            if let Some(agent) = &entry.agent {
                if !agent.is_dead() {
                    return Ok(agent.clone());
                }
            }
            let session = state
                .sessions
                .get(&entry.session)
                .ok_or_else(|| format!("no session {}", entry.session))?;

            let primary = session
                .manifest
                .primary
                .clone()
                .or_else(|| session.manifest.working_directories.first().cloned())
                .filter(|uri| !uri.is_empty());
            let cwd = primary
                .as_ref()
                .map(|uri| local_path(uri))
                .unwrap_or_else(std::env::temp_dir);
            let add_dirs: Vec<std::path::PathBuf> = session
                .manifest
                .working_directories
                .iter()
                .filter(|uri| Some(uri.as_str()) != primary.as_deref())
                .map(|uri| local_path(uri))
                .collect();
            (
                session.manifest.provider.clone(),
                entry.native_id.clone(),
                entry.provider_session_id.clone(),
                entry.spoken,
                cwd,
                add_dirs,
                session.manifest.model.clone(),
                session.manifest.permission_mode.clone(),
                session.manifest.thinking_level.clone(),
            )
        };
        let sink: crate::claude::Sink = Arc::new(ChatSink {
            host: Arc::clone(self),
            channel: chat.clone(),
        });
        let (agent, actual_provider_id) = match provider.as_str() {
            "claude" => {
                let native_id = provider_session_id.unwrap_or(storage_id);
                let agent = crate::claude::ClaudeAgent::spawn(
                    crate::claude::SpawnConfig {
                        binary: self.config.claude_binary.clone(),
                        cwd,
                        add_dirs,
                        native_id: native_id.clone(),
                        resume: spoken,
                        model,
                        permission_mode,
                        thinking_level,
                    },
                    sink,
                )
                .await?;
                (LiveAgent::Claude(agent), native_id)
            }
            "codex" => {
                let agent = crate::codex::CodexAgent::spawn(
                    crate::codex::SpawnConfig {
                        binary: self.config.codex_binary.clone(),
                        cwd,
                        add_dirs,
                        native_id: provider_session_id,
                        model,
                        permission_mode,
                        thinking_level,
                    },
                    sink,
                )
                .await?;
                let native_id = agent.native_id().to_owned();
                (LiveAgent::Codex(agent), native_id)
            }
            other => return Err(format!("unsupported agent provider: {other}")),
        };
        self.update(|state| {
            let mut entry = state
                .chats
                .get(chat)
                .cloned()
                .ok_or_else(|| format!("no chat {chat}"))?;
            entry.agent = Some(agent.clone());
            entry.provider_session_id = Some(actual_provider_id.clone());
            entry.spoken = true;
            let session_uri = entry.session.clone();
            state.chats.insert_mut(chat.clone(), entry);
            if let Some(session) = state.sessions.get(&session_uri) {
                if session.manifest.default_chat == *chat {
                    let mut session = session.clone();
                    session.manifest.provider_session_id = Some(actual_provider_id);
                    let _ = self.store.write_manifest(&session.manifest);
                    state.sessions.insert_mut(session_uri, session);
                }
            }
            Ok::<(), String>(())
        })?;
        Ok(agent)
    }

    fn mutate_session(&self, session: &Uri, mutate: impl FnOnce(&mut Manifest)) {
        self.update(|state| {
            let Some(entry) = state.sessions.get(session) else {
                return;
            };
            let mut entry = entry.clone();
            mutate(&mut entry.manifest);
            let _ = self.store.write_manifest(&entry.manifest);
            state.sessions.insert_mut(session.clone(), entry);
            let chats: Vec<Uri> = state
                .chats
                .iter()
                .filter(|(_, chat)| chat.session == *session)
                .map(|(uri, _)| uri.clone())
                .collect();
            for chat in chats {
                Self::retire_locked(state, &chat);
            }
        });
    }

    fn honor_message_model(&self, chat: &Uri, message: &ahp_types::state::Message) {
        let Some(selection) = &message.model else {
            return;
        };

        if selection.id.starts_with('@') {
            return;
        }
        let thinking = selection
            .config
            .as_ref()
            .and_then(|config| config.get("thinkingLevel"))
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        let (session, changed) = {
            let state = self.snapshot();
            let Some(entry) = state.chats.get(chat) else {
                return;
            };
            let Some(held) = state.sessions.get(&entry.session) else {
                return;
            };
            let manifest = &held.manifest;
            let changed = manifest.model.as_deref() != Some(&selection.id)
                || (thinking.is_some() && manifest.thinking_level != thinking);
            (entry.session.clone(), changed)
        };
        if !changed {
            return;
        }
        let model = selection.id.clone();
        self.mutate_session(&session, |manifest| {
            manifest.model = Some(model);
            if thinking.is_some() {
                manifest.thinking_level = thinking;
            }
        });
    }

    fn retire_locked(state: &mut State, chat: &Uri) {
        let Some(entry) = state.chats.get(chat) else {
            return;
        };
        let mut entry = entry.clone();
        let Some(agent) = &entry.agent else { return };
        if agent.idle() {
            agent.shutdown();
            entry.agent = None;
            entry.retire_after_turn = false;
        } else {
            entry.retire_after_turn = true;
        }
        state.chats.insert_mut(chat.clone(), entry);
    }

    fn agent_action(self: &Arc<Self>, chat: &Uri, action: StateAction) {
        let terminal = matches!(
            action,
            StateAction::ChatTurnComplete(_)
                | StateAction::ChatTurnCancelled(_)
                | StateAction::ChatError(_)
        );
        let natural = matches!(action, StateAction::ChatTurnComplete(_));
        self.apply(chat, action);
        if terminal {
            self.update(|state| {
                if state
                    .chats
                    .get(chat)
                    .is_some_and(|entry| entry.retire_after_turn)
                {
                    Self::retire_locked(state, chat);
                }
            });
        }
        if terminal && natural {
            self.drain_queue(chat);
        }
    }

    fn drain_queue(self: &Arc<Self>, chat: &Uri) {
        let next = {
            let state = self.snapshot();
            state.chats.get(chat).and_then(|entry| {
                entry
                    .state
                    .queued_messages
                    .as_ref()
                    .and_then(|queue| queue.first().cloned())
            })
        };
        let Some(queued) = next else { return };
        self.apply(
            chat,
            StateAction::ChatPendingMessageRemoved(ChatPendingMessageRemovedAction {
                kind: PendingMessageKind::Queued,
                id: queued.id.clone(),
            }),
        );
        let turn_id = format!("hihost-{}", crate::uuid_v4());
        let started = ChatTurnStartedAction {
            turn_id: turn_id.clone(),
            started_at: now_rfc3339(),
            message: queued.message.clone(),
            queued_message_id: Some(queued.id),
            meta: None,
        };
        let text = queued.message.text.clone();
        self.honor_message_model(chat, &queued.message);
        self.apply(chat, StateAction::ChatTurnStarted(started));
        let host = Arc::clone(&self);
        let channel = chat.clone();
        tokio::spawn(async move {
            host.prompt(&channel, turn_id, text).await;
        });
    }

    fn retitle_on_first_prompt(&self, chat: &Uri, started: &ChatTurnStartedAction) {
        let retitled = self.update(|state| {
            let entry = state.chats.get(chat)?;
            let session_uri = entry.session.clone();
            let mut session = state.sessions.get(&session_uri)?.clone();
            if session.manifest.title != "New Session" {
                return None;
            }
            let mut title = started.message.text.trim().replace('\n', " ");
            if title.len() > 64 {
                title.truncate(64);
            }
            if title.is_empty() {
                return None;
            }
            session.manifest.title = title.clone();
            session.state.title = title.clone();
            let _ = self.store.write_manifest(&session.manifest);
            state.sessions.insert_mut(session_uri.clone(), session);
            Some((session_uri, title))
        });
        let Some((session, fresh_title)) = retitled else {
            return;
        };
        self.notify_root(
            "root/sessionSummaryChanged",
            serde_json::json!({
                "channel": ROOT,
                "session": session,
                "changes": PartialSessionSummary {
                    title: Some(fresh_title),
                    modified_at: Some(now_rfc3339()),
                    ..Default::default()
                },
            }),
        );
    }

    fn list_on_first_turn(&self, chat: &Uri) {
        let flipped = self.update(|state| {
            let entry = state.chats.get(chat)?;
            let session_uri = entry.session.clone();
            let mut session = state.sessions.get(&session_uri)?.clone();
            if session.manifest.listed {
                return None;
            }
            session.manifest.listed = true;
            let _ = self.store.write_manifest(&session.manifest);
            let summary = summary_of(&session.manifest);
            state.sessions.insert_mut(session_uri, session);
            Some(summary)
        });
        if let Some(summary) = flipped {
            self.notify_root(
                "root/sessionAdded",
                serde_json::json!({
                    "channel": ROOT,
                    "summary": summary,
                }),
            );
        }
    }

    fn turn_failed(&self, chat: &Uri, turn_id: &str, error: &str) {
        self.apply(
            chat,
            StateAction::ChatError(ahp_types::actions::ChatErrorAction {
                turn_id: turn_id.to_owned(),
                error: ahp_types::state::ErrorInfo {
                    error_type: "sendFailed".to_owned(),
                    message: error.to_owned(),
                    stack: None,
                    meta: None,
                },
                duration: 0,
                meta: None,
            }),
        );
    }

    fn create_watch(self: &Arc<Self>, id: u64, params: Value) -> JsonRpcMessage {
        let Some(path) = file_path(&params["uri"]) else {
            return rpc::failure(id, INVALID_PARAMS, "not a file uri");
        };
        let uri = params["uri"].as_str().unwrap_or_default().to_owned();
        let channel = format!("ahp-resource-watch:/{}", crate::uuid_v4());
        let host = std::sync::Arc::downgrade(self);
        let fan_out = channel.clone();
        let watcher = notify::PollWatcher::new(
            move |outcome: notify::Result<notify::Event>| {
                let Ok(event) = outcome else { return };
                let Some(host) = host.upgrade() else { return };
                let items: Vec<Value> = event
                    .paths
                    .iter()
                    .map(|path| {
                        serde_json::json!({
                            "uri": crate::uris::file_uri(&path),
                            "type": "updated",
                        })
                    })
                    .collect();
                host.apply(
                    &fan_out,
                    StateAction::ResourceWatchChanged(
                        ahp_types::actions::ResourceWatchChangedAction {
                            changes: serde_json::json!({ "items": items }),
                        },
                    ),
                );
            },
            notify::Config::default().with_poll_interval(std::time::Duration::from_millis(400)),
        );
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => return rpc::failure(id, INTERNAL, error.to_string()),
        };
        if let Err(error) =
            notify::Watcher::watch(&mut watcher, &path, notify::RecursiveMode::NonRecursive)
        {
            return rpc::failure(id, -32002, error.to_string());
        }
        self.update(|state| {
            state.watches.insert_mut(
                channel.clone(),
                WatchEntry {
                    _watcher: Arc::new(watcher),
                    root: uri,
                },
            );
        });
        rpc::success(id, serde_json::json!({ "channel": channel }))
    }

    async fn search(&self, connection: u64, id: u64, params: Value) -> JsonRpcMessage {
        const DEFAULT_LIMIT: usize = 128;
        const LIMIT_CAP: usize = 1024;
        let params: himark_ahp_ext_types::SearchParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => return rpc::failure(id, INVALID_PARAMS, error.to_string()),
        };
        let roots: Vec<PathBuf> = if params.channel == host_discovery::LOCAL_FS_SESSION {
            vec![PathBuf::from("/")]
        } else {
            let state = self.snapshot();
            let Some(entry) = state.sessions.get(&params.channel) else {
                return rpc::failure(
                    id,
                    NO_SUCH_CHANNEL,
                    format!("no session {}", params.channel),
                );
            };
            entry
                .manifest
                .working_directories
                .iter()
                .map(local_path)
                .collect()
        };
        let folders: Vec<PathBuf> = match &params.folders {
            Some(entries) => {
                let mut folders = Vec::new();
                for entry in entries {
                    let Some(path) = crate::uris::file_path(entry) else {
                        return rpc::failure(id, INVALID_PARAMS, "folders must be file uris");
                    };
                    folders.push(path);
                }
                folders
            }
            None => roots.clone(),
        };
        for folder in &folders {
            if !roots.iter().any(|root| folder.starts_with(root)) {
                return rpc::failure(
                    id,
                    INVALID_PARAMS,
                    format!("folder outside the session: {}", folder.display()),
                );
            }
        }
        let query = hifind::SearchQuery {
            term: params.query,
            kind: params.kind,
            case_sensitive: params.case_sensitive,
            target: params.target,
        };
        let limit = params
            .limit
            .map(|limit| limit as usize)
            .unwrap_or(DEFAULT_LIMIT)
            .min(LIMIT_CAP);
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let superseded = self.update(|state| {
            let held = state.searches.get(&connection).cloned();
            state.searches.insert_mut(connection, Arc::clone(&cancel));
            held
        });
        if let Some(superseded) = superseded {
            superseded.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        let scanned = tokio::task::spawn_blocking({
            let folders = folders.clone();
            let cancel = Arc::clone(&cancel);
            move || hifind::scan(&folders, &query, limit, &cancel)
        })
        .await;
        {
            self.update(|state| {
                if state
                    .searches
                    .get(&connection)
                    .is_some_and(|held| Arc::ptr_eq(held, &cancel))
                {
                    state.searches.remove_mut(&connection);
                }
            });
        }
        match scanned {
            Ok(Ok(scan)) => rpc::success(
                id,
                himark_ahp_ext_types::SearchResult {
                    hits: scan
                        .hits
                        .iter()
                        .map(|hit| crate::uris::file_uri(&folders[hit.folder].join(&hit.relative)))
                        .collect(),
                    truncated: scan.truncated,
                },
            ),
            Ok(Err(message)) => rpc::failure(id, INVALID_PARAMS, message),
            Err(error) => rpc::failure(id, INTERNAL, error.to_string()),
        }
    }

    fn create_terminal(self: &Arc<Self>, id: u64, params: Value) -> JsonRpcMessage {
        let params: CreateTerminalParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => return rpc::failure(id, INVALID_PARAMS, error.to_string()),
        };
        {
            let state = self.snapshot();
            if state.terminals.contains_key(&params.channel) {
                return rpc::failure(
                    id,
                    INVALID_PARAMS,
                    format!("terminal exists: {}", params.channel),
                );
            }
        }
        let cols = params.cols.unwrap_or(80).clamp(2, 4096) as u16;
        let rows = params.rows.unwrap_or(24).clamp(2, 4096) as u16;
        let cwd = params
            .cwd
            .as_ref()
            .map(local_path)
            .filter(|path| path.is_dir())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        let pty = match crate::pty::spawn(&crate::pty::PtyConfig {
            shell: self.config.shell.clone(),
            cwd,
            cols,
            rows,
        }) {
            Ok(pty) => pty,
            Err(error) => return rpc::failure(id, INTERNAL, format!("pty spawn: {error}")),
        };
        let title = params.name.clone().unwrap_or_else(|| {
            self.config
                .shell
                .rsplit('/')
                .next()
                .unwrap_or("shell")
                .to_owned()
        });
        let seed = TerminalState {
            title,
            cwd: params.cwd.clone(),
            cols: Some(cols as i64),
            rows: Some(rows as i64),
            content: Vec::new(),
            exit_code: None,
            claim: params.claim.clone(),
            supports_command_detection: Some(false),
            is_pty: Some(true),
        };
        let channel = params.channel.clone();
        self.update(|state| {
            state.terminals.insert_mut(
                channel.clone(),
                TerminalEntry {
                    state: seed,
                    pty: Some(PtyHalves {
                        writer: Arc::clone(&pty.writer),
                        controller: Arc::clone(&pty.controller),
                        child: Arc::clone(&pty.child),
                    }),
                },
            );
        });

        let host = Arc::downgrade(self);
        let mut reader = pty.reader;
        let child = Arc::clone(&pty.child);
        let pump_channel = channel.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut tail = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                    Ok(read) => {
                        let text = crate::pty::take_valid_utf8(&mut tail, &buf[..read]);
                        let Some(host) = host.upgrade() else { return };
                        if !text.is_empty() {
                            host.apply(
                                &pump_channel,
                                StateAction::TerminalData(TerminalDataAction { data: text }),
                            );
                        }
                    }
                }
            }
            let code = child
                .lock()
                .ok()
                .and_then(|mut child| child.wait().ok())
                .and_then(|status| status.code());
            let Some(host) = host.upgrade() else { return };
            host.terminal_exited(&pump_channel, code.map(|code| code as i64));
        });
        self.terminals_changed();
        rpc::success(id, Value::Null)
    }

    fn terminal_exited(&self, channel: &Uri, code: Option<i64>) {
        {
            let state = self.snapshot();
            if !state.terminals.contains_key(channel) {
                return;
            }
        }
        self.apply(
            channel,
            StateAction::TerminalExited(TerminalExitedAction { exit_code: code }),
        );
        self.update(|state| {
            if let Some(entry) = state.terminals.get(channel) {
                let mut entry = entry.clone();
                entry.pty = None;
                state.terminals.insert_mut(channel.clone(), entry);
            }
        });
        self.terminals_changed();
    }

    fn dispose_terminal(&self, id: u64, params: Value) -> JsonRpcMessage {
        let params: DisposeTerminalParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => return rpc::failure(id, INVALID_PARAMS, error.to_string()),
        };
        let removed = self.update(|state| {
            state.subscribers.remove_mut(&params.channel);
            let removed = state.terminals.get(&params.channel).cloned();
            state.terminals.remove_mut(&params.channel);
            removed
        });
        let Some(entry) = removed else {
            return rpc::failure(
                id,
                NO_SUCH_CHANNEL,
                format!("no terminal {}", params.channel),
            );
        };
        if let Some(pty) = entry.pty {
            if let Ok(mut child) = pty.child.lock() {
                let _ = child.kill();
            }
        }
        self.terminals_changed();
        rpc::success(id, Value::Null)
    }

    fn terminals_changed(&self) {
        let terminals: Vec<TerminalInfo> = {
            let state = self.snapshot();
            state
                .terminals
                .iter()
                .map(|(uri, entry)| TerminalInfo {
                    resource: uri.clone(),
                    title: entry.state.title.clone(),
                    claim: entry.state.claim.clone(),
                    exit_code: entry.state.exit_code,
                })
                .collect()
        };
        self.apply(
            &ROOT.to_owned(),
            StateAction::RootTerminalsChanged(RootTerminalsChangedAction { terminals }),
        );
    }

    fn terminal_write(&self, channel: &Uri, data: String) {
        let writer = {
            let state = self.snapshot();
            let Some(entry) = state.terminals.get(channel) else {
                return;
            };
            if entry.state.exit_code.is_some() {
                return;
            }
            entry.pty.as_ref().map(|pty| Arc::clone(&pty.writer))
        };
        let Some(writer) = writer else { return };
        let _ = tokio::task::spawn_blocking(move || {
            use std::io::Write;
            let _ = (&*writer).write_all(data.as_bytes());
        });
    }

    fn terminal_winsize(&self, channel: &Uri, cols: u16, rows: u16) {
        let controller = {
            let state = self.snapshot();
            state
                .terminals
                .get(channel)
                .and_then(|entry| entry.pty.as_ref().map(|pty| Arc::clone(&pty.controller)))
        };
        if let Some(controller) = controller {
            crate::pty::resize(&controller, cols, rows);
        }
    }

    fn ensure_changeset(self: &Arc<Self>, channel: &Uri) {
        if let Some((folder, _)) = crate::changes::channel_commit(channel) {
            let known = {
                let state = self.snapshot();
                state.changesets.contains_key(channel)
            };
            if !known {
                self.update(|state| {
                    state.changesets.insert_mut(
                        channel.clone(),
                        ChangesetEntry {
                            state: ChangesetState {
                                status: ChangesetStatus::Computing,
                                error: None,
                                files: Vec::new(),
                                operations: None,
                            },
                            folder,
                            _watch: None,
                        },
                    );
                });
            }
            return;
        }
        let Some(folder) = crate::changes::channel_folder(channel) else {
            return;
        };
        let known = {
            let state = self.snapshot();
            state.changesets.contains_key(channel)
        };
        if !known {
            let watch = higit::toplevel(&folder)
                .and_then(|top| higit::signal(&top))
                .and_then(|signal| {
                    let host = Arc::downgrade(self);
                    let recompute = channel.clone();
                    let watcher = notify::PollWatcher::new(
                        move |outcome: notify::Result<notify::Event>| {
                            let Ok(_) = outcome else { return };
                            let Some(host) = host.upgrade() else { return };
                            host.recompute_changes(&recompute);
                        },
                        notify::Config::default()
                            .with_compare_contents(true)
                            .with_poll_interval(std::time::Duration::from_millis(750)),
                    );
                    let mut watcher = watcher.ok()?;
                    notify::Watcher::watch(
                        &mut watcher,
                        &signal,
                        notify::RecursiveMode::NonRecursive,
                    )
                    .ok()?;
                    Some(watcher)
                });
            self.update(|state| {
                state.changesets.insert_mut(
                    channel.clone(),
                    ChangesetEntry {
                        state: ChangesetState {
                            status: ChangesetStatus::Computing,
                            error: None,
                            files: Vec::new(),
                            operations: None,
                        },
                        folder,
                        _watch: watch.map(Arc::new),
                    },
                );
            });
        }
    }

    fn open_document(self: &Arc<Self>, id: u64, params: Value) -> JsonRpcMessage {
        let params =
            match serde_json::from_value::<himark_ahp_ext_types::OpenDocumentParams>(params) {
                Ok(params) => params,
                Err(error) => return rpc::failure(id, INVALID_PARAMS, error.to_string()),
            };
        if params.uri.is_some() && params.text.is_some() {
            return rpc::failure(id, INVALID_PARAMS, "uri and text are mutually exclusive");
        }

        {
            let state = self.snapshot();
            if state.sessions.get(&params.channel).is_none() {
                return rpc::failure(id, INVALID_PARAMS, format!("no session {}", params.channel));
            }
        }
        let text = match &params.uri {
            Some(uri) => {
                let Some(path) = crate::uris::file_path(uri) else {
                    return rpc::failure(id, INVALID_PARAMS, format!("unservable uri {uri}"));
                };
                match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(error) => {
                        return rpc::failure(id, INVALID_PARAMS, format!("{uri}: {error}"))
                    }
                }
            }
            None => params.text.clone().unwrap_or_default(),
        };
        let opened = self.update(|state| {
            let mut session = state.sessions.get(&params.channel)?.clone();
            if let Some(uri) = &params.uri {
                if let Some(channel) = session.mirrors.get(uri) {
                    let version = session.documents[channel].version();
                    return Some((channel.clone(), version, None));
                }
            }
            let seq = state.document_seq + 1;
            let channel = format!("{}{}", crate::documents::CHANNEL_PREFIX, seq);
            let document = crate::documents::Document::open(&text, crate::documents::mint(seq));
            let version = document.version();
            session.documents.insert_mut(channel.clone(), document);
            let feed = params.uri.as_ref().map(|uri| {
                (
                    session
                        .state
                        .working_directories
                        .clone()
                        .unwrap_or_default(),
                    uri.clone(),
                )
            });
            if let Some(uri) = &params.uri {
                session.mirrors.insert_mut(uri.clone(), channel.clone());
                session.disk_texts.insert_mut(channel.clone(), text.clone());
            }
            state.document_seq = seq;
            state.sessions.insert_mut(params.channel.clone(), session);
            Some((channel, version, feed))
        });
        let Some((channel, version, feed)) = opened else {
            return rpc::failure(id, INVALID_PARAMS, format!("no session {}", params.channel));
        };

        if let Some((dirs, uri)) = feed {
            self.lsp_feed_open(&dirs, &uri, &text, version);
            // The host owns disk reloads for this mirror from here on.
            self.watch_mirror(channel.clone(), uri);
        }
        rpc::success(
            id,
            himark_ahp_ext_types::OpenDocumentResult {
                document: channel,
                version,
            },
        )
    }

    /// Arm the file watcher behind a freshly minted mirror: the file
    /// is the agent's; when it changes, the HOST reconciles the
    /// mirror and broadcasts its own edit
    /// (docs: agents edit files; clients edit documents).
    fn watch_mirror(self: &Arc<Self>, channel: Uri, uri: Uri) {
        let Some(path) = crate::uris::file_path(&uri) else {
            return;
        };
        let host = Arc::downgrade(self);
        let target = channel.clone();
        let watcher = notify::PollWatcher::new(
            move |outcome: notify::Result<notify::Event>| {
                let Ok(_) = outcome else { return };
                let Some(host) = host.upgrade() else { return };
                host.reload_mirror(&target);
            },
            notify::Config::default()
                .with_compare_contents(true)
                .with_poll_interval(std::time::Duration::from_millis(400)),
        );
        let Ok(mut watcher) = watcher else {
            return;
        };
        if notify::Watcher::watch(&mut watcher, &path, notify::RecursiveMode::NonRecursive).is_err()
        {
            return;
        }
        self.update(|state| {
            state.mirror_watches.insert_mut(
                channel.clone(),
                WatchEntry {
                    _watcher: Arc::new(watcher),
                    root: uri.clone(),
                },
            );
        });
    }

    /// The file under a mirror changed: reconcile and BROADCAST. The
    /// mirror is the source of truth; the disk text last reconciled
    /// is the three-way base when clients hold unflushed edits; the
    /// result goes out as the host's own edit, exactly like a
    /// client's would.
    fn reload_mirror(self: &Arc<Self>, channel: &Uri) {
        let path = {
            let state = self.snapshot();
            let Some(entry) = state.mirror_watches.get(channel) else {
                return;
            };
            let Some(path) = crate::uris::file_path(&entry.root) else {
                return;
            };
            path
        };
        let Ok(fetched) = std::fs::read_to_string(&path) else {
            return;
        };
        if std::env::var_os("HIHOST_TRACE").is_some() {
            eprintln!(
                "[hihost] reload_mirror {channel}: fetched {} bytes",
                fetched.len()
            );
        }
        let feed = self.update(|state| {
            let (owner, session) = state
                .sessions
                .iter()
                .find(|(_, session)| session.documents.contains_key(channel))
                .map(|(uri, session)| (uri.clone(), session.clone()))?;
            let mut session = session;
            let mut document = session.documents[channel].clone();
            let current = himark_ahp_ext_types::text::materialize(document.text());
            let base = session
                .disk_texts
                .get(channel)
                .cloned()
                .unwrap_or_else(|| current.clone());
            if fetched == base {
                return None;
            }
            let target = match current == base {
                true => fetched.clone(),
                false => crate::documents::reload::merged(&base, &current, &fetched),
            };
            let spans = crate::documents::reload::spans(&current, &target);
            session
                .disk_texts
                .insert_mut(channel.clone(), fetched.clone());
            if spans.is_empty() {
                state.sessions.insert_mut(owner, session);
                return None;
            }
            let seq = state.document_seq + 1;
            state.document_seq = seq;
            if std::env::var_os("HIHOST_TRACE").is_some() {
                eprintln!(
                    "[hihost] reload_mirror {channel}: {} span(s), base v{}",
                    spans.len(),
                    document.version(),
                );
            }
            let action = himark_ahp_ext_types::DocumentApplied {
                base: document.version(),
                operation: himark_ahp_ext_types::text::wire_of(document.text(), &spans),
                id: crate::documents::mint(seq),
                origin: None,
            };
            if !document.dispatch(&action) {
                return None;
            }
            let uri = session
                .mirrors
                .iter()
                .find(|(_, held)| *held == channel)
                .map(|(uri, _)| uri.clone());
            let dirs = session.state.working_directories.clone();
            session.documents.insert_mut(channel.clone(), document);
            state.sessions.insert_mut(owner, session);

            state.server_seq += 1;
            let mut value = serde_json::to_value(&action).expect("a wire action");
            value["type"] = Value::String(himark_ahp_ext_types::DOCUMENT_APPLIED.to_owned());
            let envelope = ahp_types::actions::ActionEnvelope {
                channel: channel.clone(),
                action: StateAction::Unknown(value),
                server_seq: state.server_seq as u64,
                origin: None,
                rejection_reason: None,
            };
            let line = rpc::line(&rpc::notification("action", &envelope));
            if let Some(subscribers) = state.subscribers.get(channel) {
                for (_, outbox) in subscribers.iter() {
                    let _ = outbox.send(line.clone());
                }
            }
            state.replay.enqueue_mut(envelope);
            if state.replay.len() > REPLAY_DEPTH {
                state.replay.dequeue_mut();
            }
            Some((uri, dirs, action))
        });
        let Some((uri, dirs, action)) = feed else {
            return;
        };
        if let Some(uri) = uri {
            self.lsp_feed_change(
                &dirs.unwrap_or_default(),
                &uri,
                &action.operation,
                action.id,
            );
        }
    }

    fn subscribe_document(
        &self,
        connection: u64,
        outbox: &Outbox,
        id: u64,
        channel: &Uri,
    ) -> JsonRpcMessage {
        let snapshot = self.update(|state| {
            let (session, document) = state
                .sessions
                .values()
                .find_map(|session| session.documents.get(channel).map(|held| (session, held)))?;
            let uri = session
                .mirrors
                .iter()
                .find(|(_, held)| *held == channel)
                .map(|(uri, _)| uri.clone());
            let snapshot = serde_json::json!({
                "resource": channel,
                "state": document.snapshot(uri.as_deref()),
                "fromSeq": state.server_seq,
            });
            subscribe_outbox(state, channel, connection, outbox);
            Some(snapshot)
        });
        match snapshot {
            Some(snapshot) => rpc::success(id, serde_json::json!({ "snapshot": snapshot })),
            None => rpc::failure(id, NO_SUCH_CHANNEL, format!("no channel {channel}")),
        }
    }

    /// documents@1 storeDocument: dump a mirrored document to a
    /// resource. A client saves THROUGH the host so the write lands
    /// ordered behind its committed edits; `disk_texts` moves with
    /// the write so the mirror watcher does not read the host's own
    /// write back as a foreign edit.
    fn store_document(self: &Arc<Self>, id: u64, params: Value) -> JsonRpcMessage {
        let params =
            match serde_json::from_value::<himark_ahp_ext_types::StoreDocumentParams>(params) {
                Ok(params) => params,
                Err(error) => return rpc::failure(id, INVALID_PARAMS, error.to_string()),
            };
        let Some(path) = crate::uris::file_path(&params.uri) else {
            return rpc::failure(id, INVALID_PARAMS, format!("unservable uri {}", params.uri));
        };
        let found = {
            let state = self.snapshot();
            let found = state.sessions.values().find_map(|session| {
                let document = session.documents.get(&params.channel)?;
                Some((
                    himark_ahp_ext_types::text::materialize(document.text()),
                    document.version(),
                ))
            });
            found
        };
        let Some((text, version)) = found else {
            return rpc::failure(
                id,
                NO_SUCH_CHANNEL,
                format!("no channel {}", params.channel),
            );
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, &text) {
            return rpc::failure(id, INTERNAL, format!("{}: {error}", path.display()));
        }
        self.update(|state| {
            let mirrored = state.sessions.iter().find_map(|(uri, session)| {
                (session.mirrors.get(&params.uri) == Some(&params.channel))
                    .then(|| (uri.clone(), session.clone()))
            });
            let Some((owner, mut session)) = mirrored else {
                return;
            };
            session
                .disk_texts
                .insert_mut(params.channel.clone(), text.clone());
            state.sessions.insert_mut(owner, session);
        });
        self.changes_touched(&path);
        rpc::success(id, himark_ahp_ext_types::StoreDocumentResult { version })
    }

    fn document_dispatch(&self, channel: &Uri, value: Value) {
        if value["type"] != himark_ahp_ext_types::DOCUMENT_APPLIED {
            return;
        }
        let Ok(action) =
            serde_json::from_value::<himark_ahp_ext_types::DocumentApplied>(value.clone())
        else {
            return;
        };
        let feed = self.update(|state| {
            let (owner, session) = state
                .sessions
                .iter()
                .find(|(_, session)| session.documents.contains_key(channel))
                .map(|(uri, session)| (uri.clone(), session.clone()))?;
            let mut session = session;
            let mut document = session.documents[channel].clone();
            if !document.dispatch(&action) {
                return None;
            }
            let uri = session
                .mirrors
                .iter()
                .find(|(_, held)| *held == channel)
                .map(|(uri, _)| uri.clone());
            let dirs = session.state.working_directories.clone();
            session.documents.insert_mut(channel.clone(), document);
            state.sessions.insert_mut(owner, session);

            state.server_seq += 1;
            let envelope = ahp_types::actions::ActionEnvelope {
                channel: channel.clone(),
                action: StateAction::Unknown(value),
                server_seq: state.server_seq as u64,
                origin: None,
                rejection_reason: None,
            };
            let line = rpc::line(&rpc::notification("action", &envelope));
            if let Some(subscribers) = state.subscribers.get(channel) {
                for (_, outbox) in subscribers.iter() {
                    let _ = outbox.send(line.clone());
                }
            }
            state.replay.enqueue_mut(envelope);
            if state.replay.len() > REPLAY_DEPTH {
                state.replay.dequeue_mut();
            }
            Some(uri.map(|uri| (dirs.unwrap_or_default(), uri)))
        });
        let Some(feed) = feed else {
            return;
        };

        if let Some((dirs, uri)) = feed {
            self.lsp_feed_change(&dirs, &uri, &action.operation, action.id);
        }
    }

    async fn handle_lsp(
        self: &Arc<Self>,
        connection: u64,
        id: u64,
        method: &str,
        params: Value,
    ) -> JsonRpcMessage {
        let bare = &method["lsp/".len()..];
        if bare == "capabilities" {
            return self.lsp_capabilities(id, params);
        }
        if bare == "diagnostics" {
            return self.lsp_diagnostics_channel(id, params);
        }
        if LSP_EXCLUDED.contains(&bare) {
            return rpc::failure(
                id,
                LSP_METHOD_NOT_ALLOWED,
                format!("{bare} is the server's own"),
            );
        }
        let channel = params["channel"].as_str().unwrap_or_default().to_owned();
        let Some(forwarded) = params.get("params").cloned() else {
            return rpc::failure(id, INVALID_PARAMS, "no params in the envelope");
        };
        let Some(dirs) = self.session_dirs(&channel) else {
            return rpc::failure(id, INVALID_PARAMS, format!("no session {channel}"));
        };

        let target = forwarded
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| dirs.first().map(|dir| format!("{dir}/_")));
        let Some(target) = target else {
            return rpc::failure(id, LSP_NO_LANGUAGE_SERVER, "no routable target");
        };
        let Some((root, command)) = self.lsp_route(&dirs, &target) else {
            return rpc::failure(
                id,
                LSP_NO_LANGUAGE_SERVER,
                format!("no language server serves {target}"),
            );
        };
        let Some(server) = self.lsp.ensure(&root, &command) else {
            return rpc::failure(
                id,
                LSP_NO_LANGUAGE_SERVER,
                format!("the language server for {} is dead", root.display()),
            );
        };

        if let Some(uri) = forwarded
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
        {
            if let Some(path) = crate::uris::file_path(uri) {
                server.ensure_open_from_disk(uri, &path);
            }
        }
        if !server.ready().await {
            return rpc::failure(
                id,
                LSP_NO_LANGUAGE_SERVER,
                format!("the language server for {} died", root.display()),
            );
        }
        let (ls_id, answer) = server.request(bare, forwarded);
        self.lsp_inflight
            .lock()
            .expect("lsp inflight")
            .insert((connection, id), (Arc::clone(&server), ls_id));

        if self
            .lsp_cancelled
            .lock()
            .expect("lsp cancelled")
            .remove(&(connection, id))
        {
            server.forward_notification("$/cancelRequest", serde_json::json!({ "id": ls_id }));
        }
        let response = answer.await;
        self.lsp_inflight
            .lock()
            .expect("lsp inflight")
            .remove(&(connection, id));
        self.lsp_cancelled
            .lock()
            .expect("lsp cancelled")
            .remove(&(connection, id));

        if let Some(error) = response.get("error") {
            return rpc::failure(
                id,
                error["code"].as_i64().unwrap_or(-32603) as i32,
                error["message"].as_str().unwrap_or("language server error"),
            );
        }
        match response.get("result") {
            Some(result) => rpc::success(id, result.clone()),
            None => rpc::failure(id, LSP_NO_LANGUAGE_SERVER, "the language server died"),
        }
    }

    fn lsp_capabilities(&self, id: u64, params: Value) -> JsonRpcMessage {
        let channel = params["channel"].as_str().unwrap_or_default().to_owned();
        let uri = params["uri"].as_str().unwrap_or_default().to_owned();
        let Some(dirs) = self.session_dirs(&channel) else {
            return rpc::failure(id, INVALID_PARAMS, format!("no session {channel}"));
        };
        let answer = self
            .lsp_route(&dirs, &uri)
            .and_then(|(root, command)| self.lsp.ensure(&root, &command))
            .and_then(|server| server.capabilities());
        match answer {
            Some(capabilities) => {
                rpc::success(id, serde_json::json!({ "capabilities": capabilities }))
            }
            None => rpc::success(id, Value::Null),
        }
    }

    fn lsp_diagnostics_channel(&self, id: u64, params: Value) -> JsonRpcMessage {
        let session = params["channel"].as_str().unwrap_or_default().to_owned();
        let minted = self.update(|state| {
            if !state.sessions.contains_key(&session) {
                return None;
            }
            if let Some((channel, _)) = state
                .lsp_diagnostics
                .iter()
                .find(|(_, held)| held.session == session)
            {
                return Some(channel.clone());
            }
            let channel = format!("{}{}", LSP_DIAGNOSTICS_PREFIX, crate::uuid_v4());
            state.lsp_diagnostics.insert_mut(
                channel.clone(),
                LspDiagnostics {
                    session: session.clone(),
                    items: rpds::HashTrieMapSync::new_sync(),
                },
            );
            Some(channel)
        });
        match minted {
            Some(channel) => rpc::success(id, serde_json::json!({ "channel": channel })),
            None => rpc::failure(id, INVALID_PARAMS, format!("no session {session}")),
        }
    }

    fn subscribe_diagnostics(
        &self,
        connection: u64,
        outbox: &Outbox,
        id: u64,
        channel: &Uri,
    ) -> JsonRpcMessage {
        let snapshot = self.update(|state| {
            let entry = state.lsp_diagnostics.get(channel)?;
            let mut items = serde_json::Map::new();
            for (uri, (version, diagnostics)) in entry.items.iter() {
                let mut published = serde_json::json!({ "diagnostics": diagnostics });
                if let Some(version) = version {
                    published["version"] = Value::String(version.clone());
                }
                items.insert(uri.clone(), published);
            }
            let snapshot = serde_json::json!({
                "resource": channel,
                "state": { "items": items },
                "fromSeq": state.server_seq,
            });
            subscribe_outbox(state, channel, connection, outbox);
            Some(snapshot)
        });
        match snapshot {
            Some(snapshot) => rpc::success(id, serde_json::json!({ "snapshot": snapshot })),
            None => rpc::failure(id, NO_SUCH_CHANNEL, format!("no channel {channel}")),
        }
    }

    fn lsp_cancel(&self, connection: u64, params: &Value) {
        let Some(id) = params["id"].as_u64() else {
            return;
        };
        let inflight = self.lsp_inflight.lock().expect("lsp inflight");
        match inflight.get(&(connection, id)) {
            Some((server, ls_id)) => {
                server.forward_notification("$/cancelRequest", serde_json::json!({ "id": ls_id }))
            }

            None => {
                self.lsp_cancelled
                    .lock()
                    .expect("lsp cancelled")
                    .insert((connection, id));
            }
        }
    }

    fn session_dirs(&self, session: &str) -> Option<Vec<String>> {
        let state = self.snapshot();
        let entry = state.sessions.get(session)?;
        Some(entry.state.working_directories.clone().unwrap_or_default())
    }

    fn lsp_route(&self, dirs: &[String], uri: &str) -> Option<(PathBuf, String)> {
        let path = crate::uris::file_path_str(uri)?;
        let extension = path.rsplit('.').next()?.to_lowercase();
        let row = self
            .config
            .language_servers
            .iter()
            .find(|row| row.extensions.contains(&extension));
        let root = dirs
            .iter()
            .filter_map(|dir| crate::uris::file_path_str(dir))
            .filter(|dir| path.starts_with(*dir))
            .max_by_key(|dir| dir.len())?;
        row.map(|row| (PathBuf::from(root), row.command.clone()))
    }

    fn lsp_feed_open(
        &self,
        dirs: &[String],
        uri: &str,
        text: &str,
        uid: himark_ahp_ext_types::Uid,
    ) {
        if let Some((root, command)) = self.lsp_route(dirs, uri) {
            if let Some(server) = self.lsp.ensure(&root, &command) {
                server.document_opened(uri, text, uid);
            }
        }
    }

    fn lsp_feed_change(
        &self,
        dirs: &[String],
        uri: &str,
        operation: &himark_ahp_ext_types::TextOperation,
        uid: himark_ahp_ext_types::Uid,
    ) {
        if let Some((root, command)) = self.lsp_route(dirs, uri) {
            if let Some(server) = self.lsp.ensure(&root, &command) {
                server.document_changed(uri, operation, uid);
            }
        }
    }

    fn ls_event(&self, server: &Arc<crate::lsp::Server>, event: crate::lsp::LsEvent) {
        match event {
            crate::lsp::LsEvent::Diagnostics {
                uri,
                version,
                diagnostics,
            } => {
                let uid = server.uid_of(&uri, version).map(|uid| uid.to_string());
                let path = crate::uris::file_path_str(&uri).unwrap_or(&uri).to_owned();
                self.update(|state| {
                    let sessions: Vec<(Uri, bool)> = state
                        .lsp_diagnostics
                        .iter()
                        .filter(|(_, entry)| {
                            state.sessions.get(&entry.session).is_some_and(|session| {
                                session
                                    .state
                                    .working_directories
                                    .iter()
                                    .flatten()
                                    .filter_map(|dir| crate::uris::file_path_str(dir))
                                    .any(|dir| path.starts_with(dir))
                            })
                        })
                        .map(|(channel, _)| {
                            (
                                channel.clone(),
                                diagnostics.as_array().is_some_and(Vec::is_empty),
                            )
                        })
                        .collect();
                    for (channel, empty) in sessions {
                        if let Some(entry) = state.lsp_diagnostics.get(&channel) {
                            let mut entry = entry.clone();
                            if empty {
                                entry.items.remove_mut(&uri);
                            } else {
                                entry
                                    .items
                                    .insert_mut(uri.clone(), (uid.clone(), diagnostics.clone()));
                            }
                            state.lsp_diagnostics.insert_mut(channel.clone(), entry);
                        }
                        let mut action = serde_json::json!({
                            "type": "lspDiagnostics/published",
                            "uri": uri,
                            "diagnostics": diagnostics,
                        });
                        if let Some(uid) = &uid {
                            action["version"] = Value::String(uid.clone());
                        }
                        state.server_seq += 1;
                        let envelope = ahp_types::actions::ActionEnvelope {
                            channel: channel.clone(),
                            action: StateAction::Unknown(action),
                            server_seq: state.server_seq as u64,
                            origin: None,
                            rejection_reason: None,
                        };
                        let line = rpc::line(&rpc::notification("action", &envelope));
                        if let Some(subscribers) = state.subscribers.get(&channel) {
                            for (_, outbox) in subscribers.iter() {
                                let _ = outbox.send(line.clone());
                            }
                        }
                        state.replay.enqueue_mut(envelope);
                        if state.replay.len() > REPLAY_DEPTH {
                            state.replay.dequeue_mut();
                        }
                    }
                });
            }

            crate::lsp::LsEvent::Progress => {}
        }
    }

    fn republish_session_catalog(&self, channel: &Uri) {
        let directories = {
            let state = self.snapshot();
            match state.sessions.get(channel) {
                Some(entry) => entry.state.working_directories.clone().unwrap_or_default(),
                None => return,
            }
        };
        self.apply(
            channel,
            StateAction::SessionChangesetsChanged(
                ahp_types::actions::SessionChangesetsChangedAction {
                    changesets: Some(crate::changes::catalog(&directories)),
                },
            ),
        );
    }

    fn publish_changes(&self, channel: &Uri, computed: crate::changes::Computed) {
        let (live, ready) = {
            let state = self.snapshot();
            match state.changesets.get(channel) {
                Some(entry) => (true, entry.state.status == ChangesetStatus::Ready),
                None => (false, false),
            }
        };
        if !live {
            return;
        }
        match computed {
            crate::changes::Computed::NotRepository => self.apply(
                channel,
                StateAction::ChangesetStatusChanged(ChangesetStatusChangedAction {
                    status: ChangesetStatus::Error,
                    error: Some(ErrorInfo {
                        error_type: "notRepository".to_owned(),
                        message: "not a git repository".to_owned(),
                        stack: None,
                        meta: None,
                    }),
                }),
            ),
            crate::changes::Computed::Files(files) => {
                self.apply(
                    channel,
                    StateAction::ChangesetContentChanged(Box::new(ChangesetContentChangedAction {
                        files,
                        operations: None,
                        error: None,
                    })),
                );
                if !ready {
                    self.apply(
                        channel,
                        StateAction::ChangesetStatusChanged(ChangesetStatusChangedAction {
                            status: ChangesetStatus::Ready,
                            error: None,
                        }),
                    );
                }
            }
        }
    }

    fn recompute_changes(self: &Arc<Self>, channel: &Uri) {
        let folder = {
            let state = self.snapshot();
            match state.changesets.get(channel) {
                Some(entry) => entry.folder.clone(),
                None => return,
            }
        };
        let host = Arc::downgrade(self);
        let commit = crate::changes::channel_commit(channel).map(|(_, commit)| commit);
        let channel = channel.clone();
        std::thread::spawn(move || {
            let computed = match &commit {
                Some(commit) => crate::changes::compute_commit(&folder, commit),
                None => crate::changes::compute(&folder),
            };
            let Some(host) = host.upgrade() else { return };
            host.publish_changes(&channel, computed);
        });
    }

    fn subscribe_history(
        self: &Arc<Self>,
        connection: u64,
        outbox: &Outbox,
        id: u64,
        channel: &Uri,
    ) -> JsonRpcMessage {
        let Some(folder) = crate::history::channel_folder(channel) else {
            return rpc::failure(id, NO_SUCH_CHANNEL, format!("no channel {channel}"));
        };
        let known = {
            let state = self.snapshot();
            state.histories.contains_key(channel)
        };
        if !known {
            let watch = higit::toplevel(&folder)
                .and_then(|top| higit::signal(&top))
                .and_then(|signal| {
                    let host = Arc::downgrade(self);
                    let recompute = channel.clone();
                    let watcher = notify::PollWatcher::new(
                        move |outcome: notify::Result<notify::Event>| {
                            let Ok(_) = outcome else { return };
                            let Some(host) = host.upgrade() else { return };
                            host.recompute_history(&recompute);
                        },
                        notify::Config::default()
                            .with_compare_contents(true)
                            .with_poll_interval(std::time::Duration::from_millis(750)),
                    );
                    let mut watcher = watcher.ok()?;
                    notify::Watcher::watch(
                        &mut watcher,
                        &signal,
                        notify::RecursiveMode::NonRecursive,
                    )
                    .ok()?;
                    Some(watcher)
                });
            self.update(|state| {
                state.histories.insert_mut(
                    channel.clone(),
                    HistoryEntry {
                        state: himark_ahp_ext_types::history::HistoryState::default(),
                        folder,
                        _watch: watch.map(Arc::new),
                    },
                );
            });
        }
        let snapshot = self.update(|state| {
            let entry = state.histories.get(channel)?;
            let snapshot = serde_json::json!({
                "resource": channel,
                "state": entry.state,
                "fromSeq": state.server_seq,
            });
            subscribe_outbox(state, channel, connection, outbox);
            Some(snapshot)
        });

        self.recompute_history(channel);
        match snapshot {
            Some(snapshot) => rpc::success(id, serde_json::json!({ "snapshot": snapshot })),
            None => rpc::failure(id, NO_SUCH_CHANNEL, format!("no channel {channel}")),
        }
    }

    fn recompute_history(self: &Arc<Self>, channel: &Uri) {
        let (folder, depth) = {
            let state = self.snapshot();
            match state.histories.get(channel) {
                Some(entry) => (
                    entry.folder.clone(),
                    entry.state.commits.len().max(crate::history::WINDOW),
                ),
                None => return,
            }
        };
        let host = Arc::downgrade(self);
        let channel = channel.clone();
        std::thread::spawn(move || {
            let computed = crate::history::compute(&folder, depth);
            let Some(host) = host.upgrade() else { return };
            host.publish_history(&channel, computed);
        });
    }

    fn publish_history(&self, channel: &Uri, computed: crate::history::Computed) {
        use himark_ahp_ext_types::history as wire;
        let state = match computed {
            crate::history::Computed::NotRepository => wire::HistoryState {
                status: wire::HistoryStatus::Error,
                error: Some("not a git repository".to_owned()),
                ..Default::default()
            },
            crate::history::Computed::Window(state) => state,
        };
        let known = self.update(|host_state| match host_state.histories.get(channel) {
            Some(entry) => {
                let mut entry = entry.clone();
                entry.state = state.clone();
                host_state.histories.insert_mut(channel.clone(), entry);
                true
            }
            None => false,
        });
        if !known {
            return;
        }
        self.apply(
            channel,
            StateAction::Unknown(wire::action_value(
                wire::HISTORY_RESET,
                &wire::HistoryReset { state },
            )),
        );
    }

    fn history_dispatch(self: &Arc<Self>, channel: &Uri, value: Value) {
        use himark_ahp_ext_types::history as wire;
        let folder = {
            let state = self.snapshot();
            match state.histories.get(channel) {
                Some(entry) => entry.folder.clone(),
                None => return,
            }
        };
        if value["type"] == wire::HISTORY_GROW {
            let Ok(grow) = serde_json::from_value::<wire::HistoryGrow>(value) else {
                return;
            };
            let Ok(skip) = grow.before.parse::<usize>() else {
                return;
            };
            let limit = grow.limit.map_or(crate::history::WINDOW, |limit| {
                (limit as usize).clamp(1, 1000)
            });
            let host = Arc::downgrade(self);
            let channel = channel.clone();
            std::thread::spawn(move || {
                let Some((commits, more)) = crate::history::compute_slice(&folder, skip, limit)
                else {
                    return;
                };
                let Some(host) = host.upgrade() else { return };
                let known = host.update(|state| match state.histories.get(&channel) {
                    Some(entry) => {
                        let mut entry = entry.clone();

                        if entry.state.commits.len() != skip {
                            return false;
                        }
                        entry.state.commits.extend(commits.iter().cloned());
                        entry.state.more = more.clone();
                        state.histories.insert_mut(channel.clone(), entry);
                        true
                    }
                    None => false,
                });
                if known {
                    host.apply(
                        &channel,
                        StateAction::Unknown(wire::action_value(
                            wire::HISTORY_APPENDED,
                            &wire::HistoryAppended { commits, more },
                        )),
                    );
                }
            });
            return;
        }
        if value["type"] == wire::HISTORY_COMMIT {
            let Ok(ask) = serde_json::from_value::<wire::HistoryCommit>(value) else {
                return;
            };
            if ask.message.trim().is_empty() {
                return;
            }
            let host = Arc::downgrade(self);
            let channel = channel.clone();
            std::thread::spawn(move || {
                let Some(toplevel) = higit::toplevel(&folder) else {
                    return;
                };
                if let Err(error) = higit::commit_all(&toplevel, &ask.message) {
                    eprintln!("[hihost] history/commit failed: {error}");
                }
                let Some(host) = host.upgrade() else { return };

                host.recompute_history(&channel);
                let uncommitted = format!("{}{}", crate::changes::CHANNEL_PREFIX, folder.display());
                host.recompute_changes(&uncommitted);
            });
        }
    }

    fn changes_touched(self: &Arc<Self>, path: &Path) {
        let channels: Vec<Uri> = {
            let state = self.snapshot();
            state
                .changesets
                .iter()
                .filter(|(_, entry)| path.starts_with(&entry.folder))
                .map(|(channel, _)| channel.clone())
                .collect()
        };
        for channel in channels {
            self.recompute_changes(&channel);
        }
    }

    fn resource_read(&self, id: u64, params: Value) -> JsonRpcMessage {
        if let Some(uri) = params["uri"].as_str() {
            if uri.starts_with("ahp-content:/") {
                let state = self.snapshot();
                return match state.contents.get(uri) {
                    Some(text) => rpc::success(
                        id,
                        serde_json::json!({"data": text, "encoding": "utf-8", "contentType": "text/plain"}),
                    ),
                    None => rpc::failure(id, -32002, format!("no content {uri}")),
                };
            }
        }
        if let Some(uri) = params["uri"].as_str() {
            if let Some((sha, toplevel, rel)) = crate::changes::parse_ref(uri) {
                return match higit::show(&toplevel, &sha, &rel) {
                    Some(data) => rpc::success(
                        id,
                        serde_json::json!({"data": data, "encoding": "utf-8", "contentType": "text/plain"}),
                    ),
                    None => rpc::failure(id, -32002, format!("no such blob: {uri}")),
                };
            }
        }
        let Some(path) = file_path(&params["uri"]) else {
            return rpc::failure(id, INVALID_PARAMS, "not a file uri");
        };

        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => return rpc::failure(id, -32002, format!("{}: {error}", path.display())),
        };
        if params["encoding"].as_str() == Some("base64") {
            return rpc::success(id, base64_answer(&path, &bytes));
        }
        match String::from_utf8(bytes) {
            Ok(data) => rpc::success(
                id,
                serde_json::json!({"data": data, "encoding": "utf-8", "contentType": "text/plain"}),
            ),

            Err(error) => rpc::success(id, base64_answer(&path, error.as_bytes())),
        }
    }

    fn resource_write(self: &Arc<Self>, id: u64, params: Value) -> JsonRpcMessage {
        let Some(path) = file_path(&params["uri"]) else {
            return rpc::failure(id, INVALID_PARAMS, "not a file uri");
        };
        let data = params["data"].as_str().unwrap_or_default();

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&path, data) {
            Ok(()) => {
                self.changes_touched(&path);
                rpc::success(id, Value::Null)
            }
            Err(error) => rpc::failure(id, INTERNAL, format!("{}: {error}", path.display())),
        }
    }

    fn resource_list(&self, id: u64, params: Value) -> JsonRpcMessage {
        let Some(path) = file_path(&params["uri"]) else {
            return rpc::failure(id, INVALID_PARAMS, "not a file uri");
        };
        let Ok(entries) = std::fs::read_dir(&path) else {
            return rpc::failure(id, -32002, format!("unreadable: {}", path.display()));
        };

        let mut listed: Vec<(bool, String)> = entries
            .flatten()
            .map(|entry| {
                let directory = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                (directory, entry.file_name().to_string_lossy().into_owned())
            })
            .collect();
        listed.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let listed: Vec<Value> = listed
            .into_iter()
            .map(|(directory, name)| {
                serde_json::json!({
                    "name": name,
                    "type": if directory { "directory" } else { "file" },
                })
            })
            .collect();
        rpc::success(id, serde_json::json!({"entries": listed}))
    }
}

struct ChatSink {
    host: Arc<Host>,
    channel: Uri,
}

impl crate::claude::AgentSink for ChatSink {
    fn action(&self, action: StateAction) {
        self.host.agent_action(&self.channel, action);
    }

    fn stash(&self, text: String) -> String {
        let uri = format!("ahp-content:/{}", crate::uuid_v4());
        self.host.update(|state| {
            state.contents.insert_mut(uri.clone(), text);
        });
        uri
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DispatchEnvelope {
    channel: Uri,
    action: StateAction,
}

fn render_annotation(annotation: &Annotation) -> String {
    let place =
        crate::uris::file_path_str(&annotation.resource).unwrap_or(annotation.resource.as_str());
    let anchor = match &annotation.range {
        Some(range) if range.start.line == range.end.line => {
            format!("{place}, line {}", range.start.line + 1)
        }
        Some(range) => format!(
            "{place}, lines {}-{}",
            range.start.line + 1,
            range.end.line + 1
        ),
        None => format!("{place}, whole file"),
    };
    let mut block = format!("Comment ({anchor}):");
    for entry in &annotation.entries {
        let author = entry
            .meta
            .as_ref()
            .and_then(|meta| {
                meta.get("himark")?
                    .get("author")?
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "user".to_owned());
        let text = match &entry.text {
            ahp_types::common::StringOrMarkdown::Plain(text) => text.as_str(),
            ahp_types::common::StringOrMarkdown::Markdown { markdown } => markdown.as_str(),
        };
        block.push_str(&format!("\n[{author}] {text}"));
    }
    block
}

fn referenced_annotations<'a>(
    annotations: &'a [Annotation],
    ids: Option<&Vec<String>>,
) -> Vec<&'a Annotation> {
    annotations
        .iter()
        .filter(|annotation| ids.is_none_or(|wanted| wanted.iter().any(|id| id == &annotation.id)))
        .collect()
}

impl Host {
    fn expanded_prompt(&self, started: &ChatTurnStartedAction) -> String {
        let mut blocks: Vec<String> = Vec::new();
        for attachment in started.message.attachments.iter().flatten() {
            let MessageAttachment::Annotations(refs) = attachment else {
                continue;
            };
            let Some(session) = annotations_session(&refs.resource) else {
                continue;
            };
            let state = self.snapshot();
            let Some(entry) = state.sessions.get(&session) else {
                continue;
            };
            for annotation in
                referenced_annotations(&entry.annotations.annotations, refs.annotation_ids.as_ref())
            {
                blocks.push(render_annotation(annotation));
            }
        }
        if blocks.is_empty() {
            return started.message.text.clone();
        }
        format!(
            "{}\n\n<review-comments>\n{}\n</review-comments>",
            started.message.text,
            blocks.join("\n\n")
        )
    }
}

fn empty_chat(chat: &Uri, title: &str) -> ChatState {
    ChatState {
        resource: chat.clone(),
        title: title.to_owned(),
        status: 1,
        activity: None,
        modified_at: now_rfc3339(),
        origin: Some(ChatOrigin::User),
        interactivity: None,
        working_directories: None,
        turns: Vec::new(),
        turns_next_cursor: None,
        active_turn: None,
        steering_message: None,
        queued_messages: None,
        draft: None,
        meta: None,
    }
}

const LSP_METHOD_NOT_ALLOWED: i32 = -33001;
const LSP_NO_LANGUAGE_SERVER: i32 = -33002;
const LSP_DIAGNOSTICS_PREFIX: &str = "ahp-lsp-diagnostics:/";

const LSP_EXCLUDED: &[&str] = &[
    "initialize",
    "initialized",
    "shutdown",
    "exit",
    "$/setTrace",
    "textDocument/didOpen",
    "textDocument/didChange",
    "textDocument/didClose",
    "textDocument/didSave",
    "textDocument/willSave",
    "textDocument/willSaveWaitUntil",
    "workspace/didChangeWatchedFiles",
    "workspace/didChangeConfiguration",
    "workspace/didChangeWorkspaceFolders",
];

fn session_state(manifest: &Manifest) -> SessionState {
    SessionState {
        provider: manifest.provider.clone(),
        title: manifest.title.clone(),
        status: 1,
        activity: None,
        project: None,
        working_directories: Some(manifest.working_directories.clone()),
        annotations: Some(annotations_summary(
            &manifest.session,
            &manifest.annotations,
        )),
        lifecycle: SessionLifecycle::Ready,
        creation_error: None,
        server_tools: None,
        active_clients: Vec::new(),
        chats: vec![ChatSummary {
            resource: manifest.default_chat.clone(),
            title: manifest.title.clone(),
            status: 1,
            activity: None,
            modified_at: manifest.created_at.clone(),
            origin: Some(ChatOrigin::User),
            interactivity: None,
            working_directories: None,
        }],
        default_chat: Some(manifest.default_chat.clone()),

        config: Some(ahp_types::state::SessionConfigState {
            schema: session_config_schema(),
            values: {
                let mut values = session_config_values(None);
                if let Some(mode) = &manifest.permission_mode {
                    values.insert("permissionMode".to_owned(), serde_json::json!(mode));
                }
                values.insert("worktree".to_owned(), serde_json::json!(manifest.worktree));

                if let Some(model) = &manifest.model {
                    values.insert("model".to_owned(), serde_json::json!(model));
                }
                if let Some(level) = &manifest.thinking_level {
                    values.insert("thinkingLevel".to_owned(), serde_json::json!(level));
                }
                values
            },
        }),
        customizations: None,

        changesets: Some(crate::changes::catalog(&manifest.working_directories)),
        input_needed: None,
        meta: None,
    }
}

fn string_of(config: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    config.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn summary(manifest: &Manifest, state: &SessionState) -> SessionSummary {
    SessionSummary {
        provider: manifest.provider.clone(),
        title: state.title.clone(),
        status: state.status,
        activity: state.activity.clone(),
        project: None,
        working_directories: Some(manifest.working_directories.clone()),
        annotations: state.annotations.clone(),
        resource: manifest.session.clone(),
        created_at: manifest.created_at.clone(),
        modified_at: manifest.created_at.clone(),
        changes: None,
        meta: None,
    }
}

fn cli_summary(session: &crate::catalog::CliSession) -> SessionSummary {
    let stamp = humantime::format_rfc3339_millis(session.modified_at).to_string();
    SessionSummary {
        provider: session.provider.clone(),
        title: session.title.clone(),
        status: 1,
        activity: None,
        project: None,
        working_directories: session
            .cwd
            .as_ref()
            .map(|cwd| vec![crate::uris::file_uri(std::path::Path::new(&cwd))]),
        annotations: None,
        resource: format!("ahp-session:/{}", session.storage_id),
        created_at: stamp.clone(),
        modified_at: stamp,
        changes: None,
        meta: None,
    }
}

fn summary_of(manifest: &Manifest) -> SessionSummary {
    SessionSummary {
        provider: manifest.provider.clone(),
        title: manifest.title.clone(),
        status: 1,
        activity: None,
        project: None,
        working_directories: Some(manifest.working_directories.clone()),
        annotations: Some(annotations_summary(
            &manifest.session,
            &manifest.annotations,
        )),
        resource: manifest.session.clone(),
        created_at: manifest.created_at.clone(),
        modified_at: manifest.created_at.clone(),
        changes: None,
        meta: None,
    }
}

fn tail_window(full: &ChatState) -> ChatState {
    if full.turns.len() <= TURN_TAIL {
        return full.clone();
    }
    let start = full.turns.len() - TURN_TAIL;
    let mut window = full.clone();
    window.turns = full.turns[start..].to_vec();
    window.turns_next_cursor = Some(start.to_string());
    window
}

fn now_rfc3339() -> String {
    humantime::format_rfc3339_millis(std::time::SystemTime::now()).to_string()
}

fn trim_scrollback(state: &mut TerminalState) {
    const CAP: usize = 256 * 1024;
    let mut total: usize = state.content.iter().map(part_len).sum();
    while total > CAP {
        let Some(first) = state.content.first_mut() else {
            break;
        };
        let excess = total - CAP;
        let len = part_len(first);
        if len <= excess {
            state.content.remove(0);
            total -= len;
        } else {
            match first {
                TerminalContentPart::Unclassified(part) => drain_front(&mut part.value, excess),
                TerminalContentPart::Command(part) => drain_front(&mut part.output, excess),
                _ => break,
            }
            total -= excess;
        }
    }
}

fn part_len(part: &TerminalContentPart) -> usize {
    match part {
        TerminalContentPart::Unclassified(part) => part.value.len(),
        TerminalContentPart::Command(part) => part.output.len(),
        _ => 0,
    }
}

fn drain_front(text: &mut String, bytes: usize) {
    let mut cut = bytes.min(text.len());
    while cut < text.len() && !text.is_char_boundary(cut) {
        cut += 1;
    }
    text.drain(..cut);
}

fn base64_answer(path: &std::path::Path, bytes: &[u8]) -> Value {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    serde_json::json!({
        "data": data,
        "encoding": "base64",
        "contentType": content_type(path),
    })
}

fn content_type(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn file_path(uri: &Value) -> Option<PathBuf> {
    crate::uris::file_path(uri.as_str()?)
}

fn local_path(uri: &Uri) -> PathBuf {
    crate::uris::file_path(uri).unwrap_or_else(|| PathBuf::from(uri))
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn a_panicked_update_never_bricks_the_state() {
        let host = Host::new(HostConfig::default());
        host.update(|state| state.server_seq += 1);
        let before = host.snapshot().server_seq;

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            host.update(|_| panic!("a handler died mid-derivation"));
        }));
        assert!(panicked.is_err());

        assert_eq!(
            host.snapshot().server_seq,
            before,
            "the last completed state stands"
        );
        host.update(|state| state.server_seq += 1);
        assert_eq!(
            host.snapshot().server_seq,
            before + 1,
            "the host keeps serving"
        );
    }
}
