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

use crate::higent::{
    CancelTurnEffect, DispatchChatActionEffect, FetchFileEditEffect, FetchTurnsEffect,
    PollChatActionsEffect, StartTurnEffect, TurnsPage,
};
use crate::{env, fonts::ui_text_font, EditorCommand};
use ahp_types::actions::{
    ChatPendingMessageRemovedAction, ChatToolCallConfirmedAction, StateAction,
};
use ahp_types::state::{
    ChatState, ConfirmationOption, ConfirmationOptionKind, PendingMessageKind,
    ToolCallConfirmationReason, ToolInput,
};
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::{AnyEffect, CancellationToken, Effects},
    event::{Event, EventResult, Key},
    leaf::leaf,
    list::{ListCommand, ListSlice, ListView},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    Layout as _, LayoutExt as _, UiCtx, View, Widget,
};
use skia_safe::{Paint, Rect, Size};

use crate::higent::cell::{Cell, CellCommand, CellKind};
use crate::higent::composer::{Composer, ComposerCommand, ComposerProps};
use crate::higent::stack::{PermissionAsk, StackCommand, WidgetStack};
use crate::higent::tool_group::{ToolCallSpec, ToolFace, ToolUpdate};
use crate::higent::turn::{
    completed_tool_face, denied_tool_face, message_cell, part_cells, pending_tool_face,
    result_diff_specs, running_tool_face, streaming_tool_face, turn_cells, usage_cell, CellSpec,
    TurnCommand, TurnView,
};

pub enum RowCommand {
    Activate,

    Append { cell: Cell, height: f32 },
    Turn(TurnCommand),
}

#[derive(Clone)]
enum ChatRow {
    Loader { armed: bool },
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
            ChatRow::Loader { .. } => imba::focus::FocusData::default(),
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
            (ChatRow::Turn(turn), RowCommand::Append { cell, height }) => {
                turn.append(cell, height);
            }

            _ => {}
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
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
                ChatRow::Turn(turn) => Either::Turn(
                    imba::Layout::layout(turn.display(arena, store, ui), arena, constraints)
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

enum Either<A, B> {
    Loader(A),
    Turn(B),
}

impl<'a, A, B> imba::Thunk<'a, RowCommand> for Either<A, B>
where
    A: imba::Thunk<'a, RowCommand> + 'a,
    B: imba::Thunk<'a, RowCommand> + 'a,
{
    fn size(&self) -> Size {
        match self {
            Either::Loader(thunk) => thunk.size(),
            Either::Turn(thunk) => thunk.size(),
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
        }
    }
}

impl<'a, A: Widget<'a, RowCommand>, B: Widget<'a, RowCommand>> Widget<'a, RowCommand>
    for Either<A, B>
{
    fn size(&self) -> Size {
        match self {
            Either::Loader(widget) => widget.size(),
            Either::Turn(widget) => widget.size(),
        }
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, RowCommand>> {
        match self {
            Either::Loader(widget) => widget.overlays(),
            Either::Turn(widget) => widget.overlays(),
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

type Transcript = ListView<ChatRow, String>;
type Rows = ScrollView<Transcript>;
type RowsCommand = ScrollCommand<ListCommand<RowCommand>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChatArea {
    Transcript,
    Composer,
}

pub enum ChatPanelCommand {
    /// A command addressed to ONE view record — how an effect armed
    /// while rolling a view finds its way back to that view when it
    /// lands chat-scoped (no pane in sight).
    InView(ChatViewId, Box<ChatPanelCommand>),

    Rows(RowsCommand),

    Focus(ChatArea, Option<Box<ChatPanelCommand>>),

    Cell {
        turn: String,
        cell: usize,
        command: CellCommand,
    },
    Composer(ComposerCommand),
    Stack(StackCommand),

    Boot,
    Snapshot(Result<ChatState, String>),
    Older(Result<TurnsPage, String>),

    Accepted {
        placeholder: String,
        result: Result<(), String>,
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

impl ChatPanelCommand {
    pub fn is_send(&self) -> bool {
        match self {
            Self::Send => true,
            Self::Focus(_, Some(inner)) => inner.is_send(),
            Self::InView(_, inner) => inner.is_send(),
            _ => false,
        }
    }
}

#[derive(Clone)]
struct ActiveStream {
    turn: crate::higent::TurnId,
    cells: usize,
    parts: rpds::HashTrieMapSync<String, usize>,
    tools: rpds::HashTrieMapSync<String, ToolTrack>,

    group: Option<usize>,
}

#[derive(Clone)]
struct ToolTrack {
    cell: usize,
    display_name: String,
    invocation: String,

    call: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChatViewId(u64);

impl ChatViewId {
    pub(crate) fn mint() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// One turn of the MODEL transcript: the data a view lays, kept
/// current by the stream fold so a fresh view can be built at any
/// moment without replaying the wire.
#[derive(Clone)]
struct TurnRecord {
    id: String,
    cells: Vec<CellSpec>,
}

/// One model mutation, as every view consumes it — each view lays
/// the same op at its own width.
enum ViewOp {
    Reset {
        turns: Vec<TurnRecord>,
        has_more: bool,
    },
    Prepend {
        turns: Vec<TurnRecord>,
        has_more: bool,
    },
    Loader {
        armed: bool,
    },
    SpliceTurn {
        replace: Option<String>,
        key: String,
        cells: Vec<CellSpec>,
    },
    AppendCell {
        turn: String,
        index: usize,
        spec: CellSpec,
    },
    Cell {
        turn: String,
        cell: usize,
        command: CellUpdate,
    },
}

/// The model-emitted cell mutations (the view-only cell commands —
/// editor scrolls, resolved diffs — never ride an op).
#[derive(Clone)]
enum CellUpdate {
    Append(String),
    Tool(ToolUpdate),
}

/// The conversation MODEL: session truth, owner of its views.
pub struct ChatPanel {
    server: crate::higent::HostId,

    session: crate::higent::SessionUri,
    chat: crate::higent::ChatUri,
    state: Link,
    title: String,

    /// The transcript as SPECS — what new ListViews are built from.
    turns: Vec<TurnRecord>,

    stack: WidgetStack,

    cursor: Option<String>,

    fetch_token: Option<CancellationToken>,

    poll_token: Option<CancellationToken>,

    active: Option<ActiveStream>,

    pending: Option<(String, String)>,

    /// A STEERED prompt: sent while a turn ran, so the run was
    /// cancelled and this fires the moment the turn ends — the
    /// Claude Code Esc-with-prompt shape. An explicit STOP drops it.
    steering: Option<String>,

    initial_prompt: Option<String>,

    minted: u64,

    views: rpds::HashTrieMapSync<ChatViewId, ChatView>,
}

impl Clone for ChatPanel {
    fn clone(&self) -> Self {
        Self {
            server: self.server,
            session: self.session.clone(),
            chat: self.chat.clone(),
            state: self.state.clone(),
            title: self.title.clone(),
            turns: self.turns.clone(),
            stack: self.stack.clone(),
            cursor: self.cursor.clone(),
            fetch_token: self.fetch_token,
            poll_token: self.poll_token,
            active: self.active.clone(),
            pending: self.pending.clone(),
            steering: self.steering.clone(),
            initial_prompt: self.initial_prompt.clone(),
            minted: self.minted,
            views: self.views.clone(),
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

fn completion_editor(command: ::editor::EditorCommand) -> ChatPanelCommand {
    ChatPanelCommand::Composer(ComposerCommand::Editor(
        imba::scroll::ScrollCommand::Content(command),
    ))
}

impl ChatPanel {
    pub fn new(
        _store: &imba::store::Store,
        _ui: &UiCtx,
        server: crate::higent::HostId,
        session: impl Into<crate::higent::SessionUri>,
        chat: impl Into<crate::higent::ChatUri>,
    ) -> Self {
        Self {
            server,
            session: session.into(),
            chat: chat.into(),
            state: Link::Idle,
            title: "Agent Chat".to_owned(),
            turns: Vec::new(),
            stack: WidgetStack::new(),
            cursor: None,
            fetch_token: None,
            poll_token: None,
            active: None,
            pending: None,
            steering: None,
            initial_prompt: None,
            minted: 0,
            views: rpds::HashTrieMapSync::new_sync(),
        }
    }

    pub fn with_initial_prompt(mut self, prompt: String) -> Self {
        self.initial_prompt = Some(prompt);
        self
    }

    pub(crate) fn server(&self) -> crate::higent::HostId {
        self.server
    }

    pub(crate) fn session_id(&self) -> crate::SessionId {
        crate::SessionId {
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

    pub fn transcript(&self) -> Vec<(String, Vec<(String, String)>)> {
        let Some(view) = self.first_view() else {
            return Vec::new();
        };
        view.rows
            .content()
            .rows()
            .map(|row| match row {
                ChatRow::Loader { .. } => ("…".to_owned(), Vec::new()),
                ChatRow::Turn(turn) => (turn.id().to_owned(), turn.cells_oracle()),
            })
            .collect()
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

    pub fn footer_height(&self, store: &Store, nominal_height: f32) -> f32 {
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let band = self
            .first_view()
            .map(|view| view.composer.band_height(&chrome, nominal_height))
            .unwrap_or(0.0);
        band + self.stack.height(&chrome) + theme.ui().toolbar.height
    }

    pub fn ready(&self) -> bool {
        matches!(self.state, Link::Ready)
    }

    pub fn permission_oracle(&self) -> Option<(String, String, Option<String>, Vec<String>)> {
        self.stack.permission_oracle()
    }

    pub fn composer_content_height(&self) -> f32 {
        self.first_view()
            .map(|view| view.composer.content_height())
            .unwrap_or(0.0)
    }

    pub fn queue_oracle(&self) -> Vec<(String, String)> {
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
        self.pending.is_some() || self.active.is_some()
    }

    fn seat(&self, store: &Store) -> Option<std::sync::Arc<dyn crate::higent::AhpServer>> {
        crate::higent::Servers::seat(store, self.server)
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
        let seat = self.seat(store);
        let mut view = ChatView::new(store, ui);
        fx.scope(
            move |command| ChatPanelCommand::InView(id, Box::new(command)),
            |fx| {
                let lead_loader = self.cursor.is_some();
                let slice = view.build_page(store, ui, &seat, &self.turns, lead_loader, fx);
                let len = view.rows.content().len();
                view.rows.content_mut().splice_slice(0..len, slice);
                view.has_loader = lead_loader;
            },
        );
        view.reveal_tail(store);
        self.views.insert_mut(id, view);
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
        let seat = self.seat(store);
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
                        view.apply(store, ui, &seat, op, fx);
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

    fn record_mut(&mut self, turn: &str) -> Option<&mut TurnRecord> {
        self.turns.iter_mut().find(|record| record.id == turn)
    }

    fn load_older(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        if self.fetch_token.is_some() {
            return;
        }
        let Some(cursor) = self.cursor.clone() else {
            return;
        };
        let Some(seat) = self.seat(store) else {
            return;
        };
        self.roll(store, ui, &[ViewOp::Loader { armed: false }], fx);
        self.fetch_token = Some(
            fx.push(
                AnyEffect::new(FetchTurnsEffect {
                    seat,
                    chat: self.chat.clone(),
                    cursor: Some(cursor),
                })
                .map(ChatPanelCommand::Older),
            ),
        );
    }

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
                self.cursor = page.next_cursor.clone();
                let records: Vec<TurnRecord> = page
                    .turns
                    .iter()
                    .map(|turn| TurnRecord {
                        id: turn.id.clone(),
                        cells: turn_cells(turn),
                    })
                    .collect();
                self.turns.splice(0..0, records.iter().cloned());
                self.roll(
                    store,
                    ui,
                    &[ViewOp::Prepend {
                        turns: records,
                        has_more: self.cursor.is_some(),
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

    fn apply_snapshot(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        result: Result<ChatState, String>,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        match result {
            Ok(state) => {
                self.state = Link::Ready;
                self.title = state.title.clone();
                self.cursor = state.turns_next_cursor.clone();

                if let Some(turn) = state
                    .turns
                    .iter()
                    .rev()
                    .find(|turn| turn.state == ahp_types::state::TurnState::Complete)
                {
                    crate::higent::session::Agents::note_turn(
                        store,
                        self.server,
                        &self.chat,
                        &turn.id,
                    );
                }
                self.stack
                    .seed_queue(state.queued_messages.iter().flatten().cloned());
                self.turns = state
                    .turns
                    .iter()
                    .map(|turn| TurnRecord {
                        id: turn.id.clone(),
                        cells: turn_cells(turn),
                    })
                    .collect();
                self.roll(
                    store,
                    ui,
                    &[ViewOp::Reset {
                        turns: self.turns.clone(),
                        has_more: self.cursor.is_some(),
                    }],
                    fx,
                );

                self.mark_read(store, fx);
                self.relaunch_poll(store, fx);

                if let Some(prompt) = self.initial_prompt.take() {
                    let _ = self.send_text(store, ui, prompt, None, None, fx);
                }
            }
            Err(error) => {
                self.state = Link::Failed(error);
            }
        }
    }

    fn model_dispatch(
        &self,
        store: &Store,
        action: StateAction,
        undo_queue: Option<String>,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let Some(seat) = self.seat(store) else {
            return;
        };
        fx.push(
            AnyEffect::new(DispatchChatActionEffect {
                seat,
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
        let Some(seat) = self.seat(store) else {
            return;
        };
        fx.push(
            AnyEffect::new(DispatchChatActionEffect {
                seat,
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

    fn stop(&mut self, store: &Store, fx: &mut Effects<'_, ChatPanelCommand>) {
        let turn = self
            .active
            .as_ref()
            .map(|active| active.turn.clone())
            .or_else(|| self.stack.ask_turn());
        let Some(turn_id) = turn else {
            return;
        };
        let Some(seat) = self.seat(store) else {
            return;
        };
        fx.notify(CancelTurnEffect {
            seat,
            chat: self.chat.clone(),
            turn_id,
        });
    }

    fn relaunch_poll(&mut self, store: &Store, fx: &mut Effects<'_, ChatPanelCommand>) {
        if !matches!(self.state, Link::Ready) {
            return;
        }
        let Some(seat) = self.seat(store) else {
            return;
        };
        if let Some(token) = self.poll_token.take() {
            fx.cancel(token);
        }
        self.poll_token = Some(
            fx.push(
                AnyEffect::new(PollChatActionsEffect {
                    seat,
                    chat: self.chat.clone(),
                })
                .map(ChatPanelCommand::Actions),
            ),
        );
    }

    /// Fold-prim: a streamed cell joins the active turn — the model
    /// spec grows and the op carries it to every view.
    fn append_stream_spec(
        &mut self,
        turn_id: &str,
        spec: CellSpec,
        ops: &mut Vec<ViewOp>,
    ) -> Option<usize> {
        let active = self.active.clone()?;
        if active.turn.as_str() != turn_id {
            return None;
        }

        if let (CellSpec::Tools(specs), Some(group)) = (&spec, active.group) {
            if let Some(record) = self.record_mut(turn_id) {
                if let Some(CellSpec::Tools(held)) = record.cells.get_mut(group) {
                    held.extend(specs.iter().cloned());
                }
            }
            for spec in specs.iter().cloned() {
                ops.push(ViewOp::Cell {
                    turn: turn_id.to_owned(),
                    cell: group,
                    command: CellUpdate::Tool(ToolUpdate::Add(spec)),
                });
            }
            return Some(group);
        }
        let index = active.cells;
        let opens_run = matches!(spec, CellSpec::Tools(_));
        let Some(record) = self.record_mut(turn_id) else {
            return None;
        };
        record.cells.push(spec.clone());
        ops.push(ViewOp::AppendCell {
            turn: turn_id.to_owned(),
            index,
            spec,
        });
        if let Some(active) = &mut self.active {
            active.cells += 1;
            active.group = opens_run.then_some(index);
        }
        Some(index)
    }

    /// Fold-prim: streamed text joins its cell's spec and every
    /// view's laid cell.
    fn append_delta(
        &mut self,
        turn_id: &str,
        part_id: &str,
        content: String,
        ops: &mut Vec<ViewOp>,
    ) {
        let Some(active) = &self.active else {
            return;
        };
        if active.turn.as_str() != turn_id {
            return;
        }
        let Some(cell) = active.parts.get(part_id).copied() else {
            return;
        };
        if let Some(record) = self.record_mut(turn_id) {
            if let Some(CellSpec::Text(_, text)) = record.cells.get_mut(cell) {
                text.push_str(&content);
            }
        }
        ops.push(ViewOp::Cell {
            turn: turn_id.to_owned(),
            cell,
            command: CellUpdate::Append(content),
        });
    }

    /// Fold-prim: a tool call's face moved.
    fn update_tool_call(
        &mut self,
        turn_id: &str,
        tool_call_id: &str,
        face: ToolFace,
        ops: &mut Vec<ViewOp>,
    ) {
        let Some(track) = self
            .active
            .as_ref()
            .filter(|active| active.turn.as_str() == turn_id)
            .and_then(|active| active.tools.get(tool_call_id))
        else {
            return;
        };
        let cell = track.cell;
        if let Some(record) = self.record_mut(turn_id) {
            if let Some(CellSpec::Tools(held)) = record.cells.get_mut(cell) {
                if let Some(spec) = held.iter_mut().find(|spec| spec.id == tool_call_id) {
                    spec.face = face.clone();
                }
            }
        }
        ops.push(ViewOp::Cell {
            turn: turn_id.to_owned(),
            cell,
            command: CellUpdate::Tool(ToolUpdate::Face {
                id: tool_call_id.to_owned(),
                face,
            }),
        });
    }

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
            match action {
                StateAction::ChatTurnStarted(action) => {
                    let (kind, text) = message_cell(&action.message);
                    let cells = vec![CellSpec::Text(kind, text)];
                    let replace = self.pending.take().map(|(key, _)| key);
                    if let Some(key) = &replace {
                        self.turns.retain(|record| &record.id != key);
                    }
                    self.turns.push(TurnRecord {
                        id: action.turn_id.clone(),
                        cells: cells.clone(),
                    });
                    ops.push(ViewOp::SpliceTurn {
                        replace,
                        key: action.turn_id.clone(),
                        cells,
                    });
                    self.active = Some(ActiveStream {
                        turn: crate::higent::TurnId::new(action.turn_id),
                        cells: 1,
                        parts: rpds::HashTrieMapSync::new_sync(),
                        tools: rpds::HashTrieMapSync::new_sync(),
                        group: None,
                    });
                }
                StateAction::ChatResponsePart(action) => {
                    let first = self.active.as_ref().map(|active| active.cells);
                    let specs = part_cells(&action.part);
                    let minted = !specs.is_empty();
                    for spec in specs {
                        self.append_stream_spec(&action.turn_id, spec, &mut ops);
                    }
                    use ahp_types::state::ResponsePart;
                    let part_id = match &action.part {
                        ResponsePart::Markdown(part) => Some(part.id.clone()),
                        ResponsePart::Reasoning(part) => Some(part.id.clone()),
                        _ => None,
                    };
                    if let (true, Some(id), Some(first), Some(active)) =
                        (minted, part_id, first, self.active.as_mut())
                    {
                        if active.turn.as_str() == action.turn_id {
                            active.parts.insert_mut(id, first);
                        }
                    }
                }
                StateAction::ChatDelta(action) => {
                    self.append_delta(&action.turn_id, &action.part_id, action.content, &mut ops);
                }
                StateAction::ChatReasoning(action) => {
                    self.append_delta(&action.turn_id, &action.part_id, action.content, &mut ops);
                }
                StateAction::ChatToolCallStart(action) => {
                    let index = self.append_stream_spec(
                        &action.turn_id,
                        CellSpec::Tools(vec![ToolCallSpec {
                            id: action.tool_call_id.clone(),
                            display_name: action.display_name.clone(),
                            face: streaming_tool_face(&action.display_name),
                        }]),
                        &mut ops,
                    );
                    if let (Some(index), Some(active)) = (index, self.active.as_mut()) {
                        if active.turn.as_str() == action.turn_id {
                            active.tools.insert_mut(
                                action.tool_call_id,
                                ToolTrack {
                                    cell: index,
                                    display_name: action.display_name,
                                    invocation: String::new(),
                                    call: None,
                                },
                            );
                        }
                    }
                }
                StateAction::ChatToolCallReady(action) => {
                    let invocation = action.invocation_message.as_text().to_owned();
                    let call = match &action.tool_input {
                        Some(ToolInput::Inline(text)) => Some(text.clone()),
                        _ => None,
                    };
                    let display = self
                        .active
                        .as_mut()
                        .filter(|active| active.turn.as_str() == action.turn_id)
                        .and_then(|active| {
                            let mut track = active.tools.get(&action.tool_call_id)?.clone();
                            track.invocation = invocation.clone();
                            track.call = call;
                            let display = track.display_name.clone();
                            active.tools.insert_mut(action.tool_call_id.clone(), track);
                            Some(display)
                        });
                    let Some(display) = display else {
                        continue;
                    };
                    if action.confirmed.is_some() {
                        self.update_tool_call(
                            &action.turn_id,
                            &action.tool_call_id,
                            running_tool_face(&display, &invocation),
                            &mut ops,
                        );
                    } else {
                        self.update_tool_call(
                            &action.turn_id,
                            &action.tool_call_id,
                            pending_tool_face(&display, &invocation),
                            &mut ops,
                        );
                        let input = match &action.tool_input {
                            Some(ToolInput::Inline(text)) => Some(text.clone()),
                            _ => None,
                        };
                        self.stack.set_ask(PermissionAsk::new(
                            crate::higent::TurnId::new(action.turn_id.clone()),
                            action.tool_call_id.clone(),
                            action
                                .confirmation_title
                                .as_ref()
                                .map(|title| title.as_text().to_owned())
                                .unwrap_or_else(|| display.clone()),
                            invocation,
                            input,
                            action
                                .options
                                .clone()
                                .unwrap_or_else(default_confirmation_options),
                        ));
                    }
                }
                StateAction::ChatToolCallConfirmed(action) => {
                    self.stack.clear_ask_for_tool(&action.tool_call_id);
                    let track = self
                        .active
                        .as_ref()
                        .filter(|active| active.turn.as_str() == action.turn_id)
                        .and_then(|active| active.tools.get(&action.tool_call_id).cloned());
                    if let Some(track) = track {
                        let face = if action.approved {
                            running_tool_face(&track.display_name, &track.invocation)
                        } else {
                            denied_tool_face(&track.display_name)
                        };
                        self.update_tool_call(
                            &action.turn_id,
                            &action.tool_call_id,
                            face,
                            &mut ops,
                        );
                    }
                }
                StateAction::ChatToolCallComplete(action) => {
                    let track = self
                        .active
                        .as_ref()
                        .filter(|active| active.turn.as_str() == action.turn_id)
                        .and_then(|active| active.tools.get(&action.tool_call_id).cloned());
                    let Some(track) = track else {
                        continue;
                    };
                    let face = completed_tool_face(
                        &track.display_name,
                        track.call.as_deref(),
                        &action.result,
                    );
                    self.update_tool_call(&action.turn_id, &action.tool_call_id, face, &mut ops);

                    for spec in result_diff_specs(&action.result) {
                        self.append_stream_spec(&action.turn_id, spec, &mut ops);
                    }
                }
                StateAction::ChatPendingMessageSet(action)
                    if matches!(action.kind, PendingMessageKind::Queued) =>
                {
                    self.stack.insert_queued(action.id, action.message);
                }
                StateAction::ChatPendingMessageRemoved(action) => {
                    self.stack.remove_queued(&action.id);
                }
                StateAction::ChatUsage(action) => {
                    if let Some((kind, markdown)) = usage_cell(&action.usage) {
                        self.append_stream_spec(
                            &action.turn_id,
                            CellSpec::Text(kind, markdown),
                            &mut ops,
                        );
                    }
                }
                StateAction::ChatTurnComplete(action) => {
                    if self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.turn.as_str() == action.turn_id)
                    {
                        self.active = None;
                        self.stack.clear_ask();
                        steer = self.steering.take().or(steer);
                    }

                    crate::higent::session::Agents::note_turn(
                        store,
                        self.server,
                        &self.chat,
                        &action.turn_id,
                    );
                    self.mark_read(store, fx);
                }
                StateAction::ChatTurnCancelled(action) => {
                    self.append_stream_spec(
                        &action.turn_id,
                        CellSpec::Text(CellKind::Notice, "*Turn cancelled.*".to_owned()),
                        &mut ops,
                    );
                    self.active = None;
                    self.stack.clear_ask();
                    steer = self.steering.take().or(steer);
                    self.mark_read(store, fx);
                }
                StateAction::ChatError(action) => {
                    let markdown = format!(
                        "**Turn failed** ({}): {}",
                        action.error.error_type, action.error.message
                    );
                    self.append_stream_spec(
                        &action.turn_id,
                        CellSpec::Text(CellKind::Error, markdown),
                        &mut ops,
                    );
                    self.active = None;
                    self.stack.clear_ask();
                    steer = self.steering.take().or(steer);
                    self.mark_read(store, fx);
                }

                _ => {}
            }
        }
        self.roll(store, ui, &ops, fx);
        if let Some(text) = steer {
            let _ = self.send_text(store, ui, text, None, None, fx);
        }
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
        let Some(seat) = self.seat(store) else {
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
        self.minted += 1;
        let key = format!("local-{}", self.minted);
        let cells = vec![
            CellSpec::Text(CellKind::User, text.clone()),
            CellSpec::Text(CellKind::Notice, "*Thinking…*".to_owned()),
        ];
        self.turns.push(TurnRecord {
            id: key.clone(),
            cells: cells.clone(),
        });
        self.pending = Some((key.clone(), text.clone()));
        self.roll(
            store,
            ui,
            &[ViewOp::SpliceTurn {
                replace: None,
                key: key.clone(),
                cells,
            }],
            fx,
        );
        let placeholder = key;
        fx.push(
            AnyEffect::new(StartTurnEffect {
                seat,
                chat: self.chat.clone(),
                text,
                attachments,
                model,
            })
            .map(move |result| ChatPanelCommand::Accepted {
                placeholder: placeholder.clone(),
                result,
            }),
        );
        true
    }

    fn apply_send_failed(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        placeholder: String,
        error: String,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let sent = match &self.pending {
            Some((key, text)) if *key == placeholder => text.clone(),
            _ => String::new(),
        };
        self.pending = None;
        let cells = vec![
            CellSpec::Text(CellKind::User, sent),
            CellSpec::Text(
                CellKind::Error,
                format!("**The message was not delivered:** {error}"),
            ),
        ];
        let Some(record) = self.record_mut(&placeholder) else {
            return;
        };
        record.cells = cells.clone();
        self.roll(
            store,
            ui,
            &[ViewOp::SpliceTurn {
                replace: Some(placeholder.clone()),
                key: placeholder,
                cells,
            }],
            fx,
        );
    }

    // ------------------------------------------------------------------
    // The two roads in: chat-scoped landings (model) and pane
    // commands (a view id in hand).

    /// The landing road: feed results and other chat-scoped commands.
    /// A view-addressed command (`InView`) finds its record here.
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
                if matches!(self.state, Link::Idle) {
                    self.state = Link::Subscribing;
                    crate::AppRequests::push(
                        store,
                        std::sync::Arc::new(crate::higent::chats::EnsureChatFeed {
                            chat: self.chat.clone(),
                        }),
                    );
                }
            }
            ChatPanelCommand::Snapshot(result) => self.apply_snapshot(store, ui, result, fx),
            ChatPanelCommand::Older(result) => self.apply_older(store, ui, result, fx),
            ChatPanelCommand::Accepted {
                placeholder,
                result,
            } => {
                if let Err(error) = result {
                    self.apply_send_failed(store, ui, placeholder, error, fx);
                }
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
    pub(crate) fn perform_in_view(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        id: ChatViewId,
        command: ChatPanelCommand,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        self.ensure_view(store, ui, id, fx);
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
                    let editor = view.composer.editor();
                    let _ = editor;
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
                let attachments = view.completion_attachments(store, self.server, &text);
                let model = view.toolbar.model_selection();
                let consumed = self.send_text(store, ui, text, attachments, model, fx);
                if consumed {
                    view.composer.clear(store, ui);
                    view.reveal_tail(store);
                }
                self.views.insert_mut(id, view);
            }

            ChatPanelCommand::Composer(command) => {
                let Some(mut view) = self.views.get(&id).cloned() else {
                    return;
                };
                let session = self.session_id();
                fx.scope(
                    move |command| ChatPanelCommand::InView(id, Box::new(command)),
                    |fx| view.composer_command(store, ui, &session, command, fx),
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
                if let Some(channel) = super::Agents::channel(store, &self.session_id()) {
                    view.toolbar.sync(store, ui, self.server, &channel);
                }
                self.views.insert_mut(id, view);
            }

            ChatPanelCommand::Toolbar(command) => {
                let Some(mut view) = self.views.get(&id).cloned() else {
                    return;
                };
                let ask = {
                    let mut ask = super::ToolbarAsk::None;
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
                    super::ToolbarAsk::Edits(mode) => {
                        if let Some(seat) = self.seat(store) {
                            let mut config = serde_json::Map::new();
                            config.insert("permissionMode".to_owned(), serde_json::json!(mode));
                            fx.push(
                                AnyEffect::new(DispatchChatActionEffect {
                                    seat,
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

                    super::ToolbarAsk::AddFolder => {
                        crate::AppRequests::push(
                            store,
                            std::sync::Arc::new(super::AddSessionFolders {
                                server: self.server,
                                session: self.session.clone(),
                            }),
                        );
                    }
                    super::ToolbarAsk::None => {}
                }
            }

            model => self.perform_model(store, ui, model, fx),
        }
    }

    pub(crate) fn focus_data_view<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::UiCtx,
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
    ) -> Option<impl imba::Layout<'a, ChatPanelCommand> + imba::LayoutValue + 'a> {
        let view = self.views.get(&id)?;
        Some(imba::laid(
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
                    imba::Layout::layout(
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
                    Link::Ready if self.pending.is_some() => "thinking…".to_owned(),
                    Link::Ready if self.active.is_some() => "responding…".to_owned(),
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

                let toolbar_cells_right = view.toolbar.place(
                    arena,
                    &mut panel,
                    store,
                    ui,
                    size.height - toolbar_h + 1.0,
                    toolbar_h - 1.0,
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
                    let caps_font = crate::fonts::ui_font(ui, ui_theme.combo.label_size * 1.1);
                    let key_font = crate::fonts::ui_text_font(ui, ui_theme.peeker.hint_size * 0.95);
                    let pad = ui_theme.combo.pad;
                    let cell_width = label
                        .chars()
                        .map(|ch| caps_font.measure_str(ch.to_string(), None).0 + 1.5)
                        .sum::<f32>()
                        + imba::text_advance(ui, &key_font, "⌘⏎")
                        + ui_theme.combo.gap
                        + pad * 2.0;
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
                    let mut row = imba::Row::new(arena).gap(gap).child(
                        imba::text(ui, label, caps_font.clone(), on_accent)
                            .tracking(1.5)
                            .pad_insets(imba::Insets {
                                left: 0.0,
                                top: (mid + caps_font.size() * 0.35 - caps_ascent).max(0.0),
                                right: 0.0,
                                bottom: 0.0,
                            }),
                    );
                    if !stop {
                        row = row.child(
                            imba::text(ui, "⌘⏎", key_font.clone(), accent_soft).pad_insets(
                                imba::Insets {
                                    left: 0.0,
                                    top: (mid + key_font.size() * 0.35 - key_ascent).max(0.0),
                                    right: 0.0,
                                    bottom: 0.0,
                                },
                            ),
                        );
                    }
                    let cell = row
                        .pad_insets(imba::Insets {
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
                    let legend_w = imba::text_advance(ui, &key_font, &legend);
                    let legend_x = size.width - cell_width - gap - legend_w;
                    // Squeezed out by the combo cells? The legend yields.
                    if legend_x >= toolbar_cells_right + gap {
                        panel.place_boxed(
                            legend_x,
                            size.height - toolbar_h + 1.0,
                            imba::text(ui, legend, key_font.clone(), accent_soft)
                                .pad_insets(imba::Insets {
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
                let toolbar_stale =
                    super::Agents::channel(store, &self.session_id()).is_some_and(|channel| {
                        super::SessionToolbar::fingerprint(store, self.server, &channel)
                            != view.toolbar.synced
                    });

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
        seat: &Option<std::sync::Arc<dyn crate::higent::AhpServer>>,
        key: &str,
        index: usize,
        spec: CellSpec,
        content_width: f32,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> (Cell, f32) {
        let turn_key = key.to_owned();
        match spec {
            CellSpec::Text(kind, markdown) => fx.scope(
                move |command: EditorCommand| ChatPanelCommand::Cell {
                    turn: turn_key.clone(),
                    cell: index,
                    command: CellCommand::Editor(command),
                },
                |fx| Cell::build(store, ui, kind, &markdown, content_width, fx),
            ),
            CellSpec::Tools(specs) => Cell::tools(store, ui, specs, content_width),
            CellSpec::Diff(spec) => {
                let Some(seat) = seat.clone() else {
                    return Cell::pending_diff(store, spec.header, content_width);
                };
                fx.push(
                    AnyEffect::new(FetchFileEditEffect {
                        seat,
                        before: spec.before,
                        after: spec.after,
                    })
                    .map(move |result| ChatPanelCommand::Cell {
                        turn: turn_key.clone(),
                        cell: index,
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
        seat: &Option<std::sync::Arc<dyn crate::higent::AhpServer>>,
        key: &str,
        cells: Vec<CellSpec>,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> (TurnView, f32) {
        let content_width = TurnView::content_width(self.panel_width());
        let mut rows = Vec::with_capacity(cells.len());
        let mut total = 0.0;
        for (index, spec) in cells.into_iter().enumerate() {
            let (cell, height) =
                self.build_cell(store, ui, seat, key, index, spec, content_width, fx);
            total += height;
            rows.push((cell, height));
        }
        (TurnView::new(key, content_width, rows), total)
    }

    fn build_page(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        seat: &Option<std::sync::Arc<dyn crate::higent::AhpServer>>,
        turns: &[TurnRecord],
        lead_loader: bool,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> ListSlice<ChatRow, String> {
        let chrome = env::Themes::of(store).ui().chat.clone();
        let mut slice = ListSlice::new();
        if lead_loader {
            slice.push_sized(ChatRow::Loader { armed: true }, chrome.loader_height);
        }
        for record in turns {
            let (view, height) =
                self.build_turn(store, ui, seat, &record.id, record.cells.clone(), fx);
            slice.push_keyed_sized(record.id.clone(), ChatRow::Turn(view), height);
        }
        slice
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

    fn route_cell(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        turn: String,
        cell: usize,
        command: CellCommand,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let Some(range) = self.rows.content().row_range(&turn) else {
            return;
        };
        let index = range.start;
        let key = turn.clone();
        fx.scope(
            move |command: RowsCommand| lift_rows_command(&key, command),
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
        session: &crate::SessionId,
        command: ComposerCommand,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let Some(command) = self.completion_intercept(store, ui, command, fx) else {
            return;
        };
        let typed_at = matches!(
            &command,
            ComposerCommand::Editor(imba::scroll::ScrollCommand::Content(
                ::editor::EditorCommand::InsertText { text }
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
            session,
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
        let ComposerCommand::Editor(ScrollCommand::Content(::editor::EditorCommand::Inlay {
            key,
            command: inlay,
        })) = command
        else {
            return Some(command);
        };
        if Some(key) != self.completion.inlay_key() {
            return Some(ComposerCommand::Editor(ScrollCommand::Content(
                ::editor::EditorCommand::Inlay {
                    key,
                    command: inlay,
                },
            )));
        }
        let popup = match inlay.downcast::<crate::completion::CompletionCommand>() {
            Ok(popup) => *popup,
            Err(other) => {
                return Some(ComposerCommand::Editor(ScrollCommand::Content(
                    ::editor::EditorCommand::Inlay {
                        key,
                        command: other,
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
        store: &Store,
        server: crate::higent::HostId,
        text: &str,
    ) -> Option<Vec<ahp_types::state::MessageAttachment>> {
        let uris = super::Hosts::uris(store, server)?;
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
        seat: &Option<std::sync::Arc<dyn crate::higent::AhpServer>>,
        op: &ViewOp,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        match op {
            ViewOp::Reset { turns, has_more } => {
                let slice = self.build_page(store, ui, seat, turns, *has_more, fx);
                let len = self.rows.content().len();
                self.rows.content_mut().splice_slice(0..len, slice);
                self.has_loader = *has_more;
            }
            ViewOp::Prepend { turns, has_more } => {
                let slice = self.build_page(store, ui, seat, turns, *has_more, fx);
                let end = usize::from(self.has_loader);
                self.rows.content_mut().splice_slice(0..end, slice);
                self.has_loader = *has_more;
            }
            ViewOp::Loader { armed } => self.set_loader(store, *armed),
            ViewOp::SpliceTurn {
                replace,
                key,
                cells,
            } => {
                let (view, height) = self.build_turn(store, ui, seat, key, cells.clone(), fx);
                let mut slice = ListSlice::new();
                slice.push_keyed_sized(key.clone(), ChatRow::Turn(view), height);
                let range = replace
                    .as_ref()
                    .and_then(|key| self.rows.content().row_range(key));
                let len = self.rows.content().len();
                self.rows
                    .content_mut()
                    .splice_slice(range.unwrap_or(len..len), slice);
            }
            ViewOp::AppendCell { turn, index, spec } => {
                let Some(range) = self.rows.content().row_range(turn) else {
                    return;
                };
                let content_width = TurnView::content_width(self.panel_width());
                let (cell, height) = self.build_cell(
                    store,
                    ui,
                    seat,
                    turn,
                    *index,
                    spec.clone(),
                    content_width,
                    fx,
                );
                fx.scope(ChatPanelCommand::Rows, |fx| {
                    self.rows.perform(
                        store,
                        ui,
                        ScrollCommand::Content(ListCommand::Child(
                            range.start,
                            RowCommand::Append { cell, height },
                        )),
                        fx,
                    )
                });
            }
            ViewOp::Cell {
                turn,
                cell,
                command,
            } => {
                let command = match command {
                    CellUpdate::Append(content) => CellCommand::Append(content.clone()),
                    CellUpdate::Tool(update) => CellCommand::Tool(update.clone()),
                };
                self.route_cell(store, ui, turn.clone(), *cell, command, fx);
            }
        }
    }
}

fn lift_rows_command(turn: &str, command: RowsCommand) -> ChatPanelCommand {
    if let ScrollCommand::Content(ListCommand::Child(
        _,
        RowCommand::Turn(ListCommand::Child(cell, command)),
    )) = command
    {
        return ChatPanelCommand::Cell {
            turn: turn.to_owned(),
            cell,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::higent::ahp_types::actions::ChatTurnCancelledAction;
    use crate::higent::ahp_types::state::{Message, MessageKind, MessageOrigin};
    use std::sync::Arc;

    struct InertSeat;

    macro_rules! unreached {
        ($($name:ident($($arg:ident: $ty:ty),*) -> $out:ty;)*) => {
            $(fn $name(&self, $($arg: $ty),*) -> $out {
                $(let _ = $arg;)*
                unreachable!("the steering tests never reach the seat")
            })*
        };
    }

    impl crate::higent::AhpServer for InertSeat {
        unreached! {
            connect() -> crate::higent::SeatFuture<Result<crate::higent::RootInfo, String>>;
            list_sessions(cursor: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::SessionsPage, String>>;
            poll_root() -> crate::higent::SeatFuture<Vec<crate::higent::ServerEvent>>;
            create_session(dirs: Vec<String>, options: crate::higent::SessionOptions) -> crate::higent::SeatFuture<Result<crate::higent::SessionUri, String>>;
            resolve_session_config(working_directory: Option<String>, config: Option<serde_json::Map<String, serde_json::Value>>) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::commands::ResolveSessionConfigResult, String>>;
            dispose_session(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<(), String>>;
            subscribe_session(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::SessionState, String>>;
            poll_session(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Vec<StateAction>>;
            create_chat(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<crate::higent::ChatUri, String>>;
            subscribe_chat(chat: crate::higent::ChatUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::ChatState, String>>;
            fetch_turns(chat: crate::higent::ChatUri, cursor: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::TurnsPage, String>>;
            start_turn(chat: crate::higent::ChatUri, text: String, attachments: Option<Vec<crate::higent::ahp_types::state::MessageAttachment>>, model: Option<crate::higent::ahp_types::state::ModelSelection>) -> crate::higent::SeatFuture<Result<(), String>>;
            poll_chat(chat: crate::higent::ChatUri) -> crate::higent::SeatFuture<Vec<StateAction>>;
            cancel_turn(chat: crate::higent::ChatUri, turn: crate::higent::TurnId) -> crate::higent::SeatFuture<()>;
            dispatch_action(chat: crate::higent::ChannelUri, action: StateAction) -> crate::higent::SeatFuture<Result<(), String>>;
            read_file_edit(before: Option<String>, after: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::FileEditContents, String>>;
            resource_read(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri) -> crate::higent::SeatFuture<Option<String>>;
            resource_write(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri, text: String) -> crate::higent::SeatFuture<bool>;
            resource_list(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri) -> crate::higent::SeatFuture<Option<Vec<(String, bool)>>>;
            resource_watch(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri, events: Arc<dyn Fn() + Send + Sync>) -> crate::higent::SeatFuture<Option<crate::higent::WatchHandle>>;
            resource_unwatch(handle: crate::higent::WatchHandle) -> crate::higent::SeatFuture<()>;
            search(session: crate::higent::SessionUri, ask: crate::higent::SearchAsk) -> crate::higent::SeatFuture<Option<crate::higent::SearchResult>>;
            terminal_input(channel: &crate::higent::ChannelUri, data: String) -> ();
            terminal_resize(channel: &crate::higent::ChannelUri, cols: u16, rows: u16) -> ();
            terminal_dispose(channel: &crate::higent::ChannelUri) -> ();
            subscribe_changeset(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::ChangesetState, String>>;
            poll_changeset(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Vec<StateAction>>;
            unsubscribe_changeset(channel: &crate::higent::ChannelUri) -> ();
            subscribe_annotations(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::AnnotationsState, String>>;
            poll_annotations(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Vec<StateAction>>;
            dispatch_annotations(session: &crate::higent::SessionUri, action: StateAction) -> ();
            unsubscribe_annotations(session: &crate::higent::SessionUri) -> ();
            open_document(session: crate::higent::SessionUri, uri: Option<crate::higent::ResourceUri>, text: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::seat::OpenDocumentResult, String>>;
            subscribe_document(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Result<crate::higent::seat::DocumentState, String>>;
            poll_document(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Vec<crate::higent::seat::DocumentApplied>>;
            dispatch_document(channel: &crate::higent::ChannelUri, action: crate::higent::seat::DocumentApplied) -> ();
            unsubscribe_document(channel: &crate::higent::ChannelUri) -> crate::higent::SeatFuture<()>;
            lsp(session: crate::higent::SessionUri, method: String, params: serde_json::Value) -> crate::higent::SeatFuture<Result<serde_json::Value, String>>;
        }

        fn terminal_open(
            &self,
            _session: crate::higent::SessionUri,
            _channel: crate::higent::ChannelUri,
            _cwd: Option<String>,
            _cols: u16,
            _rows: u16,
            _events: Arc<dyn Fn(crate::higent::TerminalEvent) + Send + Sync>,
        ) -> crate::higent::SeatFuture<Option<crate::higent::TerminalHandle>> {
            unreachable!("the steering tests never reach the seat")
        }
    }

    fn running_panel(store: &mut Store, ui: &UiCtx) -> ChatPanel {
        let mut host = crate::higent::HostId::LOCAL;
        store.update::<crate::higent::Servers>(|servers| {
            host = servers.mint(Arc::new(InertSeat));
        });
        let mut panel = ChatPanel::new(store, ui, host, "s", "chat:1");
        panel.state = Link::Ready;
        panel.active = Some(ActiveStream {
            turn: crate::higent::TurnId::new("t1"),
            cells: 0,
            parts: rpds::HashTrieMapSync::new_sync(),
            tools: rpds::HashTrieMapSync::new_sync(),
            group: None,
        });
        panel
    }

    fn queued(id: &str) -> Message {
        Message {
            text: format!("queued {id}"),
            origin: MessageOrigin {
                kind: MessageKind::User,
            },
            attachments: None,
            model: None,
            agent: None,
            meta: None,
        }
    }

    /// The Claude Code Esc-with-prompt shape: a prompt at a running
    /// agent DROPS the queue, cancels the turn, and fires the moment
    /// the turn ends — never parked in a queue nothing drains.
    #[test]
    fn a_prompt_at_a_running_agent_steers() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        let mut panel = running_panel(&mut store, ui);
        panel.stack.insert_queued("q1".to_owned(), queued("q1"));
        let mut batch = imba::effect::Batch::new();

        let _ = panel.send_text(
            &mut store,
            ui,
            "steer me".to_owned(),
            None,
            None,
            &mut batch.effects(),
        );
        assert_eq!(panel.steering.as_deref(), Some("steer me"));
        assert!(
            panel.stack.queue_oracle().is_empty(),
            "the standing queue dropped"
        );
        assert!(panel.pending.is_none(), "no turn starts under the cancel");

        // The cancel lands: the steered prompt fires as a REAL send.
        panel.apply_actions(
            &mut store,
            ui,
            vec![StateAction::ChatTurnCancelled(ChatTurnCancelledAction {
                turn_id: "t1".to_owned(),
                duration: 0,
                meta: None,
            })],
            &mut batch.effects(),
        );
        assert!(panel.steering.is_none());
        assert_eq!(
            panel.pending.as_ref().map(|(_, text)| text.as_str()),
            Some("steer me"),
            "the steered prompt became the next turn"
        );
    }

    /// The reason for the split: one conversation, MANY mounts. A
    /// model mutation must reach every view — each lays it at its
    /// own width, none fight over a shared laid list.
    #[test]
    fn a_model_mutation_rolls_every_view() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        let mut host = crate::higent::HostId::LOCAL;
        store.update::<crate::higent::Servers>(|servers| {
            host = servers.mint(Arc::new(InertSeat));
        });
        let mut panel = ChatPanel::new(&store, ui, host, "s", "chat:2");
        panel.state = Link::Ready;
        let mut batch = imba::effect::Batch::new();

        let a = ChatViewId::mint();
        let b = ChatViewId::mint();
        panel.ensure_view(&mut store, ui, a, &mut batch.effects());
        panel.ensure_view(&mut store, ui, b, &mut batch.effects());

        panel.apply_actions(
            &mut store,
            ui,
            vec![StateAction::ChatTurnStarted(
                crate::higent::ahp_types::actions::ChatTurnStartedAction {
                    turn_id: "t1".to_owned(),
                    started_at: String::new(),
                    message: queued("hello"),
                    queued_message_id: None,
                    meta: None,
                },
            )],
            &mut batch.effects(),
        );
        panel.apply_actions(
            &mut store,
            ui,
            vec![StateAction::ChatTurnCancelled(ChatTurnCancelledAction {
                turn_id: "t1".to_owned(),
                duration: 0,
                meta: None,
            })],
            &mut batch.effects(),
        );

        assert_eq!(panel.turns.len(), 1, "the MODEL transcript holds the turn");
        assert_eq!(panel.turns[0].cells.len(), 2, "message + cancel notice");
        for id in [a, b] {
            let view = panel.views.get(&id).expect("the view record");
            let rows: Vec<ChatRow> = view.rows.content().rows().collect();
            assert_eq!(rows.len(), 1, "the turn row reached view {id:?}");
            let ChatRow::Turn(turn) = &rows[0] else {
                panic!("a turn row");
            };
            assert_eq!(
                turn.cells_oracle().len(),
                2,
                "both cells laid in view {id:?}"
            );
        }
    }

    /// An explicit STOP is just a stop — it drops a standing steer.
    #[test]
    fn an_explicit_stop_drops_the_steer() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        let mut panel = running_panel(&mut store, ui);
        let mut batch = imba::effect::Batch::new();

        let _ = panel.send_text(
            &mut store,
            ui,
            "steer me".to_owned(),
            None,
            None,
            &mut batch.effects(),
        );
        assert!(panel.steering.is_some());
        panel.perform_model(
            &mut store,
            ui,
            ChatPanelCommand::Composer(ComposerCommand::Stop),
            &mut batch.effects(),
        );
        assert!(panel.steering.is_none(), "STOP is not a steer");

        panel.apply_actions(
            &mut store,
            ui,
            vec![StateAction::ChatTurnCancelled(ChatTurnCancelledAction {
                turn_id: "t1".to_owned(),
                duration: 0,
                meta: None,
            })],
            &mut batch.effects(),
        );
        assert!(panel.pending.is_none(), "nothing fires after a plain stop");
    }
}
