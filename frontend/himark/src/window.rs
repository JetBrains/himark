// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult},
    store::Store,
    DynView as _, Thunk, UiCtx, View, Widget,
};
use skia_safe::Size;

use imba::scroll::ScrollView;

use crate::{
    app::{panel_width, AppFx},
    Application,
};
use crate::{EditorIdView, ModalRequest, ModalView, NodeCommand, Panel, Workbench, WorkbenchNode};

pub enum WindowCommand {
    Base(NodeCommand),

    Toolbar(crate::toolbar::ToolbarCommand),

    Side(imba::DynCommand),

    Dock(imba::DynCommand),
    Modal(imba::DynCommand),

    SideFocusLost,

    Focus(LayerFocus),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayerFocus {
    Toolbar,
    Content,

    Dock,
}

#[derive(Clone)]
struct Layers {
    toolbar: crate::toolbar::Toolbar,

    focus: LayerFocus,
    workbench: Workbench,

    side: Option<crate::drawer::Drawer>,
    modal: Option<Box<dyn ModalView>>,
}

impl View for Layers {
    type Command = WindowCommand;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: WindowCommand,
        fx: &mut Effects<'_, WindowCommand>,
    ) {
        match command {
            WindowCommand::Base(command) => fx.scope(WindowCommand::Base, |fx| {
                self.workbench.perform(store, ui, command, fx)
            }),
            WindowCommand::Toolbar(command) => fx.scope(WindowCommand::Toolbar, |fx| {
                self.toolbar.perform(store, ui, command, fx)
            }),
            WindowCommand::Side(command) => {
                if let Some(side) = &mut self.side {
                    fx.scope(WindowCommand::Side, |fx| {
                        imba::DynView::perform_dyn(side, store, ui, command, fx)
                    });
                }
            }
            WindowCommand::Dock(command) => {
                fx.scope(WindowCommand::Dock, |fx| {
                    self.workbench.perform_dock(store, ui, command, fx)
                });
            }
            WindowCommand::Modal(command) => {
                if let Some(modal) = &mut self.modal {
                    fx.scope(WindowCommand::Modal, |fx| {
                        modal.as_mut().perform_dyn(store, ui, command, fx)
                    });
                }
            }
            WindowCommand::SideFocusLost => {
                if let Some(side) = &mut self.side {
                    side.focus_lost();
                }
            }
            WindowCommand::Focus(focus) => {
                self.focus = focus;
            }
        }
    }

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, WindowCommand> {
        use imba::event::EventResult;
        use imba::focus::FocusData;
        let focus = self.focus;

        let toolbar = self
            .toolbar
            .focus_data(store, ui)
            .map(WindowCommand::Toolbar);
        let modal = self.modal.as_ref().map(|modal| {
            modal
                .as_ref()
                .focus_data_dyn(store, ui)
                .map(WindowCommand::Modal)
        });
        let side = self
            .side
            .as_ref()
            .map(|side| side.focus_data_dyn(store, ui).map(WindowCommand::Side));
        let dock = self
            .workbench
            .dock()
            .map(|dock| dock.focus_data_dyn(store, ui).map(WindowCommand::Dock));
        let base = self
            .workbench
            .focus_data(store, ui)
            .map(WindowCommand::Base);
        let has_modal = modal.is_some();
        let has_side = side.is_some();

        let mut commands = Vec::new();
        let mut modal = modal.unwrap_or_default();
        let mut side = side.unwrap_or_default();
        let mut dock = dock.unwrap_or_default();
        let mut toolbar = toolbar;
        let mut base = base;
        if has_modal {
            if focus == LayerFocus::Toolbar {
                commands.append(&mut toolbar.commands);
            }
            commands.append(&mut modal.commands);
        } else {
            commands.append(&mut side.commands);
            if focus == LayerFocus::Dock {
                commands.append(&mut dock.commands);
            }
            commands.append(&mut base.commands);
            if focus != LayerFocus::Dock {
                commands.append(&mut dock.commands);
            }
        }

        fn try_key<C>(
            handler: &mut Option<imba::focus::KeyHandler<'_, C>>,
            key: imba::event::Key,
            mods: imba::event::Modifiers,
        ) -> EventResult<C> {
            match handler {
                Some(handler) => handler(key, mods),
                None => EventResult::Ignored,
            }
        }
        fn try_text<C>(
            handler: &mut Option<imba::focus::TextHandler<'_, C>>,
            text: &str,
        ) -> EventResult<C> {
            match handler {
                Some(handler) => handler(text),
                None => EventResult::Ignored,
            }
        }

        let mut key_parts = (
            modal.on_key.take(),
            side.on_key.take(),
            dock.on_key.take(),
            toolbar.on_key.take(),
            base.on_key.take(),
        );
        let on_key = Some(Box::new(move |key, mods| {
            let (modal, side, dock, toolbar, base) = (
                &mut key_parts.0,
                &mut key_parts.1,
                &mut key_parts.2,
                &mut key_parts.3,
                &mut key_parts.4,
            );

            if has_modal {
                return match try_key(modal, key, mods) {
                    EventResult::Ignored => try_key(toolbar, key, mods),
                    result => result,
                };
            }
            let mut below = |key, mods| {
                if focus == LayerFocus::Dock {
                    match try_key(dock, key, mods) {
                        EventResult::Ignored => {}
                        result => return result,
                    }
                }
                match try_key(toolbar, key, mods) {
                    EventResult::Ignored => {}
                    result => return result,
                }
                try_key(base, key, mods)
            };

            if has_side {
                match try_key(side, key, mods) {
                    EventResult::Ignored => {
                        return match below(key, mods) {
                            EventResult::Ignored => EventResult::Ignored,
                            result => {
                                result.merge(EventResult::Command(WindowCommand::SideFocusLost))
                            }
                        };
                    }
                    result => return result,
                }
            }
            below(key, mods)
        }) as imba::focus::KeyHandler<'w, WindowCommand>);

        let mut text_parts = (
            modal.on_text.take(),
            side.on_text.take(),
            dock.on_text.take(),
            toolbar.on_text.take(),
            base.on_text.take(),
        );
        let on_text = Some(Box::new(move |text: &str| {
            let (modal, side, dock, toolbar, base) = (
                &mut text_parts.0,
                &mut text_parts.1,
                &mut text_parts.2,
                &mut text_parts.3,
                &mut text_parts.4,
            );

            if focus == LayerFocus::Toolbar {
                match try_text(toolbar, text) {
                    EventResult::Ignored => {}
                    result => return result,
                }
            }
            if has_modal {
                return match try_text(modal, text) {
                    EventResult::Ignored => try_text(toolbar, text),
                    result => result,
                };
            }
            if has_side {
                match try_text(side, text) {
                    EventResult::Ignored => {}
                    result => return result,
                }
            }
            if focus == LayerFocus::Dock {
                match try_text(dock, text) {
                    EventResult::Ignored => {}
                    result => return result,
                }
            }
            match try_text(toolbar, text) {
                EventResult::Ignored => {}
                result => return result,
            }
            try_text(base, text)
        }) as imba::focus::TextHandler<'w, WindowCommand>);

        fn first<T>(seats: Vec<Option<T>>) -> Option<T> {
            seats.into_iter().flatten().next()
        }
        let (clipboard, location, seat) = {
            let t = (
                toolbar.clipboard.take(),
                toolbar.location.take(),
                toolbar.seat.take(),
            );
            let m = (
                modal.clipboard.take(),
                modal.location.take(),
                modal.seat.take(),
            );
            let sd = (
                side.clipboard.take(),
                side.location.take(),
                side.seat.take(),
            );
            let d = (
                dock.clipboard.take(),
                dock.location.take(),
                dock.seat.take(),
            );
            let ba = (
                base.clipboard.take(),
                base.location.take(),
                base.seat.take(),
            );
            let toolbar_first = focus == LayerFocus::Toolbar;
            macro_rules! route_seat {
                ($slot:tt) => {{
                    if has_modal {
                        match toolbar_first {
                            true => first(vec![t.$slot, m.$slot]),
                            false => first(vec![m.$slot, t.$slot]),
                        }
                    } else {
                        let banded = |gate: bool, seat| if gate { seat } else { None };
                        let dock_seat = banded(focus == LayerFocus::Dock, d.$slot);
                        match toolbar_first {
                            true => first(vec![t.$slot, sd.$slot, dock_seat, ba.$slot]),
                            false => first(vec![sd.$slot, dock_seat, t.$slot, ba.$slot]),
                        }
                    }
                }};
            }
            (route_seat!(0), route_seat!(1), route_seat!(2))
        };

        FocusData {
            commands,
            on_key,
            on_text,
            clipboard,
            location,
            seat,
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, WindowCommand> + imba::LayoutValue + 'a {
        WindowFrame {
            layers: self,
            store,
            ui,
        }
    }
}

fn below_layer<'a, Command: 'a>(
    arena: &'a Arena,
    size: Size,
    top: f32,
    thunk: imba::ThunkBox<'a, Command>,
) -> imba::container::Container<'a, Command> {
    let mut layer = imba::container::container(arena, size);
    layer.place_boxed(0.0, top, thunk);
    layer
}

struct LayersWidget<BaseWidget, ToolbarWidget, DynWidget> {
    base: BaseWidget,
    toolbar: ToolbarWidget,
    side: Option<DynWidget>,
    dock: Option<DynWidget>,
    modal: Option<DynWidget>,

    focus: LayerFocus,

    toolbar_height: f32,

    dock_edge_x: Option<f32>,
}

impl<'a, BaseThunk, ToolbarThunk, DynThunk> Thunk<'a, WindowCommand>
    for LayersWidget<BaseThunk, ToolbarThunk, DynThunk>
where
    BaseThunk: Thunk<'a, NodeCommand> + 'a,
    ToolbarThunk: Thunk<'a, crate::toolbar::ToolbarCommand> + 'a,
    DynThunk: Thunk<'a, imba::DynCommand> + 'a,
{
    fn size(&self) -> Size {
        self.base.size()
    }

    fn realize(
        self,
        arena: &'a Arena,
        viewport: skia_safe::Rect,
    ) -> imba::WidgetBox<'a, WindowCommand> {
        let LayersWidget {
            base,
            toolbar,
            side,
            dock,
            modal,
            focus,
            toolbar_height,
            dock_edge_x,
        } = self;

        imba::WidgetBox::new(
            arena,
            RealizedLayers {
                base: base.realize(arena, viewport),
                toolbar: toolbar.realize(arena, viewport),
                side: side.map(|side| side.realize(arena, viewport)),
                dock: dock.map(|dock| dock.realize(arena, viewport)),
                modal: modal.map(|modal| modal.realize(arena, viewport)),
                focus,
                toolbar_height,
                dock_edge_x,
            },
        )
    }
}

struct RealizedLayers<'a> {
    base: imba::WidgetBox<'a, NodeCommand>,
    toolbar: imba::WidgetBox<'a, crate::toolbar::ToolbarCommand>,
    side: Option<imba::WidgetBox<'a, imba::DynCommand>>,
    dock: Option<imba::WidgetBox<'a, imba::DynCommand>>,
    modal: Option<imba::WidgetBox<'a, imba::DynCommand>>,
    focus: LayerFocus,
    toolbar_height: f32,
    dock_edge_x: Option<f32>,
}

impl<'a> Widget<'a, WindowCommand> for RealizedLayers<'a> {
    fn size(&self) -> Size {
        Widget::size(&self.base)
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, WindowCommand>> {
        use imba::overlay::map_overlays;
        let mut overlays = map_overlays(self.base.overlays(), &WindowCommand::Base);
        overlays.append(&mut map_overlays(
            self.toolbar.overlays(),
            &WindowCommand::Toolbar,
        ));
        if let Some(side) = &mut self.side {
            overlays.append(&mut map_overlays(side.overlays(), &WindowCommand::Side));
        }
        if let Some(dock) = &mut self.dock {
            overlays.append(&mut map_overlays(dock.overlays(), &WindowCommand::Dock));
        }
        if let Some(modal) = &mut self.modal {
            overlays.append(&mut map_overlays(modal.overlays(), &WindowCommand::Modal));
        }
        overlays
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, WindowCommand>
    where
        'a: 'w,
    {
        // A DUMB fold: no copy of the region priority lives here.
        // The target key names the seat; exactly one region
        // contains it.
        let mut folded = self.base.layout_data(target).map(WindowCommand::Base);
        folded = self
            .toolbar
            .layout_data(target)
            .map(WindowCommand::Toolbar)
            .merge_over(folded);
        if let Some(side) = &mut self.side {
            folded = side
                .layout_data(target)
                .map(WindowCommand::Side)
                .merge_over(folded);
        }
        if let Some(dock) = &mut self.dock {
            folded = dock
                .layout_data(target)
                .map(WindowCommand::Dock)
                .merge_over(folded);
        }
        if let Some(modal) = &mut self.modal {
            folded = modal
                .layout_data(target)
                .map(WindowCommand::Modal)
                .merge_over(folded);
        }
        folded
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: skia_safe::Rect,
    ) -> EventResult<WindowCommand> {
        if let Event::Paint { .. }
        | Event::AnimationClock { .. }
        | Event::ThemeChanged
        | Event::UserEvent(_) = event
        {
            let scoped = |to: bool| match event {
                Event::Paint { canvas, focused } => Event::Paint {
                    canvas,
                    focused: *focused && to,
                },
                other => *other,
            };
            let mut result = self
                .base
                .handle_event(
                    arena,
                    &scoped(self.modal.is_none() && self.focus == LayerFocus::Content),
                    viewport,
                )
                .map(WindowCommand::Base);
            result = result.merge(
                self.toolbar
                    .handle_event(arena, &scoped(self.focus == LayerFocus::Toolbar), viewport)
                    .map(WindowCommand::Toolbar),
            );
            if let Some(side) = &self.side {
                result = result.merge(
                    side.handle_event(arena, &scoped(self.focus != LayerFocus::Toolbar), viewport)
                        .map(WindowCommand::Side),
                );
            }
            if let Some(dock) = &self.dock {
                result = result.merge(
                    dock.handle_event(
                        arena,
                        &scoped(self.modal.is_none() && self.focus == LayerFocus::Dock),
                        viewport,
                    )
                    .map(WindowCommand::Dock),
                );
            }
            if let Some(modal) = &self.modal {
                result = result.merge(
                    modal
                        .handle_event(arena, &scoped(self.focus != LayerFocus::Toolbar), viewport)
                        .map(WindowCommand::Modal),
                );
            }
            return result;
        }

        let flip = match event {
            Event::MouseDown { point, .. } => {
                let target = if point.y < self.toolbar_height {
                    LayerFocus::Toolbar
                } else if self.dock_edge_x.is_some_and(|edge| point.x >= edge) {
                    LayerFocus::Dock
                } else {
                    LayerFocus::Content
                };
                (target != self.focus).then_some(WindowCommand::Focus(target))
            }
            _ => None,
        };
        let result = self.route(arena, event, viewport);
        match flip {
            Some(flip) => result.merge(EventResult::Command(flip)),
            None => result,
        }
    }
}

impl<'a> RealizedLayers<'a> {
    fn route(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: skia_safe::Rect,
    ) -> EventResult<WindowCommand> {
        if let Event::HitTest { point, miss } = event {
            let mut claimed = *miss;
            let tick = |claimed: &mut bool, blocks: bool| {
                let hit = !*claimed && blocks;
                if hit {
                    *claimed = true;
                }
                Event::HitTest {
                    point: *point,
                    miss: !hit,
                }
            };
            let mut merged: EventResult<WindowCommand> = EventResult::Ignored;
            if let Some(modal) = &self.modal {
                let event = tick(&mut claimed, modal.blocks_pointer(*point));
                merged = merged.merge(
                    modal
                        .handle_event(arena, &event, viewport)
                        .map(WindowCommand::Modal),
                );

                claimed = true;
            }
            if let Some(side) = &self.side {
                let event = tick(&mut claimed, side.blocks_pointer(*point));
                merged = merged.merge(
                    side.handle_event(arena, &event, viewport)
                        .map(WindowCommand::Side),
                );
            }
            if let Some(dock) = &self.dock {
                let event = tick(&mut claimed, dock.blocks_pointer(*point));
                merged = merged.merge(
                    dock.handle_event(arena, &event, viewport)
                        .map(WindowCommand::Dock),
                );
            }
            {
                let event = tick(&mut claimed, point.y < self.toolbar_height);
                merged = merged.merge(
                    self.toolbar
                        .handle_event(arena, &event, viewport)
                        .map(WindowCommand::Toolbar),
                );
            }
            let event = tick(&mut claimed, true);
            merged = merged.merge(
                self.base
                    .handle_event(arena, &event, viewport)
                    .map(WindowCommand::Base),
            );
            return merged;
        }

        if let Some(modal) = &self.modal {
            return match modal.handle_event(arena, event, viewport) {
                EventResult::Ignored => self
                    .toolbar
                    .handle_event(arena, event, viewport)
                    .map(WindowCommand::Toolbar),
                result => result.map(WindowCommand::Modal),
            };
        }

        if let Some(side) = &self.side {
            match side.handle_event(arena, event, viewport) {
                EventResult::Ignored => {
                    let below = self.below_side(arena, event, viewport);
                    let focus_moved = match event {
                        Event::MouseDown { .. } => true,
                        Event::KeyDown { .. } => !matches!(below, EventResult::Ignored),
                        _ => false,
                    };
                    return match focus_moved {
                        true => below.merge(EventResult::Command(WindowCommand::SideFocusLost)),
                        false => below,
                    };
                }
                result => return result.map(WindowCommand::Side),
            }
        }

        self.below_side(arena, event, viewport)
    }

    fn below_side(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: skia_safe::Rect,
    ) -> EventResult<WindowCommand> {
        let drag_path = matches!(event, Event::MouseDrag { .. } | Event::MouseUp { .. });
        let focus_routed = drag_path;

        if let Some(dock) = &self.dock {
            if !focus_routed || self.focus == LayerFocus::Dock {
                match dock.handle_event(arena, event, viewport) {
                    EventResult::Ignored => {}
                    result => return result.map(WindowCommand::Dock),
                }
            }
        }

        if !drag_path || self.focus == LayerFocus::Toolbar {
            match self.toolbar.handle_event(arena, event, viewport) {
                EventResult::Ignored => {}
                result => return result.map(WindowCommand::Toolbar),
            }
        }
        self.base
            .handle_event(arena, event, viewport)
            .map(WindowCommand::Base)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WindowId(u64);

impl WindowId {
    #[doc(hidden)]
    pub fn raw(self) -> u64 {
        self.0
    }

    #[doc(hidden)]
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Clone, Default)]
pub struct Windows {
    entries: rpds::HashTrieMapSync<WindowId, Window>,

    next: u64,
}

impl Windows {
    pub(crate) fn add(store: &mut imba::store::Store, entity: Window) -> WindowId {
        let mut windows = store.get::<Windows>().cloned().unwrap_or_default();
        let id = WindowId(windows.next);
        windows.next += 1;
        windows.entries.insert_mut(id, entity);
        store.put(windows);
        id
    }

    pub fn window(store: &imba::store::Store, id: WindowId) -> Option<Window> {
        Self::window_ref(store, id).cloned()
    }

    pub fn window_ref(store: &imba::store::Store, id: WindowId) -> Option<&Window> {
        store.get::<Windows>()?.entries.get(&id)
    }

    pub fn put(store: &mut imba::store::Store, id: WindowId, entity: Window) {
        store.update::<Windows>(|windows| {
            windows.entries.insert_mut(id, entity);
        });
    }

    pub fn list(store: &imba::store::Store) -> Vec<WindowId> {
        store
            .get::<Windows>()
            .map(|windows| windows.entries.keys().copied().collect())
            .unwrap_or_default()
    }

    pub(crate) fn primary(&self) -> Option<WindowId> {
        self.entries.keys().min_by_key(|id| id.0).copied()
    }

    pub(crate) fn session_of(&self, id: WindowId) -> Option<crate::SessionId> {
        self.entries.get(&id).map(|window| window.current_session())
    }

    pub(crate) fn ids(&self) -> Vec<WindowId> {
        let mut ids: Vec<WindowId> = self.entries.keys().copied().collect();
        ids.sort_by_key(|id| id.0);
        ids
    }

    pub(crate) fn entity(&self, id: WindowId) -> Option<&Window> {
        self.entries.get(&id)
    }

    pub(crate) fn project(&self, window: Option<WindowId>) -> Windows {
        let mut projected = self.clone();
        for id in self.entries.keys() {
            if Some(*id) != window {
                projected.entries.remove_mut(id);
            }
        }
        projected
    }

    pub(crate) fn absorb(&mut self, taken: Windows) {
        for (id, entity) in taken.entries.iter() {
            self.entries.insert_mut(*id, entity.clone());
        }
        self.next = taken.next;
    }

    pub(crate) fn adopt_local_host_all(&mut self, host: crate::higent::HostId) {
        let ids: Vec<WindowId> = self.entries.keys().copied().collect();
        for id in ids {
            let Some(entity) = self.entries.get(&id) else {
                continue;
            };
            let mut entity = entity.clone();
            if entity.adopt_local_host(host) {
                self.entries.insert_mut(id, entity);
            }
        }
    }
}

#[derive(Clone)]
pub struct Window {
    content: Layers,

    viewport_size: Size,

    current_session: crate::SessionId,

    dock_width: f32,

    workbenches: rpds::HashTrieMapSync<crate::SessionId, Workbench>,

    focused_location: Option<crate::ResourceLocation>,
    focus_generation: u64,
}

impl Window {
    pub fn focused_location(&self) -> Option<&crate::ResourceLocation> {
        self.focused_location.as_ref()
    }

    pub fn focus_generation(&self) -> u64 {
        self.focus_generation
    }

    pub(crate) fn note_focused_location(&mut self, location: crate::ResourceLocation) {
        if self.focused_location.as_ref() != Some(&location) {
            self.focused_location = Some(location);
            self.focus_generation += 1;
        }
    }

    pub(crate) fn new(root: WorkbenchNode, workspace: crate::SessionId) -> Self {
        Self {
            content: Layers {
                toolbar: crate::toolbar::Toolbar::default(),
                focus: LayerFocus::Content,
                workbench: Workbench::new(root),
                side: None,
                modal: None,
            },
            viewport_size: Size::new(1.0, 1.0),
            dock_width: crate::dock::DOCK_WIDTH,
            current_session: workspace,
            workbenches: rpds::HashTrieMapSync::new_sync(),
            focused_location: None,
            focus_generation: 0,
        }
    }

    pub fn current_session(&self) -> crate::SessionId {
        self.current_session.clone()
    }

    #[must_use]
    pub(crate) fn switch_to(&mut self, workspace: crate::SessionId) -> Option<crate::SessionId> {
        if workspace == self.current_session {
            return None;
        }

        if matches!(self.content.focus, LayerFocus::Dock) {
            self.content.focus = LayerFocus::Content;
        }
        let Some(stashed) = self.workbenches.get(&workspace) else {
            return Some(std::mem::replace(&mut self.current_session, workspace));
        };
        let restored = stashed.clone();
        self.workbenches.remove_mut(&workspace);
        let stashed = std::mem::replace(&mut self.content.workbench, restored);
        self.workbenches
            .insert_mut(self.current_session.clone(), stashed);
        self.current_session = workspace;
        self.content.workbench.settle_dock();
        None
    }

    pub(crate) fn rekey_current(&mut self, workspace: crate::SessionId) -> bool {
        if workspace == self.current_session {
            return true;
        }
        if self.workbenches.get(&workspace).is_some() {
            return false;
        }
        self.current_session = workspace;
        true
    }

    pub(crate) fn install_fresh(&mut self, previous: crate::SessionId, fresh: Workbench) {
        let stashed = std::mem::replace(&mut self.content.workbench, fresh);
        self.workbenches.insert_mut(previous, stashed);
    }

    pub(crate) fn adopt_local_host(&mut self, host: crate::higent::HostId) -> bool {
        let is_stale_local = |id: &crate::SessionId| {
            id.session == host_discovery::LOCAL_FS_SESSION && id.host != host
        };
        let mut changed = false;
        if is_stale_local(&self.current_session) {
            self.current_session.host = host;
            changed = true;
        }
        let stale: Vec<crate::SessionId> = self
            .workbenches
            .iter()
            .map(|(id, _)| id.clone())
            .filter(|id| is_stale_local(id))
            .collect();
        for id in stale {
            if let Some(stashed) = self.workbenches.get(&id).cloned() {
                self.workbenches.remove_mut(&id);
                let mut fresh = id;
                fresh.host = host;
                self.workbenches.insert_mut(fresh, stashed);
            }
            changed = true;
        }
        changed
    }

    pub(crate) fn stashed_workbenches(
        &self,
    ) -> impl Iterator<Item = (&crate::SessionId, &Workbench)> + '_ {
        self.workbenches.iter()
    }

    pub fn viewport_size(&self) -> Size {
        self.viewport_size
    }

    pub(crate) fn viewport_stale(&self, size: Size) -> bool {
        (self.viewport_size.width - size.width).abs() > f32::EPSILON
            || (self.viewport_size.height - size.height).abs() > f32::EPSILON
    }

    pub(crate) fn set_viewport_size(&mut self, size: Size) {
        self.viewport_size = size;
    }

    pub(crate) fn workbench(&self) -> &Workbench {
        &self.content.workbench
    }

    pub(crate) fn workbench_mut(&mut self) -> &mut Workbench {
        &mut self.content.workbench
    }

    pub fn show_modal(
        &mut self,
        store: &mut Store,
        modal: Box<dyn ModalView>,
        fx: &mut Effects<'_, WindowCommand>,
    ) {
        self.dismiss_side_panel(store, fx);
        self.dismiss_modal(store, fx);
        self.content.modal = Some(modal);
    }

    pub fn dismiss_modal(&mut self, store: &mut Store, fx: &mut Effects<'_, WindowCommand>) {
        self.release_modal_for_swap(store, fx);
        self.content.toolbar.end_session();
        self.content.focus = LayerFocus::Content;
    }

    pub(crate) fn release_modal_for_swap(
        &mut self,
        store: &mut Store,
        fx: &mut Effects<'_, WindowCommand>,
    ) {
        let released = self
            .content
            .modal
            .as_mut()
            .map(|view| view.release_widgets())
            .unwrap_or_default();
        self.restore_widgets(released);
        if let Some(mut modal) = self.content.modal.take() {
            fx.scope(WindowCommand::Modal, |fx| {
                imba::DynView::destroy_dyn(modal.as_mut(), store, fx)
            });
        }
    }

    pub(crate) fn set_overlay(
        &mut self,
        store: &mut Store,
        modal: Box<dyn ModalView>,
        fx: &mut Effects<'_, WindowCommand>,
    ) {
        self.dismiss_side_panel(store, fx);
        self.content.modal = Some(modal);
    }

    pub(crate) fn modal_set_query(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        query: &str,
        fx: &mut Effects<'_, imba::DynCommand>,
    ) {
        if let Some(modal) = &mut self.content.modal {
            modal.set_query(store, ui, query, fx);
        }
    }

    pub(crate) fn toolbar_session_class(&self) -> Option<Option<char>> {
        self.content.toolbar.session_class()
    }

    pub(crate) fn toolbar_start_session(
        &mut self,
        store: &Store,
        ui: &imba::UiCtx,
        class: Option<char>,
        text: &str,
        width: f32,
    ) {
        self.content
            .toolbar
            .start_session(store, ui, class, text, width);
        self.content.focus = LayerFocus::Toolbar;
    }

    pub(crate) fn toolbar_set_session_class(&mut self, class: Option<char>) {
        self.content.toolbar.set_session_class(class);
    }

    pub fn plugin_modal(&self) -> Option<&dyn ModalView> {
        self.content.modal.as_ref().map(|view| view.as_ref())
    }

    pub fn has_modal(&self) -> bool {
        self.content.modal.is_some()
    }

    pub fn show_side_panel(
        &mut self,
        store: &mut Store,
        panel: Box<dyn ModalView>,
        fx: &mut Effects<'_, WindowCommand>,
    ) {
        self.dismiss_side_panel(store, fx);
        self.content.side = Some(crate::drawer::Drawer::new(panel));
    }

    pub fn dismiss_side_panel(&mut self, store: &mut Store, fx: &mut Effects<'_, WindowCommand>) {
        let released = self
            .content
            .side
            .as_mut()
            .map(|view| view.release_widgets())
            .unwrap_or_default();
        self.restore_widgets(released);
        if let Some(mut side) = self.content.side.take() {
            fx.scope(WindowCommand::Side, |fx| {
                imba::DynView::destroy_dyn(&mut side, store, fx)
            });
        }
    }

    pub fn side_panel(&self) -> Option<&dyn ModalView> {
        self.content.side.as_ref().map(|drawer| drawer.content())
    }

    /// `chat.composer` (the toolbar bubble, ⌘I): FRONT the session's
    /// chat as an ordinary workbench panel — focus the standing chat
    /// pane if one is open in this workbench, otherwise mount the
    /// session's chat into the focused pane.
    pub(crate) fn front_chat(&mut self, store: &mut Store, ui: &UiCtx, fx: &mut AppFx<'_>) {
        if self.has_modal() {
            return;
        }
        let is_chat = |panel: &Panel| {
            matches!(panel, Panel::Plugin(view)
                if matches!(view.family_row(), Some(crate::FamilyRow::Chat(_))))
        };
        if self.workbench_mut().root.focus_where(&is_chat) {
            self.content.focus = LayerFocus::Content;
            return;
        }
        let session = self.current_session();
        let Some(chat) = crate::higent::Chats::list(store).into_iter().find(|chat| {
            crate::higent::Chats::chat_ref(store, chat)
                .is_some_and(|panel| panel.session_id() == session)
        }) else {
            return;
        };
        let Some(pane) = crate::family_rows::mint(store, &crate::FamilyRow::Chat(chat)) else {
            return;
        };
        if self.open_panel(store, ui, pane, fx) {
            self.content.focus = LayerFocus::Content;
        }
    }

    #[doc(hidden)]
    pub fn layer_focus(&self) -> LayerFocus {
        self.content.focus
    }

    pub fn side_panel_mut(&mut self) -> Option<&mut Box<dyn ModalView>> {
        self.content
            .side
            .as_mut()
            .map(|drawer| drawer.content_mut())
    }

    pub fn roll_away_side_panel(&mut self) {
        if let Some(drawer) = &mut self.content.side {
            drawer.focus_lost();
        }
    }

    pub fn has_side_panel(&self) -> bool {
        self.content.side.is_some()
    }

    pub fn show_dock(
        &mut self,
        store: &mut Store,
        panel: Box<dyn ModalView>,
        owner: &'static str,
        fx: &mut Effects<'_, WindowCommand>,
    ) {
        match self.content.workbench.dock_mut() {
            Some(dock) => {
                let released = dock.release_widgets();
                self.restore_widgets(released);
                let mut outgoing = self
                    .content
                    .workbench
                    .dock_mut()
                    .as_mut()
                    .expect("matched Some above")
                    .swap(panel, owner);
                fx.scope(WindowCommand::Dock, |fx| {
                    fx.scope(
                        |command| {
                            Box::new(crate::dock::DockCommand::Content(command)) as imba::DynCommand
                        },
                        |fx| imba::DynView::destroy_dyn(outgoing.as_mut(), store, fx),
                    )
                });
            }
            None => {
                *self.content.workbench.dock_mut() =
                    Some(crate::dock::Dock::new(panel, owner, self.dock_width));
            }
        }
        self.content.focus = LayerFocus::Dock;
    }

    pub fn dismiss_dock(&mut self, store: &mut Store, fx: &mut Effects<'_, WindowCommand>) {
        if std::env::var_os("HIMARK_TRACE_DOCK").is_some()
            && self.content.workbench.dock().is_some()
        {
            eprintln!(
                "[dock] dismiss_dock:\n{}",
                std::backtrace::Backtrace::force_capture()
            );
        }
        let released = self
            .content
            .workbench
            .dock_mut()
            .as_mut()
            .map(|dock| dock.release_widgets())
            .unwrap_or_default();
        self.restore_widgets(released);
        if let Some(mut dock) = self.content.workbench.dock_mut().take() {
            self.dock_width = dock.width();
            fx.scope(WindowCommand::Dock, |fx| {
                imba::DynView::destroy_dyn(&mut dock, store, fx)
            });
        }
        if self.content.focus == LayerFocus::Dock {
            self.content.focus = LayerFocus::Content;
        }
    }

    /// The keyboard goes to the dock — a re-invoked dock command
    /// (cmd-shift-f on an already-open search) FOCUSES instead of
    /// toggling away.
    pub fn focus_dock(&mut self) {
        self.content.focus = LayerFocus::Dock;
    }

    pub fn roll_away_dock(&mut self) {
        if let Some(dock) = self.content.workbench.dock_mut() {
            dock.roll_away();
        }
        if self.content.focus == LayerFocus::Dock {
            self.content.focus = LayerFocus::Content;
        }
    }

    pub fn dock_panel(&self) -> Option<&dyn ModalView> {
        self.content.workbench.dock().map(|dock| dock.content())
    }

    pub fn dock_panel_mut(&mut self) -> Option<&mut Box<dyn ModalView>> {
        self.content
            .workbench
            .dock_mut()
            .as_mut()
            .map(|dock| dock.content_mut())
    }

    pub fn dock_owner(&self) -> Option<&'static str> {
        self.content
            .workbench
            .dock()
            .filter(|dock| dock.target_width() > 0.0)
            .map(|dock| dock.owner())
    }

    pub fn has_dock(&self) -> bool {
        self.content.workbench.dock().is_some()
    }

    pub fn dock_target_width(&self) -> f32 {
        self.content
            .workbench
            .dock()
            .map_or(0.0, crate::dock::Dock::target_width)
    }

    pub(crate) fn take_dock_request(&mut self) -> Option<ModalRequest> {
        self.content
            .workbench
            .dock_mut()
            .as_mut()
            .and_then(|dock| dock.take_request())
    }

    pub(crate) fn take_modal_request(&mut self) -> Option<ModalRequest> {
        self.content
            .modal
            .as_mut()
            .and_then(|view| view.take_request())
    }

    pub(crate) fn take_toolbar_request(&mut self) -> Option<crate::toolbar::ToolbarRequest> {
        self.content.toolbar.take_request()
    }

    pub(crate) fn take_side_request(&mut self) -> Option<ModalRequest> {
        self.content
            .side
            .as_mut()
            .and_then(|view| view.take_request())
    }

    pub(crate) fn take_panel_request(&mut self) -> Option<crate::PanelRequest> {
        let mut request = None;
        self.workbench_mut().root.for_each_pane_mut(&mut |panel| {
            if request.is_none() {
                if let Panel::Plugin(view) = panel {
                    request = view.take_request();
                }
            }
        });
        request
    }

    /// The keyboard follows a deliberate jump: the same transition
    /// `WindowCommand::Focus(LayerFocus::Content)` performs, reachable
    /// from the app road (`show_document` with `focus`).
    fn focus_content_layer(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let _ = (store, ui, window, fx);
        self.content.focus = LayerFocus::Content;
    }

    pub(crate) fn replace_focused_panel(&mut self, store: &mut Store, panel: crate::Panel) {
        let _ = store;
        let displaced = self.workbench_mut().root.replace_focused_panel(panel);

        self.stash_displaced(displaced);
    }

    pub fn focused_document_id(&self) -> Option<crate::DocumentId> {
        Some(
            self.workbench()
                .root
                .focused_pane()
                .editor()?
                .content()
                .document(),
        )
    }

    fn stash_displaced(&mut self, displaced: Panel) {
        let _ = displaced;
    }

    pub fn unmount_all_widgets(
        &mut self,
    ) -> Vec<(crate::WidgetOrigin, Box<dyn crate::DynPanelView>)> {
        let mut widgets = Vec::new();
        let mut index = 0usize;
        self.workbench_mut().root.for_each_pane_mut(&mut |panel| {
            if matches!(panel, Panel::Plugin(_)) && !panel.is_blank() {
                let taken = std::mem::replace(panel, Panel::blank());
                if let Panel::Plugin(widget) = taken {
                    widgets.push((crate::WidgetOrigin::Pane(index), widget));
                }
            }
            index += 1;
        });
        widgets
    }

    pub fn restore_widgets(
        &mut self,
        widgets: Vec<(crate::WidgetOrigin, Box<dyn crate::DynPanelView>)>,
    ) {
        for (origin, widget) in widgets {
            match origin {
                crate::WidgetOrigin::Pane(target) => {
                    let mut index = 0usize;
                    let mut widget = Some(widget);
                    self.workbench_mut().root.for_each_pane_mut(&mut |panel| {
                        if index == target && panel.is_blank() {
                            if let Some(widget) = widget.take() {
                                *panel = Panel::Plugin(widget);
                            }
                        }
                        index += 1;
                    });
                }
                crate::WidgetOrigin::Family => {}
            }
        }
    }

    pub fn mount_focused(&mut self, widget: Box<dyn crate::DynPanelView>) {
        let displaced = self
            .workbench_mut()
            .root
            .replace_focused_panel(Panel::Plugin(widget));
        self.stash_displaced(displaced);
    }

    pub fn open_panel<R: 'static>(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        panel: Box<dyn crate::DynPanelView>,
        fx: &mut Effects<'_, R>,
    ) -> bool {
        if self.has_modal() {
            return false;
        }
        self.install_panel(store, ui, Panel::Plugin(panel), fx);
        true
    }

    pub(crate) fn close_current(&mut self, store: &mut Store) -> bool {
        if self.has_modal() {
            return false;
        }

        let _ = store;
        let displaced =
            std::mem::replace(self.workbench_mut().root.focused_pane_mut(), Panel::blank());
        match self.workbench_mut().root.close_focused() {
            true => {
                self.stash_displaced(displaced);
                true
            }
            false => {
                *self.workbench_mut().root.focused_pane_mut() = displaced;
                false
            }
        }
    }

    pub(crate) fn split_current(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        fx: &mut AppFx<'_>,
    ) {
        if self.has_modal() {
            return;
        }

        let Some(entity) = self
            .workbench()
            .root
            .focused_pane()
            .editor()
            .map(|pane| *pane.content())
        else {
            return;
        };
        let document_id = entity.document();
        let width = panel_width(store, self.workbench().root.focused_pane())
            .unwrap_or_else(|| crate::app::fallback_pane_editor_width(store));
        let Some(mut document) = crate::OpenDocuments::document(store, document_id) else {
            return;
        };
        let fonts = ::editor::env::Fonts::of(store)();
        let theme = ::editor::env::Themes::of(store);
        let new_editor = crate::app::entity_scope(document_id, fx, |fx| {
            document.add_editor(
                width,
                None,
                ::editor::EditorBuild::Bounded,
                &[],
                store,
                ui,
                &fonts,
                &theme,
                fx,
            )
        });
        documents::scroll_stripes::enable_scroll_stripes(
            store,
            document_id,
            &mut document,
            new_editor,
        );

        crate::OpenDocuments::put_document(store, document_id, document);

        let pane_height = crate::OpenDocuments::document_ref(store, document_id)
            .and_then(|document| document.viewport(entity.editor()))
            .map(|band| band.end - band.start);
        let pane_width = width + ::editor::env::Themes::of(store).ui().editor_gutter.width;
        let arrangement = match pane_height {
            Some(height) if pane_width < height => imba::split::Arrangement::Column,

            _ => imba::split::Arrangement::Row,
        };
        self.workbench_mut().root.split_focused(
            Panel::Editor(ScrollView::new(
                EditorIdView::new(document_id, new_editor).with_gutter(),
            )),
            arrangement,
            false,
        );
    }

    pub fn navigate(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        target: &crate::NavigationLocation,
        fx: &mut AppFx<'_>,
    ) -> bool {
        if let Some(walk) = self.workbench_mut().root.focused_slot_mut().pending.take() {
            if same_editor_location(&walk.target, target)
                && self.complete_walk(store, ui, window, &walk.target, walk.step, fx)
            {
                return true;
            }
        }
        {
            let slot = self.workbench_mut().root.focused_slot_mut();
            let outgoing = slot.panel.navigation_location(store);
            if slot.panel.navigate_to(store, ui, target, fx) {
                if let Some(place) = outgoing {
                    if !place.same(target) {
                        slot.back.push_back_mut(place);
                        slot.forward = rpds::VectorSync::new_sync();
                    }
                }
                Self::touch_recent(store, target);
                return true;
            }
        }
        let Some(panel) = crate::Navigators::navigate(store, ui, window, target, fx) else {
            return false;
        };
        self.install_panel(store, ui, panel, fx);
        Self::touch_recent(store, target);
        true
    }

    fn complete_walk(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        target: &crate::NavigationLocation,
        step: crate::workbench_node::WalkStep,
        fx: &mut AppFx<'_>,
    ) -> bool {
        use crate::workbench_node::WalkStep;
        let (outgoing, taken_in_place) = {
            let slot = self.workbench_mut().root.focused_slot_mut();
            let outgoing = slot.panel.navigation_location(store);
            (outgoing, slot.panel.navigate_to(store, ui, target, fx))
        };
        let panel = match taken_in_place {
            true => None,
            false => match crate::Navigators::navigate(store, ui, window, target, fx) {
                Some(panel) => Some(panel),
                None => return false,
            },
        };
        let slot = self.workbench_mut().root.focused_slot_mut();
        match step {
            WalkStep::Back => {
                if slot.back.last().is_some_and(|top| top.same(target)) {
                    slot.back.drop_last_mut();
                }
                if let Some(place) = outgoing {
                    slot.forward.push_back_mut(place);
                }
            }
            WalkStep::Forward => {
                if slot.forward.last().is_some_and(|top| top.same(target)) {
                    slot.forward.drop_last_mut();
                }
                if let Some(place) = outgoing {
                    slot.back.push_back_mut(place);
                }
            }
            WalkStep::Replace => {}
        }
        if let Some(panel) = panel {
            let displaced = slot.replace_panel(panel);
            self.retire_displaced(store, ui, displaced, fx);
        }
        Self::touch_recent(store, target);
        true
    }

    pub fn close_focused_widget(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        fx: &mut AppFx<'_>,
    ) -> bool {
        if self.has_modal() {
            return false;
        }
        let closed = {
            let slot = self.workbench_mut().root.focused_slot_mut();
            slot.replace_panel(Panel::blank())
        };
        match closed {
            Panel::Editor(pane) => {
                let view = *pane.content();
                crate::close_editor(store, view.document(), view.editor());
                crate::OpenDocuments::remove_on_close(store, ui, view.document(), fx);
            }
            Panel::Plugin(mut view) => {
                if !view.as_any().is::<crate::workbench_node::ClosedPanel>() {
                    view.dismantle(store);
                }
            }
        }

        let target = {
            let slot = self.workbench_mut().root.focused_slot_mut();
            let target = slot.back.last().cloned();
            if target.is_some() {
                slot.back.drop_last_mut();
            }
            target
        };
        if let Some(target) = target {
            use crate::workbench_node::{PendingWalk, WalkStep};
            if !self.complete_walk(store, ui, window, &target, WalkStep::Replace, fx)
                && target.place::<crate::EditorPlace>().is_some()
            {
                let slot = self.workbench_mut().root.focused_slot_mut();
                slot.pending = Some(PendingWalk {
                    target,
                    step: WalkStep::Replace,
                });
            }
        }
        true
    }

    fn touch_recent(store: &mut Store, target: &crate::NavigationLocation) {
        if let Some(place) = target.place::<crate::EditorPlace>() {
            crate::RecentLocations::touch(store, &place.location);
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn focused_history_depths(&self) -> (usize, usize) {
        self.workbench().root.focused_slot().history_depths()
    }

    fn install_panel<R: 'static>(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        panel: Panel,
        fx: &mut Effects<'_, R>,
    ) {
        let slot = self.workbench_mut().root.focused_slot_mut();
        if let Some(place) = slot.panel.navigation_location(store) {
            slot.back.push_back_mut(place);
            slot.forward = rpds::VectorSync::new_sync();
        }
        let displaced = slot.replace_panel(panel);
        self.retire_displaced(store, ui, displaced, fx);
    }

    pub fn navigate_back(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        fx: &mut AppFx<'_>,
    ) -> bool {
        self.navigate_history(store, ui, window, fx, true)
    }

    pub fn navigate_forward(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        fx: &mut AppFx<'_>,
    ) -> bool {
        self.navigate_history(store, ui, window, fx, false)
    }

    fn navigate_history(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        fx: &mut AppFx<'_>,
        back: bool,
    ) -> bool {
        use crate::workbench_node::{PendingWalk, WalkStep};
        let target = {
            let slot = self.workbench_mut().root.focused_slot_mut();
            let stack = if back { &slot.back } else { &slot.forward };
            match stack.last().cloned() {
                Some(target) => target,
                None => return false,
            }
        };
        let step = if back {
            WalkStep::Back
        } else {
            WalkStep::Forward
        };
        if self.complete_walk(store, ui, window, &target, step, fx) {
            return true;
        }

        match target.place::<crate::EditorPlace>().is_some() {
            true => {
                let slot = self.workbench_mut().root.focused_slot_mut();
                slot.pending = Some(PendingWalk { target, step });
                true
            }
            false => false,
        }
    }

    fn retire_displaced<R: 'static>(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        displaced: Panel,
        fx: &mut Effects<'_, R>,
    ) {
        if let Panel::Editor(pane) = &displaced {
            let view = *pane.content();
            crate::close_editor(store, view.document(), view.editor());
            crate::OpenDocuments::remove_if_editorless(store, ui, view.document(), fx);
        }
        self.stash_displaced(displaced);
    }

    pub fn show_document(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        document_id: crate::DocumentId,
        target: Option<std::ops::Range<crate::LineCol>>,
        focus: bool,
        fx: &mut AppFx<'_>,
    ) {
        if focus {
            self.focus_content_layer(store, ui, window, fx);
        }
        let Some(mut document) = crate::OpenDocuments::document(store, document_id) else {
            return;
        };

        if let Some(location) = crate::OpenDocuments::location(store, document_id) {
            if target.is_none() {
                let walk_waits = self
                    .workbench()
                    .root
                    .focused_slot()
                    .pending
                    .as_ref()
                    .is_some_and(|walk| {
                        walk.target
                            .place::<crate::EditorPlace>()
                            .is_some_and(|place| place.location == location)
                    });
                let already_shown =
                    self.workbench()
                        .root
                        .focused_pane()
                        .editor()
                        .is_some_and(|pane| {
                            crate::OpenDocuments::location(store, pane.content().document())
                                .as_ref()
                                == Some(&location)
                        });
                if already_shown && !walk_waits {
                    crate::OpenDocuments::touch(store, document_id);
                    return;
                }
            }
            let caret = target
                .as_ref()
                .map(|target| crate::offset_at(&mut document.text().view(), target.start) as u32)
                .unwrap_or(0);
            drop(document);
            let place = crate::EditorPlace {
                location,
                caret,
                scroll_y: 0.0,
            };
            if self.navigate(
                store,
                ui,
                window,
                &crate::NavigationLocation::new(place),
                fx,
            ) {
                return;
            }
            let Some(document_again) = crate::OpenDocuments::document(store, document_id) else {
                return;
            };
            document = document_again;
        }
        let width = panel_width(store, self.workbench().root.focused_pane())
            .unwrap_or_else(|| crate::app::fallback_pane_editor_width(store));
        let editor_id = crate::app::entity_scope(document_id, fx, |fx| {
            crate::mount_editor(store, ui, &mut document, width, target, fx)
        });
        documents::scroll_stripes::enable_scroll_stripes(
            store,
            document_id,
            &mut document,
            editor_id,
        );
        crate::OpenDocuments::put_document(store, document_id, document);
        crate::OpenDocuments::touch(store, document_id);
        self.replace_focused_panel(
            store,
            Panel::Editor(ScrollView::new(
                EditorIdView::new(document_id, editor_id).with_gutter(),
            )),
        );
    }
}

impl View for Window {
    type Command = WindowCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, WindowCommand> {
        self.content.focus_data(store, ui)
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: WindowCommand,
        fx: &mut Effects<'_, WindowCommand>,
    ) {
        self.content.perform(store, ui, command, fx)
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, WindowCommand> + imba::LayoutValue + 'a {
        self.content.display(arena, store, ui)
    }
}

impl Window {
    pub(crate) fn sync_viewport(window: WindowId, app: &mut Application, size: Size) {
        if app.viewport_stale(window, size) {
            app.perform_batch(vec![crate::AppCommand::ViewportResized(window, size)]);
        }
    }

    pub fn draw(window: WindowId, app: &mut Application, canvas: &skia_safe::Canvas) -> bool {
        let size = canvas.base_layer_size();
        Self::draw_with_size(
            window,
            app,
            canvas,
            Size::new(size.width as f32, size.height as f32),
        )
    }

    pub fn draw_with_size(
        window: WindowId,
        app: &mut Application,
        canvas: &skia_safe::Canvas,
        size: Size,
    ) -> bool {
        app.stats_mut().begin_frame();

        if trace_resize_enabled() {
            let entity =
                crate::Windows::window_ref(app.store(), window).expect("the window entity");
            if entity.viewport_stale(size) {
                let current = entity.viewport_size;
                eprintln!(
                    "[resize himark] draw canvas={}x{} previous_viewport={}x{}",
                    size.width, size.height, current.width, current.height
                );
            }
        }
        Self::sync_viewport(window, app, size);

        let render_started = std::time::Instant::now();

        let reconciled = app.dispatch_paint(window, canvas, size);
        app.stats_mut().record_reconcile(reconciled);
        app.stats_mut()
            .record_render_sample(render_started.elapsed());
        reconciled
    }

    #[cfg(any(test, feature = "test-support"))]

    pub fn draw_profiled(
        window: WindowId,
        app: &mut Application,
        canvas: &skia_safe::Canvas,
        size: Size,
    ) -> std::time::Duration {
        app.stats_mut().begin_frame();
        Self::sync_viewport(window, app, size);
        let paint_started = std::time::Instant::now();
        let _ = app.dispatch_paint(window, canvas, size);
        paint_started.elapsed()
    }
}

fn trace_resize_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_RESIZE").is_some())
}

fn same_editor_location(a: &crate::NavigationLocation, b: &crate::NavigationLocation) -> bool {
    match (
        a.place::<crate::EditorPlace>(),
        b.place::<crate::EditorPlace>(),
    ) {
        (Some(a), Some(b)) => a.location == b.location,
        _ => false,
    }
}

/// The window WIREFRAME, reified (docs/ui/UI.md stage 2): toolbar band,
/// base workbench, side/dock layers — geometry cut per
/// constraints, composed into the focus-routing `LayersWidget`.
/// Captures the store/ui borrows the `laid` closure used to hide;
/// hoisting the child `display` calls up is this view's next verse.
struct WindowFrame<'a> {
    layers: &'a Layers,
    store: &'a Store,
    ui: &'a UiCtx,
}

impl imba::LayoutValue for WindowFrame<'_> {}

impl<'a> imba::Layout<'a, WindowCommand> for WindowFrame<'a> {
    fn layout(
        self,
        arena: &'a Arena,
        constraints: Constraints,
    ) -> imba::ThunkBox<'a, WindowCommand> {
        let WindowFrame { layers, store, ui } = self;
        imba::ThunkBox::new(arena, {
            let title = layers.workbench.root.focused_pane().title(store);

            let size = constraints.max;
            let toolbar_height = ::editor::env::Themes::of(store).ui().toolbar.height;
            let below = Constraints::tight(Size::new(
                size.width,
                (size.height - toolbar_height).max(1.0),
            ));

            let revealed = layers
                .workbench
                .dock()
                .map_or(0.0, crate::dock::Dock::revealed);
            let base_below = Constraints::tight(Size::new(
                (size.width - revealed).max(1.0),
                (size.height - toolbar_height).max(1.0),
            ));

            LayersWidget {
                focus: layers.focus,
                toolbar_height,
                dock_edge_x: layers.workbench.dock().map(|_| size.width - revealed),
                base: below_layer(
                    arena,
                    size,
                    toolbar_height,
                    imba::ThunkBox::new(
                        arena,
                        imba::Layout::layout(
                            layers.workbench.display(arena, store, ui),
                            arena,
                            base_below,
                        ),
                    ),
                ),
                toolbar: layers.toolbar.layout(
                    arena,
                    store,
                    ui,
                    constraints.max.width,
                    title,
                    layers
                        .workbench
                        .dock()
                        .filter(|dock| dock.target_width() > 0.0)
                        .map(crate::dock::Dock::owner),
                ),
                side: layers.side.as_ref().map(|side| {
                    below_layer(
                        arena,
                        size,
                        toolbar_height,
                        side.layout_dyn(arena, store, ui, below),
                    )
                }),
                dock: layers.workbench.dock().map(|dock| {
                    below_layer(
                        arena,
                        size,
                        toolbar_height,
                        dock.layout_dyn(arena, store, ui, below),
                    )
                }),
                modal: layers.modal.as_ref().map(|modal| {
                    below_layer(
                        arena,
                        size,
                        toolbar_height,
                        modal.as_ref().layout_dyn(arena, store, ui, below),
                    )
                }),
            }
        })
    }
}
