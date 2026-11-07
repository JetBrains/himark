// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The agents feature's ENGINE half: the headless session roads
//! (`open_session`). The windowed half — the drawer, its toolbar
//! button, the new-chat gesture — lives in the workbench shell.

pub mod flows;
pub mod folder_grant;
pub mod folder_picker;
pub mod open_session;


/// The SESSIONS browser as a plugin: hosts, their folders, their
/// sessions and the speed search over them — the engine's drawer,
/// the same one the workbench stands in a side slot. Its asks are
/// the mounted road's: a pick ENTERS the session and then hands the
/// shell the place naming it.
pub struct SessionsPlugin;

impl himark::plugin::Plugin for SessionsPlugin {
    fn install(&self, app: &mut himark::app::Himark, _cx: &himark::plugin::PluginContext) {
        use std::sync::Arc;
        app.register_bridge_arm(Arc::new(|app, place| {
            let place = place.place::<crate::places::SessionsPlace>()?;
            let showing = place.0.clone();
            app.mount(move |store, ui, id, _fx| {
                let asks = Arc::new(ahp_session::session::drawer::DrawerAsks {
                open_session: Arc::new(move |_store| Some(showing.clone())),
                open_row: Arc::new(move |server, session| {
                    imba::command::Verb::once(crate::higent::open_session::ShowSession {
                        origin: id,
                        server,
                        session,
                    })
                }),
                new_session: Arc::new(move |_store, server| {
                    imba::command::Verb::once(crate::higent::open_session::CreateHeadless { host: Some(server) })
                }),
                add_host: Arc::new(move |url| {
                    himark::app::shell_verb(himark::perform::AppCommand::AtOrigin(
                        id,
                        Arc::new(crate::higent::flows::AddHost { url }),
                    ))
                }),
                });
                Some(himark::mounts::dyn_modal_mount(
                    id,
                    Box::new(ahp_session::session::drawer::AgentsPanel::open(
                        store, ui, asks,
                    )),
                ))
            })
        }));
    }
}

