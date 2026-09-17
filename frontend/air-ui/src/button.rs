// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    arena::Arena, constraints::Constraints, container::container, thunk_ext::ThunkExt, Insets,
    Layout, LayoutBox, LayoutValue, Thunk, ThunkBox,
};
use skia_safe::{Contains, Size};

impl<Command, F> LayoutValue for Button<'_, Command, F> {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ControlState {
    #[default]
    Default,
    Hovered,
    Pressed,
    Focused,
    Disabled,
}

#[derive(Clone, Copy, Default)]
pub struct ButtonVisual {
    pub fill: Option<skia_safe::Color>,
    pub stroke: Option<skia_safe::Color>,
    pub outline: Option<skia_safe::Color>,
}

pub struct Button<'a, Command, F> {
    content: LayoutBox<'a, Command>,
    disabled_content: Option<LayoutBox<'a, Command>>,
    on_click: F,
    insets: Insets,
    fill: Option<skia_safe::Color>,
    stroke: Option<skia_safe::Color>,
    radius: f32,
    min_width: f32,
    max_width: f32,
    enabled: bool,
    state: ControlState,
    visuals: Option<[ButtonVisual; 5]>,
    on_state_change: Option<Box<dyn Fn(ControlState) -> Command + 'a>>,
}

impl<'a, Command: 'a, F: Fn() -> Command + 'a> Button<'a, Command, F> {
    pub fn new(arena: &'a Arena, content: impl Layout<'a, Command> + 'a, on_click: F) -> Self {
        Self {
            content: LayoutBox::new(arena, content),
            disabled_content: None,
            on_click,
            insets: Insets::xy(10.0, 4.0),
            fill: None,
            stroke: None,
            radius: 4.0,
            min_width: 0.0,
            max_width: f32::MAX,
            enabled: true,
            state: ControlState::Default,
            visuals: None,
            on_state_change: None,
        }
    }

    pub fn fill(mut self, color: skia_safe::Color) -> Self {
        self.fill = Some(color);
        self
    }

    pub fn stroke(mut self, color: skia_safe::Color) -> Self {
        self.stroke = Some(color);
        self
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    pub fn pad_content(mut self, insets: Insets) -> Self {
        self.insets = insets;
        self
    }

    pub fn min_width(mut self, min_width: f32) -> Self {
        self.min_width = min_width;
        self
    }

    pub fn max_width(mut self, max_width: f32) -> Self {
        self.max_width = max_width;
        self
    }

    pub fn disabled_content(
        mut self,
        arena: &'a Arena,
        content: impl Layout<'a, Command> + 'a,
    ) -> Self {
        self.disabled_content = Some(LayoutBox::new(arena, content));
        self
    }

    pub fn visuals(mut self, visuals: [ButtonVisual; 5]) -> Self {
        self.visuals = Some(visuals);
        self
    }

    /// Controlled interaction state; owners retain it between rebuilt views.
    pub fn state(mut self, state: ControlState) -> Self {
        self.state = state;
        self
    }

    pub fn on_state_change(mut self, callback: impl Fn(ControlState) -> Command + 'a) -> Self {
        self.on_state_change = Some(Box::new(callback));
        self
    }

    /// A disabled button swallows presses and uses its configured disabled
    /// visual and content, falling back to the normal appearance when absent.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

impl<'a, Command: 'a, F: Fn() -> Command + 'a> Layout<'a, Command> for Button<'a, Command, F> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        let enabled = self.enabled && self.state != ControlState::Disabled;
        let state = if enabled {
            self.state
        } else {
            ControlState::Disabled
        };
        let max_width = constraints.max.width.min(self.max_width);
        let x = self.insets.left + self.insets.right;
        let y = self.insets.top + self.insets.bottom;
        let inner = Constraints {
            min: Size::default(),
            max: Size::new(
                (max_width - x).max(0.0),
                (constraints.max.height - y).max(0.0),
            ),
        };
        let content = if enabled {
            self.content
        } else {
            self.disabled_content.unwrap_or(self.content)
        }
        .layout(arena, inner);
        let label = content.size();
        let size = Size::new(
            (label.width + x).max(self.min_width).min(max_width),
            (label.height + y).min(constraints.max.height),
        );
        let mut surface = container(arena, size);
        surface.place_boxed(
            ((size.width - label.width) * 0.5).max(0.0),
            ((size.height - label.height) * 0.5).max(0.0),
            content,
        );
        let Button {
            fill,
            stroke,
            radius,
            on_click,
            visuals,
            on_state_change,
            ..
        } = self;
        let visual = visuals
            .map(|styles| styles[state as usize])
            .unwrap_or(ButtonVisual {
                fill,
                stroke,
                outline: None,
            });
        let ButtonVisual {
            fill,
            stroke,
            outline,
        } = visual;
        let chrome = surface.paint_below(move |_arena, canvas, rect| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            if let Some(fill) = fill {
                paint.set_color(fill);
                canvas.draw_round_rect(rect, radius, radius, &paint);
            }
            if let Some(stroke) = stroke {
                paint.set_stroke(true);
                paint.set_stroke_width(1.0);
                paint.set_color(stroke);
                canvas.draw_round_rect(
                    rect.with_inset((0.5, 0.5)),
                    (radius - 0.5).max(0.0),
                    (radius - 0.5).max(0.0),
                    &paint,
                );
            }
        });
        let armed = chrome.event(move |_arena, event, size| {
            use imba::event::{Event, EventResult, MouseButton};
            let next = match event {
                Event::HitTest { miss: true, .. } => Some(ControlState::Default),
                Event::HitTest { miss: false, .. } if state != ControlState::Pressed => {
                    Some(ControlState::Hovered)
                }
                Event::MouseDown {
                    button: MouseButton::Left,
                    ..
                } => Some(ControlState::Pressed),
                Event::MouseUp { point } if state == ControlState::Pressed => {
                    Some(if skia_safe::Rect::from_size(size).contains(*point) {
                        ControlState::Hovered
                    } else {
                        ControlState::Default
                    })
                }
                _ => None,
            };
            let mut result = match event {
                Event::MouseDown {
                    button: MouseButton::Left,
                    ..
                } if enabled => EventResult::Command(on_click()),
                Event::MouseDown { .. } if !enabled => EventResult::Handled,
                _ => EventResult::Ignored,
            };
            if enabled {
                if let (Some(next), Some(callback)) =
                    (next.filter(|next| *next != state), &on_state_change)
                {
                    result = result.merge(EventResult::Command(callback(next)));
                }
            }
            result
        });
        ThunkBox::new(
            arena,
            armed.hit_opaque().overlay(
                imba::overlay::WINDOW,
                crate::outline::Outline {
                    color: outline.unwrap_or(skia_safe::Color::TRANSPARENT),
                    radius: radius + 1.0,
                    outset: 2.0,
                },
            ),
        )
    }
}

impl From<editor::theme::air::ControlColors> for ButtonVisual {
    fn from(colors: editor::theme::air::ControlColors) -> Self {
        Self {
            fill: Some(colors.fill.0),
            stroke: Some(colors.border.0),
            outline: Some(colors.outline.0),
        }
    }
}
