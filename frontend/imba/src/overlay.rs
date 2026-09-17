// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::VecDeque;
use std::marker::PhantomData;

use skia_safe::{Point, Rect, Size};

use crate::{
    arena::Arena,
    container::{Container, RealizedContainer},
    event::{Event, EventResult},
    Thunk, ThunkBox, Widget, WidgetBox,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct OverlayHost(pub &'static str);

pub const WINDOW: OverlayHost = OverlayHost("window");

pub struct Overlay<'a, Command> {
    pub host: OverlayHost,

    pub anchor: Rect,

    pub content: Box<dyn OverlayContent<'a, Command> + 'a>,
}

pub trait OverlayContent<'a, Command> {
    fn layout(
        self: Box<Self>,
        arena: &'a Arena,
        host_size: Size,
        anchor: Rect,
    ) -> Vec<(Point, ThunkBox<'a, Command>)>;
}

impl<'a, Command: 'a, F> OverlayContent<'a, Command> for F
where
    F: FnOnce(Size, Rect) -> Vec<(Point, ThunkBox<'a, Command>)> + 'a,
{
    fn layout(
        self: Box<Self>,
        _arena: &'a Arena,
        host_size: Size,
        anchor: Rect,
    ) -> Vec<(Point, ThunkBox<'a, Command>)> {
        (self)(host_size, anchor)
    }
}

impl<'a, Command: 'a> Overlay<'a, Command> {
    pub fn translate(&mut self, x: f32, y: f32) {
        self.anchor.offset((x, y));
    }

    pub fn map<ParentCommand: 'a, F>(self, map: F) -> Overlay<'a, ParentCommand>
    where
        F: Fn(Command) -> ParentCommand + Clone + 'a,
    {
        Overlay {
            host: self.host,
            anchor: self.anchor,
            content: Box::new(MappedContent {
                inner: self.content,
                map,
            }),
        }
    }
}

pub fn map_overlays<'a, Command: 'a, ParentCommand: 'a, F>(
    overlays: Vec<Overlay<'a, Command>>,
    map: &F,
) -> Vec<Overlay<'a, ParentCommand>>
where
    F: Fn(Command) -> ParentCommand + Clone + 'a,
{
    overlays
        .into_iter()
        .map(|overlay| overlay.map(map.clone()))
        .collect()
}

struct MappedContent<'a, Command, F> {
    inner: Box<dyn OverlayContent<'a, Command> + 'a>,
    map: F,
}

impl<'a, Command: 'a, ParentCommand: 'a, F> OverlayContent<'a, ParentCommand>
    for MappedContent<'a, Command, F>
where
    F: Fn(Command) -> ParentCommand + Clone + 'a,
{
    fn layout(
        self: Box<Self>,
        arena: &'a Arena,
        host_size: Size,
        anchor: Rect,
    ) -> Vec<(Point, ThunkBox<'a, ParentCommand>)> {
        let MappedContent { inner, map } = *self;
        inner
            .layout(arena, host_size, anchor)
            .into_iter()
            .map(|(point, thunk)| {
                let mapped = ThunkBox::new(
                    arena,
                    MappedThunk {
                        inner: thunk,
                        map: map.clone(),
                    },
                );
                (point, mapped)
            })
            .collect()
    }
}

struct MappedThunk<'a, Command, F> {
    inner: ThunkBox<'a, Command>,
    map: F,
}

impl<'a, Command: 'a, ParentCommand: 'a, F> Thunk<'a, ParentCommand> for MappedThunk<'a, Command, F>
where
    F: Fn(Command) -> ParentCommand + Clone + 'a,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, ParentCommand> {
        let MappedThunk { inner, map } = self;
        WidgetBox::new(
            arena,
            MappedWidget {
                inner: inner.realize(arena, viewport),
                map,
            },
        )
    }
}

pub(crate) struct MappedWidget<'a, Command, F> {
    pub(crate) inner: WidgetBox<'a, Command>,
    pub(crate) map: F,
}

impl<'a, Command: 'a, ParentCommand: 'a, F> Widget<'a, ParentCommand>
    for MappedWidget<'a, Command, F>
where
    F: Fn(Command) -> ParentCommand + Clone + 'a,
{
    fn size(&self) -> Size {
        Widget::size(&self.inner)
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<ParentCommand> {
        self.inner
            .handle_event(arena, event, viewport)
            .map(&self.map)
    }

    fn overlays(&mut self) -> Vec<Overlay<'a, ParentCommand>> {
        map_overlays(self.inner.overlays(), &self.map)
    }

    fn blocks_pointer(&self, point: Point) -> bool {
        self.inner.blocks_pointer(point)
    }

    fn layout_data<'w>(
        &'w mut self,
        target: crate::focus::SeatKey,
    ) -> crate::focus::LayoutData<'w, ParentCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target).map(&self.map)
    }
}

pub(crate) struct OverlayHostThunk<Inner, Command> {
    pub(crate) key: OverlayHost,
    pub(crate) inner: Inner,
    pub(crate) _command: PhantomData<fn() -> Command>,
}

impl<'a, Command: 'a, Inner> Thunk<'a, Command> for OverlayHostThunk<Inner, Command>
where
    Inner: Thunk<'a, Command> + 'a,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        let OverlayHostThunk { key, inner, .. } = self;
        let host_size = inner.size();

        let mut stacked = {
            let mut base = Container::new(arena, host_size);
            base.place(0.0, 0.0, inner);
            base.realize_into(viewport)
        };

        let mut queue: VecDeque<Overlay<'a, Command>> = stacked.overlays().into();
        let mut bubbling = Vec::new();

        while let Some(overlay) = queue.pop_front() {
            if overlay.host != key {
                bubbling.push(overlay);
                continue;
            }
            for (point, thunk) in overlay.content.layout(arena, host_size, overlay.anchor) {
                stacked.place_realized(point.x, point.y, thunk, viewport);
                queue.extend(stacked.overlays());
            }
        }

        WidgetBox::new(arena, OverlayHostWidget { stacked, bubbling })
    }
}

pub struct OverlayHostWidget<'a, Command> {
    stacked: RealizedContainer<'a, Command>,

    bubbling: Vec<Overlay<'a, Command>>,
}

impl<'a, Command: 'a> Widget<'a, Command> for OverlayHostWidget<'a, Command> {
    fn size(&self) -> Size {
        Widget::size(&self.stacked)
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        self.stacked.handle_event(arena, event, viewport)
    }

    fn overlays(&mut self) -> Vec<Overlay<'a, Command>> {
        std::mem::take(&mut self.bubbling)
    }

    fn blocks_pointer(&self, point: Point) -> bool {
        self.stacked.blocks_pointer(point)
    }

    fn layout_data<'w>(
        &'w mut self,
        target: crate::focus::SeatKey,
    ) -> crate::focus::LayoutData<'w, Command>
    where
        'a: 'w,
    {
        self.stacked.layout_data(target)
    }
}

pub mod fit;

#[cfg(test)]
mod tests;
