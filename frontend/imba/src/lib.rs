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
pub mod ime;
pub mod lazy;
pub mod leaf;
pub mod list;
pub mod overlay;
pub mod perf;
pub mod scroll;
pub mod split;
pub mod stack;
pub mod store;
pub mod image;
pub mod svg;
pub mod tooltip;
pub mod thunk_ext;
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

pub trait View {
    type Command;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut effect::Effects<'_, Self::Command>,
    );

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a;

    fn destroy(&mut self, store: &mut Store, fx: &mut effect::Effects<'_, Self::Command>) {
        let _ = (store, fx);
    }
}

pub trait Thunk<'a, Command> {
    fn size(&self) -> Size;

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

    fn realize(mut self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        self.0.realize_dyn(arena, viewport)
    }
}

trait DynThunk<'a, Command> {
    fn size(&self) -> Size;
    fn realize_dyn(&mut self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command>;
}

struct Slot<T>(Option<T>);

impl<'a, Command: 'a, T: Thunk<'a, Command>> DynThunk<'a, Command> for Slot<T> {
    fn size(&self) -> Size {
        self.0.as_ref().expect("realized twice").size()
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
