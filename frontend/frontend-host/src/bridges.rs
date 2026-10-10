// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The NORMALIZED C surface (docs/ui/imba-ffi.md,
//! docs/himark-core.md, plan step 3): handles, never pointers —
//! every object below is a dense id with a dedicated release, and
//! a stale, foreign or double-released handle is a checked
//! refusal, not UB. An event is minted ONCE and may be dispatched
//! to any number of bridges before its release.
//!
//! Two roads, one pipeline:
//! - **The window-bridge road** (`himark_bridge_event`/`_draw`)
//!   routes through the engine's own dispatch — keyboard focus
//!   walk, scroll gesture stream, settle and theme tails included
//!   — so a shell converting entry-by-entry changes nothing
//!   behaviorally. The window is the degenerate bridge: it fills
//!   its viewport, and the engine composes it.
//! - **The stage road** (`himark_bridge_layout` and down) mirrors
//!   imba's pipeline as handles — layout (sized; `size` is the
//!   measure read), realize-at-viewport (consuming), handle-event
//!   — for hosts that own scrolling and compose the stages
//!   themselves. Scroll events do not enter this road: the host's
//!   scroller owns the physics and moves the viewport instead.
//!
//! Coexists with the legacy whole-window entries until the mac
//! port retires them (docs/himark-core.md, plan step 5).

use std::collections::HashMap;
use std::os::raw::{c_char, c_void};
use std::sync::{LazyLock, Mutex};

use himark::bridge::{BridgeLayout, Frame, ViewBridgeId};
use imba::event::{Event, EventResult, MouseButton};
use skia_safe::{Canvas, Point, Rect, Size};

use crate::{map_key, map_mods, pointed, str_from, HimarkEngine, HimarkPointed};

/// A dense handle table: ids start at 1 and never recycle, so a
/// released handle stays dead forever instead of aliasing a
/// newborn.
pub(crate) struct Handles<T> {
    next: u64,
    rows: HashMap<u64, T>,
}

impl<T> Default for Handles<T> {
    fn default() -> Self {
        Self {
            next: 1,
            rows: HashMap::new(),
        }
    }
}

impl<T> Handles<T> {
    fn mint(&mut self, row: T) -> u64 {
        let handle = self.next;
        self.next += 1;
        self.rows.insert(handle, row);
        handle
    }

    fn get(&self, handle: u64) -> Option<&T> {
        self.rows.get(&handle)
    }

    fn take(&mut self, handle: u64) -> Option<T> {
        self.rows.remove(&handle)
    }
}

fn refuse(what: &str, handle: u64) {
    eprintln!("[ffi] {what} handle {handle} is dead or foreign — refused");
}

/// An owned mirror of `imba::Event`, minted by the factories
/// below. The mirror carries the RAW shell ints (key codes,
/// modifier masks) — the one `map_key`/`map_mods` translation runs
/// at dispatch, same as the legacy entries.
#[derive(Clone)]
enum OwnedEvent {
    MouseDown { x: f32, y: f32, mods: u32, count: u32 },
    SecondaryDown { x: f32, y: f32, mods: u32 },
    MouseDrag { x: f32, y: f32, mods: u32 },
    MouseUp { x: f32, y: f32 },
    MouseMove { x: f32, y: f32, mods: u32 },
    MouseLeft,
    ModifiersChanged { x: f32, y: f32, mods: u32 },
    Scroll { x: f32, y: f32, delta_x: f32, delta_y: f32, phase: u32 },
    KeyDown { key: u32, mods: u32 },
    TextInput { text: String },
}

/// The event table is process-wide: events are engine-independent
/// values, minted on the main thread and dispatched there (the
/// whole surface's threading law); the mutex only keeps a host's
/// mistake from becoming UB.
static EVENTS: LazyLock<Mutex<Handles<OwnedEvent>>> = LazyLock::new(Default::default);

fn mint_event(event: OwnedEvent) -> u64 {
    EVENTS.lock().expect("the event table").mint(event)
}

/// The per-engine stage tables: each row remembers the bridge it
/// was derived FROM, so a released frame's arena goes home warm.
#[derive(Default)]
pub(crate) struct Stages {
    layouts: Handles<(ViewBridgeId, BridgeLayout)>,
    frames: Handles<(ViewBridgeId, Frame)>,
}

// ─── event factories ───

#[no_mangle]
pub extern "C" fn himark_event_mouse_down(x: f32, y: f32, mods: u32, count: u32) -> u64 {
    mint_event(OwnedEvent::MouseDown { x, y, mods, count })
}

#[no_mangle]
pub extern "C" fn himark_event_secondary_down(x: f32, y: f32, mods: u32) -> u64 {
    mint_event(OwnedEvent::SecondaryDown { x, y, mods })
}

#[no_mangle]
pub extern "C" fn himark_event_mouse_drag(x: f32, y: f32, mods: u32) -> u64 {
    mint_event(OwnedEvent::MouseDrag { x, y, mods })
}

#[no_mangle]
pub extern "C" fn himark_event_mouse_up(x: f32, y: f32) -> u64 {
    mint_event(OwnedEvent::MouseUp { x, y })
}

#[no_mangle]
pub extern "C" fn himark_event_mouse_move(x: f32, y: f32, mods: u32) -> u64 {
    mint_event(OwnedEvent::MouseMove { x, y, mods })
}

/// The cursor-exit miss (`Event::window_left`): hover lets go.
#[no_mangle]
pub extern "C" fn himark_event_mouse_left() -> u64 {
    mint_event(OwnedEvent::MouseLeft)
}

#[no_mangle]
pub extern "C" fn himark_event_modifiers_changed(x: f32, y: f32, mods: u32) -> u64 {
    mint_event(OwnedEvent::ModifiersChanged { x, y, mods })
}

#[no_mangle]
pub extern "C" fn himark_event_scroll(
    x: f32,
    y: f32,
    delta_x: f32,
    delta_y: f32,
    phase: u32,
) -> u64 {
    mint_event(OwnedEvent::Scroll {
        x,
        y,
        delta_x,
        delta_y,
        phase,
    })
}

#[no_mangle]
pub extern "C" fn himark_event_key_down(key: u32, mods: u32) -> u64 {
    mint_event(OwnedEvent::KeyDown { key, mods })
}

#[no_mangle]
pub unsafe extern "C" fn himark_event_text(utf8: *const c_char, len: usize) -> u64 {
    let Some(text) = str_from(utf8, len) else {
        refuse("text payload for event", 0);
        return 0;
    };
    mint_event(OwnedEvent::TextInput {
        text: text.to_owned(),
    })
}

#[no_mangle]
pub extern "C" fn himark_event_release(event: u64) {
    if EVENTS
        .lock()
        .expect("the event table")
        .take(event)
        .is_none()
    {
        refuse("event", event);
    }
}

// ─── the window-bridge road ───

/// The window AS a bridge — the degenerate case named. Today the
/// identity is direct; the handle is the only currency the
/// normalized entries below speak.
#[no_mangle]
pub unsafe extern "C" fn himark_window_bridge(engine: *mut HimarkEngine, window: u64) -> u64 {
    if engine.is_null() {
        refuse("engine for window bridge", window);
        return 0;
    }
    window
}

/// One event into one window bridge, through the engine's own
/// dispatch. The event stays alive for further bridges; release
/// is the host's explicit call.
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_event(
    engine: *mut HimarkEngine,
    bridge: u64,
    event: u64,
) -> HimarkPointed {
    let missed = HimarkPointed {
        changed: false,
        shape: 0,
    };
    let Some(engine) = engine.as_mut() else {
        return missed;
    };
    let owned = match EVENTS.lock().expect("the event table").get(event) {
        Some(owned) => owned.clone(),
        None => {
            refuse("event", event);
            return missed;
        }
    };
    let changed = |changed: bool| HimarkPointed { changed, shape: 0 };
    match owned {
        OwnedEvent::MouseDown { x, y, mods, count } => {
            changed(engine.mouse_down(bridge, x, y, mods, count))
        }
        OwnedEvent::SecondaryDown { x, y, mods } => {
            changed(engine.secondary_down(bridge, x, y, mods))
        }
        OwnedEvent::MouseDrag { x, y, mods } => changed(engine.mouse_drag(bridge, x, y, mods)),
        OwnedEvent::MouseUp { x, y } => changed(engine.mouse_up(bridge, x, y)),
        OwnedEvent::MouseMove { x, y, mods } => {
            pointed(Some(engine.mouse_move(bridge, x, y, mods)))
        }
        OwnedEvent::MouseLeft => changed(engine.mouse_left(bridge)),
        OwnedEvent::ModifiersChanged { x, y, mods } => {
            pointed(Some(engine.modifiers_changed(bridge, x, y, mods)))
        }
        OwnedEvent::Scroll {
            x,
            y,
            delta_x,
            delta_y,
            phase,
        } => changed(engine.scroll_phased_at_time(bridge, x, y, delta_x, delta_y, phase, 0.0)),
        OwnedEvent::KeyDown { key, mods } => changed(engine.key_down(bridge, key, mods)),
        OwnedEvent::TextInput { text } => changed(engine.text_input(bridge, &text)),
    }
}

/// Paint the window bridge into the host's canvas — the canvas is
/// borrowed payload for this one call, the only pointer shape the
/// normalized surface allows.
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_draw(
    engine: *mut HimarkEngine,
    bridge: u64,
    canvas: *mut c_void,
    width: f32,
    height: f32,
    scale: f32,
) -> bool {
    if engine.is_null() || canvas.is_null() {
        return false;
    }
    let canvas = &*(canvas as *const Canvas);
    engine
        .as_mut()
        .map_or(false, |engine| engine.draw(bridge, canvas, width, height, scale))
}

// ─── the stage road ───

/// display → layout: the SIZED stage, as a handle. 0 refuses an
/// unmounted bridge.
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_layout(
    engine: *mut HimarkEngine,
    bridge: u64,
    width: f32,
    height: f32,
) -> u64 {
    let Some(engine) = engine.as_mut() else {
        return 0;
    };
    let id = ViewBridgeId::from_raw(bridge);
    match engine.app.layout_bridge(id, Size::new(width, height)) {
        Some(layout) => engine.stages.layouts.mint((id, layout)),
        None => {
            refuse("bridge", bridge);
            0
        }
    }
}

/// The measure read on the sized stage — the native scroller's
/// content-size question.
#[no_mangle]
pub unsafe extern "C" fn himark_layout_size(
    engine: *mut HimarkEngine,
    layout: u64,
    out_width: *mut f32,
    out_height: *mut f32,
) -> bool {
    let Some(engine) = engine.as_ref() else {
        return false;
    };
    let Some((_, stage)) = engine.stages.layouts.get(layout) else {
        refuse("layout", layout);
        return false;
    };
    let size = stage.size();
    if !out_width.is_null() {
        *out_width = size.width;
    }
    if !out_height.is_null() {
        *out_height = size.height;
    }
    true
}

/// layout → realize: materialize the viewport's slice. CONSUMES
/// the layout handle — a new viewport is a new realize off a
/// fresh layout, exactly as inside the engine.
#[no_mangle]
pub unsafe extern "C" fn himark_layout_realize(
    engine: *mut HimarkEngine,
    layout: u64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> u64 {
    let Some(engine) = engine.as_mut() else {
        return 0;
    };
    let Some((id, stage)) = engine.stages.layouts.take(layout) else {
        refuse("layout", layout);
        return 0;
    };
    let frame = stage.realize(Rect::from_xywh(x, y, width, height));
    engine.stages.frames.mint((id, frame))
}

#[no_mangle]
pub unsafe extern "C" fn himark_layout_release(engine: *mut HimarkEngine, layout: u64) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    if engine.stages.layouts.take(layout).is_none() {
        refuse("layout", layout);
    }
}

/// One event into a realized stage frame, viewport in content
/// coordinates handed per call. The commands the view answers
/// with run through the engine's one batch loop.
#[no_mangle]
pub unsafe extern "C" fn himark_frame_event(
    engine: *mut HimarkEngine,
    frame: u64,
    event: u64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let owned = match EVENTS.lock().expect("the event table").get(event) {
        Some(owned) => owned.clone(),
        None => {
            refuse("event", event);
            return false;
        }
    };
    let viewport = Rect::from_xywh(x, y, width, height);
    let result = {
        let Some((_, stage)) = engine.stages.frames.get(frame) else {
            refuse("frame", frame);
            return false;
        };
        let event = match &owned {
            OwnedEvent::MouseDown { x, y, mods, count } => Event::MouseDown {
                point: Point::new(*x, *y),
                button: MouseButton::Left,
                mods: map_mods(*mods),
                count: (*count).min(3) as u8,
            },
            OwnedEvent::SecondaryDown { x, y, mods } => Event::MouseDown {
                point: Point::new(*x, *y),
                button: MouseButton::Right,
                mods: map_mods(*mods),
                count: 1,
            },
            OwnedEvent::MouseDrag { x, y, mods } => Event::MouseDrag {
                point: Point::new(*x, *y),
                mods: map_mods(*mods),
            },
            OwnedEvent::MouseUp { x, y } => Event::MouseUp {
                point: Point::new(*x, *y),
            },
            OwnedEvent::MouseMove { x, y, mods } => Event::MouseMove {
                point: Point::new(*x, *y),
                mods: map_mods(*mods),
            },
            OwnedEvent::MouseLeft | OwnedEvent::ModifiersChanged { .. } => Event::window_left(),
            OwnedEvent::Scroll { .. } => {
                // The stage road has no scroller: the HOST owns the
                // physics and moves the viewport it hands this very
                // call (docs/ui/imba-ffi.md, touch section).
                eprintln!("[ffi] a scroll reached the stage road — the host's scroller owns it");
                return false;
            }
            OwnedEvent::KeyDown { key, mods } => match map_key(*key) {
                Some(key) => Event::KeyDown {
                    key,
                    mods: map_mods(*mods),
                },
                None => return false,
            },
            OwnedEvent::TextInput { text } => Event::TextInput { text },
        };
        stage.handle(&event, viewport)
    };
    match result {
        EventResult::Ignored => false,
        EventResult::Handled => true,
        EventResult::Command(command) => engine.app.perform_batch(vec![command]),
        EventResult::Commands(commands) => engine.app.perform_batch(commands),
        EventResult::Pointer { commands, .. } => match commands.is_empty() {
            true => true,
            false => engine.app.perform_batch(commands),
        },
        // Reveal surfaces to the host with the intents layer
        // (docs/ui/himark-ffi.md) — not yet on this road.
        EventResult::Reveal(_) => false,
    }
}

/// Paint the realized slice into the host's canvas.
#[no_mangle]
pub unsafe extern "C" fn himark_frame_paint(
    engine: *mut HimarkEngine,
    frame: u64,
    canvas: *mut c_void,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> bool {
    if canvas.is_null() {
        return false;
    }
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let canvas = &*(canvas as *const Canvas);
    let viewport = Rect::from_xywh(x, y, width, height);
    let result = {
        let Some((_, stage)) = engine.stages.frames.get(frame) else {
            refuse("frame", frame);
            return false;
        };
        stage.handle(
            &Event::Paint {
                canvas,
                focused: true,
            },
            viewport,
        )
    };
    match result {
        EventResult::Ignored | EventResult::Handled | EventResult::Reveal(_) => false,
        EventResult::Command(command) => engine.app.perform_batch(vec![command]),
        EventResult::Commands(commands) | EventResult::Pointer { commands, .. } => {
            engine.app.perform_batch(commands)
        }
    }
}

/// Release the frame; its arena goes home warm to the bridge it
/// was derived from.
#[no_mangle]
pub unsafe extern "C" fn himark_frame_release(engine: *mut HimarkEngine, frame: u64) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    let Some((id, stage)) = engine.stages.frames.take(frame) else {
        refuse("frame", frame);
        return;
    };
    engine.app.return_arena(id, stage.recycle());
}
