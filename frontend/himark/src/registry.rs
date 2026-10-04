// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The ONE closed home for the app's registries. The store is not an
//! any-map to grab things from by type (docs/entities.md law 3):
//! what used to be six anonymous components — commands, navigators,
//! the keymap, row minters, toolbar buttons, the comments capability
//! — is one struct with NAMED fields, written through each module's
//! own register door and read through its `of`. Adding a registry
//! means adding a field here, in plain sight.

use imba::store::Store;

#[derive(Clone, Default)]
pub(crate) struct Registry {
    pub(crate) commands: crate::commands::Commands,
    pub(crate) navigators: crate::navigation::Navigators,
    /// `None` falls back to the embedded keymap.
    pub(crate) keymap: Option<crate::keymap::Keymap>,
    pub(crate) row_minters: crate::family_rows::RowMinters,
    pub(crate) toolbar_buttons: crate::toolbar::ToolbarButtons,
    /// The comments capability: armed by the shell that provides the
    /// annotation roads (`Comments::install`).
    pub(crate) comments: bool,
}

impl Registry {
    pub(crate) fn of(store: &Store) -> Option<&Registry> {
        store.get::<Registry>()
    }

    pub(crate) fn update(store: &mut Store, mutate: impl FnOnce(&mut Registry)) {
        store.update::<Registry>(mutate);
    }
}
