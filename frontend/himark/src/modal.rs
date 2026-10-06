// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::app::AppCommand;

pub fn modal_scope(
    window: ::workbench::window::WindowId,
) -> impl Fn(imba::dyn_view::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| AppCommand::Content(window, ::workbench::window::WindowCommand::Modal(command))
}

pub fn dock_scope(
    window: ::workbench::window::WindowId,
) -> impl Fn(imba::dyn_view::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| {
        AppCommand::Content(
            window,
            ::workbench::window::WindowCommand::Dock(imba::dyn_view::DynCommand::new(
                ::workbench::dock::DockCommand::Content(command),
            )),
        )
    }
}

pub fn side_scope(
    window: ::workbench::window::WindowId,
) -> impl Fn(imba::dyn_view::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| {
        AppCommand::Content(
            window,
            ::workbench::window::WindowCommand::Side(imba::dyn_view::DynCommand::new(
                ::workbench::drawer::DrawerCommand::Content(command),
            )),
        )
    }
}
