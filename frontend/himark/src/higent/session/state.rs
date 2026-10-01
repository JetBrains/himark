// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::higent::HostId;
use crate::higent::{ChatUri, SessionUri};
use ahp_types::state::{AgentInfo, ChatSummary, SessionSummary};
use imba::store::{Id, Store};

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

/// A session's family row: the IDS of the collections it owns
/// (docs/entities.md). The values live in the store's flat entity
/// table — which rides `globals`, so every gather carries them whole
/// and every collection is reached by its OWN id under any scope (the
/// property chats pioneered, now structural for all of them). The
/// row is minted once (`mint`) and its ids never change; disposal
/// retracts what the ids name.
#[derive(Clone)]
pub struct SessionState {
    chats: Id<crate::higent::Chats>,

    trees: Id<crate::hifiles::SessionTree>,

    recents: Id<crate::RecentLocations>,

    changes: Id<crate::hichanges::Changes>,

    history: Id<crate::hihistory::History>,

    comments: Id<crate::hicomments::Comments>,

    terminals: Id<crate::terminal::Terminals>,

    documents: Id<crate::OpenDocuments>,

    scratch_names: Id<documents::ScratchMint>,
}

impl SessionState {
    pub fn chats(&self) -> Id<crate::higent::Chats> {
        self.chats
    }

    pub fn trees(&self) -> Id<crate::hifiles::SessionTree> {
        self.trees
    }

    pub fn recents(&self) -> Id<crate::RecentLocations> {
        self.recents
    }

    pub fn changes(&self) -> Id<crate::hichanges::Changes> {
        self.changes
    }

    pub fn history(&self) -> Id<crate::hihistory::History> {
        self.history
    }

    pub fn comments(&self) -> Id<crate::hicomments::Comments> {
        self.comments
    }

    pub fn terminals(&self) -> Id<crate::terminal::Terminals> {
        self.terminals
    }

    fn mint() -> Self {
        Self {
            chats: Id::mint(),
            trees: Id::mint(),
            recents: Id::mint(),
            changes: Id::mint(),
            history: Id::mint(),
            comments: Id::mint(),
            terminals: Id::mint(),
            documents: Id::mint(),
            scratch_names: Id::mint(),
        }
    }

    /// Only DOCUMENTS are projected as flat components (scaffolding —
    /// docs/entities.md step 1; the `documents` crate is session-blind
    /// and does its own store reads). Everything else is reached by
    /// id and never leaves the table.
    fn gather_into(&self, store: &mut Store) {
        let documents = store.entity(self.documents).cloned().unwrap_or_default();
        let scratch_names = store
            .entity(self.scratch_names)
            .cloned()
            .unwrap_or_default();
        store.put(documents);
        store.put(scratch_names);
    }

    /// The flat projections come home to their rows.
    fn absorb_projected(&self, store: &mut Store) {
        if let Some(documents) = store.take::<crate::OpenDocuments>() {
            store.put_entity(self.documents, documents);
        }
        if let Some(scratch_names) = store.take::<documents::ScratchMint>() {
            store.put_entity(self.scratch_names, scratch_names);
        }
    }

    fn is_empty(&self, store: &Store) -> bool {
        fn empty<T: imba::store::Component>(
            store: &Store,
            id: Id<T>,
            is_empty: impl Fn(&T) -> bool,
        ) -> bool {
            store.entity(id).is_none_or(is_empty)
        }
        empty(store, self.chats, |it| it.is_empty())
            && empty(store, self.trees, |it| it.is_empty())
            && empty(store, self.recents, |it| it.is_empty())
            && empty(store, self.changes, |it| it.is_empty())
            && empty(store, self.history, |it| it.is_empty())
            && empty(store, self.comments, |it| it.is_empty())
            && empty(store, self.terminals, |it| it.is_empty())
            && empty(store, self.documents, |it| it.is_empty())
            && empty(store, self.scratch_names, |it| it.is_empty())
    }

    /// Manual lifecycle (docs/entities.md): the owner retracts what
    /// its ids name when the row goes.
    fn retract_all(&self, store: &mut Store) {
        store.retract(self.chats);
        store.retract(self.trees);
        store.retract(self.recents);
        store.retract(self.changes);
        store.retract(self.history);
        store.retract(self.comments);
        store.retract(self.terminals);
        store.retract(self.documents);
        store.retract(self.scratch_names);
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
        match self
            .entries
            .get(&scope.host)
            .and_then(|host| host.families.get(&scope.session))
        {
            Some(family) => family.gather_into(store),
            None => {
                store.put(crate::OpenDocuments::default());
                store.put(documents::ScratchMint::default());
            }
        }
    }

    pub(crate) fn scatter_session(&mut self, store: &mut Store, scope: &crate::SessionId) {
        let families = self
            .entries
            .get(&scope.host)
            .and_then(|host| host.families.get(&scope.session))
            .cloned()
            .unwrap_or_else(SessionState::mint);
        families.absorb_projected(store);
        if families.is_empty(store) {
            families.retract_all(store);
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

    pub fn family<'a>(store: &'a Store, session: &crate::SessionId) -> Option<&'a SessionState> {
        let session = Self::addressed(store, session);
        store
            .get::<Hosts>()?
            .entries
            .get(&session.host)?
            .families
            .get(&session.session)
    }

    /// The session's family row, minted into `Hosts` on first touch.
    /// Minting is STRUCTURAL (a new row in the catalog's map), so it
    /// bumps the generation; content writes land in the entity table
    /// and touch `Hosts` not at all.
    pub fn ensure_family(store: &mut Store, session: &crate::SessionId) -> SessionState {
        let session = &Self::addressed(store, session);
        if let Some(family) = store
            .get::<Hosts>()
            .and_then(|hosts| hosts.entries.get(&session.host))
            .and_then(|host| host.families.get(&session.session))
        {
            return family.clone();
        }
        let family = SessionState::mint();
        let minted = family.clone();
        store.update::<Hosts>(|hosts| {
            let mut host = match hosts.entries.get(&session.host) {
                Some(host) => host.clone(),
                None => {
                    hosts.order.push_back_mut(session.host);
                    Host::new("Local".to_owned())
                }
            };
            host.families
                .insert_mut(session.session.clone(), family.clone());
            hosts.entries.insert_mut(session.host, host);
            hosts.generation += 1;
        });
        minted
    }

    // No typed per-family doors here, deliberately: Hosts answers one
    // question — WHICH ids a session's family holds (`family`,
    // `ensure_family`) — and the collections are then read and written
    // BY ID (`store.entity` / `store.update_entity`), threaded to the
    // use sites (docs/entities.md law 3). A helper here that takes a
    // `SessionId` per read would remarry every collection to Hosts.

    /// Which session owns a terminal — the cold road, for a family row
    /// or a walk back that holds a channel and nothing else.
    pub(crate) fn session_of_terminal(
        store: &Store,
        channel: &crate::higent::ChannelUri,
    ) -> Option<crate::SessionId> {
        Self::find_session(store, |store, families| {
            store
                .entity(families.terminals)
                .is_some_and(|terminals| terminals.holds(channel))
        })
    }

    /// Which session owns a chat — the COLD road, for the places that
    /// hold a chat uri and nothing else (a window's listing, a family
    /// row, a walk back).
    pub(crate) fn session_of_chat(
        store: &Store,
        chat: &crate::higent::ChatUri,
    ) -> Option<crate::SessionId> {
        Self::find_session(store, |store, families| {
            store
                .entity(families.chats)
                .is_some_and(|chats| chats.holds(chat))
        })
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
                    store
                        .entity(families.chats)
                        .map(|chats| chats.uris())
                        .unwrap_or_default()
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
        Self::find_session(store, |store, families| {
            store
                .entity(families.documents)
                .is_some_and(|documents| documents.contains_id(document))
        })
    }

    pub(crate) fn session_of_watch(
        store: &Store,
        subscription: crate::watch::Subscription,
    ) -> Option<crate::SessionId> {
        Self::find_session(store, |store, families| {
            store
                .entity(families.documents)
                .is_some_and(|documents| documents.rides_watch(subscription))
        })
    }

    pub(crate) fn session_of_diff(
        store: &Store,
        diff: ::editor::diff::DiffId,
    ) -> Option<crate::SessionId> {
        Self::find_session(store, |store, families| {
            store
                .entity(families.documents)
                .is_some_and(|documents| documents.tracks_diff(diff))
        })
    }

    fn find_session(
        store: &Store,
        matches: impl Fn(&Store, &SessionState) -> bool,
    ) -> Option<crate::SessionId> {
        let hosts = store.get::<Hosts>()?;
        for (id, host) in hosts.entries.iter() {
            for (session, families) in host.families.iter() {
                if matches(store, families) {
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
        let mut named = Vec::new();
        if store
            .take::<crate::OpenDocuments>()
            .is_some_and(|documents| !documents.is_empty())
        {
            named.push("documents");
        }
        if store
            .take::<documents::ScratchMint>()
            .is_some_and(|names| !names.is_empty())
        {
            named.push("scratch names");
        }
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
