// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The agents feature's WINDOW half — what cannot leave the shell:
//! the drawer view, the open-session and new-chat gestures (they
//! switch and fill windows), the folder-grant picker, and the roads
//! the domain crates ask through. The catalog, the channel mirror,
//! the subscriptions and the sweeps live in `ahp-session`; the chat
//! itself in `ahp-chat`.

pub(crate) mod chat_roads;
pub mod drawer;

pub use ::hiahp::higent::folder_grant;
pub(crate) use crate::higent::folder_grant as folder_picker_gone;

pub mod new_chat;

pub mod open_session;

/// The agents feature as a plugin: the drawer toggle and its
/// toolbar button, the new-chat gesture.
pub struct AgentsPlugin;

impl himark::plugin::Plugin for AgentsPlugin {
    fn install(&self, app: &mut crate::app::Himark, _cx: &himark::plugin::PluginContext) {
        use crate::app::WorkbenchShell as _;
        use crate::app_ext::AppExt;
        use himark::app_ext::AppExt as _;
        app.register_command(std::sync::Arc::new(drawer::ToggleAgentsView));
        app.register_command(std::sync::Arc::new(new_chat::NewChat));
        app.register_toolbar_button(drawer::toolbar_button());
    }
}
