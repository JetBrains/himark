// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;

#[derive(Clone, Default)]
pub struct AppState {
    pub(crate) hosts: ahp_session::session::Hosts,

    pub(crate) windows: crate::Windows,

    pub(crate) globals: Store,
}

impl AppState {
    pub(crate) fn adopt(store: Store) -> Self {
        let mut state = AppState::default();
        state.scatter(store, None);
        state
    }

    pub(crate) fn gather_seatless(&self, scope: Option<&crate::SessionId>) -> Store {
        self.gather(None, scope, &ahp_wire::client::Servers::default())
    }

    #[allow(unused_variables)]
    pub(crate) fn gather(
        &self,
        window: Option<crate::WindowId>,
        scope: Option<&crate::SessionId>,
        clients: &ahp_wire::client::Servers,
    ) -> Store {
        let mut store = self.globals.clone();
        store.put(self.hosts.clone());

        store.put(self.windows.project(window));

        store.put(clients.clone());
        store
    }

    pub(crate) fn scatter(&mut self, mut store: Store, scope: Option<&crate::SessionId>) {
        let _ = store.take::<ahp_wire::client::Servers>();
        let mut hosts = store.take::<ahp_session::session::Hosts>().unwrap_or_default();
        if let Some(scope) = scope {
            hosts.scatter_session(&mut store, scope);
        }
        self.hosts = hosts;
        if let Some(taken) = store.take::<crate::Windows>() {
            self.windows.absorb(taken);
        }
        self.globals = store;
    }
}
