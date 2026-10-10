// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

/// Arena-allocated box/string/vec — bumpalo stays imba's private
/// dependency; these aliases are the crate's names. `ArenaBox` is
/// CRATE-PRIVATE on purpose: it carries ownership-transfer semantics
/// (`into_raw`/`from_raw`, `leak`) that can skip a destructor, and
/// only the erasure types (`ThunkBox`/`WidgetBox`/`LayoutBox`) are
/// trusted with that. Everyone else uses `alloc`, where the drop is
/// the arena's problem.
pub(crate) type ArenaBox<'a, T> = bumpalo::boxed::Box<'a, T>;
pub(crate) type ArenaString<'a> = bumpalo::collections::String<'a>;
pub(crate) type ArenaVec<'a, T> = bumpalo::collections::Vec<'a, T>;

use bumpalo::Bump;
use std::cell::RefCell;

/// One deferred destructor: a value parked in the bump until reset.
struct Finalizer {
    value: *mut u8,
    drop: unsafe fn(*mut u8),
}

unsafe fn drop_value<T>(value: *mut u8) {
    unsafe { std::ptr::drop_in_place(value.cast::<T>()) }
}

/// The frame arena — an ALLOCATOR, nothing more: `alloc` registers
/// the value's destructor and `reset` runs every one of them before
/// rewinding the bump, so parking a value here never skips its drop.
/// (Two leaks grew out of the old contract where it did: a leaked
/// rope cursor pinning whole rope snapshots, and `arena.alloc`ed
/// views pinning old document versions — 2026-10-10.)
///
/// `boxed` values are NOT registered: their `ArenaBox` is the owner
/// and runs the drop itself — it exists because the erasure types
/// MOVE values out of their slots (`Slot::take`), which a reset
/// finalizer could never know about. Both `boxed` and `ArenaBox`
/// are crate-private for exactly that reason: outside this crate
/// the arena has one verb, `alloc`, and drops are never a question.
#[derive(Default)]
pub struct Arena {
    bump: Bump,
    finalizers: RefCell<Vec<Finalizer>>,
}

impl Arena {
    pub fn reset(&mut self) {
        self.finalize();
        self.bump.reset();
    }

    /// Run the deferred destructors, newest first (later allocations
    /// may borrow earlier ones). A destructor may itself allocate —
    /// those late arrivals finalize too, before the bump rewinds.
    fn finalize(&mut self) {
        loop {
            let batch = std::mem::take(&mut *self.finalizers.borrow_mut());
            if batch.is_empty() {
                break;
            }
            for finalizer in batch.into_iter().rev() {
                // SAFETY: the pointer came from `alloc` on this bump,
                // whose memory stands until `bump.reset()` after this
                // loop; `take` hands each entry out exactly once; the
                // `&mut self` receiver means no outstanding borrows
                // of any arena value.
                unsafe { (finalizer.drop)(finalizer.value) };
            }
        }
    }

    pub fn alloc<T>(&self, value: T) -> &T {
        let slot = self.bump.alloc(value);
        if std::mem::needs_drop::<T>() {
            self.finalizers.borrow_mut().push(Finalizer {
                value: (slot as *mut T).cast(),
                drop: drop_value::<T>,
            });
        }
        slot
    }

    pub(crate) fn boxed<T>(&self, value: T) -> ArenaBox<'_, T> {
        ArenaBox::new_in(value, &self.bump)
    }

    pub fn string(&self, capacity: usize) -> ArenaString<'_> {
        ArenaString::with_capacity_in(capacity, &self.bump)
    }

    pub fn vec<T>(&self, capacity: usize) -> ArenaVec<'_, T> {
        ArenaVec::with_capacity_in(capacity, &self.bump)
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        self.finalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn alloc_runs_drops_at_reset() {
        let witness = Rc::new(());
        let mut arena = Arena::default();
        let _parked: &Rc<()> = arena.alloc(Rc::clone(&witness));
        assert_eq!(Rc::strong_count(&witness), 2);
        arena.reset();
        assert_eq!(
            Rc::strong_count(&witness),
            1,
            "reset must run the parked value's drop"
        );
    }

    #[test]
    fn alloc_runs_drops_when_the_arena_drops() {
        let witness = Rc::new(());
        {
            let arena = Arena::default();
            let _parked = arena.alloc(Rc::clone(&witness));
            assert_eq!(Rc::strong_count(&witness), 2);
        }
        assert_eq!(Rc::strong_count(&witness), 1);
    }
}
