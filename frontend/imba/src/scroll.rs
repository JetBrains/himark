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
}

pub struct ScrollWidget<ContentThunk, ContentCommand> {
    content: ContentThunk,
    viewport: Size,
    scroll_y: f32,
    glide: Option<Glide>,
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
    scrollbar: ScrollbarStyle,
    surface: crate::event::ScrollSurfaceId,
}

const GLIDE_TAU_MS: f32 = 25.0;

const GLIDE_EPSILON: f32 = 0.5;

impl<Content> ScrollView<Content> {
    pub fn new(content: Content) -> Self {
        Self {
            content,
            scroll_y: 0.0,
            glide: None,
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

                self.glide = None;
            }
            ScrollCommand::JumpTo(target) => {
                self.scroll_y = target.max(0.0);

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
                if let Some(glide) = &mut self.glide {
                    glide.last = Some(now);
                    if (glide.target - self.scroll_y).abs() <= GLIDE_EPSILON {
                        self.scroll_y = glide.target.max(0.0);
                        self.glide = None;
                    }
                }
            }
        }
    }

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let viewport = constraints.max;
        let content = self.content.layout(
            arena,
            store,
            ui,
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
            scrollbar: ui.get::<ScrollbarStyle>().copied().unwrap_or_default(),
            surface: self.surface,
            _command: PhantomData,
        }
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

    fn focus_data<'w>(&'w mut self) -> crate::focus::FocusData<'w, ScrollCommand<ContentCommand>>
    where
        'a: 'w,
    {
        let scroll_y = self.scroll_y;
        let viewport = self.viewport;
        self.content
            .focus_data()
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
        let event = event.translated(0.0, self.scroll_y);
        self.content
            .handle_event(arena, &event, self.content_viewport())
            .map(ScrollCommand::Content)
    }

    fn paint_scrollbar(&self, canvas: &Canvas) {
        let content_height = self.content.size().height;
        let max_scroll = (content_height - self.viewport.height).max(0.0);
        if max_scroll <= 0.0 {
            return;
        }

        let style = self.scrollbar;
        let track_top = style.track_inset;
        let track_height = (self.viewport.height - track_top * 2.0).max(1.0);
        let knob_height = (self.viewport.height / content_height * track_height)
            .clamp(style.min_knob.min(track_height), track_height);
        let knob_y = track_top + self.scroll_y / max_scroll * (track_height - knob_height);

        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(style.color);
        canvas.draw_round_rect(
            Rect::from_xywh(
                self.viewport.width - style.margin,
                knob_y,
                style.width,
                knob_height,
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
        let EventResult::Reveal(rect) = result else {
            return result;
        };
        let max_scroll = (self.content.size().height - self.viewport.height).max(0.0);

        let effective = self
            .glide
            .map(|glide| glide.target)
            .unwrap_or(self.scroll_y);
        let target = crate::event::reveal_scroll_target(
            effective,
            self.viewport.height,
            rect.top,
            rect.bottom,
        )
        .clamp(0.0, max_scroll);
        let distance = target - effective;
        if distance.abs() > GLIDE_EPSILON {
            if distance.abs() <= self.viewport.height * 0.6 {
                return EventResult::Command(ScrollCommand::JumpTo(target));
            }
            return EventResult::Command(ScrollCommand::GlideTo(target));
        }

        let lifted = Rect::from_ltrb(
            rect.left.clamp(0.0, self.viewport.width),
            (rect.top - target).clamp(0.0, self.viewport.height),
            rect.right.clamp(0.0, self.viewport.width),
            (rect.bottom - target).clamp(0.0, self.viewport.height),
        );
        EventResult::Reveal(lifted)
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
