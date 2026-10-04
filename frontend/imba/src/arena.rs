// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

/// Arena-allocated box/string/vec — bumpalo stays imba's private
/// dependency; these aliases are the public names.
pub type ArenaBox<'a, T> = bumpalo::boxed::Box<'a, T>;
pub type ArenaString<'a> = bumpalo::collections::String<'a>;
pub type ArenaVec<'a, T> = bumpalo::collections::Vec<'a, T>;

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
