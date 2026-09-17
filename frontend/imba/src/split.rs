// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::ui::UiCtx;
use crate::{
    arena::Arena,
    constraints::Constraints,
    container::{container, Container},
    event::{Event, EventResult},
    store::Store,
    thunk_ext::ThunkExt,
    View, Widget,
};
use skia_safe::{Contains, Rect, Size};

#[derive(Clone)]
pub struct SplitView<First, Second> {
    arrangement: Arrangement,

    ratio: f32,
    focused: Pane,
    first: First,
    second: Second,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arrangement {
    Row,

    Column,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    First,
    Second,
}

pub enum SplitCommand<FirstCommand, SecondCommand> {
    First(FirstCommand),
    Second(SecondCommand),

    Focus(Pane, Option<Box<SplitCommand<FirstCommand, SecondCommand>>>),
}

impl<First, Second> SplitView<First, Second> {
    pub fn new(arrangement: Arrangement, first: First, second: Second) -> Self {
        Self {
            arrangement,
            ratio: 0.5,
            focused: Pane::First,
            first,
            second,
        }
    }

    pub fn row(first: First, second: Second) -> Self {
        Self::new(Arrangement::Row, first, second)
    }

    pub fn column(first: First, second: Second) -> Self {
        Self::new(Arrangement::Column, first, second)
    }

    pub fn with_ratio(mut self, ratio: f32) -> Self {
        self.ratio = ratio.clamp(0.0, 1.0);
        self
    }

    pub fn ratio(&self) -> f32 {
        self.ratio
    }

    pub fn arrangement(&self) -> Arrangement {
        self.arrangement
    }

    pub fn focused(&self) -> Pane {
        self.focused
    }

    pub fn focus(&mut self, pane: Pane) {
        self.focused = pane;
    }

    pub fn first(&self) -> &First {
        &self.first
    }

    pub fn first_mut(&mut self) -> &mut First {
        &mut self.first
    }

    pub fn second(&self) -> &Second {
        &self.second
    }

    pub fn second_mut(&mut self) -> &mut Second {
        &mut self.second
    }

    pub fn into_panes(self) -> (First, Second) {
        (self.first, self.second)
    }

    pub fn divider_rect(&self, size: Size) -> Rect {
        match self.arrangement {
            Arrangement::Row => {
                let first_width = (size.width * self.ratio).floor();
                Rect::from_xywh(first_width, 0.0, 1.0, size.height)
            }
            Arrangement::Column => {
                let first_height = (size.height * self.ratio).floor();
                Rect::from_xywh(0.0, first_height, size.width, 1.0)
            }
        }
    }
}

impl<First, Second> View for SplitView<First, Second>
where
    First: View,
    Second: View,
    First::Command: Send + 'static,
    Second::Command: Send + 'static,
{
    type Command = SplitCommand<First::Command, Second::Command>;

    fn destroy(&mut self, store: &mut Store, fx: &mut crate::effect::Effects<'_, Self::Command>) {
        fx.scope(SplitCommand::First, |fx| self.first.destroy(store, fx));
        fx.scope(SplitCommand::Second, |fx| self.second.destroy(store, fx));
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut crate::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            SplitCommand::First(command) => fx.scope(SplitCommand::First, |fx| {
                self.first.perform(store, ui, command, fx)
            }),
            SplitCommand::Second(command) => fx.scope(SplitCommand::Second, |fx| {
                self.second.perform(store, ui, command, fx)
            }),
            SplitCommand::Focus(pane, then) => {
                self.focused = pane;
                if let Some(command) = then {
                    self.perform(store, ui, *command, fx);
                }
            }
        }
    }

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> crate::focus::FocusData<'w, Self::Command> {
        match self.focused {
            Pane::First => self.first.focus_data(store, ui).map(SplitCommand::First),
            Pane::Second => self.second.focus_data(store, ui).map(SplitCommand::Second),
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl crate::Layout<'a, Self::Command> + crate::LayoutValue + 'a {
        crate::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let divider = self.divider_rect(size);
            let (first_size, second_origin) = match self.arrangement {
                Arrangement::Row => (Size::new(divider.left, size.height), (divider.left, 0.0)),
                Arrangement::Column => (Size::new(size.width, divider.top), (0.0, divider.top)),
            };
            let second_size =
                Size::new(size.width - second_origin.0, size.height - second_origin.1);
            let first_rect = Rect::from_xywh(0.0, 0.0, first_size.width, first_size.height);
            let second_rect = Rect::from_xywh(
                second_origin.0,
                second_origin.1,
                second_size.width,
                second_size.height,
            );

            let first = crate::Layout::layout(
                self.first.display(arena, store, ui),
                arena,
                Constraints::tight(first_size),
            )
            .map(SplitCommand::First)
            .focus_scope(self.focused == Pane::First);
            let second = crate::Layout::layout(
                self.second.display(arena, store, ui),
                arena,
                Constraints::tight(second_size),
            )
            .map(SplitCommand::Second)
            .focus_scope(self.focused == Pane::Second);

            let mut panes = container(arena, size);
            panes.place(first_rect.left, first_rect.top, first);
            panes.place(second_rect.left, second_rect.top, second);

            SplitWidget {
                panes,
                first_rect,
                second_rect,
                focused: self.focused,
            }
        })
    }
}

struct SplitWidget<'a, FirstCommand, SecondCommand> {
    panes: Container<'a, SplitCommand<FirstCommand, SecondCommand>>,
    first_rect: Rect,
    second_rect: Rect,
    focused: Pane,
}

impl<'a, FirstCommand: 'a, SecondCommand: 'a>
    crate::Thunk<'a, SplitCommand<FirstCommand, SecondCommand>>
    for SplitWidget<'a, FirstCommand, SecondCommand>
{
    fn size(&self) -> Size {
        crate::Thunk::size(&self.panes)
    }

    fn realize(
        self,
        arena: &'a Arena,
        viewport: Rect,
    ) -> crate::WidgetBox<'a, SplitCommand<FirstCommand, SecondCommand>> {
        let SplitWidget {
            panes,
            first_rect,
            second_rect,
            focused,
        } = self;
        crate::WidgetBox::new(
            arena,
            RealizedSplit {
                panes: panes.realize_into(viewport),
                first_rect,
                second_rect,
                focused,
            },
        )
    }
}

struct RealizedSplit<'a, FirstCommand, SecondCommand> {
    panes: crate::container::RealizedContainer<'a, SplitCommand<FirstCommand, SecondCommand>>,
    first_rect: Rect,
    second_rect: Rect,
    focused: Pane,
}

impl<'a, FirstCommand: 'a, SecondCommand: 'a> Widget<'a, SplitCommand<FirstCommand, SecondCommand>>
    for RealizedSplit<'a, FirstCommand, SecondCommand>
{
    fn size(&self) -> Size {
        Widget::size(&self.panes)
    }

    fn overlays(
        &mut self,
    ) -> Vec<crate::overlay::Overlay<'a, SplitCommand<FirstCommand, SecondCommand>>> {
        self.panes.overlays()
    }

    fn layout_data<'w>(
        &'w mut self,
        target: crate::focus::SeatKey,
    ) -> crate::focus::LayoutData<'w, SplitCommand<FirstCommand, SecondCommand>>
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
    ) -> EventResult<SplitCommand<FirstCommand, SecondCommand>> {
        match event {
            Event::Paint { .. }
            | Event::Scroll { .. }
            | Event::AnimationClock { .. }
            | Event::ThemeChanged => self.panes.handle_event(arena, event, viewport),
            Event::MouseDown { point, .. } => {
                let target = if self.first_rect.contains(*point) {
                    Some(Pane::First)
                } else if self.second_rect.contains(*point) {
                    Some(Pane::Second)
                } else {
                    None
                };

                let result = self.panes.handle_event(arena, event, viewport);
                match (result, target) {
                    (result, None) => result,

                    (EventResult::Command(command), Some(pane)) => {
                        EventResult::Command(SplitCommand::Focus(pane, Some(Box::new(command))))
                    }
                    (_, Some(pane)) => EventResult::Command(SplitCommand::Focus(pane, None)),
                }
            }
            _ => {
                let index = match self.focused {
                    Pane::First => 0,
                    Pane::Second => 1,
                };
                self.panes.route_to(index, arena, event, viewport)
            }
        }
    }
}
