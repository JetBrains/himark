// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use gfx::{Point, Size};
use imba::anim::AnimationClock;
use imba::event::{Event, Key, Modifiers, MouseButton};

use crate::app::Himark;

pub fn click(app: &mut Himark, x: f32, y: f32, width: f32, height: f32) -> bool {
    app.dispatch_timed(
        app.sole_window(),
        Event::MouseDown {
            mods: Default::default(),
            point: Point::new(x, y),
            button: MouseButton::Left,
            count: 1,
        },
        Size::new(width, height),
        0.0,
    )
}

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub fn type_text(app: &mut Himark, text: &str) -> bool {
    let size = app.viewport_size();
    app.dispatch_timed(app.sole_window(), Event::TextInput { text }, size, 0.0)
}

pub fn key(app: &mut Himark, key: Key, mods: Modifiers) -> bool {
    let size = app.viewport_size();
    app.dispatch_timed(app.sole_window(), Event::KeyDown { key, mods }, size, 0.0)
}

pub fn backspace(app: &mut Himark) -> bool {
    key(app, Key::Backspace, Modifiers::default())
}

pub fn scroll(app: &mut Himark, delta_y: f32) -> bool {
    let size = app.viewport_size();
    scroll_at(app, size.width * 0.5, size.height * 0.5, delta_y)
}

pub fn mouse_move(app: &mut Himark, x: f32, y: f32) -> bool {
    mouse_move_with_mods(app, x, y, imba::event::Modifiers::default())
}

pub fn mouse_move_with_mods(
    app: &mut Himark,
    x: f32,
    y: f32,
    mods: imba::event::Modifiers,
) -> bool {
    let size = app.viewport_size();
    app.dispatch(
        app.sole_window(),
        Event::MouseMove {
            point: Point::new(x, y),
            mods,
        },
        size,
    )
}

pub fn scroll_at(app: &mut Himark, x: f32, y: f32, delta_y: f32) -> bool {
    let gesture = imba::event::ScrollGesture::default();
    scroll_with(app, &gesture, x, y, delta_y)
}

/// A scroll riding a CALLER-HELD gesture — the touch-pan shape: one
/// gesture spans the whole pan, the first claim owns every delta.
pub fn scroll_with(
    app: &mut Himark,
    gesture: &imba::event::ScrollGesture,
    x: f32,
    y: f32,
    delta_y: f32,
) -> bool {
    let size = app.viewport_size();
    app.dispatch_timed(
        app.sole_window(),
        Event::Scroll {
            delta_x: 0.0,
            point: Point::new(x, y),
            delta_y,
            gesture,
        },
        size,
        0.0,
    )
}

pub fn drag(app: &mut Himark, x: f32, y: f32) -> bool {
    let size = app.viewport_size();
    app.dispatch_timed(
        app.sole_window(),
        Event::MouseDrag {
            mods: Default::default(),
            point: Point::new(x, y),
        },
        size,
        0.0,
    )
}

pub fn mouse_up(app: &mut Himark, x: f32, y: f32) -> bool {
    let size = app.viewport_size();
    app.dispatch_timed(
        app.sole_window(),
        Event::MouseUp {
            point: Point::new(x, y),
        },
        size,
        0.0,
    )
}

pub fn animate(app: &mut Himark, now: AnimationClock) -> bool {
    let size = app.viewport_size();
    app.dispatch(app.sole_window(), Event::AnimationClock { now }, size)
}
