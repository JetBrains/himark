// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench's closed registry: navigators, row minters, toolbar
//! buttons, and the shell-installed roads (command run, outline jump,
//! the drawer button). One struct, named fields, written through each
//! module's register door.

use imba::store::Store;

#[derive(Clone, Default)]
pub struct Registry {
    pub(crate) navigators: crate::navigation::Navigators,
    pub(crate) row_minters: crate::rows::RowMinters,
    pub(crate) toolbar_buttons: crate::toolbar::ToolbarButtons,
    /// The shell's command-palette run road: the workbench's
    /// RunCommand arm fires a registered command by id without
    /// knowing the command registry's shape.
    pub run_command: Option<std::sync::Arc<dyn Fn(&mut Store, &str) + Send + Sync>>,
    /// The outline JUMP road: the shell builds the window-addressed
    /// perform; the workbench only mounts the outline with it.
    pub outline_jump: Option<
        std::sync::Arc<
            dyn Fn(
                    crate::window::WindowId,
                    hikit::navigation::EditorPlace,
                ) -> hikit::modal::ModalRequest
                + Send
                + Sync,
        >,
    >,
    /// The drawer-toggle button face, registered by the shell's toc
    /// glue.
    pub drawer_button: Option<crate::toolbar::ToolbarButton>,
}

impl Registry {
    pub fn of(store: &Store) -> Option<&Registry> {
        store.get::<Registry>()
    }

    pub fn update(store: &mut Store, mutate: impl FnOnce(&mut Registry)) {
        store.update::<Registry>(mutate);
    }
}
