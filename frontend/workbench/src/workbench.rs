// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    anim::{Animation, AnimationClock, Easing, Motion},
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult},
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    Thunk, View,
};
use skia_safe::{Contains as _, Rect, Size};

use crate::columns::{ChatSpan, ChatStance, Columns, DockSpan, DrawerSide, Span};
use crate::workbench_node::{NodeCommand, PaneSlot, Panel, WorkbenchNode};

// Like every theme metric, the window chrome's widths are PHYSICAL
// pixels tuned for 2x displays — the shell hands the engine device
// pixels and the scale is baked into the design.

/// The column's default share of the window; dragging adjusts it
/// freely upward from the chrome's `chat_min_width` — there is no
/// maximum.
const CHAT_COLUMN_RATIO: f32 = 0.42;

const HANDLE_REACH: f32 = 4.0;

/// Wide enough for the dedicated chat column: the column minimum must
/// fit while the main area keeps a real working width — not a bare
/// editor minimum. Below this the chat lives in the split tree like
/// any other panel.
pub(crate) fn chat_column_engaged(width: f32, window: &::editor::theme::WindowChrome) -> bool {
    width >= window.chat_min_width + window.first_pane_width.max(window.main_min_width)
}

/// The column's effective width at this window, from its RATIO
/// state: the split between chat and panel is proportion-first (the
/// share survives window resizes), bounded by the chat minimum and
/// the main area's guarantee.
pub(crate) fn chat_column_width(
    share: f32,
    width: f32,
    window: &::editor::theme::WindowChrome,
) -> f32 {
    (width * share).clamp(window.chat_min_width, chat_column_max(width, window))
}

/// The policy bound, derived from the CURRENT window at every
/// layout: while the chat and the tree both show, the main area
/// keeps `MAIN_COLUMN_MIN` whatever width the chat was dragged to —
/// squeezing the window compresses the chat down to its minimum
/// first, and past that the chat hides. The stored drag width is
/// untouched; it comes back when the window does.
fn chat_column_max(width: f32, window: &::editor::theme::WindowChrome) -> f32 {
    (width - window.main_min_width).max(window.chat_min_width)
}

/// TEST SUPPORT: no production caller outside this crate.
#[derive(Clone)]
#[doc(hidden)]
pub enum WorkbenchCommand {
    /// To the split tree.
    Node(NodeCommand),

    /// To the dedicated chat column's leaf.
    Chat(NodeCommand),

    /// Flip the keyboard between the chat column and the tree, then
    /// optionally deliver the click that caused the flip (the same
    /// shape as `SplitCommand::Focus`).
    ChatFocus(bool, Option<Box<WorkbenchCommand>>),

    ChatBeginResize,
    ChatResize(f32),
    ChatEndResize,

    /// A header button running a REGISTERED command (toc…): looked
    /// up and queued for the app loop.
    RunCommand(&'static str),

    /// The tree header's maximize: hide the chat until it is asked
    /// back (Cmd-I or the minimize button).
    MaximizeTree,

    /// The tree header's minimize: the chat returns — beside the
    /// tree when the window fits both, fronted over it otherwise.
    RestoreChat,

    /// The chat column's slide advancing.
    ChatTick(AnimationClock),
}

impl std::fmt::Display for WorkbenchCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkbenchCommand::Node(command) => command.fmt(out),
            WorkbenchCommand::Chat(command) => command.fmt(out),
            WorkbenchCommand::ChatFocus(..) => out.write_str("chat column focus"),
            WorkbenchCommand::ChatBeginResize => out.write_str("chat column begin resize"),
            WorkbenchCommand::ChatResize(_) => out.write_str("chat column resize"),
            WorkbenchCommand::ChatEndResize => out.write_str("chat column end resize"),
            WorkbenchCommand::RunCommand(id) => out.write_str(id),
            WorkbenchCommand::MaximizeTree => out.write_str("maximize tree"),
            WorkbenchCommand::RestoreChat => out.write_str("restore chat"),
            WorkbenchCommand::ChatTick(_) => out.write_str("chat column tick"),
        }
    }
}

/// The chat column: a single pane slot pinned left of the split
/// tree, outside it — tree operations (split, close, replace) can
/// never touch it, and panels always open to its right.
#[derive(Clone)]
pub struct ChatColumn {
    node: WorkbenchNode,

    /// The chat's share of the window — RATIO state, like a
    /// `split::Sizing::Ratio` split: dragging stores the proportion,
    /// so resizing the window keeps it.
    share: f32,

    resizing: bool,

    /// How much of the column stands revealed, 0..1 — the dock's
    /// slide, mirrored: the column rolls in from the window's left
    /// edge and the tree narrows with it.
    reveal: Animation<f32>,

    /// Rolling away: minimized once the slide lands.
    closing: bool,
}

impl ChatColumn {
    fn of(panel: Panel) -> Self {
        // Born REVEALED: a chat that opens with the window stands;
        // the slide is for the Chat tab's hide and show.
        let reveal = Animation::done(
            1.0,
            Motion::Ease {
                duration_ms: crate::dock::SLIDE_MS,
                easing: Easing::EaseOut,
            },
        );
        Self {
            node: WorkbenchNode::Leaf(PaneSlot::of(panel)),
            share: CHAT_COLUMN_RATIO,
            resizing: false,
            reveal,
            closing: false,
        }
    }

    /// The revealed share of the column's width, 0..1.
    pub(crate) fn revealed(&self) -> f32 {
        self.reveal.value().clamp(0.0, 1.0)
    }

    /// Mid-slide — the frame keeps a clock on it.
    pub(crate) fn sliding(&self) -> bool {
        self.reveal.running()
    }

    fn show(&mut self) {
        self.closing = false;
        self.reveal.set(1.0);
    }

    fn hide(&mut self) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.resizing = false;
        self.reveal.set(0.0);
    }

    pub(crate) fn replace_panel(&mut self, panel: Panel) -> Panel {
        match &mut self.node {
            WorkbenchNode::Leaf(slot) => slot.replace_panel(panel),
            WorkbenchNode::Split(_) => unreachable!("the chat column is always a leaf"),
        }
    }

    pub fn panel(&self) -> &Panel {
        match &self.node {
            WorkbenchNode::Leaf(slot) => &slot.panel.panel,
            WorkbenchNode::Split(_) => unreachable!("the chat column is always a leaf"),
        }
    }

    pub(crate) fn panel_mut(&mut self) -> &mut Panel {
        match &mut self.node {
            WorkbenchNode::Leaf(slot) => &mut slot.panel.panel,
            WorkbenchNode::Split(_) => unreachable!("the chat column is always a leaf"),
        }
    }
}

#[derive(Clone)]
pub struct Workbench {
    pub root: WorkbenchNode,

    chat: Option<ChatColumn>,

    chat_focused: bool,

    /// Cmd-I in the single-panel presentation FRONTS the hidden chat
    /// over the panel; opening any panel hands the window back. Only
    /// consulted when the window is too narrow for both.
    chat_fronted: bool,

    /// The tree header's MAXIMIZE: the user hid the chat explicitly.
    /// Cmd-I or the minimize button bring it back; the vacant tree
    /// shows the chat regardless (there is nothing else to show).
    chat_minimized: bool,
    /// The window's drawer, if one stands: over the chat column
    /// (Sessions) or over the side column (Contents).
    drawer: Option<DrawerSide>,

    dock: Option<crate::dock::Dock>,
}

impl Workbench {
    pub fn new(root: WorkbenchNode) -> Self {
        Self {
            root,
            chat: None,
            chat_focused: false,
            chat_fronted: false,
            chat_minimized: false,
            drawer: None,
            dock: None,
        }
    }

    pub(crate) fn dock(&self) -> Option<&crate::dock::Dock> {
        self.dock.as_ref()
    }

    pub(crate) fn perform_dock(
        &mut self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        command: imba::dyn_view::DynCommand,
        fx: &mut imba::effect::Effects<'_, imba::dyn_view::DynCommand>,
    ) {
        if let Some(dock) = &mut self.dock {
            imba::dyn_view::DynView::perform_dyn(dock, store, ui, command, fx);
        }
    }

    pub(crate) fn dock_mut(&mut self) -> &mut Option<crate::dock::Dock> {
        &mut self.dock
    }

    pub(crate) fn settle_dock(&mut self) {
        if let Some(dock) = &mut self.dock {
            if dock.target_width() > 0.0 {
                dock.settle();
            }
        }
    }

    pub fn chat(&self) -> Option<&ChatColumn> {
        self.chat.as_ref()
    }

    pub(crate) fn chat_mut(&mut self) -> Option<&mut ChatColumn> {
        self.chat.as_mut()
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn chat_focused(&self) -> bool {
        self.chat_focused && self.chat.is_some()
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn focus_chat(&mut self, focus: bool) {
        self.chat_focused = focus && self.chat.is_some();
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn chat_fronted(&self) -> bool {
        self.chat_fronted && self.chat.is_some()
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn chat_minimized(&self) -> bool {
        self.chat_minimized && self.chat.is_some()
    }

    /// The window's drawer opened (on its side) or closed — the
    /// band's words read it.
    pub fn set_drawer(&mut self, drawer: Option<DrawerSide>) {
        self.drawer = drawer;
    }

    /// The Sessions drawer stands over the chat column.
    pub fn side_open(&self) -> bool {
        self.drawer == Some(DrawerSide::Left)
    }

    /// The Contents drawer stands over the side column.
    pub fn contents_open(&self) -> bool {
        self.drawer == Some(DrawerSide::Right)
    }

    /// The window's columns at this size — the ONE derivation the
    /// toolbar band's sections and the body's columns both lay
    /// against (`columns::Columns`): the dock's revealed width from
    /// the right, the chat's from the left, the main area between.
    pub fn columns(&self, store: &Store, size: Size) -> Columns {
        let theme = ::editor::env::Themes::of(store);
        let window = theme.ui().window.clone();
        let band = theme.ui().toolbar.height;
        let dock = self.dock.as_ref().map(|dock| {
            let revealed = dock.revealed().round().clamp(0.0, size.width);
            DockSpan {
                span: Span {
                    x: size.width - revealed,
                    width: dock.width().max(1.0),
                },
                revealed,
            }
        });
        let available = (size.width - dock.map_or(0.0, |dock| dock.revealed)).max(1.0);
        // The chat shows WHOLE over the main area when the tree is
        // vacant, or when the window is too narrow for both and the
        // chat was fronted; BESIDE the tree when both fit; otherwise
        // it hides. Rolled away, it hides whatever the room.
        let fits_both = chat_column_engaged(available, &window);
        let whole = self.root.is_vacant() || (self.chat_fronted() && !fits_both);
        let column_width = |share: f32| match whole {
            true => available,
            false => chat_column_width(share, available, &window),
        };
        let chat = self
            .chat
            .as_ref()
            .filter(|_| !self.chat_minimized && (whole || fits_both))
            .map(|chat| {
                let width = column_width(chat.share).round().max(1.0);
                let revealed = (width * chat.revealed()).round();
                ChatSpan {
                    span: Span {
                        x: revealed - width,
                        width,
                    },
                    revealed,
                    stance: match whole {
                        true => ChatStance::Whole,
                        false => ChatStance::Beside,
                    },
                }
            });
        let chat_revealed = chat.map_or(0.0, |chat| chat.revealed);
        let main = Span {
            x: chat_revealed,
            width: (available - chat_revealed).max(0.0),
        };
        // The drawer covers the chat column — or where it would
        // stand while the chat hides. Whole pixels: its edge rule
        // lands crisp.
        let drawer = Span {
            x: 0.0,
            width: match chat {
                Some(chat) => chat.span.width,
                None => column_width(CHAT_COLUMN_RATIO).round().max(1.0),
            },
        };
        let right_width = dock
            .map_or(window.side_width, |dock| dock.span.width)
            .round()
            .max(1.0);
        let right_drawer = Span {
            x: size.width - right_width,
            width: right_width,
        };
        Columns {
            size,
            band,
            chat,
            main,
            dock,
            drawer,
            right_drawer,
        }
    }

    pub(crate) fn restore_chat(&mut self) {
        self.chat_minimized = false;
        if let Some(chat) = &mut self.chat {
            chat.show();
        }
    }

    pub(crate) fn front_chat_over_panels(&mut self) {
        self.chat_fronted = self.chat.is_some();
    }

    /// A panel landing in the tree takes the window back: the chat
    /// yields the keyboard and any fronting.
    pub(crate) fn yield_chat(&mut self) {
        self.chat_focused = false;
        self.chat_fronted = false;
    }

    /// Pin a panel as the dedicated left column.
    pub(crate) fn dock_chat(&mut self, panel: Panel) {
        self.chat = Some(ChatColumn::of(panel));
        self.chat_focused = true;
    }
}

impl View for Workbench {
    type Command = WorkbenchCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, WorkbenchCommand> {
        let root = self.root.focus_data(store, ui).map(WorkbenchCommand::Node);
        match &self.chat {
            Some(chat) => {
                let chat_takes = self.chat_focused || self.root.is_vacant();
                let chat = chat.node.focus_data(store, ui).map(WorkbenchCommand::Chat);
                match chat_takes {
                    true => chat.merge_over(root),
                    false => root.merge_over(chat),
                }
            }
            None => root,
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            WorkbenchCommand::Node(command) => fx.scope(WorkbenchCommand::Node, |fx| {
                self.root.perform(store, ui, command, fx)
            }),
            WorkbenchCommand::Chat(command) => {
                if let Some(chat) = &mut self.chat {
                    fx.scope(WorkbenchCommand::Chat, |fx| {
                        chat.node.perform(store, ui, command, fx)
                    });
                }
            }
            WorkbenchCommand::ChatFocus(to_chat, then) => {
                self.chat_focused = to_chat && self.chat.is_some();
                if let Some(then) = then {
                    self.perform(store, ui, *then, fx);
                }
            }
            WorkbenchCommand::ChatBeginResize => {
                if let Some(chat) = &mut self.chat {
                    chat.resizing = true;
                }
            }
            WorkbenchCommand::ChatResize(share) => {
                if let Some(chat) = &mut self.chat {
                    chat.share = share.clamp(0.05, 0.95);
                }
            }
            WorkbenchCommand::ChatEndResize => {
                if let Some(chat) = &mut self.chat {
                    chat.resizing = false;
                }
            }
            WorkbenchCommand::RunCommand(id) => {
                let run = crate::registry::Registry::of(store)
                    .and_then(|registry| registry.run_command.clone());
                if let Some(run) = run {
                    run(store, id);
                }
            }
            WorkbenchCommand::MaximizeTree => {
                // The column rolls away; it reads MINIMIZED once the
                // slide lands (`ChatTick`). The keyboard leaves now.
                self.chat_fronted = false;
                self.chat_focused = false;
                if let Some(chat) = &mut self.chat {
                    chat.hide();
                }
            }
            WorkbenchCommand::RestoreChat => {
                self.chat_minimized = false;
                self.chat_fronted = self.chat.is_some();
                self.chat_focused = self.chat.is_some();
                if let Some(chat) = &mut self.chat {
                    chat.show();
                }
            }
            WorkbenchCommand::ChatTick(now) => {
                if let Some(chat) = &mut self.chat {
                    chat.reveal.advance(now);
                    if chat.closing && !chat.reveal.running() {
                        chat.closing = false;
                        self.chat_minimized = true;
                    }
                }
            }
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        WorkbenchFrame {
            workbench: self,
            store,
            ui,
        }
    }
}

/// The main area's header WORDS: the focused editor's file name
/// and its directory (the location's segments, " / " between),
/// otherwise the panel's own title and subtitle.
pub(crate) fn tree_header_words(workbench: &Workbench, store: &Store) -> (String, Option<String>) {
    let slot = workbench.root.focused_slot();
    if let Some(pane) = slot.panel.editor() {
        let (documents, document) = (pane.content().documents(), pane.content().document());
        if let Some(location) = documents::OpenDocuments::location(store, documents, document) {
            if let Some((name, dirs)) = location.path().split_last() {
                let dirs = (!dirs.is_empty()).then(|| dirs.join(" / "));
                return (name.clone(), dirs);
            }
        }
    }
    let panel = workbench.root.focused_pane();
    (panel.title(store), panel.subtitle(store))
}

/// The workbench BODY under the toolbar band: the chat column, the
/// split tree and the splitter between them, laid against the
/// window's columns (the dock is the window's own layer, at the
/// same columns). The band itself is the window toolbar's.
struct WorkbenchFrame<'a> {
    workbench: &'a Workbench,
    store: &'a Store,
    ui: &'a UiCtx,
}

impl imba::layout::LayoutValue for WorkbenchFrame<'_> {}

impl<'a> imba::layout::Layout<'a, WorkbenchCommand> for WorkbenchFrame<'a> {
    fn layout(
        self,
        arena: &'a Arena,
        constraints: Constraints,
    ) -> imba::ThunkBox<'a, WorkbenchCommand> {
        let WorkbenchFrame {
            workbench,
            store,
            ui,
        } = self;
        let size = constraints.max;
        let theme = ::editor::env::Themes::of(store);
        let ground = theme.ui().ink.ground.0;
        let window = theme.ui().window.clone();
        let divider_width = window.divider_width.max(1.0);
        let divider_color = window.divider.0;

        let columns = workbench.columns(store, size);
        let top = columns.body_top();
        let height = columns.body_height();

        let mut container = imba::container::container(arena, size);
        let mut chat_rect = Rect::default();
        let mut root_rect = Rect::default();

        // The main area: the split tree, from where the chat ends to
        // where the dock begins. A whole chat leaves it no room.
        if columns.main.width >= 1.0 {
            let root = imba::layout::Layout::layout(
                workbench.root.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(columns.main.width, height)),
            )
            .map(WorkbenchCommand::Node)
            .focus_scope(!workbench.chat_focused());
            container.place(columns.main.x, top, root);
            root_rect = columns.body_rect(columns.main);
        }

        if let (Some(chat), Some(column)) = (workbench.chat.as_ref(), columns.chat) {
            // The column lays at its FULL width and slides: mid-slide
            // it stands past the window's left edge (the dock's move,
            // mirrored) and the tree takes the rest from where it shows.
            let beside = column.stance == ChatStance::Beside;
            let lays = match beside {
                true => column.span.width - divider_width,
                false => column.span.width,
            }
            .max(1.0);
            let view = imba::layout::Layout::layout(
                chat.node.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(lays, height)),
            )
            .map(WorkbenchCommand::Chat)
            .focus_scope(workbench.chat_focused());
            container.place(column.span.x, top, view);
            chat_rect = Rect::from_xywh(0.0, top, column.revealed, height);

            if beside {
                // The splitter: one hairline, band to floor — the
                // toolbar's section rule continues it — with the
                // resize handle astride it.
                let edge = columns.main.x;
                container.place_boxed(
                    edge - divider_width,
                    top,
                    hikit::ui::rule_v::<WorkbenchCommand>(divider_color, height)
                        .layout(arena, Constraints::tight(Size::new(divider_width, height))),
                );
                let sliding = chat.sliding();
                let resizing = chat.resizing;
                let available = columns.main.right();
                let handle = imba::leaf::leaf::<WorkbenchCommand>(HANDLE_REACH * 2.0, height)
                    .event(move |_arena, event, _leaf_size| match event {
                        Event::AnimationClock { now } if sliding => {
                            EventResult::command(WorkbenchCommand::ChatTick(*now))
                        }
                        Event::MouseDown { .. } if !sliding => {
                            EventResult::command(WorkbenchCommand::ChatBeginResize)
                        }
                        Event::MouseDrag { point, .. } if resizing => {
                            let width = (point.x + (edge - HANDLE_REACH))
                                .clamp(window.chat_min_width, chat_column_max(available, &window));
                            // The STATE is the proportion, not the pixel.
                            EventResult::command(WorkbenchCommand::ChatResize(width / available))
                        }
                        Event::MouseUp { .. } if resizing => {
                            EventResult::command(WorkbenchCommand::ChatEndResize)
                        }
                        _ => EventResult::Ignored,
                    });
                container.place(edge - HANDLE_REACH, top, handle);
            }
        }

        // The chat's slide needs a clock WHEREVER the chat is held —
        // showing, sliding or hidden — so a roll-away lands as
        // minimized even when no column visibly moves.
        if workbench.chat.as_ref().is_some_and(ChatColumn::sliding) {
            let clock = imba::leaf::leaf::<WorkbenchCommand>(1.0, 1.0).event(
                move |_arena, event, _size| match event {
                    Event::AnimationClock { now } => {
                        EventResult::command(WorkbenchCommand::ChatTick(*now))
                    }
                    _ => EventResult::Ignored,
                },
            );
            container.place(0.0, top, clock);
        }

        imba::ThunkBox::new(
            arena,
            ColumnsWidget {
                background: ground,
                panes: container,
                chat_rect,
                root_rect,
                chat_focused: workbench.chat_focused(),
            },
        )
    }
}

/// The two-region wrapper: paints the window background, then flips
/// the keyboard between the chat column and the tree on click — the
/// same move `SplitView` makes between its panes.
struct ColumnsWidget<'a> {
    background: skia_safe::Color,
    panes: imba::container::Container<'a, WorkbenchCommand>,
    chat_rect: Rect,
    root_rect: Rect,
    chat_focused: bool,
}

impl<'a> Thunk<'a, WorkbenchCommand> for ColumnsWidget<'a> {
    fn size(&self) -> Size {
        Thunk::size(&self.panes)
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> imba::WidgetBox<'a, WorkbenchCommand> {
        let ColumnsWidget {
            background,
            panes,
            chat_rect,
            root_rect,
            chat_focused,
        } = self;
        imba::WidgetBox::new(
            arena,
            RealizedColumns {
                background,
                panes: panes.realize_into(viewport),
                chat_rect,
                root_rect,
                chat_focused,
            },
        )
    }
}

struct RealizedColumns<'a> {
    background: skia_safe::Color,
    panes: imba::container::RealizedContainer<'a, WorkbenchCommand>,
    chat_rect: Rect,
    root_rect: Rect,
    chat_focused: bool,
}

impl<'a> imba::Widget<'a, WorkbenchCommand> for RealizedColumns<'a> {
    fn size(&self) -> Size {
        imba::Widget::size(&self.panes)
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, WorkbenchCommand>> {
        self.panes.overlays()
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, WorkbenchCommand>
    where
        'a: 'w,
    {
        self.panes.layout_data(target)
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<WorkbenchCommand> {
        match event {
            Event::Paint { canvas, .. } => {
                canvas.clear(self.background);
                self.panes.handle_event(arena, event, viewport)
            }
            Event::MouseDown { point, .. } => {
                let target = if self.chat_rect.contains(*point) {
                    Some(true)
                } else if self.root_rect.contains(*point) {
                    Some(false)
                } else {
                    None
                };
                let result = self.panes.handle_event(arena, event, viewport);
                match (result, target) {
                    (result, None) => result,
                    (result, Some(to_chat)) if to_chat == self.chat_focused => result,
                    (EventResult::Handled(mut handled), Some(to_chat))
                        if !handled.commands.is_empty() =>
                    {
                        EventResult::command(WorkbenchCommand::ChatFocus(
                            to_chat,
                            Some(Box::new(handled.commands.remove(0))),
                        ))
                    }
                    (_, Some(to_chat)) => {
                        EventResult::command(WorkbenchCommand::ChatFocus(to_chat, None))
                    }
                }
            }
            _ => self.panes.handle_event(arena, event, viewport),
        }
    }
}

pub fn pane_width(store: &Store, pane: &crate::workbench_node::EditorPane) -> Option<f32> {
    let view = pane.content();
    Some(
        documents::OpenDocuments::document_ref(store, view.documents(), view.document())?
            .layout_width(view.editor()),
    )
}

pub fn panel_width(store: &Store, panel: &Panel) -> Option<f32> {
    pane_width(store, panel.editor()?)
}

pub fn fallback_pane_editor_width(store: &Store) -> f32 {
    let theme = ::editor::env::Themes::of(store);
    let ui = theme.ui();
    (ui.window.first_pane_width - ui.editor_gutter.width).max(ui.window.min_editor_width)
}
