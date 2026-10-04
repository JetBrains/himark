// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use hikit::{modal::ModalRequest, modal::ModalView, modal::RequestSlot};

use crate::app::AppCommand;


pub fn modal_scope(
    window: crate::window::WindowId,
) -> impl Fn(imba::dyn_view::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| AppCommand::Content(window, crate::window::WindowCommand::Modal(command))
}

pub fn dock_scope(
    window: crate::window::WindowId,
) -> impl Fn(imba::dyn_view::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| {
        AppCommand::Content(
            window,
            crate::window::WindowCommand::Dock(imba::dyn_view::DynCommand::new(crate::dock::DockCommand::Content(
                command,
            ))),
        )
    }
}

pub fn side_scope(
    window: crate::window::WindowId,
) -> impl Fn(imba::dyn_view::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| {
        AppCommand::Content(
            window,
            crate::window::WindowCommand::Side(imba::dyn_view::DynCommand::new(
                crate::drawer::DrawerCommand::Content(command),
            )),
        )
    }
}
