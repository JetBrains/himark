// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench shell's OWN C surface — the window-shaped half of
//! the engine door: add-window, the window input and draw roads,
//! the chrome metrics, the form. `himark-ffi` stays window-free;
//! this module rides beside it in the apple staticlib.

use std::sync::Arc;


use gfx::{Canvas, Point, Size};
use himark_ffi::{map_key, map_mods, HimarkEngine, Pointed};
use imba::event::{Event, MouseButton};

use crate::app::WorkbenchShell as _;
use crate::app_ext::AppExt as _;

use himark::bridge::ViewBridgeId;

/// The window AS a bridge is the id pun, and it lives HERE alone:
/// the engine door speaks `ViewBridgeId`, this module converts to
/// the workbench's `WindowId` where the window roads need it.
pub fn wid(window: ViewBridgeId) -> workbench::window::WindowId {
    workbench::window::WindowId::from_raw(window.raw())
}
/// The window half of the C engine door, method syntax preserved —
/// an extension trait over `himark_ffi::HimarkEngine`.
pub trait WindowEngine {
    fn open_window(&mut self) -> ViewBridgeId;
    fn window_size(&self, window: ViewBridgeId) -> Size;
    fn draw(
        &mut self,
        window: ViewBridgeId,
        canvas: &Canvas<'_>,
        width: f32,
        height: f32,
        _scale: f32,
    ) -> bool;
    fn key_down(&mut self, window: ViewBridgeId, key: u32, mods: u32) -> bool;
    fn key_down_at(&mut self, window: ViewBridgeId, key: u32, mods: u32, event_started_at: f64) -> bool;
    fn text_input(&mut self, window: ViewBridgeId, text: &str) -> bool;
    fn text_input_at(&mut self, window: ViewBridgeId, text: &str, event_started_at: f64) -> bool;
    fn mouse_down(&mut self, window: ViewBridgeId, x: f32, y: f32, mods: u32, click_count: u32) -> bool;
    fn mouse_down_at(
        &mut self,
        window: ViewBridgeId,
        x: f32,
        y: f32,
        mods: u32,
        click_count: u32,
        event_started_at: f64,
    ) -> bool;
    fn secondary_down(&mut self, window: ViewBridgeId, x: f32, y: f32, mods: u32) -> bool;
    fn mouse_drag(&mut self, window: ViewBridgeId, x: f32, y: f32, mods: u32) -> bool;
    fn mouse_move(&mut self, window: ViewBridgeId, x: f32, y: f32, mods: u32) -> Pointed;
    fn modifiers_changed(&mut self, window: ViewBridgeId, x: f32, y: f32, mods: u32) -> Pointed;
    fn mouse_up(&mut self, window: ViewBridgeId, x: f32, y: f32) -> bool;
    fn mouse_left(&mut self, window: ViewBridgeId) -> bool;
    /// TEST SUPPORT and the gpui demo road — no C door anymore.
    fn open_document(
        &mut self,
        window: ViewBridgeId,
        name: impl Into<String>,
        source: impl Into<String>,
        primary: bool,
    ) -> bool;
}

/// A fresh window COMPOSES a session when the shell says so — the
/// production boot turns it on; tests leave it off. A store
/// resident, like every shell flag.
#[derive(Clone, Copy, Default)]
pub struct ComposeNewWindows(pub bool);

impl WindowEngine for HimarkEngine {
    fn open_window(&mut self) -> ViewBridgeId {
        let window = crate::app::WorkbenchShell::add_window(self);
        let compose = self
            .store()
            .get::<ComposeNewWindows>()
            .copied()
            .unwrap_or_default()
            .0;
        if compose {
            himark::app::Himark::perform_command(
                self,
                crate::shell::windowed(
                    window,
                    std::sync::Arc::new(crate::higent::open_session::OpenNewSession { host: None }),
                ),
            );
        }
        ViewBridgeId::from_raw(window.raw())
    }
    // The committed store is gathered for the PRIMARY window only, so a
    // sibling window's entity is projected out of it — sizes must come
    // from the live window state or every non-primary window hit-tests
    // against a 1x1 layout.
    fn window_size(&self, window: ViewBridgeId) -> Size {
        self
            .window_viewport(wid(window))
            .unwrap_or_else(|| Size::new(1.0, 1.0))
    }
    fn draw(
        &mut self,
        window: ViewBridgeId,
        canvas: &Canvas<'_>,
        width: f32,
        height: f32,
        _scale: f32,
    ) -> bool {
        // A shell that draws without ticking (tests, direct renders)
        // still applies the standing deltas before the frame.
        let flushed = self.flush_scroll(window);
        self
            .draw_window_sized(wid(window), canvas, Size::new(width, height))
            || flushed
    }
    fn key_down(&mut self, window: ViewBridgeId, key: u32, mods: u32) -> bool {
        self.key_down_at(window, key, mods, 0.0)
    }
    fn key_down_at(&mut self, window: ViewBridgeId, key: u32, mods: u32, event_started_at: f64) -> bool {
        self.flush_scroll(window);
        match map_key(key) {
            Some(key) => {
                let size = self.window_size(window);
                self.dispatch_timed(
                    wid(window),
                    Event::KeyDown {
                        key,
                        mods: map_mods(mods),
                    },
                    size,
                    event_started_at,
                )
            }
            None => false,
        }
    }
    fn text_input(&mut self, window: ViewBridgeId, text: &str) -> bool {
        self.text_input_at(window, text, 0.0)
    }
    fn text_input_at(&mut self, window: ViewBridgeId, text: &str, event_started_at: f64) -> bool {
        if text.is_empty() {
            return false;
        }
        self.flush_scroll(window);
        let size = self.window_size(window);
        self.dispatch_timed(
            wid(window),
            Event::TextInput { text },
            size,
            event_started_at,
        )
    }
    fn mouse_down(&mut self, window: ViewBridgeId, x: f32, y: f32, mods: u32, click_count: u32) -> bool {
        self.mouse_down_at(window, x, y, mods, click_count, 0.0)
    }
    fn mouse_down_at(
        &mut self,
        window: ViewBridgeId,
        x: f32,
        y: f32,
        mods: u32,
        click_count: u32,
        event_started_at: f64,
    ) -> bool {
        self.flush_scroll(window);
        let count = match click_count {
            0 => himark_ffi::ffi_seat(self.store())
                .clicks
                .lock()
                .expect("the click counter")
                .count(x, y),
            real => real.min(3) as u8,
        };
        let size = self.window_size(window);
        self.dispatch_timed(
            wid(window),
            Event::MouseDown {
                point: Point::new(x, y),
                button: MouseButton::Left,
                mods: map_mods(mods),
                count,
            },
            size,
            event_started_at,
        )
    }
    /// The secondary (context) press. Shells that can tell buttons
    /// apart route their right-button downs here; the others reach
    /// the same rows via control-click.
    fn secondary_down(&mut self, window: ViewBridgeId, x: f32, y: f32, mods: u32) -> bool {
        self.flush_scroll(window);
        let size = self.window_size(window);
        self.dispatch_timed(
            wid(window),
            Event::MouseDown {
                point: Point::new(x, y),
                button: MouseButton::Right,
                mods: map_mods(mods),
                count: 1,
            },
            size,
            0.0,
        )
    }
    fn mouse_drag(&mut self, window: ViewBridgeId, x: f32, y: f32, mods: u32) -> bool {
        let size = self.window_size(window);
        self.dispatch(
            wid(window),
            Event::MouseDrag {
                point: Point::new(x, y),
                mods: map_mods(mods),
            },
            size,
        )
    }
    fn mouse_move(&mut self, window: ViewBridgeId, x: f32, y: f32, mods: u32) -> Pointed {
        self.flush_scroll(window);
        let size = self.window_size(window);
        let (changed, shape) = self.dispatch_pointed(
            wid(window),
            Event::MouseMove {
                point: Point::new(x, y),
                mods: map_mods(mods),
            },
            size,
        );
        Pointed { changed, shape }
    }
    /// The shell's modifiers report (macOS `flagsChanged`, winit
    /// `ModifiersChanged`): a mouse move at the standing pointer's
    /// position, so modifier-gated hover looks (the cmd-hover link
    /// underline) re-hit-test without pointer motion.
    fn modifiers_changed(&mut self, window: ViewBridgeId, x: f32, y: f32, mods: u32) -> Pointed {
        self.mouse_move(window, x, y, mods)
    }
    fn mouse_up(&mut self, window: ViewBridgeId, x: f32, y: f32) -> bool {
        self.flush_scroll(window);
        let size = self.window_size(window);
        self.dispatch(
            wid(window),
            Event::MouseUp {
                point: Point::new(x, y),
            },
            size,
        )
    }
    /// The cursor left the window: a HitTest beyond any component's
    /// reach (missing, far off-screen), so hover state — tooltips,
    /// hover popups — lets go instead of sticking to the last point
    /// the window ever heard about.
    fn mouse_left(&mut self, window: ViewBridgeId) -> bool {
        let size = self.window_size(window);
        self.dispatch(wid(window), Event::window_left(), size)
    }
    fn open_document(
        &mut self,
        window: ViewBridgeId,
        name: impl Into<String>,
        source: impl Into<String>,
        primary: bool,
    ) -> bool {
        let name = name.into();
        let source = source.into();
        let document_name = name.clone();
        let languages = ::editor::env::Parsers::of(self.store())
            .expect("the plugin pass registered the languages");
        self
            .open_async(wid(window), name, primary, None, move |store, ui, theme| {
                ahp_chat::open::document_for(&languages, &document_name, &source, store, ui, theme)
            });
        true
    }
}






/// Seat the capability-gated chrome: what `himark_set_host`
/// registers when the host callbacks carry pickers or a clipboard.
pub fn seat_host_chrome(app: &mut himark::app::Himark) {
    app.with_store(|store| {
        store.put(himark_ffi::host::HostChrome {
            on_pick_folders: None,
            on_share: None,
            on_agent_filesystem: None,
            on_pick_files: Some(Arc::new(|app| {
                                app.register_command(Arc::new(crate::pickers::OpenFilePicker));
            })),
            on_clipboard: Some(Arc::new(|app| {
                                app.register_command(Arc::new(crate::higent::drawer::ShareHost));
                app.register_command(Arc::new(crate::higent::drawer::ShareHostTailnet));
            })),
        })
    });
}

/// The handle NEWTYPES: single-field structs, so C and Swift see
/// DISTINCT types — an event where a layout belongs is a compile
/// error, not a mixed-up integer. The ABI of a one-u64 struct is
/// the u64 itself; `raw` is 0 for a refusal.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkWindow {
    pub raw: u64,
}

/// The window AS a bridge — the degenerate case named. Today the
/// identity is direct; the handle is the only currency the
/// normalized entries below speak.
#[no_mangle]
pub unsafe extern "C" fn himark_window_bridge(
    engine: *mut HimarkEngine,
    window: HimarkWindow,
) -> himark_ffi::bridges::HimarkBridge {
    if engine.is_null() {
        himark_ffi::bridges::refuse("engine for window bridge", window.raw);
        return himark_ffi::bridges::HimarkBridge { raw: 0 };
    }
    himark_ffi::bridges::HimarkBridge { raw: window.raw }
}

#[no_mangle]
pub unsafe extern "C" fn himark_add_window(engine: *mut HimarkEngine) -> HimarkWindow {
    HimarkWindow {
        raw: engine
            .as_mut()
            .map_or(0, |engine| WindowEngine::open_window(engine).raw()),
    }
}

