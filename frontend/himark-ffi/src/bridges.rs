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

use himark::bridge::ViewBridgeId;
use gfx::{Point, Rect, Size};
// Only the skia-shell feature's paint doors speak the canvas —
// keep the import with them, not the default build.
#[cfg(feature = "skia-shell")]
use gfx::Canvas;
use imba::event::{Event, EventResult, MouseButton};

use crate::ffi_seat;
use crate::{
    copy_optional_utf8, map_key, map_mods, pointed, str_from, HimarkEngine, HimarkPointed, Pointed,
};


#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkBridge {
    pub raw: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkEvent {
    pub raw: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkCatalog {
    pub raw: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkPlace {
    pub raw: u64,
}

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

pub fn refuse(what: &str, handle: u64) {
    eprintln!("[ffi] {what} handle {handle} is dead or foreign — refused");
}

/// An owned mirror of `imba::Event`, minted by the factories
/// below. The mirror carries the RAW shell ints (key codes,
/// modifier masks) — the one `map_key`/`map_mods` translation runs
/// at dispatch, same as the legacy entries.
#[derive(Clone)]
enum OwnedEvent {
    MouseDown {
        x: f32,
        y: f32,
        mods: u32,
        count: u32,
    },
    SecondaryDown {
        x: f32,
        y: f32,
        mods: u32,
    },
    MouseDrag {
        x: f32,
        y: f32,
        mods: u32,
    },
    MouseUp {
        x: f32,
        y: f32,
    },
    MouseMove {
        x: f32,
        y: f32,
        mods: u32,
    },
    MouseLeft,
    ModifiersChanged {
        x: f32,
        y: f32,
        mods: u32,
    },
    Scroll {
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        phase: u32,
    },
    KeyDown {
        key: u32,
        mods: u32,
    },
    TextInput {
        text: String,
    },
}

/// The event table is process-wide: events are engine-independent
/// values, minted on the main thread and dispatched there (the
/// whole surface's threading law); the mutex only keeps a host's
/// mistake from becoming UB.
static EVENTS: LazyLock<Mutex<Handles<OwnedEvent>>> = LazyLock::new(Default::default);

fn mint_event(event: OwnedEvent) -> HimarkEvent {
    HimarkEvent {
        raw: EVENTS.lock().expect("the event table").mint(event),
    }
}

/// The place table is process-wide like the event table: places
/// are engine-independent values, and the SEATED shell (called
/// deep inside a batch, no engine struct in reach) mints the
/// handles it hands the native callback.
static PLACES: LazyLock<Mutex<Handles<PlaceEntry>>> = LazyLock::new(Default::default);

/// One held place: the location, and an opaque PAYLOAD the layer
/// above this crate attached (the AHP capi rides the place's
/// session here — this crate never reads it, only carries it).
pub type PlacePayload = std::sync::Arc<dyn std::any::Any + Send + Sync>;
pub type PlaceEntry = (hikit::navigation::NavigationLocation, Option<PlacePayload>);

/// Mint a place handle — the door the layers above mint through.
pub fn mint_place_handle(
    place: hikit::navigation::NavigationLocation,
    payload: Option<PlacePayload>,
) -> HimarkPlace {
    HimarkPlace {
        raw: PLACES.lock().expect("the place table").mint((place, payload)),
    }
}

/// Read a held place back (the payload included).
pub fn place_entry(place: HimarkPlace) -> Option<PlaceEntry> {
    PLACES.lock().expect("the place table").get(place.raw).cloned()
}

/// The seated MINT RIDER: runs after `himark_bridge_mint` mounts a
/// place that carried a payload (the AHP capi remembers the
/// session on the new bridge).
#[derive(Clone)]
pub struct PlaceMintRider(
    pub  std::sync::Arc<
        dyn Fn(&mut himark::app::Himark, himark::bridge::ViewBridgeId, &PlacePayload)
            + Send
            + Sync,
    >,
);

/// What a seated exporter answers when the native shell should
/// take a place over (`seat_callback_shell`'s classifier).
pub struct NavigateExport {
    pub kind: u32,
    pub title: String,
    pub focus: bool,
    pub payload: Option<PlacePayload>,
}

/// The seated EXPORT classifier: which places the native callback
/// is offered, with what kind, title and payload. The AHP capi
/// seats the one that knows the whole vocabulary.
#[derive(Clone)]
pub struct NavigateExporter(
    pub  std::sync::Arc<
        dyn Fn(
                &hikit::navigation::NavigationLocation,
                himark::bridge::ViewBridgeId,
                &imba::store::Store,
            ) -> Option<NavigateExport>
            + Send
            + Sync,
    >,
);

// ─── event factories ───

#[no_mangle]
pub extern "C" fn himark_event_mouse_down(x: f32, y: f32, mods: u32, count: u32) -> HimarkEvent {
    mint_event(OwnedEvent::MouseDown { x, y, mods, count })
}

#[no_mangle]
pub extern "C" fn himark_event_secondary_down(x: f32, y: f32, mods: u32) -> HimarkEvent {
    mint_event(OwnedEvent::SecondaryDown { x, y, mods })
}

#[no_mangle]
pub extern "C" fn himark_event_mouse_drag(x: f32, y: f32, mods: u32) -> HimarkEvent {
    mint_event(OwnedEvent::MouseDrag { x, y, mods })
}

#[no_mangle]
pub extern "C" fn himark_event_mouse_up(x: f32, y: f32) -> HimarkEvent {
    mint_event(OwnedEvent::MouseUp { x, y })
}

#[no_mangle]
pub extern "C" fn himark_event_mouse_move(x: f32, y: f32, mods: u32) -> HimarkEvent {
    mint_event(OwnedEvent::MouseMove { x, y, mods })
}

/// The cursor-exit miss (`Event::window_left`): hover lets go.
#[no_mangle]
pub extern "C" fn himark_event_mouse_left() -> HimarkEvent {
    mint_event(OwnedEvent::MouseLeft)
}

#[no_mangle]
pub extern "C" fn himark_event_modifiers_changed(x: f32, y: f32, mods: u32) -> HimarkEvent {
    mint_event(OwnedEvent::ModifiersChanged { x, y, mods })
}

#[no_mangle]
pub extern "C" fn himark_event_scroll(
    x: f32,
    y: f32,
    delta_x: f32,
    delta_y: f32,
    phase: u32,
) -> HimarkEvent {
    mint_event(OwnedEvent::Scroll {
        x,
        y,
        delta_x,
        delta_y,
        phase,
    })
}

#[no_mangle]
pub extern "C" fn himark_event_key_down(key: u32, mods: u32) -> HimarkEvent {
    mint_event(OwnedEvent::KeyDown { key, mods })
}

#[no_mangle]
pub unsafe extern "C" fn himark_event_text(utf8: *const c_char, len: usize) -> HimarkEvent {
    let Some(text) = str_from(utf8, len) else {
        refuse("text payload for event", 0);
        return HimarkEvent { raw: 0 };
    };
    mint_event(OwnedEvent::TextInput {
        text: text.to_owned(),
    })
}

#[no_mangle]
pub extern "C" fn himark_event_release(event: HimarkEvent) {
    if EVENTS
        .lock()
        .expect("the event table")
        .take(event.raw)
        .is_none()
    {
        refuse("event", event.raw);
    }
}

// ─── the window-bridge road ───



/// Perform a registered command at the bridge — the palette and
/// menu road, shell-agnostic.
#[no_mangle]
pub unsafe extern "C" fn himark_perform_command(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    id: *const std::os::raw::c_char,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(id) = std::ffi::CStr::from_ptr(id).to_str().ok() else {
        return false;
    };
    engine.perform_command_at(
        (bridge.raw != 0).then(|| ViewBridgeId::from_raw(bridge.raw)),
        id,
    )
}

/// One event into one window bridge, through the engine's own
/// dispatch. The event stays alive for further bridges; release
/// is the host's explicit call.
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_event(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    event: HimarkEvent,
) -> HimarkPointed {
    let missed = HimarkPointed {
        changed: false,
        shape: 0,
    };
    let Some(engine) = engine.as_mut() else {
        return missed;
    };
    let owned = match EVENTS.lock().expect("the event table").get(event.raw) {
        Some(owned) => owned.clone(),
        None => {
            refuse("event", event.raw);
            return missed;
        }
    };
    let id = himark::bridge::ViewBridgeId::from_raw(bridge.raw);
    let changed = |changed: bool| HimarkPointed { changed, shape: 0 };
    let app = engine;
    // Every input flushes the standing scroll first (claims resolve
    // against scrolled content), then rides the ONE dispatch road.
    match owned {
        OwnedEvent::MouseDown { x, y, mods, count } => {
            let flushed = app.flush_scroll(id);
            let count = match count {
                0 => ffi_seat(app.store()).clicks.lock().expect("the click counter").count(x, y),
                n => n.min(u8::MAX as u32) as u8,
            };
            changed(
                app.dispatch_bridge(
                    id,
                    Event::MouseDown {
                        point: Point::new(x, y),
                        mods: map_mods(mods),
                        button: MouseButton::Left,
                        count,
                    },
                    None,
                    0.0,
                ).needs_redraw || flushed,
            )
        }
        OwnedEvent::SecondaryDown { x, y, mods } => {
            let flushed = app.flush_scroll(id);
            changed(
                app.dispatch_bridge(
                    id,
                    Event::MouseDown {
                        point: Point::new(x, y),
                        mods: map_mods(mods),
                        button: MouseButton::Right,
                        count: 1,
                    },
                    None,
                    0.0,
                ).needs_redraw || flushed,
            )
        }
        OwnedEvent::MouseDrag { x, y, mods } => changed(
            app.dispatch_bridge(
                id,
                Event::MouseDrag {
                    point: Point::new(x, y),
                    mods: map_mods(mods),
                },
                None,
                0.0,
            )
            .needs_redraw,
        ),
        OwnedEvent::MouseUp { x, y } => {
            let flushed = app.flush_scroll(id);
            changed(
                app.dispatch_bridge(
                    id,
                    Event::MouseUp {
                        point: Point::new(x, y),
                    },
                    None,
                    0.0,
                ).needs_redraw || flushed,
            )
        }
        OwnedEvent::MouseMove { x, y, mods } => {
            let outcome = app.dispatch_bridge(
                id,
                Event::MouseMove {
                    point: Point::new(x, y),
                    mods: map_mods(mods),
                },
                None,
                0.0,
            );
            pointed(Some(Pointed {
                changed: outcome.needs_redraw,
                shape: outcome.pointer,
            }))
        }
        OwnedEvent::MouseLeft => changed(app.dispatch_bridge(id, Event::window_left(), None, 0.0).needs_redraw),
        OwnedEvent::ModifiersChanged { x, y, mods } => {
            let outcome = app.dispatch_bridge(
                id,
                Event::MouseMove {
                    point: Point::new(x, y),
                    mods: map_mods(mods),
                },
                None,
                0.0,
            );
            pointed(Some(Pointed {
                changed: outcome.needs_redraw,
                shape: outcome.pointer,
            }))
        }
        OwnedEvent::Scroll {
            x,
            y,
            delta_x,
            delta_y,
            phase,
        } => changed(app.scroll_phased_at_time(
            id,
            x,
            y,
            delta_x,
            delta_y,
            crate::scroll_phase(phase),
            0.0,
        )),
        OwnedEvent::KeyDown { key, mods } => {
            let flushed = app.flush_scroll(id);
            let Some(key) = map_key(key) else {
                return changed(flushed);
            };
            changed(
                app.dispatch_bridge(
                    id,
                    Event::KeyDown {
                        key,
                        mods: map_mods(mods),
                    },
                    None,
                    0.0,
                ).needs_redraw || flushed,
            )
        }
        OwnedEvent::TextInput { text } => {
            let flushed = app.flush_scroll(id);
            changed(
                app.dispatch_bridge(id, Event::TextInput { text: &text }, None, 0.0).needs_redraw || flushed,
            )
        }
    }
}

/// Paint the window bridge into the host's canvas — the canvas is
/// borrowed payload for this one call, the only pointer shape the
/// normalized surface allows.
#[cfg(feature = "skia-shell")]
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_draw(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    canvas: *mut c_void,
    width: f32,
    height: f32,
    _scale: f32,
) -> bool {
    if engine.is_null() || canvas.is_null() {
        return false;
    }
    let canvas: &Canvas<'_> = &gfx_skia::SkiaCanvas::new(&*(canvas as *const skia_safe::Canvas));
    engine.as_mut().map_or(false, |engine| {
        let id = ViewBridgeId::from_raw(bridge.raw);
        // A shell that draws without ticking (tests, direct renders)
        // still applies the standing deltas before the frame.
        let flushed = engine.flush_scroll(id);
        engine
            
            .draw_mounted(id, canvas, gfx::Size::new(width, height))
            || flushed
    })
}

// ─── the stage road ───

/// display → layout: the SIZED stage, as a handle. 0 refuses an
/// unmounted bridge.
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_layout(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    width: f32,
    height: f32,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let id = ViewBridgeId::from_raw(bridge.raw);
    if !engine.stage_layout(id, Size::new(width, height)) {
        refuse("bridge", bridge.raw);
        return false;
    }
    true
}

/// The measure read on the sized stage — the native scroller's
/// content-size question.
#[no_mangle]
pub unsafe extern "C" fn himark_layout_size(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    out_width: *mut f32,
    out_height: *mut f32,
) -> bool {
    let Some(engine) = engine.as_ref() else {
        return false;
    };
    let Some(size) = engine.stage_layout_size(ViewBridgeId::from_raw(bridge.raw)) else {
        refuse("layout (size)", bridge.raw);
        return false;
    };
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
    bridge: HimarkBridge,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    if !engine.stage_realize(
        ViewBridgeId::from_raw(bridge.raw),
        Rect::from_xywh(x, y, width, height),
    ) {
        refuse("layout (realize)", bridge.raw);
        return false;
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn himark_layout_release(engine: *mut HimarkEngine, bridge: HimarkBridge) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    if !engine.stage_layout_release(ViewBridgeId::from_raw(bridge.raw)) {
        refuse("layout (release)", bridge.raw);
    }
}

/// One event into a realized stage frame, viewport in content
/// coordinates handed per call. The commands the view answers
/// with run through the engine's one batch loop.
#[no_mangle]
pub unsafe extern "C" fn himark_frame_event(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    event: HimarkEvent,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let owned = match EVENTS.lock().expect("the event table").get(event.raw) {
        Some(owned) => owned.clone(),
        None => {
            refuse("event", event.raw);
            return false;
        }
    };
    let viewport = Rect::from_xywh(x, y, width, height);
    let result = {
        let Some(stage) = engine.stage_frame(ViewBridgeId::from_raw(bridge.raw)) else {
            refuse("frame", bridge.raw);
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
    let EventResult::Handled(handled) = result else {
        return false;
    };
    if let Some(reveal) = handled.reveal {
        // The stage road has no scroller — park the ask for
        // the host to drain (`himark_take_reveal`) into its own.
        let id = ViewBridgeId::from_raw(bridge.raw);
        if std::env::var_os("HIMARK_TRACE_REVEAL").is_some() {
            eprintln!(
                "[reveal] parked (stage EVENT) bridge {} {reveal:?}",
                id.raw()
            );
        }
        engine.push_reveal(id, reveal);
    }
    if !handled.commands.is_empty() {
        engine.perform_batch(handled.commands);
    }
    true
}

/// A wheel OFFER on the stage road: the overlays that kept their
/// scrolling after the descrollering (a completion popup's list, a
/// horizontal pan in unwrapped code) consume it and `true` comes
/// back; `false` hands the wheel to the host's scroller, which owns
/// the vertical physics. Point and viewport in engine pixels,
/// content coordinates; `phase` as in `himark_event_scroll`.
#[no_mangle]
pub unsafe extern "C" fn himark_frame_offer_scroll(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    x: f32,
    y: f32,
    delta_x: f32,
    delta_y: f32,
    phase: u32,
    view_x: f32,
    view_y: f32,
    view_width: f32,
    view_height: f32,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    engine.offer_stage_scroll(
        ViewBridgeId::from_raw(bridge.raw),
        x,
        y,
        delta_x,
        delta_y,
        crate::scroll_phase(phase),
        Rect::from_xywh(view_x, view_y, view_width, view_height),
    )
}


/// Drain the reveal a mounted road parked — a caret-follow or a
/// selection the view asked to be brought on screen, rect in that
/// bridge's content coordinates. The host scrolls it into view;
/// `out_placement` says HOW (0 = ensure visible, 1 = the rect's
/// origin becomes the viewport's corner — a canvas's row reveal).
/// False when nothing is parked.
#[no_mangle]
pub unsafe extern "C" fn himark_take_reveal(
    engine: *mut HimarkEngine,
    out_bridge: *mut u64,
    out_x: *mut f32,
    out_y: *mut f32,
    out_width: *mut f32,
    out_height: *mut f32,
    out_placement: *mut u32,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some((id, reveal)) = engine.take_reveal() else {
        return false;
    };
    if !out_bridge.is_null() {
        *out_bridge = id.raw();
    }
    if !out_x.is_null() {
        *out_x = reveal.rect.x();
    }
    if !out_y.is_null() {
        *out_y = reveal.rect.y();
    }
    if !out_width.is_null() {
        *out_width = reveal.rect.width();
    }
    if !out_height.is_null() {
        *out_height = reveal.rect.height();
    }
    if !out_placement.is_null() {
        *out_placement = match reveal.placement {
            imba::event::Placement::EnsureVisible => 0,
            imba::event::Placement::TopLeftAt => 1,
        };
    }
    true
}

/// Paint the realized slice into the host's canvas.
#[cfg(feature = "skia-shell")]
#[no_mangle]
pub unsafe extern "C" fn himark_frame_paint(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
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
    let canvas: &Canvas<'_> = &gfx_skia::SkiaCanvas::new(&*(canvas as *const skia_safe::Canvas));
    let viewport = Rect::from_xywh(x, y, width, height);
    let result = {
        let Some(stage) = engine.stage_frame(ViewBridgeId::from_raw(bridge.raw)) else {
            refuse("frame", bridge.raw);
            return false;
        };
        // TRANSPARENT floor: the stage road's content composites
        // over whatever the HOST puts behind it — a glass island, a
        // column color. (The clear still resets the band; only the
        // opaque floor moved host-side.)
        canvas.clear(gfx::Color::TRANSPARENT);
        // The host's canvas IS the slice: translate so content
        // coordinates land at its origin — the widget paints at
        // its layout position, the viewport names the band.
        canvas.save();
        canvas.translate(-x, -y);
        let result = stage.handle(
            &Event::Paint {
                canvas,
                focused: true,
            },
            viewport,
        );
        canvas.restore();
        // HIMARK_PAINT_SPLASH: an unmissable magenta square in the
        // slice's corner — splits "painted transparent" from
        // "present never composited" when a surface shows empty.
        if std::env::var_os("HIMARK_PAINT_SPLASH").is_some() {
            let mut paint = gfx::Paint::default();
            paint.set_color(gfx::Color::from_rgb(255, 0, 255));
            canvas.draw_rect(Rect::from_xywh(4.0, 4.0, 40.0, 40.0), &paint);
        }
        result
    };
    let EventResult::Handled(handled) = result else {
        return false;
    };
    if let Some(reveal) = handled.reveal {
        // A reveal born AT the paint (a canvas revealing its
        // row right after populate) parks exactly like the
        // event road's — the stage road has no scroller, and
        // dropping it here left the scroll waiting for the
        // NEXT event to re-surface it.
        let id = ViewBridgeId::from_raw(bridge.raw);
        if std::env::var_os("HIMARK_TRACE_REVEAL").is_some() {
            eprintln!(
                "[reveal] parked (stage PAINT) bridge {} {reveal:?}",
                id.raw()
            );
        }
        engine.push_reveal(id, reveal);
        if handled.commands.is_empty() {
            return true;
        }
    }
    match handled.commands.is_empty() {
        true => false,
        false => engine.perform_batch(handled.commands),
    }
}

/// Release the frame; its arena goes home warm to the bridge it
/// was derived from. A handle already gone is NOT a refusal: a
/// paint's own batch quiesces the bridge's stages whenever a view
/// command lands (`ToView` needs the view un-borrowed), reaping the
/// very frame the host still holds — the host releases
/// unconditionally and the engine takes it if it still stands.
#[no_mangle]
pub unsafe extern "C" fn himark_frame_release(engine: *mut HimarkEngine, bridge: HimarkBridge) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    let _ = engine.stage_frame_release(ViewBridgeId::from_raw(bridge.raw));
}

// ─── the session surface: query + control (docs/ui/himark-ffi.md) ───

/// The catalog epoch — moved means re-copy. Read after every
/// drain.
#[no_mangle]
pub unsafe extern "C" fn himark_sessions_epoch(engine: *mut HimarkEngine) -> u64 {
    engine
        .as_ref()
        .map_or(0, |engine| engine.revision())
}


/// A snapshot of the REGISTERED COMMANDS — the host-visible verbs
/// (palette material). Same handle discipline as the catalog.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkCommands {
    pub raw: u64,
}

static COMMAND_SNAPSHOTS: std::sync::LazyLock<std::sync::Mutex<Handles<Vec<(String, String)>>>> =
    std::sync::LazyLock::new(Default::default);

#[no_mangle]
pub unsafe extern "C" fn himark_commands_copy(engine: *mut HimarkEngine) -> HimarkCommands {
    let Some(engine) = engine.as_mut() else {
        return HimarkCommands { raw: 0 };
    };
    let snapshot: Vec<(String, String)> = himark::commands::Commands::of(engine.store())
        .iter()
        .map(|command| (command.id().to_owned(), command.name()))
        .collect();
    HimarkCommands {
        raw: COMMAND_SNAPSHOTS
            .lock()
            .expect("the commands table")
            .mint(snapshot),
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_commands_release(
    engine: *mut HimarkEngine,
    commands: HimarkCommands,
) {
    if engine.is_null() {
        return;
    }
    if COMMAND_SNAPSHOTS
        .lock()
        .expect("the commands table")
        .take(commands.raw)
        .is_none()
    {
        refuse("commands", commands.raw);
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_commands_len(
    engine: *mut HimarkEngine,
    commands: HimarkCommands,
) -> usize {
    if engine.is_null() {
        return 0;
    }
    COMMAND_SNAPSHOTS
        .lock()
        .expect("the commands table")
        .get(commands.raw)
        .map_or(0, |snapshot| snapshot.len())
}

#[no_mangle]
pub unsafe extern "C" fn himark_command_id(
    engine: *mut HimarkEngine,
    commands: HimarkCommands,
    index: usize,
    out: *mut c_char,
    cap: usize,
) -> usize {
    if engine.is_null() {
        return 0;
    }
    let held = COMMAND_SNAPSHOTS
        .lock()
        .expect("the commands table")
        .get(commands.raw)
        .and_then(|snapshot| snapshot.get(index).cloned());
    copy_optional_utf8(held.map(|(id, _)| id), out, cap)
}

#[no_mangle]
pub unsafe extern "C" fn himark_command_name(
    engine: *mut HimarkEngine,
    commands: HimarkCommands,
    index: usize,
    out: *mut c_char,
    cap: usize,
) -> usize {
    if engine.is_null() {
        return 0;
    }
    let held = COMMAND_SNAPSHOTS
        .lock()
        .expect("the commands table")
        .get(commands.raw)
        .and_then(|snapshot| snapshot.get(index).cloned());
    copy_optional_utf8(held.map(|(_, name)| name), out, cap)
}




// ─── places and the minting door (docs/ui/himark-ffi.md) ───


#[no_mangle]
pub unsafe extern "C" fn himark_place_release(engine: *mut HimarkEngine, place: HimarkPlace) {
    let Some(_engine) = engine.as_mut() else {
        return;
    };
    if PLACES
        .lock()
        .expect("the place table")
        .take(place.raw)
        .is_none()
    {
        refuse("place", place.raw);
    }
}


/// A bridge's exported WINDOW overlays — the anchors that become
/// the shell's popovers. The epoch moves only when the set
/// changes: poll it per frame, reconcile on change.
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_overlays(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    out_epoch: *mut u64,
) -> usize {
    let Some(engine) = engine.as_mut() else {
        return 0;
    };
    let Some((epoch, anchors)) = engine
        
        .bridge_overlays(himark::bridge::ViewBridgeId::from_raw(bridge.raw))
    else {
        return 0;
    };
    if !out_epoch.is_null() {
        *out_epoch = epoch;
    }
    anchors.len()
}

#[no_mangle]
pub unsafe extern "C" fn himark_bridge_overlay_anchor(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    index: usize,
    out_x: *mut f32,
    out_y: *mut f32,
    out_width: *mut f32,
    out_height: *mut f32,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some((_, anchors)) = engine
        
        .bridge_overlays(himark::bridge::ViewBridgeId::from_raw(bridge.raw))
    else {
        return false;
    };
    let Some((_, _, anchor)) = anchors.get(index) else {
        return false;
    };
    if !out_x.is_null() {
        *out_x = anchor.left;
    }
    if !out_y.is_null() {
        *out_y = anchor.top;
    }
    if !out_width.is_null() {
        *out_width = anchor.width();
    }
    if !out_height.is_null() {
        *out_height = anchor.height();
    }
    true
}

/// An exported overlay's KIND: 0 — a popover the shell flanks to
/// its anchor (completion lists, menus); 1 — a sticky band the
/// shell pins over its scroller's visible top, covering the
/// anchor. Indexed like `himark_bridge_overlay_anchor`.
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_overlay_kind(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    index: usize,
) -> u8 {
    let Some(engine) = engine.as_mut() else {
        return 0;
    };
    let Some((_, anchors)) = engine
        
        .bridge_overlays(himark::bridge::ViewBridgeId::from_raw(bridge.raw))
    else {
        return 0;
    };
    match anchors.get(index) {
        Some((imba::overlay::Attachment::Cover, _, _)) => 1,
        _ => 0,
    }
}

/// An exported overlay's HOST name — with `within` (how many
/// earlier indices share it) this is the overlay's STABLE identity:
/// the exported set reorders as views mount and scroll, and a shell
/// keying its layers by position would swap their contents.
/// Indexed like `himark_bridge_overlay_anchor`.
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_overlay_host(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    index: usize,
    out: *mut std::os::raw::c_char,
    cap: usize,
) -> usize {
    let Some(engine) = engine.as_mut() else {
        return 0;
    };
    let Some((_, anchors)) = engine
        
        .bridge_overlays(himark::bridge::ViewBridgeId::from_raw(bridge.raw))
    else {
        return 0;
    };
    let Some((_, host, _)) = anchors.get(index) else {
        return 0;
    };
    crate::copy_optional_utf8(Some(host.0.to_owned()), out, cap)
}

/// A POPOVER bridge's content slice, in the PARENT'S coordinates
/// — the rect the popover shows. Stashed by the bridge's last
/// layout: lay it (`himark_bridge_layout`) before reading. False
/// while the overlay stands empty.
#[no_mangle]
pub unsafe extern "C" fn himark_overlay_content_rect(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    out_x: *mut f32,
    out_y: *mut f32,
    out_width: *mut f32,
    out_height: *mut f32,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(rect) = engine
        
        .overlay_content(himark::bridge::ViewBridgeId::from_raw(bridge.raw))
    else {
        return false;
    };
    if !out_x.is_null() {
        *out_x = rect.left;
    }
    if !out_y.is_null() {
        *out_y = rect.top;
    }
    if !out_width.is_null() {
        *out_width = rect.width();
    }
    if !out_height.is_null() {
        *out_height = rect.height();
    }
    true
}

/// Mount the POPOVER's bridge for overlay #index of `bridge`.
/// Unmount it like any bridge when the popover closes.
#[no_mangle]
pub unsafe extern "C" fn himark_overlay_mint(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    index: usize,
) -> HimarkBridge {
    let Some(engine) = engine.as_mut() else {
        return HimarkBridge { raw: 0 };
    };
    match engine
        
        .mount_overlay(himark::bridge::ViewBridgeId::from_raw(bridge.raw), index)
    {
        Some(id) => HimarkBridge { raw: id.raw() },
        None => HimarkBridge { raw: 0 },
    }
}

/// Mint the view a place names, mounted as a bridge. The place
/// handle stays usable (mint again, or release when done); 0
/// refuses a place no view answers — typically a session the
/// catalog cannot serve yet, worth retrying after the epoch moves.
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_mint(
    engine: *mut HimarkEngine,
    place: HimarkPlace,
) -> HimarkBridge {
    let Some(engine) = engine.as_mut() else {
        return HimarkBridge { raw: 0 };
    };
    let held = PLACES
        .lock()
        .expect("the place table")
        .get(place.raw)
        .cloned();
    let Some(place) = held else {
        refuse("place", place.raw);
        return HimarkBridge { raw: 0 };
    };
    let (place, payload) = place;
    match himark::mounts::mount_place(engine, &place) {
        Some(id) => {
            // The payload rides to the seated rider (the AHP capi
            // remembers the session on the new bridge).
            if let (Some(payload), Some(rider)) = (
                payload,
                engine.store().get::<PlaceMintRider>().cloned(),
            ) {
                (rider.0)(engine, id, &payload);
            }
            HimarkBridge { raw: id.raw() }
        }
        None => HimarkBridge { raw: 0 },
    }
}

// ─── the shell callback: himark asks the host to navigate ───


/// The host's navigate answer: called on the MAIN thread, in the
/// middle of an engine batch — the host must only RECORD (push the
/// screen async, mint from the handle later) and answer whether it
/// claims the place. `true` claims: the handle is the host's to
/// mint from and release. `false` declines: the engine releases
/// the handle and the workbench fallthrough takes the place.
pub type HimarkNavigateFn = unsafe extern "C" fn(
    ctx: *mut c_void,
    origin: HimarkBridge,
    place: HimarkPlace,
    kind: u32,
    title: *const c_char,
    title_len: usize,
    // `focus`: a DELIBERATE jump (goto-definition, a picked search
    // hit) asks the host to move keyboard focus to the landing; a
    // passive show (an agent opening files mid-turn) must not
    // steal it.
    focus: bool,
) -> bool;

pub fn seat_callback_shell(
    engine: &mut HimarkEngine,
    ctx: *mut c_void,
    on_navigate: HimarkNavigateFn,
) {
    engine.wrap_shell(move |next| {
        std::sync::Arc::new(CallbackShell {
            ctx: ShellCtx(ctx),
            on_navigate,
            next,
        })
    });
}

struct ShellCtx(*mut c_void);
unsafe impl Send for ShellCtx {}
unsafe impl Sync for ShellCtx {}

/// The native shell DECORATING the engine's chain: READY places
/// from mounted origins go to the callback with a minted handle;
/// everything declined falls through to the seated chain.
struct CallbackShell {
    ctx: ShellCtx,
    on_navigate: HimarkNavigateFn,
    next: std::sync::Arc<dyn himark::himark::Shell>,
}

impl himark::himark::Shell for CallbackShell {
    fn navigate(
        &self,
        origin: ViewBridgeId,
        place: hikit::navigation::NavigationLocation,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        if origin.is_mounted() {
            let exported = store
                .get::<NavigateExporter>()
                .cloned()
                .and_then(|exporter| (exporter.0)(&place, origin, store));
            if let Some(export) = exported {
                let handle = mint_place_handle(place.clone(), export.payload);
                let claimed = unsafe {
                    (self.on_navigate)(
                        self.ctx.0,
                        HimarkBridge { raw: origin.raw() },
                        handle,
                        export.kind,
                        export.title.as_ptr() as *const c_char,
                        export.title.len(),
                        export.focus,
                    )
                };
                if claimed {
                    return true;
                }
                PLACES.lock().expect("the place table").take(handle.raw);
            }
        }
        self.next.navigate(origin, place, store, ui, fx)
    }
}

/// Seat the native navigate callback over the engine's shell
/// chain. One installation per engine; the last one wins.
/// Unmount a minted bridge: the slot, its frame and every stage a
/// host still holds for it go together.
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_unmount(engine: *mut HimarkEngine, bridge: HimarkBridge) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    if !engine.unmount(ViewBridgeId::from_raw(bridge.raw)) {
        refuse("bridge", bridge.raw);
    }
}

/// One empty batch: the resident lanes run (hosts connect, the
/// catalog lists). A shell with no first paint calls this once at
/// boot; the wakes carry everything after.
#[no_mangle]
pub unsafe extern "C" fn himark_sync(engine: *mut HimarkEngine) -> bool {
    engine.as_mut().map_or(false, |engine| engine.sync())
}
