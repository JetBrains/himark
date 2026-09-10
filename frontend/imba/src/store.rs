use std::{
    any::{Any, TypeId},
    sync::Arc,
};

use rpds::HashTrieMapSync;

pub trait Component: Clone + Send + Sync + 'static {}

impl<T: Clone + Send + Sync + 'static> Component for T {}

#[derive(Clone, Default)]
pub struct Store {
    components: HashTrieMapSync<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get<T: Component>(&self) -> Option<&T> {
        self.components
            .get(&TypeId::of::<T>())
            .and_then(|component| component.downcast_ref::<T>())
    }

    pub fn put<T: Component>(&mut self, value: T) {
        self.components
            .insert_mut(TypeId::of::<T>(), Arc::new(value));
    }

    pub fn update<T: Component + Default>(&mut self, mutate: impl FnOnce(&mut T)) {
        let mut value = self.get::<T>().cloned().unwrap_or_default();
        mutate(&mut value);
        self.put(value);
    }

    pub fn take<T: Component>(&mut self) -> Option<T> {
        let value = self.get::<T>().cloned();
        if value.is_some() {
            self.components.remove_mut(&TypeId::of::<T>());
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default, PartialEq, Debug)]
    struct Counter(u64);

    #[test]
    fn a_snapshot_is_isolated_from_later_writes() {
        let mut store = Store::new();
        store.put(Counter(1));
        let snapshot = store.clone();
        store.update::<Counter>(|counter| counter.0 = 2);
        assert_eq!(snapshot.get::<Counter>(), Some(&Counter(1)));
        assert_eq!(store.get::<Counter>(), Some(&Counter(2)));
    }

    #[test]
    fn update_creates_the_missing_component_from_default() {
        let mut store = Store::new();
        store.update::<Counter>(|counter| counter.0 += 5);
        assert_eq!(store.get::<Counter>(), Some(&Counter(5)));
    }
}
