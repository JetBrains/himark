// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::effect::AnyEffect;
use imba::store::Store;
use imba::{UiCtx, View};

use crate::higent::ahp_types::common::Uri;
use crate::higent::chat::{ChatPanel, ChatPanelCommand};
use crate::{AppCommand, WindowId};

#[derive(Clone, Default)]
pub struct Chats {
    chats: rpds::HashTrieMapSync<Uri, ChatPanel>,
}

impl Chats {
    pub fn chat_ref<'a>(store: &'a Store, chat: &Uri) -> Option<&'a ChatPanel> {
        store.get::<Chats>()?.chats.get(chat)
    }

    pub fn chat(store: &Store, chat: &Uri) -> Option<ChatPanel> {
        Self::chat_ref(store, chat).cloned()
    }

    pub fn put(store: &mut Store, chat: Uri, panel: ChatPanel) {
        store.update::<Chats>(|chats| {
            chats.chats.insert_mut(chat, panel);
        });
    }

    pub fn remove(store: &mut Store, chat: &Uri) {
        store.update::<Chats>(|chats| {
            chats.chats.remove_mut(chat);
        });
    }

    pub fn open(
        store: &mut Store,
        ui: &imba::UiCtx,
        server: crate::higent::HostId,
        session: Uri,
        chat: Uri,
    ) -> Box<dyn crate::DynPanelView> {
        Self::open_with(store, ui, server, session, chat, None)
    }

    pub fn open_with(
        store: &mut Store,
        ui: &imba::UiCtx,
        server: crate::higent::HostId,
        session: Uri,
        chat: Uri,
        initial_prompt: Option<String>,
    ) -> Box<dyn crate::DynPanelView> {
        let known = store
            .get::<Chats>()
            .is_some_and(|chats| chats.chats.contains_key(&chat));
        if !known {
            let mut panel = ChatPanel::new(store, ui, server, session, chat.clone());
            if let Some(prompt) = initial_prompt {
                panel = panel.with_initial_prompt(prompt);
            }
            Self::put(store, chat.clone(), panel);
        }
        Box::new(ChatPane::new(chat))
    }

    pub fn list(store: &Store) -> Vec<Uri> {
        store
            .get::<Chats>()
            .map(|chats| chats.chats.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.chats.is_empty()
    }
}

pub(crate) struct ChatLanding {
    pub(crate) chat: Uri,
    pub(crate) command: ChatPanelCommand,
}

impl crate::LandingCommand for ChatLanding {
    fn perform(
        self: Box<Self>,
        app: &mut crate::Application,
        store: &mut Store,
        window: WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(mut panel) = Chats::chat(store, &self.chat) else {
            return;
        };
        let ui = app.ui_ctx();
        let ChatLanding { chat, command } = *self;
        let scope_chat = chat.clone();
        let session = panel.session_id();
        fx.scope(
            move |command: ChatPanelCommand| {
                AppCommand::InSession(
                    session.clone(),
                    Box::new(AppCommand::Landing(
                        window,
                        Box::new(ChatLanding {
                            chat: scope_chat.clone(),
                            command,
                        }),
                    )),
                )
            },
            |fx| panel.perform(store, ui.as_ref(), command, fx),
        );
        Chats::put(store, chat, panel);
    }
}

pub(crate) struct EnsureChatFeed {
    pub(crate) chat: Uri,
}

impl crate::DynamicCommand for EnsureChatFeed {
    fn id(&self) -> &'static str {
        "agent.chat-subscribe"
    }
    fn name(&self) -> String {
        "Subscribe Chat".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(mut panel) = Chats::chat(store, &self.chat) else {
            return;
        };
        let Some(seat) = crate::higent::Servers::seat(store, panel.server()) else {
            panel.mark_failed("unregistered server".to_owned());
            Chats::put(store, self.chat.clone(), panel);
            return;
        };
        let session = panel.session_id();
        Chats::put(store, self.chat.clone(), panel);
        let chat = self.chat.clone();
        let landing = self.chat.clone();
        fx.push(
            AnyEffect::new(crate::higent::SubscribeChatEffect { seat, chat }).map(move |result| {
                AppCommand::InSession(
                    session.clone(),
                    Box::new(AppCommand::Landing(
                        window,
                        Box::new(ChatLanding {
                            chat: landing.clone(),
                            command: ChatPanelCommand::Snapshot(result),
                        }),
                    )),
                )
            }),
        );
    }
}

#[derive(Clone)]
pub struct ChatPane {
    chat: Uri,
}

impl ChatPane {
    pub fn new(chat: Uri) -> Self {
        Self { chat }
    }

    pub fn chat(&self) -> &Uri {
        &self.chat
    }
}

impl imba::View for ChatPane {
    type Command = ChatPanelCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ChatPanelCommand> {
        match Chats::chat_ref(store, &self.chat) {
            Some(panel) => panel.focus_data(store, ui),
            None => imba::focus::FocusData::default(),
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        let Some(mut panel) = Chats::chat(store, &self.chat) else {
            return;
        };
        panel.perform(store, ui, command, fx);
        Chats::put(store, self.chat.clone(), panel);
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a imba::arena::Arena, constraints: imba::constraints::Constraints| {
                let widget: imba::ThunkBox<'a, Self::Command> =
                    match Chats::chat_ref(store, &self.chat) {
                        Some(panel) => imba::ThunkBox::new(
                            arena,
                            imba::Layout::layout(
                                panel.display(arena, store, ui),
                                arena,
                                constraints,
                            ),
                        ),

                        None => imba::ThunkBox::new(
                            arena,
                            imba::leaf::leaf(constraints.max.width, constraints.max.height),
                        ),
                    };
                widget
            },
        )
    }
}

impl crate::PanelView for ChatPane {
    type Place = crate::NoPlace;

    fn family_row(&self) -> Option<crate::FamilyRow> {
        Some(crate::FamilyRow::Chat(self.chat.clone()))
    }

    fn title(&self, store: &Store) -> String {
        Chats::chat_ref(store, &self.chat)
            .map(|panel| panel.title_text())
            .unwrap_or_else(|| "Agent Chat".to_owned())
    }

    fn collapsed_height(&self, store: &Store, nominal_height: f32) -> Option<f32> {
        Chats::chat_ref(store, &self.chat).map(|panel| panel.footer_height(store, nominal_height))
    }

    fn dismantle(&mut self, store: &mut Store) {
        Chats::remove(store, &self.chat);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
