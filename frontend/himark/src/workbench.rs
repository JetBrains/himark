// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{arena::Arena, constraints::Constraints, event::{Event, EventResult}, store::Store, thunk_ext::ThunkExt, Thunk, ui::UiCtx, View};
use skia_safe::{Contains as _, Rect, Size};

use crate::workbench_node::{NodeCommand, PaneSlot, Panel, WorkbenchNode};

// Like every theme metric, these are PHYSICAL pixels tuned for 2x
// displays — the shell hands the engine device pixels and the scale
// is baked into the design (base font 32, toolbar 74, pane 900).

/// The dedicated chat column never shrinks below this (420pt at 2x).
pub const CHAT_COLUMN_MIN: f32 = 840.0;

/// The main area keeps at least this much beside the column (1100pt
/// at 2x) — below that there is no room for both, and the panel wins
/// the whole workbench.
pub const MAIN_COLUMN_MIN: f32 = 2200.0;

/// The column's default share of the window; dragging adjusts it
/// freely upward from `CHAT_COLUMN_MIN` — there is no maximum.
const CHAT_COLUMN_RATIO: f32 = 0.42;

const HANDLE_REACH: f32 = 4.0;

/// Wide enough for the dedicated chat column: the column minimum must
/// fit while the main area keeps a real working width — not a bare
/// editor minimum. Below this the chat lives in the split tree like
/// any other panel.
pub fn chat_column_engaged(width: f32, window: &::editor::theme::WindowChrome) -> bool {
    width >= CHAT_COLUMN_MIN + window.first_pane_width.max(MAIN_COLUMN_MIN)
}

/// The column's effective width at this window, from its RATIO
/// state: the split between chat and panel is proportion-first (the
/// share survives window resizes), bounded by the chat minimum and
/// the main area's guarantee.
pub fn chat_column_width(share: f32, width: f32) -> f32 {
    (width * share).clamp(CHAT_COLUMN_MIN, chat_column_max(width))
}

/// The policy bound, derived from the CURRENT window at every
/// layout: while the chat and the tree both show, the main area
/// keeps `MAIN_COLUMN_MIN` whatever width the chat was dragged to —
/// squeezing the window compresses the chat down to its minimum
/// first, and past that the chat hides. The stored drag width is
/// untouched; it comes back when the window does.
fn chat_column_max(width: f32) -> f32 {
    (width - MAIN_COLUMN_MIN).max(CHAT_COLUMN_MIN)
}

#[derive(Clone)]
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
}

impl ChatColumn {
    fn of(panel: Panel) -> Self {
        Self {
            node: WorkbenchNode::Leaf(PaneSlot::of(panel)),
            share: CHAT_COLUMN_RATIO,
            resizing: false,
        }
    }

    pub(crate) fn replace_panel(&mut self, panel: Panel) -> Panel {
        match &mut self.node {
            WorkbenchNode::Leaf(slot) => slot.replace_panel(panel),
            WorkbenchNode::Split(_) => unreachable!("the chat column is always a leaf"),
        }
    }

    pub(crate) fn panel(&self) -> &Panel {
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

    dock: Option<crate::dock::Dock>,
}

impl Workbench {
    pub(crate) fn new(root: WorkbenchNode) -> Self {
        Self {
            root,
            chat: None,
            chat_focused: false,
            chat_fronted: false,
            chat_minimized: false,
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

    pub(crate) fn chat(&self) -> Option<&ChatColumn> {
        self.chat.as_ref()
    }

    pub(crate) fn chat_mut(&mut self) -> Option<&mut ChatColumn> {
        self.chat.as_mut()
    }

    pub(crate) fn chat_focused(&self) -> bool {
        self.chat_focused && self.chat.is_some()
    }

    pub(crate) fn focus_chat(&mut self, focus: bool) {
        self.chat_focused = focus && self.chat.is_some();
    }

    pub(crate) fn chat_fronted(&self) -> bool {
        self.chat_fronted && self.chat.is_some()
    }

    pub(crate) fn chat_minimized(&self) -> bool {
        self.chat_minimized && self.chat.is_some()
    }

    pub(crate) fn restore_chat(&mut self) {
        self.chat_minimized = false;
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
                if let Some(command) = crate::commands::Commands::of(store).find(id).cloned() {
                    crate::commands::AppRequests::push(store, command);
                }
            }
            WorkbenchCommand::MaximizeTree => {
                self.chat_minimized = self.chat.is_some();
                self.chat_fronted = false;
                self.chat_focused = false;
            }
            WorkbenchCommand::RestoreChat => {
                self.chat_minimized = false;
                self.chat_fronted = self.chat.is_some();
                self.chat_focused = self.chat.is_some();
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

/// The tree header's title: the focused editor's FULL path relative
/// to the workspace (the location's segments), the plain panel title
/// otherwise.
fn tree_header_title(workbench: &Workbench, store: &Store) -> String {
    let slot = workbench.root.focused_slot();
    if let (Some(documents), Some((document, _))) = (slot.documents_id(), slot.find_target()) {
        if let Some(location) = documents::OpenDocuments::location(store, documents, document) {
            if !location.path().is_empty() {
                return location.path().join("/");
            }
        }
    }
    workbench.root.focused_pane().title(store)
}

/// The tree header's right-aligned buttons: TOC, then the chat
/// toggle chevrons — each cell ruled on its left, the group's style.
fn place_tree_buttons<'a>(
    container: &mut imba::container::Container<'a, WorkbenchCommand>,
    _arena: &'a Arena,
    theme: &::editor::theme::Theme,
    right: f32,
    top: f32,
    header_h: f32,
    toggle: Option<WorkbenchCommand>,
) {
    let chrome = theme.ui().toolbar.clone();
    let mut x = right - chrome.button_inset;
    if let Some(command) = toggle {
        x -= chrome.button_size;
        container.place(x, top, chat_toggle_button(header_h, theme, command));
    }
    x -= chrome.button_size;
    let toc = crate::toc::toolbar_button();
    let glyph = toc.glyph.clone();
    let rule = chrome.rule.0;
    let glyph_color = chrome.glyph_color.0;
    let button_size = chrome.button_size;
    let cell = imba::leaf::leaf::<WorkbenchCommand>(button_size, header_h)
        .paint_below(move |_arena, canvas, rect| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(false);
            paint.set_color(rule);
            canvas.draw_rect(
                skia_safe::Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                &paint,
            );
            paint.set_anti_alias(true);
            let square = skia_safe::Rect::from_xywh(
                rect.left,
                rect.top + (rect.height() - rect.width()) * 0.5,
                rect.width(),
                rect.width(),
            );
            let inset = (rect.width() * 0.25).max(1.0);
            glyph(canvas, square.with_inset((inset, inset)), glyph_color);
        })
        .event(|_arena, event, _size| match event {
            Event::MouseDown { .. } => {
                EventResult::Command(WorkbenchCommand::RunCommand("toc.toggle"))
            }
            _ => EventResult::Ignored,
        });
    container.place(x, top, cell);
}

/// The tree header's chat toggle: MAXIMIZE (hide the chat) when the
/// chat stands beside the tree, MINIMIZE (bring it back) when it is
/// hidden — stroked chevrons pointing out or in.
fn chat_toggle_button<'a>(
    header_h: f32,
    theme: &::editor::theme::Theme,
    command: WorkbenchCommand,
) -> impl Thunk<'a, WorkbenchCommand> + 'a {
    use imba::thunk_ext::ThunkExt;
    let chrome = theme.ui().toolbar.clone();
    let maximize = matches!(command, WorkbenchCommand::MaximizeTree);
    imba::leaf::leaf::<WorkbenchCommand>(chrome.button_size, header_h)
        .paint_below(move |_arena, canvas, rect| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(false);
            paint.set_color(chrome.rule.0);
            canvas.draw_rect(
                skia_safe::Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                &paint,
            );
            paint.set_anti_alias(true);
            paint.set_color(chrome.glyph_color.0);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.08).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let cx = rect.left + rect.width() * 0.5;
            let cy = rect.top + rect.height() * 0.5;
            let reach = rect.width() * 0.16;
            let gap = rect.width() * 0.1;
            // Two chevrons: outward = maximize, inward = restore.
            let (near, far) = match maximize {
                true => (gap, gap + reach),
                false => (gap + reach, gap),
            };
            for direction in [-1.0f32, 1.0] {
                let mut path = skia_safe::PathBuilder::new();
                path.move_to((cx + direction * near, cy - reach));
                path.line_to((cx + direction * far, cy));
                path.line_to((cx + direction * near, cy + reach));
                canvas.draw_path(&path.detach(), &paint);
            }
        })
        .event(move |_arena, event, _size| match event {
            Event::MouseDown { .. } => EventResult::Command(command.clone()),
            _ => EventResult::Ignored,
        })
}

/// The workbench frame, reified: the chat column (when engaged)
/// beside the root split, placed by the window's geometry rule over
/// the cleared background.
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

        let full_bleed = workbench.root.full_bleed();
        let chat = workbench.chat.as_ref().filter(|_| !full_bleed);

        let geometry = match full_bleed {
            // Full-bleed covers the workbench, not the window's top
            // band: the semaphore and the global cluster keep their
            // corner.
            true => WorkbenchGeometry {
                x: 0.0,
                top: theme.ui().toolbar.height,
                split_width: size.width,
            },
            false => workbench_geometry(size.width, size.height, &theme.ui().window),
        };
        let split_height = (size.height - geometry.top).max(1.0);

        // Every COLUMN draws its own header; the leftmost one insets
        // past the global cluster (semaphore + window buttons). A
        // full-bleed root (the composer) owns the window whole.
        let header_h = match full_bleed {
            true => 0.0,
            false => theme.ui().toolbar.height,
        };
        let presented = workbench.chat_presented(size.width, &theme.ui().window);
        let cluster = crate::toolbar::global_cluster_width(store, ui, !presented);
        let tree_buttons = theme.ui().toolbar.button_inset + 2.0 * theme.ui().toolbar.button_size;
        // The dock cluster mirrors the global one top-right: the
        // RIGHTMOST column header leaves room for it while the dock
        // is closed (open, the dock's own header holds the buttons).
        let dock_strip = match workbench
            .dock()
            .is_some_and(|dock| dock.target_width() > 0.0)
        {
            true => 0.0,
            false => crate::toolbar::dock_cluster_width(store),
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
                let title = tree_header_title(workbench, store);
                container.place_boxed(
                    geometry.x,
                    geometry.top,
                    crate::toolbar::column_header::<WorkbenchCommand>(
                        arena,
                        store,
                        ui,
                        geometry.split_width,
                        title,
                        cluster,
                        tree_buttons + dock_strip,
                    ),
                );
                place_tree_buttons(
                    &mut container,
                    arena,
                    &theme,
                    geometry.x + geometry.split_width - dock_strip,
                    geometry.top,
                    header_h,
                    None,
                );
            }
            container.place(geometry.x, geometry.top + header_h, root);

            return imba::ThunkBox::new(
                arena,
                container.paint_below(move |_arena, canvas, _| {
                    canvas.clear(theme.ui().window.background.0);
                }),
            );
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
            let title = chat.panel().title(store);
            container.place_boxed(
                geometry.x,
                geometry.top,
                crate::toolbar::column_header::<WorkbenchCommand>(
                    arena,
                    store,
                    ui,
                    geometry.split_width,
                    title,
                    cluster,
                    dock_strip,
                ),
            );
            container.place(geometry.x, geometry.top + header_h, column);

            return imba::ThunkBox::new(
                arena,
                container.paint_below(move |_arena, canvas, _| {
                    canvas.clear(theme.ui().window.background.0);
                }),
            );
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
            let title = tree_header_title(workbench, store);
            container.place_boxed(
                geometry.x,
                geometry.top,
                crate::toolbar::column_header::<WorkbenchCommand>(
                    arena,
                    store,
                    ui,
                    geometry.split_width,
                    title,
                    cluster,
                    tree_buttons + dock_strip,
                ),
            );
            if header_h > 0.0 {
                // The restore button only when there IS a chat.
                let toggle = workbench
                    .chat
                    .is_some()
                    .then_some(WorkbenchCommand::RestoreChat);
                place_tree_buttons(
                    &mut container,
                    arena,
                    &theme,
                    geometry.x + geometry.split_width - dock_strip,
                    geometry.top,
                    header_h,
                    toggle,
                );
            }
            container.place(geometry.x, geometry.top + header_h, root);

            return imba::ThunkBox::new(
                arena,
                container.paint_below(move |_arena, canvas, _| {
                    canvas.clear(theme.ui().window.background.0);
                }),
            );
        }

        let column_width = chat_column_width(chat.share, size.width);
        let divider_width = theme.ui().window.divider_width.max(1.0);
        let divider_color = theme.ui().window.divider.0;

        let chat_rect = Rect::from_xywh(geometry.x, geometry.top, column_width, split_height);
        let root_x = geometry.x + column_width;
        let root_width = (geometry.split_width - column_width).max(1.0);
        let root_rect = Rect::from_xywh(root_x, geometry.top, root_width, split_height);

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

        let rule = imba::leaf::leaf::<WorkbenchCommand>(divider_width, split_height).paint_instead(
            move |_arena, canvas, rect| {
                let mut paint = skia_safe::Paint::default();
                paint.set_color(divider_color);
                canvas.draw_rect(rect, &paint);
            },
        );

        let resizing = chat.resizing;
        let surface_width = size.width;
        let handle = imba::leaf::leaf::<WorkbenchCommand>(HANDLE_REACH * 2.0, split_height).event(
            move |_arena, event, _leaf_size| match event {
                Event::MouseDown { .. } => EventResult::Command(WorkbenchCommand::ChatBeginResize),
                Event::MouseDrag { point, .. } if resizing => {
                    let width = (point.x + (root_x - HANDLE_REACH))
                        .clamp(CHAT_COLUMN_MIN, chat_column_max(surface_width));
                    // The STATE is the proportion, not the pixel.
                    EventResult::Command(WorkbenchCommand::ChatResize(width / surface_width))
                }
                Event::MouseUp { .. } if resizing => {
                    EventResult::Command(WorkbenchCommand::ChatEndResize)
                }
                _ => EventResult::Ignored,
            },
        );

        let mut container = imba::container::container(arena, size);
        // Each column's OWN header: the chat names its session (the
        // leftmost, so it insets past the global cluster), the tree
        // names its focused file.
        container.place_boxed(
            geometry.x,
            geometry.top,
            crate::toolbar::column_header::<WorkbenchCommand>(
                arena,
                store,
                ui,
                (column_width - divider_width).max(1.0),
                chat.panel().title(store),
                cluster,
                0.0,
            ),
        );
        container.place_boxed(
            root_x,
            geometry.top,
            crate::toolbar::column_header::<WorkbenchCommand>(
                arena,
                store,
                ui,
                root_width,
                tree_header_title(workbench, store),
                0.0,
                tree_buttons + dock_strip,
            ),
        );
        place_tree_buttons(
            &mut container,
            arena,
            &theme,
            root_x + root_width - dock_strip,
            geometry.top,
            header_h,
            Some(WorkbenchCommand::MaximizeTree),
        );
        container.place(geometry.x, geometry.top + header_h, column);
        container.place(root_x - divider_width, geometry.top, rule);
        container.place(root_x, geometry.top + header_h, root);
        container.place(root_x - HANDLE_REACH, geometry.top, handle);

        imba::ThunkBox::new(
            arena,
            ColumnsWidget {
                background: theme.ui().window.background.0,
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
                    (EventResult::Command(command), Some(to_chat)) => EventResult::Command(
                        WorkbenchCommand::ChatFocus(to_chat, Some(Box::new(command))),
                    ),
                    (_, Some(to_chat)) => {
                        EventResult::Command(WorkbenchCommand::ChatFocus(to_chat, None))
                    }
                }
            }
            _ => self.panes.handle_event(arena, event, viewport),
        }
    }
}
