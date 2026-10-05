// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use ahp_session::session::agents::Agents;
use ahp_wire::effects::CreateChatEffect;
use ahp_wire::client::HostId;
use crate::app::AppCommand;
use crate::commands::WindowedCommand;
use ahp_wire::SessionId;
use ::workbench::window::Windows;
use imba::effect::AnyEffect;
use imba::store::Store;

fn current_session(store: &Store, window: ::workbench::window::WindowId) -> Option<SessionId> {
    Some(Windows::window_ref(store, window)?.current_session())
}

pub struct NewChat;

impl WindowedCommand for NewChat {
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
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(workspace) = current_session(store, window) else {
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
            .map(move |created| {
                AppCommand::Windowed(window, Arc::new(OpenCreatedChat { server, created }))
            }),
        );
    }
}

struct OpenCreatedChat {
    server: HostId,
    created: Result<ahp_wire::client::ChatUri, String>,
}

impl WindowedCommand for OpenCreatedChat {
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
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = ui;
        let chat = match &self.created {
            Ok(chat) => chat.clone(),
            Err(error) => {
                eprintln!("[higent] createChat failed: {error}");
                return;
            }
        };

        let mut entity = Windows::window(store, window).expect("the window entity");
        let session = entity.current_session().session;
        let pane = ahp_chat::chats::Chats::open(
            store,
            ui,
            entity.state().chats(),
            self.server,
            session,
            chat,
        );

        let _ = entity.open_chat_panel(store, ui, pane, fx);
        Windows::put(store, window, entity);
    }
}
