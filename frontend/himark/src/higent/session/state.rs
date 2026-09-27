// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::higent::HostId;
use crate::higent::{ChatUri, SessionUri};
use ahp_types::state::{AgentInfo, ChatSummary, SessionSummary};
use imba::store::Store;

#[derive(Clone, Debug)]
pub enum HostStatus {
    Idle,
    Connecting,
    Connected,
    Failed(String),
}

#[derive(Clone, Default)]
pub struct SessionChannel {
    pub provider: String,
    pub chats: rpds::VectorSync<ChatSummary>,
    pub default_chat: Option<ChatUri>,

    pub working_directories: rpds::VectorSync<String>,

    pub config: Option<Arc<ahp_types::state::SessionConfigState>>,
}

#[derive(Clone)]
pub struct Host {
    pub name: String,
    pub status: HostStatus,
    pub agents: rpds::VectorSync<AgentInfo>,

    pub sessions: rpds::VectorSync<SessionSummary>,

    pub states: rpds::HashTrieMapSync<SessionUri, SessionChannel>,

    families: rpds::HashTrieMapSync<SessionUri, SessionState>,

    uris: Option<Arc<dyn crate::higent::ResourceUriMap>>,
}

#[derive(Clone, Default)]
pub(crate) struct SessionState {
    /// The conversations this session owns. They live HERE and nowhere
    /// else, and they die with the session — but they are never
    /// gathered into the store as a component: a command gathered for
    /// another session would then hold a second, empty copy of them
    /// and write it back over this one. Every road reaches a chat by
    /// its OWN `SessionId`, through `Hosts` (which every gather
    /// carries whole).
    chats: crate::higent::Chats,

    pub(crate) trees: crate::hifiles::SessionTree,

    pub(crate) recents: crate::RecentLocations,

    pub(crate) changes: crate::hichanges::Changes,

    pub(crate) history: crate::hihistory::History,

    pub(crate) comments: crate::hicomments::Comments,

    pub(crate) terminals: crate::terminal::Terminals,

    documents: crate::OpenDocuments,

    scratch_names: documents::ScratchMint,
}

impl SessionState {
    fn gather_into(&self, store: &mut Store) {
        store.put(self.documents.clone());
        store.put(self.scratch_names.clone());
    }

    /// Only DOCUMENTS are projected now (they live in the `documents`
    /// crate, which is session-blind by design and does its own store
    /// reads; `command_scope` derives their owner instead). Everything
    /// else is session-ADDRESSED and was never in the store, so it is
    /// carried over from the family that stood before this batch.
    fn take_from(store: &mut Store, held: Option<&SessionState>) -> Self {
        Self {
            // Session-addressed state was never projected: it is carried
            // over from the family that stood before this batch.
            chats: held.map(|held| held.chats.clone()).unwrap_or_default(),
            trees: held.map(|held| held.trees.clone()).unwrap_or_default(),
            recents: held.map(|held| held.recents.clone()).unwrap_or_default(),
            terminals: held.map(|held| held.terminals.clone()).unwrap_or_default(),
            history: held.map(|held| held.history.clone()).unwrap_or_default(),
            comments: held.map(|held| held.comments.clone()).unwrap_or_default(),
            changes: held.map(|held| held.changes.clone()).unwrap_or_default(),
            documents: store.take().unwrap_or_default(),
            scratch_names: store.take().unwrap_or_default(),
        }
    }

    /// The projected components this batch wrote, by name — what a
    /// scopeless scatter is about to drop on the floor.
    fn orphans(&self) -> Vec<&'static str> {
        let mut named = Vec::new();
        if !self.documents.is_empty() {
            named.push("documents");
        }
        if !self.scratch_names.is_empty() {
            named.push("scratch names");
        }
        named
    }

    fn is_empty(&self) -> bool {
        self.chats.is_empty()
            && self.trees.is_empty()
            && self.recents.is_empty()
            && self.changes.is_empty()
            && self.history.is_empty()
            && self.comments.is_empty()
            && self.terminals.is_empty()
            && self.documents.is_empty()
            && self.scratch_names.is_empty()
    }
}

impl Host {
    fn new(name: String) -> Self {
        Self {
            name,
            status: HostStatus::Idle,
            agents: rpds::VectorSync::new_sync(),
            sessions: rpds::VectorSync::new_sync(),
            states: rpds::HashTrieMapSync::new_sync(),
            families: rpds::HashTrieMapSync::new_sync(),
            uris: None,
        }
    }

    pub fn summary(&self, session: &SessionUri) -> Option<&SessionSummary> {
        self.sessions
            .iter()
            .find(|summary| summary.resource == session.as_str())
    }
}

#[derive(Clone, Default)]
pub struct Hosts {
    entries: rpds::HashTrieMapSync<HostId, Host>,
    order: rpds::VectorSync<HostId>,

    generation: u64,
}

impl Hosts {
    pub fn list(store: &Store) -> Vec<(HostId, Host)> {
        let Some(hosts) = store.get::<Hosts>() else {
            return Vec::new();
        };
        hosts
            .order
            .iter()
            .filter_map(|id| hosts.entries.get(id).map(|host| (*id, host.clone())))
            .collect()
    }

    pub fn host(store: &Store, id: HostId) -> Option<Host> {
        store.get::<Hosts>()?.entries.get(&id).cloned()
    }

    pub fn generation(store: &Store) -> u64 {
        store
            .get::<Hosts>()
            .map(|hosts| hosts.generation)
            .unwrap_or(0)
    }

    pub fn install_uris(
        store: &mut Store,
        id: HostId,
        map: Arc<dyn crate::higent::ResourceUriMap>,
    ) {
        Self::update(store, id, |host| host.uris = Some(Arc::clone(&map)));
    }

    pub fn uris(store: &Store, id: HostId) -> Option<Arc<dyn crate::higent::ResourceUriMap>> {
        store.get::<Hosts>()?.entries.get(&id)?.uris.clone()
    }

    pub(super) fn update(store: &mut Store, id: HostId, mutate: impl FnOnce(&mut Host)) {
        store.update::<Hosts>(|hosts| {
            let Some(mut host) = hosts.entries.get(&id).cloned() else {
                return;
            };
            mutate(&mut host);
            hosts.entries.insert_mut(id, host);
            hosts.generation += 1;
        });
    }

    pub fn host_ref(store: &Store, id: HostId) -> Option<&Host> {
        store.get::<Hosts>()?.entries.get(&id)
    }

    pub(super) fn ensure_row(store: &mut Store, id: HostId, name: String) {
        store.update::<Hosts>(|hosts| {
            if hosts.entries.contains_key(&id) {
                return;
            }
            hosts.order.push_back_mut(id);
            hosts.entries.insert_mut(id, Host::new(name));
            hosts.generation += 1;
        });
    }

    pub(crate) fn gather_session(&self, store: &mut Store, scope: &crate::SessionId) {
        self.entries
            .get(&scope.host)
            .and_then(|host| host.families.get(&scope.session))
            .cloned()
            .unwrap_or_default()
            .gather_into(store);
    }

    pub(crate) fn scatter_session(&mut self, store: &mut Store, scope: &crate::SessionId) {
        let held = self
            .entries
            .get(&scope.host)
            .and_then(|host| host.families.get(&scope.session));
        let families = SessionState::take_from(store, held);
        if families.is_empty() {
            if let Some(host) = self.entries.get(&scope.host) {
                if host.families.contains_key(&scope.session) {
                    let mut host = host.clone();
                    host.families.remove_mut(&scope.session);
                    self.entries.insert_mut(scope.host, host);
                }
            }
            return;
        }
        let mut host = match self.entries.get(&scope.host) {
            Some(host) => host.clone(),
            None => {
                self.order.push_back_mut(scope.host);
                Host::new("Local".to_owned())
            }
        };
        host.families.insert_mut(scope.session.clone(), families);
        self.entries.insert_mut(scope.host, host);
    }

    /// A session's conversations, reached by its own id whatever the
    /// batch was gathered for.
    /// A session's own state, addressed by its id. `Hosts` rides EVERY
    /// gather whole, so this road works under any scope — or none.
    /// That is the point: state reached this way cannot be filed into
    /// the family a batch happened to be gathered for, and cannot be
    /// dropped by a scopeless scatter.
    /// `HostId::LOCAL` is a PLACEHOLDER until the local seat registers
    /// and `rekey_local_families` moves the family to the real id — and
    /// it moves the family, not the ids panes and landings already hold.
    /// So an address naming the placeholder resolves to the live local
    /// host, and vice versa: the id a caller carries never goes stale.
    fn addressed(store: &Store, session: &crate::SessionId) -> crate::SessionId {
        let live = store
            .get::<crate::higent::LocalHost>()
            .and_then(|local| local.0);
        let Some(live) = live else {
            return session.clone();
        };
        let known = |host: HostId| {
            store.get::<Hosts>().is_some_and(|hosts| {
                hosts
                    .entries
                    .get(&host)
                    .is_some_and(|row| row.families.contains_key(&session.session))
            })
        };
        if session.host == HostId::LOCAL && !known(HostId::LOCAL) {
            return crate::SessionId {
                host: live,
                session: session.session.clone(),
            };
        }
        if session.host == live && !known(live) && known(HostId::LOCAL) {
            return crate::SessionId {
                host: HostId::LOCAL,
                session: session.session.clone(),
            };
        }
        session.clone()
    }

    pub(crate) fn family<'a>(
        store: &'a Store,
        session: &crate::SessionId,
    ) -> Option<&'a SessionState> {
        let session = Self::addressed(store, session);
        store
            .get::<Hosts>()?
            .entries
            .get(&session.host)?
            .families
            .get(&session.session)
    }

    /// Mutate a session's own state in place. The family is minted if
    /// this is the session's first.
    pub(crate) fn update_family(
        store: &mut Store,
        session: &crate::SessionId,
        mutate: impl FnOnce(&mut SessionState),
    ) {
        let session = &Self::addressed(store, session);
        store.update::<Hosts>(|hosts| {
            let mut host = match hosts.entries.get(&session.host) {
                Some(host) => host.clone(),
                None => {
                    hosts.order.push_back_mut(session.host);
                    Host::new("Local".to_owned())
                }
            };
            let mut family = host
                .families
                .get(&session.session)
                .cloned()
                .unwrap_or_default();
            mutate(&mut family);
            host.families.insert_mut(session.session.clone(), family);
            hosts.entries.insert_mut(session.host, host);
            hosts.generation += 1;
        });
    }

    pub(crate) fn chats_of<'a>(
        store: &'a Store,
        session: &crate::SessionId,
    ) -> Option<&'a crate::higent::Chats> {
        Self::family(store, session).map(|family| &family.chats)
    }

    pub(crate) fn update_chats(
        store: &mut Store,
        session: &crate::SessionId,
        mutate: impl FnOnce(&mut crate::higent::Chats),
    ) {
        Self::update_family(store, session, |family| mutate(&mut family.chats));
    }

    /// Which session owns a terminal — the cold road, for a family row
    /// or a walk back that holds a channel and nothing else.
    pub(crate) fn session_of_terminal(
        store: &Store,
        channel: &crate::higent::ChannelUri,
    ) -> Option<crate::SessionId> {
        Self::find_session(store, |families| families.terminals.holds(channel))
    }

    /// Which session owns a chat — the COLD road, for the places that
    /// hold a chat uri and nothing else (a window's listing, a family
    /// row, a walk back).
    pub(crate) fn session_of_chat(
        store: &Store,
        chat: &crate::higent::ChatUri,
    ) -> Option<crate::SessionId> {
        Self::find_session(store, |families| families.chats.holds(chat))
    }

    /// Every chat of every session, with the session that owns it —
    /// the cross-family listing, for a window that has to find a chat
    /// without knowing whose it is.
    pub fn every_chat(store: &Store) -> Vec<(crate::SessionId, crate::higent::ChatUri)> {
        let Some(hosts) = store.get::<Hosts>() else {
            return Vec::new();
        };
        hosts
            .entries
            .iter()
            .flat_map(|(host, row)| {
                row.families.iter().flat_map(move |(session, families)| {
                    let id = crate::SessionId {
                        host: *host,
                        session: session.clone(),
                    };
                    families
                        .chats
                        .uris()
                        .into_iter()
                        .map(move |chat| (id.clone(), chat))
                })
            })
            .collect()
    }

    pub(crate) fn session_of_document(
        store: &Store,
        document: crate::DocumentId,
    ) -> Option<crate::SessionId> {
        Self::find_session(store, |families| families.documents.contains_id(document))
    }

    pub(crate) fn session_of_watch(
        store: &Store,
        subscription: crate::watch::Subscription,
    ) -> Option<crate::SessionId> {
        Self::find_session(store, |families| {
            families.documents.rides_watch(subscription)
        })
    }

    pub(crate) fn session_of_diff(
        store: &Store,
        diff: ::editor::diff::DiffId,
    ) -> Option<crate::SessionId> {
        Self::find_session(store, |families| families.documents.tracks_diff(diff))
    }

    fn find_session(
        store: &Store,
        matches: impl Fn(&SessionState) -> bool,
    ) -> Option<crate::SessionId> {
        let hosts = store.get::<Hosts>()?;
        for (id, host) in hosts.entries.iter() {
            for (session, families) in host.families.iter() {
                if matches(families) {
                    return Some(crate::SessionId {
                        host: *id,
                        session: session.clone(),
                    });
                }
            }
        }
        None
    }

    pub(crate) fn rekey_local_families(&mut self, previous: Option<HostId>, target: HostId) {
        let fs = SessionUri::new(host_discovery::LOCAL_FS_SESSION);
        let mut sources = vec![HostId::LOCAL];
        if let Some(previous) = previous {
            sources.push(previous);
        }
        for source in sources {
            if source == target {
                continue;
            }
            let Some(row) = self.entries.get(&source) else {
                continue;
            };
            let Some(families) = row.families.get(&fs).cloned() else {
                continue;
            };
            let mut row = row.clone();
            row.families.remove_mut(&fs);
            self.entries.insert_mut(source, row);
            let mut host = match self.entries.get(&target) {
                Some(host) => host.clone(),
                None => {
                    self.order.push_back_mut(target);
                    Host::new("Local".to_owned())
                }
            };
            if !host.families.contains_key(&fs) {
                host.families.insert_mut(fs.clone(), families);
            }
            self.entries.insert_mut(target, host);
        }
    }

    /// A batch gathered without a session scope has no family to
    /// scatter into. The CHAT records are safe — they live in the
    /// global store now, one home whatever the gather — but anything
    /// else written here would be dropped on the floor.
    /// A batch gathered without a session scope has no family to
    /// scatter into, so whatever it wrote to a PROJECTED component is
    /// dropped here. Session-ADDRESSED state (chats, trees, recents,
    /// terminals) is not projected and cannot be lost this way; the
    /// rest can, and it must never be lost QUIETLY — a `debug_assert`
    /// alone is invisible in the build the user runs.
    pub(crate) fn assert_no_family_orphans(store: &mut Store) {
        let orphans = SessionState::take_from(store, None);
        let named = orphans.orphans();
        if !named.is_empty() {
            eprintln!(
                "[state] DROPPED a session-family write made with no session scope: {}\n{}",
                named.join(", "),
                std::backtrace::Backtrace::force_capture()
            );
        }
        debug_assert!(
            named.is_empty(),
            "a session-family write happened in a batch gathered without a session scope: {}",
            named.join(", ")
        );
    }
}

#[cfg(test)]
mod family_tests {
    use super::*;
    use crate::higent::{ChatUri, Chats};

    fn session(uri: &str) -> crate::SessionId {
        crate::SessionId {
            host: HostId::LOCAL,
            session: SessionUri::new(uri),
        }
    }

    fn put(store: &mut Store, session: &crate::SessionId, chat: &str) -> ChatUri {
        let uri = ChatUri::new(chat);
        let panel = crate::higent::chat::ChatPanel::new(
            store,
            ::editor::test_document::test_ui(),
            session.host,
            session.session.clone(),
            uri.clone(),
        );
        Chats::put(store, uri.clone(), panel);
        uri
    }

    /// A chat is reached by its OWN session, so a batch gathered for
    /// ANOTHER session — the pane road, whenever the window is not on
    /// the chat's session — still finds the one record.
    #[test]
    fn a_chat_is_reached_by_its_own_session() {
        let mut state = crate::AppState::default();
        let seats = crate::higent::Servers::default();
        let home = session("s-a");

        let mut store = state.gather(None, Some(&home), &seats);
        let uri = put(&mut store, &home, "chat:1");
        state.scatter(store);

        // The next batch is gathered for a DIFFERENT session.
        let store = state.gather(None, Some(&session("s-b")), &seats);
        assert!(
            Chats::chat_ref(&store, &home, &uri).is_some(),
            "the record is found by its own address"
        );
    }

    #[test]
    fn a_chat_survives_a_scopeless_batch() {
        let mut state = crate::AppState::default();
        let seats = crate::higent::Servers::default();
        let home = session("s-a");

        let mut store = state.gather(None, Some(&home), &seats);
        let uri = put(&mut store, &home, "chat:2");
        state.scatter(store);

        // A batch with NO session scope: it writes other things, and
        // the chats must not be dragged out of their family with them.
        let mut store = state.gather(None, None, &seats);
        state.scatter(std::mem::replace(&mut store, Store::new()));

        let store = state.gather(None, Some(&home), &seats);
        assert!(Chats::chat_ref(&store, &home, &uri).is_some());
    }

    #[test]
    fn a_write_in_a_foreign_gather_lands_in_the_right_family() {
        let mut state = crate::AppState::default();
        let seats = crate::higent::Servers::default();
        let home = session("s-a");
        let elsewhere = session("s-b");

        // The batch is gathered for s-b; the chat belongs to s-a.
        let mut store = state.gather(None, Some(&elsewhere), &seats);
        let uri = put(&mut store, &home, "chat:3");
        state.scatter(store);

        let store = state.gather(None, Some(&home), &seats);
        assert!(
            Chats::chat_ref(&store, &home, &uri).is_some(),
            "filed by the panel's session, not the gather's"
        );
        assert!(
            Chats::chat_ref(&store, &elsewhere, &uri).is_none(),
            "and nowhere else"
        );
    }

    #[test]
    fn a_sessions_chats_are_its_own() {
        let mut state = crate::AppState::default();
        let seats = crate::higent::Servers::default();
        let a = session("s-a");
        let b = session("s-b");

        let mut store = state.gather(None, Some(&a), &seats);
        let mine = put(&mut store, &a, "chat:a");
        let theirs = put(&mut store, &b, "chat:b");
        state.scatter(store);

        let store = state.gather(None, Some(&a), &seats);
        assert_eq!(Chats::list(&store, &a), vec![mine]);
        assert_eq!(Chats::list(&store, &b), vec![theirs]);
    }

    /// The session is the LIFETIME of its chats.
    #[test]
    fn letting_a_session_go_takes_its_chats() {
        let mut state = crate::AppState::default();
        let seats = crate::higent::Servers::default();
        let home = session("s-a");

        let mut store = state.gather(None, Some(&home), &seats);
        let uri = put(&mut store, &home, "chat:1");
        Chats::forget_session(&mut store, &home);
        state.scatter(store);

        let store = state.gather(None, Some(&home), &seats);
        assert!(Chats::chat_ref(&store, &home, &uri).is_none());
        assert!(Chats::list(&store, &home).is_empty());
    }

    #[test]
    fn a_chat_names_the_session_that_owns_it() {
        let mut state = crate::AppState::default();
        let seats = crate::higent::Servers::default();
        let home = session("s-a");

        let mut store = state.gather(None, Some(&home), &seats);
        let uri = put(&mut store, &home, "chat:1");
        state.scatter(store);

        let store = state.gather(None, None, &seats);
        assert_eq!(Hosts::session_of_chat(&store, &uri), Some(home));
    }

    #[test]
    fn an_unknown_chat_names_no_session() {
        let state = crate::AppState::default();
        let store = state.gather(None, None, &crate::higent::Servers::default());
        assert!(Hosts::session_of_chat(&store, &ChatUri::new("chat:ghost")).is_none());
    }
}
