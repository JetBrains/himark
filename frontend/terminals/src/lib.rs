// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The terminals COLLECTION (docs/entities.md): a session's terminal
//! emulators, addressed by `(Id<Terminals>, TerminalId)`. Pure model
//! — the grid (libghostty-vt behind `vt::Vt`), the backend door. The
//! pane, the keymap and the wire that feeds `Session::output` live
//! with the shells; which wire channel feeds the PTY is the backend's
//! business, never the collection's key.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use imba::store::Store;

pub mod chrome;
pub mod pane;
pub mod vt;

use vt::{Colors, Frame, Select, Vt};

/// The PTY end a session writes into — the one door to the wire,
/// implemented by whoever owns the transport.
pub trait TerminalBackend: Send + Sync {
    fn write(&self, bytes: &[u8]);

    fn resize(&self, cols: u16, rows: u16, px_width: f32, px_height: f32);

    fn hangup(&self);
}

pub struct Session {
    vt: Mutex<Vt>,
    backend: Box<dyn TerminalBackend>,
    title: Mutex<String>,
    exited: Mutex<Option<i32>>,
    hung_up: AtomicBool,

    told: Mutex<(u16, u16)>,

    /// What the pane's press armed: drags and releases are broadcast,
    /// so only the session that owns the press may claim them.
    pressed: Mutex<Option<Press>>,
}

/// What a mouse press started: the program took it (with this button),
/// or a selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Press {
    Reported(imba::event::MouseButton),
    Selecting,
}

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub const DEFAULT_COLS: u16 = 80;
/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub const DEFAULT_ROWS: u16 = 24;

impl Session {
    pub fn new(backend: Box<dyn TerminalBackend>) -> Arc<Self> {
        Arc::new(Self {
            vt: Mutex::new(Vt::new(DEFAULT_COLS, DEFAULT_ROWS, Colors::default())),
            backend,
            title: Mutex::new(String::new()),
            exited: Mutex::new(None),
            hung_up: AtomicBool::new(false),
            told: Mutex::new((DEFAULT_COLS, DEFAULT_ROWS)),
            pressed: Mutex::new(None),
        })
    }

    /// Program output lands in the grid; whatever the terminal wants
    /// to say back (cursor reports, size reports) rides to the PTY.
    pub fn output(&self, bytes: &[u8]) -> bool {
        let mut vt = self.vt();
        vt.write(bytes);
        self.pump(&mut vt);
        true
    }

    pub fn exited(&self, code: i32) -> bool {
        *self.exited.lock().expect("exit lock") = Some(code);
        true
    }

    /// Wipe the grid and the parser whole — the RE-ATTACH road. A
    /// reconnect replays the host's snapshot from byte one, and
    /// feeding that into a grid still holding the old copy would
    /// double every line.
    pub fn reset(&self) -> bool {
        let (cols, rows) = *self.told.lock().expect("told lock");
        let mut vt = self.vt();
        *vt = Vt::new(cols, rows, vt.colors().clone());
        true
    }

    fn vt(&self) -> MutexGuard<'_, Vt> {
        self.vt.lock().expect("vt lock")
    }

    /// Drains what the emulator produced besides the grid: replies to
    /// the program, and the title.
    fn pump(&self, vt: &mut Vt) {
        let replies = vt.take_replies();
        if !replies.is_empty() {
            self.write(&replies);
        }
        if let Some(title) = vt.take_title() {
            *self.title.lock().expect("title lock") = title;
        }
    }

    /// One frame for the pane to draw.
    pub fn frame(&self) -> Frame {
        self.vt().frame()
    }

    /// The theme's colors, applied live.
    pub fn set_colors(&self, colors: Colors) {
        self.vt().set_colors(colors);
    }

    pub fn title(&self) -> String {
        self.title.lock().expect("title lock").clone()
    }

    /// The grid size last told to the PTY — the paint reconciles
    /// against it.
    pub fn told(&self) -> (u16, u16) {
        *self.told.lock().expect("told lock")
    }

    /// The emulator's own grid size.
    pub fn grid_size(&self) -> (u16, u16) {
        let vt = self.vt();
        (vt.cols, vt.rows)
    }

    pub fn write(&self, bytes: &[u8]) {
        if self.exited.lock().expect("exit lock").is_none() {
            self.backend.write(bytes);
        }
    }

    /// A pane key press: encoded in the program's keyboard mode and
    /// sent. Typing lets go of the selection and rides the viewport
    /// back to the prompt.
    pub fn key_input(&self, key: imba::event::Key, mods: imba::event::Modifiers) -> bool {
        let bytes = {
            let mut vt = self.vt();
            let Some(bytes) = vt.key_bytes(key, mods) else {
                return false;
            };
            vt.clear_selection();
            vt.scroll_to_bottom();
            bytes
        };
        self.write(&bytes);
        true
    }

    /// Typed text, encoded per character so kitty-keyboard programs
    /// see real key events.
    pub fn text_input(&self, text: &str) {
        let bytes = {
            let mut vt = self.vt();
            let bytes = vt.text_bytes(text);
            vt.clear_selection();
            vt.scroll_to_bottom();
            bytes
        };
        self.write(&bytes);
    }

    /// Pasted text, wrapped the way the program asked (bracketed paste).
    pub fn paste(&self, text: &str) {
        let bytes = {
            let mut vt = self.vt();
            let bytes = vt.paste_bytes(text);
            vt.scroll_to_bottom();
            bytes
        };
        self.write(&bytes);
    }

    pub fn selection_text(&self) -> Option<String> {
        self.vt().selection_text()
    }

    /// One mouse step of a selection gesture, in grid cells and pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn select(
        &self,
        step: Select,
        col: u16,
        row: u16,
        x: f32,
        y: f32,
        cell_w: f32,
        height: f32,
    ) {
        self.vt().select(step, col, row, x, y, cell_w, height);
    }

    /// Whether the program tracks the mouse (then clicks are its, not
    /// a selection's — unless Shift holds them back).
    pub fn mouse_tracking(&self) -> bool {
        self.vt().mouse_tracking()
    }

    pub fn alt_screen(&self) -> bool {
        self.vt().alt_screen()
    }

    /// A mouse report for a program that tracks the mouse; empty when
    /// the mode doesn't ask for this event.
    #[allow(clippy::too_many_arguments)]
    pub fn mouse_report(
        &self,
        action: ghostty_sys::GhosttyMouseAction,
        button: Option<ghostty_sys::GhosttyMouseButton>,
        mods: imba::event::Modifiers,
        x: f32,
        y: f32,
        size: (u32, u32, f32, f32),
        any_pressed: bool,
    ) {
        let bytes = self
            .vt()
            .encode_mouse(action, button, mods, x, y, size, any_pressed);
        if !bytes.is_empty() {
            self.write(&bytes);
        }
    }

    /// What the pane's last press armed, if it still holds.
    pub fn press(&self) -> Option<Press> {
        *self.pressed.lock().expect("press lock")
    }

    pub fn set_press(&self, press: Option<Press>) {
        *self.pressed.lock().expect("press lock") = press;
    }

    pub fn resize(&self, cols: u16, rows: u16, px_width: f32, px_height: f32) {
        let cols = cols.max(2);
        let rows = rows.max(2);
        {
            let mut told = self.told.lock().expect("told lock");
            if *told == (cols, rows) {
                return;
            }
            *told = (cols, rows);
        }
        let cell_w = (px_width / cols as f32).round().max(1.0) as u32;
        let cell_h = (px_height / rows as f32).round().max(1.0) as u32;
        {
            let mut vt = self.vt();
            vt.resize(cols, rows, cell_w, cell_h);
            // An in-band size report (mode 2048) answers right away.
            self.pump(&mut vt);
        }
        self.backend.resize(cols, rows, px_width, px_height);
    }

    pub fn hangup(&self) {
        if !self.hung_up.swap(true, Ordering::SeqCst) {
            self.backend.hangup();
        }
    }

    pub(crate) fn scroll_lines(&self, lines: i32) {
        // Positive scrolls INTO history, as the pane's wheel delta has
        // it; libghostty's delta goes the other way.
        self.vt().scroll_viewport(-(lines as isize));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.hangup();
    }
}

/// A terminal's identity in its session — minted at the landing that
/// files the session, carried by the pane and its place.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TerminalId(u64);

impl TerminalId {
    pub fn mint() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// The minted ordinal — the pane's display number and its scroll
    /// surface key.
    pub fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Default)]
pub struct Terminals(rpds::HashTrieMapSync<TerminalId, Arc<Session>>);

/// A terminal belongs to the session whose id reached here — threaded
/// from the owning session's row by whoever had the session context
/// (docs/entities.md law 3); this crate never sees a session.
impl Terminals {
    pub fn put(
        store: &mut Store,
        terminals: imba::store::Id<Self>,
        id: TerminalId,
        session: Arc<Session>,
    ) {
        store.update_entity(terminals, |terminals| {
            terminals.0.insert_mut(id, session);
        });
    }

    pub fn session(
        store: &Store,
        terminals: imba::store::Id<Self>,
        id: TerminalId,
    ) -> Option<Arc<Session>> {
        Self::session_ref(store, terminals, id).cloned()
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn session_ref<'a>(
        store: &'a Store,
        terminals: imba::store::Id<Self>,
        id: TerminalId,
    ) -> Option<&'a Arc<Session>> {
        store.entity(terminals)?.0.get(&id)
    }

    pub fn remove(store: &mut Store, terminals: imba::store::Id<Self>, id: TerminalId) {
        store.update_entity(terminals, |terminals| {
            terminals.0.remove_mut(&id);
        });
    }

    pub fn list(store: &Store, terminals: imba::store::Id<Self>) -> Vec<TerminalId> {
        store
            .entity(terminals)
            .map(|terminals| terminals.0.keys().copied().collect())
            .unwrap_or_default()
    }

    pub fn holds(&self, id: TerminalId) -> bool {
        self.0.contains_key(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct Recorder {
        written: Arc<Mutex<Vec<u8>>>,
        resizes: Arc<Mutex<Vec<(u16, u16)>>>,
    }

    impl TerminalBackend for Recorder {
        fn write(&self, bytes: &[u8]) {
            self.written.lock().unwrap().extend_from_slice(bytes);
        }

        fn resize(&self, cols: u16, rows: u16, _px_width: f32, _px_height: f32) {
            self.resizes.lock().unwrap().push((cols, rows));
        }

        fn hangup(&self) {}
    }

    fn row_text(session: &Session, row: usize) -> String {
        let frame = session.frame();
        frame.rows[row]
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn output_lands_in_the_grid() {
        let session = Session::new(Box::new(Recorder::default()));
        session.output(b"hello \x1b[1;32mworld\x1b[0m\r\ncolors");
        assert_eq!(row_text(&session, 0), "hello world");
        assert_eq!(row_text(&session, 1), "colors");
    }

    #[test]
    fn cursor_reports_write_back_through_the_backend() {
        let recorder = Recorder::default();
        let session = Session::new(Box::new(recorder.clone()));

        session.output(b"\x1b[6n");
        let written = recorder.written.lock().unwrap().clone();
        assert_eq!(String::from_utf8_lossy(&written), "\x1b[1;1R");
    }

    #[test]
    fn resize_reaches_term_and_backend_once() {
        let recorder = Recorder::default();
        let session = Session::new(Box::new(recorder.clone()));
        session.resize(120, 40, 960.0, 1200.0);
        session.resize(120, 40, 960.0, 1200.0);
        assert_eq!(*recorder.resizes.lock().unwrap(), vec![(120, 40)]);
        assert_eq!(session.grid_size(), (120, 40));
    }

    /// The re-attach replays the snapshot from byte one — the grid
    /// must be EMPTY when it lands or every line arrives twice.
    #[test]
    fn reset_wipes_the_grid_for_the_replay() {
        let recorder = Recorder::default();
        let session = Session::new(Box::new(recorder.clone()));
        session.resize(100, 30, 800.0, 900.0);
        session.output(b"stale line");
        assert!(session.reset());
        assert_eq!(
            session.grid_size(),
            (100, 30),
            "the reset keeps the TOLD size"
        );
        assert_eq!(row_text(&session, 0), "", "the stale copy is gone");
        session.output(b"fresh");
        assert_eq!(
            row_text(&session, 0),
            "fresh",
            "the replay lands on a clean grid"
        );
    }

    #[test]
    fn an_exited_session_stops_writing() {
        let recorder = Recorder::default();
        let session = Session::new(Box::new(recorder.clone()));
        session.write(b"ls\r");
        session.exited(0);
        session.write(b"ignored");
        assert_eq!(recorder.written.lock().unwrap().as_slice(), b"ls\r");
    }

    #[test]
    fn titles_follow_the_stream() {
        let session = Session::new(Box::new(Recorder::default()));
        session.output(b"\x1b]0;vim\x07\x1b[?1049h");
        assert_eq!(session.title(), "vim");
        assert!(session.alt_screen());
    }

    #[test]
    fn typing_snaps_the_viewport_back_to_the_prompt() {
        let recorder = Recorder::default();
        let session = Session::new(Box::new(recorder.clone()));
        for line in 0..100 {
            session.output(format!("line {line}\r\n").as_bytes());
        }
        session.scroll_lines(50);
        assert!(
            row_text(&session, 0).starts_with("line 27"),
            "{}",
            row_text(&session, 0)
        );
        session.text_input("x");
        assert_eq!(recorder.written.lock().unwrap().as_slice(), b"x");
        assert!(
            row_text(&session, 0).starts_with("line 77"),
            "{}",
            row_text(&session, 0)
        );
    }
}
