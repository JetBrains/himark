// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::ui::UiCtx;
use std::marker::PhantomData;

use skia_safe::{Canvas, Color, Contains, Paint, Rect, Size};

use crate::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult},
    store::Store,
    Thunk, View, Widget,
};

#[derive(Clone, Copy, Default)]
pub struct ScrollbarStyle {
    pub color: Color,
    pub width: f32,

    pub margin: f32,
    pub radius: f32,
    pub min_knob: f32,

    pub track_inset: f32,
}

#[derive(Clone)]
pub struct ScrollView<Content> {
    content: Content,
    scroll_y: f32,

    glide: Option<Glide>,

    /// A live knob drag: the pointer's offset within the knob at the
    /// grab. While set, drags steer the scroll instead of the content.
    drag: Option<f32>,

    surface: crate::event::ScrollSurfaceId,
}

#[derive(Clone, Copy)]
pub struct Glide {
    target: f32,
    last: Option<crate::anim::AnimationClock>,
}

pub enum ScrollCommand<ContentCommand> {
    Content(ContentCommand),
    SetScrollY(f32),

    GlideTo(f32),

    JumpTo(f32),

    GlideStep(f32, crate::anim::AnimationClock),

    /// A press on the knob (or the track: the knob jumps under the
    /// pointer first) arms dragging with the given grab offset.
    BeginKnobDrag {
        scroll_y: f32,
        grab: f32,
    },
    EndKnobDrag,
}

pub struct ScrollWidget<ContentThunk, ContentCommand> {
    content: ContentThunk,
    viewport: Size,
    scroll_y: f32,
    glide: Option<Glide>,
    drag: Option<f32>,
    scrollbar: ScrollbarStyle,
    surface: crate::event::ScrollSurfaceId,
    _command: PhantomData<fn() -> ContentCommand>,
}

impl<'a, ContentThunk, ContentCommand: 'a> Thunk<'a, ScrollCommand<ContentCommand>>
    for ScrollWidget<ContentThunk, ContentCommand>
where
    ContentThunk: Thunk<'a, ContentCommand> + 'a,
{
    fn size(&self) -> Size {
        self.viewport
    }

    fn realize(
        self,
        arena: &'a Arena,
        _viewport: Rect,
    ) -> crate::WidgetBox<'a, ScrollCommand<ContentCommand>> {
        let ScrollWidget {
            content,
            viewport,
            scroll_y,
            glide,
            drag,
            scrollbar,
            surface,
            ..
        } = self;

        let content = content.realize(
            arena,
            Rect::from_xywh(0.0, scroll_y, viewport.width, viewport.height),
        );
        crate::WidgetBox::new(
            arena,
            RealizedScroll {
                content,
                viewport,
                scroll_y,
                glide,
                drag,
                scrollbar,
                surface,
            },
        )
    }
}

pub struct RealizedScroll<'a, ContentCommand> {
    content: crate::WidgetBox<'a, ContentCommand>,
    viewport: Size,
    scroll_y: f32,
    glide: Option<Glide>,
    drag: Option<f32>,
    scrollbar: ScrollbarStyle,
    surface: crate::event::ScrollSurfaceId,
}

/// The knob's geometry this frame — present only when there is
/// something to scroll.
struct KnobBand {
    top: f32,
    height: f32,
    knob_height: f32,
    knob_y: f32,
    max_scroll: f32,
}

impl KnobBand {
    /// The scroll position that puts the knob's top at `knob_y`.
    fn scroll_at(&self, knob_y: f32) -> f32 {
        let travel = self.height - self.knob_height;
        if travel <= 0.0 {
            return 0.0;
        }
        ((knob_y - self.top) / travel * self.max_scroll).clamp(0.0, self.max_scroll)
    }
}

/// Extra reach LEFT of the knob so the 4px lane is grabbable; the
/// stripe lane sits well further in (its own inset), untouched.
const KNOB_GRAB_SLOP: f32 = 4.0;

const GLIDE_TAU_MS: f32 = 25.0;

const GLIDE_EPSILON: f32 = 0.5;

impl<Content> ScrollView<Content> {
    pub fn new(content: Content) -> Self {
        Self {
            content,
            scroll_y: 0.0,
            glide: None,
            drag: None,
            surface: crate::event::ScrollSurfaceId::mint(),
        }
    }

    pub fn content(&self) -> &Content {
        &self.content
    }

    pub fn content_mut(&mut self) -> &mut Content {
        &mut self.content
    }

    pub fn scroll_y(&self) -> f32 {
        self.scroll_y
    }

    /// Programmatic placement (constructor restores, tail pins,
    /// navigation). Content that anchors its viewport re-observes
    /// the top on its next traversal — Paint, or the settle pulse a
    /// mutating command raises (docs/editor/viewport-preservation.md §3.1).
    pub fn set_scroll_y(&mut self, scroll_y: f32) {
        self.scroll_y = scroll_y.max(0.0);
        self.glide = None;
    }
}

impl<Content> View for ScrollView<Content>
where
    Content: View,
    Content::Command: Send + 'static,
{
    type Command = ScrollCommand<Content::Command>;

    fn destroy(&mut self, store: &mut Store, fx: &mut crate::effect::Effects<'_, Self::Command>) {
        fx.scope(ScrollCommand::Content, |fx| self.content.destroy(store, fx))
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut crate::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            ScrollCommand::Content(command) => fx.scope(ScrollCommand::Content, |fx| {
                self.content.perform(store, ui, command, fx)
            }),
            ScrollCommand::SetScrollY(scroll_y) => {
                self.scroll_y = scroll_y.max(0.0);
                // The content is not told: it RE-OBSERVES on the
                // settle pulse this raise triggers — the pulse runs
                // before any later batch, so a door never anchors on
                // a top older than this move (docs §3.1).
                fx.settle();

                self.glide = None;
            }
            ScrollCommand::JumpTo(target) => {
                self.scroll_y = target.max(0.0);
                fx.settle();

                self.glide = None;
            }
            ScrollCommand::GlideTo(target) => {
                let last = self.glide.and_then(|glide| glide.last);
                self.glide = Some(Glide {
                    target: target.max(0.0),
                    last,
                });
            }
            ScrollCommand::GlideStep(next, now) => {
                self.scroll_y = next.max(0.0);
                fx.settle();
                if let Some(glide) = &mut self.glide {
                    glide.last = Some(now);
                    if (glide.target - self.scroll_y).abs() <= GLIDE_EPSILON {
                        self.scroll_y = glide.target.max(0.0);
                        self.glide = None;
                    }
                }
            }
            ScrollCommand::BeginKnobDrag { scroll_y, grab } => {
                self.scroll_y = scroll_y.max(0.0);
                fx.settle();
                self.drag = Some(grab);
                self.glide = None;
            }
            ScrollCommand::EndKnobDrag => {
                self.drag = None;
            }
        }
    }

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> crate::focus::FocusData<'w, Self::Command> {
        self.content
            .focus_data(store, ui)
            .map(ScrollCommand::Content)
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl crate::Layout<'a, Self::Command> + crate::LayoutValue + 'a {
        crate::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let viewport = constraints.max;
            let content = crate::Layout::layout(
                self.content.display(arena, store, ui),
                arena,
                Constraints {
                    min: Size::new(0.0, viewport.height),
                    max: Size::new(viewport.width, f32::MAX),
                },
            );
            let max_scroll = (content.size().height - viewport.height).max(0.0);

            ScrollWidget {
                content,
                viewport,
                scroll_y: self.scroll_y.clamp(0.0, max_scroll),
                glide: self.glide.map(|glide| Glide {
                    target: glide.target.clamp(0.0, max_scroll),
                    last: glide.last,
                }),
                drag: self.drag,
                scrollbar: ui.get::<ScrollbarStyle>().copied().unwrap_or_default(),
                surface: self.surface,
                _command: PhantomData,
            }
        })
    }
}

impl<'a, ContentCommand: 'a> Widget<'a, ScrollCommand<ContentCommand>>
    for RealizedScroll<'a, ContentCommand>
{
    fn size(&self) -> Size {
        self.viewport
    }

    fn overlays(&mut self) -> Vec<crate::overlay::Overlay<'a, ScrollCommand<ContentCommand>>> {
        let mut overlays = self.content.overlays();
        for overlay in &mut overlays {
            overlay.translate(0.0, -self.scroll_y);
        }
        crate::overlay::map_overlays(overlays, &ScrollCommand::Content)
    }

    fn layout_data<'w>(
        &'w mut self,
        target: crate::focus::SeatKey,
    ) -> crate::focus::LayoutData<'w, ScrollCommand<ContentCommand>>
    where
        'a: 'w,
    {
        let scroll_y = self.scroll_y;
        let viewport = self.viewport;
        self.content
            .layout_data(target)
            .translated(0.0, -scroll_y)
            .clipped(Rect::from_wh(viewport.width, viewport.height))
            .map(ScrollCommand::Content)
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<ScrollCommand<ContentCommand>> {
        match event {
            Event::Paint { .. } => self.paint(arena, event),
            Event::MouseDown { .. } => self.mouse_down(arena, event),

            // A live knob drag steers the scroll; the content never
            // sees the pointer until the release lands.
            Event::MouseDrag { point, .. } if self.drag.is_some() => {
                let grab = self.drag.expect("guarded above");
                match self.knob() {
                    Some(band) => EventResult::Command(ScrollCommand::SetScrollY(
                        band.scroll_at(point.y - grab),
                    )),
                    None => EventResult::Handled,
                }
            }
            Event::MouseUp { .. } if self.drag.is_some() => {
                EventResult::Command(ScrollCommand::EndKnobDrag)
            }

            Event::MouseDrag { .. }
            | Event::MouseUp { .. }
            | Event::MouseMove { .. }
            | Event::HitTest { .. } => {
                let event = event.translated(0.0, self.scroll_y);
                self.content
                    .handle_event(arena, &event, self.content_viewport())
                    .map(ScrollCommand::Content)
            }
            Event::Scroll {
                point,
                delta_x,
                delta_y,
                gesture,
            } => {
                let forwarded = Event::Scroll {
                    point: *point,
                    delta_x: *delta_x,
                    delta_y: *delta_y,
                    gesture,
                }
                .translated(0.0, self.scroll_y);
                let inner = self
                    .content
                    .handle_event(arena, &forwarded, self.content_viewport())
                    .map(ScrollCommand::Content);
                if !matches!(inner, EventResult::Ignored) {
                    return self.reveal_intercepted(inner);
                }
                if gesture.owned_by_other(self.surface) {
                    return EventResult::Ignored;
                }
                let max_scroll = (self.content.size().height - self.viewport.height).max(0.0);
                if !gesture.owned_by(self.surface) {
                    if max_scroll <= 0.0 || delta_x.abs() > delta_y.abs() {
                        return EventResult::Ignored;
                    }
                    let _ = gesture.claims(self.surface);
                }
                let next = (self.scroll_y + *delta_y).clamp(0.0, max_scroll);
                match (next - self.scroll_y).abs() > 0.1 {
                    true => EventResult::Command(ScrollCommand::SetScrollY(next)),

                    false => EventResult::Handled,
                }
            }
            Event::AnimationClock { now } => {
                let inner = self.reveal_intercepted(
                    self.content
                        .handle_event(arena, event, self.content_viewport())
                        .map(ScrollCommand::Content),
                );

                match self.glide {
                    Some(glide) if (glide.target - self.scroll_y).abs() > GLIDE_EPSILON => {
                        let dt = glide
                            .last
                            .map(|last| now.millis_since(last))
                            .unwrap_or(16.0)
                            .clamp(1.0, 200.0) as f32;
                        let factor = 1.0 - (-dt / GLIDE_TAU_MS).exp();
                        let mut next = self.scroll_y + (glide.target - self.scroll_y) * factor;

                        let remaining = glide.target - next;
                        if remaining.abs() > self.viewport.height {
                            next = glide.target - self.viewport.height.copysign(remaining);
                        }
                        inner.merge(EventResult::Command(ScrollCommand::GlideStep(next, *now)))
                    }
                    Some(glide) => inner.merge(EventResult::Command(ScrollCommand::GlideStep(
                        glide.target,
                        *now,
                    ))),
                    None => inner,
                }
            }
            _ => self.reveal_intercepted(
                self.content
                    .handle_event(arena, event, self.content_viewport())
                    .map(ScrollCommand::Content),
            ),
        }
    }
}

impl<'a, ContentCommand: 'a> RealizedScroll<'a, ContentCommand> {
    fn paint(
        &self,
        arena: &Arena,
        event: &Event<'_>,
    ) -> EventResult<ScrollCommand<ContentCommand>> {
        let canvas = match event {
            Event::Paint { canvas, .. } => *canvas,
            _ => unreachable!(),
        };

        canvas.save();
        canvas.clip_rect(Rect::from_size(self.viewport), None, true);
        canvas.translate((0.0, -self.scroll_y));
        let result = self
            .content
            .handle_event(arena, event, self.content_viewport())
            .map(ScrollCommand::Content);
        canvas.restore();

        self.paint_scrollbar(canvas);

        match result {
            EventResult::Ignored => EventResult::Handled,
            result => result,
        }
    }

    fn mouse_down(
        &self,
        arena: &Arena,
        event: &Event<'_>,
    ) -> EventResult<ScrollCommand<ContentCommand>> {
        let Event::MouseDown { point, .. } = event else {
            unreachable!()
        };
        if !Rect::from_size(self.viewport).contains(*point) {
            return EventResult::Ignored;
        }
        if let Some(command) = self.knob_grab(*point) {
            return EventResult::Command(command);
        }
        let event = event.translated(0.0, self.scroll_y);
        self.content
            .handle_event(arena, &event, self.content_viewport())
            .map(ScrollCommand::Content)
    }

    fn knob(&self) -> Option<KnobBand> {
        let content_height = self.content.size().height;
        let max_scroll = (content_height - self.viewport.height).max(0.0);
        if max_scroll <= 0.0 {
            return None;
        }
        let style = self.scrollbar;
        let top = style.track_inset;
        let height = (self.viewport.height - top * 2.0).max(1.0);
        let knob_height = (self.viewport.height / content_height * height)
            .clamp(style.min_knob.min(height), height);
        let knob_y = top + self.scroll_y / max_scroll * (height - knob_height);
        Some(KnobBand {
            top,
            height,
            knob_height,
            knob_y,
            max_scroll,
        })
    }

    fn knob_grab(&self, point: skia_safe::Point) -> Option<ScrollCommand<ContentCommand>> {
        let band = self.knob()?;
        if point.x < self.viewport.width - self.scrollbar.margin - KNOB_GRAB_SLOP {
            return None;
        }
        let on_knob = point.y >= band.knob_y && point.y < band.knob_y + band.knob_height;
        let (grab, scroll_y) = match on_knob {
            true => (point.y - band.knob_y, self.scroll_y),
            // A track press: the knob jumps under the pointer and the
            // drag is armed in the same grip.
            false => {
                let grab = band.knob_height * 0.5;
                (grab, band.scroll_at(point.y - grab))
            }
        };
        Some(ScrollCommand::BeginKnobDrag { scroll_y, grab })
    }

    fn paint_scrollbar(&self, canvas: &Canvas) {
        let Some(band) = self.knob() else {
            return;
        };
        let style = self.scrollbar;
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(style.color);
        canvas.draw_round_rect(
            Rect::from_xywh(
                self.viewport.width - style.margin,
                band.knob_y,
                style.width,
                band.knob_height,
            ),
            style.radius,
            style.radius,
            &paint,
        );
    }

    fn reveal_intercepted(
        &self,
        result: EventResult<ScrollCommand<ContentCommand>>,
    ) -> EventResult<ScrollCommand<ContentCommand>> {
        let EventResult::Reveal(reveal) = result else {
            return result;
        };
        let rect = reveal.rect;
        let max_scroll = (self.content.size().height - self.viewport.height).max(0.0);

        let effective = self
            .glide
            .map(|glide| glide.target)
            .unwrap_or(self.scroll_y);
        let target = match reveal.placement {
            crate::event::Placement::EnsureVisible => crate::event::reveal_scroll_target(
                effective,
                self.viewport.height,
                rect.top,
                rect.bottom,
            )
            .clamp(0.0, max_scroll),

            // The rect's origin becomes the viewport's corner, exactly.
            crate::event::Placement::TopLeftAt => rect.top.clamp(0.0, max_scroll),
        };
        let distance = target - effective;
        if distance.abs() > GLIDE_EPSILON {
            let jump = match reveal.motion {
                crate::event::Motion::Jump => true,
                crate::event::Motion::Auto => distance.abs() <= self.viewport.height * 0.6,
            };
            return match jump {
                true => EventResult::Command(ScrollCommand::JumpTo(target)),
                false => EventResult::Command(ScrollCommand::GlideTo(target)),
            };
        }
        if matches!(reveal.placement, crate::event::Placement::TopLeftAt) {
            // An exact placement is per-scroll business: satisfied
            // here, it must not leak into an outer scroll's anchor.
            return EventResult::Handled;
        }

        let lifted = Rect::from_ltrb(
            rect.left.clamp(0.0, self.viewport.width),
            (rect.top - target).clamp(0.0, self.viewport.height),
            rect.right.clamp(0.0, self.viewport.width),
            (rect.bottom - target).clamp(0.0, self.viewport.height),
        );
        EventResult::Reveal(crate::event::Reveal {
            rect: lifted,
            ..reveal
        })
    }

    fn content_viewport(&self) -> Rect {
        Rect::from_xywh(
            0.0,
            self.scroll_y,
            self.viewport.width,
            self.viewport.height,
        )
    }
}
