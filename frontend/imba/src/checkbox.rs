// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use skia_safe::{Paint, PaintCap, PathBuilder, Rect};

use crate::{event::Event, event::EventResult, leaf::leaf, thunk_ext::ThunkExt, Thunk};

#[derive(Clone, Copy, Default)]
pub struct CheckboxStyle {
    pub size: f32,
    pub radius: f32,
    pub stroke: f32,
    pub border: skia_safe::Color,
    pub fill: skia_safe::Color,
    pub check: skia_safe::Color,
}

pub enum CheckboxCommand {
    Toggle,
}

pub fn checkbox<'a>(checked: bool, style: CheckboxStyle) -> impl Thunk<'a, CheckboxCommand> + 'a {
    let size = style.size;
    leaf::<CheckboxCommand>(size, size)
        .paint_instead(move |_arena, canvas, rect| {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            if checked {
                paint.set_color(style.fill);
                canvas.draw_round_rect(
                    Rect::from_xywh(rect.left, rect.top, size, size),
                    style.radius,
                    style.radius,
                    &paint,
                );
                paint.set_color(style.check);
                paint.set_style(skia_safe::PaintStyle::Stroke);
                paint.set_stroke_width(style.stroke);
                paint.set_stroke_cap(PaintCap::Round);
                let mut tick = PathBuilder::new();
                tick.move_to((rect.left + size * 0.26, rect.top + size * 0.55));
                tick.line_to((rect.left + size * 0.44, rect.top + size * 0.72));
                tick.line_to((rect.left + size * 0.76, rect.top + size * 0.30));
                canvas.draw_path(&tick.detach(), &paint);
            } else {
                paint.set_color(style.border);
                paint.set_style(skia_safe::PaintStyle::Stroke);
                paint.set_stroke_width(style.stroke);
                canvas.draw_round_rect(
                    Rect::from_xywh(
                        rect.left + style.stroke * 0.5,
                        rect.top + style.stroke * 0.5,
                        size - style.stroke,
                        size - style.stroke,
                    ),
                    style.radius,
                    style.radius,
                    &paint,
                );
            }
        })
        .event(|_arena, event, _size| match event {
            Event::MouseDown { .. } => EventResult::Command(CheckboxCommand::Toggle),
            _ => EventResult::Ignored,
        })
}
