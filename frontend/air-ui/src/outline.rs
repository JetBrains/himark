// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{arena::Arena, leaf::leaf, overlay::OverlayContent, thunk_ext::ThunkExt, ThunkBox};
use skia_safe::{Color, Paint, Point, Rect, Size};

/// Focus ink is an ordinary, non-interactive overlay. The control's measured
/// size, hit bounds and all ancestor clips keep their usual meaning.
pub(crate) struct Outline {
    pub color: Color,
    pub radius: f32,
    pub outset: f32,
}

impl<'a, Command: 'a> OverlayContent<'a, Command> for Outline {
    fn layout(
        self: Box<Self>,
        arena: &'a Arena,
        _host_size: Size,
        anchor: Rect,
    ) -> Vec<(Point, ThunkBox<'a, Command>)> {
        if self.color.a() == 0 {
            return Vec::new();
        }
        let rect = anchor.with_outset((self.outset, self.outset));
        let ink = leaf::<Command>(rect.width(), rect.height()).paint_instead(
            move |_arena, canvas, rect| {
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(self.color);
                paint.set_stroke(true);
                paint.set_stroke_width(2.0);
                canvas.draw_round_rect(
                    rect.with_inset((1.0, 1.0)),
                    self.radius,
                    self.radius,
                    &paint,
                );
            },
        );
        vec![(Point::new(rect.left, rect.top), ThunkBox::new(arena, ink))]
    }
}
