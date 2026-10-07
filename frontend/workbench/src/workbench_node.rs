// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use hikit::{panel::DynPanelView, panel::PanelView};

use editor::editor_view::EditorCommand;

use documents::entity_view::EditorIdView;
use imba::{
    arena::Arena,
    constraints::Constraints,
    scroll::{ScrollCommand, ScrollView},
    split::{Arrangement, Pane, SplitCommand, SplitView},
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View,
};

#[derive(Clone)]
pub(crate) struct ClosedPanel;

impl imba::View for ClosedPanel {
    type Command = imba::dyn_view::DynCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        _store: &'a Store,
        _ui: &'a imba::ui::UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::Fill::new()
    }
}

impl PanelView for ClosedPanel {
    type Place = hikit::navigation::NoPlace;

    fn title(&self, _store: &Store) -> String {
        String::new()
    }

    fn dismantle(&mut self, _store: &mut Store) {}

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type EditorPane = ScrollView<EditorIdView>;
pub type PaneCommand = ScrollCommand<EditorCommand>;

pub enum Panel {
    Editor(EditorPane),

    Plugin(Box<dyn DynPanelView>),
}

impl Clone for Panel {
    fn clone(&self) -> Self {
        match self {
            Self::Editor(pane) => Self::Editor(pane.clone()),
            Self::Plugin(view) => Self::Plugin(view.clone_panel()),
        }
    }
}

#[derive(Clone)]
pub enum PanelCommand {
    Editor(PaneCommand),
    Plugin(imba::dyn_view::DynCommand),
}

impl std::fmt::Display for PanelCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PanelCommand::Editor(command) => command.fmt(out),
            PanelCommand::Plugin(command) => command.fmt(out),
        }
    }
}

impl Panel {
    pub(crate) fn full_bleed(&self) -> bool {
        match self {
            Self::Editor(_) => false,
            Self::Plugin(view) => view.full_bleed(),
        }
    }

    pub fn editor(&self) -> Option<&EditorPane> {
        match self {
            Self::Editor(pane) => Some(pane),
            _ => None,
        }
    }

    pub fn dismantle(&mut self, store: &mut Store) {
        if let Self::Plugin(view) = self {
            view.dismantle(store);
        }
    }

    /// The panel left its slot and is about to be dropped (see
    /// `PanelView::displaced`).
    pub fn displaced(&mut self, store: &mut Store) {
        if let Self::Plugin(view) = self {
            view.displaced(store);
        }
    }

    pub fn scroll_y(&self) -> f32 {
        match self {
            Self::Editor(pane) => pane.scroll_y(),
            Self::Plugin(_) => 0.0,
        }
    }

    pub fn set_scroll_y(&mut self, scroll_y: f32) {
        match self {
            Self::Editor(pane) => pane.set_scroll_y(scroll_y),
            Self::Plugin(_) => {}
        }
    }

    pub fn title(&self, store: &Store) -> String {
        match self {
            Self::Editor(pane) => {
                let documents = pane.content().documents();
                let document = pane.content().document();
                let name = documents::OpenDocuments::name(store, documents, document)
                    .unwrap_or_else(|| "untitled".to_owned());
                // The unsaved mark rides the omnibox title.
                match documents::OpenDocuments::entity(store, documents, document)
                    .is_some_and(|entity| entity.modified())
                {
                    true => format!("{name}*"),
                    false => name,
                }
            }
            Self::Plugin(view) => view.title(store),
        }
    }

    pub(crate) fn blank() -> Panel {
        Panel::Plugin(Box::new(ClosedPanel))
    }

    pub(crate) fn is_blank(&self) -> bool {
        match self {
            Self::Plugin(view) => view.as_any().is::<ClosedPanel>(),
            Self::Editor(_) => false,
        }
    }

    pub fn drawer_view(
        &self,
        store: &Store,
        ui: &UiCtx,
        window: crate::window::WindowId,
    ) -> Option<Box<dyn hikit::modal::ModalView>> {
        match self {
            Self::Editor(pane) => {
                let view = pane.content();
                let location =
                    documents::OpenDocuments::location(store, view.documents(), view.document())?;

                let document = documents::OpenDocuments::document_ref(
                    store,
                    view.documents(),
                    view.document(),
                )?;
                if !document.has_outline() {
                    return None;
                }
                let road = crate::registry::Registry::of(store)?.outline.clone()?;
                Some(road(
                    store,
                    ui,
                    view.documents(),
                    view.document(),
                    location,
                    window,
                ))
            }
            Self::Plugin(view) => view.drawer_view_dyn(store, ui),
        }
    }

    pub(crate) fn navigation_location(
        &self,
        store: &Store,
    ) -> Option<hikit::navigation::NavigationLocation> {
        match self {
            Self::Editor(pane) => {
                let view = pane.content();
                let location =
                    documents::OpenDocuments::location(store, view.documents(), view.document())?;
                let document = documents::OpenDocuments::document_ref(
                    store,
                    view.documents(),
                    view.document(),
                )?;

                if documents::is_scratch(&location) && document.revision() == 0 {
                    return None;
                }
                let caret = document.caret_byte(view.editor());
                Some(hikit::navigation::NavigationLocation::new(
                    hikit::navigation::EditorPlace {
                        location,
                        caret,
                        scroll_y: pane.scroll_y(),
                    },
                ))
            }
            Self::Plugin(view) => view.navigation_location_dyn(store),
        }
    }

    pub(crate) fn navigate_to(
        &mut self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        target: &hikit::navigation::NavigationLocation,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        match self {
            Self::Editor(pane) => {
                let Some(place) = target.place::<hikit::navigation::EditorPlace>() else {
                    return false;
                };
                let view = *pane.content();
                if documents::OpenDocuments::location(store, view.documents(), view.document())
                    .as_ref()
                    != Some(&place.location)
                {
                    return false;
                }
                let Some(mut document) =
                    documents::OpenDocuments::document(store, view.documents(), view.document())
                else {
                    return false;
                };
                let fonts = ::editor::env::Fonts::of(store)();
                let theme = ::editor::env::Themes::of(store);
                let (documents, target) = (view.documents(), view.document());
                fx.scope(
                    move |command| {
                        imba::command::Verb::at(
                            documents,
                            documents::DocumentsCommand::Editor(target, command),
                        )
                    },
                    |fx| {
                        document.reveal_at(
                            view.editor(),
                            place.caret,
                            store,
                            ui,
                            &fonts,
                            &theme,
                            fx,
                        )
                    },
                );
                documents::OpenDocuments::put_document(
                    store,
                    view.documents(),
                    view.document(),
                    document,
                );
                pane.set_scroll_y(place.scroll_y);
                true
            }
            Self::Plugin(view) => view.navigate_to_dyn(store, target, fx),
        }
    }

    fn placeholder(&self) -> Panel {
        match self {
            Self::Editor(pane) => Self::Editor(ScrollView::new(pane.content().clone())),
            Self::Plugin(view) => Self::Plugin(view.clone_panel()),
        }
    }
}

impl View for Panel {
    type Command = PanelCommand;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match (self, command) {
            (Self::Editor(pane), PanelCommand::Editor(command)) => {
                let anchor = match &command {
                    ScrollCommand::Content(EditorCommand::Viewport { width, anchor, .. }) => {
                        let view = pane.content();
                        documents::OpenDocuments::document_ref(
                            store,
                            view.documents(),
                            view.document(),
                        )
                        .map(|document| (document.layout_width(view.editor()) - *width).abs() > 1.0)
                        .unwrap_or(false)
                    }
                    .then_some(*anchor),
                    _ => None,
                };

                let cancels_reveal = matches!(&command, ScrollCommand::SetScrollY(_));
                fx.scope(PanelCommand::Editor, |fx| {
                    pane.perform(store, ui, command, fx)
                });
                if cancels_reveal {
                    {
                        let view = *pane.content();
                        if let Some(mut document) = documents::OpenDocuments::document(
                            store,
                            view.documents(),
                            view.document(),
                        ) {
                            document.cancel_reveal(view.editor());
                            documents::OpenDocuments::put_document(
                                store,
                                view.documents(),
                                view.document(),
                                document,
                            );
                        }
                    }
                }
                if let Some(anchor) = anchor {
                    {
                        let view = *pane.content();
                        if let Some(document) = documents::OpenDocuments::document_ref(
                            store,
                            view.documents(),
                            view.document(),
                        ) {
                            let target = document.height_before(view.editor(), anchor);
                            pane.set_scroll_y(target);
                        }
                    }
                }
            }
            (Self::Plugin(view), PanelCommand::Plugin(command)) => {
                fx.scope(PanelCommand::Plugin, |fx| {
                    view.as_mut().perform_dyn(store, ui, command, fx)
                });
            }

            _ => {}
        }
    }

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, PanelCommand> {
        match self {
            Self::Editor(pane) => pane.focus_data(store, ui).map(PanelCommand::Editor),
            Self::Plugin(view) => view
                .as_ref()
                .focus_data_dyn(store, ui)
                .map(PanelCommand::Plugin),
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let content: imba::ThunkBox<'a, PanelCommand> = match self {
                Self::Editor(pane) => imba::ThunkBox::new(
                    arena,
                    imba::layout::Layout::layout(
                        pane.display(arena, store, ui),
                        arena,
                        constraints,
                    )
                    .map(PanelCommand::Editor)
                    .overlay_host(editor::sticky::HOST)
                    .overlay_host(editor::scroll_stripe::HOST),
                ),
                Self::Plugin(view) => imba::ThunkBox::new(
                    arena,
                    view.layout_dyn(arena, store, ui, constraints)
                        .map(PanelCommand::Plugin),
                ),
            };
            let mut panel = imba::container::container(arena, constraints.max);
            panel.place_boxed(0.0, 0.0, content);
            panel
        })
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum WorkbenchNode {
    Leaf(PaneSlot),
    Split(Box<SplitView<WorkbenchNode, WorkbenchNode>>),
}

impl WorkbenchNode {
    pub(crate) fn full_bleed(&self) -> bool {
        match self {
            Self::Leaf(slot) => slot.panel.full_bleed(),
            Self::Split(_) => false,
        }
    }
}

/// A workbench-minted identity for the panel occupying a leaf. Effects a
/// panel launches are routed by leaf PATH, so an async result can land
/// after a swap put a different panel in that leaf; the id the command
/// was tagged with lets the delivery drop a stale command instead of
/// mis-delivering it to whoever now sits there (docs/editor/diff-canvas.md).
/// Minted fresh whenever a leaf's occupant is replaced; it rides the slot
/// (and so travels with the panel) when the tree reshapes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PanelId(u64);

impl PanelId {
    fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// A panel bundled with its workbench identity, so the two are always
/// replaced together (a `PaneSlot` outlives its occupants — its history
/// persists across navigations — so keeping `id` beside `panel` as
/// separate fields would risk them drifting out of sync). Derefs to the
/// `Panel` so read access reads through transparently; use `PaneSlot::
/// replace_panel` to swap the occupant, which mints a fresh id.
#[derive(Clone)]
pub struct PanelWithId {
    pub(crate) id: PanelId,
    pub panel: Panel,
}

impl PanelWithId {
    fn new(panel: Panel) -> Self {
        Self {
            id: PanelId::mint(),
            panel,
        }
    }
}

impl std::ops::Deref for PanelWithId {
    type Target = Panel;
    fn deref(&self) -> &Panel {
        &self.panel
    }
}

impl std::ops::DerefMut for PanelWithId {
    fn deref_mut(&mut self) -> &mut Panel {
        &mut self.panel
    }
}

#[derive(Clone)]
pub struct PaneSlot {
    pub panel: PanelWithId,
    pub(crate) back: rpds::VectorSync<hikit::navigation::NavigationLocation>,
    pub(crate) forward: rpds::VectorSync<hikit::navigation::NavigationLocation>,

    pub(crate) pending: Option<PendingWalk>,
}

#[derive(Clone)]
pub(crate) struct PendingWalk {
    pub(crate) target: hikit::navigation::NavigationLocation,
    pub(crate) step: WalkStep,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WalkStep {
    Back,

    Forward,

    Replace,
}

impl PaneSlot {
    pub(crate) fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, PanelCommand> {
        self.panel.focus_data(store, ui)
    }

    pub(crate) fn of(panel: Panel) -> Self {
        Self {
            panel: PanelWithId::new(panel),
            back: rpds::VectorSync::new_sync(),
            forward: rpds::VectorSync::new_sync(),
            pending: None,
        }
    }

    pub(crate) fn panel_id(&self) -> PanelId {
        self.panel.id
    }

    /// Swap in a new occupant, minting it a fresh `PanelId` so any effect
    /// still in flight for the departing panel is dropped on delivery
    /// rather than mis-routed to the newcomer. Returns the displaced panel.
    pub(crate) fn replace_panel(&mut self, panel: Panel) -> Panel {
        std::mem::replace(&mut self.panel, PanelWithId::new(panel)).panel
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn history_depths(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }
}

#[derive(Clone)]
pub enum NodeCommand {
    Leaf {
        /// The occupant this command was addressed to; a leaf drops it if
        /// its current occupant no longer carries this id (a stale route
        /// after a panel swap).
        target: PanelId,
        command: PanelCommand,
    },
    Split(Box<SplitCommand<NodeCommand, NodeCommand>>),
}

impl std::fmt::Display for NodeCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeCommand::Leaf { command, .. } => command.fmt(out),
            NodeCommand::Split(command) => command.fmt(out),
        }
    }
}

/// Tag a leaf's outgoing commands with the occupant they belong to.
fn wrap_leaf(target: PanelId) -> impl Fn(PanelCommand) -> NodeCommand + Copy {
    move |command| NodeCommand::Leaf { target, command }
}

impl WorkbenchNode {
    pub fn editor_leaf(pane: EditorPane) -> Self {
        Self::Leaf(PaneSlot::of(Panel::Editor(pane)))
    }

    /// A tree with nothing open: the sole leaf holds the blank panel.
    pub fn vacant() -> Self {
        Self::Leaf(PaneSlot::of(Panel::blank()))
    }

    /// Nothing open — the chat column (when present) owns the whole
    /// workbench, and closing the last panel brings the chat back.
    #[doc(hidden)]
    pub fn is_vacant(&self) -> bool {
        matches!(self, Self::Leaf(slot) if slot.panel.panel.is_blank())
    }

    pub fn close_focused(&mut self) -> bool {
        match self {
            Self::Leaf(_) => false,
            Self::Split(split) => {
                let focused = split.focused();
                let focused_is_leaf = matches!(
                    match focused {
                        Pane::First => split.first(),
                        Pane::Second => split.second(),
                    },
                    Self::Leaf(_)
                );
                if !focused_is_leaf {
                    return match focused {
                        Pane::First => split.first_mut().close_focused(),
                        Pane::Second => split.second_mut().close_focused(),
                    };
                }

                let placeholder = Self::Leaf(PaneSlot::of(Panel::Plugin(Box::new(ClosedPanel))));
                let Self::Split(taken) = std::mem::replace(self, placeholder) else {
                    unreachable!("the enclosing match arm checked Split");
                };
                let (first, second) = taken.into_panes();
                *self = match focused {
                    Pane::First => second,
                    Pane::Second => first,
                };
                true
            }
        }
    }

    pub fn focused_pane(&self) -> &Panel {
        &self.focused_slot().panel.panel
    }

    pub(crate) fn focused_pane_mut(&mut self) -> &mut Panel {
        &mut self.focused_slot_mut().panel.panel
    }

    /// Swap the focused leaf's occupant, minting it a fresh `PanelId`.
    pub(crate) fn replace_focused_panel(&mut self, panel: Panel) -> Panel {
        self.focused_slot_mut().replace_panel(panel)
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn focused_slot(&self) -> &PaneSlot {
        match self {
            Self::Leaf(slot) => slot,
            Self::Split(split) => match split.focused() {
                Pane::First => split.first().focused_slot(),
                Pane::Second => split.second().focused_slot(),
            },
        }
    }

    pub fn focused_slot_mut(&mut self) -> &mut PaneSlot {
        match self {
            Self::Leaf(slot) => slot,
            Self::Split(split) => match split.focused() {
                Pane::First => split.first_mut().focused_slot_mut(),
                Pane::Second => split.second_mut().focused_slot_mut(),
            },
        }
    }

    pub fn for_each_pane(&self, visit: &mut impl FnMut(&Panel)) {
        match self {
            Self::Leaf(slot) => visit(&slot.panel.panel),
            Self::Split(split) => {
                split.first().for_each_pane(visit);
                split.second().for_each_pane(visit);
            }
        }
    }

    pub fn for_each_slot_mut(&mut self, visit: &mut impl FnMut(&mut PaneSlot)) {
        match self {
            Self::Leaf(slot) => visit(slot),
            Self::Split(split) => {
                split.first_mut().for_each_slot_mut(visit);
                split.second_mut().for_each_slot_mut(visit);
            }
        }
    }

    pub(crate) fn for_each_pane_mut(&mut self, visit: &mut impl FnMut(&mut Panel)) {
        match self {
            Self::Leaf(slot) => visit(&mut slot.panel.panel),
            Self::Split(split) => {
                split.first_mut().for_each_pane_mut(visit);
                split.second_mut().for_each_pane_mut(visit);
            }
        }
    }

    pub fn split_focused(&mut self, new_panel: Panel, arrangement: Arrangement, focus_new: bool) {
        match self {
            Self::Split(split) => match split.focused() {
                Pane::First => split
                    .first_mut()
                    .split_focused(new_panel, arrangement, focus_new),
                Pane::Second => split
                    .second_mut()
                    .split_focused(new_panel, arrangement, focus_new),
            },
            Self::Leaf(slot) => {
                let placeholder = new_panel.placeholder();
                let current = std::mem::replace(&mut slot.panel, PanelWithId::new(placeholder));
                let history = (slot.back.clone(), slot.forward.clone());
                let mut current_slot = PaneSlot::of(current.panel);
                current_slot.back = history.0.clone();
                current_slot.forward = history.1.clone();
                let mut new_slot = PaneSlot::of(new_panel);
                new_slot.back = history.0;
                new_slot.forward = history.1;
                let (first, second) = (Self::Leaf(current_slot), Self::Leaf(new_slot));
                let mut split = match arrangement {
                    Arrangement::Row => SplitView::row(first, second),
                    Arrangement::Column => SplitView::column(first, second),
                };
                if focus_new {
                    split.focus(Pane::Second);
                }
                *self = Self::Split(Box::new(split));
            }
        }
    }
}

impl View for WorkbenchNode {
    type Command = NodeCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, NodeCommand> {
        match self {
            Self::Leaf(slot) => slot.focus_data(store, ui).map(wrap_leaf(slot.panel_id())),
            Self::Split(split) => split
                .focus_data(store, ui)
                .map(|command| NodeCommand::Split(Box::new(command))),
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match (self, command) {
            (Self::Leaf(slot), NodeCommand::Leaf { target, command }) => {
                // Stale route: an async result addressed to a panel that
                // has since left this leaf (a swap put another there).
                // Drop it — its panel, if still alive elsewhere, re-derives.
                if slot.panel_id() != target {
                    return;
                }
                let wrap = wrap_leaf(target);
                match command {
                    command => fx.scope(wrap, |fx| {
                        slot.panel.perform(store, ui, command, fx);
                    }),
                }
            }
            (Self::Split(split), NodeCommand::Split(command)) => fx.scope(
                |command| NodeCommand::Split(Box::new(command)),
                |fx| split.perform(store, ui, *command, fx),
            ),

            _ => {}
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let widget: imba::ThunkBox<'a, NodeCommand> = match self {
                Self::Leaf(slot) => imba::ThunkBox::new(
                    arena,
                    imba::layout::Layout::layout(
                        slot.panel.display(arena, store, ui),
                        arena,
                        constraints,
                    )
                    .map(wrap_leaf(slot.panel_id())),
                ),

                Self::Split(split) => {
                    let divider = split.divider_rect(constraints.max);
                    let theme = ::editor::env::Themes::of(store);
                    let window = &theme.ui().window;
                    let color = window.divider.0;
                    let inset = window.divider_inset;
                    imba::ThunkBox::new(
                        arena,
                        imba::layout::Layout::layout(
                            split.display(arena, store, ui),
                            arena,
                            constraints,
                        )
                        .map(|command| NodeCommand::Split(Box::new(command)))
                        .paint_below(move |_arena, canvas, _| {
                            let mut paint = skia_safe::Paint::default();
                            paint.set_anti_alias(true);
                            paint.set_color(color);
                            canvas.draw_rect(divider.with_offset((-inset, 0.0)), &paint);
                        }),
                    )
                }
            };

            widget
        })
    }
}
