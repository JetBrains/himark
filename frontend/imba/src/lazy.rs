// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::marker::PhantomData;

use gfx::{Rect, Size};

use crate::{arena::Arena, Widget, WidgetBox};

pub struct Lazy<Command, Layout, Content> {
    size: Size,
    layout: Layout,
    _marker: PhantomData<fn() -> (Command, Content)>,
}

pub fn lazy<'a, Command, Layout, Content>(
    size: Size,
    layout: Layout,
) -> Lazy<Command, Layout, Content>
where
    Layout: Fn(Rect) -> Content,
    Content: Widget<'a, Command>,
{
    Lazy {
        size,
        layout,
        _marker: PhantomData,
    }
}

impl<'a, Command: 'a, Layout, Content> crate::Thunk<'a, Command> for Lazy<Command, Layout, Content>
where
    Layout: Fn(Rect) -> Content,
    Content: Widget<'a, Command> + 'a,
{
    fn size(&self) -> Size {
        self.size
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        WidgetBox::new(arena, (self.layout)(viewport))
    }
}
