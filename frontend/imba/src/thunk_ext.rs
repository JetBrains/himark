// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::marker::PhantomData;

use skia_safe::{Canvas, Rect, Size};

use crate::{
    arena::Arena,
    event::{Event, EventResult},
    focus::FocusData,
    overlay::{Overlay, OverlayContent, OverlayHost},
    PresentableCommand, Thunk, Widget, WidgetBox,
};

pub trait ThunkExt<'a, Command>: Thunk<'a, Command> + Sized {
    fn map<ParentCommand, F>(self, map: F) -> impl Thunk<'a, ParentCommand> + 'a
    where
        Self: 'a,
        Command: 'a,
        ParentCommand: 'a,
        F: Fn(Command) -> ParentCommand + Clone + 'a,
    {
        MapThunk {
            inner: self,
            map,
            _command: PhantomData,
        }
    }

    fn paint_below<F>(self, paint: F) -> impl Thunk<'a, Command> + 'a
    where
        Self: 'a,
        Command: 'a,
        F: Fn(&Arena, &Canvas, Rect) + 'a,
    {
        PaintThunk {
            inner: self,
            paint,
            mode: PaintMode::Below,
            _command: PhantomData,
        }
    }

    fn paint_above<F>(self, paint: F) -> impl Thunk<'a, Command> + 'a
    where
        Self: 'a,
        Command: 'a,
        F: Fn(&Arena, &Canvas, Rect) + 'a,
    {
        PaintThunk {
            inner: self,
            paint,
            mode: PaintMode::Above,
            _command: PhantomData,
        }
    }

    fn paint_instead<F>(self, paint: F) -> impl Thunk<'a, Command> + 'a
    where
        Self: 'a,
        Command: 'a,
        F: Fn(&Arena, &Canvas, Rect) + 'a,
    {
        PaintThunk {
            inner: self,
            paint,
            mode: PaintMode::Instead,
            _command: PhantomData,
        }
    }

    fn event<F>(self, event: F) -> impl Thunk<'a, Command> + 'a
    where
        Self: 'a,
        Command: 'a,
        F: for<'event> Fn(&Arena, &Event<'event>, Size) -> EventResult<Command> + 'a,
    {
        EventThunk {
            inner: self,
            event,
            _command: PhantomData,
        }
    }

    fn hit_opaque(self) -> impl Thunk<'a, Command> + 'a
    where
        Self: 'a,
        Command: 'a,
    {
        HitOpaqueThunk { inner: self }
    }

    fn focus_scope(self, focused: bool) -> impl Thunk<'a, Command> + 'a
    where
        Self: 'a,
        Command: 'a,
    {
        FocusScopeThunk {
            inner: self,
            focused,
            _command: PhantomData,
        }
    }

    fn commands<F>(self, commands: F) -> impl Thunk<'a, Command> + 'a
    where
        Self: 'a,
        Command: 'a,
        F: Fn() -> Vec<PresentableCommand<Command>> + 'a,
    {
        CommandsThunk {
            inner: self,
            commands,
            _command: PhantomData,
        }
    }

    fn overlay<F>(self, host: OverlayHost, content: F) -> impl Thunk<'a, Command> + 'a
    where
        Self: 'a,
        Command: 'a,
        F: OverlayContent<'a, Command> + 'a,
    {
        OverlayRequestThunk {
            inner: self,
            host,
            content,
            _command: PhantomData,
        }
    }

    fn overlay_host(self, key: OverlayHost) -> impl Thunk<'a, Command> + 'a
    where
        Self: 'a,
        Command: 'a,
    {
        crate::overlay::OverlayHostThunk {
            key,
            inner: self,
            _command: PhantomData,
        }
    }

    fn wrap<W, F>(self, wrap: F) -> impl Thunk<'a, Command> + 'a
    where
        Self: 'a,
        Command: 'a,
        W: Widget<'a, Command> + 'a,
        F: FnOnce(WidgetBox<'a, Command>) -> W + 'a,
    {
        WrapThunk {
            inner: self,
            wrap,
            _command: PhantomData,
        }
    }
}

impl<'a, Command, T> ThunkExt<'a, Command> for T where T: Thunk<'a, Command> + Sized {}

struct MapThunk<Inner, Command, F> {
    inner: Inner,
    map: F,
    _command: PhantomData<fn() -> Command>,
}

impl<'a, Command, ParentCommand, Inner, F> Thunk<'a, ParentCommand> for MapThunk<Inner, Command, F>
where
    Command: 'a,
    ParentCommand: 'a,
    Inner: Thunk<'a, Command> + 'a,
    F: Fn(Command) -> ParentCommand + Clone + 'a,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn first_baseline(&self) -> Option<f32> {
        self.inner.first_baseline()
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, ParentCommand> {
        let MapThunk { inner, map, .. } = self;
        WidgetBox::new(
            arena,
            crate::overlay::MappedWidget {
                inner: inner.realize(arena, viewport),
                map,
            },
        )
    }
}

struct CommandsThunk<Inner, Command, F> {
    inner: Inner,
    commands: F,
    _command: PhantomData<fn() -> Command>,
}

impl<'a, Command: 'a, Inner, F> Thunk<'a, Command> for CommandsThunk<Inner, Command, F>
where
    Inner: Thunk<'a, Command> + 'a,
    F: Fn() -> Vec<PresentableCommand<Command>> + 'a,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn first_baseline(&self) -> Option<f32> {
        self.inner.first_baseline()
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        let CommandsThunk {
            inner, commands, ..
        } = self;
        WidgetBox::new(
            arena,
            CommandsWidget {
                inner: inner.realize(arena, viewport),
                commands,
            },
        )
    }
}

struct FocusScopeThunk<Inner, Command> {
    inner: Inner,
    focused: bool,
    _command: PhantomData<fn() -> Command>,
}

impl<'a, Command: 'a, Inner> Thunk<'a, Command> for FocusScopeThunk<Inner, Command>
where
    Inner: Thunk<'a, Command> + 'a,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn first_baseline(&self) -> Option<f32> {
        self.inner.first_baseline()
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        let FocusScopeThunk { inner, focused, .. } = self;
        WidgetBox::new(
            arena,
            FocusScopeWidget {
                inner: inner.realize(arena, viewport),
                focused,
            },
        )
    }
}

struct PaintThunk<Inner, Command, F> {
    inner: Inner,
    paint: F,
    mode: PaintMode,
    _command: PhantomData<fn() -> Command>,
}

impl<'a, Command: 'a, Inner, F> Thunk<'a, Command> for PaintThunk<Inner, Command, F>
where
    Inner: Thunk<'a, Command> + 'a,
    F: Fn(&Arena, &Canvas, Rect) + 'a,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn first_baseline(&self) -> Option<f32> {
        self.inner.first_baseline()
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        let PaintThunk {
            inner, paint, mode, ..
        } = self;
        WidgetBox::new(
            arena,
            PaintWidget {
                inner: inner.realize(arena, viewport),
                paint,
                mode,
            },
        )
    }
}

struct EventThunk<Inner, Command, F> {
    inner: Inner,
    event: F,
    _command: PhantomData<fn() -> Command>,
}

impl<'a, Command: 'a, Inner, F> Thunk<'a, Command> for EventThunk<Inner, Command, F>
where
    Inner: Thunk<'a, Command> + 'a,
    F: for<'event> Fn(&Arena, &Event<'event>, Size) -> EventResult<Command> + 'a,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn first_baseline(&self) -> Option<f32> {
        self.inner.first_baseline()
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        let EventThunk { inner, event, .. } = self;
        WidgetBox::new(
            arena,
            FallbackEventWidget {
                inner: inner.realize(arena, viewport),
                event,
                arena,
            },
        )
    }
}

struct OverlayRequestThunk<Inner, Command, F> {
    inner: Inner,
    host: OverlayHost,
    content: F,
    _command: PhantomData<fn() -> Command>,
}

impl<'a, Command: 'a, Inner, F> Thunk<'a, Command> for OverlayRequestThunk<Inner, Command, F>
where
    Inner: Thunk<'a, Command> + 'a,
    F: OverlayContent<'a, Command> + 'a,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn first_baseline(&self) -> Option<f32> {
        self.inner.first_baseline()
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        let OverlayRequestThunk {
            inner,
            host,
            content,
            ..
        } = self;
        WidgetBox::new(
            arena,
            OverlayRequestWidget {
                inner: inner.realize(arena, viewport),
                host,
                content: Some(content),
            },
        )
    }
}

struct WrapThunk<Inner, Command, F> {
    inner: Inner,
    wrap: F,
    _command: PhantomData<fn() -> Command>,
}

impl<'a, Command: 'a, Inner, W, F> Thunk<'a, Command> for WrapThunk<Inner, Command, F>
where
    Inner: Thunk<'a, Command> + 'a,
    W: Widget<'a, Command> + 'a,
    F: FnOnce(WidgetBox<'a, Command>) -> W + 'a,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn first_baseline(&self) -> Option<f32> {
        self.inner.first_baseline()
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        let WrapThunk { inner, wrap, .. } = self;
        WidgetBox::new(arena, (wrap)(inner.realize(arena, viewport)))
    }
}

struct CommandsWidget<Inner, F> {
    inner: Inner,
    commands: F,
}

impl<'a, Command: 'a, Inner, F> Widget<'a, Command> for CommandsWidget<Inner, F>
where
    Inner: Widget<'a, Command>,
    F: Fn() -> Vec<PresentableCommand<Command>>,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<Overlay<'a, Command>> {
        self.inner.overlays()
    }

    fn blocks_pointer(&self, point: skia_safe::Point) -> bool {
        self.inner.blocks_pointer(point)
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        self.inner.handle_event(arena, event, viewport)
    }

    fn focus_data<'w>(&'w mut self) -> FocusData<'w, Command>
    where
        'a: 'w,
    {
        self.inner
            .focus_data()
            .merge_under(FocusData::of_commands((self.commands)()))
    }
}

struct FocusScopeWidget<Inner> {
    inner: Inner,
    focused: bool,
}

impl<'a, Command: 'a, Inner> Widget<'a, Command> for FocusScopeWidget<Inner>
where
    Inner: Widget<'a, Command>,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<Overlay<'a, Command>> {
        self.inner.overlays()
    }

    fn blocks_pointer(&self, point: skia_safe::Point) -> bool {
        self.inner.blocks_pointer(point)
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        match event {
            Event::Paint { canvas, focused } => self.inner.handle_event(
                arena,
                &Event::Paint {
                    canvas,
                    focused: *focused && self.focused,
                },
                viewport,
            ),

            Event::MouseDrag { .. } | Event::MouseUp { .. } if !self.focused => {
                EventResult::Ignored
            }
            _ => self.inner.handle_event(arena, event, viewport),
        }
    }

    fn focus_data<'w>(&'w mut self) -> FocusData<'w, Command>
    where
        'a: 'w,
    {
        match self.focused {
            true => self.inner.focus_data(),
            false => FocusData::default(),
        }
    }
}

struct OverlayRequestWidget<Inner, F> {
    inner: Inner,
    host: OverlayHost,

    content: Option<F>,
}

impl<'a, Command: 'a, Inner, F> Widget<'a, Command> for OverlayRequestWidget<Inner, F>
where
    Inner: Widget<'a, Command>,
    F: OverlayContent<'a, Command> + 'a,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        self.inner.handle_event(arena, event, viewport)
    }

    fn overlays(&mut self) -> Vec<Overlay<'a, Command>> {
        let mut overlays = self.inner.overlays();
        if let Some(content) = self.content.take() {
            overlays.push(Overlay {
                host: self.host,
                anchor: Rect::from_size(self.inner.size()),
                content: Box::new(content),
            });
        }
        overlays
    }

    fn focus_data<'w>(&'w mut self) -> FocusData<'w, Command>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}

#[derive(Clone, Copy)]
enum PaintMode {
    Below,
    Above,
    Instead,
}

struct PaintWidget<Inner, F> {
    inner: Inner,
    paint: F,
    mode: PaintMode,
}

impl<'a, Command: 'a, Inner, F> Widget<'a, Command> for PaintWidget<Inner, F>
where
    Inner: Widget<'a, Command>,
    F: Fn(&Arena, &Canvas, Rect),
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<Overlay<'a, Command>> {
        self.inner.overlays()
    }

    fn blocks_pointer(&self, point: skia_safe::Point) -> bool {
        self.inner.blocks_pointer(point)
    }

    fn focus_data<'w>(&'w mut self) -> FocusData<'w, Command>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        let (canvas, rect) = match event {
            Event::Paint { canvas, .. } => (*canvas, Rect::from_size(self.inner.size())),
            _ => return self.inner.handle_event(arena, event, viewport),
        };

        match self.mode {
            PaintMode::Below => {
                (self.paint)(arena, canvas, rect);
                match self.inner.handle_event(arena, event, viewport) {
                    EventResult::Ignored => EventResult::Handled,
                    result => result,
                }
            }
            PaintMode::Above => {
                let result = self.inner.handle_event(arena, event, viewport);
                (self.paint)(arena, canvas, rect);
                match result {
                    EventResult::Ignored => EventResult::Handled,
                    result => result,
                }
            }
            PaintMode::Instead => {
                (self.paint)(arena, canvas, rect);
                EventResult::Handled
            }
        }
    }
}

struct FallbackEventWidget<'a, Inner, F> {
    inner: Inner,
    event: F,
    arena: &'a Arena,
}

impl<'a, Command: 'a, Inner, F> Widget<'a, Command> for FallbackEventWidget<'a, Inner, F>
where
    Inner: Widget<'a, Command>,
    F: for<'event> Fn(&Arena, &Event<'event>, Size) -> EventResult<Command>,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<Overlay<'a, Command>> {
        self.inner.overlays()
    }

    fn blocks_pointer(&self, point: skia_safe::Point) -> bool {
        self.inner.blocks_pointer(point)
    }

    fn focus_data<'w>(&'w mut self) -> FocusData<'w, Command>
    where
        'a: 'w,
    {
        let size = self.inner.size();
        let data = self.inner.focus_data();
        let arena = self.arena;
        let on_key = &self.event;
        let on_text = &self.event;
        data.merge_under(FocusData {
            on_key: Some(Box::new(move |key, mods| {
                (on_key)(arena, &Event::KeyDown { key, mods }, size)
            })),
            on_text: Some(Box::new(move |text| {
                (on_text)(arena, &Event::TextInput { text }, size)
            })),
            ..FocusData::default()
        })
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        match self.inner.handle_event(arena, event, viewport) {
            EventResult::Ignored => (self.event)(arena, event, self.inner.size()),
            result => result,
        }
    }
}

struct HitOpaqueThunk<Inner> {
    inner: Inner,
}

impl<'a, Command: 'a, Inner> Thunk<'a, Command> for HitOpaqueThunk<Inner>
where
    Inner: Thunk<'a, Command> + 'a,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn first_baseline(&self) -> Option<f32> {
        self.inner.first_baseline()
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        WidgetBox::new(
            arena,
            HitOpaqueWidget {
                inner: self.inner.realize(arena, viewport),
            },
        )
    }
}

struct HitOpaqueWidget<'a, Command> {
    inner: WidgetBox<'a, Command>,
}

impl<'a, Command: 'a> Widget<'a, Command> for HitOpaqueWidget<'a, Command> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        self.inner.handle_event(arena, event, viewport)
    }

    fn overlays(&mut self) -> Vec<Overlay<'a, Command>> {
        self.inner.overlays()
    }

    fn blocks_pointer(&self, _point: skia_safe::Point) -> bool {
        true
    }

    fn focus_data<'w>(&'w mut self) -> FocusData<'w, Command>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}
