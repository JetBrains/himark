// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The EMULATOR: libghostty-vt fed with the session's bytes, read back
//! as frames for the pane to draw, and asked to encode keys and mouse
//! input in whatever mode the program negotiated. One `Vt` is one
//! terminal; the collection's `Session` serializes every call behind
//! its mutex.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ghostty_sys::*;
use skia_safe::Color;

/// The terminal's colors, from the theme part. The palette's 16 seats
/// override libghostty's defaults; 16..255 stay the standard cube.
#[derive(Clone, PartialEq)]
pub struct Colors {
    pub fg: Color,
    pub bg: Color,
    pub cursor: Color,
    pub selection: Color,
    pub palette16: [Color; 16],
}

impl Colors {
    pub fn from_chrome(chrome: &crate::chrome::TerminalChrome) -> Self {
        let mut palette16 = [chrome.foreground.0; 16];
        for (seat, color) in palette16.iter_mut().zip(&chrome.palette) {
            *seat = color.0;
        }
        Self {
            fg: chrome.foreground.0,
            bg: chrome.background.0,
            cursor: chrome.cursor.0,
            selection: chrome.selection.0,
            palette16,
        }
    }
}

impl Default for Colors {
    fn default() -> Self {
        Self::from_chrome(&crate::chrome::TerminalChrome::default())
    }
}

fn rgb(color: Color) -> GhosttyColorRgb {
    GhosttyColorRgb {
        r: color.r(),
        g: color.g(),
        b: color.b(),
    }
}

fn color(rgb: GhosttyColorRgb) -> Color {
    Color::from_argb(0xff, rgb.r, rgb.g, rgb.b)
}

/// One cell as the pane draws it.
#[derive(Clone, Default, PartialEq)]
pub struct Cell {
    /// Empty for blank cells and for the tail half of a wide character.
    pub text: String,
    pub fg: Color,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub underline: bool,
    pub strike: bool,
    pub wide: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
    Hollow,
}

#[derive(Clone, Copy)]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    pub shape: CursorShape,
    pub color: Color,
}

pub struct Frame {
    /// A row that didn't change since the last frame is the same `Arc`.
    pub rows: Vec<Arc<Vec<Cell>>>,
    pub cursor: Option<Cursor>,
    pub bg: Color,
}

/// Holds what the terminal's callbacks write into; boxed so its address is stable.
struct Shared {
    state: GhosttyRenderState,
    held_since: Option<Instant>,
    title_changed: bool,
    /// Replies the terminal writes back to the program (cursor reports, size reports).
    replies: Vec<u8>,
}

unsafe extern "C" fn on_write_pty(
    _t: GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    let shared = unsafe { &mut *(userdata as *mut Shared) };
    if !data.is_null() && len > 0 {
        shared
            .replies
            .extend_from_slice(unsafe { std::slice::from_raw_parts(data, len) });
    }
}

unsafe extern "C" fn on_render_hold(_t: GhosttyTerminal, userdata: *mut c_void, held: bool) {
    let shared = unsafe { &mut *(userdata as *mut Shared) };
    if held {
        // Capture the finished frame the program wants on screen before it redraws.
        unsafe { ghostty_render_state_update(shared.state, _t) };
        shared.held_since = Some(Instant::now());
    } else {
        shared.held_since = None;
    }
}

unsafe extern "C" fn on_title(_t: GhosttyTerminal, userdata: *mut c_void) {
    let shared = unsafe { &mut *(userdata as *mut Shared) };
    shared.title_changed = true;
}

const SYNC_OUTPUT: GhosttyMode = 2026;
const BRACKETED_PASTE: GhosttyMode = 2004;
const MAX_HOLD: Duration = Duration::from_millis(1000);
const SCROLLBACK_MAX_LINES: usize = 10_000;
// Bound memory per terminal; wide or heavily styled rows may hit this first.
const SCROLLBACK_MAX_BYTES: usize = 10_000_000;
/// Units per pixel for selection gestures (see `select`).
const SUBPIXEL: f32 = 64.0;

pub struct Vt {
    term: GhosttyTerminal,
    shared: Box<Shared>,
    rows_it: GhosttyRenderStateRowIterator,
    cells: GhosttyRenderStateRowCells,
    keys: GhosttyKeyEncoder,
    key_event: GhosttyKeyEvent,
    mouse: GhosttyMouseEncoder,
    mouse_event: GhosttyMouseEvent,
    last_mouse_report: Vec<u8>,
    pub cols: u16,
    pub rows: u16,
    colors: Colors,
    /// The built-in 256-color palette, which the theme's 16 colors override.
    base_palette: [GhosttyColorRgb; 256],
    /// Ghostty's own click/double-click/triple-click/drag selection logic.
    gesture: GhosttySelectionGesture,
    press: GhosttySelectionGestureEvent,
    drag: GhosttySelectionGestureEvent,
    release: GhosttySelectionGestureEvent,
    /// The rows read last frame by libghostty's row id, with their selection:
    /// one that isn't dirty is the same row and isn't read again.
    seen: HashMap<[u64; 2], SeenRow>,
    /// The width and colors they were read with: a change reads every row.
    seen_with: (u16, u64),
}

/// The raw handles answer to no thread; the owning `Session` serializes
/// every call behind its mutex, and frames cross threads as `Arc` rows.
unsafe impl Send for Vt {}

/// A row as read last frame, with the selection span it was read under.
type SeenRow = (Arc<Vec<Cell>>, Option<(u16, u16)>);

#[derive(Clone, Copy, Debug)]
pub enum Select {
    Press,
    Drag,
    Release,
}

fn ok(r: GhosttyResult, what: &str) {
    assert!(r == GHOSTTY_SUCCESS, "{what} failed: {r}");
}

impl Vt {
    pub fn new(cols: u16, rows: u16, colors: Colors) -> Vt {
        unsafe {
            let mut term = std::ptr::null_mut();
            ok(
                ghostty_terminal_new(std::ptr::null(), &mut term, cols.max(1), rows.max(1)),
                "terminal_new",
            );
            let mut state = std::ptr::null_mut();
            ok(
                ghostty_render_state_new(std::ptr::null(), &mut state),
                "render_state_new",
            );
            let mut rows_it = std::ptr::null_mut();
            ok(
                ghostty_render_state_row_iterator_new(std::ptr::null(), &mut rows_it),
                "row_iterator_new",
            );
            let mut cells = std::ptr::null_mut();
            ok(
                ghostty_render_state_row_cells_new(std::ptr::null(), &mut cells),
                "row_cells_new",
            );
            let mut keys = std::ptr::null_mut();
            ok(
                ghostty_key_encoder_new(std::ptr::null(), &mut keys),
                "key_encoder_new",
            );
            let mut key_event = std::ptr::null_mut();
            ok(
                ghostty_key_event_new(std::ptr::null(), &mut key_event),
                "key_event_new",
            );
            let mut mouse = std::ptr::null_mut();
            ok(
                ghostty_mouse_encoder_new(std::ptr::null(), &mut mouse),
                "mouse_encoder_new",
            );
            let mut mouse_event = std::ptr::null_mut();
            ok(
                ghostty_mouse_event_new(std::ptr::null(), &mut mouse_event),
                "mouse_event_new",
            );

            let mut shared = Box::new(Shared {
                state,
                held_since: None,
                title_changed: false,
                replies: Vec::new(),
            });
            let ud = &mut *shared as *mut Shared as *const c_void;
            ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_USERDATA, ud);
            let hold: GhosttyTerminalRenderHoldFn = Some(on_render_hold);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_RENDER_HOLD,
                std::mem::transmute::<GhosttyTerminalRenderHoldFn, *const c_void>(hold),
            );
            let title: GhosttyTerminalTitleChangedFn = Some(on_title);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
                std::mem::transmute::<GhosttyTerminalTitleChangedFn, *const c_void>(title),
            );
            let write: GhosttyTerminalWritePtyFn = Some(on_write_pty);
            ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                std::mem::transmute::<GhosttyTerminalWritePtyFn, *const c_void>(write),
            );

            ghostty_render_state_update(state, term);
            let mut colors_data = GhosttyRenderStateColors {
                size: std::mem::size_of::<GhosttyRenderStateColors>(),
                ..Default::default()
            };
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut colors_data as *mut _ as *mut c_void,
            );

            ok(
                ghostty_terminal_set(
                    term,
                    GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                    &SCROLLBACK_MAX_BYTES as *const _ as *const c_void,
                ),
                "scrollback_max_bytes",
            );
            ok(
                ghostty_terminal_set(
                    term,
                    GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
                    &SCROLLBACK_MAX_LINES as *const _ as *const c_void,
                ),
                "scrollback_max_lines",
            );

            let mut gesture = std::ptr::null_mut();
            ok(
                ghostty_selection_gesture_new(std::ptr::null(), &mut gesture),
                "selection_gesture_new",
            );
            let ev = |t| {
                let mut e = std::ptr::null_mut();
                ok(
                    ghostty_selection_gesture_event_new(std::ptr::null(), &mut e, t),
                    "gesture_event_new",
                );
                e
            };
            let (press, drag, release) = (
                ev(GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS),
                ev(GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG),
                ev(GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE),
            );
            let mut vt = Vt {
                term,
                shared,
                rows_it,
                cells,
                keys,
                key_event,
                mouse,
                mouse_event,
                last_mouse_report: Vec::new(),
                cols,
                rows,
                colors,
                base_palette: colors_data.palette,
                gesture,
                press,
                drag,
                release,
                seen: HashMap::new(),
                seen_with: (0, 0),
            };
            vt.apply_colors();
            vt
        }
    }

    pub fn colors(&self) -> &Colors {
        &self.colors
    }

    /// Switches colors live (the theme changed).
    pub fn set_colors(&mut self, colors: Colors) {
        if self.colors != colors {
            self.colors = colors;
            self.apply_colors();
            // Rows carry the theme's own colors too (the selection's).
            self.seen.clear();
        }
    }

    fn apply_colors(&mut self) {
        let t = &self.colors;
        let (fg, bg, cur) = (rgb(t.fg), rgb(t.bg), rgb(t.cursor));
        let mut palette = self.base_palette;
        for (seat, color) in palette.iter_mut().zip(&t.palette16) {
            *seat = rgb(*color);
        }
        unsafe {
            ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
                &fg as *const _ as *const c_void,
            );
            ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
                &bg as *const _ as *const c_void,
            );
            ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
                &cur as *const _ as *const c_void,
            );
            ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                &palette as *const _ as *const c_void,
            );
        }
    }

    /// What the terminal wants to say back to the program since the last call.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.shared.replies)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        unsafe { ghostty_terminal_vt_write(self.term, bytes.as_ptr(), bytes.len()) };
    }

    pub fn resize(&mut self, cols: u16, rows: u16, cell_w: u32, cell_h: u32) {
        if cols == 0 || rows == 0 {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        unsafe { ghostty_terminal_resize(self.term, cols, rows, cell_w.max(1), cell_h.max(1)) };
    }

    pub fn take_title(&mut self) -> Option<String> {
        if !std::mem::take(&mut self.shared.title_changed) {
            return None;
        }
        let mut s = GhosttyString::default();
        unsafe {
            ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_TITLE,
                &mut s as *mut _ as *mut c_void,
            )
        };
        if s.ptr.is_null() {
            return Some(String::new());
        }
        let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    fn get_bool(&self, data: GhosttyTerminalData) -> bool {
        let mut v = false;
        unsafe { ghostty_terminal_get(self.term, data, &mut v as *mut _ as *mut c_void) };
        v
    }

    fn mode(&self, mode: GhosttyMode) -> bool {
        let mut cfg = GhosttyTerminalModeConfig { mode, value: false };
        unsafe {
            ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_MODE,
                &mut cfg as *mut _ as *mut c_void,
            )
        };
        cfg.value
    }

    pub fn mouse_tracking(&self) -> bool {
        self.get_bool(GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING)
    }

    pub fn alt_screen(&self) -> bool {
        let mut screen: GhosttyTerminalScreen = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        unsafe {
            ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                &mut screen as *mut _ as *mut c_void,
            )
        };
        screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE
    }

    pub fn scroll_viewport(&mut self, delta: isize) {
        let mut s = GhosttyTerminalScrollViewport {
            tag: GHOSTTY_SCROLL_VIEWPORT_DELTA,
            ..Default::default()
        };
        s.value.delta = delta;
        unsafe { ghostty_terminal_scroll_viewport(self.term, s) };
    }

    pub fn scroll_to_bottom(&mut self) {
        let s = GhosttyTerminalScrollViewport {
            tag: GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
            ..Default::default()
        };
        unsafe { ghostty_terminal_scroll_viewport(self.term, s) };
    }

    /// Wraps pasted text the way the program asked (bracketed paste or not).
    pub fn paste_bytes(&self, text: &str) -> Vec<u8> {
        if self.mode(BRACKETED_PASTE) {
            let clean = text.replace("\x1b[201~", "");
            [b"\x1b[200~".as_slice(), clean.as_bytes(), b"\x1b[201~"].concat()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        }
    }

    /// Feeds one mouse step of a selection gesture (cell + pixel position in the grid).
    /// Applies the resulting selection; a plain click clears it.
    #[allow(clippy::too_many_arguments)]
    pub fn select(
        &mut self,
        step: Select,
        col: u16,
        row: u16,
        x: f32,
        y: f32,
        cell_w: f32,
        height: f32,
    ) {
        unsafe {
            let mut r = GhosttyGridRef {
                size: std::mem::size_of::<GhosttyGridRef>(),
                ..Default::default()
            };
            let mut p = GhosttyPoint {
                tag: GHOSTTY_POINT_TAG_VIEWPORT,
                ..Default::default()
            };
            p.value.coordinate = GhosttyPointCoordinate {
                x: col.min(self.cols.saturating_sub(1)),
                y: row.min(self.rows.saturating_sub(1)) as u32,
            };
            if ghostty_terminal_grid_ref(self.term, p, &mut r) != GHOSTTY_SUCCESS {
                return;
            }
            // Ghostty takes the cell width as a whole number: at zoom a cell is
            // 9.75 px, and 9 put "the middle of a cell" in the wrong place, so
            // the highlight jumped between cells while dragging. Positions and
            // geometry go in 1/64 px, where the width stays exact.
            let pos = GhosttySurfacePosition {
                x: (x * SUBPIXEL) as f64,
                y: (y * SUBPIXEL) as f64,
            };
            let e = match step {
                Select::Press => self.press,
                Select::Drag => self.drag,
                Select::Release => self.release,
            };
            ghostty_selection_gesture_event_set(
                e,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                &r as *const _ as *const c_void,
            );
            ghostty_selection_gesture_event_set(
                e,
                GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION,
                &pos as *const _ as *const c_void,
            );
            match step {
                Select::Press => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0);
                    ghostty_selection_gesture_event_set(
                        e,
                        GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS,
                        &now as *const _ as *const c_void,
                    );
                    let interval = 500_000_000u64;
                    let distance = f64::from(4.0 * SUBPIXEL);
                    ghostty_selection_gesture_event_set(
                        e,
                        GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_INTERVAL_NS,
                        &interval as *const _ as *const c_void,
                    );
                    ghostty_selection_gesture_event_set(
                        e,
                        GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_DISTANCE,
                        &distance as *const _ as *const c_void,
                    );
                }
                Select::Drag => {
                    let g = GhosttySelectionGestureGeometry {
                        columns: self.cols as u32,
                        cell_width: (cell_w.max(1.0) * SUBPIXEL).round() as u32,
                        padding_left: 0,
                        screen_height: (height.max(1.0) * SUBPIXEL).round() as u32,
                    };
                    ghostty_selection_gesture_event_set(
                        e,
                        GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY,
                        &g as *const _ as *const c_void,
                    );
                }
                Select::Release => {}
            }
            let mut sel = GhosttySelection {
                size: std::mem::size_of::<GhosttySelection>(),
                ..Default::default()
            };
            match ghostty_selection_gesture_event(self.gesture, self.term, e, &mut sel) {
                GHOSTTY_SUCCESS => {
                    ghostty_terminal_set(
                        self.term,
                        GHOSTTY_TERMINAL_OPT_SELECTION,
                        &sel as *const _ as *const c_void,
                    );
                }
                _ if matches!(step, Select::Press) => {
                    ghostty_terminal_set(
                        self.term,
                        GHOSTTY_TERMINAL_OPT_SELECTION,
                        std::ptr::null(),
                    );
                }
                _ => {}
            }
        }
    }

    /// The selected text, if anything is selected.
    pub fn selection_text(&self) -> Option<String> {
        unsafe {
            let opts = GhosttyTerminalSelectionFormatOptions {
                size: std::mem::size_of::<GhosttyTerminalSelectionFormatOptions>(),
                emit: GHOSTTY_FORMATTER_FORMAT_PLAIN,
                unwrap: true,
                trim: true,
                ..Default::default()
            };
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len = 0usize;
            if ghostty_terminal_selection_format_alloc(
                self.term,
                std::ptr::null(),
                opts,
                &mut ptr,
                &mut len,
            ) != GHOSTTY_SUCCESS
            {
                return None;
            }
            if ptr.is_null() {
                return None;
            }
            let text = String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned();
            ghostty_free(std::ptr::null(), ptr, len);
            (!text.is_empty()).then_some(text)
        }
    }

    pub fn clear_selection(&mut self) {
        unsafe {
            ghostty_terminal_set(self.term, GHOSTTY_TERMINAL_OPT_SELECTION, std::ptr::null())
        };
    }

    /// A hash of the live colors: OSC 4/10/11 recolor rows without
    /// marking them dirty, so frames compare against it.
    unsafe fn colors_hash(&self) -> u64 {
        unsafe {
            let mut c = GhosttyRenderStateColors {
                size: std::mem::size_of::<GhosttyRenderStateColors>(),
                ..Default::default()
            };
            ghostty_render_state_get(
                self.shared.state,
                GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut c as *mut _ as *mut c_void,
            );
            let mut h = std::hash::DefaultHasher::new();
            let mix = |h: &mut std::hash::DefaultHasher, x: GhosttyColorRgb| {
                std::hash::Hash::hash(&(x.r, x.g, x.b), h)
            };
            for x in [c.background, c.foreground]
                .into_iter()
                .chain(c.palette)
                .chain(c.cursor_has_value.then_some(c.cursor))
            {
                mix(&mut h, x);
            }
            std::hash::Hasher::finish(&h)
        }
    }

    /// Updates the render state (respecting synchronized output) and reads one frame.
    pub fn frame(&mut self) -> Frame {
        unsafe {
            if let Some(since) = self.shared.held_since {
                if since.elapsed() > MAX_HOLD {
                    let cfg = GhosttyTerminalModeConfig {
                        mode: SYNC_OUTPUT,
                        value: false,
                    };
                    ghostty_terminal_set(
                        self.term,
                        GHOSTTY_TERMINAL_OPT_MODE,
                        &cfg as *const _ as *const c_void,
                    );
                    self.shared.held_since = None;
                }
            }
            let state = self.shared.state;
            if self.shared.held_since.is_none() {
                ghostty_render_state_update(state, self.term);
            }

            let mut colors = GhosttyRenderStateColors {
                size: std::mem::size_of::<GhosttyRenderStateColors>(),
                ..Default::default()
            };
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut colors as *mut _ as *mut c_void,
            );
            let fg_default = color(colors.foreground);
            let bg_default = color(colors.background);

            // Rows libghostty keeps clean are taken as read last frame (see
            // "Row Identity" in its render.h); a resize or new colors (which
            // don't mark rows dirty) read them all.
            let with = (self.cols, self.colors_hash());
            let mut dirty: GhosttyRenderStateDirty = GHOSTTY_RENDER_STATE_DIRTY_FULL;
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_DIRTY,
                &mut dirty as *mut _ as *mut c_void,
            );
            let all = dirty == GHOSTTY_RENDER_STATE_DIRTY_FULL || with != self.seen_with;
            let mut seen = HashMap::with_capacity(self.rows as usize);
            let mut rows = Vec::with_capacity(self.rows as usize);
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                &mut self.rows_it as *mut _ as *mut c_void,
            );
            while ghostty_render_state_row_iterator_next(self.rows_it) {
                let mut sel = GhosttyRenderStateRowSelection {
                    size: std::mem::size_of::<GhosttyRenderStateRowSelection>(),
                    ..Default::default()
                };
                let selected = ghostty_render_state_row_get(
                    self.rows_it,
                    GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                    &mut sel as *mut _ as *mut c_void,
                ) == GHOSTTY_SUCCESS;
                let selection = selected.then_some((sel.start_x, sel.end_x));
                let mut id = GhosttyRenderStateRowId::default();
                let known = ghostty_render_state_row_get(
                    self.rows_it,
                    GHOSTTY_RENDER_STATE_ROW_DATA_ID,
                    &mut id as *mut _ as *mut c_void,
                ) == GHOSTTY_SUCCESS;
                // Everything changed: no row is kept (but each is remembered).
                let mut row_dirty = true;
                if !all {
                    ghostty_render_state_row_get(
                        self.rows_it,
                        GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
                        &mut row_dirty as *mut _ as *mut c_void,
                    );
                }
                let same = (known && !row_dirty)
                    .then(|| self.seen.get(&id.bits))
                    .flatten()
                    .filter(|(_, s)| *s == selection);
                let row = match same {
                    Some((row, _)) => row.clone(),
                    None => {
                        ghostty_render_state_row_get(
                            self.rows_it,
                            GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                            &mut self.cells as *mut _ as *mut c_void,
                        );
                        let mut row = Vec::with_capacity(self.cols as usize);
                        let mut x: u16 = 0;
                        while ghostty_render_state_row_cells_next(self.cells) {
                            let in_selection = selected && x >= sel.start_x && x <= sel.end_x;
                            row.push(self.read_cell(fg_default, bg_default, in_selection));
                            x += 1;
                        }
                        Arc::new(row)
                    }
                };
                if known {
                    seen.insert(id.bits, (row.clone(), selection));
                }
                rows.push(row);
            }
            (self.seen, self.seen_with) = (seen, with);

            let mut cur = GhosttyRenderStateCursor {
                size: std::mem::size_of::<GhosttyRenderStateCursor>(),
                ..Default::default()
            };
            ghostty_render_state_get(
                state,
                GHOSTTY_RENDER_STATE_DATA_CURSOR,
                &mut cur as *mut _ as *mut c_void,
            );
            let cursor = (cur.visible && cur.viewport_has_value).then(|| Cursor {
                x: if cur.wide_tail {
                    cur.viewport_x.saturating_sub(1)
                } else {
                    cur.viewport_x
                },
                y: cur.viewport_y,
                shape: match cur.visual_style {
                    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => CursorShape::Bar,
                    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => CursorShape::Underline,
                    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => CursorShape::Hollow,
                    _ => CursorShape::Block,
                },
                color: if colors.cursor_has_value {
                    color(colors.cursor)
                } else {
                    fg_default
                },
            });

            ghostty_render_state_clean(state);
            Frame {
                rows,
                cursor,
                bg: bg_default,
            }
        }
    }

    unsafe fn read_cell(&mut self, fg_default: Color, bg_default: Color, selected: bool) -> Cell {
        unsafe {
            let c = self.cells;
            let mut raw: GhosttyCell = Default::default();
            ghostty_render_state_row_cells_get(
                c,
                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                &mut raw as *mut _ as *mut c_void,
            );
            let mut wide: GhosttyCellWide = GHOSTTY_CELL_WIDE_NARROW;
            ghostty_cell_get(
                raw,
                GHOSTTY_CELL_DATA_WIDE,
                &mut wide as *mut _ as *mut c_void,
            );

            let mut style = GhosttyStyle {
                size: std::mem::size_of::<GhosttyStyle>(),
                ..Default::default()
            };
            ghostty_render_state_row_cells_get(
                c,
                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                &mut style as *mut _ as *mut c_void,
            );

            let mut fg_raw = GhosttyColorRgb::default();
            let fg = if ghostty_render_state_row_cells_get(
                c,
                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                &mut fg_raw as *mut _ as *mut c_void,
            ) == GHOSTTY_SUCCESS
            {
                color(fg_raw)
            } else {
                fg_default
            };
            let mut bg_raw = GhosttyColorRgb::default();
            let bg = (ghostty_render_state_row_cells_get(
                c,
                GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                &mut bg_raw as *mut _ as *mut c_void,
            ) == GHOSTTY_SUCCESS)
                .then(|| color(bg_raw));

            let (mut fg, mut bg) = (fg, bg);
            if style.inverse {
                let new_fg = bg.unwrap_or(bg_default);
                bg = Some(fg);
                fg = new_fg;
            }
            if selected {
                bg = Some(self.colors.selection);
            }
            let mut text = String::new();
            if wide != GHOSTTY_CELL_WIDE_SPACER_TAIL
                && wide != GHOSTTY_CELL_WIDE_SPACER_HEAD
                && !style.invisible
            {
                let mut len: u32 = 0;
                ghostty_render_state_row_cells_get(
                    c,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
                    &mut len as *mut _ as *mut c_void,
                );
                if len > 0 {
                    let mut cps = vec![0u32; len as usize];
                    ghostty_render_state_row_cells_get(
                        c,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
                        cps.as_mut_ptr() as *mut c_void,
                    );
                    text = cps.into_iter().filter_map(char::from_u32).collect();
                }
            }

            // Kitty graphics placeholders aren't drawable text (the images
            // themselves are a follow-up; the cells must still stay blank).
            if text.starts_with('\u{10eeee}') {
                text.clear();
                style.underline = 0;
                style.strikethrough = false;
            }

            Cell {
                text,
                fg,
                bg,
                bold: style.bold,
                italic: style.italic,
                faint: style.faint,
                underline: style.underline != 0,
                strike: style.strikethrough,
                wide: wide == GHOSTTY_CELL_WIDE_WIDE,
            }
        }
    }

    /// Encodes one key press with the program's current keyboard modes
    /// (application cursor, kitty keyboard protocol, modifyOtherKeys).
    fn encode_key(
        &mut self,
        key: GhosttyKey,
        mods: GhosttyMods,
        consumed: GhosttyMods,
        text: Option<&str>,
        unshifted: Option<char>,
    ) -> Vec<u8> {
        unsafe {
            ghostty_key_encoder_setopt_from_terminal(self.keys, self.term);
            let e = self.key_event;
            ghostty_key_event_set_action(e, GHOSTTY_KEY_ACTION_PRESS);
            ghostty_key_event_set_key(e, key);
            ghostty_key_event_set_mods(e, mods);
            let printable =
                text.is_some_and(|t| !t.is_empty() && !t.chars().any(|c| c.is_control()));
            ghostty_key_event_set_consumed_mods(e, if printable { consumed } else { 0 });
            match text {
                Some(t) if printable => {
                    ghostty_key_event_set_utf8(e, t.as_ptr() as *const _, t.len())
                }
                _ => ghostty_key_event_set_utf8(e, std::ptr::null(), 0),
            }
            ghostty_key_event_set_unshifted_codepoint(e, unshifted.map_or(0, |c| c as u32));
            let mut buf = [0u8; 128];
            let mut len = 0usize;
            let r = ghostty_key_encoder_encode(
                self.keys,
                e,
                buf.as_mut_ptr() as *mut _,
                buf.len(),
                &mut len,
            );
            if r != GHOSTTY_SUCCESS {
                return Vec::new();
            }
            buf[..len].to_vec()
        }
    }

    /// What a pane key press sends to the program. Plain characters
    /// arrive as TEXT (`text_bytes`); this road carries the named keys
    /// and the modified characters.
    pub fn key_bytes(
        &mut self,
        key: imba::event::Key,
        mods: imba::event::Modifiers,
    ) -> Option<Vec<u8>> {
        use imba::event::Key;
        let (code, shifted, unshifted) = match key {
            Key::Enter => (GHOSTTY_KEY_ENTER, false, None),
            Key::Backspace => (GHOSTTY_KEY_BACKSPACE, false, None),
            Key::Tab => (GHOSTTY_KEY_TAB, false, None),
            Key::Escape => (GHOSTTY_KEY_ESCAPE, false, None),
            Key::Up => (GHOSTTY_KEY_ARROW_UP, false, None),
            Key::Down => (GHOSTTY_KEY_ARROW_DOWN, false, None),
            Key::Left => (GHOSTTY_KEY_ARROW_LEFT, false, None),
            Key::Right => (GHOSTTY_KEY_ARROW_RIGHT, false, None),
            Key::Home => (GHOSTTY_KEY_HOME, false, None),
            Key::End => (GHOSTTY_KEY_END, false, None),
            Key::PageUp => (GHOSTTY_KEY_PAGE_UP, false, None),
            Key::PageDown => (GHOSTTY_KEY_PAGE_DOWN, false, None),
            Key::Delete => (GHOSTTY_KEY_DELETE, false, None),
            Key::F(n @ 1..=12) => (GHOSTTY_KEY_F1 + (n - 1) as GhosttyKey, false, None),
            Key::F(_) => return None,
            // Plain characters travel the TEXT road; only modified ones land here.
            Key::Char(_) if !mods.control && !mods.alt && !mods.command => return None,
            Key::Char(c) => {
                let (code, shifted, base) = map_char(c);
                (code, shifted, Some(base))
            }
        };
        let encoded = self.encode_key(code, ghostty_mods(mods, shifted), 0, None, unshifted);
        (!encoded.is_empty()).then_some(encoded)
    }

    /// What typed text sends to the program: each character encoded as
    /// its key, so kitty-keyboard programs see real key events.
    pub fn text_bytes(&mut self, text: &str) -> Vec<u8> {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => {
                let (code, shifted, base) = map_char(c);
                let mods = if shifted {
                    GHOSTTY_MODS_SHIFT as GhosttyMods
                } else {
                    0
                };
                let utf8 = c.to_string();
                let encoded = self.encode_key(code, mods, mods, Some(&utf8), Some(base));
                match encoded.is_empty() {
                    // A character no key answers to still reaches the program.
                    true => utf8.into_bytes(),
                    false => encoded,
                }
            }
            // Composed input (IME commits) is text, not keys.
            _ => text.as_bytes().to_vec(),
        }
    }

    /// Encodes a mouse event for a program that tracks the mouse. Positions are in pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_mouse(
        &mut self,
        action: GhosttyMouseAction,
        button: Option<GhosttyMouseButton>,
        mods: imba::event::Modifiers,
        x: f32,
        y: f32,
        // (columns, rows, cell width, cell height), cells in px.
        size: (u32, u32, f32, f32),
        any_pressed: bool,
    ) -> Vec<u8> {
        // Ghostty takes whole-pixel cells; at zoom a cell is 11.7 px, and
        // rounding it alone made the reported column run ahead of the pointer
        // (2 cells by mid-line). So the position moves into a grid of whole
        // cells: same column and row exactly, pixels (SGR-pixels mode) within
        // a few percent.
        let (cols, rows, cw, ch) = size;
        let (cwi, chi) = (cw.round().max(1.0), ch.round().max(1.0));
        let (x, y) = (x / cw.max(1.0) * cwi, y / ch.max(1.0) * chi);
        unsafe {
            ghostty_mouse_encoder_setopt_from_terminal(self.mouse, self.term);
            let s = GhosttyMouseEncoderSize {
                size: std::mem::size_of::<GhosttyMouseEncoderSize>(),
                screen_width: cols * cwi as u32,
                screen_height: rows * chi as u32,
                cell_width: cwi as u32,
                cell_height: chi as u32,
                ..Default::default()
            };
            ghostty_mouse_encoder_setopt(
                self.mouse,
                GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                &s as *const _ as *const c_void,
            );
            ghostty_mouse_encoder_setopt(
                self.mouse,
                GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                &any_pressed as *const _ as *const c_void,
            );
            let e = self.mouse_event;
            ghostty_mouse_event_set_action(e, action);
            match button {
                Some(b) => ghostty_mouse_event_set_button(e, b),
                None => ghostty_mouse_event_clear_button(e),
            }
            ghostty_mouse_event_set_mods(e, ghostty_mods(mods, false));
            ghostty_mouse_event_set_position(e, GhosttyMousePosition { x, y });
            let mut buf = [0u8; 64];
            let mut len = 0usize;
            let r = ghostty_mouse_encoder_encode(
                self.mouse,
                e,
                buf.as_mut_ptr() as *mut _,
                buf.len(),
                &mut len,
            );
            if r != GHOSTTY_SUCCESS || len == 0 {
                return Vec::new();
            }
            // Refreshing the encoder's terminal/size options resets its cell tracking.
            if action == GHOSTTY_MOUSE_ACTION_MOTION && self.last_mouse_report == buf[..len] {
                return Vec::new();
            }
            self.last_mouse_report.clear();
            self.last_mouse_report.extend_from_slice(&buf[..len]);
            buf[..len].to_vec()
        }
    }
}

fn ghostty_mods(mods: imba::event::Modifiers, shifted: bool) -> GhosttyMods {
    let mut out: GhosttyMods = 0;
    if mods.shift || shifted {
        out |= GHOSTTY_MODS_SHIFT as GhosttyMods;
    }
    if mods.control {
        out |= GHOSTTY_MODS_CTRL as GhosttyMods;
    }
    if mods.alt {
        out |= GHOSTTY_MODS_ALT as GhosttyMods;
    }
    if mods.command {
        out |= GHOSTTY_MODS_SUPER as GhosttyMods;
    }
    out
}

/// A character's key: the unshifted US-layout key it sits on, and
/// whether reaching it took Shift. Characters from other layouts keep
/// `UNIDENTIFIED` and ride on their text.
fn map_char(c: char) -> (GhosttyKey, bool, char) {
    const SHIFTED: &[(char, char)] = &[
        ('~', '`'),
        ('!', '1'),
        ('@', '2'),
        ('#', '3'),
        ('$', '4'),
        ('%', '5'),
        ('^', '6'),
        ('&', '7'),
        ('*', '8'),
        ('(', '9'),
        (')', '0'),
        ('_', '-'),
        ('+', '='),
        ('{', '['),
        ('}', ']'),
        ('|', '\\'),
        (':', ';'),
        ('"', '\''),
        ('<', ','),
        ('>', '.'),
        ('?', '/'),
    ];
    let lower = c.to_ascii_lowercase();
    let (base, shifted) = match SHIFTED.iter().find(|(s, _)| *s == c) {
        Some((_, b)) => (*b, true),
        None => (lower, c.is_ascii_uppercase()),
    };
    let code = match base {
        'a'..='z' => GHOSTTY_KEY_A + (base as u8 - b'a') as GhosttyKey,
        '0'..='9' => GHOSTTY_KEY_DIGIT_0 + (base as u8 - b'0') as GhosttyKey,
        ' ' => GHOSTTY_KEY_SPACE,
        '`' => GHOSTTY_KEY_BACKQUOTE,
        '\\' => GHOSTTY_KEY_BACKSLASH,
        '[' => GHOSTTY_KEY_BRACKET_LEFT,
        ']' => GHOSTTY_KEY_BRACKET_RIGHT,
        ',' => GHOSTTY_KEY_COMMA,
        '=' => GHOSTTY_KEY_EQUAL,
        '-' => GHOSTTY_KEY_MINUS,
        '.' => GHOSTTY_KEY_PERIOD,
        '\'' => GHOSTTY_KEY_QUOTE,
        ';' => GHOSTTY_KEY_SEMICOLON,
        '/' => GHOSTTY_KEY_SLASH,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    };
    (code, shifted, base)
}

impl Drop for Vt {
    fn drop(&mut self) {
        unsafe {
            ghostty_selection_gesture_event_free(self.press);
            ghostty_selection_gesture_event_free(self.drag);
            ghostty_selection_gesture_event_free(self.release);
            ghostty_selection_gesture_free(self.gesture, self.term);
            ghostty_mouse_event_free(self.mouse_event);
            ghostty_mouse_encoder_free(self.mouse);
            ghostty_key_event_free(self.key_event);
            ghostty_key_encoder_free(self.keys);
            ghostty_render_state_row_cells_free(self.cells);
            ghostty_render_state_row_iterator_free(self.rows_it);
            ghostty_terminal_free(self.term);
            ghostty_render_state_free(self.shared.state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imba::event::{Key, Modifiers};

    fn vt(cols: u16, rows: u16) -> Vt {
        Vt::new(cols, rows, Colors::default())
    }

    fn row_text(frame: &Frame, y: usize) -> String {
        frame.rows[y].iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn styled_text_reaches_the_frame() {
        let mut vt = vt(20, 3);
        vt.write(b"hi \x1b[1;31mred\x1b[0m\r\n\xe4\xb8\xad!");
        let f = vt.frame();
        assert_eq!(f.rows.len(), 3);
        assert!(row_text(&f, 0).starts_with("hi red"));
        assert!(f.rows[0][3].bold);
        assert_eq!(f.rows[0][3].fg, Colors::default().palette16[1]);
        assert!(f.rows[1][0].wide);
        assert_eq!(f.rows[1][0].text, "中");
        assert_eq!(f.rows[1][1].text, "");
        assert_eq!(f.rows[1][2].text, "!");
        let c = f.cursor.unwrap();
        assert_eq!((c.x, c.y), (3, 1));
    }

    #[test]
    fn a_frame_reads_again_only_the_rows_that_changed() {
        let mut vt = vt(20, 4);
        // The cursor ends on the last row: the row it leaves counts as changed.
        vt.write(b"one\r\ntwo\r\nthree\r\n");
        let before = vt.frame();
        vt.write(b"\x1b[2;1Hchanged");
        let after = vt.frame();
        let same = |y: usize| Arc::ptr_eq(&before.rows[y], &after.rows[y]);
        assert!(same(0) && same(2), "untouched rows aren't read again");
        assert!(!same(1));
        assert!(row_text(&after, 1).starts_with("changed"));
        vt.write(b"\x1b]4;1;rgb:12/34/56\x1b\\");
        let recolored = vt.frame();
        assert!(
            !Arc::ptr_eq(&after.rows[0], &recolored.rows[0]),
            "a palette change reads every row"
        );
    }

    #[test]
    fn scrollback_retains_styled_lines() {
        let mut vt = vt(80, 25);
        for line in 1..=2000 {
            vt.write(format!("\x1b[1;31m{line:04}\x1b[0m {}\r\n", "x".repeat(70)).as_bytes());
        }
        vt.scroll_viewport(-10_000);
        let frame = vt.frame();
        assert!(row_text(&frame, 0).starts_with("0001 "));
        vt.scroll_to_bottom();
        let frame = vt.frame();
        assert!(row_text(&frame, 23).starts_with("2000 "));
    }

    #[test]
    fn keys_follow_the_program_modes() {
        let plain = Modifiers::default();
        let ctrl = Modifiers {
            control: true,
            ..Default::default()
        };
        let mut vt = vt(20, 3);
        assert_eq!(vt.text_bytes("a"), b"a");
        assert_eq!(vt.text_bytes("L"), b"L");
        assert_eq!(vt.key_bytes(Key::Char('c'), ctrl), Some(vec![0x03]));
        assert_eq!(vt.key_bytes(Key::Enter, plain), Some(b"\r".to_vec()));
        assert_eq!(vt.key_bytes(Key::Up, plain), Some(b"\x1b[A".to_vec()));
        vt.write(b"\x1b[?1h"); // application cursor keys
        assert_eq!(vt.key_bytes(Key::Up, plain), Some(b"\x1bOA".to_vec()));
        assert_eq!(
            vt.key_bytes(Key::Char('x'), plain),
            None,
            "plain text travels as text"
        );
        // Modified arrows carry their modifier (the old keymap dropped it).
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(vt.key_bytes(Key::Right, ctrl), Some(b"\x1b[1;5C".to_vec()));
        assert_eq!(vt.key_bytes(Key::Tab, shift), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn kitty_keyboard_mode_reports_releases_of_plain_text() {
        let mut vt = vt(20, 3);
        vt.write(b"\x1b[>1u"); // push kitty disambiguate flags
        let ctrl = Modifiers {
            control: true,
            ..Default::default()
        };
        assert_eq!(
            vt.key_bytes(Key::Char('c'), ctrl),
            Some(b"\x1b[99;5u".to_vec())
        );
        vt.write(b"\x1b[<u");
        assert_eq!(vt.key_bytes(Key::Char('c'), ctrl), Some(vec![0x03]));
    }

    #[test]
    fn mouse_reports_the_cell_under_the_pointer_with_fractional_cells() {
        let mut vt = vt(80, 2);
        vt.write(b"\x1b[?1000h\x1b[?1006h");
        assert!(vt.mouse_tracking());
        let (cw, ch) = (11.7, 30.0);
        let x = 39.0 * cw + 5.0; // inside column 39 (1-based 40)
        let bytes = vt.encode_mouse(
            GHOSTTY_MOUSE_ACTION_PRESS,
            Some(GHOSTTY_MOUSE_BUTTON_LEFT),
            Modifiers::default(),
            x,
            10.0,
            (80, 2, cw, ch),
            false,
        );
        assert_eq!(String::from_utf8_lossy(&bytes), "\x1b[<0;40;1M");
    }

    #[test]
    fn mouse_hover_tracks_only_changed_cells_and_respects_button_mode() {
        let mut vt = vt(80, 2);
        let size = (80, 2, 10.0, 20.0);
        let plain = Modifiers::default();
        vt.write(b"\x1b[?1003h\x1b[?1006h");
        assert_eq!(
            vt.encode_mouse(
                GHOSTTY_MOUSE_ACTION_MOTION,
                None,
                plain,
                15.0,
                5.0,
                size,
                false
            ),
            b"\x1b[<35;2;1M"
        );
        assert!(vt
            .encode_mouse(
                GHOSTTY_MOUSE_ACTION_MOTION,
                None,
                plain,
                16.0,
                5.0,
                size,
                false
            )
            .is_empty());
        vt.write(b"\x1b[?1002h");
        assert!(vt
            .encode_mouse(
                GHOSTTY_MOUSE_ACTION_MOTION,
                None,
                plain,
                35.0,
                5.0,
                size,
                false
            )
            .is_empty());
        assert_eq!(
            vt.encode_mouse(
                GHOSTTY_MOUSE_ACTION_MOTION,
                Some(GHOSTTY_MOUSE_BUTTON_LEFT),
                plain,
                35.0,
                5.0,
                size,
                true
            ),
            b"\x1b[<32;4;1M"
        );
    }

    #[test]
    fn repeated_clicks_select_words_then_lines() {
        let mut vt = vt(30, 3);
        vt.write(b"hello world\r\nsecond line");
        for (x, expected) in [
            (12.0, None),
            (14.0, Some("hello")),
            (13.0, Some("hello world")),
        ] {
            vt.select(Select::Press, 1, 0, x, 5.0, 10.0, 60.0);
            vt.select(Select::Release, 1, 0, x, 5.0, 10.0, 60.0);
            assert_eq!(vt.selection_text().as_deref(), expected);
        }
    }

    #[test]
    fn dragging_selects_and_copies_text() {
        let mut vt = vt(20, 3);
        vt.write(b"hello world\r\nsecond");
        vt.select(Select::Press, 0, 0, 2.0, 5.0, 10.0, 60.0);
        vt.select(Select::Drag, 4, 0, 46.0, 5.0, 10.0, 60.0);
        vt.select(Select::Release, 4, 0, 46.0, 5.0, 10.0, 60.0);
        assert_eq!(vt.selection_text().as_deref(), Some("hello"));
        let frame = vt.frame();
        assert_eq!(frame.rows[0][0].bg, Some(Colors::default().selection));
        vt.clear_selection();
        assert_eq!(vt.selection_text(), None);
    }

    #[test]
    fn paste_wraps_only_when_the_program_asked() {
        let mut vt = vt(20, 3);
        assert_eq!(vt.paste_bytes("a\nb"), b"a\rb");
        vt.write(b"\x1b[?2004h");
        assert_eq!(vt.paste_bytes("a\nb"), b"\x1b[200~a\nb\x1b[201~");
    }

    #[test]
    fn cursor_reports_write_back_through_replies() {
        let mut vt = vt(20, 3);
        vt.write(b"\x1b[6n");
        assert_eq!(vt.take_replies(), b"\x1b[1;1R");
        assert!(vt.take_replies().is_empty());
    }

    #[test]
    fn title_is_reported_once() {
        let mut vt = vt(20, 3);
        vt.write(b"\x1b]2;hello\x07");
        assert_eq!(vt.take_title().as_deref(), Some("hello"));
        assert_eq!(vt.take_title(), None);
    }

    #[test]
    fn alt_screen_follows_the_stream() {
        let mut vt = vt(20, 3);
        assert!(!vt.alt_screen());
        vt.write(b"\x1b[?1049h");
        assert!(vt.alt_screen());
        vt.write(b"\x1b[?1049l");
        assert!(!vt.alt_screen());
    }
}
