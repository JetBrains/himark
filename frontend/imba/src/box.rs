use crate::{
    arena::Arena,
    event::{Event, EventResult},
    Widget,
};
use skia_safe::{Rect, Size};

impl<'a, Command: 'a, W> Widget<'a, Command> for &W
where
    W: Widget<'a, Command> + ?Sized,
{
    fn size(&self) -> Size {
        (**self).size()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        (**self).handle_event(arena, event, viewport)
    }
}

impl<'a, Command: 'a, W> Widget<'a, Command> for Box<W>
where
    W: Widget<'a, Command> + ?Sized,
{
    fn size(&self) -> Size {
        (**self).size()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        (**self).handle_event(arena, event, viewport)
    }

    fn overlays(&mut self) -> Vec<crate::overlay::Overlay<'a, Command>> {
        (**self).overlays()
    }

    fn focus_data<'w>(&'w mut self) -> crate::focus::FocusData<'w, Command>
    where
        'a: 'w,
    {
        (**self).focus_data()
    }
}
