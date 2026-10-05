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
    /// The OUTLINE road: an editor pane's drawer view, built whole by
    /// the shell (the outline is a component; the workbench only asks
    /// for the drawer's face).
    #[allow(clippy::type_complexity)]
    pub outline: Option<
        std::sync::Arc<
            dyn Fn(
                    &imba::store::Store,
                    &imba::ui::UiCtx,
                    imba::store::Id<documents::OpenDocuments>,
                    documents::DocumentId,
                    editor::location::ResourceLocation,
                    crate::window::WindowId,
                ) -> Box<dyn hikit::modal::ModalView>
                + Send
                + Sync,
        >,
    >,
    /// The drawer-toggle button face, registered by the shell's toc
    /// glue.
    pub drawer_button: Option<crate::toolbar::ToolbarButton>,
    /// The editor pane's services face (find bar, completion) — see
    /// `crate::services`.
    pub pane_services: Option<std::sync::Arc<dyn crate::services::PaneServices>>,
}

impl Registry {
    pub fn of(store: &Store) -> Option<&Registry> {
        store.get::<Registry>()
    }

    pub fn update(store: &mut Store, mutate: impl FnOnce(&mut Registry)) {
        store.update::<Registry>(mutate);
    }
}
