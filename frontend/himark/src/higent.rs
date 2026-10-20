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
pub mod flows;
pub mod folder_grant;
pub mod new_chat;
pub mod open_session;

/// The SESSIONS browser as a plugin: hosts, their folders, their
/// sessions and the speed search over them — the engine's drawer,
/// the same one the workbench stands in a side slot. Its asks are
/// the mounted road's: a pick ENTERS the session and then hands the
/// shell the place naming it.
pub struct SessionsPlugin;

impl crate::plugin::Plugin for SessionsPlugin {
    fn install(&self, app: &mut crate::app::Himark, _cx: &crate::plugin::PluginContext) {
        use std::sync::Arc;
        app.register_place_arm(Arc::new(|store, ui, id, _fx, place| {
            let place = place.place::<crate::places::SessionsPlace>()?;
            let showing = place.0.clone();
            let asks = Arc::new(ahp_session::session::drawer::DrawerAsks {
                open_session: Arc::new(move |_store| Some(showing.clone())),
                open_row: Arc::new(move |server, session| {
                    imba::command::Verb::once(open_session::ShowSession {
                        origin: id,
                        server,
                        session,
                    })
                }),
                new_session: Arc::new(move |_store, server| {
                    imba::command::Verb::once(open_session::CreateHeadless {
                        host: Some(server),
                    })
                }),
                add_host: Arc::new(move |url| {
                    crate::app::shell_verb(crate::shell::windowed(
                        ::workbench::window::WindowId::from_raw(id.raw()),
                        Arc::new(drawer::AddHost { url }),
                    ))
                }),
            });
            Some(Box::new(ahp_session::session::drawer::AgentsPanel::open(
                store, ui, asks,
            )))
        }));
    }
}

/// The agents feature as a plugin: the drawer toggle and its
/// toolbar button, the new-chat gesture.
pub struct AgentsPlugin;

impl crate::plugin::Plugin for AgentsPlugin {
    fn install(&self, app: &mut crate::app::Himark, _cx: &crate::plugin::PluginContext) {
        use crate::app_ext::AppExt;
        app.register_command(std::sync::Arc::new(drawer::ToggleAgentsView));
        app.register_command(std::sync::Arc::new(new_chat::NewChat));
        app.register_toolbar_button(drawer::toolbar_button());
    }
}
