// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The minimize / maximize / close buttons a client-decorated window
//! draws itself (GNOME on Wayland draws none), at the toolbar band's
//! trailing edge. Adwaita's shape: round 24px buttons, a quiet
//! backdrop on hover, thin glyphs.

use himark_gpui::gpui::{
    canvas, div, outline, point, prelude::*, px, rgb, rgba, size, AnyElement, App, BorderStyle,
    Bounds, MouseButton, PathBuilder, Pixels, Window, WindowControls,
};

const BUTTON: f32 = 24.0;
const GAP: f32 = 8.0;
/// The last button's distance from the window's right edge.
const MARGIN: f32 = 12.0;
/// The toolbar's last word keeps this far off the first button.
const WORDS_GAP: f32 = 24.0;
const GLYPH: u32 = 0xb5b2ab;
const HOVER: u32 = 0xffffff1a;

#[derive(Clone, Copy)]
enum Button {
    Minimize,
    Maximize,
    Close,
}

/// The buttons the compositor offers, in GNOME's default order.
fn buttons(controls: WindowControls) -> Vec<Button> {
    let mut buttons = Vec::new();
    if controls.minimize {
        buttons.push(Button::Minimize);
    }
    if controls.maximize {
        buttons.push(Button::Maximize);
    }
    buttons.push(Button::Close);
    buttons
}

/// The width, in logical pixels, the toolbar keeps clear at its
/// trailing edge for these buttons.
pub fn clearance(controls: WindowControls) -> f32 {
    let count = buttons(controls).len() as f32;
    count * BUTTON + (count - 1.0) * GAP + MARGIN + WORDS_GAP
}

/// The button row, centered on a band `band` logical pixels tall.
pub fn row(controls: WindowControls, band: f32) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .right(px(MARGIN))
        .h(px(band))
        .flex()
        .items_center()
        .gap(px(GAP))
        .children(buttons(controls).into_iter().map(button))
        .into_any_element()
}

fn button(kind: Button) -> impl IntoElement {
    let id = match kind {
        Button::Minimize => "window-minimize",
        Button::Maximize => "window-maximize",
        Button::Close => "window-close",
    };
    div()
        .id(id)
        .size(px(BUTTON))
        .rounded_full()
        .occlude()
        .hover(|style| style.bg(rgba(HOVER)))
        // The press is the button's: no window move, no engine hit.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, _| match kind {
            Button::Minimize => window.minimize_window(),
            Button::Maximize => window.zoom_window(),
            Button::Close => window.remove_window(),
        })
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, cx| glyph(kind, bounds, window, cx),
            )
            .size_full(),
        )
}

fn glyph(kind: Button, bounds: Bounds<Pixels>, window: &mut Window, _cx: &mut App) {
    let center = bounds.center();
    let half = px(4.5);
    match kind {
        Button::Minimize => {
            let mut path = PathBuilder::stroke(px(1.5));
            path.move_to(point(center.x - half, center.y + px(3.0)));
            path.line_to(point(center.x + half, center.y + px(3.0)));
            paint(path, window);
        }
        Button::Maximize => {
            let side = half * 2.0;
            let square = Bounds::new(point(center.x - half, center.y - half), size(side, side));
            window.paint_quad(outline(square, rgb(GLYPH), BorderStyle::Solid));
        }
        Button::Close => {
            let mut path = PathBuilder::stroke(px(1.5));
            path.move_to(point(center.x - half, center.y - half));
            path.line_to(point(center.x + half, center.y + half));
            path.move_to(point(center.x + half, center.y - half));
            path.line_to(point(center.x - half, center.y + half));
            paint(path, window);
        }
    }
}

fn paint(path: PathBuilder, window: &mut Window) {
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(GLYPH));
    }
}
