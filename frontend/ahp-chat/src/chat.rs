// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The chat, split Document/Editor-style (docs/model-view.md step 5):
//! `ChatPanel` is the conversation MODEL — the transcript as cell
//! SPECS, the stream fold, the permission ask, the feed — and it owns
//! its `ChatView` records, one per mount. A view is the laid
//! furniture: the ListView of measured cells (heights are width-bound
//! — the reason a single view could never mount twice), the scroll,
//! the composer with its per-view draft, focus, completion, toolbar.
//! A model mutation rolls every view in the same batch (`roll`), each
//! re-laying at its own width; a fresh view is built from the spec
//! transcript alone.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ahp_types::actions::{
    ChatPendingMessageRemovedAction, ChatToolCallConfirmedAction, StateAction,
};
use ahp_types::state::{
    ChatState, ConfirmationOption, ConfirmationOptionKind, PendingMessageKind,
    ToolCallConfirmationReason, ToolInput,
};
use ahp_wire::client::TurnsPage;
use ahp_wire::effects::{
    CancelTurnEffect, DispatchChatActionEffect, FetchTurnsEffect, PollChatActionsEffect,
};
use editor::env;
use hikit::fonts::ui_text_font;
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::{AnyEffect, CancellationToken, Effects},
    event::{Event, EventResult, Key},
    layout::Layout as _,
    layout::LayoutExt as _,
    leaf::leaf,
    list::{ListCommand, ListSlice, ListView},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View, Widget,
};
use skia_safe::{Paint, Rect, Size};

use crate::cell::{Cell, CellCommand};
use crate::composer::{Composer, ComposerCommand, ComposerProps};
use crate::stack::{PermissionAsk, StackCommand, WidgetStack};
use crate::turn::{CellSpec, TurnCommand, TurnView};

#[derive(Clone)]
pub enum RowCommand {
    Activate,

    /// A SLEEPING row painted: build me at the row's real width.
    Wake,

    /// A cell joins the turn where its key belongs.
    Place {
        key: crate::turn::CellKey,
        cell: Cell,
        height: f32,
    },
    Turn(TurnCommand),
}

impl std::fmt::Display for RowCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RowCommand::Turn(command) => command.fmt(out),
            RowCommand::Activate => out.write_str("row activate"),
            RowCommand::Wake => out.write_str("row wake"),
            RowCommand::Place { .. } => out.write_str("row place"),
        }
    }
}

#[derive(Clone)]
enum ChatRow {
    Loader {
        armed: bool,
    },

    /// An off-viewport turn holds the MODEL turn, not its editors —
    /// opening a chat is O(visible), not O(history). The first paint
    /// wakes it (the loader's arm pattern); the height correction
    /// rides the settle pulse, above the anchor.
    Sleeping(model::Turn),

    Turn(TurnView),
}

impl View for ChatRow {
    type Command = RowCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, RowCommand> {
        match self {
            ChatRow::Turn(turn) => turn.focus_data(store, ui).map(RowCommand::Turn),
            ChatRow::Loader { .. } | ChatRow::Sleeping(_) => imba::focus::FocusData::default(),
        }
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        if let ChatRow::Turn(turn) = self {
            fx.scope(RowCommand::Turn, |fx| turn.destroy(store, fx));
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match (self, command) {
            (ChatRow::Turn(turn), RowCommand::Turn(command)) => {
                fx.scope(RowCommand::Turn, |fx| turn.perform(store, ui, command, fx))
            }
            (ChatRow::Turn(turn), RowCommand::Place { key, cell, height }) => {
                turn.place(key, cell, height);
            }

            _ => {}
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(
            move |_arena: &'a Arena, constraints: Constraints| match self {
                ChatRow::Loader { armed } => {
                    let chrome = env::Themes::of(store).ui().chat.clone();
                    let width = constraints.max.width.max(1.0);
                    let text = if *armed {
                        "· · ·  older turns above  · · ·"
                    } else {
                        "loading older turns…"
                    };
                    let font = ui_text_font(ui, chrome.title_size * 0.8);
                    let color = chrome.loader_color.0;
                    let height = chrome.loader_height;
                    let band = leaf::<RowCommand>(width, height).paint_instead(
                        move |_arena, canvas, rect| {
                            let mut paint = Paint::default();
                            paint.set_anti_alias(true);
                            paint.set_color(color);
                            let label_width = font.measure_str(text, None).0;
                            canvas.draw_str(
                                text,
                                (
                                    rect.left + (rect.width() - label_width) / 2.0,
                                    rect.top + rect.height() * 0.6,
                                ),
                                &font,
                                &paint,
                            );
                        },
                    );
                    let mut row = container(arena, Size::new(width, height));
                    row.place(0.0, 0.0, band);
                    let armed = *armed;
                    Either::Loader(row.wrap(move |inner| ArmedLoader { inner, armed }))
                }
                ChatRow::Sleeping(_) => {
                    let width = constraints.max.width.max(1.0);
                    let height = constraints.max.height.max(1.0);
                    let band = imba::leaf::leaf::<RowCommand>(width, height).event(
                        |_arena, event, _size| match event {
                            Event::Paint { .. } => EventResult::Command(RowCommand::Wake),
                            _ => EventResult::Ignored,
                        },
                    );
                    let mut row = container(arena, Size::new(width, height));
                    row.place(0.0, 0.0, band);
                    Either::Sleeping(row.wrap(move |inner| ArmedLoader {
                        inner,
                        armed: false,
                    }))
                }
                ChatRow::Turn(turn) => Either::Turn(
                    imba::layout::Layout::layout(
                        turn.display(arena, store, ui),
                        arena,
                        constraints,
                    )
                    .map(RowCommand::Turn),
                ),
            },
        )
    }
}

struct ArmedLoader<Inner> {
    inner: Inner,
    armed: bool,
}

impl<'a, Inner: Widget<'a, RowCommand>> Widget<'a, RowCommand> for ArmedLoader<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, RowCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<RowCommand> {
        let result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) && self.armed {
            return result.merge(EventResult::Command(RowCommand::Activate));
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, RowCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

enum Either<A, B, C> {
    Loader(A),
    Turn(B),
    Sleeping(C),
}

impl<'a, A, B, C> imba::Thunk<'a, RowCommand> for Either<A, B, C>
where
    A: imba::Thunk<'a, RowCommand> + 'a,
    B: imba::Thunk<'a, RowCommand> + 'a,
    C: imba::Thunk<'a, RowCommand> + 'a,
{
    fn size(&self) -> Size {
        match self {
            Either::Loader(thunk) => thunk.size(),
            Either::Turn(thunk) => thunk.size(),
            Either::Sleeping(thunk) => thunk.size(),
        }
    }

    fn realize(
        self,
        arena: &'a imba::arena::Arena,
        viewport: Rect,
    ) -> imba::WidgetBox<'a, RowCommand> {
        match self {
            Either::Loader(thunk) => thunk.realize(arena, viewport),
            Either::Turn(thunk) => thunk.realize(arena, viewport),
            Either::Sleeping(thunk) => thunk.realize(arena, viewport),
        }
    }
}

impl<'a, A, B, C> Widget<'a, RowCommand> for Either<A, B, C>
where
    A: Widget<'a, RowCommand>,
    B: Widget<'a, RowCommand>,
    C: Widget<'a, RowCommand>,
{
    fn size(&self) -> Size {
        match self {
            Either::Loader(widget) => widget.size(),
            Either::Turn(widget) => widget.size(),
            Either::Sleeping(widget) => widget.size(),
        }
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, RowCommand>> {
        match self {
            Either::Loader(widget) => widget.overlays(),
            Either::Turn(widget) => widget.overlays(),
            Either::Sleeping(widget) => widget.overlays(),
        }
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<RowCommand> {
        match self {
            Either::Loader(widget) => widget.handle_event(arena, event, viewport),
            Either::Turn(widget) => widget.handle_event(arena, event, viewport),
            Either::Sleeping(widget) => widget.handle_event(arena, event, viewport),
        }
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, RowCommand>
    where
        'a: 'w,
    {
        match self {
            Either::Loader(widget) => widget.layout_data(target),
            Either::Turn(widget) => widget.layout_data(target),
            Either::Sleeping(widget) => widget.layout_data(target),
        }
    }
}

#[derive(Clone)]
enum Link {
    Idle,
    Subscribing,
    Ready,
    Failed(String),
}

type Transcript = ListView<ChatRow, ahp_wire::client::TurnId>;
type Rows = ScrollView<Transcript>;
type RowsCommand = ScrollCommand<ListCommand<RowCommand>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChatArea {
    Transcript,
    Composer,
}

#[derive(Clone)]
pub enum ChatPanelCommand {
    /// A command addressed to ONE view record — how an effect armed
    /// while rolling a view finds its way back to that view when it
    /// lands chat-scoped (no pane in sight).
    InView(ChatViewId, Box<ChatPanelCommand>),

    Rows(RowsCommand),

    Focus(ChatArea, Option<Box<ChatPanelCommand>>),

    /// A command for one cell, addressed by KEY: a landing that was in
    /// flight while the row changed shape still finds its cell.
    Cell {
        turn: ahp_wire::client::TurnId,
        cell: crate::turn::CellKey,
        command: CellCommand,
    },
    Composer(ComposerCommand),
    Stack(StackCommand),

    Boot,
    Snapshot(Result<ChatState, String>),
    Older(Result<TurnsPage, String>),

    /// The host took the turn we dispatched. Nothing to do — the row
    /// has been on screen since we folded our own action.
    Sent,
    /// The host refused it: the turn we minted failed.
    SendFailed {
        turn: ahp_wire::client::TurnId,
        error: String,
    },

    Actions(Vec<StateAction>),
    Send,

    Answer(usize),

    Dispatched {
        undo_queue: Option<String>,
        result: Result<(), String>,
    },

    Toolbar(super::session_toolbar::ToolbarCommand),

    Blurred(bool),

    ToolbarSync,

    CompletionFound(crate::completion::CompletionFound),
}

impl std::fmt::Display for ChatPanelCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatPanelCommand::InView(_, command) => command.fmt(out),
            ChatPanelCommand::Rows(command) => command.fmt(out),
            ChatPanelCommand::Focus(_, Some(command)) => command.fmt(out),
            ChatPanelCommand::Focus(_, None) => out.write_str("chat focus"),
            ChatPanelCommand::Cell { command, .. } => command.fmt(out),
            ChatPanelCommand::Composer(command) => command.fmt(out),
            ChatPanelCommand::Stack(command) => command.fmt(out),
            ChatPanelCommand::Toolbar(command) => command.fmt(out),
            ChatPanelCommand::Boot => out.write_str("chat boot"),
            ChatPanelCommand::Snapshot(_) => out.write_str("chat snapshot"),
            ChatPanelCommand::Older(_) => out.write_str("chat older"),
            ChatPanelCommand::Sent => out.write_str("chat sent"),
            ChatPanelCommand::SendFailed { .. } => out.write_str("chat send failed"),
            ChatPanelCommand::Actions(_) => out.write_str("chat actions"),
            ChatPanelCommand::Send => out.write_str("chat send"),
            ChatPanelCommand::Answer(_) => out.write_str("chat answer"),
            ChatPanelCommand::Dispatched { .. } => out.write_str("chat dispatched"),
            ChatPanelCommand::Blurred(_) => out.write_str("chat blurred"),
            ChatPanelCommand::ToolbarSync => out.write_str("toolbar sync"),
            ChatPanelCommand::CompletionFound(_) => out.write_str("completion found"),
        }
    }
}

impl ChatPanelCommand {}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChatViewId(u64);

impl ChatViewId {
    pub(crate) fn mint() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// One model mutation, as every view consumes it — each view lays
/// the same op at its own width.
enum ViewOp {
    /// The whole conversation re-lays: a subscribe snapshot landed.
    Reset {
        turns: rpds::VectorSync<model::Turn>,
        has_more: bool,
    },
    /// A page of older turns joined the top.
    Prepend {
        turns: rpds::VectorSync<model::Turn>,
        has_more: bool,
    },
    Loader {
        armed: bool,
    },
    /// One turn re-lays whole: a turn we did not hold joins the tail, a
    /// turn we did is replaced where it stands (a replayed start RESETS
    /// its parts, so the old cells must go). The op carries the MODEL
    /// turn — every view dresses it at its own width.
    Row(model::Turn),
    /// ONE cell of a turn moved: placed if the row lacks it, re-dressed
    /// in place if it stands. Every other cell keeps the document it
    /// already has — a cell is a document, a parse and a layout, and a
    /// streamed turn must never mint them again. The op carries the
    /// SPEC, dressed once; the views only lay it.
    Cell {
        turn: ahp_wire::client::TurnId,
        key: crate::turn::CellKey,
        spec: CellSpec,
    },
    /// A turn ended: the cells after its parts (how it ended, what it
    /// spent) join the row. At most two.
    Tail {
        turn: ahp_wire::client::TurnId,
        cells: crate::turn::DressedCells,
    },
    /// Streamed text joined one part. The view hops from the part id to
    /// its cell and appends — no counting, no re-lay.
    Grew {
        turn: ahp_wire::client::TurnId,
        part: model::PartId,
        text: String,
    },
}

/// The conversation MODEL: session truth, owner of its views.
pub struct ChatPanel {
    server: ahp_wire::client::HostId,

    session: ahp_wire::client::SessionUri,
    /// The collection this chat files into — wired at mint
    /// (docs/entities.md law 4).
    chats: imba::store::Id<crate::chats::Chats>,
    chat: ahp_wire::client::ChatUri,
    /// The ceremony-wired catalog roads, stamped at mint — the panel
    /// runs under its collection's lease, so it must never read the
    /// collection back to find them.
    catalog: crate::chats::Catalog,
    state: Link,
    title: String,

    /// THE conversation: turns by id, the turn in flight, the cursor
    /// behind the window. Every wire action folds into this and
    /// nothing else — see `model`.
    conversation: model::Conversation,

    stack: WidgetStack,

    fetch_token: Option<CancellationToken>,

    poll_token: Option<CancellationToken>,

    /// A STEERED prompt: sent while a turn ran, so the run was
    /// cancelled and this fires the moment the turn ends — the
    /// Claude Code Esc-with-prompt shape. An explicit STOP drops it.
    steering: Option<String>,

    initial_prompt: Option<String>,

    minted: u64,

    views: rpds::HashTrieMapSync<ChatViewId, ChatView>,

    /// The mounts panes LEFT BEHIND, warm: each stays in `views` and
    /// keeps taking ops, so walking back to the chat lays nothing. The
    /// next pane claims the newest (`claim_view`) and dismantles the
    /// rest. Opens and closes alternate, so this holds at most one
    /// mount per window that ever showed the chat.
    parked: rpds::VectorSync<ChatViewId>,
}

impl Clone for ChatPanel {
    fn clone(&self) -> Self {
        Self {
            server: self.server,
            session: self.session.clone(),
            chats: self.chats,
            chat: self.chat.clone(),
            catalog: self.catalog.clone(),
            state: self.state.clone(),
            title: self.title.clone(),
            conversation: self.conversation.clone(),
            stack: self.stack.clone(),
            fetch_token: self.fetch_token,
            poll_token: self.poll_token,
            steering: self.steering.clone(),
            initial_prompt: self.initial_prompt.clone(),
            minted: self.minted,
            views: self.views.clone(),
            parked: self.parked.clone(),
        }
    }
}

/// One MOUNT's furniture: the laid list (heights are width-bound),
/// the scroll, the composer with its per-view draft, focus,
/// completion, toolbar.
pub struct ChatView {
    rows: Rows,

    composer: Composer,

    completion: crate::completion::Completion,

    picked: rpds::VectorSync<crate::completion::PickedFile>,

    focus: ChatArea,

    has_loader: bool,

    toolbar: super::session_toolbar::SessionToolbar,

    panel_width: AtomicU32,

    rows_height: AtomicU32,
}

impl Clone for ChatView {
    fn clone(&self) -> Self {
        Self {
            rows: self.rows.clone(),
            composer: self.composer.clone(),
            completion: self.completion.clone(),
            picked: self.picked.clone(),
            focus: self.focus,
            has_loader: self.has_loader,
            toolbar: self.toolbar.clone(),
            panel_width: AtomicU32::new(self.panel_width.load(Ordering::Relaxed)),
            rows_height: AtomicU32::new(self.rows_height.load(Ordering::Relaxed)),
        }
    }
}

/// How many tail turns build eagerly on open — roughly the reachable
/// viewport; everything older sleeps until painted.
const EAGER_TAIL: usize = 12;

/// A sleeping row's reserved band — a line-count estimate off the
/// SPECS. Wrong is fine: the wake's correction lands above the
/// anchor and rides the settle pulse.
fn sleeping_height(turn: &model::Turn, chrome: &editor::theme::ChatChrome) -> f32 {
    let line = chrome.title_size * 1.5;
    let body: f32 = crate::turn::dress(turn)
        .iter()
        .map(|(_, cell)| match cell {
            CellSpec::Text(_, text) => {
                (text.lines().count().clamp(1, 40) as f32) * line + chrome.gap
            }
            CellSpec::Tools(specs) => (specs.len().max(1) as f32) * line + chrome.gap,
            CellSpec::Diff(_) => line * 3.0 + chrome.gap,
        })
        .sum();
    body.max(line) + chrome.gap
}

fn completion_editor(command: ::editor::editor_view::EditorCommand) -> ChatPanelCommand {
    ChatPanelCommand::Composer(ComposerCommand::Editor(
        imba::scroll::ScrollCommand::Content(command),
    ))
}

impl ChatPanel {
    pub fn new(
        _store: &imba::store::Store,
        _ui: &UiCtx,
        server: ahp_wire::client::HostId,
        session: impl Into<ahp_wire::client::SessionUri>,
        chats: imba::store::Id<crate::chats::Chats>,
        chat: impl Into<ahp_wire::client::ChatUri>,
        catalog: crate::chats::Catalog,
    ) -> Self {
        Self {
            server,
            session: session.into(),
            chats,
            chat: chat.into(),
            catalog,
            state: Link::Idle,
            title: "Agent Chat".to_owned(),
            conversation: model::Conversation::default(),
            stack: WidgetStack::new(),
            fetch_token: None,
            poll_token: None,
            steering: None,
            initial_prompt: None,
            minted: 0,
            views: rpds::HashTrieMapSync::new_sync(),
            parked: rpds::VectorSync::new_sync(),
        }
    }

    pub(crate) fn with_initial_prompt(mut self, prompt: String) -> Self {
        self.initial_prompt = Some(prompt);
        self
    }

    pub(crate) fn server(&self) -> ahp_wire::client::HostId {
        self.server
    }

    pub fn session_id(&self) -> ahp_wire::SessionId {
        ahp_wire::SessionId {
            host: self.server,
            session: self.session.clone(),
        }
    }

    pub(crate) fn mark_failed(&mut self, error: String) {
        self.state = Link::Failed(error);
    }

    pub(crate) fn title_text(&self) -> String {
        self.title.clone()
    }

    fn first_view(&self) -> Option<&ChatView> {
        self.views.values().next()
    }

    /// TEST SUPPORT: every view's transcript — the multi-mount
    /// oracle (a model mutation must reach them all identically).
    #[doc(hidden)]
    pub fn view_transcripts(&self) -> Vec<Vec<(String, Vec<(String, String)>)>> {
        self.views
            .values()
            .map(|view| Self::rows_oracle(view))
            .collect()
    }

    fn rows_oracle(view: &ChatView) -> Vec<(String, Vec<(String, String)>)> {
        view.rows
            .content()
            .rows()
            .map(|row| match row {
                ChatRow::Loader { .. } => ("…".to_owned(), Vec::new()),
                ChatRow::Sleeping(turn) => (
                    turn.id.as_str().to_owned(),
                    crate::turn::dress(&turn)
                        .iter()
                        .map(|(_, cell)| match cell {
                            CellSpec::Text(kind, text) => (format!("{kind:?}"), text.clone()),
                            CellSpec::Tools(specs) => (
                                "Tool".to_owned(),
                                specs
                                    .iter()
                                    .map(|spec| spec.face.line.clone())
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                            ),
                            CellSpec::Diff(spec) => ("Diff".to_owned(), spec.header.title.clone()),
                        })
                        .collect(),
                ),
                ChatRow::Turn(turn) => (turn.id().to_owned(), turn.cells_oracle()),
            })
            .collect()
    }

    pub fn transcript(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.first_view().map(Self::rows_oracle).unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn completion_open(&self) -> bool {
        self.first_view().is_some_and(|view| view.completion.open())
    }

    #[doc(hidden)]
    pub fn completion_rows(&self) -> Vec<String> {
        self.first_view()
            .map(|view| view.completion.row_labels())
            .unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn completion_picked(&self) -> Vec<String> {
        self.first_view()
            .map(|view| view.picked.iter().map(|pick| pick.rel.clone()).collect())
            .unwrap_or_default()
    }

    pub fn composer_text(&self) -> String {
        self.first_view()
            .map(|view| view.composer.text())
            .unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn blurred(&self) -> bool {
        self.first_view()
            .is_some_and(|view| view.composer.blurred())
    }

    pub(crate) fn footer_height(&self, store: &Store, nominal_height: f32) -> f32 {
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let band = self
            .first_view()
            .map(|view| view.composer.band_height(&chrome, nominal_height))
            .unwrap_or(0.0);
        band + self.stack.height(&chrome) + theme.ui().toolbar.height
    }

    /// Idle -> Subscribing, exactly once: the double-subscription
    /// guard every boot road shares (paint, install, walk-back).
    pub(crate) fn boot_feed(&mut self) -> bool {
        match self.state {
            Link::Idle => {
                self.state = Link::Subscribing;
                true
            }
            _ => false,
        }
    }

    pub fn ready(&self) -> bool {
        matches!(self.state, Link::Ready)
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn permission_oracle(&self) -> Option<(String, String, Option<String>, Vec<String>)> {
        self.stack.permission_oracle()
    }

    #[cfg(test)]
    pub(crate) fn queue_oracle(&self) -> Vec<(String, String)> {
        self.stack.queue_oracle()
    }

    #[doc(hidden)]
    pub fn toolbar_probe(&self) -> super::session_toolbar::ToolbarProbe {
        self.first_view()
            .expect("a mounted chat view")
            .toolbar
            .probe()
    }

    fn busy(&self) -> bool {
        self.conversation.is_running()
    }

    fn client(&self, store: &Store) -> Option<ahp_wire::client::Client> {
        ahp_wire::client::Servers::client(store, self.server)
    }

    // ------------------------------------------------------------------
    // The view registry: the model owns its views, panes hold ids.

    /// Build the view for `id` from the spec transcript — the road a
    /// pane takes on its first command (a pane is minted storeless,
    /// so the record arrives lazily).
    pub(crate) fn ensure_view(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        id: ChatViewId,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        if self.views.contains_key(&id) {
            return;
        }
        eprintln!(
            "[higent] view rebuild for {}: {} turns",
            self.chat,
            self.conversation.len()
        );
        let client = self.client(store);
        let mut view = ChatView::new(store, ui);
        fx.scope(
            move |command| ChatPanelCommand::InView(id, Box::new(command)),
            |fx| {
                let lead_loader = self.conversation.older().is_some();
                let slice = view.build_page(
                    store,
                    ui,
                    &client,
                    self.conversation.turns(),
                    lead_loader,
                    EAGER_TAIL,
                    fx,
                );
                let len = view.rows.content().len();
                view.rows.content_mut().splice_slice(0..len, slice);
                view.has_loader = lead_loader;
            },
        );
        view.reveal_tail(store);
        self.views.insert_mut(id, view);
    }

    /// The mount a fresh pane takes: the one the last pane PARKED
    /// (laid, and fed every op since — nothing to rebuild), else a new
    /// one, built here. Any other parked mount is dismantled now: this
    /// is the road that has effects to spend, and a pane that comes
    /// back adopts whichever mount is warm.
    pub(crate) fn claim_view(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> ChatViewId {
        let parked = std::mem::replace(&mut self.parked, rpds::VectorSync::new_sync());
        let claimed = parked.last().copied();
        for stale in parked.iter().copied().filter(|id| Some(*id) != claimed) {
            self.destroy_view(store, stale, fx);
        }
        match claimed {
            Some(id) => id,
            None => {
                let id = ChatViewId::mint();
                self.ensure_view(store, ui, id, fx);
                id
            }
        }
    }

    /// A pane let go of its mount: it stays in `views`, warm and fed,
    /// for the walk back. Teardown carries no effects (`dismantle` and
    /// `displaced` have the store and nothing else), so parking only
    /// MARKS; the next claim dismantles what it does not adopt.
    pub(crate) fn park_view(&mut self, id: ChatViewId) {
        if self.views.contains_key(&id) && !self.parked.iter().any(|parked| *parked == id) {
            self.parked.push_back_mut(id);
        }
    }

    /// A pane CLOSED: its mount goes now. Teardown carries no effects,
    /// and the mount's own are cancellations of builds in flight —
    /// those land on an id no view holds and are dropped there.
    pub(crate) fn close_view(&mut self, store: &mut Store, id: ChatViewId) {
        let mut throwaway = imba::effect::Batch::new();
        self.destroy_view(store, id, &mut throwaway.effects());
    }

    pub(crate) fn destroy_view(
        &mut self,
        store: &mut Store,
        id: ChatViewId,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let Some(mut view) = self.views.get(&id).cloned() else {
            return;
        };
        self.views.remove_mut(&id);
        if self.parked.iter().any(|parked| *parked == id) {
            self.parked = self
                .parked
                .iter()
                .copied()
                .filter(|parked| *parked != id)
                .collect();
        }
        fx.scope(ChatPanelCommand::Rows, |fx| view.rows.destroy(store, fx));
        fx.scope(ChatPanelCommand::Composer, |fx| {
            view.composer.destroy(store, fx)
        });
    }

    /// THE UPDATE RULE: a model mutation reaches every view in the
    /// same batch. Each view keeps its own reading position — only a
    /// view already at the tail follows it.
    fn roll(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        ops: &[ViewOp],
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        if ops.is_empty() {
            return;
        }
        let client = self.client(store);
        let ids: Vec<ChatViewId> = self.views.keys().copied().collect();
        for id in ids {
            let Some(mut view) = self.views.get(&id).cloned() else {
                continue;
            };
            let follow = view.near_tail();
            fx.scope(
                move |command| ChatPanelCommand::InView(id, Box::new(command)),
                |fx| {
                    for op in ops {
                        view.apply(store, ui, &client, op, fx);
                    }
                },
            );
            if follow {
                view.reveal_tail(store);
            }
            self.views.insert_mut(id, view);
        }
    }

    // ------------------------------------------------------------------
    // The model fold: wire state in, spec transcript + view ops out.

    fn load_older(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        if self.fetch_token.is_some() {
            return;
        }
        let Some(cursor) = self.conversation.older().cloned() else {
            return;
        };
        let Some(client) = self.client(store) else {
            return;
        };
        self.roll(store, ui, &[ViewOp::Loader { armed: false }], fx);
        self.fetch_token = Some(
            fx.push(
                AnyEffect::new(FetchTurnsEffect {
                    client: client.chat.clone(),
                    chat: self.chat.clone(),
                    cursor: Some(cursor),
                })
                .map(ChatPanelCommand::Older),
            ),
        );
    }

    /// The page behind the cursor. The model puts those turns before
    /// everything it holds; a turn it already holds keeps its place.
    fn apply_older(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        result: Result<TurnsPage, String>,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        self.fetch_token = None;
        match result {
            Ok(page) => {
                let before: rpds::HashTrieMapSync<ahp_wire::client::TurnId, ()> = self
                    .conversation
                    .turns()
                    .map(|turn| (turn.id.clone(), ()))
                    .collect();
                let (next, _) = self.conversation.prepended(&page.turns, page.next_cursor);
                self.conversation = next;
                // Only the turns that were NOT already held join the top.
                let landed: rpds::VectorSync<model::Turn> = self
                    .conversation
                    .turns()
                    .filter(|turn| !before.contains_key(&turn.id))
                    .cloned()
                    .collect();
                let has_more = self.conversation.older().is_some();
                self.roll(
                    store,
                    ui,
                    &[ViewOp::Prepend {
                        turns: landed,
                        has_more,
                    }],
                    fx,
                );
                // The prepend landed above the viewport: the settle
                // pulse re-aims the scroll at the anchored row before
                // this frame paints (docs/editor/viewport-preservation.md).
                fx.settle();
            }
            Err(error) => {
                eprintln!("[higent] fetchTurns failed: {error}");
                self.roll(store, ui, &[ViewOp::Loader { armed: true }], fx);
            }
        }
    }

    /// The subscribe snapshot. The model merges it: the wire window is
    /// the host's tail, the turn in flight is adopted, and history we
    /// hold beyond the window is ours to keep.
    fn apply_snapshot(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        result: Result<ChatState, String>,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let state = match result {
            Ok(state) => state,
            Err(error) => {
                self.state = Link::Failed(error);
                return;
            }
        };
        eprintln!(
            "[higent] chat snapshot for {}: {} wire turns over {} held",
            self.chat,
            state.turns.len(),
            self.conversation.len()
        );
        self.state = Link::Ready;
        self.title = state.title.clone();
        if let Some(turn) = state
            .turns
            .iter()
            .rev()
            .find(|turn| turn.state == ahp_types::state::TurnState::Complete)
        {
            (self.catalog.note_turn)(store, self.server, &self.chat, &turn.id);
        }
        self.stack
            .seed_queue(state.queued_messages.iter().flatten().cloned());

        let (next, _) = self.conversation.landed(&state);
        self.conversation = next;
        self.roll(
            store,
            ui,
            &[ViewOp::Reset {
                turns: self.conversation.turns().cloned().collect(),
                has_more: self.conversation.older().is_some(),
            }],
            fx,
        );
        self.mark_read(store, fx);
        self.relaunch_poll(store, fx);
        if let Some(prompt) = self.initial_prompt.take() {
            let _ = self.send_text(store, ui, prompt, None, None, fx);
        }
    }

    fn model_dispatch(
        &self,
        store: &Store,
        action: StateAction,
        undo_queue: Option<String>,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let Some(client) = self.client(store) else {
            return;
        };
        fx.push(
            AnyEffect::new(DispatchChatActionEffect {
                client: client.session.clone(),
                channel: self.chat.as_channel(),
                action,
            })
            .map(move |result| ChatPanelCommand::Dispatched {
                undo_queue: undo_queue.clone(),
                result,
            }),
        );
    }

    /// The panel is showing the session's latest state — tell the server so
    /// the session list drops its unread mark.
    fn mark_read(&self, store: &Store, fx: &mut Effects<'_, ChatPanelCommand>) {
        let Some(client) = self.client(store) else {
            return;
        };
        fx.push(
            AnyEffect::new(DispatchChatActionEffect {
                client: client.session.clone(),
                channel: self.session.as_channel(),
                action: StateAction::SessionIsReadChanged(
                    ahp_types::actions::SessionIsReadChangedAction { is_read: true },
                ),
            })
            .map(|result| ChatPanelCommand::Dispatched {
                undo_queue: None,
                result,
            }),
        );
    }

    fn answer(&mut self, store: &Store, index: usize, fx: &mut Effects<'_, ChatPanelCommand>) {
        let Some((turn, tool, option)) = self.stack.answer_payload(index) else {
            return;
        };
        let approved = matches!(option.kind, ConfirmationOptionKind::Approve);
        self.model_dispatch(
            store,
            StateAction::ChatToolCallConfirmed(ChatToolCallConfirmedAction {
                turn_id: turn,
                tool_call_id: tool,
                meta: None,
                approved,
                confirmed: approved.then_some(ToolCallConfirmationReason::UserAction),
                reason: None,
                edited_tool_input: None,
                user_suggestion: None,
                reason_message: None,
                selected_option_id: Some(option.id.clone()),
            }),
            None,
            fx,
        );
        self.stack.clear_ask();
    }

    /// The turn STOP cancels: the stream we fold, else the turn
    /// holding a permission ask (it outlives the stream), else the
    /// turn the wire last told us was in flight. With none of these
    /// the button has nothing to name — and does nothing.
    #[doc(hidden)]
    pub fn cancel_target(&self) -> Option<ahp_wire::client::TurnId> {
        self.conversation
            .live()
            .cloned()
            .or_else(|| self.stack.ask_turn())
    }

    fn stop(&mut self, store: &Store, fx: &mut Effects<'_, ChatPanelCommand>) {
        let Some(turn_id) = self.cancel_target() else {
            return;
        };
        let Some(client) = self.client(store) else {
            return;
        };
        fx.notify(CancelTurnEffect {
            client: client.chat.clone(),
            chat: self.chat.clone(),
            turn_id,
        });
    }

    fn relaunch_poll(&mut self, store: &Store, fx: &mut Effects<'_, ChatPanelCommand>) {
        if !matches!(self.state, Link::Ready) {
            return;
        }
        let Some(client) = self.client(store) else {
            return;
        };
        if let Some(token) = self.poll_token.take() {
            fx.cancel(token);
        }
        self.poll_token = Some(
            fx.push(
                AnyEffect::new(PollChatActionsEffect {
                    client: client.chat.clone(),
                    chat: self.chat.clone(),
                })
                .map(ChatPanelCommand::Actions),
            ),
        );
    }

    /// Fold a batch of wire actions into the conversation, then carry
    /// what each one MOVED to the views. The conversation is the only
    /// state that folds; the asks and the queue are panel furniture and
    /// ride beside it.
    fn apply_actions(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        actions: Vec<StateAction>,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let mut ops: Vec<ViewOp> = Vec::new();
        let mut steer: Option<String> = None;
        for action in actions {
            self.furniture(&action);
            let ends = matches!(
                action,
                StateAction::ChatTurnComplete(_)
                    | StateAction::ChatTurnCancelled(_)
                    | StateAction::ChatError(_)
            );
            let (next, change) = self.conversation.fold(&action);
            let moved = change != model::Change::Nothing;
            self.conversation = next;
            self.ops_of(&change, &mut ops);
            if ends && moved {
                self.stack.clear_ask();
                steer = self.steering.take().or(steer);
                if let StateAction::ChatTurnComplete(done) = &action {
                    (self.catalog.note_turn)(store, self.server, &self.chat, &done.turn_id);
                }
                self.mark_read(store, fx);
            }
        }
        self.roll(store, ui, &ops, fx);
        if let Some(text) = steer {
            let _ = self.send_text(store, ui, text, None, None, fx);
        }
    }

    /// The ops a change asks the views for — dressed here ONCE, laid by
    /// every view at its own width.
    fn ops_of(&self, change: &model::Change, ops: &mut Vec<ViewOp>) {
        let cell = |turn: &ahp_wire::client::TurnId, part: &model::PartId| {
            let held = self.conversation.turn(turn)?;
            let moved = held.part(part)?;
            // A tool call's cell is its RUN — consecutive calls share
            // one cell, keyed by the run's first call, however late a
            // call joins or re-dresses.
            if matches!(moved, model::Part::Tool(_)) {
                let (key, spec) = crate::turn::dress_tool_run(held, part)?;
                return Some(ViewOp::Cell {
                    turn: turn.clone(),
                    key,
                    spec,
                });
            }
            let spec = crate::turn::dress_part(moved);
            Some(ViewOp::Cell {
                turn: turn.clone(),
                key: crate::turn::CellKey::Part(part.clone()),
                spec,
            })
        };
        let op = match change {
            model::Change::Nothing => None,
            // A start RESETS the turn: the row re-lays whole.
            model::Change::Said(turn) => self.conversation.turn(turn).cloned().map(ViewOp::Row),
            // A part landed or was re-dressed: THAT cell moves.
            model::Change::Part { turn, part } => cell(turn, part),
            model::Change::Parts { turn, parts } => {
                ops.extend(parts.iter().filter_map(|part| cell(turn, part)));
                None
            }
            model::Change::Retired(turn) => self.conversation.turn(turn).map(|turn| ViewOp::Tail {
                turn: turn.id.clone(),
                cells: crate::turn::dress_tail(turn),
            }),
            model::Change::Grew { turn, part, text } => Some(ViewOp::Grew {
                turn: turn.clone(),
                part: part.clone(),
                text: text.clone(),
            }),
            model::Change::Page => Some(ViewOp::Reset {
                turns: self.conversation.turns().cloned().collect(),
                has_more: self.conversation.older().is_some(),
            }),
        };
        ops.extend(op);
    }

    /// The panel's own furniture: the permission ask a tool call
    /// raises, and the queue the host keeps beside the transcript.
    fn furniture(&mut self, action: &StateAction) {
        match action {
            StateAction::ChatToolCallReady(ready) if ready.confirmed.is_none() => {
                let display = self
                    .conversation
                    .live()
                    .filter(|live| live.as_str() == ready.turn_id)
                    .and_then(|live| self.conversation.turn(live))
                    .and_then(|turn| turn.tool(&model::PartId::new(ready.tool_call_id.clone())))
                    .map(|call| call.display.clone());
                let Some(display) = display else {
                    return;
                };
                let invocation = ready.invocation_message.as_text().to_owned();
                let input = match &ready.tool_input {
                    Some(ToolInput::Inline(text)) => Some(text.clone()),
                    _ => None,
                };
                self.stack.set_ask(PermissionAsk::new(
                    ahp_wire::client::TurnId::new(ready.turn_id.clone()),
                    ready.tool_call_id.clone(),
                    ready
                        .confirmation_title
                        .as_ref()
                        .map(|title| title.as_text().to_owned())
                        .unwrap_or_else(|| display.clone()),
                    invocation,
                    input,
                    ready
                        .options
                        .clone()
                        .unwrap_or_else(default_confirmation_options),
                ));
            }
            StateAction::ChatToolCallConfirmed(answered) => {
                self.stack.clear_ask_for_tool(&answered.tool_call_id);
            }
            StateAction::ChatPendingMessageSet(set)
                if matches!(set.kind, PendingMessageKind::Queued) =>
            {
                self.stack
                    .insert_queued(set.id.clone(), set.message.clone());
            }
            StateAction::ChatPendingMessageRemoved(gone) => {
                self.stack.remove_queued(&gone.id);
            }
            _ => {}
        }
    }

    /// The composer's road: send what was typed, then — on a view
    /// re-read AFTER the model rolled — clear the draft and follow
    /// the tail.
    pub(crate) fn submit(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        id: ChatViewId,
        text: String,
        attachments: Option<Vec<ahp_types::state::MessageAttachment>>,
        model: Option<ahp_types::state::ModelSelection>,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        if !self.send_text(store, ui, text, attachments, model, fx) {
            return;
        }
        let Some(mut view) = self.views.get(&id).cloned() else {
            return;
        };
        view.composer.clear(store, ui);
        view.reveal_tail(store);
        self.views.insert_mut(id, view);
    }

    /// Send a prompt. Returns true when the text was CONSUMED (sent
    /// or parked as a steer) — the caller's cue to clear its draft.
    pub(crate) fn send_text(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        text: String,
        attachments: Option<Vec<ahp_types::state::MessageAttachment>>,
        model: Option<ahp_types::state::ModelSelection>,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> bool {
        if !matches!(self.state, Link::Ready) {
            return false;
        }
        let Some(client) = self.client(store) else {
            return false;
        };
        if text.is_empty() {
            return false;
        }

        if self.busy() {
            // STEERING, the Claude Code Esc-with-prompt shape: a
            // prompt sent at a running agent drops the queue, cancels
            // the turn, and fires the moment the turn ends. (The old
            // road QUEUED here — into a queue nothing ever drained.)
            for (id, _) in self.stack.queue_oracle() {
                self.stack.remove_queued(&id);
                self.model_dispatch(
                    store,
                    StateAction::ChatPendingMessageRemoved(ChatPendingMessageRemovedAction {
                        kind: PendingMessageKind::Queued,
                        id,
                    }),
                    None,
                    fx,
                );
            }
            self.steering = Some(text);
            self.stop(store, fx);
            return true;
        }
        // WRITE-AHEAD, the protocol's own shape: the client MINTS the
        // turn id, folds its own `chat/turnStarted`, and dispatches
        // that very action. The row is on screen before the host has
        // heard of it, and the host's echo re-applies the same action
        // and changes nothing — there is no placeholder to reconcile,
        // and nothing that can leave the composer stuck.
        self.minted += 1;
        let turn = ahp_wire::client::TurnId::new(format!(
            "himark-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_millis())
                .unwrap_or_default(),
            self.minted
        ));
        let action = StateAction::ChatTurnStarted(model::Conversation::opening(
            &turn,
            &text,
            attachments,
            model,
        ));
        let (next, change) = self.conversation.fold(&action);
        self.conversation = next;
        let mut ops: Vec<ViewOp> = Vec::new();
        self.ops_of(&change, &mut ops);
        self.roll(store, ui, &ops, fx);

        let chat = self.chat.clone();
        let failed = turn.clone();
        fx.push(
            AnyEffect::new(DispatchChatActionEffect {
                client: client.session.clone(),
                channel: chat.as_channel(),
                action,
            })
            .map(move |result| match result {
                Ok(()) => ChatPanelCommand::Sent,
                Err(error) => ChatPanelCommand::SendFailed {
                    turn: failed.clone(),
                    error,
                },
            }),
        );
        true
    }

    /// A send the host never took: the turn we minted is FAILED, said
    /// the way the protocol says a turn fails.
    fn apply_send_failed(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        turn: ahp_wire::client::TurnId,
        error: String,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        eprintln!("[higent] the send was not delivered: {error}");
        let action = StateAction::ChatError(ahp_types::actions::ChatErrorAction {
            turn_id: turn.as_str().to_owned(),
            duration: 0,
            part: ahp_types::state::ErrorResponsePart {
                error: ahp_types::state::ErrorInfo {
                    error_type: "not-delivered".to_owned(),
                    message: error,
                    stack: None,
                    meta: None,
                },
                resumable: None,
            },
            meta: None,
        });
        let (next, change) = self.conversation.fold(&action);
        self.conversation = next;
        let mut ops: Vec<ViewOp> = Vec::new();
        self.ops_of(&change, &mut ops);
        self.roll(store, ui, &ops, fx);
    }

    /// The diff header's OPEN: hand the wire uri to the app, which
    /// resolves it against this session's client and opens the WORKING
    /// COPY — the live file, not the snapshots the diff was built from.
    fn open_edited_file(&self, store: &mut Store, uri: String) {
        let Some(road) = store.get::<OpenEditedRoad>().map(|road| road.0.clone()) else {
            return;
        };
        road(store, self.server, self.session.clone(), uri);
    }

    pub(crate) fn perform_model(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: ChatPanelCommand,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        match command {
            ChatPanelCommand::InView(id, inner) => {
                self.perform_in_view(store, ui, id, *inner, fx);
            }
            ChatPanelCommand::Boot => {
                if self.boot_feed() {
                    imba::command::Requests::push(
                        store,
                        std::sync::Arc::new(crate::chats::EnsureChatFeed {
                            chats: self.chats,
                            chat: self.chat.clone(),
                        }),
                    );
                }
            }
            ChatPanelCommand::Snapshot(result) => self.apply_snapshot(store, ui, result, fx),
            ChatPanelCommand::Older(result) => self.apply_older(store, ui, result, fx),
            ChatPanelCommand::Sent => {}
            ChatPanelCommand::SendFailed { turn, error } => {
                self.apply_send_failed(store, ui, turn, error, fx)
            }
            ChatPanelCommand::Actions(actions) => {
                self.apply_actions(store, ui, actions, fx);
                self.relaunch_poll(store, fx);
            }
            ChatPanelCommand::Answer(index) => self.answer(store, index, fx),
            ChatPanelCommand::Dispatched { undo_queue, result } => {
                if let Err(error) = result {
                    eprintln!("[higent] dispatch failed: {error}");
                    if let Some(id) = undo_queue {
                        self.stack.remove_queued(&id);
                    }
                }
            }
            ChatPanelCommand::Composer(ComposerCommand::Stop) => {
                self.steering = None;
                self.stop(store, fx);
            }
            ChatPanelCommand::Stack(StackCommand::Answer(index)) => self.answer(store, index, fx),
            ChatPanelCommand::Stack(StackCommand::ToggleQueue) => self.stack.toggle_collapsed(),
            ChatPanelCommand::Stack(StackCommand::RemoveQueued(id)) => {
                self.stack.remove_queued(&id);
                self.model_dispatch(
                    store,
                    StateAction::ChatPendingMessageRemoved(ChatPendingMessageRemovedAction {
                        kind: PendingMessageKind::Queued,
                        id,
                    }),
                    None,
                    fx,
                );
            }
            // A view command with no view address: the sheet-era
            // roads never mint these chat-scoped.
            other => {
                let _ = other;
            }
        }
    }

    /// The pane road: `view` is the pane's record. View furniture
    /// commands land on it; model commands fall through.
    /// Point a view at an area. Fronting the chat (⌘I) must land the
    /// keyboard in the composer — without this the view keeps whatever
    /// area a transcript click left it on.
    pub(crate) fn focus_view(&mut self, id: ChatViewId, area: ChatArea) {
        if let Some(mut view) = self.views.get(&id).cloned() {
            view.focus = area;
            self.views.insert_mut(id, view);
        }
    }

    pub(crate) fn focus_area_view(&self, id: ChatViewId) -> Option<ChatArea> {
        self.views.get(&id).map(|view| view.focus)
    }

    pub(crate) fn perform_in_view(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        id: ChatViewId,
        command: ChatPanelCommand,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        // A view is minted by `claim_view` and nowhere else: a landing
        // addressed to a mount that was dismantled is simply late.
        if !self.views.contains_key(&id) {
            return;
        }
        match command {
            ChatPanelCommand::InView(id, inner) => {
                self.perform_in_view(store, ui, id, *inner, fx);
            }

            ChatPanelCommand::Focus(area, then) => {
                if let Some(mut view) = self.views.get(&id).cloned() {
                    view.focus = area;
                    self.views.insert_mut(id, view);
                }
                if let Some(command) = then {
                    self.perform_in_view(store, ui, id, *command, fx);
                }
            }

            ChatPanelCommand::Rows(command) => {
                if peeled_activation(&command) {
                    self.load_older(store, ui, fx);
                    return;
                }
                // A diff header's OPEN is MODEL work — the client lives
                // on the panel — so it is consumed before the command
                // reaches the mount's furniture.
                if let Some(uri) = peeled_open_file(&command) {
                    self.open_edited_file(store, uri);
                    return;
                }
                if let Some(index) = peeled_wake(&command) {
                    let client = self.client(store);
                    let Some(mut view) = self.views.get(&id).cloned() else {
                        return;
                    };
                    fx.scope(
                        move |command| ChatPanelCommand::InView(id, Box::new(command)),
                        |fx| view.wake_row(store, ui, &client, index, fx),
                    );
                    self.views.insert_mut(id, view);
                    return;
                }
                let Some(mut view) = self.views.get(&id).cloned() else {
                    return;
                };
                fx.scope(
                    move |command| {
                        ChatPanelCommand::InView(id, Box::new(ChatPanelCommand::Rows(command)))
                    },
                    |fx| view.rows.perform(store, ui, command, fx),
                );
                self.views.insert_mut(id, view);
            }

            ChatPanelCommand::Cell {
                command: CellCommand::OpenFile(uri),
                ..
            } => {
                self.open_edited_file(store, uri);
            }

            ChatPanelCommand::Cell {
                turn,
                cell,
                command,
            } => {
                let Some(mut view) = self.views.get(&id).cloned() else {
                    return;
                };
                fx.scope(
                    move |command| ChatPanelCommand::InView(id, Box::new(command)),
                    |fx| view.route_cell(store, ui, turn, cell, command, fx),
                );
                self.views.insert_mut(id, view);
            }

            ChatPanelCommand::Send | ChatPanelCommand::Composer(ComposerCommand::Submit) => {
                let Some(mut view) = self.views.get(&id).cloned() else {
                    return;
                };
                if view.composer.is_empty() {
                    return;
                }
                let text = view.composer.text().trim().to_owned();
                if view.completion.open() {
                    fx.scope(
                        move |command| ChatPanelCommand::InView(id, Box::new(command)),
                        |fx| {
                            view.completion.drop_state(
                                view.composer.document_mut(),
                                store,
                                ui,
                                fx,
                                completion_editor,
                            )
                        },
                    );
                }
                let uris = (self.catalog.uris)(store, self.server);
                let attachments = view.completion_attachments(uris, &text);
                let model = view.toolbar.model_selection();
                // The view goes BACK before the send: `send_text` rolls
                // the echo row into every mounted view, and a clone
                // taken before that roll would carry the pre-send rows
                // right back over it — the user's own message would
                // then appear only when the next op reached the view.
                self.views.insert_mut(id, view);
                self.submit(store, ui, id, text, attachments, model, fx);
            }

            // The STOP button lives in the composer band, so its click
            // arrives view-scoped — but stopping is MODEL work, and
            // the composer's own road drops it on the floor.
            ChatPanelCommand::Composer(ComposerCommand::Stop) => {
                self.steering = None;
                self.stop(store, fx);
            }

            ChatPanelCommand::Composer(command) => {
                let Some(mut view) = self.views.get(&id).cloned() else {
                    return;
                };
                let session = self.session_id();
                let Some(recents) = store.entity(self.chats).map(|chats| chats.recents()) else {
                    return;
                };
                let folders = std::sync::Arc::new((self.catalog.folders)(store, &session));
                fx.scope(
                    move |command| ChatPanelCommand::InView(id, Box::new(command)),
                    |fx| view.composer_command(store, ui, folders, recents, command, fx),
                );
                self.views.insert_mut(id, view);
            }

            ChatPanelCommand::Blurred(blurred) => {
                let Some(mut view) = self.views.get(&id).cloned() else {
                    return;
                };
                fx.scope(
                    move |command| ChatPanelCommand::InView(id, Box::new(command)),
                    |fx| {
                        if blurred && view.completion.open() {
                            view.completion.drop_state(
                                view.composer.document_mut(),
                                store,
                                ui,
                                fx,
                                completion_editor,
                            );
                        }
                    },
                );
                view.composer.set_blurred(blurred);
                self.views.insert_mut(id, view);
            }

            ChatPanelCommand::CompletionFound(found) => {
                let Some(mut view) = self.views.get(&id).cloned() else {
                    return;
                };
                let editor = view.composer.editor();
                let (completion, mut composer) = (&mut view.completion, &mut view.composer);
                completion.land(store, ui, composer.document_mut(), editor, found);
                let _ = &mut composer;
                self.views.insert_mut(id, view);
            }

            ChatPanelCommand::ToolbarSync => {
                let Some(mut view) = self.views.get(&id).cloned() else {
                    return;
                };
                if let Some(channel) = (self.catalog.channel)(store, &self.session_id()) {
                    let agents = (self.catalog.agents)(store, self.server);
                    view.toolbar.sync(store, ui, &agents, &channel);
                }
                self.views.insert_mut(id, view);
            }

            ChatPanelCommand::Toolbar(command) => {
                let Some(mut view) = self.views.get(&id).cloned() else {
                    return;
                };
                let ask = {
                    let mut ask = crate::session_toolbar::ToolbarAsk::None;
                    fx.scope(
                        move |command| {
                            ChatPanelCommand::InView(
                                id,
                                Box::new(ChatPanelCommand::Toolbar(command)),
                            )
                        },
                        |fx| ask = view.toolbar.perform(store, ui, command, fx),
                    );
                    ask
                };
                self.views.insert_mut(id, view);
                match ask {
                    crate::session_toolbar::ToolbarAsk::Edits(mode) => {
                        if let Some(client) = self.client(store) {
                            let mut config = serde_json::Map::new();
                            config.insert("permissionMode".to_owned(), serde_json::json!(mode));
                            fx.push(
                                AnyEffect::new(DispatchChatActionEffect {
                                    client: client.session.clone(),
                                    channel: self.session.as_channel(),
                                    action: StateAction::SessionConfigChanged(
                                        ahp_types::actions::SessionConfigChangedAction {
                                            config,
                                            replace: None,
                                        },
                                    ),
                                })
                                .map(|result| {
                                    ChatPanelCommand::Dispatched {
                                        undo_queue: None,
                                        result,
                                    }
                                }),
                            );
                        }
                    }

                    crate::session_toolbar::ToolbarAsk::None => {}
                }
            }

            model => self.perform_model(store, ui, model, fx),
        }
    }

    pub(crate) fn focus_data_view<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::ui::UiCtx,
        id: ChatViewId,
    ) -> imba::focus::FocusData<'w, ChatPanelCommand> {
        use imba::focus::FocusData;
        let Some(view) = self.views.get(&id) else {
            return FocusData::default();
        };
        let ask = self.stack.ask_keys();
        let composer_empty = view.composer.is_empty();
        let expanded = view.composer.expanded();
        let focus = view.focus;

        let own = FocusData {
            on_key: Some(Box::new(move |key, mods| match (key, mods) {
                (Key::Enter, mods) if ask.is_some() && composer_empty && !mods.shift => {
                    EventResult::Command(ChatPanelCommand::Answer(0))
                }
                (Key::Escape, _) if ask.is_some() => match ask.and_then(|ask| ask.deny) {
                    Some(deny) => EventResult::Command(ChatPanelCommand::Answer(deny)),
                    None => EventResult::Ignored,
                },

                (Key::Escape, _) if expanded => {
                    EventResult::Command(ChatPanelCommand::Composer(ComposerCommand::ToggleExpand))
                }

                (Key::Enter, mods) if mods.command && focus == ChatArea::Composer => {
                    EventResult::Command(ChatPanelCommand::Send)
                }
                _ => EventResult::Ignored,
            })),
            on_text: Some(Box::new(move |text| {
                let Some(ask) = ask else {
                    return EventResult::Ignored;
                };
                match text.chars().next().and_then(|c| c.to_digit(10)) {
                    Some(digit) if digit >= 1 && (digit as usize) <= ask.count => {
                        EventResult::Command(ChatPanelCommand::Answer(digit as usize - 1))
                    }
                    _ => EventResult::Ignored,
                }
            })),
            ..FocusData::default()
        };
        let area = match view.focus {
            ChatArea::Composer => view
                .composer
                .focus_data(store, ui)
                .map(ChatPanelCommand::Composer),
            ChatArea::Transcript => view.rows.focus_data(store, ui).map(ChatPanelCommand::Rows),
        };
        own.merge_under(area)
    }

    pub(crate) fn display_view<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        id: ChatViewId,
    ) -> Option<impl imba::layout::Layout<'a, ChatPanelCommand> + imba::layout::LayoutValue + 'a>
    {
        let view = self.views.get(&id)?;
        Some(imba::layout::laid(
            move |_arena: &'a Arena, constraints: Constraints| {
                let size = constraints.max;
                let theme = env::Themes::of(store);
                let chrome = theme.ui().chat.clone();
                view.panel_width
                    .store(size.width.max(1.0).to_bits(), Ordering::Relaxed);

                let band_h = view.composer.band_height(&chrome, size.height);
                let stack_h = self.stack.height(&chrome);

                let toolbar_h = theme.ui().toolbar.height;
                let rows_height = (size.height - band_h - stack_h - toolbar_h).max(1.0);
                view.rows_height
                    .store(rows_height.to_bits(), Ordering::Relaxed);

                let mut panel = container(arena, size);

                panel.place(
                    0.0,
                    0.0,
                    imba::layout::Layout::layout(
                        view.rows.display(arena, store, ui),
                        arena,
                        Constraints {
                            min: Size::new(size.width, rows_height),
                            max: Size::new(size.width, rows_height),
                        },
                    )
                    .map(ChatPanelCommand::Rows)
                    .focus_scope(view.focus == ChatArea::Transcript),
                );

                let status = match &self.state {
                    Link::Idle | Link::Subscribing => "connecting…".to_owned(),
                    Link::Failed(error) => format!("failed: {error}"),
                    Link::Ready if self.conversation.is_running() => "responding…".to_owned(),
                    Link::Ready => String::new(),
                };
                let composer_empty = view.composer.is_empty();
                panel.place(
                    0.0,
                    rows_height + stack_h,
                    view.composer
                        .layout(
                            arena,
                            store,
                            ui,
                            size.width,
                            size.height,
                            ComposerProps {
                                focused: view.focus == ChatArea::Composer,
                            },
                        )
                        .map(ChatPanelCommand::Composer),
                );

                if stack_h > 0.0 {
                    panel.place(
                        0.0,
                        rows_height,
                        self.stack
                            .layout(arena, ui, &chrome, size.width)
                            .map(ChatPanelCommand::Stack),
                    );
                }

                // The SEND cell's width, computed up front: the combo
                // strip's budget ends where SEND begins — the button
                // always fits and nothing ever covers it.
                let send_cell_width = {
                    let ui_theme = theme.ui();
                    let busy = self.busy();
                    let stop = busy && composer_empty;
                    let label = if stop {
                        "STOP"
                    } else if busy {
                        "STEER"
                    } else {
                        "SEND"
                    };
                    let caps_font = hikit::fonts::ui_font(ui, ui_theme.combo.label_size * 1.1);
                    let key_font = hikit::fonts::ui_text_font(ui, ui_theme.peeker.hint_size * 0.95);
                    label
                        .chars()
                        .map(|ch| caps_font.measure_str(ch.to_string(), None).0 + 1.5)
                        .sum::<f32>()
                        + imba::layout::text_advance(ui, &key_font, "⌘⏎")
                        + ui_theme.combo.gap
                        + ui_theme.combo.pad * 2.0
                };
                let toolbar_cells_right = view.toolbar.place(
                    arena,
                    &mut panel,
                    store,
                    ui,
                    size.height - toolbar_h + 1.0,
                    toolbar_h - 1.0,
                    (size.width - send_cell_width - theme.ui().combo.gap).max(0.0),
                );

                // The footer's edges: a hairline between the transcript
                // and the composer band, and one above the toolbar row
                // (whose placement already reserves the pixel).
                {
                    let rule = theme.ui().toolbar.rule.0;
                    let hairline =
                        move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: skia_safe::Rect| {
                            let mut paint = skia_safe::Paint::default();
                            paint.set_anti_alias(false);
                            paint.set_color(rule);
                            canvas.draw_rect(rect, &paint);
                        };
                    panel.place(
                        0.0,
                        rows_height,
                        leaf::<ChatPanelCommand>(size.width, 1.0).paint_instead(hairline),
                    );
                    panel.place(
                        0.0,
                        size.height - toolbar_h,
                        leaf::<ChatPanelCommand>(size.width, 1.0).paint_instead(hairline),
                    );
                }

                {
                    let ui_theme = theme.ui();
                    let busy = self.busy();
                    let sendable = !composer_empty && matches!(self.state, Link::Ready);
                    let stop = busy && composer_empty;
                    let label = if stop {
                        "STOP"
                    } else if busy {
                        "STEER"
                    } else {
                        "SEND"
                    };
                    let caps_font = hikit::fonts::ui_font(ui, ui_theme.combo.label_size * 1.1);
                    let key_font = hikit::fonts::ui_text_font(ui, ui_theme.peeker.hint_size * 0.95);
                    let pad = ui_theme.combo.pad;
                    let cell_width = send_cell_width;
                    let accent = if stop {
                        chrome.stop_color.0
                    } else {
                        chrome.accent.0
                    };
                    let on_accent = chrome.on_accent.0;
                    let accent_soft = ui_theme.peeker.dim_text.0;
                    let gap = ui_theme.combo.gap;
                    // The cell's two texts as a Row of `Text`s at exact
                    // baseline parity: the old per-char loop advanced by
                    // glyph width + 1.5 (= `.tracking(1.5)`) from x =
                    // left + pad, and drew the key hint a `gap` after the
                    // label's last advance (= the Row's `.gap`). Each text
                    // pads down so its baseline lands on the old
                    // mid + font.size() * 0.35 line. The accent fill and
                    // left rule stay a backdrop painter; the press is
                    // `.on_click`, minting Stop or Submit like the old
                    // event closure.
                    let cell_h = toolbar_h - 1.0;
                    let mid = cell_h * 0.5;
                    let caps_ascent = -caps_font.metrics().1.ascent;
                    let key_ascent = -key_font.metrics().1.ascent;
                    let mut row = imba::layout::Row::new(arena).gap(gap).child(
                        imba::layout::text(ui, label, caps_font.clone(), on_accent)
                            .tracking(1.5)
                            .pad_insets(imba::layout::Insets {
                                left: 0.0,
                                top: (mid + caps_font.size() * 0.35 - caps_ascent).max(0.0),
                                right: 0.0,
                                bottom: 0.0,
                            }),
                    );
                    if !stop {
                        row = row.child(
                            imba::layout::text(ui, "⌘⏎", key_font.clone(), accent_soft).pad_insets(
                                imba::layout::Insets {
                                    left: 0.0,
                                    top: (mid + key_font.size() * 0.35 - key_ascent).max(0.0),
                                    right: 0.0,
                                    bottom: 0.0,
                                },
                            ),
                        );
                    }
                    let cell = row
                        .pad_insets(imba::layout::Insets {
                            left: pad,
                            top: 0.0,
                            right: 0.0,
                            bottom: 0.0,
                        })
                        .sized(cell_width, cell_h)
                        .backdrop(
                            move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: Rect| {
                                let mut paint = skia_safe::Paint::default();
                                let mut fill = accent;
                                if !sendable && !busy {
                                    fill = fill.with_a(0x50);
                                }
                                paint.set_color(fill.with_a(fill.a() / 3));
                                canvas.draw_rect(rect, &paint);
                                paint.set_anti_alias(false);
                                paint.set_color(fill);
                                canvas.draw_rect(
                                    skia_safe::Rect::from_xywh(
                                        rect.left,
                                        rect.top,
                                        1.0,
                                        rect.height(),
                                    ),
                                    &paint,
                                );
                            },
                        )
                        .on_click(move || {
                            ChatPanelCommand::Composer(match stop {
                                true => ComposerCommand::Stop,
                                false => ComposerCommand::Submit,
                            })
                        });
                    panel.place_boxed(
                        size.width - cell_width,
                        size.height - toolbar_h + 1.0,
                        cell.layout(arena, Constraints::tight(Size::new(cell_width, cell_h))),
                    );

                    // The shortcut legend (or the link's status) sits on
                    // the toolbar, right against the SEND cell. The ⌘⏎
                    // hint already lives on the cell itself.
                    let legend = match status.is_empty() {
                        true => "⎋ chat".to_owned(),
                        false => status.clone(),
                    };
                    let legend_w = imba::layout::text_advance(ui, &key_font, &legend);
                    let legend_x = size.width - cell_width - gap - legend_w;
                    // Squeezed out by the combo cells? The legend yields.
                    if legend_x >= toolbar_cells_right + gap {
                        panel.place_boxed(
                            legend_x,
                            size.height - toolbar_h + 1.0,
                            imba::layout::text(ui, legend, key_font.clone(), accent_soft)
                                .pad_insets(imba::layout::Insets {
                                    left: 0.0,
                                    top: (mid + key_font.size() * 0.35 - key_ascent).max(0.0),
                                    right: 0.0,
                                    bottom: 0.0,
                                })
                                .layout(arena, Constraints::tight(Size::new(legend_w, cell_h))),
                        );
                    }
                }
                let strip_origin = std::sync::Arc::clone(&view.toolbar.strip_origin);
                let toolbar_stale = (self.catalog.channel)(store, &self.session_id())
                    .map(|channel| {
                        let agents = (self.catalog.agents)(store, self.server);
                        crate::session_toolbar::SessionToolbar::fingerprint(&agents, &channel)
                    })
                    .is_some_and(|fingerprint| fingerprint != view.toolbar.synced);

                let rows_height_ = rows_height;
                let focus = view.focus;
                let boot = matches!(self.state, Link::Idle);
                let strip_top = size.height - toolbar_h + 1.0;
                panel.wrap_realized(move |panel| ChatWidget {
                    panel,
                    rows_height: rows_height_,
                    focus,
                    boot,
                    toolbar_stale,
                    strip_origin,
                    strip_top,
                })
            },
        ))
    }
}

impl ChatView {
    fn new(store: &Store, ui: &UiCtx) -> Self {
        Self {
            rows: ScrollView::new(ListView::empty()),
            composer: Composer::new(store, ui),
            completion: crate::completion::Completion::new(),
            picked: rpds::VectorSync::new_sync(),
            focus: ChatArea::Composer,
            has_loader: false,
            toolbar: super::session_toolbar::SessionToolbar::new(store, ui),
            panel_width: AtomicU32::new(800.0_f32.to_bits()),
            rows_height: AtomicU32::new(600.0_f32.to_bits()),
        }
    }

    fn panel_width(&self) -> f32 {
        f32::from_bits(self.panel_width.load(Ordering::Relaxed))
    }

    fn near_tail(&self) -> bool {
        let viewport = f32::from_bits(self.rows_height.load(Ordering::Relaxed));
        let total = self.rows.content().total_height();
        self.rows.scroll_y() + viewport >= total - viewport * 0.5
    }

    fn reveal_tail(&mut self, store: &mut Store) {
        let _ = store;
        self.rows.set_scroll_y(f32::MAX);
    }

    fn build_cell(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        client: &Option<ahp_wire::client::Client>,
        turn: &ahp_wire::client::TurnId,
        key: &crate::turn::CellKey,
        spec: CellSpec,
        content_width: f32,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> (Cell, f32) {
        let turn_key = turn.clone();
        let cell_key = key.clone();
        match spec {
            // A markdown cell is a TEXT, a PARSE and a LAYOUT. None of
            // that belongs on the UI thread: the cell is born as a band
            // at an estimated height and `BuildDocumentEffect` builds
            // the document off-thread — the landing only mounts it.
            CellSpec::Text(kind, markdown) => {
                let (cell, height) = Cell::pending_text(store, kind, &markdown, content_width);
                let nonce = cell.pending_nonce().expect("born pending");
                fx.push(
                    AnyEffect::new(documents::BuildDocumentEffect {
                        location: cell_location(),
                        text: markdown,
                    })
                    .map(move |built| ChatPanelCommand::Cell {
                        turn: turn_key.clone(),
                        cell: cell_key.clone(),
                        command: CellCommand::ResolveText { nonce, built },
                    }),
                );
                (cell, height)
            }
            CellSpec::Tools(specs) => Cell::tools(store, ui, specs, content_width),
            // A diff cell is TWO documents, a parse each, a diff and its
            // marks. The cell is born as a header band and
            // `BuildFileEditEffect` fetches both sides and builds the
            // pair off-thread — the landing only lays the editors.
            CellSpec::Diff(spec) => {
                let Some(client) = client.clone() else {
                    return Cell::pending_diff(store, spec.header, content_width);
                };
                fx.push(
                    AnyEffect::new(crate::file_edit::BuildFileEditEffect {
                        client: client.chat.clone(),
                        before: spec.before,
                        after: spec.after,
                        name: spec.header.title.clone(),
                    })
                    .map(move |result| ChatPanelCommand::Cell {
                        turn: turn_key.clone(),
                        cell: cell_key.clone(),
                        command: CellCommand::ResolveDiff(result),
                    }),
                );
                Cell::pending_diff(store, spec.header, content_width)
            }
        }
    }

    fn build_turn(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        client: &Option<ahp_wire::client::Client>,
        turn: &model::Turn,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> (TurnView, f32) {
        let content_width = TurnView::content_width(self.panel_width());
        let mut cells = ListSlice::new();
        let mut total = 0.0;
        for (key, spec) in crate::turn::dress(turn).iter().cloned() {
            let (cell, height) =
                self.build_cell(store, ui, client, &turn.id, &key, spec, content_width, fx);
            total += height;
            cells.push_keyed_sized(key, cell, height);
        }
        (TurnView::new(turn.id.as_str(), content_width, cells), total)
    }

    /// Only the tail's `eager` turns build their editors now — the
    /// rest SLEEP at an estimated height and wake on first paint, so
    /// opening a chat costs the viewport, not the history.
    fn build_page<'t>(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        client: &Option<ahp_wire::client::Client>,
        turns: impl IntoIterator<Item = &'t model::Turn>,
        lead_loader: bool,
        eager: usize,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> ListSlice<ChatRow, ahp_wire::client::TurnId> {
        let chrome = env::Themes::of(store).ui().chat.clone();
        let mut slice = ListSlice::new();
        if lead_loader {
            slice.push_sized(ChatRow::Loader { armed: true }, chrome.loader_height);
        }
        let turns: Vec<&model::Turn> = turns.into_iter().collect();
        let asleep = turns.len().saturating_sub(eager);
        for (at, record) in turns.into_iter().enumerate() {
            if at < asleep {
                slice.push_keyed_sized(
                    record.id.clone(),
                    ChatRow::Sleeping(record.clone()),
                    sleeping_height(record, &chrome),
                );
                continue;
            }
            let (view, height) = self.build_turn(store, ui, client, record, fx);
            slice.push_keyed_sized(record.id.clone(), ChatRow::Turn(view), height);
        }
        slice
    }

    /// Wake ONE sleeping row: build its editors at this view's width
    /// and settle the anchor over the height correction.
    fn wake_row(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        client: &Option<ahp_wire::client::Client>,
        index: usize,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let Some(ChatRow::Sleeping(record)) = self.rows.content().view_at(index) else {
            return;
        };
        let (view, height) = self.build_turn(store, ui, client, &record, fx);
        let mut slice = ListSlice::new();
        slice.push_keyed_sized(record.id.clone(), ChatRow::Turn(view), height);
        self.rows
            .content_mut()
            .splice_slice(index..index + 1, slice);
        fx.settle();
    }

    fn set_loader(&mut self, store: &Store, armed: bool) {
        if !self.has_loader {
            return;
        }
        let height = env::Themes::of(store).ui().chat.loader_height;
        self.rows
            .content_mut()
            .splice(0..1, [(ChatRow::Loader { armed }, height)]);
    }

    /// ONE cell moved: a row that lacks it builds it and puts it where
    /// its key belongs; a row that has it re-dresses it in place. A row
    /// still asleep waits for its wake, which dresses the whole turn
    /// from the model anyway.
    fn place_cell(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        client: &Option<ahp_wire::client::Client>,
        turn: &ahp_wire::client::TurnId,
        key: &crate::turn::CellKey,
        spec: CellSpec,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let Some(range) = self.rows.content().row_range(turn) else {
            return;
        };
        let Some(ChatRow::Turn(row)) = self.rows.content().view_at(range.start) else {
            return;
        };
        if row.cell_at(key).is_some() {
            for command in redress(&spec) {
                self.route_cell(store, ui, turn.clone(), key.clone(), command, fx);
            }
            return;
        }
        let content_width = TurnView::content_width(self.panel_width());
        let (cell, height) = self.build_cell(store, ui, client, turn, key, spec, content_width, fx);
        fx.scope(ChatPanelCommand::Rows, |fx| {
            self.rows.perform(
                store,
                ui,
                ScrollCommand::Content(ListCommand::Child(
                    range.start,
                    RowCommand::Place {
                        key: key.clone(),
                        cell,
                        height,
                    },
                )),
                fx,
            )
        });
    }

    /// From a turn and a cell key to the cell, in two keyed hops — and
    /// whatever the cell sends back (an editor effect's landing) is
    /// addressed the same way, so it finds the cell however the row has
    /// moved meanwhile.
    fn route_cell(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        turn: ahp_wire::client::TurnId,
        key: crate::turn::CellKey,
        command: CellCommand,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let Some(range) = self.rows.content().row_range(&turn) else {
            return;
        };
        let Some(ChatRow::Turn(row)) = self.rows.content().view_at(range.start) else {
            return;
        };
        let Some(cell) = row.cell_at(&key) else {
            return;
        };
        let index = range.start;
        fx.scope(
            move |command: RowsCommand| lift_rows_command(&turn, &key, command),
            |fx| {
                self.rows.perform(
                    store,
                    ui,
                    ScrollCommand::Content(ListCommand::Child(
                        index,
                        RowCommand::Turn(ListCommand::Child(cell, command)),
                    )),
                    fx,
                )
            },
        );
    }

    fn composer_command(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        folders: std::sync::Arc<Vec<editor::location::ResourceLocation>>,
        recents: imba::store::Id<recents::RecentLocations>,
        command: ComposerCommand,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let Some(command) = self.completion_intercept(store, ui, command, fx) else {
            return;
        };
        let typed_at = matches!(
            &command,
            ComposerCommand::Editor(imba::scroll::ScrollCommand::Content(
                ::editor::editor_view::EditorCommand::InsertText { text }
            )) if text == "@"
        );
        fx.scope(ChatPanelCommand::Composer, |fx| {
            self.composer.perform(store, ui, command, fx)
        });

        let editor = self.composer.editor();
        let at = typed_at.then(|| {
            self.composer
                .document()
                .caret_byte(editor)
                .saturating_sub(1)
        });
        self.completion.sync_path(
            store,
            ui,
            self.composer.document_mut(),
            editor,
            at,
            folders,
            recents,
            None,
            fx,
            ChatPanelCommand::CompletionFound,
            completion_editor,
        );
    }

    fn completion_intercept(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: ComposerCommand,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> Option<ComposerCommand> {
        use imba::scroll::ScrollCommand;
        let ComposerCommand::Editor(ScrollCommand::Content(
            ::editor::editor_view::EditorCommand::Inlay {
                key,
                command: inlay,
            },
        )) = command
        else {
            return Some(command);
        };
        if Some(key) != self.completion.inlay_key() {
            return Some(ComposerCommand::Editor(ScrollCommand::Content(
                ::editor::editor_view::EditorCommand::Inlay {
                    key,
                    command: inlay,
                },
            )));
        }
        let popup = match inlay.downcast_ref::<crate::completion::CompletionCommand>() {
            Some(_) => inlay
                .downcast::<crate::completion::CompletionCommand>()
                .expect("probed above"),
            None => {
                return Some(ComposerCommand::Editor(ScrollCommand::Content(
                    ::editor::editor_view::EditorCommand::Inlay {
                        key,
                        command: inlay,
                    },
                )))
            }
        };
        use crate::completion::CompletionCommand;
        let editor = self.composer.editor();
        match popup {
            CompletionCommand::Select(delta) => {
                self.completion
                    .select(store, self.composer.document_mut(), editor, delta);
            }
            CompletionCommand::PickCursor => {
                let row = self.completion.selected();
                if let Some(pick) = self.completion.apply_pick(
                    store,
                    ui,
                    self.composer.document_mut(),
                    editor,
                    row,
                    fx,
                    completion_editor,
                ) {
                    self.picked.push_back_mut(pick);
                }
            }
            CompletionCommand::Rows(rows) => {
                let picked = self.completion.rows_command(
                    store,
                    ui,
                    self.composer.document_mut(),
                    editor,
                    rows,
                );
                if let Some(row) = picked {
                    if let Some(pick) = self.completion.apply_pick(
                        store,
                        ui,
                        self.composer.document_mut(),
                        editor,
                        row,
                        fx,
                        completion_editor,
                    ) {
                        self.picked.push_back_mut(pick);
                    }
                }
            }
            CompletionCommand::Close => {
                self.completion.drop_state(
                    self.composer.document_mut(),
                    store,
                    ui,
                    fx,
                    completion_editor,
                );
            }
        }
        None
    }

    fn completion_attachments(
        &mut self,
        uris: Option<std::sync::Arc<dyn ahp_wire::client::ResourceUriMap>>,
        text: &str,
    ) -> Option<Vec<ahp_types::state::MessageAttachment>> {
        let uris = uris?;
        let picked: Vec<crate::completion::PickedFile> = std::mem::take(&mut self.picked)
            .into_iter()
            .cloned()
            .collect();
        super::file_completion::resource_attachments(text, picked, uris.as_ref())
    }

    /// One model mutation, laid at THIS view's width.
    fn apply(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        client: &Option<ahp_wire::client::Client>,
        op: &ViewOp,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        match op {
            ViewOp::Reset { turns, has_more } => {
                let slice = self.build_page(store, ui, client, turns, *has_more, EAGER_TAIL, fx);
                let len = self.rows.content().len();
                self.rows.content_mut().splice_slice(0..len, slice);
                self.has_loader = *has_more;
            }
            ViewOp::Prepend { turns, has_more } => {
                // The page lands above the viewport: every turn
                // sleeps; scrolling up wakes them one paint at a time.
                let slice = self.build_page(store, ui, client, turns, *has_more, 0, fx);
                let end = usize::from(self.has_loader);
                self.rows.content_mut().splice_slice(0..end, slice);
                self.has_loader = *has_more;
            }
            ViewOp::Loader { armed } => self.set_loader(store, *armed),
            ViewOp::Row(turn) => {
                // A turn we hold is replaced where it stands; one we do
                // not joins the tail. The key is the turn id, so this is
                // the same op either way.
                let (view, height) = self.build_turn(store, ui, client, turn, fx);
                let mut slice = ListSlice::new();
                slice.push_keyed_sized(turn.id.clone(), ChatRow::Turn(view), height);
                let range = self.rows.content().row_range(&turn.id);
                let len = self.rows.content().len();
                self.rows
                    .content_mut()
                    .splice_slice(range.unwrap_or(len..len), slice);
            }
            ViewOp::Cell { turn, key, spec } => {
                self.place_cell(store, ui, client, turn, key, spec.clone(), fx);
            }
            ViewOp::Tail { turn, cells } => {
                for (key, spec) in cells.iter().cloned() {
                    self.place_cell(store, ui, client, turn, &key, spec, fx);
                }
            }
            ViewOp::Grew { turn, part, text } => {
                // From the part id straight to its cell: no counting.
                self.route_cell(
                    store,
                    ui,
                    turn.clone(),
                    crate::turn::CellKey::Part(part.clone()),
                    CellCommand::Append(text.clone()),
                    fx,
                );
            }
        }
    }
}

/// The name a chat cell's document carries: the build road picks its
/// parser off the extension, and a cell is always markdown.
fn cell_location() -> editor::location::ResourceLocation {
    editor::location::ResourceLocation::new(
        editor::location::ResourceType::document(),
        ::editor::location::Authority::new("chat"),
        vec!["cell.md".to_owned()],
    )
}

/// How a cell that already stands takes a new spec — in place, never
/// by minting a document again.
fn redress(spec: &CellSpec) -> Vec<CellCommand> {
    match spec {
        CellSpec::Text(_, text) => {
            vec![CellCommand::Rewrite(text::text::Text::from_string_exact(
                text,
            ))]
        }
        // ADD, not Face: a call the group already holds takes it as a
        // face refresh, a call that just joined the run splices in.
        CellSpec::Tools(specs) => specs
            .iter()
            .map(|spec| CellCommand::Tool(crate::tool_group::ToolUpdate::Add(spec.clone())))
            .collect(),
        // A diff cell resolves itself through its own landing.
        CellSpec::Diff(_) => Vec::new(),
    }
}

/// What a routed cell sends back, lifted to the cell's OWN address.
fn lift_rows_command(
    turn: &ahp_wire::client::TurnId,
    key: &crate::turn::CellKey,
    command: RowsCommand,
) -> ChatPanelCommand {
    if let ScrollCommand::Content(ListCommand::Child(
        _,
        RowCommand::Turn(ListCommand::Child(_, command)),
    )) = command
    {
        return ChatPanelCommand::Cell {
            turn: turn.clone(),
            cell: key.clone(),
            command,
        };
    }
    ChatPanelCommand::Rows(command)
}

fn default_confirmation_options() -> Vec<ConfirmationOption> {
    vec![
        ConfirmationOption {
            id: "approve".to_owned(),
            label: "Yes".to_owned(),
            kind: ConfirmationOptionKind::Approve,
            group: None,
        },
        ConfirmationOption {
            id: "deny".to_owned(),
            label: "No".to_owned(),
            kind: ConfirmationOptionKind::Deny,
            group: None,
        },
    ]
}

/// A click on a diff header's OPEN, peeled off the rows road the way
/// `peeled_wake` peels a wake — clicks arrive focus-wrapped at both
/// the rows list and the turn's cells.
fn peeled_open_file(command: &RowsCommand) -> Option<String> {
    fn peel_turn(command: &TurnCommand) -> Option<String> {
        match command {
            ListCommand::Child(_, CellCommand::OpenFile(uri)) => Some(uri.clone()),
            ListCommand::Focus(_, Some(inner)) => peel_turn(inner),
            _ => None,
        }
    }
    fn peel(command: &ListCommand<RowCommand>) -> Option<String> {
        match command {
            ListCommand::Child(_, RowCommand::Turn(inner)) => peel_turn(inner),
            ListCommand::Focus(_, Some(inner)) => peel(inner),
            _ => None,
        }
    }
    match command {
        ScrollCommand::Content(command) => peel(command),
        _ => None,
    }
}

/// The shell's open-working-copy road, installed at boot — the
/// card resolves nothing window-side.
#[derive(Clone)]
pub struct OpenEditedRoad(
    pub  std::sync::Arc<
        dyn Fn(&mut Store, ahp_wire::client::HostId, ahp_wire::client::SessionUri, String)
            + Send
            + Sync,
    >,
);

impl OpenEditedRoad {
    pub fn install(store: &mut Store, road: OpenEditedRoad) {
        store.put(road);
    }
}

fn peeled_wake(command: &RowsCommand) -> Option<usize> {
    fn peel(command: &ListCommand<RowCommand>) -> Option<usize> {
        match command {
            ListCommand::Child(index, RowCommand::Wake) => Some(*index),
            ListCommand::Focus(_, Some(inner)) => peel(inner),
            _ => None,
        }
    }
    match command {
        ScrollCommand::Content(command) => peel(command),
        _ => None,
    }
}

fn peeled_activation(command: &RowsCommand) -> bool {
    fn peel(command: &ListCommand<RowCommand>) -> bool {
        match command {
            ListCommand::Child(_, RowCommand::Activate) => true,
            ListCommand::Focus(_, Some(inner)) => peel(inner),
            _ => false,
        }
    }
    match command {
        ScrollCommand::Content(command) => peel(command),
        _ => false,
    }
}

pub(crate) struct ChatWidget<Inner> {
    pub(crate) panel: Inner,
    pub(crate) rows_height: f32,
    pub(crate) focus: ChatArea,
    pub(crate) boot: bool,

    pub(crate) toolbar_stale: bool,

    pub(crate) strip_origin: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub(crate) strip_top: f32,
}

const ROWS_CHILD: usize = 0;
const COMPOSER_CHILD: usize = 1;

impl<'a> Widget<'a, ChatPanelCommand>
    for ChatWidget<imba::container::RealizedContainer<'a, ChatPanelCommand>>
{
    fn size(&self) -> Size {
        self.panel.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, ChatPanelCommand>> {
        self.panel.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<ChatPanelCommand> {
        match event {
            Event::Paint { .. } => {
                self.strip_origin.store(
                    ((viewport.left.to_bits() as u64) << 32)
                        | (viewport.top + self.strip_top).to_bits() as u64,
                    Ordering::Relaxed,
                );
                let mut result = self.panel.handle_event(arena, event, viewport);
                if self.boot {
                    result = result.merge(EventResult::Command(ChatPanelCommand::Boot));
                }
                if self.toolbar_stale {
                    result = result.merge(EventResult::Command(ChatPanelCommand::ToolbarSync));
                }
                result
            }

            Event::MouseDown { point, .. } => {
                let area = if point.y < self.rows_height {
                    ChatArea::Transcript
                } else {
                    ChatArea::Composer
                };
                match self.panel.handle_event(arena, event, viewport) {
                    EventResult::Command(command) => {
                        EventResult::Command(ChatPanelCommand::Focus(area, Some(Box::new(command))))
                    }
                    _ => EventResult::Command(ChatPanelCommand::Focus(area, None)),
                }
            }

            Event::Scroll { .. } | Event::AnimationClock { .. } | Event::ThemeChanged => {
                self.panel.handle_event(arena, event, viewport)
            }

            _ => {
                let index = match self.focus {
                    ChatArea::Transcript => ROWS_CHILD,
                    ChatArea::Composer => COMPOSER_CHILD,
                };
                self.panel.route_to(index, arena, event, viewport)
            }
        }
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, ChatPanelCommand>
    where
        'a: 'w,
    {
        self.panel.layout_data(target)
    }
}

pub mod model;

#[cfg(test)]
mod panel_tests;

#[cfg(test)]
mod model_tests;
