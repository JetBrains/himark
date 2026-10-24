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
    /// The window's side drawer stands over the chat column.
    side_open: bool,

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
            side_open: false,
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

    /// The window's side drawer (Sessions) opened or closed over the
    /// chat column — the leftmost header's tabs read it.
    pub fn set_side_open(&mut self, open: bool) {
        self.side_open = open;
    }

    /// Where the chat column stands — or would stand, while the chat
    /// hides — UNDER its header, in a `size` window: the side drawer
    /// covers exactly this. The same derivation the frame lays.
    pub fn chat_column_rect(&self, store: &Store, size: Size) -> Rect {
        let theme = ::editor::env::Themes::of(store);
        let window = theme.ui().window.clone();
        let header_h = theme.ui().toolbar.height;
        let full_bleed = self.root.full_bleed();
        let geometry = match full_bleed {
            true => WorkbenchGeometry {
                x: 0.0,
                top: header_h,
                split_width: size.width,
            },
            false => workbench_geometry(size.width, size.height, &window),
        };
        let top = match full_bleed {
            true => geometry.top,
            false => geometry.top + header_h,
        };
        let engaged = chat_column_engaged(size.width, &window) && !self.chat_minimized();
        let whole = self.root.is_vacant() || (self.chat_fronted() && !engaged);
        let width = match (&self.chat, whole, engaged) {
            (Some(_), true, _) => geometry.split_width,
            (Some(chat), false, true) => {
                chat_column_width(chat.share, size.width, &window) - window.divider_width.max(1.0)
            }
            _ => chat_column_width(CHAT_COLUMN_RATIO, size.width, &window),
        };
        // Whole pixels: the column's edge rule lands crisp.
        Rect::from_xywh(
            geometry.x,
            top,
            width.max(1.0).round(),
            (size.height - top).max(1.0),
        )
    }

    pub(crate) fn restore_chat(&mut self) {
        self.chat_minimized = false;
        if let Some(chat) = &mut self.chat {
            chat.show();
        }
    }

    /// Is the chat DISPLAYED at this width, by the layout's own
    /// derivation? The global cluster shows the chat button exactly
    /// when this is false.
    pub(crate) fn chat_presented(
        &self,
        width: f32,
        window: &::editor::theme::WindowChrome,
    ) -> bool {
        if self.chat.is_none() || self.root.full_bleed() {
            return false;
        }
        if self.root.is_vacant() {
            return true;
        }
        let engaged = chat_column_engaged(width, window) && !self.chat_minimized();
        engaged || (self.chat_fronted() && !engaged)
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

#[derive(Clone, Copy)]
pub struct WorkbenchGeometry {
    pub x: f32,
    pub top: f32,
    pub split_width: f32,
}

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub fn workbench_geometry(
    width: f32,
    height: f32,
    window: &::editor::theme::WindowChrome,
) -> WorkbenchGeometry {
    let margin = (width * window.margin_ratio).clamp(window.margin_min, window.margin_max);
    WorkbenchGeometry {
        x: margin,
        top: (height * window.top_ratio).clamp(window.top_min, window.top_max),
        split_width: (width - margin * 2.0).max(1.0),
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

/// The tree column's header WORDS: the focused editor's file name
/// and its directory (the location's segments, " / " between),
/// otherwise the panel's own title and subtitle.
fn tree_header_words(workbench: &Workbench, store: &Store) -> (String, Option<String>) {
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

/// The registered words a header shows at its trailing edge.
struct HeaderWords {
    /// The chat column's actions (Sessions) — the display rank.
    session_actions: Vec<(&'static str, &'static str)>,
    /// The focused file's action (Contents) — the quiet rank.
    file_actions: Vec<(&'static str, &'static str)>,
    /// The chat's own word, shown by the tree while the chat hides.
    chat_word: Vec<(&'static str, &'static str)>,
    /// The side column's tabs, shown dim while the dock is closed.
    tabs: Vec<(&'static str, &'static str)>,
}

impl HeaderWords {
    fn of(store: &Store, dock_open: bool) -> Self {
        let buttons = crate::toolbar::ToolbarButtons::of(store);
        let drawer = crate::registry::Registry::of(store)
            .and_then(|registry| registry.drawer_button.as_ref())
            .map(|button| (button.label, button.command));
        let word = |button: &crate::toolbar::ToolbarButton| (button.label, button.command);
        Self {
            session_actions: buttons
                .on(crate::toolbar::ToolbarSide::Left)
                .filter(|button| drawer.is_none_or(|(_, command)| command != button.command))
                .map(word)
                .collect(),
            file_actions: drawer.into_iter().collect(),
            chat_word: buttons.on(crate::toolbar::ToolbarSide::Well).map(word).collect(),
            tabs: match dock_open {
                true => Vec::new(),
                false => buttons.tabs().into_iter().map(word).collect(),
            },
        }
    }
}

type Header<'a> = hikit::header::ColumnHeader<'a, WorkbenchCommand>;

fn run(command: &'static str) -> impl Fn() -> WorkbenchCommand {
    move || WorkbenchCommand::RunCommand(command)
}

/// The LEFTMOST column's tabs, right after the semaphore: Sessions
/// (the drawer over the chat column) and Chat — the standing one in
/// the title rank, the other dim. Whichever column is leftmost
/// carries them: the chat's, or the tree's while the chat hides.
fn column_tabs<'a>(
    mut header: Header<'a>,
    words: &HeaderWords,
    workbench: &Workbench,
    chat_showing: bool,
) -> Header<'a> {
    let side_open = workbench.side_open;
    for (label, command) in &words.session_actions {
        header = header.tab(*label, side_open, run(command));
    }
    // The Chat tab TOGGLES: standing, it rolls the column away;
    // otherwise it fronts the chat (the registered `chat.composer`,
    // which also rolls an open drawer away).
    for (label, command) in &words.chat_word {
        let command: &'static str = command;
        let standing = chat_showing && !side_open;
        header = header.tab(*label, standing, move || match standing {
            true => WorkbenchCommand::MaximizeTree,
            false => WorkbenchCommand::RunCommand(command),
        });
    }
    header
}

/// The chat column's header: the tabs, then the session and where
/// it works — unless the drawer stands over the column, when the
/// Sessions tab says it all.
fn chat_header<'a>(
    arena: &'a Arena,
    store: &'a Store,
    ui: &'a UiCtx,
    workbench: &Workbench,
    chat: &ChatColumn,
    words: &HeaderWords,
    with_tabs: bool,
) -> Header<'a> {
    let header = Header::new(arena, store, ui, crate::toolbar::leading_inset(store, ui));
    let mut header = column_tabs(header, words, workbench, true);
    if !workbench.side_open {
        header = header.title(chat.panel().title(store));
        if let Some(subtitle) = chat.panel().subtitle(store) {
            header = header.subtitle(format!("in {subtitle}"));
        }
    }
    if with_tabs {
        for (label, command) in &words.tabs {
            header = header.dim_button(*label, run(command));
        }
    }
    header
}

/// The tree column's header: the focused file, its directory, the
/// file's actions and — while the dock is closed — the side column's
/// tabs. LEFTMOST (no chat column standing), it insets past the
/// semaphore and carries the Sessions · Chat tabs the chat header
/// would.
fn tree_header<'a>(
    arena: &'a Arena,
    store: &'a Store,
    ui: &'a UiCtx,
    workbench: &Workbench,
    words: &HeaderWords,
    leftmost: bool,
) -> Header<'a> {
    let (title, subtitle) = tree_header_words(workbench, store);
    let window = ::editor::env::Themes::of(store).ui().window.clone();
    let lead = match leftmost {
        true => crate::toolbar::leading_inset(store, ui).max(window.code_text_edge),
        false => window.code_text_edge,
    };
    let mut header = Header::new(arena, store, ui, lead);
    if leftmost {
        header = column_tabs(header, words, workbench, false);
    }
    header = header.title(title);
    if let Some(subtitle) = subtitle {
        header = header.subtitle(subtitle);
    }
    for (label, command) in &words.file_actions {
        header = header.button(*label, run(command));
    }
    for (label, command) in &words.tabs {
        header = header.dim_button(*label, run(command));
    }
    header
}

/// The workbench frame, reified: the chat column (when engaged)
/// beside the root split, placed by the window's geometry rule over
/// the cleared ground.
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

        let full_bleed = workbench.root.full_bleed();
        let chat = workbench.chat.as_ref().filter(|_| !full_bleed);

        let geometry = match full_bleed {
            // Full-bleed covers the workbench, not the window's top
            // band: the semaphore keeps its corner.
            true => WorkbenchGeometry {
                x: 0.0,
                top: theme.ui().toolbar.height,
                split_width: size.width,
            },
            false => workbench_geometry(size.width, size.height, &theme.ui().window),
        };
        let split_height = (size.height - geometry.top).max(1.0);

        // Every COLUMN draws its own header. A full-bleed root (the
        // composer) owns the window whole.
        let header_h = match full_bleed {
            true => 0.0,
            false => theme.ui().toolbar.height,
        };
        let dock_open = workbench
            .dock()
            .is_some_and(|dock| dock.target_width() > 0.0);
        let words = HeaderWords::of(store, dock_open);
        let header_constraints = |width: f32| {
            Constraints::tight(Size::new(width.max(1.0), header_h.max(1.0)))
        };
        let page = move |container: imba::container::Container<'a, WorkbenchCommand>| {
            imba::ThunkBox::new(
                arena,
                container.paint_below(move |_arena, canvas, _| {
                    canvas.clear(ground);
                }),
            )
        };

        // The chat column's slide needs a clock WHEREVER the chat is
        // held — whole, hidden or beside the tree — so a roll-away
        // lands as minimized even when no column visibly moves.
        let slide_clock = move |container: &mut imba::container::Container<'a, WorkbenchCommand>| {
            let clock = imba::leaf::leaf::<WorkbenchCommand>(1.0, 1.0).event(
                move |_arena, event, _size| match event {
                    Event::AnimationClock { now } => {
                        EventResult::command(WorkbenchCommand::ChatTick(*now))
                    }
                    _ => EventResult::Ignored,
                },
            );
            container.place(0.0, geometry.top, clock);
        };

        let Some(chat) = chat else {
            let root = imba::layout::Layout::layout(
                workbench.root.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(
                    geometry.split_width,
                    (split_height - header_h).max(1.0),
                )),
            )
            .map(WorkbenchCommand::Node);

            let mut container = imba::container::container(arena, size);
            if header_h > 0.0 {
                container.place_boxed(
                    geometry.x,
                    geometry.top,
                    tree_header(arena, store, ui, workbench, &words, true)
                        .layout(arena, header_constraints(geometry.split_width)),
                );
            }
            container.place(geometry.x, geometry.top + header_h, root);
            return page(container);
        };

        // The chat slot is ALWAYS open; what shows is decided here,
        // from the space and the tree alone:
        //   tree vacant          -> the chat owns the whole workbench
        //   both fit             -> chat column | split tree
        //   too narrow for both  -> the tree alone, the chat hidden
        let engaged =
            chat_column_engaged(size.width, &theme.ui().window) && !workbench.chat_minimized();
        if workbench.root.is_vacant() || (workbench.chat_fronted() && !engaged) {
            let column = imba::layout::Layout::layout(
                chat.node.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(
                    geometry.split_width,
                    (split_height - header_h).max(1.0),
                )),
            )
            .map(WorkbenchCommand::Chat);

            let mut container = imba::container::container(arena, size);
            container.place_boxed(
                geometry.x,
                geometry.top,
                chat_header(arena, store, ui, workbench, chat, &words, true)
                    .layout(arena, header_constraints(geometry.split_width)),
            );
            container.place(geometry.x, geometry.top + header_h, column);
            if chat.sliding() {
                slide_clock(&mut container);
            }
            return page(container);
        }

        if !engaged {
            let root = imba::layout::Layout::layout(
                workbench.root.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(
                    geometry.split_width,
                    (split_height - header_h).max(1.0),
                )),
            )
            .map(WorkbenchCommand::Node);

            let mut container = imba::container::container(arena, size);
            if header_h > 0.0 {
                // The chat is hidden: its tab brings it back.
                container.place_boxed(
                    geometry.x,
                    geometry.top,
                    tree_header(arena, store, ui, workbench, &words, true)
                        .layout(arena, header_constraints(geometry.split_width)),
                );
            }
            container.place(geometry.x, geometry.top + header_h, root);
            if chat.sliding() {
                slide_clock(&mut container);
            }
            return page(container);
        }

        let column_width = chat_column_width(chat.share, size.width, &theme.ui().window);
        let divider_width = theme.ui().window.divider_width.max(1.0);
        let divider_color = theme.ui().window.divider.0;

        // The slide (the dock's, mirrored): the column lays at its
        // full width but stands `slide` past the window's left edge,
        // and the tree takes the rest from where it shows.
        let revealed = (column_width * chat.revealed()).round();
        let slide = column_width - revealed;
        let chat_x = geometry.x - slide;
        let chat_rect = Rect::from_xywh(geometry.x, geometry.top, revealed, split_height);
        let root_x = geometry.x + revealed;
        let root_width = (geometry.split_width - revealed).max(1.0);
        let root_rect = Rect::from_xywh(root_x, geometry.top, root_width, split_height);
        let sliding = chat.sliding();

        let column = imba::layout::Layout::layout(
            chat.node.display(arena, store, ui),
            arena,
            Constraints::tight(Size::new(
                (column_width - divider_width).max(1.0),
                (split_height - header_h).max(1.0),
            )),
        )
        .map(WorkbenchCommand::Chat)
        .focus_scope(workbench.chat_focused());

        let root = imba::layout::Layout::layout(
            workbench.root.display(arena, store, ui),
            arena,
            Constraints::tight(Size::new(root_width, (split_height - header_h).max(1.0))),
        )
        .map(WorkbenchCommand::Node)
        .focus_scope(!workbench.chat_focused());

        let resizing = chat.resizing;
        let surface_width = size.width;
        let window = theme.ui().window.clone();
        let handle = imba::leaf::leaf::<WorkbenchCommand>(HANDLE_REACH * 2.0, split_height).event(
            move |_arena, event, _leaf_size| match event {
                Event::AnimationClock { now } if sliding => {
                    EventResult::command(WorkbenchCommand::ChatTick(*now))
                }
                Event::MouseDown { .. } if !sliding => {
                    EventResult::command(WorkbenchCommand::ChatBeginResize)
                }
                Event::MouseDrag { point, .. } if resizing => {
                    let width = (point.x + (root_x - HANDLE_REACH))
                        .clamp(window.chat_min_width, chat_column_max(surface_width, &window));
                    // The STATE is the proportion, not the pixel.
                    EventResult::command(WorkbenchCommand::ChatResize(width / surface_width))
                }
                Event::MouseUp { .. } if resizing => {
                    EventResult::command(WorkbenchCommand::ChatEndResize)
                }
                _ => EventResult::Ignored,
            },
        );

        let mut container = imba::container::container(arena, size);
        // Each column's OWN header: the chat names its session (the
        // leftmost, so it insets past the semaphore), the tree names
        // its focused file.
        container.place_boxed(
            chat_x,
            geometry.top,
            chat_header(arena, store, ui, workbench, chat, &words, false)
                .layout(arena, header_constraints(column_width - divider_width)),
        );
        container.place_boxed(
            root_x,
            geometry.top,
            tree_header(arena, store, ui, workbench, &words, false)
            .layout(arena, header_constraints(root_width)),
        );
        container.place(chat_x, geometry.top + header_h, column);
        // The column divider: one hairline, header to floor.
        container.place_boxed(
            root_x - divider_width,
            geometry.top,
            hikit::ui::rule_v::<WorkbenchCommand>(divider_color, split_height).layout(
                arena,
                Constraints::tight(Size::new(divider_width, split_height)),
            ),
        );
        container.place(root_x, geometry.top + header_h, root);
        container.place(root_x - HANDLE_REACH, geometry.top, handle);
        if sliding {
            slide_clock(&mut container);
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
