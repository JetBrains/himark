// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The app's closed registry: the commands and the keymap. The
//! workbench furniture has its own (`workbench::registry`).

use imba::store::Store;

#[derive(Clone, Default)]
pub(crate) struct Registry {
    pub(crate) commands: crate::commands::Commands,
    /// `None` falls back to the embedded keymap.
    pub(crate) keymap: Option<crate::keymap::Keymap>,
}

impl Registry {
    pub(crate) fn of(store: &Store) -> Option<&Registry> {
        store.get::<Registry>()
    }

    pub(crate) fn update(store: &mut Store, mutate: impl FnOnce(&mut Registry)) {
        store.update::<Registry>(mutate);
    }
}
