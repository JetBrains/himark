// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! One bridge as one rune element — an `El::effect` slot plus the
//! compositor that paints the bridge's recording at it. Input
//! lands as real `imba::event::Event`s through the engine's bridge
//! dispatch; glyphs rasterize into rune's own atlas.

use std::cell::Cell;
use std::rc::Rc;

use himark::bridge::ViewBridgeId;
use himark::host::ScrollPhase;
use imba::event::Event;
use rune::render::{KIND_EFFECT, NO_CLIP};
use rune::GesturePhase;

use crate::engine::Engine;
use crate::painter::Recorder;

/// The himark view in a rune app: `el()` goes into the view tree
/// (size it like any box), `compositor()` attaches to the App.
/// One per mounted bridge.
pub struct BridgeView {
    engine: Engine,
    bridge: ViewBridgeId,
    mark: u32,
    /// Device pixels per layout point at the slot, as the last
    /// frame painted it — what event positions convert through.
    scale: Rc<Cell<f32>>,
}

impl BridgeView {
    pub fn new(engine: Engine, bridge: ViewBridgeId, mark: u32) -> Self {
        Self {
            engine,
            bridge,
            mark,
            scale: Rc::new(Cell::new(2.0)),
        }
    }

    /// The element — an effect slot wired with the full input
    /// translation. Call per view build (`El` is a build-time
    /// value); size it like any box.
    pub fn el<S: 'static>(&self) -> rune::El {
        let k = |cell: &Rc<Cell<f32>>| Rc::clone(cell);

        let engine = self.engine.clone();
        let bridge = self.bridge;
        let scale = k(&self.scale);
        let drag = rune::el::effect(self.mark)
            .focusable(true)
            .on_drag_any(move |d, _tx| {
                let s = scale.get();
                let point = gfx::Point::new(d.x * s, d.y * s);
                let at = engine.now();
                let mut app = engine.raw().borrow_mut();
                match d.phase {
                    rune::el::Phase::Down => {
                        app.flush_scroll(bridge);
                        app.dispatch_bridge(
                            bridge,
                            Event::MouseDown {
                                point,
                                button: imba::event::MouseButton::Left,
                                mods: imba::event::Modifiers::default(),
                                count: d.clicks.clamp(1, 3) as u8,
                            },
                            None,
                            at,
                        );
                    }
                    rune::el::Phase::Move => {
                        app.dispatch_bridge(
                            bridge,
                            Event::MouseDrag {
                                point,
                                mods: imba::event::Modifiers::default(),
                            },
                            None,
                            at,
                        );
                    }
                    rune::el::Phase::Up => {
                        app.dispatch_bridge(bridge, Event::MouseUp { point }, None, at);
                    }
                }
            });

        let engine = self.engine.clone();
        let scale = k(&self.scale);
        let moved = drag.on_pointer_move_local::<S>(move |_s, p, _tx| {
            let s = scale.get();
            let mut app = engine.raw().borrow_mut();
            app.dispatch_bridge(
                bridge,
                Event::MouseMove {
                    point: gfx::Point::new(p.x * s, p.y * s),
                    mods: imba::event::Modifiers::default(),
                },
                None,
                0.0,
            );
        });

        let engine = self.engine.clone();
        let scale = k(&self.scale);
        let secondary = moved.on_secondary_press_local::<S>(move |_s, p, _tx| {
            let s = scale.get();
            let point = gfx::Point::new(p.x * s, p.y * s);
            let at = engine.now();
            let mut app = engine.raw().borrow_mut();
            app.dispatch_bridge(
                bridge,
                Event::MouseDown {
                    point,
                    button: imba::event::MouseButton::Right,
                    mods: imba::event::Modifiers::default(),
                    count: 1,
                },
                None,
                at,
            );
            app.dispatch_bridge(bridge, Event::MouseUp { point }, None, at);
        });

        let engine = self.engine.clone();
        let scale = k(&self.scale);
        let wheeled = secondary.on_wheel::<S>(move |_s, w, _tx| {
            let s = scale.get();
            // NEGATED like every shell feeds the coalescer: the
            // engine speaks scroll offsets, the platform content
            // motion.
            // A plain mouse wheel (imprecise) carries no gesture:
            // the 250 ms pause is its boundary, like every shell.
            let phase = if !w.precise {
                ScrollPhase::None
            } else {
                match w.phase {
                    GesturePhase::Began => ScrollPhase::Began,
                    GesturePhase::Moved => ScrollPhase::Changed,
                    GesturePhase::Ended => ScrollPhase::Ended,
                }
            };
            let at = engine.now();
            engine.raw().borrow_mut().scroll_phased_at_time(
                bridge,
                w.x * s,
                w.y * s,
                -w.dx * s,
                -w.dy * s,
                phase,
                at,
            );
            true
        });

        let engine = self.engine.clone();
        wheeled.on_key::<S>(move |_s, key, _tx| {
            let at = engine.now();
            let mods = imba::event::Modifiers {
                shift: key.mods.shift,
                control: key.mods.ctrl,
                alt: key.mods.alt,
                command: key.mods.cmd,
            };
            let mapped = match &key.key {
                rune::Key::Text(text) => {
                    let mut app = engine.raw().borrow_mut();
                    app.flush_scroll(bridge);
                    app.dispatch_bridge(bridge, Event::TextInput { text }, None, at);
                    return true;
                }
                rune::Key::Command(c) => Some(imba::event::Key::Char(*c)),
                rune::Key::Enter => Some(imba::event::Key::Enter),
                rune::Key::Backspace => Some(imba::event::Key::Backspace),
                rune::Key::Delete => Some(imba::event::Key::Delete),
                rune::Key::Left => Some(imba::event::Key::Left),
                rune::Key::Right => Some(imba::event::Key::Right),
                rune::Key::Up => Some(imba::event::Key::Up),
                rune::Key::Down => Some(imba::event::Key::Down),
                rune::Key::Home => Some(imba::event::Key::Home),
                rune::Key::End => Some(imba::event::Key::End),
                rune::Key::PageUp => Some(imba::event::Key::PageUp),
                rune::Key::PageDown => Some(imba::event::Key::PageDown),
                rune::Key::Escape => Some(imba::event::Key::Escape),
                rune::Key::Tab => Some(imba::event::Key::Tab),
                _ => None,
            };
            let Some(key) = mapped else {
                return false;
            };
            let mut app = engine.raw().borrow_mut();
            app.flush_scroll(bridge);
            app.dispatch_bridge(bridge, Event::KeyDown { key, mods }, None, at)
                .needs_redraw
        })
    }

    /// The compositor half — attach with `App::compositor`.
    pub fn compositor(&self) -> BridgeCompositor {
        BridgeCompositor {
            engine: self.engine.clone(),
            bridge: self.bridge,
            mark: self.mark,
            scale: Rc::clone(&self.scale),
            animating: Cell::new(false),
        }
    }
}

/// Paints the frame: himark's recording spliced into the painter
/// stream at the slot, then rune's default draw.
pub struct BridgeCompositor {
    engine: Engine,
    bridge: ViewBridgeId,
    mark: u32,
    scale: Rc<Cell<f32>>,
    animating: Cell<bool>,
}

impl<S: 'static> rune::Compositor<S> for BridgeCompositor {
    fn render(&mut self, f: &mut rune::Frame<'_, S>) {
        // The engine's frame half: owed landings, the clock, the
        // standing coalesced scroll.
        let now_ms = self.engine.now() * 1000.0;
        let animating = {
            let mut app = self.engine.raw().borrow_mut();
            let _ = app.drain();
            drop(app);
            self.engine.tick(now_ms)
        };
        self.animating.set(animating);

        // The slot, by mark, in this frame's painter stream.
        let slot = f
            .rt
            .instances
            .iter()
            .position(|i| {
                i.params[1] == KIND_EFFECT
                    && ((i.border[0] as u32 & 0xFFFF) | ((i.border[1] as u32 & 0xFFFF) << 16))
                        == self.mark
            });
        if let Some(index) = slot {
            let instance = f.rt.instances[index];
            let rect = instance.rect;
            let clip = if instance.clip == [0.0; 4] { NO_CLIP } else { instance.clip };
            self.scale.set(instance.params[3].max(0.01));

            let recorded = {
                let mut app = self.engine.raw().borrow_mut();
                app.flush_scroll(self.bridge);
                let recorder = Recorder::new(
                    &mut f.rt.text,
                    rect,
                    clip,
                    f.ui.srgb_blend(),
                    instance.params[3],
                );
                app.draw_mounted(
                    self.bridge,
                    &recorder,
                    gfx::Size::new(rect[2].max(1.0), rect[3].max(1.0)),
                );
                recorder.finish()
            };

            // Splice right after the slot record, marks kept
            // parallel (`track_marks` is on while a compositor is
            // attached).
            // The one line a headless check can read: the paint
            // road ran end to end.
            static FIRST: std::sync::Once = std::sync::Once::new();
            FIRST.call_once(|| {
                eprintln!(
                    "[himark-rune] first frame recorded {} instances at {}x{}",
                    recorded.len(),
                    rect[2] as u32,
                    rect[3] as u32
                );
            });
            let count = recorded.len();
            f.rt.instances.splice(index + 1..index + 1, recorded);
            if f.rt.track_marks {
                f.rt.marks
                    .splice(index + 1..index + 1, std::iter::repeat(self.mark).take(count));
            }
        }

        f.draw_ui();
    }

    fn animating(&self) -> bool {
        self.animating.get()
    }
}
