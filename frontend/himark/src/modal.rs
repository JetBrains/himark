// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::app::AppCommand;

pub use hikit::{ModalRequest, ModalView, RequestSlot};

pub fn modal_scope(
    window: crate::WindowId,
) -> impl Fn(imba::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| AppCommand::Content(window, crate::WindowCommand::Modal(command))
}

pub fn dock_scope(
    window: crate::WindowId,
) -> impl Fn(imba::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| {
        AppCommand::Content(
            window,
            crate::WindowCommand::Dock(imba::DynCommand::new(crate::dock::DockCommand::Content(
                command,
            ))),
        )
    }
}

pub fn side_scope(
    window: crate::WindowId,
) -> impl Fn(imba::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| {
        AppCommand::Content(
            window,
            crate::WindowCommand::Side(imba::DynCommand::new(
                crate::drawer::DrawerCommand::Content(command),
            )),
        )
    }
}
