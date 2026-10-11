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
use imba::event::{Event, EventResult, MouseButton};
use skia_safe::{Canvas, Point, Rect, Size};

use crate::{copy_optional_utf8, map_key, map_mods, pointed, str_from, HimarkEngine, HimarkPointed};

/// The handle NEWTYPES: single-field structs, so C and Swift see
/// DISTINCT types — an event where a layout belongs is a compile
/// error, not a mixed-up integer. The ABI of a one-u64 struct is
/// the u64 itself; `raw` is 0 for a refusal.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkWindow {
    pub raw: u64,
}

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
pub struct HimarkLayout {
    pub raw: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkFrame {
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

fn mint_event(event: OwnedEvent) -> HimarkEvent {
    HimarkEvent {
        raw: EVENTS.lock().expect("the event table").mint(event),
    }
}

/// The per-engine stage tables: each row remembers the bridge it
/// was derived FROM, so a released frame's arena goes home warm.
/// The C-only tables; the stage tables themselves live on the
/// engine (`Himark`), where the `ToView` road can quiesce them.
#[derive(Default)]
pub(crate) struct Stages {
    catalogs: Handles<himark::himark::Catalog>,
}

/// The place table is process-wide like the event table: places
/// are engine-independent values, and the SEATED shell (called
/// deep inside a batch, no engine struct in reach) mints the
/// handles it hands the native callback.
static PLACES: LazyLock<Mutex<Handles<hikit::navigation::NavigationLocation>>> =
    LazyLock::new(Default::default);

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

/// The window AS a bridge — the degenerate case named. Today the
/// identity is direct; the handle is the only currency the
/// normalized entries below speak.
#[no_mangle]
pub unsafe extern "C" fn himark_window_bridge(
    engine: *mut HimarkEngine,
    window: HimarkWindow,
) -> HimarkBridge {
    if engine.is_null() {
        refuse("engine for window bridge", window.raw);
        return HimarkBridge { raw: 0 };
    }
    HimarkBridge { raw: window.raw }
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
    let bridge = bridge.raw;
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
    bridge: HimarkBridge,
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
        .map_or(false, |engine| engine.draw(bridge.raw, canvas, width, height, scale))
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
) -> HimarkLayout {
    let Some(engine) = engine.as_mut() else {
        return HimarkLayout { raw: 0 };
    };
    let id = ViewBridgeId::from_raw(bridge.raw);
    match engine.app.stage_layout(id, Size::new(width, height)) {
        0 => {
            refuse("bridge", bridge.raw);
            HimarkLayout { raw: 0 }
        }
        raw => HimarkLayout { raw },
    }
}

/// The measure read on the sized stage — the native scroller's
/// content-size question.
#[no_mangle]
pub unsafe extern "C" fn himark_layout_size(
    engine: *mut HimarkEngine,
    layout: HimarkLayout,
    out_width: *mut f32,
    out_height: *mut f32,
) -> bool {
    let Some(engine) = engine.as_ref() else {
        return false;
    };
    let Some(size) = engine.app.stage_layout_size(layout.raw) else {
        refuse("layout", layout.raw);
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
    layout: HimarkLayout,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> HimarkFrame {
    let Some(engine) = engine.as_mut() else {
        return HimarkFrame { raw: 0 };
    };
    match engine
        .app
        .stage_realize(layout.raw, Rect::from_xywh(x, y, width, height))
    {
        0 => {
            refuse("layout", layout.raw);
            HimarkFrame { raw: 0 }
        }
        raw => HimarkFrame { raw },
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_layout_release(engine: *mut HimarkEngine, layout: HimarkLayout) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    if !engine.app.stage_layout_release(layout.raw) {
        refuse("layout", layout.raw);
    }
}

/// One event into a realized stage frame, viewport in content
/// coordinates handed per call. The commands the view answers
/// with run through the engine's one batch loop.
#[no_mangle]
pub unsafe extern "C" fn himark_frame_event(
    engine: *mut HimarkEngine,
    frame: HimarkFrame,
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
        let Some(stage) = engine.app.stage_frame(frame.raw) else {
            refuse("frame", frame.raw);
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
    frame: HimarkFrame,
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
        let Some(stage) = engine.app.stage_frame(frame.raw) else {
            refuse("frame", frame.raw);
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
pub unsafe extern "C" fn himark_frame_release(engine: *mut HimarkEngine, frame: HimarkFrame) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    if !engine.app.stage_frame_release(frame.raw) {
        refuse("frame", frame.raw);
    }
}

// ─── the session surface: query + control (docs/ui/himark-ffi.md) ───

/// The catalog epoch — moved means re-copy. Read after every
/// drain.
#[no_mangle]
pub unsafe extern "C" fn himark_sessions_epoch(engine: *mut HimarkEngine) -> u64 {
    engine.as_ref().map_or(0, |engine| engine.app.sessions_epoch())
}

/// Snapshot the catalog — an owned, immutable copy behind a
/// handle; strings read out of it stay valid until its release.
#[no_mangle]
pub unsafe extern "C" fn himark_sessions_copy(engine: *mut HimarkEngine) -> HimarkCatalog {
    let Some(engine) = engine.as_mut() else {
        return HimarkCatalog { raw: 0 };
    };
    let snapshot = engine.app.catalog();
    HimarkCatalog {
        raw: engine.stages.catalogs.mint(snapshot),
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_release(engine: *mut HimarkEngine, catalog: HimarkCatalog) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    if engine.stages.catalogs.take(catalog.raw).is_none() {
        refuse("catalog", catalog.raw);
    }
}

unsafe fn with_catalog<R>(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    missing: R,
    read: impl FnOnce(&himark::himark::Catalog) -> R,
) -> R {
    let Some(engine) = engine.as_ref() else {
        return missing;
    };
    match engine.stages.catalogs.get(catalog.raw) {
        Some(snapshot) => read(snapshot),
        None => {
            refuse("catalog", catalog.raw);
            missing
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_hosts(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
) -> usize {
    with_catalog(engine, catalog, 0, |snapshot| snapshot.hosts.len())
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_host_id(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
) -> u64 {
    with_catalog(engine, catalog, 0, |snapshot| {
        snapshot.hosts.get(index).map_or(0, |host| host.host.raw())
    })
}

/// 0 idle · 1 connecting · 2 connected · 3 failed.
#[no_mangle]
pub unsafe extern "C" fn himark_catalog_host_status(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
) -> u32 {
    with_catalog(engine, catalog, 0, |snapshot| {
        snapshot.hosts.get(index).map_or(0, |host| host.status)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_host_name(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
    out: *mut c_char,
    cap: usize,
) -> usize {
    with_catalog(engine, catalog, 0, |snapshot| {
        copy_optional_utf8(snapshot.hosts.get(index).map(|host| host.name.clone()), out, cap)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_sessions(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
) -> usize {
    with_catalog(engine, catalog, 0, |snapshot| snapshot.sessions.len())
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_session_host(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
) -> u64 {
    with_catalog(engine, catalog, 0, |snapshot| {
        snapshot.sessions.get(index).map_or(0, |row| row.host.raw())
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_session_status(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
) -> i64 {
    with_catalog(engine, catalog, 0, |snapshot| {
        snapshot.sessions.get(index).map_or(0, |row| row.status)
    })
}

unsafe fn catalog_session_string(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
    out: *mut c_char,
    cap: usize,
    read: impl Fn(&himark::himark::SessionRow) -> String,
) -> usize {
    with_catalog(engine, catalog, 0, |snapshot| {
        copy_optional_utf8(snapshot.sessions.get(index).map(read), out, cap)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_session_uri(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
    out: *mut c_char,
    cap: usize,
) -> usize {
    catalog_session_string(engine, catalog, index, out, cap, |row| row.uri.clone())
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_session_title(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
    out: *mut c_char,
    cap: usize,
) -> usize {
    catalog_session_string(engine, catalog, index, out, cap, |row| row.title.clone())
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_session_provider(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
    out: *mut c_char,
    cap: usize,
) -> usize {
    catalog_session_string(engine, catalog, index, out, cap, |row| row.provider.clone())
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_session_modified(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
    out: *mut c_char,
    cap: usize,
) -> usize {
    catalog_session_string(engine, catalog, index, out, cap, |row| row.modified_at.clone())
}

/// Enter a session headlessly: subscribe, adopt, wire, poll — the
/// native shell's selection. The session's views are mintable
/// after the entry lands; the catalog carries the visible state.
#[no_mangle]
pub unsafe extern "C" fn himark_session_enter(
    engine: *mut HimarkEngine,
    host: u64,
    uri: *const c_char,
    len: usize,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(uri) = str_from(uri, len) else {
        return false;
    };
    engine.app.enter_session(
        ahp_wire::client::HostId::from_raw(host),
        ahp_wire::client::SessionUri::new(uri.to_owned()),
    )
}

/// Create a session headlessly on the host (0 = the local
/// default); the landing enters it.
#[no_mangle]
pub unsafe extern "C" fn himark_session_new(engine: *mut HimarkEngine, host: u64) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let host = match host {
        0 => None,
        raw => Some(ahp_wire::client::HostId::from_raw(raw)),
    };
    engine.app.create_session(host)
}

// ─── places and the minting door (docs/ui/himark-ffi.md) ───

unsafe fn mint_session_place(
    engine: *mut HimarkEngine,
    host: u64,
    uri: *const c_char,
    len: usize,
    place: impl Fn(ahp_wire::SessionId) -> hikit::navigation::NavigationLocation,
) -> HimarkPlace {
    let Some(engine) = engine.as_mut() else {
        return HimarkPlace { raw: 0 };
    };
    let Some(uri) = str_from(uri, len) else {
        return HimarkPlace { raw: 0 };
    };
    let session = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::from_raw(host),
        session: ahp_wire::client::SessionUri::new(uri.to_owned()),
    };
    let _ = engine;
    HimarkPlace {
        raw: PLACES.lock().expect("the place table").mint(place(session)),
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_place_chat(
    engine: *mut HimarkEngine,
    host: u64,
    uri: *const c_char,
    len: usize,
) -> HimarkPlace {
    mint_session_place(engine, host, uri, len, |session| {
        hikit::navigation::NavigationLocation::new(himark::places::ChatPlace(session))
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_place_changes(
    engine: *mut HimarkEngine,
    host: u64,
    uri: *const c_char,
    len: usize,
) -> HimarkPlace {
    mint_session_place(engine, host, uri, len, |session| {
        hikit::navigation::NavigationLocation::new(himark::places::ChangesPlace(session))
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_place_files(
    engine: *mut HimarkEngine,
    host: u64,
    uri: *const c_char,
    len: usize,
) -> HimarkPlace {
    mint_session_place(engine, host, uri, len, |session| {
        hikit::navigation::NavigationLocation::new(himark::places::FilesPlace(session))
    })
}

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
    match himark::mounts::mount_place(&mut engine.app, &place) {
        Some(id) => HimarkBridge { raw: id.raw() },
        None => HimarkBridge { raw: 0 },
    }
}

// ─── the shell callback: himark asks the host to navigate ───

/// The place-kind vocabulary the callback speaks. `DOCUMENT` and
/// `DIFF` are READY places — minting is synchronous; the session
/// kinds are the tab places the host already knows how to mint.
pub const HIMARK_PLACE_CHAT: u32 = 1;
pub const HIMARK_PLACE_CHANGES: u32 = 2;
pub const HIMARK_PLACE_FILES: u32 = 3;
pub const HIMARK_PLACE_DOCUMENT: u32 = 4;
pub const HIMARK_PLACE_DIFF: u32 = 5;

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
) -> bool;

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
            let classified = if let Some(ready) =
                place.place::<himark::places::OpenDocumentPlace>()
            {
                // The document's name: it is registered by the time
                // this place is navigated (the open landing's law).
                let title = ahp_session::session::state::Hosts::states(store)
                    .into_iter()
                    .map(|state| state.documents())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .find_map(|collection| {
                        documents::OpenDocuments::name(store, collection, ready.document)
                    })
                    .unwrap_or_default();
                Some((HIMARK_PLACE_DOCUMENT, title))
            } else if let Some(ready) = place.place::<himark::places::DiffViewPlace>() {
                Some((HIMARK_PLACE_DIFF, ready.title.clone()))
            } else {
                None
            };
            if let Some((kind, title)) = classified {
                let handle = HimarkPlace {
                    raw: PLACES.lock().expect("the place table").mint(place.clone()),
                };
                let claimed = unsafe {
                    (self.on_navigate)(
                        self.ctx.0,
                        HimarkBridge { raw: origin.raw() },
                        handle,
                        kind,
                        title.as_ptr() as *const c_char,
                        title.len(),
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
#[no_mangle]
pub unsafe extern "C" fn himark_set_shell(
    engine: *mut HimarkEngine,
    ctx: *mut c_void,
    on_navigate: HimarkNavigateFn,
) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    engine.app.seat_shell(move |next| {
        std::sync::Arc::new(CallbackShell {
            ctx: ShellCtx(ctx),
            on_navigate,
            next,
        })
    });
}

/// Unmount a minted bridge: the slot, its frame and every stage a
/// host still holds for it go together.
#[no_mangle]
pub unsafe extern "C" fn himark_bridge_unmount(engine: *mut HimarkEngine, bridge: HimarkBridge) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    if !engine.app.unmount(ViewBridgeId::from_raw(bridge.raw)) {
        refuse("bridge", bridge.raw);
    }
}

/// One empty batch: the resident lanes run (hosts connect, the
/// catalog lists). A shell with no first paint calls this once at
/// boot; the wakes carry everything after.
#[no_mangle]
pub unsafe extern "C" fn himark_sync(engine: *mut HimarkEngine) -> bool {
    engine.as_mut().map_or(false, |engine| engine.app.sync())
}
