use std::sync::Arc;

use super::Agents;
use crate::higent::{CreateChatEffect, HostId};
use crate::{AppCommand, DynamicCommand, SessionId, Windows};
use ahp_types::common::Uri;
use imba::effect::AnyEffect;
use imba::store::Store;

fn current_session(store: &Store, window: crate::WindowId) -> Option<SessionId> {
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
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(workspace) = current_session(store, window) else {
            return;
        };
        let Some(key) = Agents::live_session(store, &workspace) else {
            return;
        };
        let Some(seat) = crate::higent::Servers::seat(store, key.host) else {
            return;
        };
        let server = key.host;
        fx.push(
            AnyEffect::new(CreateChatEffect {
                seat,
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
    created: Result<Uri, String>,
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
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let chat = match &self.created {
            Ok(chat) => chat.clone(),
            Err(error) => {
                eprintln!("[higent] createChat failed: {error}");
                return;
            }
        };

        let session = Windows::window_ref(store, window)
            .expect("the window entity")
            .current_session()
            .session;
        let pane = crate::higent::Chats::open(store, self.server, session, chat);
        let mut entity = Windows::window(store, window).expect("the window entity");

        if crate::FloatingChat::on(store) {
            entity.open_bottom(pane);
        } else {
            let _ = entity.open_panel(store, pane, fx);
        }
        Windows::put(store, window, entity);
    }
}
