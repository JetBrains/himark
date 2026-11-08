// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! One bridge as one gpui view — the `HimarkBridgeView` of the
//! macOS shell, in Rust. Input lands as `imba::event::Event`s
//! through Himark's own bridge dispatch; frames go out through
//! `gfx_gpui::GpuiPainter`; the engine's scroll coalescer and
//! animation clock pace the roads that need pacing.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gfx_gpui::{GpuiPainter, PainterCaches};
use gpui::{
    canvas, div, prelude::*, ClipboardItem, Context, FocusHandle, KeyDownEvent,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollDelta,
    ScrollWheelEvent, TouchPhase, Window,
};
use himark::bridge::ViewBridgeId;
use himark::host::ScrollPhase;
use imba::event::Event;
use imba::ime::ImeClient;
use imba_gpui::events::{cursor_style_of, device_point, key_of, modifiers_of};

use crate::engine::Engine;

pub struct BridgeView {
    engine: Engine,
    bridge: ViewBridgeId,
    caches: Rc<RefCell<PainterCaches>>,
    focus: FocusHandle,
    started_at: Instant,
    modifiers: gpui::Modifiers,
    /// Where the pointer last was, in device pixels — what a
    /// modifier change re-hit-tests at (gpui's `ModifiersChangedEvent`
    /// carries no position).
    pointer: Cell<gfx::Point>,
    /// The shape the last hit test answered. Only RECORDED here:
    /// `set_window_cursor_style` is a paint-phase API (it panics
    /// anywhere else), so the canvas paint applies it each frame.
    cursor: Rc<Cell<gpui::CursorStyle>>,
    /// Where the canvas painted last — what IME geometry converts
    /// through (set each frame by the paint closure).
    origin: Rc<Cell<gpui::Point<gpui::Pixels>>>,
}

impl BridgeView {
    pub fn new(engine: Engine, bridge: ViewBridgeId, cx: &mut Context<Self>) -> Self {
        engine.attach(cx.weak_entity());
        Self {
            engine,
            bridge,
            caches: Rc::new(RefCell::new(PainterCaches::default())),
            focus: cx.focus_handle(),
            started_at: Instant::now(),
            modifiers: gpui::Modifiers::default(),
            pointer: Cell::new(gfx::Point::new(-1.0e6, -1.0e6)),
            cursor: Rc::new(Cell::new(gpui::CursorStyle::Arrow)),
            origin: Rc::new(Cell::new(gpui::Point::default())),
        }
    }

    fn now(&self) -> f64 {
        self.started_at.elapsed().as_secs_f64()
    }

    /// One event into this bridge, through Himark's own dispatch —
    /// no key codes, no masks, the slot's standing size.
    fn dispatch(&self, event: Event<'_>, cx: &mut Context<Self>) {
        let at = self.now();
        let outcome = self
            .engine
            .raw()
            .borrow_mut()
            .dispatch_bridge(self.bridge, event, None, at);
        if outcome.needs_redraw {
            cx.notify();
        }
    }

    fn dispatch_pointed(&self, event: Event<'_>, cx: &mut Context<Self>) {
        let outcome = self
            .engine
            .raw()
            .borrow_mut()
            .dispatch_bridge(self.bridge, event, None, 0.0);
        let style = cursor_style_of(outcome.pointer);
        if self.cursor.replace(style) != style || outcome.needs_redraw {
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

        // A standing composition owns the keyboard — the input
        // method sees every stroke first, the same gate every
        // shell keeps.
        if self
            .with_ime(|client| client.has_marked_text())
            .unwrap_or(false)
        {
            return;
        }

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

        // Printable text NEVER rides key-down: the registered
        // input handler is gpui's `insertText:` and delivers it
        // through `replace_text_in_range` — dispatching the
        // insertable here too typed every character twice.
        let (key, _insertable) = key_of(keystroke);
        let mods = modifiers_of(self.modifiers);
        if let Some(key) = key {
            let at = self.now();
            let changed = {
                let mut app = self.engine.raw().borrow_mut();
                // A shell that types without ticking still applies
                // the standing scroll before the stroke lands.
                let flushed = app.flush_scroll(self.bridge);
                app.dispatch_bridge(self.bridge, Event::KeyDown { key, mods }, None, at)
                    .needs_redraw
                    || flushed
            };
            if changed {
                cx.notify();
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
            .with_clipboard_client(self.bridge, act)
    }

    fn with_ime<R>(&self, act: impl FnOnce(&mut dyn ImeClient) -> R) -> Option<R> {
        self.engine
            .raw()
            .borrow_mut()
            .with_ime_client(self.bridge, act)
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
        let origin = self.origin.clone();
        let focus = self.focus.clone();
        let entity = cx.entity();
        let started_at = self.started_at;
        let bridge = self.bridge;

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
                    // under the new chord — a re-hit-test at the
                    // pointer's standing position (the cmd-hover
                    // link underline follows without motion).
                    this.dispatch_pointed(
                        Event::MouseMove {
                            point: this.pointer.get(),
                            mods: modifiers_of(event.modifiers),
                        },
                        cx,
                    );
                },
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    let point = device_point(event.position, window.scale_factor());
                    this.pointer.set(point);
                    this.dispatch(
                        Event::MouseDown {
                            point,
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
                    let point = device_point(event.position, window.scale_factor());
                    this.pointer.set(point);
                    this.dispatch(
                        Event::MouseDown {
                            point,
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
                    let point = device_point(event.position, window.scale_factor());
                    this.pointer.set(point);
                    this.dispatch(Event::MouseUp { point }, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                let point = device_point(event.position, window.scale_factor());
                this.pointer.set(point);
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
                this.pointer.set(point);
                // The ENGINE's scroll road, not a direct dispatch:
                // its coalescer folds a trackpad's event rate into
                // one batch per tick — machinery, not marshaling.
                // NEGATED like every shell feeds it: the engine
                // speaks scroll offsets, AppKit speaks content
                // motion.
                const LINE_SCROLL_PX: f32 = 48.0;
                let (delta_x, delta_y, phase) = match event.delta {
                    ScrollDelta::Pixels(delta) => (
                        -f32::from(delta.x) * scale,
                        -f32::from(delta.y) * scale,
                        match event.touch_phase {
                            TouchPhase::Started => ScrollPhase::Began,
                            TouchPhase::Moved => ScrollPhase::Changed,
                            TouchPhase::Ended => ScrollPhase::Ended,
                        },
                    ),
                    ScrollDelta::Lines(lines) => (
                        -lines.x * LINE_SCROLL_PX * scale,
                        -lines.y * LINE_SCROLL_PX * scale,
                        ScrollPhase::None,
                    ),
                };
                if delta_x != 0.0 || delta_y != 0.0 || phase == ScrollPhase::Began {
                    let at = this.now();
                    let changed = this.engine.raw().borrow_mut().scroll_phased_at_time(
                        this.bridge,
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
                        origin.set(bounds.origin);
                        // The input method's door — NSTextInputClient
                        // in the Swift shell, this handler here.
                        window.handle_input(
                            &focus,
                            gpui::ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                        let scale = window.scale_factor();
                        let now_ms = started_at.elapsed().as_secs_f64() * 1000.0;
                        let animating = engine.tick(now_ms);

                        let width = (f32::from(bounds.size.width) * scale).max(1.0);
                        let height = (f32::from(bounds.size.height) * scale).max(1.0);
                        let painter =
                            GpuiPainter::new(window, cx, bounds.origin, scale, caches.clone());
                        let reconciling = {
                            let mut app = engine.raw().borrow_mut();
                            // A shell that draws without ticking
                            // still applies the standing coalesced
                            // scroll before the frame.
                            let flushed = app.flush_scroll(bridge);
                            app.dispatch_bridge(
                                bridge,
                                Event::Paint {
                                    canvas: &painter,
                                    focused: true,
                                },
                                Some(gfx::Size::new(width, height)),
                                0.0,
                            )
                            .needs_redraw
                                || flushed
                        };
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

/// The input method's protocol — `NSTextInputClient` in the Swift
/// shell, mapped method for method onto the same engine IME road.
/// Ranges are UTF-16, like the platform speaks them.
impl gpui::EntityInputHandler for BridgeView {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let (start, length) = utf16_of(&range);
        let text = self.with_ime(|client| client.substring_utf16(start, length))??;
        *adjusted_range = Some(range);
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        let (start, end) = self.with_ime(|client| client.selected_range())??;
        Some(gpui::UTF16Selection {
            range: start as usize..end as usize,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let (start, end) = self.with_ime(|client| client.marked_range())??;
        Some(start as usize..end as usize)
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.with_ime(|client| client.unmark_text()).is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed = match range {
            Some(range) => {
                let replacement = utf16_of(&range);
                self.with_ime(|client| client.insert_text(text, Some(replacement)))
                    .is_some()
            }
            None if !text.is_empty() => {
                let at = self.now();
                let mut app = self.engine.raw().borrow_mut();
                let flushed = app.flush_scroll(self.bridge);
                app.dispatch_bridge(self.bridge, Event::TextInput { text }, None, at)
                    .needs_redraw
                    || flushed
            }
            None => false,
        };
        if changed {
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        new_text: &str,
        new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = new_selected_range
            .map(|range| utf16_of(&range))
            .unwrap_or((new_text.encode_utf16().count() as u32, 0));
        let replacement = range.map(|range| utf16_of(&range));
        let changed = self
            .with_ime(|client| client.set_marked_text(new_text, selected, replacement))
            .is_some();
        if changed {
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        element_bounds: gpui::Bounds<gpui::Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        let (start, length) = utf16_of(&range_utf16);
        let (x, y, width, height) = self.with_ime(|client| client.first_rect(start, length))??;
        let scale = window.scale_factor();
        Some(gpui::Bounds {
            origin: gpui::point(
                element_bounds.origin.x + gpui::px(x / scale),
                element_bounds.origin.y + gpui::px(y / scale),
            ),
            size: gpui::size(gpui::px(width / scale), gpui::px(height / scale)),
        })
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let scale = window.scale_factor();
        let origin = self.origin.get();
        let index = self.with_ime(|client| {
            client.char_index_at(
                f32::from(point.x - origin.x) * scale,
                f32::from(point.y - origin.y) * scale,
            )
        })??;
        Some(index as usize)
    }
}

fn utf16_of(range: &std::ops::Range<usize>) -> (u32, u32) {
    (range.start as u32, range.len() as u32)
}
