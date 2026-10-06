// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::marker::PhantomData;

use skia_safe::{Rect, Size};

use crate::{
    arena::Arena,
    event::{Event, EventResult},
    Widget,
};

pub struct Leaf<'a, Command> {
    size: Size,
    _marker: PhantomData<(&'a (), fn() -> Command)>,
}

pub fn leaf<'a, Command>(width: f32, height: f32) -> crate::Eager<Leaf<'a, Command>> {
    crate::Eager(Leaf {
        size: Size::new(width, height),
        _marker: PhantomData,
    })
}

impl<'a, Command> Widget<'a, Command> for Leaf<'a, Command> {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        _event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<Command> {
        EventResult::Ignored
    }

    fn blocks_pointer(&self, _point: skia_safe::Point) -> bool {
        false
    }
}
