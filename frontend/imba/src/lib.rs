// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub mod anim;
pub mod arena;
mod r#box;
pub mod checkbox;
pub mod clipboard;
pub mod constraints;
pub mod container;
mod dyn_view;
pub mod effect;
pub mod event;
pub mod focus;
pub mod image;
pub mod ime;
pub mod layout;
pub mod lazy;
pub mod leaf;
pub mod list;
pub mod overlay;
pub mod perf;
pub mod scroll;
pub mod split;
pub mod stack;
pub mod store;
pub mod svg;
pub mod thunk_ext;
pub mod tooltip;
pub mod ui;

pub use clipboard::{ClipboardClient, ClipboardContent};
pub use dyn_view::{CloneDynView, DynCommand, DynView};
pub use ime::ImeClient;
pub use store::{Component, Store};
pub use ui::UiCtx;

use arena::Arena;
use constraints::Constraints;
use event::{Event, EventResult};
use skia_safe::{Point, Rect, Size};

pub struct PresentableCommand<Command> {
    pub id: &'static str,

    pub name: String,

    pub command: Command,
}

impl<Command> PresentableCommand<Command> {
    pub fn new(id: &'static str, name: impl Into<String>, command: Command) -> Self {
        Self {
            id,
            name: name.into(),
            command,
        }
    }

    pub fn map<Parent>(self, wrap: impl FnOnce(Command) -> Parent) -> PresentableCommand<Parent> {
        PresentableCommand {
            id: self.id,
            name: self.name,
            command: wrap(self.command),
        }
    }
}

pub use layout::{
    fixed, laid, spacer, text, Align, Alignment, Backdrop, Button, Column, CrossAlign,
    EventHandler, Fill, Fixed, Insets, Laid, Layout, LayoutBox, LayoutExt, LayoutValue, MapLayout,
    OnClick, OnEvent, Pad, Row, Shield, SizedBox, Text, WithBaseline, ZBox,
};

pub trait View {
    type Command;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut effect::Effects<'_, Self::Command>,
    );

    /// The enclosing scroll moved this content's viewport top —
    /// called from the scroll's own perform, so it is ordinary
    /// retained-state mutation, not a paint back-channel. Content
    /// that anchors its viewport (docs/viewport-preservation.md)
    /// keeps the top here (and drops any pending correction — a
    /// landed scroll supersedes it); everyone else ignores it.
    fn scrolled(&mut self, store: &mut Store, top: f32) {
        let _ = (store, top);
    }

    /// Read the state, name the structure (docs/UI.md, revision 3) —
    /// the ONLY stage with the store in scope; borrows from it and
    /// from the view ride the returned layout for the frame.
    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl Layout<'a, Self::Command> + LayoutValue + 'a;

    /// The composed pipeline — display, then size. Containers and
    /// the frame root call this; layout combinators address the
    /// stages separately.
    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> ThunkBox<'a, Self::Command>
    where
        Self: Sized,
    {
        self.display(arena, store, ui).layout(arena, constraints)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut effect::Effects<'_, Self::Command>) {
        let _ = (store, fx);
    }
}

pub trait Thunk<'a, Command> {
    fn size(&self) -> Size;

    /// Compose's `FirstBaseline` alignment line: distance from this
    /// thunk's top to its first text baseline, when it has one.
    /// Wrappers forward it; containers propagate the topmost placed
    /// line; `Row` children can align by it.
    fn first_baseline(&self) -> Option<f32> {
        None
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command>
    where
        Self: Sized;
}

pub struct ThunkBox<'a, Command>(arena::ArenaBox<'a, dyn DynThunk<'a, Command> + 'a>);

impl<'a, Command: 'a> ThunkBox<'a, Command> {
    pub fn new<T: Thunk<'a, Command> + 'a>(arena: &'a Arena, thunk: T) -> Self {
        let slot = arena.boxed(Slot(Some(thunk)));
        let raw: *mut Slot<T> = arena::ArenaBox::into_raw(slot);

        Self(unsafe { arena::ArenaBox::from_raw(raw as *mut (dyn DynThunk<'a, Command> + 'a)) })
    }
}

impl<'a, Command: 'a> Thunk<'a, Command> for ThunkBox<'a, Command> {
    fn size(&self) -> Size {
        self.0.size()
    }

    fn first_baseline(&self) -> Option<f32> {
        self.0.first_baseline()
    }

    fn realize(mut self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        self.0.realize_dyn(arena, viewport)
    }
}

trait DynThunk<'a, Command> {
    fn size(&self) -> Size;
    fn first_baseline(&self) -> Option<f32>;
    fn realize_dyn(&mut self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command>;
}

struct Slot<T>(Option<T>);

impl<'a, Command: 'a, T: Thunk<'a, Command>> DynThunk<'a, Command> for Slot<T> {
    fn size(&self) -> Size {
        self.0.as_ref().expect("realized twice").size()
    }

    fn first_baseline(&self) -> Option<f32> {
        self.0.as_ref().expect("realized twice").first_baseline()
    }

    fn realize_dyn(&mut self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        self.0
            .take()
            .expect("realized twice")
            .realize(arena, viewport)
    }
}

pub struct WidgetBox<'a, Command>(arena::ArenaBox<'a, dyn Widget<'a, Command> + 'a>);

impl<'a, Command: 'a> WidgetBox<'a, Command> {
    pub fn new<W: Widget<'a, Command> + 'a>(arena: &'a Arena, widget: W) -> Self {
        let boxed = arena.boxed(widget);
        let raw: *mut W = arena::ArenaBox::into_raw(boxed);

        Self(unsafe { arena::ArenaBox::from_raw(raw as *mut (dyn Widget<'a, Command> + 'a)) })
    }
}

impl<'a, Command: 'a> Widget<'a, Command> for WidgetBox<'a, Command> {
    fn size(&self) -> Size {
        self.0.size()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        self.0.handle_event(arena, event, viewport)
    }

    fn overlays(&mut self) -> Vec<overlay::Overlay<'a, Command>> {
        self.0.overlays()
    }

    fn blocks_pointer(&self, point: Point) -> bool {
        self.0.blocks_pointer(point)
    }

    fn focus_data<'w>(&'w mut self) -> focus::FocusData<'w, Command>
    where
        'a: 'w,
    {
        self.0.focus_data()
    }
}

pub struct Eager<W>(pub W);

pub fn eager<W>(widget: W) -> Eager<W> {
    Eager(widget)
}

impl<'a, Command: 'a, W: Widget<'a, Command>> Widget<'a, Command> for Eager<W> {
    fn size(&self) -> Size {
        self.0.size()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        self.0.handle_event(arena, event, viewport)
    }

    fn overlays(&mut self) -> Vec<overlay::Overlay<'a, Command>> {
        self.0.overlays()
    }

    fn blocks_pointer(&self, point: Point) -> bool {
        self.0.blocks_pointer(point)
    }

    fn focus_data<'w>(&'w mut self) -> focus::FocusData<'w, Command>
    where
        'a: 'w,
    {
        self.0.focus_data()
    }
}

impl<'a, Command: 'a, W: Widget<'a, Command> + 'a> Thunk<'a, Command> for Eager<W> {
    fn size(&self) -> Size {
        Widget::size(self)
    }

    fn realize(self, arena: &'a Arena, _viewport: Rect) -> WidgetBox<'a, Command> {
        WidgetBox::new(arena, self.0)
    }
}

pub trait Widget<'a, Command> {
    fn size(&self) -> Size;

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command>;

    fn blocks_pointer(&self, point: Point) -> bool {
        let _ = point;
        true
    }

    fn overlays(&mut self) -> Vec<overlay::Overlay<'a, Command>> {
        Vec::new()
    }

    fn focus_data<'w>(&'w mut self) -> focus::FocusData<'w, Command>
    where
        'a: 'w,
        Command: 'a,
    {
        focus::FocusData::default()
    }
}

#[cfg(test)]
mod tests;
