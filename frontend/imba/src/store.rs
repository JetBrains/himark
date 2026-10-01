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

/// One row's slot. A LEASED row is out with its perform
/// (docs/entities.md law 5): the marker stays behind so a read during
/// the lease is distinguishable from gone — that read is a REENTRANCY
/// BUG, not a race.
#[derive(Clone)]
enum Slot<T> {
    Present(T),
    Leased,
}

/// One kind's entity rows — an ordinary component, so the table
/// rides every store clone and projection as pointer bumps.
#[derive(Clone)]
struct EntityRows<T>(HashTrieMapSync<u64, Slot<T>>);

impl<T> Default for EntityRows<T> {
    fn default() -> Self {
        Self(HashTrieMapSync::new_sync())
    }
}

/// A collection addressable by `At(Id<T>, T::Command)` — the one
/// command road (docs/entities.md law 5). `perform` runs under a
/// LEASE: own state is `self`, siblings are reached through the store
/// it is handed, and effects are already scoped to `Self::Command` —
/// the router stamps the address. `destroy` runs at dispose and
/// retracts the entities this one owns.
pub trait Entity: Component {
    type Command: 'static;

    fn perform(
        &mut self,
        id: Id<Self>,
        command: Self::Command,
        store: &mut Store,
        ui: &crate::ui::UiCtx,
        fx: &mut crate::effect::Effects<'_, Self::Command>,
    );

    fn destroy(&mut self, store: &mut Store);
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
    /// convention (docs/entities.md). A read during the row's own
    /// perform is a REENTRANCY BUG: it panics in debug and answers
    /// `None` with a loud log in release — never silently.
    pub fn entity<T: Component>(&self, id: Id<T>) -> Option<&T> {
        match self.get::<EntityRows<T>>()?.0.get(&id.serial)? {
            Slot::Present(value) => Some(value),
            Slot::Leased => {
                debug_assert!(
                    false,
                    "{id:?} read during its own perform (lease reentrancy)"
                );
                eprintln!("[store] {id:?} read during its own perform (lease reentrancy)");
                None
            }
        }
    }

    pub fn put_entity<T: Component>(&mut self, id: Id<T>, value: T) {
        self.update::<EntityRows<T>>(|rows| {
            rows.0.insert_mut(id.serial, Slot::Present(value));
        });
    }

    /// Take the row out for its perform, leaving the lease MARKER —
    /// the take half of the one command road (docs/entities.md law 5).
    /// `None` means gone (the discard road); leasing a leased row is
    /// the same reentrancy bug as reading one.
    pub fn lease<T: Component>(&mut self, id: Id<T>) -> Option<T> {
        let slot = self.get::<EntityRows<T>>()?.0.get(&id.serial)?.clone();
        match slot {
            Slot::Present(value) => {
                self.update::<EntityRows<T>>(|rows| {
                    rows.0.insert_mut(id.serial, Slot::Leased);
                });
                Some(value)
            }
            Slot::Leased => {
                debug_assert!(
                    false,
                    "{id:?} leased during its own perform (lease reentrancy)"
                );
                eprintln!("[store] {id:?} leased during its own perform (lease reentrancy)");
                None
            }
        }
    }

    /// Put the leased row back. If the row was RETRACTED during its
    /// own perform (self-dispose, a cascade), the slot is gone and the
    /// returned value drops — retraction wins.
    pub fn unlease<T: Component>(&mut self, id: Id<T>, value: T) {
        let occupied = self
            .get::<EntityRows<T>>()
            .and_then(|rows| rows.0.get(&id.serial));
        match occupied {
            Some(Slot::Leased) => self.update::<EntityRows<T>>(|rows| {
                rows.0.insert_mut(id.serial, Slot::Present(value));
            }),
            Some(Slot::Present(_)) => {
                debug_assert!(false, "{id:?} replaced while leased");
                eprintln!("[store] {id:?} replaced while leased; the replacement stands");
            }
            None => {}
        }
    }

    /// Route one addressed command: lease the row, perform with
    /// effects stamped by `address`, put it back. A gone target
    /// discards the command — the race road, not an error.
    pub fn route<T: Entity, R: 'static>(
        &mut self,
        id: Id<T>,
        command: T::Command,
        ui: &crate::ui::UiCtx,
        address: impl Fn(T::Command) -> R + Send + Clone + 'static,
        fx: &mut crate::effect::Effects<'_, R>,
    ) {
        let Some(mut row) = self.lease(id) else {
            return;
        };
        fx.scope(address, |fx| row.perform(id, command, self, ui, fx));
        self.unlease(id, row);
    }

    /// Dispose an entity (docs/entities.md law 6): the row leaves the
    /// table FIRST, then its `destroy` runs and retracts what it owns
    /// — teardown cascades by ownership.
    pub fn dispose<T: Entity>(&mut self, id: Id<T>) {
        if let Some(mut row) = self.retract(id) {
            row.destroy(self);
        }
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
    /// still in flight resolves `None` from here on. Retracting a
    /// LEASED row removes the marker too — retraction wins, and the
    /// perform's `unlease` finds the slot gone and drops the value.
    pub fn retract<T: Component>(&mut self, id: Id<T>) -> Option<T> {
        let slot = self
            .get::<EntityRows<T>>()
            .and_then(|rows| rows.0.get(&id.serial))
            .cloned();
        if slot.is_some() {
            self.update::<EntityRows<T>>(|rows| {
                rows.0.remove_mut(&id.serial);
            });
        }
        match slot {
            Some(Slot::Present(value)) => Some(value),
            Some(Slot::Leased) | None => None,
        }
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

    #[test]
    fn a_lease_takes_the_row_and_unlease_restores_it() {
        let mut store = Store::new();
        let id = Id::<Counter>::mint();
        store.put_entity(id, Counter(7));
        let mut row = store.lease(id).expect("the row is present");
        row.0 += 1;
        store.unlease(id, row);
        assert_eq!(store.entity(id), Some(&Counter(8)));
    }

    #[test]
    #[should_panic(expected = "lease reentrancy")]
    fn reading_a_leased_row_is_a_reentrancy_bug() {
        let mut store = Store::new();
        let id = Id::<Counter>::mint();
        store.put_entity(id, Counter(1));
        let _row = store.lease(id).expect("the row is present");
        let _ = store.entity(id);
    }

    #[test]
    fn leasing_a_gone_row_is_the_discard_road() {
        let mut store = Store::new();
        let id = Id::<Counter>::mint();
        assert!(store.lease(id).is_none());
    }

    /// Retraction during the row's own perform wins: the marker goes
    /// with the slot, and the perform's unlease drops the value.
    #[test]
    fn retracting_a_leased_row_beats_the_unlease() {
        let mut store = Store::new();
        let id = Id::<Counter>::mint();
        store.put_entity(id, Counter(3));
        let row = store.lease(id).expect("the row is present");
        assert_eq!(store.retract(id), None, "the value is out with the lease");
        store.unlease(id, row);
        assert_eq!(store.entity(id), None, "retraction stands");
    }

    #[derive(Clone)]
    struct Owner {
        owned: Id<Counter>,
    }

    impl Entity for Owner {
        type Command = u64;
        fn perform(
            &mut self,
            _id: Id<Self>,
            command: u64,
            store: &mut Store,
            _ui: &crate::ui::UiCtx,
            _fx: &mut crate::effect::Effects<'_, u64>,
        ) {
            store.update_entity(self.owned, |counter| counter.0 += command);
        }
        fn destroy(&mut self, store: &mut Store) {
            store.retract(self.owned);
        }
    }

    #[test]
    fn dispose_runs_destroy_and_the_cascade_retracts_the_owned() {
        let mut store = Store::new();
        let owned = Id::<Counter>::mint();
        store.put_entity(owned, Counter(1));
        let owner = Id::<Owner>::mint();
        store.put_entity(owner, Owner { owned });
        store.dispose(owner);
        assert!(store.entity(owner).is_none());
        assert!(store.entity(owned).is_none(), "the cascade retracts");
    }

    #[test]
    fn route_leases_performs_and_restores() {
        let mut store = Store::new();
        let owned = Id::<Counter>::mint();
        store.put_entity(owned, Counter(0));
        let owner = Id::<Owner>::mint();
        store.put_entity(owner, Owner { owned });
        let ui = crate::ui::UiCtx::dont_use_too_slow();
        let mut batch = crate::effect::Batch::<u64>::new();
        store.route(owner, 5, &ui, |command| command, &mut batch.effects());
        assert_eq!(store.entity(owned), Some(&Counter(5)));
        assert!(store.entity(owner).is_some(), "the row came back");
    }
}
