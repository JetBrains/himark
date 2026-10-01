// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::{
    any::{Any, TypeId},
    marker::PhantomData,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use rpds::HashTrieMapSync;

pub trait Component: Clone + Send + Sync + 'static {}

impl<T: Clone + Send + Sync + 'static> Component for T {}

/// The identity of one ENTITY: a value living in the store's flat
/// per-kind table, reached by id instead of by type
/// (docs/entities.md). Serials come from one app-global monotonic
/// mint and are never reused, so a dangling id can only ever be a
/// true dangle — never an aliased slot. The id confers no lifetime:
/// every reference is weak, and `Store::entity` answers `None` for
/// the retracted.
pub struct Id<T> {
    serial: u64,
    marker: PhantomData<fn() -> T>,
}

static MINT: AtomicU64 = AtomicU64::new(1);

impl<T> Id<T> {
    pub fn mint() -> Self {
        Self {
            serial: MINT.fetch_add(1, Ordering::Relaxed),
            marker: PhantomData,
        }
    }
}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Id<T> {}

impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.serial == other.serial
    }
}

impl<T> Eq for Id<T> {}

impl<T> std::hash::Hash for Id<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.serial.hash(state);
    }
}

impl<T> std::fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Id<{}>({})",
            std::any::type_name::<T>().rsplit("::").next().unwrap_or(""),
            self.serial
        )
    }
}

/// One kind's entity rows — an ordinary component, so the table
/// rides every store clone and projection as pointer bumps.
#[derive(Clone)]
struct EntityRows<T>(HashTrieMapSync<u64, T>);

impl<T> Default for EntityRows<T> {
    fn default() -> Self {
        Self(HashTrieMapSync::new_sync())
    }
}

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

    /// Resolve an entity by id. `None` means retracted (or never
    /// put) — the caller handles or discards, the gone-document
    /// convention (docs/entities.md).
    pub fn entity<T: Component>(&self, id: Id<T>) -> Option<&T> {
        self.get::<EntityRows<T>>()?.0.get(&id.serial)
    }

    pub fn put_entity<T: Component>(&mut self, id: Id<T>, value: T) {
        self.update::<EntityRows<T>>(|rows| {
            rows.0.insert_mut(id.serial, value);
        });
    }

    /// Mutate an entity in place, minting the row from `Default` on
    /// first write — the lazily-created-row door.
    pub fn update_entity<T: Component + Default>(
        &mut self,
        id: Id<T>,
        mutate: impl FnOnce(&mut T),
    ) {
        let mut value = self.entity(id).cloned().unwrap_or_default();
        mutate(&mut value);
        self.put_entity(id, value);
    }

    /// Remove an entity's row. Manual lifecycle: retraction is the
    /// OWNER's duty at dispose (docs/entities.md step 2); every id
    /// still in flight resolves `None` from here on.
    pub fn retract<T: Component>(&mut self, id: Id<T>) -> Option<T> {
        let value = self.entity(id).cloned();
        if value.is_some() {
            self.update::<EntityRows<T>>(|rows| {
                rows.0.remove_mut(&id.serial);
            });
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

    #[test]
    fn entities_resolve_by_id_and_ids_never_collide() {
        let mut store = Store::new();
        let first = Id::<Counter>::mint();
        let second = Id::<Counter>::mint();
        assert_ne!(first, second);
        store.put_entity(first, Counter(1));
        store.put_entity(second, Counter(2));
        assert_eq!(store.entity(first), Some(&Counter(1)));
        assert_eq!(store.entity(second), Some(&Counter(2)));
    }

    #[test]
    fn a_retracted_entity_resolves_none() {
        let mut store = Store::new();
        let id = Id::<Counter>::mint();
        store.put_entity(id, Counter(7));
        assert_eq!(store.retract(id), Some(Counter(7)));
        assert_eq!(store.entity(id), None);
        assert_eq!(store.retract(id), None);
    }

    #[test]
    fn an_entity_snapshot_is_isolated_from_later_writes() {
        let mut store = Store::new();
        let id = Id::<Counter>::mint();
        store.put_entity(id, Counter(1));
        let snapshot = store.clone();
        store.update_entity(id, |counter| counter.0 = 2);
        assert_eq!(snapshot.entity(id), Some(&Counter(1)));
        assert_eq!(store.entity(id), Some(&Counter(2)));
    }

    #[test]
    fn update_entity_mints_the_missing_row_from_default() {
        let mut store = Store::new();
        let id = Id::<Counter>::mint();
        store.update_entity(id, |counter| counter.0 += 3);
        assert_eq!(store.entity(id), Some(&Counter(3)));
    }
}
