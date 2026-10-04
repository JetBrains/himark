// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use ahp_types::actions::StateAction;
use ahp_types::common::Uri;
use ahp_types::notifications::PartialSessionSummary;
use ahp_types::state::{AgentInfo, ChatState, SessionState, SessionSummary};
pub use himark_ahp_ext_types::{
    DocumentApplied, DocumentState, OpenDocumentResult, SearchKind, SearchResult, SearchTarget,
};
use imba::store::Store;

#[derive(Clone, Debug)]
pub struct TurnsPage {
    pub turns: Vec<ahp_types::state::Turn>,

    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FileEditContents {
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct HostId(u64);

impl HostId {
    pub const LOCAL: HostId = HostId(0);

    fn raw(self) -> u64 {
        self.0
    }

    fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

pub type ClientFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Clone, Debug)]
pub enum ServerEvent {
    SessionAdded(SessionSummary),
    SessionRemoved(SessionUri),

    SessionChanged {
        session: SessionUri,
        changes: PartialSessionSummary,
    },
    AgentsChanged(Vec<AgentInfo>),
}

#[derive(Clone, Debug)]
pub struct RootInfo {
    pub agents: Vec<AgentInfo>,
}

#[derive(Clone, Debug)]
pub struct SessionsPage {
    pub sessions: Vec<SessionSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Default)]
pub struct SessionOptions {
    pub provider: Option<String>,
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    pub model: Option<ahp_types::state::ModelSelection>,
}

/// The session facet: connection, catalog and session lifecycle,
/// the session channel, and channel-action dispatch. The old
/// all-knowing `AhpServer` trait is burned — a caller holds the
/// facet it drives, never the entire session.
pub trait SessionClient: Send + Sync + 'static {
    fn connect(&self) -> ClientFuture<Result<RootInfo, String>>;
    fn list_sessions(&self, cursor: Option<String>) -> ClientFuture<Result<SessionsPage, String>>;

    fn poll_root(&self) -> ClientFuture<Vec<ServerEvent>>;

    fn create_session(
        &self,
        working_directories: Vec<Uri>,
        options: SessionOptions,
    ) -> ClientFuture<Result<SessionUri, String>>;

    fn resolve_session_config(
        &self,
        working_directory: Option<Uri>,
        config: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> ClientFuture<Result<ahp_types::commands::ResolveSessionConfigResult, String>>;
    fn dispose_session(&self, session: SessionUri) -> ClientFuture<Result<(), String>>;

    fn http_serve(&self) -> ClientFuture<Result<String, String>> {
        Box::pin(std::future::ready(Err(
            "this host cannot serve over http".to_owned()
        )))
    }

    fn subscribe_session(&self, session: SessionUri) -> ClientFuture<Result<SessionState, String>>;
    fn poll_session(&self, session: SessionUri) -> ClientFuture<Vec<StateAction>>;

    fn dispatch_action(
        &self,
        channel: ChannelUri,
        action: StateAction,
    ) -> ClientFuture<Result<(), String>>;
}

/// The chat facet: chats, turns, and the file-edit reads a chat's
/// diff cells are built from.
pub trait ChatClient: Send + Sync + 'static {
    fn create_chat(&self, session: SessionUri) -> ClientFuture<Result<ChatUri, String>>;

    fn subscribe_chat(&self, chat: ChatUri) -> ClientFuture<Result<ChatState, String>>;
    fn fetch_turns(
        &self,
        chat: ChatUri,
        cursor: Option<String>,
    ) -> ClientFuture<Result<TurnsPage, String>>;

    fn start_turn(
        &self,
        chat: ChatUri,
        text: String,
        attachments: Option<Vec<ahp_types::state::MessageAttachment>>,
        model: Option<ahp_types::state::ModelSelection>,
    ) -> ClientFuture<Result<(), String>>;
    fn poll_chat(&self, chat: ChatUri) -> ClientFuture<Vec<StateAction>>;
    fn cancel_turn(&self, chat: ChatUri, turn_id: TurnId) -> ClientFuture<()>;

    fn read_file_edit(
        &self,
        before: Option<Uri>,
        after: Option<Uri>,
    ) -> ClientFuture<Result<FileEditContents, String>>;
}

/// The resource facet: the session's filesystem — reads, writes,
/// listings, watches, and the quick-open path find.
pub trait ResourceClient: Send + Sync + 'static {
    fn resource_read(&self, session: SessionUri, uri: ResourceUri) -> ClientFuture<Option<String>>;

    fn resource_read_bytes(
        &self,
        session: SessionUri,
        uri: ResourceUri,
    ) -> ClientFuture<Option<Vec<u8>>> {
        let _ = (session, uri);
        Box::pin(std::future::ready(None))
    }

    fn resource_write(
        &self,
        session: SessionUri,
        uri: ResourceUri,
        text: String,
    ) -> ClientFuture<bool>;

    /// Creates an empty file; never overwrites — false when the
    /// resource already exists.
    fn resource_create(&self, session: SessionUri, uri: ResourceUri) -> ClientFuture<bool> {
        let _ = (session, uri);
        Box::pin(std::future::ready(false))
    }

    fn resource_delete(
        &self,
        session: SessionUri,
        uri: ResourceUri,
        recursive: bool,
    ) -> ClientFuture<bool> {
        let _ = (session, uri, recursive);
        Box::pin(std::future::ready(false))
    }

    /// A rename: fails when the destination exists.
    fn resource_move(
        &self,
        session: SessionUri,
        from: ResourceUri,
        to: ResourceUri,
    ) -> ClientFuture<bool> {
        let _ = (session, from, to);
        Box::pin(std::future::ready(false))
    }

    fn resource_list(
        &self,
        session: SessionUri,
        uri: ResourceUri,
    ) -> ClientFuture<Option<Vec<(String, bool)>>>;

    fn resource_watch(
        &self,
        session: SessionUri,
        uri: ResourceUri,
        events: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> ClientFuture<Option<WatchHandle>>;

    fn resource_unwatch(&self, handle: WatchHandle) -> ClientFuture<()>;

    fn search(&self, session: SessionUri, ask: SearchAsk) -> ClientFuture<Option<SearchResult>>;
}

/// The locations facet: the streaming `ahp-locations:/…` channels —
/// content search and the location-answering LSP asks.
pub trait LocationsClient: Send + Sync + 'static {
    /// locations@1 `searchLocations`: answers the minted
    /// `ahp-locations:/…` channel; results stream as channel actions;
    /// unsubscribing cancels the walk (docs/ahp/ahp-locations.md).
    fn search_locations(
        &self,
        session: SessionUri,
        ask: LocationsAsk,
    ) -> ClientFuture<Result<ChannelUri, String>> {
        let _ = (session, ask);
        Box::pin(std::future::ready(Err(
            "locations@1 searchLocations not served".to_owned(),
        )))
    }

    /// locations@1 `lsp/locations`: the location-answering LSP asks
    /// (references, implementations), streamed the same way.
    fn lsp_locations(
        &self,
        session: SessionUri,
        method: String,
        params: serde_json::Value,
    ) -> ClientFuture<Result<ChannelUri, String>> {
        let _ = (session, method, params);
        Box::pin(std::future::ready(Err(
            "locations@1 lsp/locations not served".to_owned(),
        )))
    }

    fn subscribe_locations(
        &self,
        channel: ChannelUri,
    ) -> ClientFuture<Result<himark_ahp_ext_types::LocationList, String>> {
        let _ = channel;
        Box::pin(std::future::ready(Err("locations@1 not served".to_owned())))
    }

    /// Typed poll: only `locations/extend` bodies come back, in
    /// arrival order. A client that never served the subscribe is
    /// never polled.
    fn poll_locations(
        &self,
        channel: ChannelUri,
    ) -> ClientFuture<Vec<himark_ahp_ext_types::LocationList>> {
        let _ = channel;
        Box::pin(std::future::pending())
    }

    /// The cancel: dropping the last subscription disposes the
    /// channel and stops its producer host-side.
    fn unsubscribe_locations(&self, channel: &ChannelUri) {
        let _ = channel;
    }
}

/// The terminal facet: PTY channels and their event pumps.
pub trait TerminalClient: Send + Sync + 'static {
    fn terminal_open(
        &self,
        session: SessionUri,
        channel: ChannelUri,
        cwd: Option<Uri>,
        cols: u16,
        rows: u16,
        events: Arc<dyn Fn(TerminalEvent) + Send + Sync>,
    ) -> ClientFuture<Option<TerminalHandle>>;

    fn terminal_input(&self, channel: &ChannelUri, data: String);

    fn terminal_resize(&self, channel: &ChannelUri, cols: u16, rows: u16);

    fn terminal_dispose(&self, channel: &ChannelUri);
}

/// The changes facet: the changeset channel.
pub trait ChangesClient: Send + Sync + 'static {
    fn subscribe_changeset(
        &self,
        channel: ChannelUri,
    ) -> ClientFuture<Result<ahp_types::state::ChangesetState, String>>;

    fn poll_changeset(&self, channel: ChannelUri) -> ClientFuture<Vec<StateAction>>;

    fn unsubscribe_changeset(&self, channel: &ChannelUri);
}

/// The history facet: the commit-history channel (history@1).
pub trait HistoryClient: Send + Sync + 'static {
    fn subscribe_history(
        &self,
        channel: ChannelUri,
    ) -> ClientFuture<Result<himark_ahp_ext_types::history::HistoryState, String>> {
        let _ = channel;
        Box::pin(async { Err("history@1 not served".to_owned()) })
    }
}

/// The annotations facet: the comments channel.
pub trait AnnotationsClient: Send + Sync + 'static {
    fn subscribe_annotations(
        &self,
        session: SessionUri,
    ) -> ClientFuture<Result<ahp_types::state::AnnotationsState, String>>;

    fn poll_annotations(&self, session: SessionUri) -> ClientFuture<Vec<StateAction>>;

    fn dispatch_annotations(&self, session: &SessionUri, action: StateAction);

    fn unsubscribe_annotations(&self, session: &SessionUri);
}

/// The docsync facet: documents@1 — mirrored documents and their
/// operation channels.
pub trait DocumentsClient: Send + Sync + 'static {
    fn open_document(
        &self,
        session: SessionUri,
        uri: Option<ResourceUri>,
        text: Option<String>,
    ) -> ClientFuture<Result<himark_ahp_ext_types::OpenDocumentResult, String>>;

    fn subscribe_document(
        &self,
        channel: ChannelUri,
    ) -> ClientFuture<Result<himark_ahp_ext_types::DocumentState, String>>;

    fn poll_document(
        &self,
        channel: ChannelUri,
    ) -> ClientFuture<Vec<himark_ahp_ext_types::DocumentApplied>>;

    fn dispatch_document(
        &self,
        channel: &ChannelUri,
        action: himark_ahp_ext_types::DocumentApplied,
    );

    /// documents@1 storeDocument: the host dumps the mirror — its own
    /// text, the source of truth — to the resource.
    fn store_document(
        &self,
        channel: ChannelUri,
        uri: ResourceUri,
    ) -> ClientFuture<Result<(), String>> {
        let _ = (channel, uri);
        Box::pin(std::future::ready(Err(
            "documents@1 storeDocument not served".to_owned(),
        )))
    }

    /// Resolves once the host has dropped the subscription (or the
    /// connection is dead, which drops it with the connection) — the
    /// caller can order a fresh subscribe strictly AFTER it.
    fn unsubscribe_document(&self, channel: &ChannelUri) -> ClientFuture<()>;
}

/// The LSP facet: the raw lsp@1 pass-through.
pub trait LspClient: Send + Sync + 'static {
    fn lsp(
        &self,
        session: SessionUri,
        method: String,
        params: serde_json::Value,
    ) -> ClientFuture<Result<serde_json::Value, String>>;
}

/// One session's channel digest: the provider, chats, working
/// directories and config the session feed has landed so far — pure
/// wire data, held by the catalog, read by the chat through its
/// ceremony-wired roads.
#[derive(Clone, Default)]
pub struct SessionChannel {
    pub provider: String,
    pub chats: rpds::VectorSync<ahp_types::state::ChatSummary>,
    pub default_chat: Option<ChatUri>,

    pub working_directories: rpds::VectorSync<String>,

    pub config: Option<Arc<ahp_types::state::SessionConfigState>>,
}

/// One host's client, faceted: every protocol domain holds only its
/// slice. The bundle is ten `Arc`s onto (usually) one implementation;
/// cloning it is pointer bumps.
#[derive(Clone)]
pub struct Client {
    pub session: Arc<dyn SessionClient>,
    pub resources: Arc<dyn ResourceClient>,
    pub terminals: Arc<dyn TerminalClient>,
    pub changes: Arc<dyn ChangesClient>,
    pub history: Arc<dyn HistoryClient>,
    pub annotations: Arc<dyn AnnotationsClient>,
    pub locations: Arc<dyn LocationsClient>,
    pub documents: Arc<dyn DocumentsClient>,
    pub lsp: Arc<dyn LspClient>,
    pub chat: Arc<dyn ChatClient>,
}

impl Client {
    /// The whole-protocol client: one implementation (the wire, a full
    /// test host) serving every facet.
    pub fn of<T>(client: Arc<T>) -> Self
    where
        T: SessionClient
            + ResourceClient
            + TerminalClient
            + ChangesClient
            + HistoryClient
            + AnnotationsClient
            + LocationsClient
            + DocumentsClient
            + LspClient
            + ChatClient,
    {
        Client {
            session: Arc::clone(&client) as Arc<dyn SessionClient>,
            resources: Arc::clone(&client) as Arc<dyn ResourceClient>,
            terminals: Arc::clone(&client) as Arc<dyn TerminalClient>,
            changes: Arc::clone(&client) as Arc<dyn ChangesClient>,
            history: Arc::clone(&client) as Arc<dyn HistoryClient>,
            annotations: Arc::clone(&client) as Arc<dyn AnnotationsClient>,
            locations: Arc::clone(&client) as Arc<dyn LocationsClient>,
            documents: Arc::clone(&client) as Arc<dyn DocumentsClient>,
            lsp: Arc::clone(&client) as Arc<dyn LspClient>,
            chat: client,
        }
    }
}

#[derive(Debug)]
pub enum TerminalEvent {
    Data(String),
    Exited(Option<i32>),
}

#[derive(Clone, Debug)]
pub struct TerminalHandle {
    pub channel: ChannelUri,
}

#[derive(Clone, Debug)]
pub struct SearchAsk {
    pub folders: Vec<Uri>,
    pub query: String,
    pub kind: SearchKind,
    pub case_sensitive: bool,
    pub target: SearchTarget,
    pub limit: usize,
}

/// The `searchLocations` ask — content search only, so no target;
/// `kind` is text or regex (a fuzzy ask is refused by the host).
#[derive(Clone, Debug)]
pub struct LocationsAsk {
    pub folders: Vec<Uri>,
    pub query: String,
    pub kind: SearchKind,
    pub case_sensitive: bool,
    pub limit: usize,
}

#[derive(Clone, Default)]
pub struct Servers {
    clients: rpds::HashTrieMapSync<HostId, Client>,
    order: rpds::VectorSync<HostId>,
    next: u64,
}

impl Servers {
    pub fn mint(&mut self, client: Client) -> HostId {
        self.next += 1;
        let minted = HostId(self.next);
        self.clients.insert_mut(minted, client);
        self.order.push_back_mut(minted);
        minted
    }

    pub fn client(store: &Store, id: HostId) -> Option<Client> {
        store.get::<Servers>()?.clients.get(&id).cloned()
    }

    pub fn list(store: &Store) -> Vec<HostId> {
        store
            .get::<Servers>()
            .map(|servers| servers.order.iter().copied().collect())
            .unwrap_or_default()
    }
}

const PREFIX: &str = "ahp:";

pub fn authority(server: HostId, session: &SessionUri) -> String {
    format!("{PREFIX}{}:{}", server.raw(), session)
}

pub fn parse(authority: &str) -> Option<(HostId, SessionUri)> {
    let rest = authority.strip_prefix(PREFIX)?;
    let (server, session) = rest.split_once(':')?;
    Some((
        HostId::from_raw(server.parse().ok()?),
        SessionUri::new(session),
    ))
}

pub fn scoped(authority: &str) -> bool {
    authority.starts_with(PREFIX)
}

#[derive(Clone, Copy, Default)]
pub struct LocalHost(pub Option<HostId>);

pub fn route(store: &Store, authority: &str) -> Option<(HostId, SessionUri)> {
    if scoped(authority) {
        return parse(authority);
    }
    if authority == "local" {
        let host = store.get::<LocalHost>()?.0?;
        return Some((host, SessionUri::new(crate::LOCAL_FS_SESSION)));
    }
    None
}

pub fn route_client(store: &Store, authority: &str) -> Option<(HostId, Client, SessionUri)> {
    let (host, session) = route(store, authority)?;
    let client = Servers::client(store, host)?;
    Some((host, client, session))
}

pub fn route_authority(host: HostId, session: &SessionUri) -> editor::location::Authority {
    if session.as_str() == crate::LOCAL_FS_SESSION {
        editor::location::Authority::new("local")
    } else {
        editor::location::Authority::new(authority(host, session))
    }
}

#[derive(Clone, Debug)]
pub struct WatchHandle {
    pub channel: ChannelUri,
}

/// An AHP session URI (`ahp-session:/<uuid>`, `hihost-fs:/local`).
/// (A REAL newtype — the wire's `Uri` is a bare `type Uri = String`
/// alias, no protection at all.)
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct SessionUri(String);

impl SessionUri {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl SessionUri {
    /// A session URI IS a subscribable channel (session snapshots and
    /// actions ride it) — the explicit bridge, so the conversion reads
    /// as intent instead of a stringly cast.
    pub fn as_channel(&self) -> ChannelUri {
        ChannelUri::new(self.0.clone())
    }
}

impl std::fmt::Display for SessionUri {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}

impl From<String> for SessionUri {
    fn from(raw: String) -> Self {
        Self(raw)
    }
}

impl From<&str> for SessionUri {
    fn from(raw: &str) -> Self {
        Self(raw.to_owned())
    }
}

/// A subscribable channel URI (changesets, history, terminals, locations, documents).
/// (A REAL newtype — the wire's `Uri` is a bare `type Uri = String`
/// alias, no protection at all.)
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct ChannelUri(String);

impl ChannelUri {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for ChannelUri {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}

impl From<String> for ChannelUri {
    fn from(raw: String) -> Self {
        Self(raw)
    }
}

impl From<&str> for ChannelUri {
    fn from(raw: &str) -> Self {
        Self(raw.to_owned())
    }
}

/// A chat URI (`ahp-chat:/<uuid>`).
/// (A REAL newtype — the wire's `Uri` is a bare `type Uri = String`
/// alias, no protection at all.)
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct ChatUri(String);

impl ChatUri {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl ChatUri {
    /// A chat URI IS a dispatch channel (turn actions ride it).
    pub fn as_channel(&self) -> ChannelUri {
        ChannelUri::new(self.0.clone())
    }
}

impl std::fmt::Display for ChatUri {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}

impl From<String> for ChatUri {
    fn from(raw: String) -> Self {
        Self(raw)
    }
}

impl From<&str> for ChatUri {
    fn from(raw: &str) -> Self {
        Self(raw.to_owned())
    }
}

/// A turn id within a chat.
/// (A REAL newtype — the wire's `Uri` is a bare `type Uri = String`
/// alias, no protection at all.)
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct TurnId(String);

impl TurnId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for TurnId {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}

impl From<String> for TurnId {
    fn from(raw: String) -> Self {
        Self(raw)
    }
}

/// Keyed lookups take a wire `&str` without minting an id — the
/// derived `Hash` is the inner `String`'s, which is `str`'s, so the
/// `Borrow` contract holds.
impl std::borrow::Borrow<str> for TurnId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<&str> for TurnId {
    fn from(raw: &str) -> Self {
        Self(raw.to_owned())
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ResourceUri(String);

impl ResourceUri {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for ResourceUri {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}

pub trait ResourceUriMap: Send + Sync + 'static {
    fn uri_of(&self, location: &editor::location::ResourceLocation) -> ResourceUri;

    fn location_of(
        &self,
        uri: &ResourceUri,
        kind: editor::location::ResourceType,
        authority: &editor::location::Authority,
    ) -> Option<editor::location::ResourceLocation>;
}

/// The client mirror tests mint hosts with: every facet answers
/// `unreachable!`. A test scripting one domain overrides that facet
/// alone — `Client { chat: scripted, ..client::inert() }` — instead of
/// mocking the entire protocol.
#[cfg(any(test, feature = "test-support"))]
pub fn inert() -> Client {
    pub struct Inert;

    macro_rules! unreached {
        ($($name:ident($($arg:ident: $ty:ty),*) -> $out:ty;)*) => {
            $(fn $name(&self, $($arg: $ty),*) -> $out {
                $(let _ = $arg;)*
                unreachable!("the inert client is never reached")
            })*
        };
    }

    impl SessionClient for Inert {
        unreached! {
            connect() -> ClientFuture<Result<RootInfo, String>>;
            list_sessions(cursor: Option<String>) -> ClientFuture<Result<SessionsPage, String>>;
            poll_root() -> ClientFuture<Vec<ServerEvent>>;
            create_session(dirs: Vec<Uri>, options: SessionOptions) -> ClientFuture<Result<SessionUri, String>>;
            resolve_session_config(working_directory: Option<Uri>, config: Option<serde_json::Map<String, serde_json::Value>>) -> ClientFuture<Result<ahp_types::commands::ResolveSessionConfigResult, String>>;
            dispose_session(session: SessionUri) -> ClientFuture<Result<(), String>>;
            subscribe_session(session: SessionUri) -> ClientFuture<Result<SessionState, String>>;
            poll_session(session: SessionUri) -> ClientFuture<Vec<StateAction>>;
            dispatch_action(channel: ChannelUri, action: StateAction) -> ClientFuture<Result<(), String>>;
        }
    }

    impl ChatClient for Inert {
        unreached! {
            create_chat(session: SessionUri) -> ClientFuture<Result<ChatUri, String>>;
            subscribe_chat(chat: ChatUri) -> ClientFuture<Result<ChatState, String>>;
            fetch_turns(chat: ChatUri, cursor: Option<String>) -> ClientFuture<Result<TurnsPage, String>>;
            start_turn(chat: ChatUri, text: String, attachments: Option<Vec<ahp_types::state::MessageAttachment>>, model: Option<ahp_types::state::ModelSelection>) -> ClientFuture<Result<(), String>>;
            poll_chat(chat: ChatUri) -> ClientFuture<Vec<StateAction>>;
            cancel_turn(chat: ChatUri, turn_id: TurnId) -> ClientFuture<()>;
            read_file_edit(before: Option<Uri>, after: Option<Uri>) -> ClientFuture<Result<FileEditContents, String>>;
        }
    }

    impl ResourceClient for Inert {
        unreached! {
            resource_read(session: SessionUri, uri: ResourceUri) -> ClientFuture<Option<String>>;
            resource_write(session: SessionUri, uri: ResourceUri, text: String) -> ClientFuture<bool>;
            resource_list(session: SessionUri, uri: ResourceUri) -> ClientFuture<Option<Vec<(String, bool)>>>;
            resource_watch(session: SessionUri, uri: ResourceUri, events: Arc<dyn Fn() + Send + Sync>) -> ClientFuture<Option<WatchHandle>>;
            resource_unwatch(handle: WatchHandle) -> ClientFuture<()>;
            search(session: SessionUri, ask: SearchAsk) -> ClientFuture<Option<SearchResult>>;
        }
    }

    impl LocationsClient for Inert {}

    impl TerminalClient for Inert {
        unreached! {
            terminal_input(channel: &ChannelUri, data: String) -> ();
            terminal_resize(channel: &ChannelUri, cols: u16, rows: u16) -> ();
            terminal_dispose(channel: &ChannelUri) -> ();
        }

        fn terminal_open(
            &self,
            _session: SessionUri,
            _channel: ChannelUri,
            _cwd: Option<Uri>,
            _cols: u16,
            _rows: u16,
            _events: Arc<dyn Fn(TerminalEvent) + Send + Sync>,
        ) -> ClientFuture<Option<TerminalHandle>> {
            unreachable!("the inert client is never reached")
        }
    }

    impl ChangesClient for Inert {
        unreached! {
            subscribe_changeset(channel: ChannelUri) -> ClientFuture<Result<ahp_types::state::ChangesetState, String>>;
            poll_changeset(channel: ChannelUri) -> ClientFuture<Vec<StateAction>>;
            unsubscribe_changeset(channel: &ChannelUri) -> ();
        }
    }

    impl HistoryClient for Inert {}

    impl AnnotationsClient for Inert {
        unreached! {
            subscribe_annotations(session: SessionUri) -> ClientFuture<Result<ahp_types::state::AnnotationsState, String>>;
            poll_annotations(session: SessionUri) -> ClientFuture<Vec<StateAction>>;
            dispatch_annotations(session: &SessionUri, action: StateAction) -> ();
            unsubscribe_annotations(session: &SessionUri) -> ();
        }
    }

    impl DocumentsClient for Inert {
        unreached! {
            open_document(session: SessionUri, uri: Option<ResourceUri>, text: Option<String>) -> ClientFuture<Result<himark_ahp_ext_types::OpenDocumentResult, String>>;
            subscribe_document(channel: ChannelUri) -> ClientFuture<Result<himark_ahp_ext_types::DocumentState, String>>;
            poll_document(channel: ChannelUri) -> ClientFuture<Vec<himark_ahp_ext_types::DocumentApplied>>;
            dispatch_document(channel: &ChannelUri, action: himark_ahp_ext_types::DocumentApplied) -> ();
            unsubscribe_document(channel: &ChannelUri) -> ClientFuture<()>;
        }
    }

    impl LspClient for Inert {
        unreached! {
            lsp(session: SessionUri, method: String, params: serde_json::Value) -> ClientFuture<Result<serde_json::Value, String>>;
        }
    }

    Client::of(Arc::new(Inert))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorities_round_trip_and_gate() {
        let session = SessionUri::new("claude:/abc-123");
        let server = HostId(7);
        let encoded = authority(server, &session);
        assert!(scoped(&encoded));
        assert!(!scoped("local"));
        assert!(!scoped("scratch"));
        let (parsed_server, parsed_session) = parse(&encoded).expect("parses");
        assert_eq!(parsed_server, server);
        assert_eq!(
            parsed_session, session,
            "the session uri's own colons survive"
        );
        assert!(parse("local").is_none());
    }
}
