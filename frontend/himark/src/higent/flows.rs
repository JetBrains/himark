// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The shell flows the embedders install at boot: how to START a
//! new session (a window command) and how to ADD a host (an
//! Application-level registration) — both shell vocabulary, kept
//! out of the catalog.

use std::sync::Arc;

use imba::store::Store;

use ahp_wire::client::HostId;

pub type NewSessionFlow =
    Arc<dyn Fn(HostId) -> Arc<dyn crate::commands::WindowedCommand> + Send + Sync>;

pub type AddHostFlow = Arc<dyn Fn(&mut Store, &str) -> Option<HostId> + Send + Sync>;

#[derive(Clone, Default)]
pub struct AgentFlows {
    new_session: Option<NewSessionFlow>,
    add_host: Option<AddHostFlow>,
}

impl AgentFlows {
    pub fn install_new_session(store: &mut Store, flow: NewSessionFlow) {
        store.update::<AgentFlows>(|flows| flows.new_session = Some(flow));
    }

    pub fn new_session_flow(store: &Store) -> Option<NewSessionFlow> {
        store.get::<AgentFlows>()?.new_session.clone()
    }

    pub fn install_add_host(store: &mut Store, flow: AddHostFlow) {
        store.update::<AgentFlows>(|flows| flows.add_host = Some(flow));
    }

    pub fn add_host_flow(store: &Store) -> Option<AddHostFlow> {
        store.get::<AgentFlows>()?.add_host.clone()
    }
}
