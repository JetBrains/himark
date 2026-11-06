// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The C BRIDGE to himark — sessions, minted bridges, the stage
//! road, IME, overlays: the surface a native shell (the iOS app)
//! consumes. Window-free by construction: the workbench's own C
//! half lives in `workbench-shell::capi`.

use himark::app_ext::AppExt as _;
#[allow(unused_imports)]
use std::collections::VecDeque;
use std::ffi::{c_char, c_void};
use std::sync::{Arc, Mutex};

use ahp_docsync as docsync;
use ahp_locations::find;
use ahp_session::fsroute;
pub mod bridges;
pub use crate::bridges::himark_perform_command;
pub mod host;
use himark::app::AppFonts;
use ahp_wire::uris;
pub use host::{HimarkHostCallbacks, HimarkLocation};

use himark::app::AppCommand;
use himark::app::Himark;
use himark::effects::BackgroundRunner;
use imba::anim::AnimationClock;
pub use imba::event::PointerShape;
use imba::event::{Event, Key, MouseButton};

/// A pointer event's answer: whether a frame is owed, and the shape
/// the pointer takes over what the hit test found. Derived from this
/// event alone — the shell drives the platform cursor from it and
/// keeps nothing, so no stale shape can stick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pointed {
    pub changed: bool,
    pub shape: PointerShape,
}
use gfx::{Canvas, Point, Size};

type WakeCallback = Box<dyn Fn() + Send + 'static>;



#[derive(Clone, Copy, Default)]
pub struct AgentHostFilesystemCapabilities {
    fetch_document: bool,
    store_document: bool,
    list_directory: bool,
}

impl AgentHostFilesystemCapabilities {
    pub const fn all() -> Self {
        Self {
            fetch_document: true,
            store_document: true,
            list_directory: true,
        }
    }

    fn include(&mut self, other: Self) {
        self.fetch_document |= other.fetch_document;
        self.store_document |= other.store_document;
        self.list_directory |= other.list_directory;
    }
}

pub struct HimarkEngine {
    pub app: Himark,

    /// The plugin-collected language set, shared — the late
    /// capability-gated routes and the open road reparse with it.
    languages: Arc<editor::reparse::SyntaxLanguages>,

    /// The normalized surface's stage tables (`bridges`).
    pub(crate) stages: crate::bridges::Stages,

    pub compose_new_windows: bool,

    host: Option<Arc<host::HostBridge>>,

    agent_host_filesystem: AgentHostFilesystemCapabilities,

    clients: Arc<ahp_wire::fs::ClientDirectory>,

    _document_channels: Arc<docsync::DocumentChannels>,

    resource_uris: Arc<dyn ahp_wire::client::ResourceUriMap>,

    pub clicks: ClickCounter,

    /// The cut text between the host's two sized-string calls
    /// (`himark_cut`): the sizing probe performs the cut — it deletes
    /// the selection, so it must run exactly once — and the fill call
    /// drains this stash.
    pending_cut: Option<String>,

}

#[derive(Default)]
pub struct ClickCounter {
    last: Option<std::time::Instant>,
    point: (f32, f32),
    count: u8,
}

impl ClickCounter {
    const SLACK: f32 = 6.0;

    pub fn count(&mut self, x: f32, y: f32) -> u8 {
        let now = std::time::Instant::now();
        let run = self
            .last
            .is_some_and(|last| now.duration_since(last).as_millis() < 500)
            && (x - self.point.0).abs() <= Self::SLACK
            && (y - self.point.1).abs() <= Self::SLACK;
        self.count = if run { (self.count + 1).min(3) } else { 1 };
        self.last = Some(now);
        self.point = (x, y);
        self.count
    }
}



pub use himark::host::Worker as HimarkWorker;

#[derive(Clone, Copy)]
struct Wake {
    callback: extern "C" fn(*mut c_void),
    context: *mut c_void,
}

unsafe impl Send for Wake {}

impl Wake {
    fn fire(self) {
        (self.callback)(self.context);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HimarkRange {
    pub start: u32,

    pub length: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HimarkRect {
    pub x: f32,

    pub y: f32,

    pub width: f32,

    pub height: f32,
}

pub const HIMARK_KEY_BACKSPACE: u32 = 1;

pub const HIMARK_KEY_ENTER: u32 = 2;

pub const HIMARK_KEY_LEFT: u32 = 3;

pub const HIMARK_KEY_RIGHT: u32 = 4;

pub const HIMARK_KEY_UP: u32 = 5;

pub const HIMARK_KEY_DOWN: u32 = 6;

pub const HIMARK_KEY_ESCAPE: u32 = 7;

pub const HIMARK_KEY_TAB: u32 = 8;

pub const HIMARK_KEY_HOME: u32 = 9;

pub const HIMARK_KEY_END: u32 = 10;

pub const HIMARK_KEY_PAGE_UP: u32 = 11;

pub const HIMARK_KEY_PAGE_DOWN: u32 = 12;

pub const HIMARK_KEY_DELETE: u32 = 13;

pub const HIMARK_KEY_F1: u32 = 20;

pub const HIMARK_KEY_CHAR_BASE: u32 = 0x20;

pub const HIMARK_MOD_SHIFT: u32 = 1;

pub const HIMARK_MOD_CONTROL: u32 = 2;

pub const HIMARK_MOD_ALT: u32 = 4;

pub const HIMARK_MOD_COMMAND: u32 = 8;

/// No phase information: a legacy wheel tick, or a host that cannot
/// tell gestures apart.
pub const HIMARK_SCROLL_PHASE_NONE: u32 = 0;

pub const HIMARK_SCROLL_PHASE_MAY_BEGIN: u32 = 1;

pub const HIMARK_SCROLL_PHASE_BEGAN: u32 = 2;

pub const HIMARK_SCROLL_PHASE_CHANGED: u32 = 3;

pub const HIMARK_SCROLL_PHASE_ENDED: u32 = 4;

pub const HIMARK_SCROLL_PHASE_MOMENTUM_BEGAN: u32 = 5;

pub const HIMARK_SCROLL_PHASE_MOMENTUM_CHANGED: u32 = 6;

pub const HIMARK_SCROLL_PHASE_MOMENTUM_ENDED: u32 = 7;

/// Whether this event starts a new scroll gesture. Explicit finger-down
/// phases always do; momentum never does — it belongs to the flick that
/// spawned it. Phaseless events and bare `changed` streams (hosts that
/// never report a `began`) fall back to the pause heuristic so a stale
/// owner cannot wedge the routing forever.
pub(crate) fn scroll_phase(phase: u32) -> himark::host::ScrollPhase {
    use himark::host::ScrollPhase as P;
    match phase {
        HIMARK_SCROLL_PHASE_MAY_BEGIN => P::MayBegin,
        HIMARK_SCROLL_PHASE_BEGAN => P::Began,
        HIMARK_SCROLL_PHASE_CHANGED => P::Changed,
        HIMARK_SCROLL_PHASE_ENDED => P::Ended,
        HIMARK_SCROLL_PHASE_CANCELLED => P::Cancelled,
        HIMARK_SCROLL_PHASE_MOMENTUM_BEGAN => P::MomentumBegan,
        HIMARK_SCROLL_PHASE_MOMENTUM_CHANGED => P::MomentumChanged,
        HIMARK_SCROLL_PHASE_MOMENTUM_ENDED => P::MomentumEnded,
        _ => P::None,
    }
}

pub fn scroll_gesture_boundary(phase: u32, paused: bool) -> bool {
    match phase {
        HIMARK_SCROLL_PHASE_MAY_BEGIN | HIMARK_SCROLL_PHASE_BEGAN => true,
        HIMARK_SCROLL_PHASE_ENDED
        | HIMARK_SCROLL_PHASE_MOMENTUM_BEGAN
        | HIMARK_SCROLL_PHASE_MOMENTUM_CHANGED
        | HIMARK_SCROLL_PHASE_MOMENTUM_ENDED => false,
        _ => paused,
    }
}

pub fn map_key(code: u32) -> Option<Key> {
    Some(match code {
        HIMARK_KEY_BACKSPACE => Key::Backspace,
        HIMARK_KEY_ENTER => Key::Enter,
        HIMARK_KEY_LEFT => Key::Left,
        HIMARK_KEY_RIGHT => Key::Right,
        HIMARK_KEY_UP => Key::Up,
        HIMARK_KEY_DOWN => Key::Down,
        HIMARK_KEY_ESCAPE => Key::Escape,
        HIMARK_KEY_TAB => Key::Tab,
        HIMARK_KEY_HOME => Key::Home,
        HIMARK_KEY_END => Key::End,
        HIMARK_KEY_PAGE_UP => Key::PageUp,
        HIMARK_KEY_PAGE_DOWN => Key::PageDown,
        HIMARK_KEY_DELETE => Key::Delete,
        code if (HIMARK_KEY_F1..HIMARK_KEY_F1 + 12).contains(&code) => {
            Key::F((code - HIMARK_KEY_F1 + 1) as u8)
        }
        code if code >= HIMARK_KEY_CHAR_BASE => Key::Char(char::from_u32(code)?),
        _ => return None,
    })
}

pub fn map_mods(mods: u32) -> imba::event::Modifiers {
    imba::event::Modifiers {
        shift: mods & HIMARK_MOD_SHIFT != 0,
        control: mods & HIMARK_MOD_CONTROL != 0,
        alt: mods & HIMARK_MOD_ALT != 0,
        command: mods & HIMARK_MOD_COMMAND != 0,
    }
}



/// Everything the engine needs from its host, taken AT CREATION —
/// no half-wired engine ever exists, and the effects a boot batch
/// launches wake the host's executor instead of firing into empty
/// slots. One `ctx` serves every callback; `on_navigate` may be
/// null for a shell that claims no places.
#[repr(C)]
pub struct HimarkEngineCallbacks {
    pub ctx: *mut c_void,
    pub wake: extern "C" fn(*mut c_void),
    pub effect_wake: extern "C" fn(*mut c_void),
    #[allow(clippy::type_complexity)]
    pub on_navigate: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            origin: crate::bridges::HimarkBridge,
            place: crate::bridges::HimarkPlace,
            kind: u32,
            title: *const c_char,
            title_len: usize,
            focus: bool,
        ) -> bool,
    >,
}

impl HimarkEngine {
    pub fn worker(&self) -> HimarkWorker {
        self.app.worker()
    }

    pub fn set_wake(&self, callback: impl Fn() + Send + 'static) {
        self.app.set_wake(callback);
    }

    pub fn set_wake_raw(&self, callback: extern "C" fn(*mut c_void), context: *mut c_void) {
        let wake = Wake { callback, context };
        self.set_wake(move || wake.fire());
    }

    pub fn set_effect_wake(&self, callback: impl Fn() + Send + 'static) {
        self.app.set_effect_wake(callback);
    }

    pub fn set_effect_wake_raw(&self, callback: extern "C" fn(*mut c_void), context: *mut c_void) {
        let wake = Wake { callback, context };
        self.set_effect_wake(move || wake.fire());
    }

    #[doc(hidden)]
    pub fn set_drain_pacing(&mut self, chunk: usize, budget: std::time::Duration) {
        self.app.set_drain_pacing(chunk, budget);
    }

    #[doc(hidden)]
    pub fn queued_landings(&self) -> usize {
        self.app.queued_landings()
    }

    pub fn drain(&mut self) -> bool {
        self.app.drain()
    }



    pub fn record_latency(&mut self, now_secs: f64) {
        let latency_ns = self.app.stats().latency_ns();
        if let Some(event_started_at) = self.app.stats().latency_event_start() {
            let latency = (now_secs - event_started_at).max(0.0);
            latency_ns.store(
                (latency * 1_000_000_000.0) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            self.app.stats_mut().clear_latency_event_start();
        }
    }









    pub fn clipboard_copy(&mut self, window: u64) -> Option<String> {
        self.app
            .with_clipboard_client(himark::bridge::ViewBridgeId::from_raw(window), |client| {
                client.copy().map(|content| content.text)
            })
            .flatten()
    }

    pub fn clipboard_cut(&mut self, window: u64) -> Option<String> {
        self.app
            .with_clipboard_client(himark::bridge::ViewBridgeId::from_raw(window), |client| {
                client.cut().map(|content| content.text)
            })
            .flatten()
    }

    pub fn clipboard_paste(&mut self, window: u64, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        self.app
            .with_clipboard_client(himark::bridge::ViewBridgeId::from_raw(window), |client| {
                client.paste(&imba::clipboard::ClipboardContent {
                    text: text.to_owned(),
                })
            })
            .unwrap_or(false)
    }

    // Pub like its C export `himark_text_input_replacing`: the Rust
    // shell walks the same IME road the Swift one does.
    pub fn text_input_replacing(
        &mut self,
        window: u64,
        text: &str,
        replacement: HimarkRange,
    ) -> bool {
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| {
                client.insert_text(text, Some((replacement.start, replacement.length)))
            })
            .is_some()
    }



















    pub fn scroll(&mut self, window: u64, x: f32, y: f32, delta_x: f32, delta_y: f32) -> bool {
        self.scroll_at_time(window, x, y, delta_x, delta_y, 0.0)
    }

    pub(crate) fn scroll_at_time(
        &mut self,
        window: u64,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        event_started_at: f64,
    ) -> bool {
        self.scroll_phased_at_time(
            window,
            x,
            y,
            delta_x,
            delta_y,
            HIMARK_SCROLL_PHASE_NONE,
            event_started_at,
        )
    }

    /// A scroll carrying the host's gesture phase. The phase decides where
    /// one gesture ends and the next begins, which is what pins a whole
    /// flick — momentum included — to the scroller it started on, the way
    /// native scroll views route. Hosts without phase information pass
    /// `HIMARK_SCROLL_PHASE_NONE` and fall back to treating a 250 ms pause
    /// as the boundary.
    pub fn scroll_phased_at_time(
        &mut self,
        window: u64,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        phase: u32,
        event_started_at: f64,
    ) -> bool {
        self.app.scroll_phased_at_time(
            himark::bridge::ViewBridgeId::from_raw(window),
            x,
            y,
            delta_x,
            delta_y,
            scroll_phase(phase),
            event_started_at,
        )
    }

    /// A synchronous wheel OFFER to a stage frame: the overlays that
    /// still own their scrolling after the descrollering — a
    /// completion popup's list, a horizontal pan in unwrapped code —
    /// consume it; `false` hands the wheel back to the host's
    /// scroller, which owns the vertical physics on the stage road
    /// (`himark_frame_event` keeps refusing Scroll for that reason).
    pub fn offer_stage_scroll(
        &mut self,
        frame: u64,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        phase: u32,
        viewport: gfx::Rect,
    ) -> bool {
        self.app
            .offer_stage_scroll(frame, x, y, delta_x, delta_y, scroll_phase(phase), viewport)
    }

    /// Dispatch the window's accumulated wheel deltas: the per-tick
    /// application of a scroll. Also the ordering flush before any
    /// other input, so a click or keystroke lands on scrolled
    /// content, and before a gesture boundary or a moved point, so
    /// claims resolve exactly as they would have per event.
    pub fn flush_scroll(&mut self, window: u64) -> bool {
        self.app
            .flush_scroll(himark::bridge::ViewBridgeId::from_raw(window))
    }

    pub fn animation_tick(&mut self, now_ms: f64) -> bool {
        self.app.animation_tick(now_ms)
    }







    /// Perform a registered command at a BRIDGE origin — the
    /// general road: the native palette and keymaps bind to ids and
    /// perform them here.
    pub fn perform_command(&mut self, bridge: u64, id: &str) -> bool {
        self.app
            .perform_registered_at(himark::bridge::ViewBridgeId::from_raw(bridge), id)
    }

    pub fn install_agent_host_filesystem(&mut self, capabilities: AgentHostFilesystemCapabilities) {
        let installed = self.agent_host_filesystem;

        if capabilities.fetch_document && !installed.fetch_document {
            himark::hiahp::install_open_handlers(
                &mut self.app,
                Arc::clone(&self.languages),
                Arc::new(myersdiff::Myers),
            );
            self.app
                .observe_stripe_bases(Arc::new(fsroute::resolve_base));
            self.app
                .register_handler::<::editor::completion::CompletionEffect>(
                    ahp_lsp::CompletionRoute {
                        directory: Arc::clone(&self.clients),
                        uris: Arc::clone(&self.resource_uris),
                    },
                );
            self.app
                .register_handler::<::editor::hover::HoverEffect>(ahp_lsp::HoverRoute {
                    directory: Arc::clone(&self.clients),
                    uris: Arc::clone(&self.resource_uris),
                });
            self.app
                .register_handler::<ahp_locations::LspLocationsEffect>(
                    ahp_locations::routes::RouteLspLocations {
                        directory: Arc::clone(&self.clients),
                        uris: Arc::clone(&self.resource_uris),
                    },
                );
        }
        if capabilities.list_directory && !installed.list_directory {
            self.app
                .register_handler::<::editor::completion::PathCompletionEffect>(
                    find::NativeFindHandler {
                        directory: Arc::clone(&self.clients),
                    },
                );
            self.app
                .register_handler::<ahp_locations::FindEffect>(find::NativeFindHandler {
                    directory: Arc::clone(&self.clients),
                });
            self.app
                .register_handler::<ahp_locations::SearchLocationsEffect>(
                    ahp_locations::routes::RouteSearchLocations {
                        directory: Arc::clone(&self.clients),
                    },
                );
            self.app
                .register_handler::<ahp_locations::SymbolLocationsEffect>(
                    ahp_locations::routes::RouteSymbolLocations {
                        directory: Arc::clone(&self.clients),
                    },
                );
        }

        self.agent_host_filesystem.include(capabilities);
    }

    pub fn set_host(&mut self, callbacks: HimarkHostCallbacks) {
        let bridge = host::HostBridge::new(callbacks);
        if callbacks.pick_files.is_some() {
            self.app
                .register_handler::<host::FilePickerEffect>(host::FilePickerHandler(Arc::clone(
                    &bridge,
                )));

            self.app
                .register_handler::<himark::higent::folder_grant::PickFoldersEffect>(
                    host::PickFoldersHandler(Arc::clone(&bridge)),
                );
            if let Some(chrome) = self.app.store().get::<host::HostChrome>().cloned() {
                if let Some(on_pick_files) = chrome.on_pick_files {
                    on_pick_files(&mut self.app);
                }
            }
        }
        if callbacks.set_clipboard.is_some() {
            self.app
                .register_handler::<ahp_wire::effects::ShareHostEffect>(host::ShareHostHandler(
                    Arc::clone(&bridge),
                ));
            if let Some(chrome) = self.app.store().get::<host::HostChrome>().cloned() {
                if let Some(on_clipboard) = chrome.on_clipboard {
                    on_clipboard(&mut self.app);
                }
            }
            self.app
                .register_handler::<ahp_wire::effects::ShareTailnetEffect>(
                    host::ShareTailnetHandler(Arc::clone(&bridge)),
                );
        }

        self.install_agent_host_filesystem(AgentHostFilesystemCapabilities {
            fetch_document: callbacks.fetch_document.is_some(),
            store_document: callbacks.store_document.is_some(),
            list_directory: callbacks.list_directory.is_some(),
        });
        if callbacks.pick_save.is_some() {
            self.app
                .register_handler::<documents::PickSaveEffect>(host::PickSaveHandler(Arc::clone(
                    &bridge,
                )));
        }
        self.host = Some(bridge);
    }

    pub fn host_picked(
        &mut self,
        request: u64,
        locations: Vec<editor::location::ResourceLocation>,
    ) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.requests.fulfill(request, Box::new(locations)))
    }

    pub fn host_picked_folder(
        &mut self,
        request: u64,
        location: Option<editor::location::ResourceLocation>,
    ) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.requests.fulfill(request, Box::new(location)))
    }

    pub fn host_fetched(&mut self, request: u64, text: Option<String>) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.requests.fulfill(request, Box::new(text)))
    }

    #[doc(hidden)]
    pub fn file_changed(&mut self, subscription: u64) -> bool {
        if subscription == 0 {
            return false;
        }
        self.app
            .poster()
            .post(himark::app::AppCommand::FileChanged(
                documents::watch::Subscription(subscription),
            ));
        true
    }

    pub(crate) fn host_stored(&mut self, request: u64, stored: bool) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.requests.fulfill(request, Box::new(stored)))
    }

    pub(crate) fn host_listed(
        &mut self,
        request: u64,
        entries: Option<Vec<editor::location::ResourceLocation>>,
    ) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.requests.fulfill(request, Box::new(entries)))
    }



    pub fn has_text_focus(&mut self, window: u64) -> bool {
        self.app.with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |_| ()).is_some()
    }

    pub fn has_marked_text(&mut self, window: u64) -> bool {
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| client.has_marked_text())
            .unwrap_or(false)
    }

    pub fn marked_range(&mut self, window: u64) -> Option<HimarkRange> {
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| client.marked_range())?
            .map(|(start, end)| HimarkRange {
                start,
                length: end.saturating_sub(start),
            })
    }

    pub fn selected_range(&mut self, window: u64) -> Option<HimarkRange> {
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| client.selected_range())?
            .map(|(start, end)| HimarkRange {
                start,
                length: end.saturating_sub(start),
            })
    }

    pub fn set_marked_text(
        &mut self,
        window: u64,
        text: &str,
        selected: HimarkRange,
        replacement: Option<HimarkRange>,
    ) -> bool {
        let replacement = replacement.map(|range| (range.start, range.length));
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| {
                client.set_marked_text(text, (selected.start, selected.length), replacement)
            })
            .is_some()
    }

    pub fn unmark_text(&mut self, window: u64) -> bool {
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| client.unmark_text())
            .is_some()
    }

    pub fn set_selected_range(&mut self, window: u64, range: HimarkRange) -> bool {
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| {
                client.set_selected_range(range.start, range.length)
            })
            .is_some()
    }

    pub fn document_length(&mut self, window: u64) -> Option<u32> {
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| client.document_length())
    }

    pub fn reveal_selection(&mut self, window: u64) -> bool {
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| client.reveal_selection())
            .is_some()
    }

    pub fn selection_rects(&mut self, window: u64, range: HimarkRange) -> Vec<HimarkRect> {
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| {
                client
                    .selection_rects(range.start, range.length)
                    .into_iter()
                    .map(|(x, y, width, height)| HimarkRect {
                        x,
                        y,
                        width,
                        height,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn first_rect(&mut self, window: u64, range: HimarkRange) -> Option<HimarkRect> {
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| {
                client.first_rect(range.start, range.length)
            })?
            .map(|(x, y, width, height)| HimarkRect {
                x,
                y,
                width,
                height,
            })
    }

    pub fn char_index_at(&mut self, window: u64, x: f32, y: f32) -> Option<u32> {
        self.app
            .with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| client.char_index_at(x, y))?
    }

    pub fn substring(&mut self, window: u64, range: HimarkRange) -> Option<String> {
        self.app.with_ime_client(himark::bridge::ViewBridgeId::from_raw(window), |client| {
            client.substring_utf16(range.start, range.length)
        })?
    }

    unsafe fn substring_raw(
        &mut self,
        window: u64,
        range: HimarkRange,
        out: *mut c_char,
        cap: usize,
    ) -> usize {
        copy_optional_utf8(self.substring(window, range), out, cap)
    }

    unsafe fn marked_range_raw(&mut self, window: u64, out: *mut HimarkRange) -> bool {
        write_optional_out(self.marked_range(window), out)
    }

    unsafe fn selected_range_raw(&mut self, window: u64, out: *mut HimarkRange) -> bool {
        write_optional_out(self.selected_range(window), out)
    }

    unsafe fn first_rect_raw(
        &mut self,
        window: u64,
        range: HimarkRange,
        out: *mut HimarkRect,
    ) -> bool {
        write_optional_out(self.first_rect(window, range), out)
    }

    unsafe fn document_length_raw(&mut self, window: u64, out: *mut u32) -> bool {
        write_optional_out(self.document_length(window), out)
    }

    unsafe fn selection_rects_raw(
        &mut self,
        window: u64,
        range: HimarkRange,
        out: *mut HimarkRect,
        cap: usize,
    ) -> usize {
        let rects = self.selection_rects(window, range);
        if out.is_null() || rects.len() > cap {
            return rects.len();
        }
        std::ptr::copy_nonoverlapping(rects.as_ptr(), out, rects.len());
        rects.len()
    }
}

impl HimarkEngine {
    /// Assemble the C engine over a BOOTED app and its wires — the
    /// composition root (himark-capi, an app crate) builds the app
    /// (plugins, host loop, connector) and hands it in whole.
    #[allow(clippy::too_many_arguments)]
    pub fn assemble(
        app: Himark,
        languages: Arc<editor::reparse::SyntaxLanguages>,
        clients: Arc<ahp_wire::fs::ClientDirectory>,
        document_channels: Arc<docsync::DocumentChannels>,
        resource_uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
    ) -> Self {
        Self {
            app,
            languages,
            compose_new_windows: false,
            clicks: ClickCounter::default(),
            stages: Default::default(),
            pending_cut: None,
            host: None,
            agent_host_filesystem: AgentHostFilesystemCapabilities::default(),
            clients,
            _document_channels: document_channels,
            resource_uris,
        }
    }

    pub fn register_agent_server(
        &mut self,
        name: &str,
        client: ahp_wire::client::Client,
    ) -> ahp_wire::client::HostId {
        let id = self.app.register_client(client.clone());
        let clients = Arc::clone(&self.clients);
        self.app.setup(|store| {
            ahp_session::session::agents::Agents::seed(store, id, name);
            ahp_session::session::state::Hosts::install_uris(
                store,
                id,
                Arc::new(uris::FileUris),
            );
        });
        clients.record(id, client);
        id
    }

    pub fn set_local_backend(&mut self, server: ahp_wire::client::HostId) {
        self.clients.set_local(server);
        self.app.set_local_host(server);
    }

    /// The wire client directory and the uri map — the HOST's late
    /// registrations (routes for effects whose types sit above this
    /// crate) close over them.
    pub fn languages(&self) -> Arc<editor::reparse::SyntaxLanguages> {
        Arc::clone(&self.languages)
    }

    pub fn clients(&self) -> Arc<ahp_wire::fs::ClientDirectory> {
        Arc::clone(&self.clients)
    }

    pub fn resource_uris(&self) -> Arc<dyn ahp_wire::client::ResourceUriMap> {
        Arc::clone(&self.resource_uris)
    }

    pub fn runtime(&self) -> tokio::runtime::Handle {
        self.app.runtime()
    }
}





#[no_mangle]
pub unsafe extern "C" fn himark_destroy(engine: *mut HimarkEngine) {
    if !engine.is_null() {
        drop(Box::from_raw(engine));
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_run_pending(engine: *mut HimarkEngine) {
    if let Some(engine) = engine.as_ref() {
        engine.worker().run_pending();
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_drain(engine: *mut HimarkEngine) -> bool {
    engine.as_mut().map_or(false, HimarkEngine::drain)
}

#[no_mangle]
pub unsafe extern "C" fn himark_copy(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    out: *mut c_char,
    cap: usize,
) -> usize {
    engine.as_mut().map_or(0, |engine| {
        copy_optional_utf8(engine.clipboard_copy(bridge.raw), out, cap)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_cut(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    out: *mut c_char,
    cap: usize,
) -> usize {
    engine.as_mut().map_or(0, |engine| {
        // The sized-string protocol calls twice. Cut is
        // side-effecting — the first run deletes the selection — so
        // it runs ONLY on the sizing probe; the fill call copies the
        // stashed text instead of cutting again (which would find no
        // selection and hand the pasteboard nothing).
        if out.is_null() {
            engine.pending_cut = engine.clipboard_cut(bridge.raw);
            return engine.pending_cut.as_ref().map_or(0, String::len);
        }
        copy_optional_utf8(engine.pending_cut.take(), out, cap)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_paste(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    utf8: *const c_char,
    len: usize,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(text) = str_from(utf8, len) else {
        return false;
    };
    engine.clipboard_paste(bridge.raw, text)
}

#[no_mangle]
pub unsafe extern "C" fn himark_text_input_replacing(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    utf8: *const c_char,
    len: usize,
    replacement: HimarkRange,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(text) = str_from(utf8, len) else {
        return false;
    };
    engine.text_input_replacing(bridge.raw, text, replacement)
}



/// The pointer event's answer across the FFI: `changed` owes a frame;
/// `shape` is 0 for the platform default arrow, 1 for the pointing
/// hand. The shell applies the shape right away and stores nothing.
#[repr(C)]
pub struct HimarkPointed {
    pub changed: bool,
    pub shape: u32,
}

pub(crate) fn pointed(pointed: Option<Pointed>) -> HimarkPointed {
    let pointed = pointed.unwrap_or(Pointed {
        changed: false,
        shape: PointerShape::Default,
    });
    HimarkPointed {
        changed: pointed.changed,
        shape: match pointed.shape {
            PointerShape::Default => 0,
            PointerShape::Pointer => 1,
        },
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_animation_tick(engine: *mut HimarkEngine, now_ms: f64) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.animation_tick(now_ms))
}





/// The desktop form: the workbench's theme pair (theme.json,
/// theme-light.json) — the engine's own default.
pub const HIMARK_FORM_DESKTOP: u32 = 0;
/// The phone form: the iOS pair (theme-ios.json, theme-ios-light.json)
/// — 3x metrics, rows that are tap targets, the phone's insets.
pub const HIMARK_FORM_PHONE: u32 = 1;



#[no_mangle]
pub unsafe extern "C" fn himark_set_host(
    engine: *mut HimarkEngine,
    callbacks: HimarkHostCallbacks,
) {
    if let Some(engine) = engine.as_mut() {
        engine.set_host(callbacks);
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_host_picked(
    engine: *mut HimarkEngine,
    request: u64,
    locations: *const HimarkLocation,
    count: usize,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let mut picked = Vec::with_capacity(count);
    if count > 0 {
        if locations.is_null() {
            return false;
        }
        for location in std::slice::from_raw_parts(locations, count) {
            let Some(location) = host::location_from_abi(location) else {
                return false;
            };
            picked.push(location);
        }
    }
    engine.host_picked(request, picked)
}

#[no_mangle]
pub unsafe extern "C" fn himark_host_picked_folder(
    engine: *mut HimarkEngine,
    request: u64,
    location: *const HimarkLocation,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let location = match location.is_null() {
        true => None,
        false => match host::location_from_abi(&*location) {
            Some(location) => Some(location),
            None => return false,
        },
    };
    engine.host_picked_folder(request, location)
}

#[no_mangle]
pub unsafe extern "C" fn himark_host_fetched(
    engine: *mut HimarkEngine,
    request: u64,
    text: *const c_char,
    text_len: usize,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let text = match text.is_null() {
        true => None,
        false => match str_from(text, text_len) {
            Some(text) => Some(text.to_owned()),
            None => return false,
        },
    };
    engine.host_fetched(request, text)
}

#[no_mangle]
pub unsafe extern "C" fn himark_host_stored(
    engine: *mut HimarkEngine,
    request: u64,
    stored: bool,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    engine.host_stored(request, stored)
}

#[no_mangle]
pub unsafe extern "C" fn himark_host_listed(
    engine: *mut HimarkEngine,
    request: u64,
    entries: *const HimarkLocation,
    count: usize,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let entries = match entries.is_null() {
        true => None,
        false => {
            let mut listed = Vec::with_capacity(count);
            for entry in std::slice::from_raw_parts(entries, count) {
                let Some(location) = host::location_from_abi(entry) else {
                    return false;
                };
                listed.push(location);
            }
            Some(listed)
        }
    };
    engine.host_listed(request, entries)
}



/// DEMO documents, registered by name — the demo plugin owns the
/// builders (the crate sits above this one); the door only looks
pub use himark::demo_docs::DemoDocs;




#[no_mangle]
pub unsafe extern "C" fn himark_has_text_focus(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.has_text_focus(bridge.raw))
}

#[no_mangle]
pub unsafe extern "C" fn himark_has_marked_text(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.has_marked_text(bridge.raw))
}

#[no_mangle]
pub unsafe extern "C" fn himark_set_marked_text(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    utf8: *const c_char,
    len: usize,
    selected: HimarkRange,
    has_replacement: bool,
    replacement: HimarkRange,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(text) = str_from(utf8, len) else {
        return false;
    };
    let replacement = has_replacement.then_some(replacement);
    engine.set_marked_text(bridge.raw, text, selected, replacement)
}

#[no_mangle]
pub unsafe extern "C" fn himark_unmark_text(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.unmark_text(bridge.raw))
}

#[no_mangle]
pub unsafe extern "C" fn himark_marked_range(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    out: *mut HimarkRange,
) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.marked_range_raw(bridge.raw, out))
}

#[no_mangle]
pub unsafe extern "C" fn himark_selected_range(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    out: *mut HimarkRange,
) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.selected_range_raw(bridge.raw, out))
}

#[no_mangle]
pub unsafe extern "C" fn himark_set_selected_range(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    range: HimarkRange,
) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.set_selected_range(bridge.raw, range))
}

#[no_mangle]
pub unsafe extern "C" fn himark_document_length(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    out: *mut u32,
) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.document_length_raw(bridge.raw, out))
}

#[no_mangle]
pub unsafe extern "C" fn himark_reveal_selection(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.reveal_selection(bridge.raw))
}

#[no_mangle]
pub unsafe extern "C" fn himark_selection_rects(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    range: HimarkRange,
    out: *mut HimarkRect,
    cap: usize,
) -> usize {
    engine.as_mut().map_or(0, |engine| {
        engine.selection_rects_raw(bridge.raw, range, out, cap)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_first_rect(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    range: HimarkRange,
    out: *mut HimarkRect,
) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.first_rect_raw(bridge.raw, range, out))
}

#[no_mangle]
pub unsafe extern "C" fn himark_char_index_at(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    x: f32,
    y: f32,
) -> i64 {
    let Some(engine) = engine.as_mut() else {
        return -1;
    };
    engine
        .char_index_at(bridge.raw, x, y)
        .map_or(-1, |index| index as i64)
}

#[no_mangle]
pub unsafe extern "C" fn himark_substring(
    engine: *mut HimarkEngine,
    bridge: crate::bridges::HimarkBridge,
    range: HimarkRange,
    out: *mut c_char,
    cap: usize,
) -> usize {
    engine.as_mut().map_or(0, |engine| {
        engine.substring_raw(bridge.raw, range, out, cap)
    })
}

pub unsafe fn str_from<'a>(utf8: *const c_char, len: usize) -> Option<&'a str> {
    if utf8.is_null() {
        return (len == 0).then_some("");
    }
    let bytes = std::slice::from_raw_parts(utf8 as *const u8, len);
    std::str::from_utf8(bytes).ok()
}

pub unsafe fn copy_optional_utf8(text: Option<String>, out: *mut c_char, cap: usize) -> usize {
    let Some(text) = text else {
        return 0;
    };
    let bytes = text.as_bytes();
    if out.is_null() || bytes.len() > cap {
        return bytes.len();
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, out, bytes.len());
    bytes.len()
}

unsafe fn write_optional_out<T: Copy>(value: Option<T>, out: *mut T) -> bool {
    let Some(value) = value else {
        return false;
    };
    if out.is_null() {
        return false;
    }
    *out = value;
    true
}
