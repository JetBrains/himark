// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use ahp_session::session::agents::Agents;
use ahp_wire::effects::CreateChatEffect;
use ahp_wire::client::HostId;
use crate::app::AppCommand;
use crate::commands::DynamicCommand;
use ahp_wire::SessionId;
use crate::window::Windows;
use imba::effect::AnyEffect;
use imba::store::Store;

fn current_session(store: &Store, window: crate::window::WindowId) -> Option<SessionId> {
    Some(Windows::window_ref(store, window)?.current_session())
}

pub struct NewChat;

impl DynamicCommand for NewChat {
    fn id(&self) -> &'static str {
        "agent.new-chat"
    }

    fn name(&self) -> String {
        "New Agent Chat".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::app::Application,
        store: &mut Store,
        window: crate::window::WindowId,
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
                AppCommand::Dynamic(window, Arc::new(OpenCreatedChat { server, created }))
            }),
        );
    }
}

struct OpenCreatedChat {
    server: HostId,
    created: Result<ahp_wire::client::ChatUri, String>,
}

impl DynamicCommand for OpenCreatedChat {
    fn id(&self) -> &'static str {
        "agent.open-created-chat"
    }

    fn name(&self) -> String {
        "Open Created Chat".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::app::Application,
        store: &mut Store,
        window: crate::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
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
