// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::effect::AnyEffect;
use imba::store::Store;
use imba::UiCtx;

use crate::higent::chat::{ChatPanel, ChatPanelCommand, ChatViewId};
use crate::higent::{ChatUri, SessionUri};
use crate::{AppCommand, WindowId};

/// The conversations, ONE home for all of them: the global store.
///
/// The conversations of ONE session, held in that session's family —
/// the session is the lifetime of its chats.
///
/// The family is never gathered into the store as a component. It used
/// to be, and that was the bug: a command gathered for session A holds
/// A's chats, and the roads that touch a chat do not agree on which
/// session that is. The pane's own commands are scoped to the WINDOW's
/// current session (a file in another session focused, a session being
/// entered while the window still points at the previous one), while
/// the chat's feed landings are scoped to the chat's own. Two scopes
/// meant two families, each holding a different version of the same
/// conversation, and whichever gather came last won: a message showed
/// for one frame and vanished, a reply went missing on the walk back
/// with ⌘I, and re-entering a session minted a FRESH record over a
/// live one.
///
/// So every road reaches a chat by its OWN `SessionId`, through
/// `Hosts` — which every gather carries whole, scope or no scope.
#[derive(Clone, Default)]
pub struct Chats {
    chats: rpds::HashTrieMapSync<ChatUri, ChatPanel>,
}

impl Chats {
    pub fn chat_ref<'a>(
        store: &'a Store,
        session: &crate::SessionId,
        chat: &ChatUri,
    ) -> Option<&'a ChatPanel> {
        crate::higent::Hosts::chats_of(store, session)?
            .chats
            .get(chat)
    }

    pub fn chat(store: &Store, session: &crate::SessionId, chat: &ChatUri) -> Option<ChatPanel> {
        Self::chat_ref(store, session, chat).cloned()
    }

    /// The chat and the session that owns it, for the cold roads that
    /// hold a uri and nothing else.
    pub fn found(store: &Store, chat: &ChatUri) -> Option<(crate::SessionId, ChatPanel)> {
        let session = crate::higent::Hosts::session_of_chat(store, chat)?;
        let panel = Self::chat(store, &session, chat)?;
        Some((session, panel))
    }

    pub(crate) fn holds(&self, chat: &ChatUri) -> bool {
        self.chats.contains_key(chat)
    }

    pub(crate) fn uris(&self) -> Vec<ChatUri> {
        self.chats.keys().cloned().collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.chats.is_empty()
    }

    /// File a record back. The family is the PANEL's own session, so a
    /// write-back cannot land in the family a command happened to be
    /// gathered for.
    pub fn put(store: &mut Store, chat: ChatUri, panel: ChatPanel) {
        let session = panel.session_id();
        crate::higent::Hosts::update_chats(store, &session, |chats| {
            chats.chats.insert_mut(chat, panel);
        });
    }

    /// The session let go: its conversations go with it.
    pub fn forget_session(store: &mut Store, session: &crate::SessionId) {
        crate::higent::Hosts::update_chats(store, session, |chats| {
            chats.chats = rpds::HashTrieMapSync::new_sync();
        });
    }

    pub fn open(
        store: &mut Store,
        ui: &imba::UiCtx,
        server: crate::higent::HostId,
        session: SessionUri,
        chat: ChatUri,
    ) -> Box<dyn crate::DynPanelView> {
        Self::open_with(store, ui, server, session, chat, None)
    }

    pub fn open_with(
        store: &mut Store,
        ui: &imba::UiCtx,
        server: crate::higent::HostId,
        session: SessionUri,
        chat: ChatUri,
        initial_prompt: Option<String>,
    ) -> Box<dyn crate::DynPanelView> {
        let home = crate::SessionId {
            host: server,
            session: session.clone(),
        };
        let known = Self::chat_ref(store, &home, &chat).is_some();
        if !known {
            eprintln!("[higent] minting a chat record: {chat}");
            let mut panel = ChatPanel::new(store, ui, server, session.clone(), chat.clone());
            if let Some(prompt) = initial_prompt {
                panel = panel.with_initial_prompt(prompt);
            }
            Self::put(store, chat.clone(), panel);
        }
        Box::new(ChatPane::new(home, chat))
    }

    /// Every chat of one session.
    pub fn list(store: &Store, session: &crate::SessionId) -> Vec<ChatUri> {
        crate::higent::Hosts::chats_of(store, session)
            .map(Chats::uris)
            .unwrap_or_default()
    }
}

pub(crate) struct ChatLanding {
    pub(crate) session: crate::SessionId,
    pub(crate) chat: ChatUri,
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
        let Some(mut panel) = Chats::chat(store, &self.session, &self.chat) else {
            return;
        };
        let ui = app.ui_ctx();
        let ChatLanding {
            session,
            chat,
            command,
        } = *self;
        let scope_chat = chat.clone();
        let scope_session = session.clone();
        fx.scope(
            move |command: ChatPanelCommand| {
                AppCommand::InSession(
                    scope_session.clone(),
                    Box::new(AppCommand::Landing(
                        window,
                        Box::new(ChatLanding {
                            session: scope_session.clone(),
                            chat: scope_chat.clone(),
                            command,
                        }),
                    )),
                )
            },
            |fx| panel.perform_model(store, ui.as_ref(), command, fx),
        );
        Chats::put(store, chat, panel);
    }
}

pub(crate) struct EnsureChatFeed {
    pub(crate) session: crate::SessionId,
    pub(crate) chat: ChatUri,
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
        let Some(mut panel) = Chats::chat(store, &self.session, &self.chat) else {
            return;
        };
        let Some(seat) = crate::higent::Servers::seat(store, panel.server()) else {
            panel.mark_failed("unregistered server".to_owned());
            Chats::put(store, self.chat.clone(), panel);
            return;
        };
        Chats::put(store, self.chat.clone(), panel);
        eprintln!("[higent] chat feed SUBSCRIBES anew: {}", self.chat);
        let chat = self.chat.clone();
        let landing = self.chat.clone();
        let session = self.session.clone();
        let scope = self.session.clone();
        fx.push(
            AnyEffect::new(crate::higent::SubscribeChatEffect { seat, chat }).map(move |result| {
                AppCommand::InSession(
                    scope.clone(),
                    Box::new(AppCommand::Landing(
                        window,
                        Box::new(ChatLanding {
                            session: session.clone(),
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
    /// The session that OWNS this chat — not whichever session the
    /// window happens to be on when a command arrives.
    session: crate::SessionId,
    chat: ChatUri,
    /// The mount, CLAIMED on the pane's first command — the warm one a
    /// previous pane parked when it closed, else a fresh one. Minting
    /// an id here instead would re-lay the whole transcript on every
    /// walk back to the chat.
    view: Option<ChatViewId>,
}

impl ChatPane {
    /// Storeless by design (family rows mint with `&Store`): the mount
    /// is CLAIMED from the model on the pane's first command
    /// (`claim_view`), which adopts the parked one when there is one.
    pub fn new(session: crate::SessionId, chat: ChatUri) -> Self {
        Self {
            session,
            chat,
            view: None,
        }
    }

    /// Mint a pane for a chat whose session the caller does not hold.
    pub fn of_chat(store: &Store, chat: ChatUri) -> Option<Self> {
        let session = crate::higent::Hosts::session_of_chat(store, &chat)?;
        Some(Self::new(session, chat))
    }

    pub fn chat(&self) -> &ChatUri {
        &self.chat
    }

    /// The pane goes, the MOUNT stays — parked and still fed, so the
    /// walk back re-displays it instead of rebuilding every cell's
    /// document. The chat itself is session truth either way.
    fn park(&mut self, store: &mut Store) {
        let Some(view) = self.view.take() else {
            return;
        };
        let Some(mut panel) = Chats::chat(store, &self.session, &self.chat) else {
            return;
        };
        panel.park_view(view);
        Chats::put(store, self.chat.clone(), panel);
    }
}

impl imba::View for ChatPane {
    type Command = ChatPanelCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ChatPanelCommand> {
        match (self.view, Chats::chat_ref(store, &self.session, &self.chat)) {
            (Some(view), Some(panel)) => panel.focus_data_view(store, ui, view),
            _ => imba::focus::FocusData::default(),
        }
    }

    fn destroy(&mut self, store: &mut Store, _fx: &mut imba::effect::Effects<'_, Self::Command>) {
        self.park(store);
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        let Some(mut panel) = Chats::chat(store, &self.session, &self.chat) else {
            return;
        };
        let view = match self.view {
            Some(view) => view,
            None => {
                let view = panel.claim_view(store, ui, fx);
                self.view = Some(view);
                view
            }
        };
        panel.perform_in_view(store, ui, view, command, fx);
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
                let widget: imba::ThunkBox<'a, Self::Command> = match self.view.and_then(|view| {
                    Chats::chat_ref(store, &self.session, &self.chat)
                        .and_then(|panel| panel.display_view(arena, store, ui, view))
                }) {
                    Some(laid) => {
                        imba::ThunkBox::new(arena, imba::Layout::layout(laid, arena, constraints))
                    }

                    // No view record yet (panes mint storeless): a
                    // blank frame whose paint asks for Boot — the
                    // perform road builds the view and subscribes.
                    None => imba::ThunkBox::new(
                        arena,
                        imba::thunk_ext::ThunkExt::event(
                            imba::leaf::leaf::<Self::Command>(
                                constraints.max.width,
                                constraints.max.height,
                            ),
                            |_arena, event, _size| match event {
                                imba::event::Event::Paint { .. } => {
                                    imba::event::EventResult::Command(ChatPanelCommand::Boot)
                                }
                                _ => imba::event::EventResult::Ignored,
                            },
                        ),
                    ),
                };
                widget
            },
        )
    }
}

/// The chat pane's navigation identity: the chat uri. A recorded place
/// is what makes leaving a chat WALKABLE — without one, a navigation
/// away pushes nothing and back has nowhere to return.
#[derive(Clone, PartialEq)]
pub struct ChatPlace {
    pub chat: ChatUri,
}

impl crate::Place for ChatPlace {}

/// The walk-back road: re-mint the reference pane off the family row.
/// A dismantled chat has no home to walk back to.
pub struct ChatNavigator;

impl crate::Navigator for ChatNavigator {
    type Place = ChatPlace;

    fn navigate(
        &self,
        store: &mut Store,
        _ui: &UiCtx,
        _window: WindowId,
        place: &ChatPlace,
        _fx: &mut crate::AppFx<'_>,
    ) -> Option<crate::Panel> {
        crate::higent::Hosts::session_of_chat(store, &place.chat)?;
        Some(crate::Panel::Plugin(crate::family_rows::mint(
            store,
            &crate::FamilyRow::Chat(place.chat.clone()),
        )?))
    }
}

impl crate::PanelView for ChatPane {
    type Place = ChatPlace;

    fn family_row(&self) -> Option<crate::FamilyRow> {
        Some(crate::FamilyRow::Chat(self.chat.clone()))
    }

    fn navigation_location(&self, _store: &Store) -> Option<ChatPlace> {
        Some(ChatPlace {
            chat: self.chat.clone(),
        })
    }

    fn navigate_to(
        &mut self,
        _store: &mut Store,
        place: &ChatPlace,
        _fx: &mut crate::AppFx<'_>,
    ) -> bool {
        place.chat == self.chat
    }

    fn title(&self, store: &Store) -> String {
        Chats::chat_ref(store, &self.session, &self.chat)
            .map(|panel| panel.title_text())
            .unwrap_or_else(|| "Agent Chat".to_owned())
    }

    fn collapsed_height(&self, store: &Store, nominal_height: f32) -> Option<f32> {
        Chats::chat_ref(store, &self.session, &self.chat)
            .map(|panel| panel.footer_height(store, nominal_height))
    }

    // Closing the PANE must not end the CONVERSATION: the pane is a
    // reference view; the chat is session truth in `Chats`, its feed
    // keeps landing turns, and ⌘I / the widget drawer / back all
    // re-mint the pane from it. The chat dies with its session, not
    // with a workbench slot. (The remove here was the sheet era's
    // lifecycle — it made reopening impossible.)
    fn dismantle(&mut self, store: &mut Store) {
        self.park(store);
    }

    /// Walked away from, or displaced by another panel: the instance
    /// goes, the laid mount stays parked for the walk back.
    fn displaced(&mut self, store: &mut Store) {
        self.park(store);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
