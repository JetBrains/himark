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
