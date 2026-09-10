use imba::anim::AnimationClock;
use imba::event::{Event, Key, Modifiers, MouseButton};
use skia_safe::{Point, Size};

use crate::Application;

pub fn click(app: &mut Application, x: f32, y: f32, width: f32, height: f32) -> bool {
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

pub fn type_text(app: &mut Application, text: &str) -> bool {
    let size = app.viewport_size();
    app.dispatch_timed(app.sole_window(), Event::TextInput { text }, size, 0.0)
}

pub fn key(app: &mut Application, key: Key, mods: Modifiers) -> bool {
    let size = app.viewport_size();
    app.dispatch_timed(app.sole_window(), Event::KeyDown { key, mods }, size, 0.0)
}

pub fn backspace(app: &mut Application) -> bool {
    key(app, Key::Backspace, Modifiers::default())
}

pub fn scroll(app: &mut Application, delta_y: f32) -> bool {
    let size = app.viewport_size();
    scroll_at(app, size.width * 0.5, size.height * 0.5, delta_y)
}

pub fn mouse_move(app: &mut Application, x: f32, y: f32) -> bool {
    let size = app.viewport_size();
    app.dispatch(
        app.sole_window(),
        Event::MouseMove {
            point: Point::new(x, y),
        },
        size,
    )
}

pub fn scroll_at(app: &mut Application, x: f32, y: f32, delta_y: f32) -> bool {
    let gesture = imba::event::ScrollGesture::default();
    let size = app.viewport_size();
    app.dispatch_timed(
        app.sole_window(),
        Event::Scroll {
            delta_x: 0.0,
            point: Point::new(x, y),
            delta_y,
            gesture: &gesture,
        },
        size,
        0.0,
    )
}

pub fn drag(app: &mut Application, x: f32, y: f32) -> bool {
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

pub fn mouse_up(app: &mut Application, x: f32, y: f32) -> bool {
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

pub fn animate(app: &mut Application, now: AnimationClock) -> bool {
    let size = app.viewport_size();
    app.dispatch(app.sole_window(), Event::AnimationClock { now }, size)
}
