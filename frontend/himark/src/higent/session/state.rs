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

    changes_wire: Id<crate::drivers::changes::ChangesWire>,

    history: Id<crate::hihistory::History>,

    comments: Id<crate::hicomments::Comments>,

    terminals: Id<crate::terminal::Terminals>,

    documents: Id<crate::OpenDocuments>,

    lists: Id<crate::locations::LocationLists>,

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

    pub fn changes_wire(&self) -> Id<crate::drivers::changes::ChangesWire> {
        self.changes_wire
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

    pub fn documents(&self) -> Id<crate::OpenDocuments> {
        self.documents
    }

    pub fn lists(&self) -> Id<crate::locations::LocationLists> {
        self.lists
    }

    pub fn scratch_names(&self) -> Id<documents::ScratchMint> {
        self.scratch_names
    }

    fn mint() -> Self {
        Self {
            chats: Id::mint(),
            trees: Id::mint(),
            recents: Id::mint(),
            changes: Id::mint(),
            changes_wire: Id::mint(),
            history: Id::mint(),
            comments: Id::mint(),
            terminals: Id::mint(),
            documents: Id::mint(),
            lists: Id::mint(),
            scratch_names: Id::mint(),
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
            && empty(store, self.changes_wire, |it| it.is_empty())
            && empty(store, self.trees, |it| it.is_empty())
            && empty(store, self.recents, |it| it.is_empty())
            && empty(store, self.changes, |it| it.is_empty())
            && empty(store, self.history, |it| it.is_empty())
            && empty(store, self.comments, |it| it.is_empty())
            && empty(store, self.terminals, |it| it.is_empty())
            && empty(store, self.documents, |it| it.is_empty())
            && empty(store, self.lists, |it| it.is_empty())
            && empty(store, self.scratch_names, |it| it.is_empty())
    }

    /// Manual lifecycle (docs/entities.md): the owner retracts what
    /// its ids name when the row goes.
    fn retract_all(&self, store: &mut Store) {
        store.dispose(self.chats);
        store.retract(self.trees);
        store.retract(self.recents);
        store.retract(self.changes);
        store.retract(self.changes_wire);
        store.dispose(self.history);
        store.dispose(self.comments);
        store.retract(self.terminals);
        // Converted collections leave through `dispose`: the row goes,
        // then its `destroy` retracts what it owns (law 6). The others
        // follow as they convert.
        store.dispose(self.documents);
        // The lists' live streams die with the row: the poll tokens
        // and channel ends drop with it (the Terminals precedent).
        store.retract(self.lists);
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
        // Families minted before the map arrived read their own
        // stamp — heal them now (docs/entities.md law 4).
        Self::stamp_families_uris(store, id);
    }

    /// Stamp the host's uri map onto every family it holds: the mint
    /// stamps, and the two moves that outrun it — a map installed
    /// after a mint, the local placeholder rekeyed to the real
    /// host — heal here.
    pub(crate) fn stamp_families_uris(store: &mut Store, id: HostId) {
        let Some(map) = Self::uris(store, id) else {
            return;
        };
        let families: Vec<SessionState> = store
            .get::<Hosts>()
            .and_then(|hosts| hosts.entries.get(&id))
            .map(|host| host.families.values().cloned().collect())
            .unwrap_or_default();
        for family in families {
            crate::drivers::changes::ChangesWire::stamp_uris(store, family.changes_wire, &map);
            crate::hicomments::Comments::stamp_uris(store, family.comments, &map);
        }
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

    /// The all-empty housekeeping sweep: a family whose every
    /// collection emptied leaves the catalog, and its rows leave the
    /// table. Nothing is projected and nothing comes back — the store
    /// is single and global, and the table IS the data.
    pub(crate) fn scatter_session(&mut self, store: &mut Store, scope: &crate::SessionId) {
        let Some(families) = self
            .entries
            .get(&scope.host)
            .and_then(|host| host.families.get(&scope.session))
            .cloned()
        else {
            return;
        };
        // A family a live window HOLDS is not garbage, however empty:
        // fresh sessions start with nothing open (the chat owns the
        // workbench), and the window's grip is what keeps the bundle's
        // ids valid until content arrives.
        if families.is_empty(store) && !crate::Windows::any_window_holds(store, scope) {
            families.retract_all(store);
            if let Some(host) = self.entries.get(&scope.host) {
                let mut host = host.clone();
                host.families.remove_mut(&scope.session);
                self.entries.insert_mut(scope.host, host);
            }
        }
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

    /// Every session family, host order — the batch tail's domain:
    /// the sync lanes run over all of them, each lane draining its own
    /// pending queue, so a clean family costs map reads. A family is a
    /// row of ids; the clone is pointer bumps.
    pub(crate) fn families(store: &Store) -> Vec<SessionState> {
        let Some(hosts) = store.get::<Hosts>() else {
            return Vec::new();
        };
        hosts
            .entries
            .values()
            .flat_map(|host| host.families.values().cloned())
            .collect()
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
        // The collections that hold SIBLING ids are put wired, here,
        // the one place that knows the whole wiring (law 4) — the
        // host's uri map rides in with them (a placeholder host has
        // none yet; the designate/rekey heal stamps it after).
        let uris = Self::uris(store, session.host);
        store.put_entity(
            family.changes,
            crate::hichanges::ChangeSets::wired(family.documents, family.history),
        );
        store.put_entity(
            family.changes_wire,
            crate::drivers::changes::ChangesWire::wired(
                family.changes,
                family.history,
                uris.clone(),
            ),
        );
        store.put_entity(
            family.history,
            crate::hihistory::History::wired(family.changes),
        );
        store.put_entity(
            family.comments,
            crate::hicomments::Comments::wired(family.documents, uris),
        );
        // The documents→comments borders (the document hooks, the
        // comment gesture) get INSTANCES wired with the sibling id,
        // scoped to this family's documents — retired by the
        // collection's `destroy`.
        crate::OpenDocuments::install_scoped_hook(
            store,
            family.documents,
            std::sync::Arc::new(crate::hicomments::CommentsHook {
                comments: family.comments,
            }),
        );
        crate::DocumentCommands::register_scoped(
            store,
            family.documents,
            std::sync::Arc::new(crate::hicomments::AddComment {
                comments: family.comments,
            }),
        );
        store.put_entity(
            family.lists,
            crate::locations::LocationLists::wired(family.documents),
        );
        crate::OpenDocuments::install_scoped_hook(
            store,
            family.documents,
            std::sync::Arc::new(crate::locations::LocationsWashHook {
                lists: family.lists,
            }),
        );
        store.put_entity(family.chats, crate::higent::Chats::wired(family.recents));
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

    /// The other half of the ceremony (docs/entities.md step 2,
    /// law 6): the session is the LIFETIME of everything its family
    /// row names. Removing the row RETRACTS every entity it minted —
    /// terminals' PTYs hang up on the drop, the backstop they always
    /// had. Removal is structural, so the generation bumps. The ONE
    /// deletion road; `scatter_session`'s all-empty sweep is mere
    /// housekeeping over the same retract.
    pub fn dispose_family(store: &mut Store, session: &crate::SessionId) {
        let session = &Self::addressed(store, session);
        let Some(family) = store
            .get::<Hosts>()
            .and_then(|hosts| hosts.entries.get(&session.host))
            .and_then(|host| host.families.get(&session.session))
            .cloned()
        else {
            return;
        };
        store.update::<Hosts>(|hosts| {
            let Some(host) = hosts.entries.get(&session.host) else {
                return;
            };
            let mut host = host.clone();
            host.families.remove_mut(&session.session);
            hosts.entries.insert_mut(session.host, host);
            hosts.generation += 1;
        });
        family.retract_all(store);
    }

    /// A session RENAMED (the composer's placeholder uri becomes the
    /// provider's real one): the family row moves to the new key — the
    /// ids never change, only the catalog's name for them. Windows
    /// keep their bundle through the rekey; this keeps the catalog
    /// telling the same story.
    pub fn rekey_family(store: &mut Store, from: &crate::SessionId, to: &crate::SessionId) {
        if from == to {
            return;
        }
        let Some(family) = Self::family(store, from).cloned() else {
            return;
        };
        if Self::family(store, to).is_some() {
            eprintln!("[higent] NOT rekeying {from:?} -> {to:?}: the target has a family");
            return;
        }
        store.update::<Hosts>(|hosts| {
            let Some(source) = hosts.entries.get(&from.host) else {
                return;
            };
            let mut source = source.clone();
            source.families.remove_mut(&from.session);
            hosts.entries.insert_mut(from.host, source);
            let mut target = match hosts.entries.get(&to.host) {
                Some(host) => host.clone(),
                None => {
                    hosts.order.push_back_mut(to.host);
                    Host::new("Local".to_owned())
                }
            };
            target
                .families
                .insert_mut(to.session.clone(), family.clone());
            hosts.entries.insert_mut(to.host, target);
            hosts.generation += 1;
        });
        if from.host != to.host {
            // Crossed hosts: the family's stamped uri map is the old
            // host's — re-stamp with the new one's.
            Self::stamp_families_uris(store, to.host);
        }
    }

    // No typed per-family doors here, deliberately: Hosts answers one
    // question — WHICH ids a session's family holds (`family`,
    // `ensure_family`) — and the collections are then read and written
    // BY ID (`store.entity` / `store.update_entity`), threaded to the
    // use sites (docs/entities.md law 3). A helper here that takes a
    // `SessionId` per read would remarry every collection to Hosts.

    /// Which session owns a documents collection — an ID COMPARE over
    /// the family rows, no content resolution: an addressed command
    /// scopes to its owner whether or not the addressed record still
    /// exists. The per-collection compares live with the collections
    /// (`AppEntity::family_id`); this is their one iteration.

    /// The family whose documents collection this is — the sibling
    /// road for an edge that holds a documents id and needs the
    /// collection next to it (the stripe-base resolver).
    pub fn family_of_documents(
        store: &Store,
        documents: Id<crate::OpenDocuments>,
    ) -> Option<&SessionState> {
        let hosts = store.get::<Hosts>()?;
        hosts.entries.values().find_map(|host| {
            host.families
                .values()
                .find(|families| families.documents == documents)
        })
    }

    /// Which documents collection holds a document — the cold road
    /// for a landing that names only the document.
    pub fn documents_of_document(
        store: &Store,
        document: crate::DocumentId,
    ) -> Option<Id<crate::OpenDocuments>> {
        let hosts = store.get::<Hosts>()?;
        for (_, host) in hosts.entries.iter() {
            for (_, families) in host.families.iter() {
                let documents = families.documents;
                if store
                    .entity(documents)
                    .is_some_and(|docs| docs.contains_id(document))
                {
                    return Some(documents);
                }
            }
        }
        None
    }

    /// Which session rides a watch — the ONE id-less border road:
    /// file events arrive from the watcher with a subscription and
    /// nothing else.
    /// Which documents collection holds a diff view — the cold
    /// re-mint road for a family row that names only the pair.
    /// The documents collection riding a watch subscription — the one
    /// id-less border road (file events arrive with a subscription and
    /// nothing else), found once, here.
    pub(crate) fn documents_of_watch(
        store: &Store,
        subscription: crate::watch::Subscription,
    ) -> Option<Id<crate::OpenDocuments>> {
        let hosts = store.get::<Hosts>()?;
        hosts.entries.values().find_map(|host| {
            host.families.values().find_map(|families| {
                store
                    .entity(families.documents)
                    .is_some_and(|documents| documents.rides_watch(subscription))
                    .then_some(families.documents)
            })
        })
    }

    /// The session and family a documents collection belongs to — for
    /// a pane that holds the collection's id and needs the catalog's
    /// name for its folders beside the sibling ids.
    pub(crate) fn home_of_documents(
        store: &Store,
        documents: Id<crate::OpenDocuments>,
    ) -> Option<(crate::SessionId, SessionState)> {
        let hosts = store.get::<Hosts>()?;
        for (host, row) in hosts.entries.iter() {
            for (session, families) in row.families.iter() {
                if families.documents == documents {
                    return Some((
                        crate::SessionId {
                            host: *host,
                            session: session.clone(),
                        },
                        families.clone(),
                    ));
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

    /// Mint a chat into ITS session's collection — the mint door is the
    /// one catalog consult; the record carries the collection id after.
    fn put(store: &mut Store, session: &crate::SessionId, chat: &str) -> ChatUri {
        let uri = ChatUri::new(chat);
        let chats = Hosts::ensure_family(store, session).chats();
        let panel = crate::higent::chat::ChatPanel::new(
            store,
            ::editor::test_document::test_ui(),
            session.host,
            session.session.clone(),
            chats,
            uri.clone(),
        );
        Chats::put(store, chats, uri.clone(), panel);
        uri
    }

    /// The cold read a test takes: session → collection → record.
    fn chat_in(store: &Store, session: &crate::SessionId, uri: &ChatUri) -> bool {
        Hosts::family(store, session)
            .and_then(|family| Chats::chat_ref(store, family.chats(), uri))
            .is_some()
    }

    fn list_in(store: &Store, session: &crate::SessionId) -> Vec<ChatUri> {
        Hosts::family(store, session)
            .map(|family| Chats::list(store, family.chats()))
            .unwrap_or_default()
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
        state.scatter(store, Some(&home));

        // The next batch is gathered for a DIFFERENT session.
        let store = state.gather(None, Some(&session("s-b")), &seats);
        assert!(
            chat_in(&store, &home, &uri),
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
        state.scatter(store, Some(&home));

        // A batch with NO session scope: it writes other things, and
        // the chats must not be dragged out of their family with them.
        let mut store = state.gather(None, None, &seats);
        state.scatter(std::mem::replace(&mut store, Store::new()), None);

        let store = state.gather(None, Some(&home), &seats);
        assert!(chat_in(&store, &home, &uri));
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
        state.scatter(store, Some(&home));

        let store = state.gather(None, Some(&home), &seats);
        assert!(
            chat_in(&store, &home, &uri),
            "filed by the panel's session, not the gather's"
        );
        assert!(!chat_in(&store, &elsewhere, &uri), "and nowhere else");
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
        state.scatter(store, Some(&a));

        let store = state.gather(None, Some(&a), &seats);
        assert_eq!(list_in(&store, &a), vec![mine]);
        assert_eq!(list_in(&store, &b), vec![theirs]);
    }

    /// The session is the LIFETIME of its chats.
    #[test]
    fn letting_a_session_go_takes_its_chats() {
        let mut state = crate::AppState::default();
        let seats = crate::higent::Servers::default();
        let home = session("s-a");

        let mut store = state.gather(None, Some(&home), &seats);
        let uri = put(&mut store, &home, "chat:1");
        Hosts::dispose_family(&mut store, &home);
        state.scatter(store, Some(&home));

        let store = state.gather(None, Some(&home), &seats);
        assert!(!chat_in(&store, &home, &uri));
        assert!(list_in(&store, &home).is_empty());
    }

    /// Disposal is the whole ceremony: the family row leaves `Hosts`
    /// and EVERY entity row its ids named retracts from the table —
    /// nothing session-scoped can outlive its session
    /// (docs/entities.md step 2).
    #[test]
    fn disposal_retracts_every_family_entity() {
        let mut state = crate::AppState::default();
        let seats = crate::higent::Servers::default();
        let home = session("s-a");

        let mut store = state.gather(None, Some(&home), &seats);
        put(&mut store, &home, "chat:1");
        let family = Hosts::ensure_family(&mut store, &home);
        store.update_entity(family.recents, |_recents: &mut crate::RecentLocations| {});
        store.update_entity(family.trees, |_trees| {});
        store.update_entity(family.terminals, |_terminals| {});
        state.scatter(store, Some(&home));

        let mut store = state.gather(None, Some(&home), &seats);
        assert!(Hosts::family(&store, &home).is_some(), "the row is live");
        Hosts::dispose_family(&mut store, &home);

        assert!(Hosts::family(&store, &home).is_none(), "the row is gone");
        assert!(store.entity(family.chats).is_none());
        assert!(store.entity(family.trees).is_none());
        assert!(store.entity(family.recents).is_none());
        assert!(store.entity(family.changes).is_none());
        assert!(store.entity(family.history).is_none());
        assert!(store.entity(family.comments).is_none());
        assert!(store.entity(family.terminals).is_none());
        assert!(store.entity(family.documents).is_none());
        assert!(store.entity(family.scratch_names).is_none());
        state.scatter(store, Some(&home));

        // And scatter resurrects nothing from the scaffolding.
        let store = state.gather(None, Some(&home), &seats);
        assert!(Hosts::family(&store, &home).is_none());
    }
}
