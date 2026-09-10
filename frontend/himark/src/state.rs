use imba::store::Store;

#[derive(Clone, Default)]
pub struct AppState {
    pub(crate) hosts: crate::higent::Hosts,

    pub(crate) windows: crate::Windows,

    pub(crate) globals: Store,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Gathered(pub crate::SessionId);

impl Gathered {
    pub fn scope(store: &Store) -> Option<&crate::SessionId> {
        store.get::<Gathered>().map(|gathered| &gathered.0)
    }
}

impl AppState {
    pub(crate) fn adopt(store: Store) -> Self {
        let mut state = AppState::default();
        state.scatter(store);
        state
    }

    pub(crate) fn gather_seatless(&self, scope: Option<&crate::SessionId>) -> Store {
        self.gather(None, scope, &crate::higent::Servers::default())
    }

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
        if let Some(scope) = scope {
            self.hosts.gather_session(&mut store, scope);
            store.put(Gathered(scope.clone()));
        }
        store
    }

    pub(crate) fn scatter(&mut self, mut store: Store) {
        let _ = store.take::<crate::higent::Servers>();
        let scope = store.take::<Gathered>().map(|gathered| gathered.0);
        let mut hosts = store.take::<crate::higent::Hosts>().unwrap_or_default();
        match scope {
            Some(scope) => hosts.scatter_session(&mut store, &scope),

            None => crate::higent::Hosts::assert_no_family_orphans(&mut store),
        }
        self.hosts = hosts;
        if let Some(taken) = store.take::<crate::Windows>() {
            self.windows.absorb(taken);
        }
        self.globals = store;
    }
}
