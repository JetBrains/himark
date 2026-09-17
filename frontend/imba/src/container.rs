// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use skia_safe::Contains;
use skia_safe::{Rect, Size};

use crate::{
    arena::{Arena, ArenaVec},
    event::{Event, EventResult},
    overlay::Overlay,
    Thunk, ThunkBox, Widget, WidgetBox,
};

pub struct Container<'a, Command> {
    arena: &'a Arena,
    size: Size,
    children: ArenaVec<'a, (f32, f32, ThunkBox<'a, Command>)>,
}

pub fn container<'a, Command: 'a>(arena: &'a Arena, size: Size) -> Container<'a, Command> {
    Container::new(arena, size)
}

impl<'a, Command: 'a> Container<'a, Command> {
    pub fn new(arena: &'a Arena, size: Size) -> Container<'a, Command> {
        Container {
            arena,
            size,
            children: arena.vec(0),
        }
    }

    pub fn place<Child>(&mut self, x: f32, y: f32, child: Child)
    where
        Child: Thunk<'a, Command> + 'a,
    {
        self.place_boxed(x, y, ThunkBox::new(self.arena, child));
    }

    pub fn place_boxed(&mut self, x: f32, y: f32, child: ThunkBox<'a, Command>) {
        self.children.push((x, y, child));
    }

    pub fn wrap_realized<W, F>(self, wrap: F) -> impl Thunk<'a, Command> + 'a
    where
        W: Widget<'a, Command> + 'a,
        F: FnOnce(RealizedContainer<'a, Command>) -> W + 'a,
    {
        ContainerWrap {
            container: self,
            wrap,
        }
    }

    pub fn realize_into(self, viewport: Rect) -> RealizedContainer<'a, Command> {
        let arena = self.arena;
        let mut realized = RealizedContainer {
            arena,
            size: self.size,
            children: arena.vec(self.children.len()),
            overlays: Vec::new(),
        };
        for (x, y, thunk) in self.children {
            realized.place_realized(x, y, thunk, viewport);
        }
        realized
    }
}

impl<'a, Command: 'a> Thunk<'a, Command> for Container<'a, Command> {
    fn first_baseline(&self) -> Option<f32> {
        // Propagate the TOPMOST placed line (Compose merges
        // FirstBaseline with min), offset by the child's placement.
        self.children
            .iter()
            .filter_map(|(_, y, child)| child.first_baseline().map(|line| line + y))
            .fold(None, |best, line| {
                Some(match best {
                    Some(best) if best <= line => best,
                    _ => line,
                })
            })
    }

    fn size(&self) -> Size {
        self.size
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        WidgetBox::new(arena, self.realize_into(viewport))
    }
}

struct ContainerWrap<'a, Command, F> {
    container: Container<'a, Command>,
    wrap: F,
}

impl<'a, Command: 'a, W, F> Thunk<'a, Command> for ContainerWrap<'a, Command, F>
where
    W: Widget<'a, Command> + 'a,
    F: FnOnce(RealizedContainer<'a, Command>) -> W + 'a,
{
    fn size(&self) -> Size {
        self.container.size
    }

    fn first_baseline(&self) -> Option<f32> {
        self.container.first_baseline()
    }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command> {
        let ContainerWrap { container, wrap } = self;
        WidgetBox::new(arena, (wrap)(container.realize_into(viewport)))
    }
}

pub struct RealizedContainer<'a, Command> {
    arena: &'a Arena,
    size: Size,

    children: ArenaVec<'a, Child<'a, Command>>,
    overlays: Vec<Overlay<'a, Command>>,
}

struct Child<'a, Command> {
    rect: Rect,
    widget: WidgetBox<'a, Command>,
}

impl<'a, Command: 'a> RealizedContainer<'a, Command> {
    pub(crate) fn place_realized(
        &mut self,
        x: f32,
        y: f32,
        thunk: ThunkBox<'a, Command>,
        viewport: Rect,
    ) {
        let size = thunk.size();
        let rect = Rect::from_xywh(x, y, size.width, size.height);

        let child_viewport = viewport_for_child(viewport, rect).unwrap_or_default();
        let mut widget = thunk.realize(self.arena, child_viewport);

        let mut overlays = widget.overlays();
        for overlay in &mut overlays {
            overlay.translate(x, y);
        }
        self.overlays.append(&mut overlays);

        self.children.push(Child { rect, widget });
    }

    pub fn focus_data_of(&mut self, index: usize) -> crate::focus::FocusData<'_, Command> {
        match self.children.get_mut(index) {
            Some(child) => {
                let rect = child.rect;
                child.widget.focus_data().translated(rect.left, rect.top)
            }
            None => crate::focus::FocusData::default(),
        }
    }

    pub fn route_to(
        &self,
        index: usize,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        match self.children.get(index) {
            Some(child) => child.handle_event(arena, event, viewport),
            None => EventResult::Ignored,
        }
    }
}

impl<'a, Command> Widget<'a, Command> for RealizedContainer<'a, Command> {
    fn size(&self) -> Size {
        self.size
    }

    fn overlays(&mut self) -> Vec<Overlay<'a, Command>> {
        std::mem::take(&mut self.overlays)
    }

    fn blocks_pointer(&self, point: skia_safe::Point) -> bool {
        self.children
            .iter()
            .any(|child| child.blocks_pointer_at(point))
    }

    fn focus_data<'w>(&'w mut self) -> crate::focus::FocusData<'w, Command>
    where
        'a: 'w,
    {
        let mut folded = crate::focus::FocusData::default();
        for child in self.children.iter_mut().rev() {
            let rect = child.rect;
            folded = folded.merge_over(child.widget.focus_data().translated(rect.left, rect.top));
        }
        folded
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        match event {
            Event::Paint { .. }
            | Event::AnimationClock { .. }
            | Event::Settle
            | Event::ThemeChanged => {
                let mut merged = EventResult::Ignored;
                for child in self.children.iter() {
                    merged = merged.merge(child.handle_event(arena, event, viewport));
                }
                merged
            }

            Event::HitTest { point, miss } => {
                let hit = match miss {
                    true => None,
                    false => self
                        .children
                        .iter()
                        .rposition(|child| child.blocks_pointer_at(*point)),
                };
                let mut merged = EventResult::Ignored;
                for (index, child) in self.children.iter().enumerate() {
                    let event = Event::HitTest {
                        point: *point,
                        miss: Some(index) != hit,
                    };
                    merged = merged.merge(child.handle_event(arena, &event, viewport));
                }
                merged
            }
            _ => {
                for child in self.children.iter().rev() {
                    let result = child.handle_event(arena, event, viewport);
                    if !matches!(result, EventResult::Ignored) {
                        return result;
                    }
                }

                EventResult::Ignored
            }
        }
    }
}

impl<Command> Child<'_, Command> {
    fn blocks_pointer_at(&self, point: skia_safe::Point) -> bool {
        self.rect.contains(point)
            && self.widget.blocks_pointer(skia_safe::Point::new(
                point.x - self.rect.left,
                point.y - self.rect.top,
            ))
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        match event {
            Event::Paint { .. } => {
                let Some(child_viewport) = viewport_for_child(viewport, self.rect) else {
                    return EventResult::Handled;
                };
                let canvas = match event {
                    Event::Paint { canvas, .. } => *canvas,
                    _ => unreachable!(),
                };

                canvas.save();
                canvas.translate((self.rect.left, self.rect.top));
                canvas.clip_rect(Rect::from_size(self.rect.size()), None, true);
                let result = self.widget.handle_event(arena, event, child_viewport);
                canvas.restore();
                result.reveal_translated(self.rect.left, self.rect.top)
            }
            Event::MouseDown { point, .. } | Event::Scroll { point, .. } => {
                if !self.rect.contains(*point) {
                    return EventResult::Ignored;
                }
                let event = event.translated(-self.rect.left, -self.rect.top);
                self.widget
                    .handle_event(arena, &event, self.child_viewport(viewport))
                    .reveal_translated(self.rect.left, self.rect.top)
            }

            Event::MouseDrag { .. }
            | Event::MouseUp { .. }
            | Event::MouseMove { .. }
            | Event::HitTest { .. } => {
                let event = event.translated(-self.rect.left, -self.rect.top);
                self.widget
                    .handle_event(arena, &event, self.child_viewport(viewport))
                    .reveal_translated(self.rect.left, self.rect.top)
            }

            _ => self
                .widget
                .handle_event(arena, event, self.child_viewport(viewport))
                .reveal_translated(self.rect.left, self.rect.top),
        }
    }

    fn child_viewport(&self, viewport: Rect) -> Rect {
        viewport_for_child(viewport, self.rect).unwrap_or_default()
    }
}

pub fn viewport_for_child(viewport: Rect, child: Rect) -> Option<Rect> {
    let left = viewport.left.max(child.left);
    let top = viewport.top.max(child.top);
    let right = viewport.right.min(child.right);
    let bottom = viewport.bottom.min(child.bottom);
    if left >= right || top >= bottom {
        return None;
    }

    Some(Rect::from_xywh(
        left - child.left,
        top - child.top,
        right - left,
        bottom - top,
    ))
}
