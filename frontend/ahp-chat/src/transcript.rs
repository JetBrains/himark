// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The transcript COMPONENT: the measured cell list — rows of
//! turns, the older-turns loader, the sleeping off-viewport turn.
//! The workbench wraps it in an imba scroller; a native shell
//! mounts it BARE and scrolls it itself (docs/ui/imba-ffi.md, the
//! stage road). Its command vocabulary is its own (`ListCommand<
//! RowCommand>`): the panel wraps.

use editor::env;
use hikit::fonts::ui_text_font;
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::Effects,
    event::{Event, EventResult},
    leaf::leaf,
    list::{ListCommand, ListView},
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View, Widget,
};
use skia_safe::{Paint, Rect, Size};

use crate::cell::Cell;
use crate::chat::model;
use crate::turn::{TurnCommand, TurnView};

pub(crate) type Transcript = ListView<ChatRow, ahp_wire::client::TurnId>;

/// The transcript as a VIEW of its own: the measured cell list.
/// `ChatPanel` owns one per mount and rolls it on every model
/// change; panes decide whether an imba scroller wraps it (the
/// workbench) or a native one owns the viewport (a mounted shell).
#[derive(Clone)]
pub(crate) struct TranscriptView {
    pub(crate) list: Transcript,
}

impl TranscriptView {
    pub(crate) fn empty() -> Self {
        Self {
            list: ListView::empty(),
        }
    }

    // The list surface the panel's roll machinery speaks — plain
    // delegation; the component boundary is the type, not a new
    // vocabulary.
    pub(crate) fn len(&self) -> usize {
        self.list.len()
    }

    pub(crate) fn total_height(&self) -> f32 {
        self.list.total_height()
    }

    /// One row's STORED element height — the height accounting
    /// compares it against the cells' sum.
    pub(crate) fn height_at(&self, index: usize) -> Option<f32> {
        self.list.height_at(index)
    }

    pub(crate) fn row_range(
        &self,
        key: &ahp_wire::client::TurnId,
    ) -> Option<std::ops::Range<usize>> {
        self.list.row_range(key)
    }

    pub(crate) fn view_at(&self, index: usize) -> Option<ChatRow> {
        self.list.view_at(index)
    }

    pub(crate) fn rows(&self) -> impl Iterator<Item = ChatRow> + '_ {
        self.list.rows()
    }

    pub(crate) fn splice(
        &mut self,
        range: std::ops::Range<usize>,
        rows: impl IntoIterator<Item = (ChatRow, f32)>,
    ) {
        self.list.splice(range, rows)
    }

    pub(crate) fn splice_slice(
        &mut self,
        range: std::ops::Range<usize>,
        slice: imba::list::ListSlice<ChatRow, ahp_wire::client::TurnId>,
    ) {
        self.list.splice_slice(range, slice)
    }

    pub(crate) fn reveal_tail(&mut self) {
        self.list.reveal_tail()
    }

    pub(crate) fn probe_viewport_top(&self) -> f32 {
        self.list.probe_viewport_top()
    }

    pub(crate) fn probe_viewport_height(&self) -> f32 {
        self.list.probe_viewport_height()
    }
}

impl View for TranscriptView {
    type Command = ListCommand<RowCommand>;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        self.list.focus_data(store, ui)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        self.list.destroy(store, fx);
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        self.list.perform(store, ui, command, fx);
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        self.list.display(arena, store, ui)
    }
}

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
pub(crate) enum ChatRow {
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
                            Event::Paint { .. } => EventResult::command(RowCommand::Wake),
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

pub(crate) struct ArmedLoader<Inner> {
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
            return result.merge(EventResult::command(RowCommand::Activate));
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

pub(crate) enum Either<A, B, C> {
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

/// The transcript PANE: the component bare, a native scroller's
/// content — himark mounts it; the workbench never shows this
/// flavor (its chat pane scrolls the transcript itself).
#[derive(Clone)]
pub struct TranscriptPane {
    seat: crate::chats::PaneSeat,
}

impl TranscriptPane {
    pub fn new(
        chats: imba::store::Id<crate::chats::Chats>,
        chat: ahp_wire::client::ChatUri,
    ) -> Self {
        Self {
            seat: crate::chats::PaneSeat::new(chats, chat),
        }
    }
}

impl View for TranscriptPane {
    type Command = crate::chat::ChatPanelCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        self.seat.focus_data(store, ui)
    }

    fn destroy(&mut self, store: &mut Store, _fx: &mut Effects<'_, Self::Command>) {
        self.seat.park(store);
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        self.seat.perform(store, ui, command, fx);
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            match self.seat.view.and_then(|view| {
                crate::chats::Chats::chat_ref(store, self.seat.chats, &self.seat.chat)
                    .and_then(|panel| panel.display_transcript(arena, store, ui, view))
            }) {
                Some(laid) => imba::ThunkBox::new(
                    arena,
                    imba::layout::Layout::layout(laid, arena, constraints),
                ),
                None => crate::chats::PaneSeat::boot_leaf(arena, constraints),
            }
        })
    }
}

impl hikit::panel::PanelView for TranscriptPane {
    type Place = crate::chats::ChatPlace;

    fn title(&self, store: &Store) -> String {
        self.seat.title(store)
    }

    fn dismantle(&mut self, store: &mut Store) {
        self.seat.dismantle(store);
    }

    fn displaced(&mut self, store: &mut Store) {
        self.seat.park(store);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
