// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench shell's OWN C surface — the window-shaped half of
//! the engine door: add-window, the window input and draw roads,
//! the chrome metrics, the form. `himark-ffi` stays window-free;
//! this module rides beside it in the apple staticlib.

use std::sync::Arc;

use std::ffi::c_char;

use gfx::{Canvas, Point, Size};
use himark_ffi::str_from;
use himark_ffi::{
    map_key, map_mods, HimarkEngine, Pointed, HIMARK_FORM_DESKTOP, HIMARK_FORM_PHONE,
};
use imba::event::{Event, MouseButton};

use crate::app::WorkbenchShell as _;
use crate::app_ext::AppExt as _;
use himark::app_ext::AppExt as _;

pub fn wid(window: u64) -> workbench::window::WindowId {
    workbench::window::WindowId::from_raw(window)
}
/// The window half of the C engine door, method syntax preserved —
/// an extension trait over `himark_ffi::HimarkEngine`.
pub trait WindowEngine {
    fn add_window(&mut self) -> u64;
    fn window_size(&self, window: u64) -> Size;
    fn draw(
        &mut self,
        window: u64,
        canvas: &Canvas<'_>,
        width: f32,
        height: f32,
        _scale: f32,
    ) -> bool;
    fn key_down(&mut self, window: u64, key: u32, mods: u32) -> bool;
    fn key_down_at(&mut self, window: u64, key: u32, mods: u32, event_started_at: f64) -> bool;
    fn text_input(&mut self, window: u64, text: &str) -> bool;
    fn text_input_at(&mut self, window: u64, text: &str, event_started_at: f64) -> bool;
    fn mouse_down(&mut self, window: u64, x: f32, y: f32, mods: u32, click_count: u32) -> bool;
    fn mouse_down_at(
        &mut self,
        window: u64,
        x: f32,
        y: f32,
        mods: u32,
        click_count: u32,
        event_started_at: f64,
    ) -> bool;
    fn secondary_down(&mut self, window: u64, x: f32, y: f32, mods: u32) -> bool;
    fn toolbar_height(&self) -> f32;
    fn mouse_drag(&mut self, window: u64, x: f32, y: f32, mods: u32) -> bool;
    fn mouse_move(&mut self, window: u64, x: f32, y: f32, mods: u32) -> Pointed;
    fn modifiers_changed(&mut self, window: u64, x: f32, y: f32, mods: u32) -> Pointed;
    fn mouse_up(&mut self, window: u64, x: f32, y: f32) -> bool;
    fn mouse_left(&mut self, window: u64) -> bool;
    fn perform_command(&mut self, window: u64, id: &str) -> bool;
    fn set_chrome_clearance(&mut self, width: f32);
    fn set_form(&mut self, form: u32) -> bool;
    fn open_document(
        &mut self,
        window: u64,
        name: impl Into<String>,
        source: impl Into<String>,
        primary: bool,
    ) -> bool;
}

impl WindowEngine for HimarkEngine {
    fn add_window(&mut self) -> u64 {
        let window = self.app.add_window();
        if self.compose_new_windows {
            self.app.perform_command(crate::shell::windowed(
                window,
                std::sync::Arc::new(crate::higent::open_session::OpenNewSession { host: None }),
            ));
        }
        window.raw()
    }
    // The committed store is gathered for the PRIMARY window only, so a
    // sibling window's entity is projected out of it — sizes must come
    // from the live window state or every non-primary window hit-tests
    // against a 1x1 layout.
    fn window_size(&self, window: u64) -> Size {
        self.app
            .window_viewport(wid(window))
            .unwrap_or_else(|| Size::new(1.0, 1.0))
    }
    fn draw(
        &mut self,
        window: u64,
        canvas: &Canvas<'_>,
        width: f32,
        height: f32,
        _scale: f32,
    ) -> bool {
        // A shell that draws without ticking (tests, direct renders)
        // still applies the standing deltas before the frame.
        let flushed = self.flush_scroll(window);
        self.app
            .draw_window_sized(wid(window), canvas, Size::new(width, height))
            || flushed
    }
    fn key_down(&mut self, window: u64, key: u32, mods: u32) -> bool {
        self.key_down_at(window, key, mods, 0.0)
    }
    fn key_down_at(&mut self, window: u64, key: u32, mods: u32, event_started_at: f64) -> bool {
        self.flush_scroll(window);
        match map_key(key) {
            Some(key) => {
                let size = self.window_size(window);
                self.app.dispatch_timed(
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
    fn text_input(&mut self, window: u64, text: &str) -> bool {
        self.text_input_at(window, text, 0.0)
    }
    fn text_input_at(&mut self, window: u64, text: &str, event_started_at: f64) -> bool {
        if text.is_empty() {
            return false;
        }
        self.flush_scroll(window);
        let size = self.window_size(window);
        self.app.dispatch_timed(
            wid(window),
            Event::TextInput { text },
            size,
            event_started_at,
        )
    }
    fn mouse_down(&mut self, window: u64, x: f32, y: f32, mods: u32, click_count: u32) -> bool {
        self.mouse_down_at(window, x, y, mods, click_count, 0.0)
    }
    fn mouse_down_at(
        &mut self,
        window: u64,
        x: f32,
        y: f32,
        mods: u32,
        click_count: u32,
        event_started_at: f64,
    ) -> bool {
        self.flush_scroll(window);
        let count = match click_count {
            0 => self.clicks.count(x, y),
            real => real.min(3) as u8,
        };
        let size = self.window_size(window);
        self.app.dispatch_timed(
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
    fn secondary_down(&mut self, window: u64, x: f32, y: f32, mods: u32) -> bool {
        self.flush_scroll(window);
        let size = self.window_size(window);
        self.app.dispatch_timed(
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
    fn toolbar_height(&self) -> f32 {
        editor::env::Themes::of(self.app.store())
            .part(::hikit::chrome::ToolbarChrome::KEY)
            .height
    }
    fn mouse_drag(&mut self, window: u64, x: f32, y: f32, mods: u32) -> bool {
        let size = self.window_size(window);
        self.app.dispatch(
            wid(window),
            Event::MouseDrag {
                point: Point::new(x, y),
                mods: map_mods(mods),
            },
            size,
        )
    }
    fn mouse_move(&mut self, window: u64, x: f32, y: f32, mods: u32) -> Pointed {
        self.flush_scroll(window);
        let size = self.window_size(window);
        let (changed, shape) = self.app.dispatch_pointed(
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
    fn modifiers_changed(&mut self, window: u64, x: f32, y: f32, mods: u32) -> Pointed {
        self.mouse_move(window, x, y, mods)
    }
    fn mouse_up(&mut self, window: u64, x: f32, y: f32) -> bool {
        self.flush_scroll(window);
        let size = self.window_size(window);
        self.app.dispatch(
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
    fn mouse_left(&mut self, window: u64) -> bool {
        let size = self.window_size(window);
        self.app.dispatch(wid(window), Event::window_left(), size)
    }
    fn perform_command(&mut self, window: u64, id: &str) -> bool {
        self.app.perform_registered(wid(window), id)
    }
    fn set_chrome_clearance(&mut self, width: f32) {
        self.app.set_chrome_clearance(width);
    }
    /// The shell's form, set once at boot (`HIMARK_FORM_*`): the
    /// appearance commands pick the form's theme pair from here on,
    /// and the standing appearance re-applies in the new pair — through
    /// the registered command, so the change propagates like any
    /// theme switch. Answers whether the form was known.
    fn set_form(&mut self, form: u32) -> bool {
        let form = match form {
            HIMARK_FORM_DESKTOP => crate::builtins::Form::Desktop,
            HIMARK_FORM_PHONE => crate::builtins::Form::Phone,
            other => {
                eprintln!("[ffi] form {other} is not one this engine knows — refused");
                return false;
            }
        };
        if crate::builtins::Form::of(self.app.store()) == form {
            return true;
        }
        self.app
            .setup(|store| crate::builtins::Form::set(store, form));
        let dark = crate::builtins::theme_is_dark(self.app.store());
        self.perform_command(0, if dark { "theme.dark" } else { "theme.light" })
    }
    fn open_document(
        &mut self,
        window: u64,
        name: impl Into<String>,
        source: impl Into<String>,
        primary: bool,
    ) -> bool {
        let name = name.into();
        let source = source.into();
        let document_name = name.clone();
        let languages = self.languages();
        self.app
            .open_async(wid(window), name, primary, None, move |store, ui, theme| {
                ahp_chat::open::document_for(&languages, &document_name, &source, store, ui, theme)
            });
        true
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_add_window(engine: *mut HimarkEngine) -> himark_ffi::bridges::HimarkWindow {
    himark_ffi::bridges::HimarkWindow {
        raw: engine.as_mut().map_or(0, HimarkEngine::add_window),
    }
}
#[no_mangle]
pub unsafe extern "C" fn himark_toolbar_height(engine: *mut HimarkEngine) -> f32 {
    engine
        .as_mut()
        .map_or(0.0, |engine| engine.toolbar_height())
}
#[no_mangle]
pub unsafe extern "C" fn himark_perform_command(
    engine: *mut HimarkEngine,
    bridge: himark_ffi::bridges::HimarkBridge,
    id: *const c_char,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    if id.is_null() {
        return false;
    }
    let Ok(id) = std::ffi::CStr::from_ptr(id).to_str() else {
        return false;
    };
    engine.perform_command(bridge.raw, id)
}
#[no_mangle]
pub unsafe extern "C" fn himark_set_chrome_clearance(engine: *mut HimarkEngine, width: f32) {
    if let Some(engine) = engine.as_mut() {
        engine.set_chrome_clearance(width);
    }
}
/// Tell the engine what it runs in — once, at boot, before the first
/// paint. The appearance commands (`theme.dark`, `theme.light`,
/// `theme.toggle`) pick the form's pair from then on; the standing
/// appearance re-applies in the new pair at once. Answers false for
/// an unknown form.
#[no_mangle]
pub unsafe extern "C" fn himark_set_form(engine: *mut HimarkEngine, form: u32) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.set_form(form))
}
#[no_mangle]
pub unsafe extern "C" fn himark_open_document(
    engine: *mut HimarkEngine,
    bridge: himark_ffi::bridges::HimarkBridge,
    name: *const c_char,
    name_len: usize,
    utf8: *const c_char,
    len: usize,
    primary: bool,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(name) = str_from(name, name_len) else {
        return false;
    };
    let Some(source) = str_from(utf8, len) else {
        return false;
    };
    engine.open_document(bridge.raw, name, source, primary)
}

/// Open a registered demo document into the bridge — the debug
/// harness road.
#[no_mangle]
pub unsafe extern "C" fn himark_open_demo(
    engine: *mut HimarkEngine,
    bridge: himark_ffi::bridges::HimarkBridge,
    name: *const c_char,
    len: usize,
) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    let Some(name) = str_from(name, len) else {
        return;
    };
    let Some((location, build)) = engine
        .app
        .store()
        .get::<himark::demo_docs::DemoDocs>()
        .and_then(|docs| docs.0.get(name).cloned())
    else {
        eprintln!("[himark] no demo document registered as '{name}'");
        return;
    };
    use crate::app_ext::AppExt as _;
    engine.app.open_async(
        wid(bridge.raw),
        name.to_owned(),
        true,
        Some(location),
        move |store, ui, theme| build(store, ui, theme),
    );
}

/// Seat the capability-gated chrome: what `himark_set_host`
/// registers when the host callbacks carry pickers or a clipboard.
pub fn seat_host_chrome(app: &mut himark::app::Himark) {
    app.setup(|store| {
        store.put(himark_ffi::host::HostChrome {
            on_pick_files: Some(Arc::new(|app| {
                use himark::app_ext::AppExt as _;
                app.register_command(Arc::new(crate::pickers::OpenFilePicker));
            })),
            on_clipboard: Some(Arc::new(|app| {
                use himark::app_ext::AppExt as _;
                app.register_command(Arc::new(crate::higent::drawer::ShareHost));
                app.register_command(Arc::new(crate::higent::drawer::ShareHostTailnet));
            })),
        })
    });
}
