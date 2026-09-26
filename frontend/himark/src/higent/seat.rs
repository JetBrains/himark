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

use crate::higent::effects::{FileEditContents, TurnsPage};

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

pub type SeatFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Debug)]
pub enum ServerEvent {
    SessionAdded(SessionSummary),
    SessionRemoved(SessionUri),

    SessionChanged {
        session: SessionUri,
        changes: PartialSessionSummary,
    },
    AgentsChanged(Vec<AgentInfo>),
}

#[derive(Debug)]
pub struct RootInfo {
    pub agents: Vec<AgentInfo>,
}

#[derive(Debug)]
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

pub trait AhpServer: Send + Sync + 'static {
    fn connect(&self) -> SeatFuture<Result<RootInfo, String>>;
    fn list_sessions(&self, cursor: Option<String>) -> SeatFuture<Result<SessionsPage, String>>;

    fn poll_root(&self) -> SeatFuture<Vec<ServerEvent>>;

    fn create_session(
        &self,
        working_directories: Vec<Uri>,
        options: SessionOptions,
    ) -> SeatFuture<Result<SessionUri, String>>;

    fn resolve_session_config(
        &self,
        working_directory: Option<Uri>,
        config: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> SeatFuture<Result<ahp_types::commands::ResolveSessionConfigResult, String>>;
    fn dispose_session(&self, session: SessionUri) -> SeatFuture<Result<(), String>>;

    fn http_serve(&self) -> SeatFuture<Result<String, String>> {
        Box::pin(std::future::ready(Err(
            "this host cannot serve over http".to_owned()
        )))
    }

    fn subscribe_session(&self, session: SessionUri) -> SeatFuture<Result<SessionState, String>>;
    fn poll_session(&self, session: SessionUri) -> SeatFuture<Vec<StateAction>>;

    fn create_chat(&self, session: SessionUri) -> SeatFuture<Result<ChatUri, String>>;

    fn subscribe_chat(&self, chat: ChatUri) -> SeatFuture<Result<ChatState, String>>;
    fn fetch_turns(
        &self,
        chat: ChatUri,
        cursor: Option<String>,
    ) -> SeatFuture<Result<TurnsPage, String>>;

    fn start_turn(
        &self,
        chat: ChatUri,
        text: String,
        attachments: Option<Vec<ahp_types::state::MessageAttachment>>,
        model: Option<ahp_types::state::ModelSelection>,
    ) -> SeatFuture<Result<(), String>>;
    fn poll_chat(&self, chat: ChatUri) -> SeatFuture<Vec<StateAction>>;
    fn cancel_turn(&self, chat: ChatUri, turn_id: TurnId) -> SeatFuture<()>;

    fn dispatch_action(
        &self,
        channel: ChannelUri,
        action: StateAction,
    ) -> SeatFuture<Result<(), String>>;
    fn read_file_edit(
        &self,
        before: Option<Uri>,
        after: Option<Uri>,
    ) -> SeatFuture<Result<FileEditContents, String>>;

    fn resource_read(&self, session: SessionUri, uri: ResourceUri) -> SeatFuture<Option<String>>;

    fn resource_read_bytes(
        &self,
        session: SessionUri,
        uri: ResourceUri,
    ) -> SeatFuture<Option<Vec<u8>>> {
        let _ = (session, uri);
        Box::pin(std::future::ready(None))
    }

    fn resource_write(
        &self,
        session: SessionUri,
        uri: ResourceUri,
        text: String,
    ) -> SeatFuture<bool>;

    fn resource_list(
        &self,
        session: SessionUri,
        uri: ResourceUri,
    ) -> SeatFuture<Option<Vec<(String, bool)>>>;

    fn resource_watch(
        &self,
        session: SessionUri,
        uri: ResourceUri,
        events: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> SeatFuture<Option<WatchHandle>>;

    fn resource_unwatch(&self, handle: WatchHandle) -> SeatFuture<()>;

    fn search(&self, session: SessionUri, ask: SearchAsk) -> SeatFuture<Option<SearchResult>>;

    /// locations@1 `searchLocations`: answers the minted
    /// `ahp-locations:/…` channel; results stream as channel actions;
    /// unsubscribing cancels the walk (docs/ahp/ahp-locations.md).
    fn search_locations(
        &self,
        session: SessionUri,
        ask: LocationsAsk,
    ) -> SeatFuture<Result<ChannelUri, String>> {
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
    ) -> SeatFuture<Result<ChannelUri, String>> {
        let _ = (session, method, params);
        Box::pin(std::future::ready(Err(
            "locations@1 lsp/locations not served".to_owned(),
        )))
    }

    fn subscribe_locations(
        &self,
        channel: ChannelUri,
    ) -> SeatFuture<Result<himark_ahp_ext_types::LocationList, String>> {
        let _ = channel;
        Box::pin(std::future::ready(Err("locations@1 not served".to_owned())))
    }

    /// Typed poll: only `locations/extend` bodies come back, in
    /// arrival order. A seat that never served the subscribe is
    /// never polled.
    fn poll_locations(
        &self,
        channel: ChannelUri,
    ) -> SeatFuture<Vec<himark_ahp_ext_types::LocationList>> {
        let _ = channel;
        Box::pin(std::future::pending())
    }

    /// The cancel: dropping the last subscription disposes the
    /// channel and stops its producer host-side.
    fn unsubscribe_locations(&self, channel: &ChannelUri) {
        let _ = channel;
    }

    fn terminal_open(
        &self,
        session: SessionUri,
        channel: ChannelUri,
        cwd: Option<Uri>,
        cols: u16,
        rows: u16,
        events: Arc<dyn Fn(TerminalEvent) + Send + Sync>,
    ) -> SeatFuture<Option<TerminalHandle>>;

    fn terminal_input(&self, channel: &ChannelUri, data: String);

    fn terminal_resize(&self, channel: &ChannelUri, cols: u16, rows: u16);

    fn terminal_dispose(&self, channel: &ChannelUri);

    fn subscribe_changeset(
        &self,
        channel: ChannelUri,
    ) -> SeatFuture<Result<ahp_types::state::ChangesetState, String>>;

    fn poll_changeset(&self, channel: ChannelUri) -> SeatFuture<Vec<StateAction>>;

    fn unsubscribe_changeset(&self, channel: &ChannelUri);

    fn subscribe_history(
        &self,
        channel: ChannelUri,
    ) -> SeatFuture<Result<himark_ahp_ext_types::history::HistoryState, String>> {
        let _ = channel;
        Box::pin(async { Err("history@1 not served".to_owned()) })
    }

    fn subscribe_annotations(
        &self,
        session: SessionUri,
    ) -> SeatFuture<Result<ahp_types::state::AnnotationsState, String>>;

    fn poll_annotations(&self, session: SessionUri) -> SeatFuture<Vec<StateAction>>;

    fn dispatch_annotations(&self, session: &SessionUri, action: StateAction);

    fn unsubscribe_annotations(&self, session: &SessionUri);

    fn open_document(
        &self,
        session: SessionUri,
        uri: Option<ResourceUri>,
        text: Option<String>,
    ) -> SeatFuture<Result<himark_ahp_ext_types::OpenDocumentResult, String>>;

    fn subscribe_document(
        &self,
        channel: ChannelUri,
    ) -> SeatFuture<Result<himark_ahp_ext_types::DocumentState, String>>;

    fn poll_document(
        &self,
        channel: ChannelUri,
    ) -> SeatFuture<Vec<himark_ahp_ext_types::DocumentApplied>>;

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
    ) -> SeatFuture<Result<(), String>> {
        let _ = (channel, uri);
        Box::pin(std::future::ready(Err(
            "documents@1 storeDocument not served".to_owned(),
        )))
    }

    /// Resolves once the host has dropped the subscription (or the
    /// connection is dead, which drops it with the connection) — the
    /// caller can order a fresh subscribe strictly AFTER it.
    fn unsubscribe_document(&self, channel: &ChannelUri) -> SeatFuture<()>;

    fn lsp(
        &self,
        session: SessionUri,
        method: String,
        params: serde_json::Value,
    ) -> SeatFuture<Result<serde_json::Value, String>>;
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
    seats: rpds::HashTrieMapSync<HostId, Arc<dyn AhpServer>>,
    order: rpds::VectorSync<HostId>,
    next: u64,
}

impl Servers {
    pub(crate) fn mint(&mut self, seat: Arc<dyn AhpServer>) -> HostId {
        self.next += 1;
        let minted = HostId(self.next);
        self.seats.insert_mut(minted, seat);
        self.order.push_back_mut(minted);
        minted
    }

    pub fn seat(store: &Store, id: HostId) -> Option<Arc<dyn AhpServer>> {
        store.get::<Servers>()?.seats.get(&id).cloned()
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
        return Some((host, SessionUri::new(host_discovery::LOCAL_FS_SESSION)));
    }
    None
}

pub fn route_seat(
    store: &Store,
    authority: &str,
) -> Option<(HostId, Arc<dyn AhpServer>, SessionUri)> {
    let (host, session) = route(store, authority)?;
    let seat = Servers::seat(store, host)?;
    Some((host, seat, session))
}

pub fn route_authority(host: HostId, session: &SessionUri) -> crate::Authority {
    if session.as_str() == host_discovery::LOCAL_FS_SESSION {
        crate::Authority::new("local")
    } else {
        crate::Authority::new(authority(host, session))
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
    fn uri_of(&self, location: &crate::ResourceLocation) -> ResourceUri;

    fn location_of(
        &self,
        uri: &ResourceUri,
        kind: crate::ResourceType,
        authority: &crate::Authority,
    ) -> Option<crate::ResourceLocation>;
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
