// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::effect::AnyEffect;
use imba::store::Store;
use imba::UiCtx;

use crate::higent::chat::{ChatPanel, ChatPanelCommand, ChatViewId};
use crate::higent::{ChatUri, SessionUri};

/// The conversations of ONE session — a collection reached by its
/// `Id<Chats>` and nothing else (docs/entities.md). The id is wired
/// into every record and pane at the mint, carried by the family row
/// and the navigation place, and stamped on every feed landing: no
/// road consults the catalog to find a chat, so a command gathered for
/// another session cannot file a record anywhere but home.
#[derive(Clone)]
pub struct Chats {
    /// The recents the composer's `@` completion lists — wired at the
    /// family mint (docs/entities.md law 4).
    recents: imba::store::Id<crate::higent::RecentLocations>,

    chats: rpds::HashTrieMapSync<ChatUri, ChatPanel>,
}

impl Chats {
    /// A collection wired to its sibling — minted by the family
    /// ceremony, and by tests that stand one up alone.
    pub fn wired(recents: imba::store::Id<crate::higent::RecentLocations>) -> Self {
        Self {
            recents,
            chats: rpds::HashTrieMapSync::new_sync(),
        }
    }

    pub fn recents(&self) -> imba::store::Id<crate::higent::RecentLocations> {
        self.recents
    }

    pub fn chat_ref<'a>(
        store: &'a Store,
        chats: imba::store::Id<Chats>,
        chat: &ChatUri,
    ) -> Option<&'a ChatPanel> {
        store.entity(chats)?.chats.get(chat)
    }

    pub fn chat(store: &Store, chats: imba::store::Id<Chats>, chat: &ChatUri) -> Option<ChatPanel> {
        Self::chat_ref(store, chats, chat).cloned()
    }

    pub fn holds(&self, chat: &ChatUri) -> bool {
        self.chats.contains_key(chat)
    }

    pub(crate) fn uris(&self) -> Vec<ChatUri> {
        self.chats.keys().cloned().collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.chats.is_empty()
    }

    /// File a record back BY ID — the panel carries its collection
    /// (wired at mint), so a write-back cannot land in the family a
    /// command happened to be gathered for.
    pub fn put(store: &mut Store, chats: imba::store::Id<Chats>, chat: ChatUri, panel: ChatPanel) {
        let Some(mut rows) = store.entity(chats).cloned() else {
            return;
        };
        rows.chats.insert_mut(chat, panel);
        store.put_entity(chats, rows);
    }

    pub fn open(
        store: &mut Store,
        ui: &imba::UiCtx,
        chats: imba::store::Id<Chats>,
        server: crate::higent::HostId,
        session: SessionUri,
        chat: ChatUri,
    ) -> Box<dyn hikit::DynPanelView> {
        Self::open_with(store, ui, chats, server, session, chat, None)
    }

    /// Open a chat INTO its collection: the caller holds the id (the
    /// window's family at session entry); the record is minted here if
    /// the collection does not hold it yet.
    pub fn open_with(
        store: &mut Store,
        ui: &imba::UiCtx,
        chats: imba::store::Id<Chats>,
        server: crate::higent::HostId,
        session: SessionUri,
        chat: ChatUri,
        initial_prompt: Option<String>,
    ) -> Box<dyn hikit::DynPanelView> {
        let known = Self::chat_ref(store, chats, &chat).is_some();
        if !known {
            eprintln!("[higent] minting a chat record: {chat}");
            let mut panel = ChatPanel::new(store, ui, server, session.clone(), chats, chat.clone());
            if let Some(prompt) = initial_prompt {
                panel = panel.with_initial_prompt(prompt);
            }
            Self::put(store, chats, chat.clone(), panel);
        }
        Box::new(ChatPane::new(chats, chat))
    }

    /// Every chat of one collection.
    pub fn list(store: &Store, chats: imba::store::Id<Chats>) -> Vec<ChatUri> {
        store.entity(chats).map(Chats::uris).unwrap_or_default()
    }
}

/// What the chats collection answers to, behind its `At` address
/// (docs/entities.md law 5): the panel road — feed landings and model
/// performs, keyed by the collection's PRIVATE uri.
#[derive(Clone)]
pub enum ChatsCommand {
    Panel(ChatUri, ChatPanelCommand),
}

impl std::fmt::Display for ChatsCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatsCommand::Panel(_, command) => command.fmt(out),
        }
    }
}

impl imba::store::Entity for Chats {
    type Command = ChatsCommand;

    fn perform(
        &mut self,
        _id: imba::store::Id<Self>,
        command: ChatsCommand,
        store: &mut Store,
        ui: &imba::UiCtx,
        fx: &mut imba::effect::Effects<'_, ChatsCommand>,
    ) {
        match command {
            ChatsCommand::Panel(chat, command) => {
                let Some(mut panel) = self.chats.get(&chat).cloned() else {
                    return;
                };
                let scope = chat.clone();
                fx.scope(
                    move |command: ChatPanelCommand| ChatsCommand::Panel(scope.clone(), command),
                    |fx| panel.perform_model(store, ui, command, fx),
                );
                self.chats.insert_mut(chat, panel);
            }
        }
    }

    fn destroy(&mut self, _store: &mut Store) {
        // The records are the collection's PRIVATE schema — nothing to
        // retract; feed tokens die with the drop.
    }
}

/// Boot a chat's feed by id, pane or no pane: liveness never depends
/// on the chat being laid out — a hidden or not-yet-shown chat still
/// connects and streams (the window merely chooses what to paint).
pub struct BootChat {
    pub chats: imba::store::Id<Chats>,
    pub chat: ChatUri,
}

impl imba::command::DynamicCommand for BootChat {
    fn id(&self) -> &'static str {
        "agent.chat-boot"
    }
    fn name(&self) -> String {
        "Boot Chat".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::UiCtx, _fx: &mut imba::command::Fx<'_>) {
        let Some(mut panel) = Chats::chat(store, self.chats, &self.chat) else {
            return;
        };
        let boots = panel.boot_feed();
        Chats::put(store, self.chats, self.chat.clone(), panel);
        if boots {
            imba::command::Requests::push(
                store,
                std::sync::Arc::new(EnsureChatFeed {
                    chats: self.chats,
                    chat: self.chat.clone(),
                }),
            );
        }
    }
}

pub struct EnsureChatFeed {
    pub chats: imba::store::Id<Chats>,
    pub chat: ChatUri,
}

impl imba::command::DynamicCommand for EnsureChatFeed {
    fn id(&self) -> &'static str {
        "agent.chat-subscribe"
    }
    fn name(&self) -> String {
        "Subscribe Chat".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::UiCtx, fx: &mut imba::command::Fx<'_>) {
        let chats = self.chats;
        let Some(mut panel) = Chats::chat(store, chats, &self.chat) else {
            return;
        };
        let Some(client) = crate::higent::Servers::client(store, panel.server()) else {
            panel.mark_failed("unregistered server".to_owned());
            Chats::put(store, chats, self.chat.clone(), panel);
            return;
        };
        Chats::put(store, chats, self.chat.clone(), panel);
        eprintln!("[higent] chat feed SUBSCRIBES anew: {}", self.chat);
        let chat = self.chat.clone();
        let landing = self.chat.clone();
        fx.push(
            AnyEffect::new(crate::higent::SubscribeChatEffect { client: client.chat.clone(), chat }).map(move |result| {
                imba::command::Verb::at(
                    chats,
                    ChatsCommand::Panel(landing.clone(), ChatPanelCommand::Snapshot(result)),
                )
            }),
        );
    }
}

#[derive(Clone)]
pub struct ChatPane {
    /// The collection that OWNS this chat — wired at mint, not
    /// whichever session the window happens to be on when a command
    /// arrives.
    chats: imba::store::Id<Chats>,
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
    pub fn new(chats: imba::store::Id<Chats>, chat: ChatUri) -> Self {
        Self {
            chats,
            chat,
            view: None,
        }
    }

    pub fn chat(&self) -> &ChatUri {
        &self.chat
    }

    /// The collection this pane addresses.
    pub fn chats(&self) -> imba::store::Id<Chats> {
        self.chats
    }

    /// The pane goes but the MOUNT stays — parked and still fed, so the
    /// walk back re-displays it instead of rebuilding every cell's
    /// document. The chat itself is session truth either way.
    fn park(&mut self, store: &mut Store) {
        let Some(view) = self.view.take() else {
            return;
        };
        let Some(mut panel) = Chats::chat(store, self.chats, &self.chat) else {
            return;
        };
        panel.park_view(view);
        Chats::put(store, self.chats, self.chat.clone(), panel);
    }
}

impl imba::View for ChatPane {
    type Command = ChatPanelCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ChatPanelCommand> {
        match (self.view, Chats::chat_ref(store, self.chats, &self.chat)) {
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
        let Some(mut panel) = Chats::chat(store, self.chats, &self.chat) else {
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
        Chats::put(store, self.chats, self.chat.clone(), panel);
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
                    Chats::chat_ref(store, self.chats, &self.chat)
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

/// The chat pane's navigation identity: the collection and the chat.
/// A recorded place is what makes leaving a chat WALKABLE — without
/// one, a navigation away pushes nothing and back has nowhere to
/// return.
#[derive(Clone, PartialEq)]
pub struct ChatPlace {
    pub chats: imba::store::Id<Chats>,
    pub chat: ChatUri,
}

impl hikit::Place for ChatPlace {}

/// The walk-back road: re-mint the reference pane off the family row.
/// A dismantled chat has no home to walk back to.
pub struct ChatNavigator;

impl hikit::Navigator for ChatNavigator {
    type Place = ChatPlace;

    fn navigate(
        &self,
        store: &mut Store,
        _ui: &UiCtx,
        place: &ChatPlace,
        _fx: &mut imba::command::Fx<'_>,
    ) -> Option<Box<dyn hikit::DynPanelView>> {
        // The ChatRow arm of the shell's row mint, inlined: the pane
        // is minted off the id; a dismantled chat has no home.
        store
            .entity(place.chats)
            .filter(|rows| rows.holds(&place.chat))
            .map(|_| {
                Box::new(ChatPane::new(place.chats, place.chat.clone()))
                    as Box<dyn hikit::DynPanelView>
            })
    }
}

impl hikit::PanelView for ChatPane {
    type Place = ChatPlace;

    fn family_row(&self) -> Option<hikit::FamilyRow> {
        Some(hikit::FamilyRow::new(crate::higent::ChatRow(
            self.chats,
            self.chat.clone(),
        )))
    }

    fn navigation_location(&self, _store: &Store) -> Option<ChatPlace> {
        Some(ChatPlace {
            chats: self.chats,
            chat: self.chat.clone(),
        })
    }

    fn navigate_to(
        &mut self,
        _store: &mut Store,
        place: &ChatPlace,
        _fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        place.chats == self.chats && place.chat == self.chat
    }

    fn title(&self, store: &Store) -> String {
        Chats::chat_ref(store, self.chats, &self.chat)
            .map(|panel| panel.title_text())
            .unwrap_or_else(|| "Agent Chat".to_owned())
    }

    fn collapsed_height(&self, store: &Store, nominal_height: f32) -> Option<f32> {
        Chats::chat_ref(store, self.chats, &self.chat)
            .map(|panel| panel.footer_height(store, nominal_height))
    }

    // Closing the PANE must not end the CONVERSATION: the pane is a
    // reference view; the chat is session truth in `Chats`, its feed
    // keeps landing turns, and ⌘I / the widget drawer / back all
    // re-mint the pane from it. The chat dies with its session, not
    // with a workbench slot. (The remove here was the sheet era's
    // lifecycle — it made reopening impossible.)
    //
    // The MOUNT does end here: closed is closed, and a mount nobody
    // shows must not keep laying every part that streams in.
    fn dismantle(&mut self, store: &mut Store) {
        let Some(view) = self.view.take() else {
            return;
        };
        let Some(mut panel) = Chats::chat(store, self.chats, &self.chat) else {
            return;
        };
        panel.close_view(store, view);
        Chats::put(store, self.chats, self.chat.clone(), panel);
    }

    /// Walked away from, or displaced by another panel — not closed:
    /// the instance goes, the laid mount stays parked for the walk back.
    fn displaced(&mut self, store: &mut Store) {
        self.park(store);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, PartialEq)]
pub struct ChatRow(
    pub imba::store::Id<crate::higent::Chats>,
    pub crate::higent::ChatUri,
);

impl hikit::Row for ChatRow {}
