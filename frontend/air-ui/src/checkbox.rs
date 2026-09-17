// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    event::{Event, EventResult, MouseButton},
    leaf::leaf,
    thunk_ext::ThunkExt,
    Thunk,
};
use skia_safe::{Color, Paint, PathBuilder, Rect};

#[derive(Clone, Copy, Default)]
pub struct CheckboxStyle {
    pub size: f32,
    pub radius: f32,
    pub stroke: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckboxValue {
    Unchecked,
    Checked,
    Indeterminate,
}

#[derive(Clone, Copy)]
pub struct CheckboxVisual {
    pub background: Color,
    pub border: Color,
    pub icon: Color,
    pub outline: Color,
}

pub enum CheckboxCommand {
    Toggle,
}

/// Air's 16px control contains a 14px box, with a 1px border and SVG indicator.
/// Keep the value, visual state and input availability independent for catalogues.
pub fn checkbox_with_visual<'a>(
    value: CheckboxValue,
    style: CheckboxStyle,
    visual: CheckboxVisual,
    enabled: bool,
) -> impl Thunk<'a, CheckboxCommand> + 'a {
    leaf::<CheckboxCommand>(style.size, style.size)
        .paint_instead(move |_arena, canvas, rect| {
            let scale = style.size / 16.0;
            let box_rect = rect.with_inset((scale, scale));
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(visual.background);
            canvas.draw_round_rect(box_rect, style.radius, style.radius, &paint);
            paint.set_color(visual.border);
            paint.set_style(skia_safe::PaintStyle::Stroke);
            paint.set_stroke_width(style.stroke);
            let radius = (style.radius - style.stroke / 2.0).max(0.0);
            canvas.draw_round_rect(
                box_rect.with_inset((style.stroke / 2.0, style.stroke / 2.0)),
                radius,
                radius,
                &paint,
            );
            paint.set_style(skia_safe::PaintStyle::Fill);
            paint.set_color(visual.icon);
            canvas.save();
            canvas.translate((rect.left, rect.top));
            canvas.scale((scale, scale));
            match value {
                CheckboxValue::Unchecked => {}
                CheckboxValue::Indeterminate => {
                    canvas.draw_rect(Rect::from_xywh(4.0, 7.25, 8.0, 1.5), &paint);
                }
                CheckboxValue::Checked => {
                    // resources/icons/checkbox-checked.svg in the Air UI source.
                    let points = [
                        (13.3115, 4.72363),
                        (12.8057, 5.27734),
                        (7.22754, 11.3779),
                        (6.67969, 11.9785),
                        (6.12598, 11.3828),
                        (3.45117, 8.51172),
                        (2.94043, 7.96289),
                        (4.03809, 6.94043),
                        (4.54883, 7.48926),
                        (6.66895, 9.76562),
                        (11.6992, 4.26562),
                        (12.2051, 3.71191),
                    ];
                    let mut path = PathBuilder::new();
                    path.move_to(points[0]);
                    for point in &points[1..] {
                        path.line_to(*point);
                    }
                    path.close();
                    canvas.draw_path(&path.detach(), &paint);
                }
            }
            canvas.restore();
        })
        .overlay(
            imba::overlay::WINDOW,
            crate::outline::Outline {
                color: visual.outline,
                radius: style.radius + 1.0,
                outset: 1.0,
            },
        )
        .event(move |_arena, event, _size| match event {
            Event::MouseDown {
                button: MouseButton::Left,
                ..
            } if enabled => EventResult::Command(CheckboxCommand::Toggle),
            Event::MouseDown { .. } if !enabled => EventResult::Handled,
            _ => EventResult::Ignored,
        })
}
