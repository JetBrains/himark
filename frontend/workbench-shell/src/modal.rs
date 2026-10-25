// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use himark::app::AppCommand;

pub fn modal_scope(
    window: ::workbench::window::WindowId,
) -> impl Fn(imba::dyn_view::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| {
        himark::shell::content(window, ::workbench::window::WindowCommand::Modal(command))
    }
}

pub fn dock_scope(
    window: ::workbench::window::WindowId,
) -> impl Fn(imba::dyn_view::DynCommand) -> AppCommand + Send + Clone + 'static {
    move |command| {
        himark::shell::content(
            window,
            ::workbench::window::WindowCommand::Dock(imba::dyn_view::DynCommand::new(
                ::workbench::dock::DockCommand::Content(command),
            )),
        )
    }
}
