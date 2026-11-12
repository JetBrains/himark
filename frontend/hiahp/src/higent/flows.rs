// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The shell flows the embedders install at boot: how to START a
//! new session (a window command) and how to ADD a host (an
//! Himark-level registration) — both shell vocabulary, kept
//! out of the catalog.

use std::sync::Arc;

use imba::store::Store;

use ahp_wire::client::HostId;

pub type NewSessionFlow =
    Arc<dyn Fn(HostId) -> Arc<dyn himark::commands::HimarkCommand> + Send + Sync>;

/// The store-in-hand host mint — what the Add Host flow and embedder
/// discovery share. The SEAT IS MINTED BEFORE THE ROW IS SEEDED: a
/// row seeded first would connect against an unregistered seat and
/// land Failed("unregistered").
pub fn mint_agent_server(
    store: &mut Store,
    clients: &ahp_wire::fs::ClientDirectory,
    name: &str,
    client: ahp_wire::client::Client,
) -> HostId {
    let mut id = HostId::LOCAL;
    store.update::<ahp_wire::client::Servers>(|servers| id = servers.mint(client.clone()));
    ahp_session::session::agents::Agents::seed(store, id, name);
    ahp_session::session::state::Hosts::install_uris(store, id, Arc::new(ahp_wire::uris::FileUris));
    clients.record(id, client);
    id
}

/// Which URL each dialed host was minted at — so a host that
/// arrives by a second road (typed, then discovered on the tailnet;
/// saved, then typed again) is recognized instead of seated twice.
/// Hosts without a URL (the local socket, the VS Code discovery) are
/// not here.
#[derive(Clone, Default)]
pub struct HostUrls(rpds::HashTrieMapSync<String, HostId>);

impl HostUrls {
    pub fn note(store: &mut Store, url: &str, id: HostId) {
        let url = url.trim().to_owned();
        store.update::<HostUrls>(|urls| {
            urls.0.insert_mut(url, id);
        });
    }

    /// The host already seated at `url`, if any.
    pub fn host_at(store: &Store, url: &str) -> Option<HostId> {
        store.get::<HostUrls>()?.0.get(url.trim()).copied()
    }

    /// The URL a host was seated at, if it came by one.
    pub fn url_of(store: &Store, id: HostId) -> Option<String> {
        store
            .get::<HostUrls>()?
            .0
            .iter()
            .find_map(|(url, held)| (*held == id).then(|| url.clone()))
    }
}

/// `mint_agent_server` for a host dialed at a URL: the seat, the row,
/// and the URL noted — unless that URL is already a host, which is
/// then the answer.
pub fn mint_agent_server_at(
    store: &mut Store,
    clients: &ahp_wire::fs::ClientDirectory,
    name: &str,
    url: &str,
    client: ahp_wire::client::Client,
) -> HostId {
    if let Some(id) = HostUrls::host_at(store, url) {
        return id;
    }
    let id = mint_agent_server(store, clients, name, client);
    HostUrls::note(store, url, id);
    id
}

pub(crate) type AddHostFlow = Arc<dyn Fn(&mut Store, &str) -> Option<HostId> + Send + Sync>;

#[derive(Clone, Default)]
pub struct AgentFlows {
    new_session: Option<NewSessionFlow>,
    add_host: Option<AddHostFlow>,
    /// The composition layer's FOLD-BACK after a host lands: the
    /// workbench re-opens its drawer when the gesture rode a window;
    /// a mounted origin's catalog refreshes itself and this stays
    /// `None` or a no-op.
    refold: Option<Arc<dyn himark::commands::HimarkCommand>>,
}

impl AgentFlows {
    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn install_new_session(store: &mut Store, flow: NewSessionFlow) {
        store.update::<AgentFlows>(|flows| flows.new_session = Some(flow));
    }

    pub fn new_session_flow(store: &Store) -> Option<NewSessionFlow> {
        store.get::<AgentFlows>()?.new_session.clone()
    }

    pub fn install_add_host(store: &mut Store, flow: AddHostFlow) {
        store.update::<AgentFlows>(|flows| flows.add_host = Some(flow));
    }

    pub fn install_refold(store: &mut Store, refold: Arc<dyn himark::commands::HimarkCommand>) {
        store.update::<AgentFlows>(|flows| flows.refold = Some(refold));
    }

    pub(crate) fn refold(store: &Store) -> Option<Arc<dyn himark::commands::HimarkCommand>> {
        store.get::<AgentFlows>()?.refold.clone()
    }

    pub(crate) fn add_host_flow(store: &Store) -> Option<AddHostFlow> {
        store.get::<AgentFlows>()?.add_host.clone()
    }
}

pub struct AddHost {
    pub url: String,
}

impl himark::commands::HimarkCommand for AddHost {
    fn id(&self) -> &'static str {
        "agent.add-host"
    }

    fn name(&self) -> String {
        "Add Host".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(flow) = crate::higent::flows::AgentFlows::add_host_flow(store) else {
            eprintln!("[higent] no add-host capability installed — url dropped");
            return;
        };
        if flow(store, &self.url).is_none() {
            return;
        }
        // The composition layer's fold-back (the workbench re-opens
        // its drawer when a window is bound); a mounted origin's
        // catalog refreshes itself.
        if let Some(refold) = AgentFlows::refold(store) {
            refold.perform(store, ui, bound, fx);
        }
    }
}
