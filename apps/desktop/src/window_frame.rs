// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench WINDOW around its bridge view — the window-only
//! dressing the macOS shell's `HimarkView` adds: a left press on the
//! toolbar band that the engine did not take moves the window, and a
//! double press there toggles maximize. GNOME draws no titlebar for
//! a Wayland client, so without this the window cannot be moved; for
//! the same reason the frame draws the window buttons itself while
//! the window is client-decorated, and the toolbar clears them. On
//! macOS the window has no title bar either: the frame stands the
//! system's traffic lights in the toolbar's lead inset.

use himark_ffi::HimarkEngineExt as _;
use himark_gpui::gpui::{
    div, prelude::*, Context, Decorations, Entity, MouseButton, MouseDownEvent, Window,
};
#[cfg(target_os = "macos")]
use himark_gpui::gpui::{point, px};
use himark_gpui::{BridgeView, Engine};

use crate::window_buttons;

pub struct WindowFrame {
    engine: Engine,
    bridge: Entity<BridgeView>,
    /// The trailing clearance last written to the engine, in
    /// physical pixels.
    clearance: f32,
    /// The toolbar height the traffic lights were last centered in,
    /// in logical pixels.
    #[cfg(target_os = "macos")]
    lights_band: f32,
}

/// The traffic lights' leading edge: AppKit's own, so they end
/// short of the toolbar's lead inset.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS_LEFT: f32 = 9.0;
/// A standard window button's frame height.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_HEIGHT: f32 = 16.0;
/// The three buttons' span, close's leading edge to zoom's
/// trailing one, as macOS 26 lays them out (earlier systems are
/// narrower).
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS_WIDTH: f32 = 60.0;
/// The air between the last button and the toolbar's first word.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS_GAP: f32 = 14.0;

impl WindowFrame {
    pub fn new(engine: Engine, bridge: Entity<BridgeView>) -> Self {
        Self {
            engine,
            bridge,
            clearance: 0.0,
            #[cfg(target_os = "macos")]
            lights_band: 0.0,
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

    /// Stand the traffic lights vertically centered in the toolbar
    /// band and have the toolbar clear them — again whenever the band's height changes (a theme, a
    /// display's scale).
    #[cfg(target_os = "macos")]
    fn center_traffic_lights(&mut self, band: f32, window: &Window) {
        if self.lights_band == band || band <= 0.0 {
            return;
        }
        self.lights_band = band;
        window.set_traffic_light_position(point(
            px(TRAFFIC_LIGHTS_LEFT),
            px((band - TRAFFIC_LIGHT_HEIGHT) / 2.0),
        ));
        // The toolbar leads past the buttons: the host's geometry,
        // in the engine's physical pixels.
        let clearance = (TRAFFIC_LIGHTS_LEFT + TRAFFIC_LIGHTS_WIDTH + TRAFFIC_LIGHTS_GAP)
            * window.scale_factor();
        self.engine
            .with_app(|app| app.set_chrome_clearance(clearance));
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
        #[cfg(target_os = "macos")]
        self.center_traffic_lights(self.toolbar_height() / scale, window);

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
