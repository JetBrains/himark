// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::atomic::{AtomicU32, Ordering};

use crate::higent::{
    CancelTurnEffect, DispatchChatActionEffect, FetchFileEditEffect, FetchTurnsEffect,
    PollChatActionsEffect, StartTurnEffect, TurnsPage,
};
use crate::{env, fonts::ui_text_font, EditorCommand, PanelView};
use ahp_types::actions::{
    ChatPendingMessageRemovedAction, ChatToolCallConfirmedAction, StateAction,
};
use ahp_types::state::{
    ChatState, ConfirmationOption, ConfirmationOptionKind, PendingMessageKind,
    ToolCallConfirmationReason, ToolInput, Turn,
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
            _ => false,
        }
    }
}

#[derive(Clone)]
struct ActiveStream {
    turn: String,
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

pub struct ChatPanel {
    server: crate::higent::HostId,

    session: ahp_types::common::Uri,
    chat: ahp_types::common::Uri,
    state: Link,
    title: String,
    rows: Rows,

    composer: Composer,

    stack: WidgetStack,

    completion: crate::completion::Completion,

    picked: rpds::VectorSync<crate::completion::PickedFile>,

    focus: ChatArea,

    cursor: Option<String>,

    fetch_token: Option<CancellationToken>,

    poll_token: Option<CancellationToken>,

    active: Option<ActiveStream>,

    has_loader: bool,

    pending: Option<(String, String)>,

    /// A STEERED prompt: sent while a turn ran, so the run was
    /// cancelled and this fires the moment the turn ends — the
    /// Claude Code Esc-with-prompt shape. An explicit STOP drops it.
    steering: Option<String>,

    initial_prompt: Option<String>,

    toolbar: super::session_toolbar::SessionToolbar,

    minted: u64,

    panel_width: AtomicU32,

    rows_height: AtomicU32,
}

fn completion_editor(command: ::editor::EditorCommand) -> ChatPanelCommand {
    ChatPanelCommand::Composer(ComposerCommand::Editor(
        imba::scroll::ScrollCommand::Content(command),
    ))
}

impl Clone for ChatPanel {
    fn clone(&self) -> Self {
        Self {
            server: self.server,
            session: self.session.clone(),
            chat: self.chat.clone(),
            state: self.state.clone(),
            title: self.title.clone(),
            rows: self.rows.clone(),
            composer: self.composer.clone(),
            stack: self.stack.clone(),
            completion: self.completion.clone(),
            picked: self.picked.clone(),
            focus: self.focus,
            cursor: self.cursor.clone(),
            fetch_token: self.fetch_token,
            poll_token: self.poll_token,
            active: self.active.clone(),
            steering: self.steering.clone(),
            has_loader: self.has_loader,
            pending: self.pending.clone(),
            initial_prompt: self.initial_prompt.clone(),
            toolbar: self.toolbar.clone(),
            minted: self.minted,
            panel_width: AtomicU32::new(self.panel_width.load(Ordering::Relaxed)),
            rows_height: AtomicU32::new(self.rows_height.load(Ordering::Relaxed)),
        }
    }
}

impl ChatPanel {
    pub fn new(
        store: &imba::store::Store,
        ui: &UiCtx,
        server: crate::higent::HostId,
        session: impl Into<ahp_types::common::Uri>,
        chat: impl Into<ahp_types::common::Uri>,
    ) -> Self {
        Self {
            server,
            session: session.into(),
            chat: chat.into(),
            state: Link::Idle,
            title: "Agent Chat".to_owned(),
            rows: ScrollView::new(ListView::empty()),
            composer: Composer::new(store, ui),
            stack: WidgetStack::new(),
            completion: crate::completion::Completion::new(),
            picked: rpds::VectorSync::new_sync(),
            focus: ChatArea::Composer,
            cursor: None,
            fetch_token: None,
            poll_token: None,
            active: None,
            has_loader: false,
            pending: None,
            steering: None,
            initial_prompt: None,
            toolbar: super::session_toolbar::SessionToolbar::new(store, ui),
            minted: 0,
            panel_width: AtomicU32::new(800.0_f32.to_bits()),
            rows_height: AtomicU32::new(600.0_f32.to_bits()),
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

    pub fn transcript(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.rows
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
        self.completion.open()
    }

    #[doc(hidden)]
    pub fn completion_rows(&self) -> Vec<String> {
        self.completion.row_labels()
    }

    #[doc(hidden)]
    pub fn completion_picked(&self) -> Vec<String> {
        self.picked.iter().map(|pick| pick.rel.clone()).collect()
    }

    pub fn composer_text(&self) -> String {
        self.composer.text()
    }

    #[doc(hidden)]
    pub fn blurred(&self) -> bool {
        self.composer.blurred()
    }

    pub fn footer_height(&self, store: &Store, nominal_height: f32) -> f32 {
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        self.composer.band_height(&chrome, nominal_height)
            + self.stack.height(&chrome)
            + theme.ui().toolbar.height
    }

    pub fn ready(&self) -> bool {
        matches!(self.state, Link::Ready)
    }

    pub fn permission_oracle(&self) -> Option<(String, String, Option<String>, Vec<String>)> {
        self.stack.permission_oracle()
    }

    pub fn composer_content_height(&self) -> f32 {
        self.composer.content_height()
    }

    pub fn queue_oracle(&self) -> Vec<(String, String)> {
        self.stack.queue_oracle()
    }

    fn busy(&self) -> bool {
        self.pending.is_some() || self.active.is_some()
    }

    fn panel_width(&self) -> f32 {
        f32::from_bits(self.panel_width.load(Ordering::Relaxed))
    }

    fn seat(&self, store: &Store) -> Option<std::sync::Arc<dyn crate::higent::AhpServer>> {
        crate::higent::Servers::seat(store, self.server)
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
                let Some(seat) = self.seat(store) else {
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
        key: &str,
        cells: Vec<CellSpec>,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> (TurnView, f32) {
        let content_width = TurnView::content_width(self.panel_width());
        let mut rows = Vec::with_capacity(cells.len());
        let mut total = 0.0;
        for (index, spec) in cells.into_iter().enumerate() {
            let (cell, height) = self.build_cell(store, ui, key, index, spec, content_width, fx);
            total += height;
            rows.push((cell, height));
        }
        (TurnView::new(key, content_width, rows), total)
    }

    fn build_page(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        turns: &[Turn],
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> ListSlice<ChatRow, String> {
        let chrome = env::Themes::of(store).ui().chat.clone();
        let mut slice = ListSlice::new();
        if self.cursor.is_some() {
            slice.push_sized(ChatRow::Loader { armed: true }, chrome.loader_height);
        }
        for turn in turns {
            let (view, height) = self.build_turn(store, ui, &turn.id, turn_cells(turn), fx);
            slice.push_keyed_sized(turn.id.clone(), ChatRow::Turn(view), height);
        }
        slice
    }

    fn load_older(&mut self, store: &Store, fx: &mut Effects<'_, ChatPanelCommand>) {
        if self.fetch_token.is_some() {
            return;
        }
        let Some(cursor) = self.cursor.clone() else {
            return;
        };
        let Some(seat) = self.seat(store) else {
            return;
        };
        self.set_loader(store, false);
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

    fn set_loader(&mut self, store: &Store, armed: bool) {
        if !self.has_loader {
            return;
        }
        let height = env::Themes::of(store).ui().chat.loader_height;
        self.rows
            .content_mut()
            .splice(0..1, [(ChatRow::Loader { armed }, height)]);
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
                let slice = self.build_page(store, ui, &page.turns, fx);
                let end = usize::from(self.has_loader);
                self.rows.content_mut().splice_slice(0..end, slice);
                self.has_loader = self.cursor.is_some();
                // The prepend landed above the viewport: the settle
                // pulse re-aims the scroll at the anchored row before
                // this frame paints (docs/editor/viewport-preservation.md).
                fx.settle();
            }
            Err(error) => {
                eprintln!("[higent] fetchTurns failed: {error}");
                self.set_loader(store, true);
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
                let slice = self.build_page(store, ui, &state.turns, fx);
                let len = self.rows.content().len();
                self.rows.content_mut().splice_slice(0..len, slice);
                self.has_loader = self.cursor.is_some();
                self.reveal_tail(store);

                self.mark_read(store, fx);
                self.relaunch_poll(store, fx);

                if let Some(prompt) = self.initial_prompt.take() {
                    self.send_text(store, ui, prompt, fx);
                }
            }
            Err(error) => {
                self.state = Link::Failed(error);
            }
        }
    }

    #[doc(hidden)]
    pub fn toolbar_probe(&self) -> super::session_toolbar::ToolbarProbe {
        self.toolbar.probe()
    }

    fn model_pick(&self) -> Option<ahp_types::state::ModelSelection> {
        self.toolbar.model_selection()
    }

    fn dispatch(
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
                channel: self.chat.clone(),
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
                channel: self.session.clone(),
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
        self.dispatch(
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

    fn update_tool_call(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        turn_id: &str,
        tool_call_id: &str,
        face: ToolFace,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let Some(track) = self
            .active
            .as_ref()
            .filter(|active| active.turn == turn_id)
            .and_then(|active| active.tools.get(tool_call_id))
        else {
            return;
        };
        let cell = track.cell;
        self.route_cell(
            store,
            ui,
            turn_id.to_owned(),
            cell,
            CellCommand::Tool(ToolUpdate::Face {
                id: tool_call_id.to_owned(),
                face,
            }),
            fx,
        );
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

    fn apply_actions(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        actions: Vec<StateAction>,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let follow = self.near_tail();
        for action in actions {
            match action {
                StateAction::ChatTurnStarted(action) => {
                    let (kind, text) = message_cell(&action.message);
                    let (view, height) = self.build_turn(
                        store,
                        ui,
                        &action.turn_id,
                        vec![CellSpec::Text(kind, text)],
                        fx,
                    );
                    let mut slice = ListSlice::new();
                    slice.push_keyed_sized(action.turn_id.clone(), ChatRow::Turn(view), height);

                    let range = self
                        .pending
                        .take()
                        .and_then(|(key, _)| self.rows.content().row_range(&key));
                    let len = self.rows.content().len();
                    self.rows
                        .content_mut()
                        .splice_slice(range.unwrap_or(len..len), slice);
                    self.active = Some(ActiveStream {
                        turn: action.turn_id,
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
                        self.append_stream_cell(store, ui, &action.turn_id, spec, fx);
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
                        if active.turn == action.turn_id {
                            active.parts.insert_mut(id, first);
                        }
                    }
                }
                StateAction::ChatDelta(action) => {
                    self.append_delta(
                        store,
                        ui,
                        &action.turn_id,
                        &action.part_id,
                        action.content,
                        fx,
                    );
                }
                StateAction::ChatReasoning(action) => {
                    self.append_delta(
                        store,
                        ui,
                        &action.turn_id,
                        &action.part_id,
                        action.content,
                        fx,
                    );
                }
                StateAction::ChatToolCallStart(action) => {
                    let index = self.append_stream_cell(
                        store,
                        ui,
                        &action.turn_id,
                        CellSpec::Tools(vec![ToolCallSpec {
                            id: action.tool_call_id.clone(),
                            display_name: action.display_name.clone(),
                            face: streaming_tool_face(&action.display_name),
                        }]),
                        fx,
                    );
                    if let (Some(index), Some(active)) = (index, self.active.as_mut()) {
                        if active.turn == action.turn_id {
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
                        .filter(|active| active.turn == action.turn_id)
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
                            store,
                            ui,
                            &action.turn_id,
                            &action.tool_call_id,
                            running_tool_face(&display, &invocation),
                            fx,
                        );
                    } else {
                        self.update_tool_call(
                            store,
                            ui,
                            &action.turn_id,
                            &action.tool_call_id,
                            pending_tool_face(&display, &invocation),
                            fx,
                        );
                        let input = match &action.tool_input {
                            Some(ToolInput::Inline(text)) => Some(text.clone()),
                            _ => None,
                        };
                        self.stack.set_ask(PermissionAsk::new(
                            action.turn_id.clone(),
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
                        .filter(|active| active.turn == action.turn_id)
                        .and_then(|active| active.tools.get(&action.tool_call_id).cloned());
                    if let Some(track) = track {
                        let face = if action.approved {
                            running_tool_face(&track.display_name, &track.invocation)
                        } else {
                            denied_tool_face(&track.display_name)
                        };
                        self.update_tool_call(
                            store,
                            ui,
                            &action.turn_id,
                            &action.tool_call_id,
                            face,
                            fx,
                        );
                    }
                }
                StateAction::ChatToolCallComplete(action) => {
                    let track = self
                        .active
                        .as_ref()
                        .filter(|active| active.turn == action.turn_id)
                        .and_then(|active| active.tools.get(&action.tool_call_id).cloned());
                    let Some(track) = track else {
                        continue;
                    };
                    let face = completed_tool_face(
                        &track.display_name,
                        track.call.as_deref(),
                        &action.result,
                    );
                    self.update_tool_call(
                        store,
                        ui,
                        &action.turn_id,
                        &action.tool_call_id,
                        face,
                        fx,
                    );

                    for spec in result_diff_specs(&action.result) {
                        self.append_stream_cell(store, ui, &action.turn_id, spec, fx);
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
                        self.append_stream_cell(
                            store,
                            ui,
                            &action.turn_id,
                            CellSpec::Text(kind, markdown),
                            fx,
                        );
                    }
                }
                StateAction::ChatTurnComplete(action) => {
                    if self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.turn == action.turn_id)
                    {
                        self.active = None;
                        self.stack.clear_ask();
                        if let Some(text) = self.steering.take() {
                            self.send_text(store, ui, text, fx);
                        }
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
                    self.append_stream_cell(
                        store,
                        ui,
                        &action.turn_id,
                        CellSpec::Text(CellKind::Notice, "*Turn cancelled.*".to_owned()),
                        fx,
                    );
                    self.active = None;
                    self.stack.clear_ask();
                    if let Some(text) = self.steering.take() {
                        self.send_text(store, ui, text, fx);
                    }
                    self.mark_read(store, fx);
                }
                StateAction::ChatError(action) => {
                    let markdown = format!(
                        "**Turn failed** ({}): {}",
                        action.error.error_type, action.error.message
                    );
                    self.append_stream_cell(
                        store,
                        ui,
                        &action.turn_id,
                        CellSpec::Text(CellKind::Error, markdown),
                        fx,
                    );
                    self.active = None;
                    self.stack.clear_ask();
                    if let Some(text) = self.steering.take() {
                        self.send_text(store, ui, text, fx);
                    }
                    self.mark_read(store, fx);
                }

                _ => {}
            }
        }
        if follow {
            self.reveal_tail(store);
        }
    }

    fn append_delta(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        turn_id: &str,
        part_id: &str,
        content: String,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let Some(active) = &self.active else {
            return;
        };
        if active.turn != turn_id {
            return;
        }
        let Some(cell) = active.parts.get(part_id).copied() else {
            return;
        };
        self.route_cell(
            store,
            ui,
            turn_id.to_owned(),
            cell,
            CellCommand::Append(content),
            fx,
        );
    }

    fn append_stream_cell(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        turn_id: &str,
        spec: CellSpec,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) -> Option<usize> {
        let active = self.active.clone()?;
        if active.turn != turn_id {
            return None;
        }

        if let (CellSpec::Tools(specs), Some(group)) = (&spec, active.group) {
            for spec in specs.iter().cloned() {
                self.route_cell(
                    store,
                    ui,
                    turn_id.to_owned(),
                    group,
                    CellCommand::Tool(ToolUpdate::Add(spec)),
                    fx,
                );
            }
            return Some(group);
        }
        let range = self.rows.content().row_range(&active.turn)?;
        let content_width = TurnView::content_width(self.panel_width());
        let index = active.cells;
        let opens_run = matches!(spec, CellSpec::Tools(_));
        let (cell, height) =
            self.build_cell(store, ui, &active.turn, index, spec, content_width, fx);
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
        if let Some(active) = &mut self.active {
            active.cells += 1;
            active.group = opens_run.then_some(index);
        }
        Some(index)
    }

    fn send(&mut self, store: &mut Store, ui: &UiCtx, fx: &mut Effects<'_, ChatPanelCommand>) {
        if self.composer.is_empty() {
            return;
        }

        let text = self.composer.text().trim().to_owned();
        self.send_text(store, ui, text, fx);
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
        text: &str,
    ) -> Option<Vec<ahp_types::state::MessageAttachment>> {
        let uris = super::Hosts::uris(store, self.server)?;
        let picked: Vec<crate::completion::PickedFile> = std::mem::take(&mut self.picked)
            .into_iter()
            .cloned()
            .collect();
        super::file_completion::resource_attachments(text, picked, uris.as_ref())
    }

    fn send_text(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        text: String,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        if !matches!(self.state, Link::Ready) {
            return;
        }
        let Some(seat) = self.seat(store) else {
            return;
        };
        if text.is_empty() {
            return;
        }

        if self.completion.open() {
            self.completion.drop_state(
                self.composer.document_mut(),
                store,
                ui,
                fx,
                completion_editor,
            );
        }
        if self.busy() {
            // STEERING, the Claude Code Esc-with-prompt shape: a
            // prompt sent at a running agent drops the queue, cancels
            // the turn, and fires the moment the turn ends. (The old
            // road QUEUED here — into a queue nothing ever drained.)
            for (id, _) in self.stack.queue_oracle() {
                self.stack.remove_queued(&id);
                self.dispatch(
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
            self.composer.clear(store, ui);
            return;
        }
        let attachments = self.completion_attachments(store, &text);
        self.minted += 1;
        let key = format!("local-{}", self.minted);
        let cells = vec![
            CellSpec::Text(CellKind::User, text.clone()),
            CellSpec::Text(CellKind::Notice, "*Thinking…*".to_owned()),
        ];
        let (view, height) = self.build_turn(store, ui, &key, cells, fx);

        let mut slice = ListSlice::new();
        slice.push_keyed_sized(key.clone(), ChatRow::Turn(view), height);
        let len = self.rows.content().len();
        self.rows.content_mut().splice_slice(len..len, slice);
        self.pending = Some((key.clone(), text.clone()));
        let placeholder = key;
        fx.push(
            AnyEffect::new(StartTurnEffect {
                seat,
                chat: self.chat.clone(),
                text,
                attachments,
                model: self.model_pick(),
            })
            .map(move |result| ChatPanelCommand::Accepted {
                placeholder: placeholder.clone(),
                result,
            }),
        );
        self.composer.clear(store, ui);
        self.reveal_tail(store);
    }

    fn apply_send_failed(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        placeholder: String,
        error: String,
        fx: &mut Effects<'_, ChatPanelCommand>,
    ) {
        let follow = self.near_tail();
        let sent = match &self.pending {
            Some((key, text)) if *key == placeholder => text.clone(),
            _ => String::new(),
        };
        self.pending = None;
        let Some(range) = self.rows.content().row_range(&placeholder) else {
            return;
        };
        let cells = vec![
            CellSpec::Text(CellKind::User, sent),
            CellSpec::Text(
                CellKind::Error,
                format!("**The message was not delivered:** {error}"),
            ),
        ];
        let (view, height) = self.build_turn(store, ui, &placeholder, cells, fx);
        let mut slice = ListSlice::new();
        slice.push_keyed_sized(placeholder.clone(), ChatRow::Turn(view), height);
        self.rows.content_mut().splice_slice(range, slice);
        if follow {
            self.reveal_tail(store);
        }
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

impl View for ChatPanel {
    type Command = ChatPanelCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::UiCtx,
    ) -> imba::focus::FocusData<'w, ChatPanelCommand> {
        use imba::focus::FocusData;
        let ask = self.stack.ask_keys();
        let composer_empty = self.composer.is_empty();
        let expanded = self.composer.expanded();
        let focus = self.focus;

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
        let area = match self.focus {
            ChatArea::Composer => self
                .composer
                .focus_data(store, ui)
                .map(ChatPanelCommand::Composer),
            ChatArea::Transcript => self.rows.focus_data(store, ui).map(ChatPanelCommand::Rows),
        };
        own.merge_under(area)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        if let Some(token) = self.fetch_token.take() {
            fx.cancel(token);
        }
        if let Some(token) = self.poll_token.take() {
            fx.cancel(token);
        }
        fx.scope(ChatPanelCommand::Rows, |fx| self.rows.destroy(store, fx));
        fx.scope(ChatPanelCommand::Composer, |fx| {
            self.composer.destroy(store, fx)
        });
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
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
            ChatPanelCommand::Focus(area, then) => {
                self.focus = area;
                if let Some(command) = then {
                    self.perform(store, ui, *command, fx);
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
            ChatPanelCommand::Send => self.send(store, ui, fx),
            ChatPanelCommand::Answer(index) => self.answer(store, index, fx),
            ChatPanelCommand::Dispatched { undo_queue, result } => {
                if let Err(error) = result {
                    eprintln!("[higent] dispatch failed: {error}");
                    if let Some(id) = undo_queue {
                        self.stack.remove_queued(&id);
                    }
                }
            }
            ChatPanelCommand::ToolbarSync => {
                if let Some(channel) = super::Agents::channel(store, &self.session_id()) {
                    self.toolbar.sync(store, ui, self.server, &channel);
                }
            }
            ChatPanelCommand::Toolbar(command) => match {
                let mut ask = super::ToolbarAsk::None;
                fx.scope(ChatPanelCommand::Toolbar, |fx| {
                    ask = self.toolbar.perform(store, ui, command, fx)
                });
                ask
            } {
                super::ToolbarAsk::Edits(mode) => {
                    if let Some(seat) = self.seat(store) {
                        let mut config = serde_json::Map::new();
                        config.insert("permissionMode".to_owned(), serde_json::json!(mode));
                        fx.push(
                            AnyEffect::new(DispatchChatActionEffect {
                                seat,
                                channel: self.session.clone(),
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
            },
            ChatPanelCommand::Cell {
                turn,
                cell,
                command,
            } => self.route_cell(store, ui, turn, cell, command, fx),
            ChatPanelCommand::Rows(command) => {
                if peeled_activation(&command) {
                    self.load_older(store, fx);
                    return;
                }
                fx.scope(ChatPanelCommand::Rows, |fx| {
                    self.rows.perform(store, ui, command, fx)
                });
            }

            ChatPanelCommand::Blurred(blurred) => {
                if blurred && self.completion.open() {
                    let editor = self.composer.editor();
                    let _ = editor;
                    self.completion.drop_state(
                        self.composer.document_mut(),
                        store,
                        ui,
                        fx,
                        completion_editor,
                    );
                }
                self.composer.set_blurred(blurred)
            }
            ChatPanelCommand::CompletionFound(found) => {
                let editor = self.composer.editor();
                self.completion
                    .land(store, ui, self.composer.document_mut(), editor, found);
            }
            ChatPanelCommand::Composer(ComposerCommand::Submit) => self.send(store, ui, fx),
            ChatPanelCommand::Composer(ComposerCommand::Stop) => {
                self.steering = None;
                self.stop(store, fx);
            }
            ChatPanelCommand::Composer(command) => {
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
                let session = self.session_id();
                self.completion.sync_path(
                    store,
                    ui,
                    self.composer.document_mut(),
                    editor,
                    at,
                    &session,
                    None,
                    fx,
                    ChatPanelCommand::CompletionFound,
                    completion_editor,
                );
            }
            ChatPanelCommand::Stack(StackCommand::Answer(index)) => self.answer(store, index, fx),
            ChatPanelCommand::Stack(StackCommand::ToggleQueue) => self.stack.toggle_collapsed(),
            ChatPanelCommand::Stack(StackCommand::RemoveQueued(id)) => {
                self.stack.remove_queued(&id);
                self.dispatch(
                    store,
                    StateAction::ChatPendingMessageRemoved(ChatPendingMessageRemovedAction {
                        kind: PendingMessageKind::Queued,
                        id,
                    }),
                    None,
                    fx,
                );
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let theme = env::Themes::of(store);
            let chrome = theme.ui().chat.clone();
            self.panel_width
                .store(size.width.max(1.0).to_bits(), Ordering::Relaxed);

            let band_h = self.composer.band_height(&chrome, size.height);
            let stack_h = self.stack.height(&chrome);

            let toolbar_h = theme.ui().toolbar.height;
            let rows_height = (size.height - band_h - stack_h - toolbar_h).max(1.0);
            self.rows_height
                .store(rows_height.to_bits(), Ordering::Relaxed);

            let mut panel = container(arena, size);

            panel.place(
                0.0,
                0.0,
                imba::Layout::layout(
                    self.rows.display(arena, store, ui),
                    arena,
                    Constraints {
                        min: Size::new(size.width, rows_height),
                        max: Size::new(size.width, rows_height),
                    },
                )
                .map(ChatPanelCommand::Rows)
                .focus_scope(self.focus == ChatArea::Transcript),
            );

            let status = match &self.state {
                Link::Idle | Link::Subscribing => "connecting…".to_owned(),
                Link::Failed(error) => format!("failed: {error}"),
                Link::Ready if self.pending.is_some() => "thinking…".to_owned(),
                Link::Ready if self.active.is_some() => "responding…".to_owned(),
                Link::Ready => String::new(),
            };
            let composer_empty = self.composer.is_empty();
            panel.place(
                0.0,
                rows_height + stack_h,
                self.composer
                    .layout(
                        arena,
                        store,
                        ui,
                        size.width,
                        size.height,
                        ComposerProps {
                            focused: self.focus == ChatArea::Composer,
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

            let toolbar_cells_right = self.toolbar.place(
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
                                skia_safe::Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
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
            let strip_origin = std::sync::Arc::clone(&self.toolbar.strip_origin);
            let toolbar_stale =
                super::Agents::channel(store, &self.session_id()).is_some_and(|channel| {
                    super::SessionToolbar::fingerprint(store, self.server, &channel)
                        != self.toolbar.synced
                });

            let rows_height_ = rows_height;
            let focus = self.focus;
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
        })
    }
}

struct ChatWidget<Inner> {
    panel: Inner,
    rows_height: f32,
    focus: ChatArea,
    boot: bool,

    toolbar_stale: bool,

    strip_origin: std::sync::Arc<std::sync::atomic::AtomicU64>,
    strip_top: f32,
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

impl PanelView for ChatPanel {
    type Place = crate::NoPlace;

    fn title(&self, _store: &Store) -> String {
        self.title.clone()
    }

    fn dismantle(&mut self, _store: &mut Store) {}

    fn as_any(&self) -> &dyn std::any::Any {
        self
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
            create_session(dirs: Vec<String>, options: crate::higent::SessionOptions) -> crate::higent::SeatFuture<Result<String, String>>;
            resolve_session_config(working_directory: Option<String>, config: Option<serde_json::Map<String, serde_json::Value>>) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::commands::ResolveSessionConfigResult, String>>;
            dispose_session(session: String) -> crate::higent::SeatFuture<Result<(), String>>;
            subscribe_session(session: String) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::SessionState, String>>;
            poll_session(session: String) -> crate::higent::SeatFuture<Vec<StateAction>>;
            create_chat(session: String) -> crate::higent::SeatFuture<Result<String, String>>;
            subscribe_chat(chat: String) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::ChatState, String>>;
            fetch_turns(chat: String, cursor: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::TurnsPage, String>>;
            start_turn(chat: String, text: String, attachments: Option<Vec<crate::higent::ahp_types::state::MessageAttachment>>, model: Option<crate::higent::ahp_types::state::ModelSelection>) -> crate::higent::SeatFuture<Result<(), String>>;
            poll_chat(chat: String) -> crate::higent::SeatFuture<Vec<StateAction>>;
            cancel_turn(chat: String, turn: String) -> crate::higent::SeatFuture<()>;
            dispatch_action(chat: String, action: StateAction) -> crate::higent::SeatFuture<Result<(), String>>;
            read_file_edit(before: Option<String>, after: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::FileEditContents, String>>;
            resource_read(session: String, uri: crate::higent::ResourceUri) -> crate::higent::SeatFuture<Option<String>>;
            resource_write(session: String, uri: crate::higent::ResourceUri, text: String) -> crate::higent::SeatFuture<bool>;
            resource_list(session: String, uri: crate::higent::ResourceUri) -> crate::higent::SeatFuture<Option<Vec<(String, bool)>>>;
            resource_watch(session: String, uri: crate::higent::ResourceUri, events: Arc<dyn Fn() + Send + Sync>) -> crate::higent::SeatFuture<Option<crate::higent::WatchHandle>>;
            resource_unwatch(handle: crate::higent::WatchHandle) -> crate::higent::SeatFuture<()>;
            search(session: String, ask: crate::higent::SearchAsk) -> crate::higent::SeatFuture<Option<crate::higent::SearchResult>>;
            terminal_input(channel: &String, data: String) -> ();
            terminal_resize(channel: &String, cols: u16, rows: u16) -> ();
            terminal_dispose(channel: &String) -> ();
            subscribe_changeset(channel: String) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::ChangesetState, String>>;
            poll_changeset(channel: String) -> crate::higent::SeatFuture<Vec<StateAction>>;
            unsubscribe_changeset(channel: &String) -> ();
            subscribe_annotations(session: String) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::AnnotationsState, String>>;
            poll_annotations(session: String) -> crate::higent::SeatFuture<Vec<StateAction>>;
            dispatch_annotations(session: &String, action: StateAction) -> ();
            unsubscribe_annotations(session: &String) -> ();
            open_document(session: String, uri: Option<crate::higent::ResourceUri>, text: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::seat::OpenDocumentResult, String>>;
            subscribe_document(channel: String) -> crate::higent::SeatFuture<Result<crate::higent::seat::DocumentState, String>>;
            poll_document(channel: String) -> crate::higent::SeatFuture<Vec<crate::higent::seat::DocumentApplied>>;
            dispatch_document(channel: &String, action: crate::higent::seat::DocumentApplied) -> ();
            unsubscribe_document(channel: &String) -> crate::higent::SeatFuture<()>;
            lsp(session: String, method: String, params: serde_json::Value) -> crate::higent::SeatFuture<Result<serde_json::Value, String>>;
        }

        fn terminal_open(
            &self,
            _session: String,
            _channel: String,
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
            turn: "t1".to_owned(),
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

        panel.send_text(&mut store, ui, "steer me".to_owned(), &mut batch.effects());
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

    /// An explicit STOP is just a stop — it drops a standing steer.
    #[test]
    fn an_explicit_stop_drops_the_steer() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        let mut panel = running_panel(&mut store, ui);
        let mut batch = imba::effect::Batch::new();

        panel.send_text(&mut store, ui, "steer me".to_owned(), &mut batch.effects());
        assert!(panel.steering.is_some());
        imba::View::perform(
            &mut panel,
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
