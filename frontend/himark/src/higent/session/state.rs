// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::higent::HostId;
use ahp_types::common::Uri;
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
    pub default_chat: Option<Uri>,

    pub working_directories: rpds::VectorSync<Uri>,

    pub config: Option<Arc<ahp_types::state::SessionConfigState>>,
}

#[derive(Clone)]
pub struct Host {
    pub name: String,
    pub status: HostStatus,
    pub agents: rpds::VectorSync<AgentInfo>,

    pub sessions: rpds::VectorSync<SessionSummary>,

    pub states: rpds::HashTrieMapSync<Uri, SessionChannel>,

    families: rpds::HashTrieMapSync<Uri, SessionState>,

    uris: Option<Arc<dyn crate::higent::ResourceUriMap>>,
}

#[derive(Clone, Default)]
pub(crate) struct SessionState {
    trees: crate::hifiles::SessionTree,

    recents: crate::RecentLocations,

    chats: crate::higent::Chats,

    changes: crate::hichanges::Changes,

    history: crate::hihistory::History,

    comments: crate::hicomments::Comments,

    terminals: crate::terminal::Terminals,

    documents: crate::OpenDocuments,

    scratch_names: documents::ScratchMint,
}

impl SessionState {
    fn gather_into(&self, store: &mut Store) {
        store.put(self.trees.clone());
        store.put(self.recents.clone());
        store.put(self.chats.clone());
        store.put(self.changes.clone());
        store.put(self.history.clone());
        store.put(self.comments.clone());
        store.put(self.terminals.clone());
        store.put(self.documents.clone());
        store.put(self.scratch_names.clone());
    }

    fn take_from(store: &mut Store) -> Self {
        Self {
            trees: store.take().unwrap_or_default(),
            recents: store.take().unwrap_or_default(),
            chats: store.take().unwrap_or_default(),
            changes: store.take().unwrap_or_default(),
            history: store.take().unwrap_or_default(),
            comments: store.take().unwrap_or_default(),
            terminals: store.take().unwrap_or_default(),
            documents: store.take().unwrap_or_default(),
            scratch_names: store.take().unwrap_or_default(),
        }
    }

    fn is_empty(&self) -> bool {
        self.trees.is_empty()
            && self.recents.is_empty()
            && self.chats.is_empty()
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

    pub fn summary(&self, session: &Uri) -> Option<&SessionSummary> {
        self.sessions
            .iter()
            .find(|summary| &summary.resource == session)
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
        let families = SessionState::take_from(store);
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
        let fs: Uri = host_discovery::LOCAL_FS_SESSION.to_owned();
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

    pub(crate) fn assert_no_family_orphans(store: &mut Store) {
        let orphans = SessionState::take_from(store);
        debug_assert!(
            orphans.is_empty(),
            "a session-family write happened in a batch gathered without a session scope"
        );
    }
}
