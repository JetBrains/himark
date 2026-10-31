// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! One bridge as one gpui view — the `HimarkBridgeView` of the
//! macOS shell, in Rust. Input lands as `imba::event::Event`s
//! through Himark's own dispatch; frames go out through
//! `imba_gpui::GpuiPainter`; the engine's scroll coalescer and
//! animation clock pace the roads that need pacing.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    canvas, div, prelude::*, ClipboardItem, Context, FocusHandle, KeyDownEvent,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ScrollDelta, ScrollWheelEvent, TouchPhase, Window,
};
use imba::event::Event;
use imba_gpui::events::{cursor_style_of, device_point, key_of, modifiers_of};
use imba_gpui::{GpuiPainter, PainterCaches};
use workbench::window::WindowId;

use crate::engine::{Bridge, Engine};

pub struct BridgeView {
    engine: Engine,
    bridge: Bridge,
    caches: Rc<RefCell<PainterCaches>>,
    focus: FocusHandle,
    started_at: Instant,
    modifiers: gpui::Modifiers,
    /// The shape the last hit test answered. Only RECORDED here:
    /// `set_window_cursor_style` is a paint-phase API (it panics
    /// anywhere else), so the canvas paint applies it each frame.
    cursor: Rc<Cell<gpui::CursorStyle>>,
}

impl BridgeView {
    pub fn new(engine: Engine, bridge: Bridge, cx: &mut Context<Self>) -> Self {
        engine.attach(cx.weak_entity());
        Self {
            engine,
            bridge,
            caches: Rc::new(RefCell::new(PainterCaches::default())),
            focus: cx.focus_handle(),
            started_at: Instant::now(),
            modifiers: gpui::Modifiers::default(),
            cursor: Rc::new(Cell::new(gpui::CursorStyle::Arrow)),
        }
    }

    fn wid(&self) -> WindowId {
        WindowId::from_raw(self.bridge.0)
    }

    fn now(&self) -> f64 {
        self.started_at.elapsed().as_secs_f64()
    }

    /// The viewport this bridge last laid out at — what an event
    /// dispatch measures against.
    fn viewport(&self) -> gfx::Size {
        self.engine
            .raw()
            .borrow()
            .app
            .window_viewport(self.wid())
            .unwrap_or_else(|| gfx::Size::new(1.0, 1.0))
    }

    /// One event into this bridge, through Himark's own dispatch —
    /// no key codes, no masks.
    fn dispatch(&self, event: Event<'_>, cx: &mut Context<Self>) {
        let size = self.viewport();
        let at = self.now();
        let changed =
            self.engine
                .raw()
                .borrow_mut()
                .app
                .dispatch_timed(self.wid(), event, size, at);
        if changed {
            cx.notify();
        }
    }

    fn dispatch_pointed(&self, event: Event<'_>, cx: &mut Context<Self>) {
        let size = self.viewport();
        let (changed, shape) =
            self.engine
                .raw()
                .borrow_mut()
                .app
                .dispatch_pointed(self.wid(), event, size);
        let style = cursor_style_of(shape);
        if self.cursor.replace(style) != style || changed {
            cx.notify();
        }
    }

    /// The primary shortcut chord — command on mac, control
    /// elsewhere, the same split every shell makes.
    fn primary_down(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.modifiers.platform
        } else {
            self.modifiers.control
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        self.modifiers = event.keystroke.modifiers;
        let keystroke = &event.keystroke;

        // Clipboard chords route through the platform pasteboard —
        // what the engine answers goes to the OS, not a shell-local
        // stash.
        if self.primary_down() && !self.modifiers.alt {
            let handled = match keystroke.key.as_str() {
                "c" => self
                    .with_clipboard(|client| client.copy().map(|content| content.text))
                    .flatten()
                    .map(|text| cx.write_to_clipboard(ClipboardItem::new_string(text)))
                    .is_some(),
                "x" => self
                    .with_clipboard(|client| client.cut().map(|content| content.text))
                    .flatten()
                    .map(|text| cx.write_to_clipboard(ClipboardItem::new_string(text)))
                    .is_some(),
                "v" => {
                    let text = cx
                        .read_from_clipboard()
                        .and_then(|item| item.text().map(|text| text.to_string()));
                    match text {
                        Some(text) if !text.is_empty() => self
                            .with_clipboard(|client| {
                                client.paste(&imba::clipboard::ClipboardContent { text })
                            })
                            .unwrap_or(false),
                        _ => false,
                    }
                }
                _ => false,
            };
            if handled {
                cx.notify();
                return;
            }
        }

        let (key, insertable) = key_of(keystroke);
        let mods = modifiers_of(self.modifiers);
        if let Some(key) = key {
            let size = self.viewport();
            let at = self.now();
            let changed = self.engine.raw().borrow_mut().app.dispatch_timed(
                self.wid(),
                Event::KeyDown { key, mods },
                size,
                at,
            );
            if changed {
                cx.notify();
                return;
            }
        }

        let blocked = self.primary_down() || self.modifiers.control;
        if !blocked {
            if let Some(text) = insertable {
                if desktop::is_insertable_text(&text) {
                    let size = self.viewport();
                    let at = self.now();
                    let changed = self.engine.raw().borrow_mut().app.dispatch_timed(
                        self.wid(),
                        Event::TextInput { text: &text },
                        size,
                        at,
                    );
                    if changed {
                        cx.notify();
                    }
                }
            }
        }
    }

    fn with_clipboard<R>(
        &self,
        act: impl FnOnce(&mut dyn imba::clipboard::ClipboardClient) -> R,
    ) -> Option<R> {
        self.engine
            .raw()
            .borrow_mut()
            .app
            .with_clipboard_client(self.wid(), act)
    }
}

impl Render for BridgeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }

        let engine = self.engine.clone();
        let caches = self.caches.clone();
        let cursor = self.cursor.clone();
        let started_at = self.started_at;
        let bridge_raw = self.bridge.0;

        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.key_down(event, cx);
            }))
            .on_modifiers_changed(cx.listener(
                |this, event: &ModifiersChangedEvent, _window, cx| {
                    this.modifiers = event.modifiers;
                    // The workbench re-derives hover affordances
                    // under the new chord; no position rides a
                    // modifier change, so a far-off point serves,
                    // like a leave.
                    this.dispatch_pointed(
                        Event::MouseMove {
                            point: gfx::Point::new(-1.0e6, -1.0e6),
                            mods: modifiers_of(event.modifiers),
                        },
                        cx,
                    );
                },
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.dispatch(
                        Event::MouseDown {
                            point: device_point(event.position, window.scale_factor()),
                            button: imba::event::MouseButton::Left,
                            mods: modifiers_of(this.modifiers),
                            count: event.click_count.min(3) as u8,
                        },
                        cx,
                    );
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.dispatch(
                        Event::MouseDown {
                            point: device_point(event.position, window.scale_factor()),
                            button: imba::event::MouseButton::Right,
                            mods: modifiers_of(this.modifiers),
                            count: 1,
                        },
                        cx,
                    );
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.dispatch(
                        Event::MouseUp {
                            point: device_point(event.position, window.scale_factor()),
                        },
                        cx,
                    );
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                let point = device_point(event.position, window.scale_factor());
                let mods = modifiers_of(this.modifiers);
                if event.pressed_button == Some(MouseButton::Left) {
                    this.dispatch(Event::MouseDrag { point, mods }, cx);
                } else {
                    this.dispatch_pointed(Event::MouseMove { point, mods }, cx);
                }
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                let scale = window.scale_factor();
                let point = device_point(event.position, scale);
                // The ENGINE's scroll road, not a direct dispatch:
                // its coalescer folds a trackpad's event rate into
                // one batch per tick — machinery, not marshaling.
                let (delta_x, delta_y, phase) = match event.delta {
                    ScrollDelta::Pixels(delta) => (
                        f32::from(delta.x) * scale,
                        f32::from(delta.y) * scale,
                        match event.touch_phase {
                            TouchPhase::Started => himark_api::HIMARK_SCROLL_PHASE_BEGAN,
                            TouchPhase::Moved => himark_api::HIMARK_SCROLL_PHASE_CHANGED,
                            TouchPhase::Ended => himark_api::HIMARK_SCROLL_PHASE_ENDED,
                        },
                    ),
                    ScrollDelta::Lines(lines) => (
                        lines.x * 40.0 * scale,
                        lines.y * 40.0 * scale,
                        himark_api::HIMARK_SCROLL_PHASE_NONE,
                    ),
                };
                if delta_x != 0.0
                    || delta_y != 0.0
                    || phase == himark_api::HIMARK_SCROLL_PHASE_BEGAN
                {
                    let at = this.now();
                    let changed = this.engine.raw().borrow_mut().scroll_phased_at_time(
                        this.bridge.0,
                        point.x,
                        point.y,
                        delta_x,
                        delta_y,
                        phase,
                        at,
                    );
                    if changed {
                        cx.notify();
                    }
                }
            }))
            .child(
                canvas(
                    |_bounds, _window, _cx| (),
                    move |bounds, _state, window, cx| {
                        window.set_window_cursor_style(cursor.get());
                        let scale = window.scale_factor();
                        let now_ms = started_at.elapsed().as_secs_f64() * 1000.0;
                        let mut raw = engine.raw().borrow_mut();
                        let animating = raw.animation_tick(now_ms);

                        let width = (f32::from(bounds.size.width) * scale).max(1.0);
                        let height = (f32::from(bounds.size.height) * scale).max(1.0);
                        let painter =
                            GpuiPainter::new(window, cx, bounds.origin, scale, caches.clone());
                        // The engine's draw, not a bare
                        // draw_window_sized: it applies the standing
                        // coalesced scroll before the frame.
                        let reconciling =
                            raw.draw(bridge_raw, &painter, width, height, scale);
                        drop(painter);

                        // The one line a headless check can read:
                        // the paint road ran end to end.
                        static FIRST_FRAME: std::sync::Once = std::sync::Once::new();
                        FIRST_FRAME.call_once(|| {
                            eprintln!(
                                "[himark-gpui] first frame painted {}x{} at scale {scale}",
                                width as u32, height as u32
                            );
                        });

                        if animating || reconciling {
                            window.request_animation_frame();
                        }
                    },
                )
                .size_full(),
            )
    }
}
