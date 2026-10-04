// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;

#[derive(Clone, Default)]
pub struct AppState {
    pub(crate) hosts: crate::higent::Hosts,

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
        self.gather(None, scope, &crate::higent::Servers::default())
    }

    #[allow(unused_variables)]
    pub(crate) fn gather(
        &self,
        window: Option<crate::WindowId>,
        scope: Option<&crate::SessionId>,
        seats: &crate::higent::Servers,
    ) -> Store {
        let mut store = self.globals.clone();
        store.put(self.hosts.clone());

        store.put(self.windows.project(window));

        store.put(seats.clone());
        store
    }

    pub(crate) fn scatter(&mut self, mut store: Store, scope: Option<&crate::SessionId>) {
        let _ = store.take::<crate::higent::Servers>();
        let mut hosts = store.take::<crate::higent::Hosts>().unwrap_or_default();
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
