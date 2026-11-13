// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench WINDOW around its bridge view — the window-only
//! dressing the macOS shell's `HimarkView` adds: a left press on the
//! toolbar band that the engine did not take moves the window, and a
//! double press there toggles maximize. GNOME draws no titlebar for
//! a Wayland client, so without this the window cannot be moved; for
//! the same reason the frame draws the window buttons itself while
//! the window is client-decorated, and the toolbar clears them.

use himark_ffi::HimarkEngineExt as _;
use himark_gpui::gpui::{
    div, prelude::*, Context, Decorations, Entity, MouseButton, MouseDownEvent, Window,
};
use himark_gpui::{BridgeView, Engine};

use crate::window_buttons;

pub struct WindowFrame {
    engine: Engine,
    bridge: Entity<BridgeView>,
    /// The trailing clearance last written to the engine, in
    /// physical pixels.
    clearance: f32,
}

impl WindowFrame {
    pub fn new(engine: Engine, bridge: Entity<BridgeView>) -> Self {
        Self {
            engine,
            bridge,
            clearance: 0.0,
        }
    }

    fn toolbar_height(&self) -> f32 {
        self.engine.with_app(|app| app.toolbar_height())
    }

    /// Tell the toolbar how much of its trailing edge the buttons
    /// take, and repaint the bridge when that changed.
    fn clear(&mut self, clearance: f32, cx: &mut Context<Self>) {
        if self.clearance == clearance {
            return;
        }
        self.clearance = clearance;
        self.engine.with_app(|app| {
            app.ui_ctx()
                .env(hikit::chrome::ChromeTrailingClearance::default)
                .set(clearance)
        });
        self.bridge.update(cx, |_, cx| cx.notify());
    }

    /// The press reached here, so the engine missed it (the bridge
    /// view stops the ones it takes).
    fn missed_press(&mut self, event: &MouseDownEvent, window: &mut Window) {
        let y = f32::from(event.position.y) * window.scale_factor();
        if y > self.toolbar_height() {
            return;
        }
        if event.click_count == 2 {
            window.zoom_window();
        } else {
            window.start_window_move();
        }
    }
}

impl Render for WindowFrame {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
        let controls = window.window_controls();
        let clearance = match client_decorated {
            true => window_buttons::clearance(controls) * scale,
            false => 0.0,
        };
        self.clear(clearance, cx);

        div()
            .size_full()
            .relative()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, _cx| {
                    this.missed_press(event, window);
                }),
            )
            .child(self.bridge.clone())
            .when(client_decorated, |frame| {
                frame.child(window_buttons::row(controls, self.toolbar_height() / scale))
            })
    }
}
