// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::commands::HimarkCommand;
use ::workbench::window::Windows;
use ahp_session::session::agents::Agents;
use ahp_wire::client::HostId;
use ahp_wire::effects::CreateChatEffect;
use ahp_wire::SessionId;
use imba::effect::AnyEffect;
use imba::store::Store;

pub struct NewChat;

impl HimarkCommand for NewChat {
    fn id(&self) -> &'static str {
        "agent.new-chat"
    }

    fn name(&self) -> String {
        "New Agent Chat".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // The SESSION road: the walk bound the session's id, so a
        // new chat opens from any shell.
        let Some(workspace) = bound.bound::<SessionId>().cloned() else {
            eprintln!("[shell] a session command landed with no session bound — dropped: new-chat");
            return;
        };
        let Some(key) = Agents::live_session(store, &workspace) else {
            return;
        };
        let Some(client) = ahp_wire::client::Servers::client(store, key.host) else {
            return;
        };
        let server = key.host;
        fx.push(
            AnyEffect::new(CreateChatEffect {
                client: client.chat.clone(),
                session: key.session.clone(),
            })
            .map({
                let bound = bound.clone();
                move |created| {
                    crate::shell::bound(
                        bound.clone(),
                        Arc::new(OpenCreatedChat { server, created }),
                    )
                }
            }),
        );
    }
}

struct OpenCreatedChat {
    server: HostId,
    created: Result<ahp_wire::client::ChatUri, String>,
}

impl HimarkCommand for OpenCreatedChat {
    fn id(&self) -> &'static str {
        "agent.open-created-chat"
    }

    fn name(&self) -> String {
        "Open Created Chat".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
        let ui = ui;
        let chat = match &self.created {
            Ok(chat) => chat.clone(),
            Err(error) => {
                eprintln!("[higent] createChat failed: {error}");
                return;
            }
        };

        let mut entity = Windows::window(store, window).expect("the window entity");
        let session = crate::workspace::entity_session(&entity).session;
        let pane = ahp_chat::chats::Chats::open(
            store,
            ui,
            crate::workspace::entity_state(&entity).chats(),
            self.server,
            session,
            chat,
        );

        let _ = entity.open_chat_panel(store, ui, pane, fx);
        Windows::put(store, window, entity);
    }
}
