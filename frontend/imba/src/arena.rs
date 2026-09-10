pub use bumpalo::boxed::Box as ArenaBox;
pub use bumpalo::collections::{String as ArenaString, Vec as ArenaVec};

use bumpalo::Bump;

#[derive(Default)]
pub struct Arena {
    bump: Bump,
}

impl Arena {
    pub fn reset(&mut self) {
        self.bump.reset();
    }

    pub fn alloc<T>(&self, value: T) -> &T {
        self.bump.alloc(value)
    }

    pub fn boxed<T>(&self, value: T) -> ArenaBox<'_, T> {
        ArenaBox::new_in(value, &self.bump)
    }

    pub fn string(&self, capacity: usize) -> ArenaString<'_> {
        ArenaString::with_capacity_in(capacity, &self.bump)
    }

    pub fn vec<T>(&self, capacity: usize) -> ArenaVec<'_, T> {
        ArenaVec::with_capacity_in(capacity, &self.bump)
    }
}
