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
    Arc<dyn Fn(HostId) -> Arc<dyn crate::commands::HimarkCommand> + Send + Sync>;

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

pub(crate) type AddHostFlow = Arc<dyn Fn(&mut Store, &str) -> Option<HostId> + Send + Sync>;

#[derive(Clone, Default)]
pub struct AgentFlows {
    new_session: Option<NewSessionFlow>,
    add_host: Option<AddHostFlow>,
}

impl AgentFlows {
    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn install_new_session(store: &mut Store, flow: NewSessionFlow) {
        store.update::<AgentFlows>(|flows| flows.new_session = Some(flow));
    }

    pub(crate) fn new_session_flow(store: &Store) -> Option<NewSessionFlow> {
        store.get::<AgentFlows>()?.new_session.clone()
    }

    pub fn install_add_host(store: &mut Store, flow: AddHostFlow) {
        store.update::<AgentFlows>(|flows| flows.add_host = Some(flow));
    }

    pub(crate) fn add_host_flow(store: &Store) -> Option<AddHostFlow> {
        store.get::<AgentFlows>()?.add_host.clone()
    }
}
