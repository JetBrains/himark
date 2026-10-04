// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{anim::{Animation, AnimationClock, Easing, Motion}, arena::Arena, constraints::Constraints, container::container, event::{Event, EventResult}, leaf::leaf, store::Store, thunk_ext::ThunkExt, ui::UiCtx, View};
use skia_safe::{Paint, Rect, Size};

use hikit::modal::ModalRequest;
use hikit::modal::ModalView;

pub const DOCK_WIDTH: f32 = 600.0;

pub const DOCK_MIN_WIDTH: f32 = 240.0;

pub const DOCK_MAX_RATIO: f32 = 0.6;

const SLIDE_MS: f64 = 160.0;

const HANDLE_REACH: f32 = 4.0;

#[derive(Clone)]
pub enum DockCommand {
    /// The dock's OWN header buttons (the `ToolbarSide::Right`
    /// registrations live here now).
    HeaderButton(usize),

    Tick(AnimationClock),

    BeginResize,

    Resize(f32),

    EndResize,

    Content(imba::dyn_view::DynCommand),
}

impl std::fmt::Display for DockCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockCommand::Content(command) => command.fmt(out),
            DockCommand::HeaderButton(_) => out.write_str("dock header button"),
            DockCommand::Tick(_) => out.write_str("dock tick"),
            DockCommand::BeginResize => out.write_str("dock begin resize"),
            DockCommand::Resize(_) => out.write_str("dock resize"),
            DockCommand::EndResize => out.write_str("dock end resize"),
        }
    }
}

pub(crate) struct Dock {
    content: Box<dyn ModalView>,

    owner: &'static str,

    width: f32,

    reveal: Animation<f32>,

    closing: bool,

    resizing: bool,

    request: Option<ModalRequest>,

    /// A pressed header button's registered command id, drained by
    /// the app loop like any toolbar request.
    command: Option<&'static str>,
}

impl Clone for Dock {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            owner: self.owner,
            width: self.width,
            reveal: self.reveal,
            closing: self.closing,
            resizing: self.resizing,

            request: None,
            command: None,
        }
    }
}

impl Dock {
    pub(crate) fn new(content: Box<dyn ModalView>, owner: &'static str, width: f32) -> Self {
        let mut reveal = Animation::done(
            0.0,
            Motion::Ease {
                duration_ms: SLIDE_MS,
                easing: Easing::EaseOut,
            },
        );
        reveal.set(width);
        Self {
            content,
            owner,
            width,
            reveal,
            closing: false,
            resizing: false,
            request: None,
            command: None,
        }
    }

    pub(crate) fn content(&self) -> &dyn ModalView {
        self.content.as_ref()
    }

    pub(crate) fn content_mut(&mut self) -> &mut Box<dyn ModalView> {
        &mut self.content
    }

    pub(crate) fn owner(&self) -> &'static str {
        self.owner
    }

    pub(crate) fn swap(
        &mut self,
        content: Box<dyn ModalView>,
        owner: &'static str,
    ) -> Box<dyn ModalView> {
        if self.closing {
            self.closing = false;
            self.reveal.set(self.width);
        }
        self.owner = owner;
        std::mem::replace(&mut self.content, content)
    }

    pub(crate) fn revealed(&self) -> f32 {
        self.reveal.value()
    }

    pub(crate) fn target_width(&self) -> f32 {
        match self.closing {
            true => 0.0,
            false => self.width,
        }
    }

    pub(crate) fn width(&self) -> f32 {
        self.width
    }

    pub(crate) fn roll_away(&mut self) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.resizing = false;
        self.reveal.set(0.0);
    }

    pub(crate) fn settle(&mut self) {
        self.resizing = false;
        self.reveal.jump(self.width);
    }
}

impl View for Dock {
    type Command = DockCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::ui::UiCtx,
    ) -> imba::focus::FocusData<'w, DockCommand> {
        self.content
            .as_ref()
            .focus_data_dyn(store, ui)
            .map(DockCommand::Content)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        fx.scope(DockCommand::Content, |fx| {
            imba::dyn_view::DynView::destroy_dyn(self.content_mut().as_mut(), store, fx)
        })
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: DockCommand,
        fx: &mut imba::effect::Effects<'_, DockCommand>,
    ) {
        match command {
            DockCommand::Tick(now) => {
                self.reveal.advance(now);
                if self.closing && !self.reveal.running() {
                    self.request = Some(ModalRequest::Close);
                }
            }
            DockCommand::BeginResize => {
                self.resizing = true;
            }
            DockCommand::Resize(width) => {
                self.width = width;
                self.reveal.jump(width);
            }
            DockCommand::EndResize => {
                self.resizing = false;
            }
            DockCommand::Content(command) => fx.scope(DockCommand::Content, |fx| {
                self.content.as_mut().perform_dyn(store, ui, command, fx)
            }),
            DockCommand::HeaderButton(index) => {
                if let Some(button) = crate::toolbar::ToolbarButtons::of(store).iter().nth(index) {
                    self.command = Some(button.command);
                }
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let revealed = self.revealed();
            let edge = size.width - revealed;
            let mut surface = container(arena, size);

            let theme = ::editor::env::Themes::of(store);
            let chrome = &theme.ui().peeker;
            let body_bg = chrome.background.0;
            let rule = chrome.rule.0;
            let body_width = self.width.max(1.0);
            let mut body = container(arena, Size::new(body_width, size.height));
            let backdrop = leaf::<DockCommand>(body_width, size.height)
                .paint_instead(move |_arena, canvas, rect| {
                    let mut paint = Paint::default();
                    paint.set_color(body_bg);
                    canvas.draw_rect(rect, &paint);
                    let mut hairline = Paint::default();
                    hairline.set_color(rule);
                    canvas.draw_rect(
                        Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                        &hairline,
                    );
                })
                .event(|_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::Handled,
                    _ => EventResult::Ignored,
                })
                .hit_opaque();
            body.place(0.0, 0.0, backdrop);

            // The dock's OWN header: its buttons, right-aligned, the
            // active owner pressed — the window has no toolbar.
            let bar = theme.ui().toolbar.clone();
            let header_h = bar.height;
            let buttons = crate::toolbar::ToolbarButtons::of(store);
            let owner = self.owner;
            {
                let bar = bar.clone();
                let header = leaf::<DockCommand>(body_width, header_h).paint_below(
                    move |_arena, canvas, rect| {
                        let mut paint = Paint::default();
                        paint.set_color(bar.background.0);
                        canvas.draw_rect(rect, &paint);
                        paint.set_anti_alias(false);
                        paint.set_color(bar.rule.0);
                        canvas.draw_rect(
                            Rect::from_xywh(rect.left, rect.bottom - 1.0, rect.width(), 1.0),
                            &paint,
                        );
                        // The dock's SEPARATOR runs through its header
                        // too — the column edge is one line, top to
                        // bottom.
                        canvas.draw_rect(
                            Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                            &paint,
                        );
                    },
                );
                body.place(0.0, 0.0, header);
            }
            let mut x = body_width
                - bar.button_inset
                - buttons
                    .iter()
                    .filter(|button| matches!(button.side, crate::toolbar::ToolbarSide::Right))
                    .count() as f32
                    * bar.button_size;
            for (index, button) in buttons.iter().enumerate() {
                if !matches!(button.side, crate::toolbar::ToolbarSide::Right) {
                    continue;
                }
                let glyph = button.glyph.clone();
                let pressed = button.command == owner;
                let bar = bar.clone();
                let cell = leaf::<DockCommand>(bar.button_size, header_h)
                    .paint_below(move |_arena, canvas, rect| {
                        let mut paint = Paint::default();
                        paint.set_anti_alias(false);
                        paint.set_color(bar.rule.0);
                        canvas.draw_rect(
                            Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                            &paint,
                        );
                        if pressed {
                            paint.set_color(bar.pressed_fill.0);
                            canvas.draw_rect(
                                Rect::from_xywh(
                                    rect.left,
                                    rect.top,
                                    rect.width(),
                                    rect.height() - 1.0,
                                ),
                                &paint,
                            );
                        }
                        paint.set_anti_alias(true);
                        let square = Rect::from_xywh(
                            rect.left,
                            rect.top + (rect.height() - rect.width()) * 0.5,
                            rect.width(),
                            rect.width(),
                        );
                        let inset = (rect.width() * 0.25).max(1.0);
                        glyph(canvas, square.with_inset((inset, inset)), bar.glyph_color.0);
                    })
                    .event(move |_arena, event, _size| match event {
                        Event::MouseDown { .. } => {
                            EventResult::Command(DockCommand::HeaderButton(index))
                        }
                        _ => EventResult::Ignored,
                    });
                body.place(x, 0.0, cell);
                x += bar.button_size;
            }

            let content = self
                .content
                .as_ref()
                .layout_dyn(
                    arena,
                    store,
                    ui,
                    Constraints::tight(Size::new(
                        (body_width - 1.0).max(1.0),
                        (size.height - header_h).max(1.0),
                    )),
                )
                .map(DockCommand::Content);
            body.place(1.0, header_h, content);
            surface.place(edge, 0.0, body);

            let animating = self.reveal.running();
            let resizing = self.resizing;
            let surface_width = size.width;
            let handle = leaf::<DockCommand>(HANDLE_REACH * 2.0, size.height).event(
                move |_arena, event, _leaf_size| match event {
                    Event::AnimationClock { now } if animating => {
                        EventResult::Command(DockCommand::Tick(*now))
                    }
                    Event::MouseDown { .. } if !animating => {
                        EventResult::Command(DockCommand::BeginResize)
                    }
                    Event::MouseDrag { point, .. } if resizing => {
                        let width = (surface_width - (edge - HANDLE_REACH) - point.x)
                            .clamp(DOCK_MIN_WIDTH, surface_width * DOCK_MAX_RATIO);
                        EventResult::Command(DockCommand::Resize(width))
                    }
                    Event::MouseUp { .. } if resizing => {
                        EventResult::Command(DockCommand::EndResize)
                    }
                    _ => EventResult::Ignored,
                },
            );
            surface.place(edge - HANDLE_REACH, 0.0, handle);

            if animating {
                let clock =
                    leaf::<DockCommand>(1.0, 1.0).event(move |_arena, event, _size| match event {
                        Event::AnimationClock { now } => {
                            EventResult::Command(DockCommand::Tick(*now))
                        }
                        _ => EventResult::Ignored,
                    });
                surface.place(0.0, 0.0, clock);
            }
            surface
        })
    }
}

impl Dock {
    pub(crate) fn take_command(&mut self) -> Option<&'static str> {
        self.command.take()
    }

    pub(crate) fn take_request(&mut self) -> Option<ModalRequest> {
        if let Some(request) = self.request.take() {
            return Some(request);
        }
        match self.content.take_request() {
            Some(ModalRequest::Close) => {
                self.roll_away();
                None
            }
            other => other,
        }
    }

    pub(crate) fn release_widgets(
        &mut self,
    ) -> Vec<(hikit::panel::WidgetOrigin, Box<dyn hikit::panel::DynPanelView>)> {
        self.content.release_widgets()
    }
}
