// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use ahp_types::state::{AgentInfo, SessionSummary};
use ahp_wire::client::HostId;
use ahp_wire::client::SessionChannel;
use ahp_wire::client::SessionUri;
use imba::store::{Id, Store};

use super::agents::Agents;

#[derive(Clone, Debug)]
pub enum HostStatus {
    Idle,
    Connecting,
    Connected,
    Failed(String),
}

#[derive(Clone)]
pub struct Host {
    pub name: String,
    pub status: HostStatus,
    pub agents: rpds::VectorSync<AgentInfo>,

    pub sessions: rpds::VectorSync<SessionSummary>,

    pub states: rpds::HashTrieMapSync<SessionUri, SessionChannel>,

    rows: rpds::HashTrieMapSync<SessionUri, SessionState>,

    /// The rows the sweep has SEEN hold content. A newborn row is
    /// empty because nothing landed in it yet, not because its
    /// content went away — only a row that once held something is
    /// garbage when it reads empty again (`sweep_empty`).
    filled: rpds::HashTrieSetSync<SessionUri>,

    uris: Option<Arc<dyn ahp_wire::client::ResourceUriMap>>,
}

/// A session's session row: the IDS of the collections it owns
/// (docs/entities.md). The values live in the store's flat entity
/// table — which rides `globals`, so every gather carries them whole
/// and every collection is reached by its OWN id under any scope (the
/// property chats pioneered, now structural for all of them). The
/// row is minted once (`mint`) and its ids never change; disposal
/// retracts what the ids name.
#[derive(Clone)]
pub struct SessionState {
    pub chats: Id<ahp_chat::chats::Chats>,

    pub trees: Id<filetree::SessionTree>,

    pub recents: Id<recents::RecentLocations>,

    pub changes: Id<changesview::hichanges::Changes>,

    pub changes_wire: Id<ahp_changes::changes::ChangesWire>,

    pub canvas_router: Id<canvas::canvas::CanvasRouter>,

    pub history_wire: Id<ahp_changes::history::HistoryWire>,

    pub comments_wire: Id<ahp_comments::CommentsWire>,

    pub history: Id<changesview::hihistory::History>,

    pub comments: Id<comments::Comments>,

    pub terminals: Id<terminals::Terminals>,

    pub documents: Id<documents::OpenDocuments>,

    pub lists: Id<locations::LocationLists>,

    pub locations_wire: Id<ahp_locations::driver::LocationsWire>,

    pub scratch_names: Id<documents::ScratchMint>,
}

impl SessionState {
    pub fn chats(&self) -> Id<ahp_chat::chats::Chats> {
        self.chats
    }

    pub fn trees(&self) -> Id<filetree::SessionTree> {
        self.trees
    }

    pub fn recents(&self) -> Id<recents::RecentLocations> {
        self.recents
    }

    pub fn changes(&self) -> Id<changesview::hichanges::Changes> {
        self.changes
    }

    pub fn changes_wire(&self) -> Id<ahp_changes::changes::ChangesWire> {
        self.changes_wire
    }

    pub fn canvas_router(&self) -> Id<canvas::canvas::CanvasRouter> {
        self.canvas_router
    }

    pub fn history_wire(&self) -> Id<ahp_changes::history::HistoryWire> {
        self.history_wire
    }

    pub fn comments_wire(&self) -> Id<ahp_comments::CommentsWire> {
        self.comments_wire
    }

    pub fn history(&self) -> Id<changesview::hihistory::History> {
        self.history
    }

    pub fn comments(&self) -> Id<comments::Comments> {
        self.comments
    }

    pub fn terminals(&self) -> Id<terminals::Terminals> {
        self.terminals
    }

    pub fn documents(&self) -> Id<documents::OpenDocuments> {
        self.documents
    }

    pub fn lists(&self) -> Id<locations::LocationLists> {
        self.lists
    }

    pub fn locations_wire(&self) -> Id<ahp_locations::driver::LocationsWire> {
        self.locations_wire
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
            canvas_router: Id::mint(),
            history_wire: Id::mint(),
            comments_wire: Id::mint(),
            history: Id::mint(),
            comments: Id::mint(),
            terminals: Id::mint(),
            documents: Id::mint(),
            lists: Id::mint(),
            locations_wire: Id::mint(),
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
            && empty(store, self.canvas_router, |it| it.is_empty())
            && empty(store, self.history_wire, |it| it.is_empty())
            && empty(store, self.comments_wire, |it| it.is_empty())
            && empty(store, self.trees, |it| it.is_empty())
            && empty(store, self.recents, |it| it.is_empty())
            && empty(store, self.changes, |it| it.is_empty())
            && empty(store, self.history, |it| it.is_empty())
            && empty(store, self.comments, |it| it.is_empty())
            && empty(store, self.terminals, |it| it.is_empty())
            && empty(store, self.documents, |it| it.is_empty())
            && empty(store, self.lists, |it| it.is_empty())
            && empty(store, self.locations_wire, |it| it.is_empty())
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
        store.retract(self.canvas_router);
        store.retract(self.history_wire);
        store.retract(self.comments_wire);
        // The ceremony installed the scoped hooks and commands; the
        // ceremony retires them (law 6 symmetry).
        documents::OpenDocuments::retire_scope(store, self.documents);
        store.retract(self.history);
        store.retract(self.comments);
        store.retract(self.terminals);
        // Converted collections leave through `dispose`: the row goes,
        // then its `destroy` retracts what it owns (law 6). The others
        // follow as they convert.
        store.dispose(self.documents);
        store.retract(self.lists);
        // The lists' live streams die with the WIRE row: the poll
        // tokens and channel ends drop with it (the Terminals
        // precedent).
        store.retract(self.locations_wire);
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
            rows: rpds::HashTrieMapSync::new_sync(),
            filled: rpds::HashTrieSetSync::new_sync(),
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
        map: Arc<dyn ahp_wire::client::ResourceUriMap>,
    ) {
        Self::update(store, id, |host| host.uris = Some(Arc::clone(&map)));
        // Families minted before the map arrived read their own
        // stamp — heal them now (docs/entities.md law 4).
        Self::stamp_host_uris(store, id);
    }

    /// Stamp the host's uri map onto every session it holds: the mint
    /// stamps, and the two moves that outrun it — a map installed
    /// after a mint, the local placeholder rekeyed to the real
    /// host — heal here.
    pub fn stamp_host_uris(store: &mut Store, id: HostId) {
        let Some(map) = Self::uris(store, id) else {
            return;
        };
        let states: Vec<SessionState> = store
            .get::<Hosts>()
            .and_then(|hosts| hosts.entries.get(&id))
            .map(|host| host.rows.values().cloned().collect())
            .unwrap_or_default();
        for state in states {
            ahp_changes::changes::ChangesWire::stamp_uris(store, state.changes_wire, &map);
            ahp_changes::history::HistoryWire::stamp_uris(store, state.history_wire, &map);
            ahp_comments::CommentsWire::stamp_uris(store, state.comments_wire, &map);
        }
    }

    pub fn uris(store: &Store, id: HostId) -> Option<Arc<dyn ahp_wire::client::ResourceUriMap>> {
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

    /// The all-empty housekeeping sweep, run by the shell at batch
    /// tails: a session whose every collection emptied leaves the
    /// catalog, and its rows leave the table. Nothing is projected
    /// and nothing comes back — the store is single and global, and
    /// the table IS the data.
    pub fn sweep_empty(store: &mut Store) {
        /// What the sweep makes of one session row.
        enum Verdict {
            /// It holds content — LATCH it, so the next time it reads
            /// empty we know the content went away.
            Filled,
            /// Empty but not garbage: either nothing has landed in it
            /// yet (a newborn row, mid-setup) or a live window holds
            /// it across a retraction it may yet walk back from.
            Spared,
            /// Filled once, empty now, unheld — garbage.
            Collect,
        }

        let mut hosts = store.take::<Hosts>().unwrap_or_default();
        let rows: Vec<(ahp_wire::SessionId, SessionState, bool)> = hosts
            .entries
            .iter()
            .flat_map(|(host, entry)| {
                entry.rows.iter().map(move |(session, row)| {
                    let scope = ahp_wire::SessionId {
                        host: *host,
                        session: session.clone(),
                    };
                    (scope, row.clone(), entry.filled.contains(session))
                })
            })
            .collect();

        // DECIDE first, over the table as it stands: no row's verdict
        // can depend on another's, since rows name disjoint ids.
        let verdicts: Vec<(ahp_wire::SessionId, SessionState, Verdict)> = rows
            .into_iter()
            .map(|(scope, row, filled)| {
                let verdict = if !row.is_empty(store) {
                    Verdict::Filled
                } else if !filled || Self::held_by_a_window(store, &scope) {
                    Verdict::Spared
                } else {
                    Verdict::Collect
                };
                (scope, row, verdict)
            })
            .collect();

        // Then APPLY.
        for (scope, row, verdict) in verdicts {
            match verdict {
                Verdict::Spared => {}
                Verdict::Filled => hosts.latch(&scope),
                Verdict::Collect => {
                    row.retract_all(store);
                    hosts.drop_row(&scope);
                }
            }
        }
        store.put(hosts);
    }

    fn held_by_a_window(store: &Store, scope: &ahp_wire::SessionId) -> bool {
        store
            .get::<WindowGrip>()
            .is_some_and(|grip| (grip.0)(store, scope))
    }

    /// Record that a row HAS held content — the sweep's latch, which
    /// is what tells an emptied row apart from a newborn one.
    fn latch(&mut self, scope: &ahp_wire::SessionId) {
        let Some(host) = self.entries.get(&scope.host) else {
            return;
        };
        if host.filled.contains(&scope.session) {
            return;
        }
        let mut host = host.clone();
        host.filled.insert_mut(scope.session.clone());
        self.entries.insert_mut(scope.host, host);
    }

    /// Drop a collected row from the catalog.
    fn drop_row(&mut self, scope: &ahp_wire::SessionId) {
        let Some(host) = self.entries.get(&scope.host) else {
            return;
        };
        let mut host = host.clone();
        host.rows.remove_mut(&scope.session);
        host.filled.remove_mut(&scope.session);
        self.entries.insert_mut(scope.host, host);
    }

    /// A session's conversations, reached by its own id whatever the
    /// batch was gathered for.
    /// A session's own state, addressed by its id. `Hosts` rides EVERY
    /// gather whole, so this road works under any scope — or none.
    /// That is the point: state reached this way cannot be filed into
    /// the session a batch happened to be gathered for, and cannot be
    /// dropped by a scopeless scatter.
    /// `HostId::LOCAL` is a PLACEHOLDER until the local client registers
    /// and `rekey_local_sessions` moves the session to the real id — and
    /// it moves the session, not the ids panes and landings already hold.
    /// So an address naming the placeholder resolves to the live local
    /// host, and vice versa: the id a caller carries never goes stale.
    fn addressed(store: &Store, session: &ahp_wire::SessionId) -> ahp_wire::SessionId {
        let live = store
            .get::<ahp_wire::client::LocalHost>()
            .and_then(|local| local.0);
        let Some(live) = live else {
            return session.clone();
        };
        let known = |host: HostId| {
            store.get::<Hosts>().is_some_and(|hosts| {
                hosts
                    .entries
                    .get(&host)
                    .is_some_and(|row| row.rows.contains_key(&session.session))
            })
        };
        if session.host == HostId::LOCAL && !known(HostId::LOCAL) {
            return ahp_wire::SessionId {
                host: live,
                session: session.session.clone(),
            };
        }
        if session.host == live && !known(live) && known(HostId::LOCAL) {
            return ahp_wire::SessionId {
                host: HostId::LOCAL,
                session: session.session.clone(),
            };
        }
        session.clone()
    }

    /// Every session session, host order — the batch tail's domain:
    /// the sync lanes run over all of them, each lane draining its own
    /// pending queue, so a clean session costs map reads. A session is a
    /// row of ids; the clone is pointer bumps.
    pub fn states(store: &Store) -> Vec<SessionState> {
        let Some(hosts) = store.get::<Hosts>() else {
            return Vec::new();
        };
        hosts
            .entries
            .values()
            .flat_map(|host| host.rows.values().cloned())
            .collect()
    }

    pub fn state<'a>(store: &'a Store, session: &ahp_wire::SessionId) -> Option<&'a SessionState> {
        let session = Self::addressed(store, session);
        store
            .get::<Hosts>()?
            .entries
            .get(&session.host)?
            .rows
            .get(&session.session)
    }

    /// The session's session row, minted into `Hosts` on first touch.
    /// Minting is STRUCTURAL (a new row in the catalog's map), so it
    /// bumps the generation; content writes land in the entity table
    /// and touch `Hosts` not at all.
    pub fn ensure_state(store: &mut Store, session: &ahp_wire::SessionId) -> SessionState {
        let session = &Self::addressed(store, session);
        if let Some(state) = store
            .get::<Hosts>()
            .and_then(|hosts| hosts.entries.get(&session.host))
            .and_then(|host| host.rows.get(&session.session))
        {
            return state.clone();
        }
        let state = SessionState::mint();
        let minted = state.clone();
        // The collections that hold SIBLING ids are put wired, here,
        // the one place that knows the whole wiring (law 4) — the
        // host's uri map rides in with them (a placeholder host has
        // none yet; the designate/rekey heal stamps it after).
        let uris = Self::uris(store, session.host);
        store.put_entity(
            state.changes,
            changesview::hichanges::ChangeSets::wired(state.documents, state.history),
        );
        store.put_entity(
            state.canvas_router,
            canvas::canvas::CanvasRouter::wired(state.changes),
        );
        store.put_entity(
            state.changes_wire,
            ahp_changes::changes::ChangesWire::wired(
                state.changes,
                state.history_wire,
                uris.clone(),
            ),
        );
        store.put_entity(
            state.history_wire,
            ahp_changes::history::HistoryWire::wired(state.history, state.changes, uris.clone()),
        );
        store.put_entity(
            state.history,
            changesview::hihistory::History::wired(state.changes),
        );
        store.put_entity(state.comments, comments::Comments::wired(state.documents));
        // The driver's catalog consults, closed over HERE — the
        // ceremony is the one place that knows the catalog and the
        // sibling ids (law 4); the driver below holds only the roads.
        let documents = state.documents;
        let roads = ahp_comments::CatalogRoads {
            default_chat: std::sync::Arc::new(move |store, host, session| {
                let key = ahp_wire::SessionId {
                    host,
                    session: session.clone(),
                };
                if let Some(chat) =
                    Agents::channel(store, &key).and_then(|channel| channel.default_chat)
                {
                    return Some(chat);
                }
                // The fallback workspace is the comments' own session —
                // the catalog names it; no window consulted.
                let bound = Hosts::home_of_documents(store, documents)
                    .map(|(workspace, _)| workspace)
                    .and_then(|workspace| Agents::live_session(store, &workspace))
                    .filter(|bound| bound.host == host)?;
                Agents::channel(store, &bound).and_then(|channel| channel.default_chat)
            }),
            latest_turn: std::sync::Arc::new(|store, host, session| {
                Agents::latest_turn(
                    store,
                    &ahp_wire::SessionId {
                        host,
                        session: session.clone(),
                    },
                )
            }),
        };
        store.put_entity(
            state.comments_wire,
            ahp_comments::CommentsWire::wired(state.comments, uris, roads),
        );
        // The documents→comments borders (the document hooks, the
        // comment gesture) get INSTANCES wired with the sibling id,
        // scoped to this session's documents — retired by the
        // collection's `destroy`.
        documents::OpenDocuments::install_scoped_hook(
            store,
            state.documents,
            std::sync::Arc::new(comments::cards::CommentsHook {
                comments: state.comments,
            }),
        );
        documents::dynamic::DocumentCommands::register_scoped(
            store,
            state.documents,
            std::sync::Arc::new(comments::view::AddComment {
                comments: state.comments,
            }),
        );
        store.put_entity(
            state.lists,
            locations::LocationLists::wired(state.documents),
        );
        store.put_entity(
            state.locations_wire,
            ahp_locations::driver::LocationsWire::wired(state.lists),
        );
        documents::OpenDocuments::install_scoped_hook(
            store,
            state.documents,
            std::sync::Arc::new(locations::views::LocationsWashHook { lists: state.lists }),
        );
        store.put_entity(
            state.chats,
            ahp_chat::chats::Chats::wired(
                state.recents,
                ahp_chat::chats::Catalog {
                    uris: std::sync::Arc::new(Hosts::uris),
                    agents: std::sync::Arc::new(|store, host| {
                        Hosts::host_ref(store, host)
                            .map(|row| row.agents.iter().cloned().collect())
                            .unwrap_or_default()
                    }),
                    channel: std::sync::Arc::new(Agents::channel),
                    note_turn: std::sync::Arc::new(Agents::note_turn),
                    folders: std::sync::Arc::new(|store, session| {
                        super::folders::session_folders(store, session)
                    }),
                },
            ),
        );
        store.update::<Hosts>(|hosts| {
            let mut host = match hosts.entries.get(&session.host) {
                Some(host) => host.clone(),
                None => {
                    hosts.order.push_back_mut(session.host);
                    Host::new("Local".to_owned())
                }
            };
            host.rows.insert_mut(session.session.clone(), state.clone());
            hosts.entries.insert_mut(session.host, host);
            hosts.generation += 1;
        });
        minted
    }

    /// The other half of the ceremony (docs/entities.md step 2,
    /// law 6): the session is the LIFETIME of everything its session
    /// row names. Removing the row RETRACTS every entity it minted —
    /// terminals' PTYs hang up on the drop, the backstop they always
    /// had. Removal is structural, so the generation bumps. The ONE
    /// deletion road; `scatter_session`'s all-empty sweep is mere
    /// housekeeping over the same retract.
    pub fn dispose_state(store: &mut Store, session: &ahp_wire::SessionId) {
        let session = &Self::addressed(store, session);
        let Some(state) = store
            .get::<Hosts>()
            .and_then(|hosts| hosts.entries.get(&session.host))
            .and_then(|host| host.rows.get(&session.session))
            .cloned()
        else {
            return;
        };
        store.update::<Hosts>(|hosts| {
            let Some(host) = hosts.entries.get(&session.host) else {
                return;
            };
            let mut host = host.clone();
            host.rows.remove_mut(&session.session);
            hosts.entries.insert_mut(session.host, host);
            hosts.generation += 1;
        });
        state.retract_all(store);
    }

    /// A session RENAMED (the composer's placeholder uri becomes the
    /// provider's real one): the session row moves to the new key — the
    /// ids never change, only the catalog's name for them. Windows
    /// keep their bundle through the rekey; this keeps the catalog
    /// telling the same story.
    pub fn rekey_state(store: &mut Store, from: &ahp_wire::SessionId, to: &ahp_wire::SessionId) {
        if from == to {
            return;
        }
        let Some(state) = Self::state(store, from).cloned() else {
            return;
        };
        if Self::state(store, to).is_some() {
            eprintln!("[higent] NOT rekeying {from:?} -> {to:?}: the target has a state");
            return;
        }
        store.update::<Hosts>(|hosts| {
            let Some(source) = hosts.entries.get(&from.host) else {
                return;
            };
            let mut source = source.clone();
            source.rows.remove_mut(&from.session);
            hosts.entries.insert_mut(from.host, source);
            let mut target = match hosts.entries.get(&to.host) {
                Some(host) => host.clone(),
                None => {
                    hosts.order.push_back_mut(to.host);
                    Host::new("Local".to_owned())
                }
            };
            target.rows.insert_mut(to.session.clone(), state.clone());
            hosts.entries.insert_mut(to.host, target);
            hosts.generation += 1;
        });
        if from.host != to.host {
            // Crossed hosts: the session's stamped uri map is the old
            // host's — re-stamp with the new one's.
            Self::stamp_host_uris(store, to.host);
        }
    }

    // No typed per-session doors here, deliberately: Hosts answers one
    // question — WHICH ids a session's session holds (`session`,
    // `ensure_state`) — and the collections are then read and written
    // BY ID (`store.entity` / `store.update_entity`), threaded to the
    // use sites (docs/entities.md law 3). A helper here that takes a
    // `SessionId` per read would remarry every collection to Hosts.

    /// Which session owns a documents collection — an ID COMPARE over
    /// the session rows, no content resolution: an addressed command
    /// scopes to its owner whether or not the addressed record still
    /// exists. The per-collection compares live with the collections
    /// (retired with AppEntity); this is their one iteration.

    /// The session whose documents collection this is — the sibling
    /// road for an edge that holds a documents id and needs the
    /// collection next to it (the stripe-base resolver).
    pub fn owner_of_documents(
        store: &Store,
        documents: Id<documents::OpenDocuments>,
    ) -> Option<&SessionState> {
        let hosts = store.get::<Hosts>()?;
        hosts.entries.values().find_map(|host| {
            host.rows
                .values()
                .find(|states| states.documents == documents)
        })
    }

    /// Which documents collection holds a document — the cold road
    /// for a landing that names only the document.
    pub fn documents_of_document(
        store: &Store,
        document: documents::DocumentId,
    ) -> Option<Id<documents::OpenDocuments>> {
        let hosts = store.get::<Hosts>()?;
        for (_, host) in hosts.entries.iter() {
            for (_, states) in host.rows.iter() {
                let documents = states.documents;
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
    /// re-mint road for a session row that names only the pair.
    /// The documents collection riding a watch subscription — the one
    /// id-less border road (file events arrive with a subscription and
    /// nothing else), found once, here.
    pub fn documents_of_watch(
        store: &Store,
        subscription: documents::watch::Subscription,
    ) -> Option<Id<documents::OpenDocuments>> {
        let hosts = store.get::<Hosts>()?;
        hosts.entries.values().find_map(|host| {
            host.rows.values().find_map(|states| {
                store
                    .entity(states.documents)
                    .is_some_and(|documents| documents.rides_watch(subscription))
                    .then_some(states.documents)
            })
        })
    }

    /// The session and session a documents collection belongs to — for
    /// a pane that holds the collection's id and needs the catalog's
    /// name for its folders beside the sibling ids.
    pub fn home_of_documents(
        store: &Store,
        documents: Id<documents::OpenDocuments>,
    ) -> Option<(ahp_wire::SessionId, SessionState)> {
        let hosts = store.get::<Hosts>()?;
        for (host, row) in hosts.entries.iter() {
            for (session, states) in row.rows.iter() {
                if states.documents == documents {
                    return Some((
                        ahp_wire::SessionId {
                            host: *host,
                            session: session.clone(),
                        },
                        states.clone(),
                    ));
                }
            }
        }
        None
    }

    pub fn rekey_local_sessions(&mut self, previous: Option<HostId>, target: HostId) {
        let fs = SessionUri::new(ahp_wire::LOCAL_FS_SESSION);
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
            let Some(states) = row.rows.get(&fs).cloned() else {
                continue;
            };
            let mut row = row.clone();
            row.rows.remove_mut(&fs);
            self.entries.insert_mut(source, row);
            let mut host = match self.entries.get(&target) {
                Some(host) => host.clone(),
                None => {
                    self.order.push_back_mut(target);
                    Host::new("Local".to_owned())
                }
            };
            if !host.rows.contains_key(&fs) {
                host.rows.insert_mut(fs.clone(), states);
            }
            self.entries.insert_mut(target, host);
        }
    }
}

/// The shell's window grip, installed at boot: a session a live
/// window HOLDS is not garbage, however empty — the sweep asks
/// through this road instead of knowing windows.
#[derive(Clone)]
pub struct WindowGrip(
    pub std::sync::Arc<dyn Fn(&Store, &ahp_wire::SessionId) -> bool + Send + Sync>,
);
