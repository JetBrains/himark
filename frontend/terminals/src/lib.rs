// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The terminals COLLECTION (docs/entities.md): a session's terminal
//! emulators, addressed by `(Id<Terminals>, TerminalId)`. Pure model
//! — the grid, the parser, the backend door. The pane, the keymap and
//! the wire that feeds `Session::output` live with the shells; which
//! wire channel feeds the PTY is the backend's business, never the
//! collection's key.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

use imba::store::Store;

pub mod pane;

/// The PTY end a session writes into — the one door to the wire,
/// implemented by whoever owns the transport.
pub trait TerminalBackend: Send + Sync {
    fn write(&self, bytes: &[u8]);

    fn resize(&self, cols: u16, rows: u16, px_width: f32, px_height: f32);

    fn hangup(&self);
}

#[derive(Clone)]
pub struct Collector(Arc<Mutex<Vec<TermEvent>>>);

impl EventListener for Collector {
    fn send_event(&self, event: TermEvent) {
        self.0.lock().expect("collector lock").push(event);
    }
}

pub struct Session {
    term: FairMutex<Term<Collector>>,
    parser: Mutex<Processor>,
    events: Arc<Mutex<Vec<TermEvent>>>,
    backend: Box<dyn TerminalBackend>,
    title: Mutex<String>,
    exited: Mutex<Option<i32>>,
    hung_up: AtomicBool,

    told: Mutex<(u16, u16)>,
}

pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

impl Session {
    pub fn new(backend: Box<dyn TerminalBackend>) -> Arc<Self> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let size = TermSize::new(DEFAULT_COLS as usize, DEFAULT_ROWS as usize);
        let term = Term::new(Config::default(), &size, Collector(events.clone()));
        Arc::new(Self {
            term: FairMutex::new(term),
            parser: Mutex::new(Processor::new()),
            events,
            backend,
            title: Mutex::new(String::new()),
            exited: Mutex::new(None),
            hung_up: AtomicBool::new(false),
            told: Mutex::new((DEFAULT_COLS, DEFAULT_ROWS)),
        })
    }

    pub fn output(&self, bytes: &[u8]) -> bool {
        {
            let mut parser = self.parser.lock().expect("parser lock");
            let mut term = self.term.lock();
            parser.advance(&mut *term, bytes);
        }
        self.drain_events();
        true
    }

    pub fn exited(&self, code: i32) -> bool {
        *self.exited.lock().expect("exit lock") = Some(code);
        true
    }

    /// The emulator, for the shell that paints the grid.
    pub fn term(&self) -> &FairMutex<Term<Collector>> {
        &self.term
    }

    pub fn title(&self) -> String {
        self.title.lock().expect("title lock").clone()
    }

    /// The grid size last told to the PTY — the paint reconciles
    /// against it.
    pub fn told(&self) -> (u16, u16) {
        *self.told.lock().expect("told lock")
    }

    fn drain_events(&self) {
        let events: Vec<TermEvent> = std::mem::take(&mut *self.events.lock().expect("events"));
        for event in events {
            match event {
                TermEvent::PtyWrite(text) => self.write(text.as_bytes()),
                TermEvent::Title(title) => {
                    *self.title.lock().expect("title lock") = title;
                }
                TermEvent::ResetTitle => self.title.lock().expect("title lock").clear(),
                TermEvent::TextAreaSizeRequest(format) => {
                    let (cols, rows) = *self.told.lock().expect("told lock");
                    let reply = format(WindowSize {
                        num_lines: rows,
                        num_cols: cols,
                        cell_width: 0,
                        cell_height: 0,
                    });
                    self.write(reply.as_bytes());
                }

                _ => {}
            }
        }
    }

    pub fn write(&self, bytes: &[u8]) {
        if self.exited.lock().expect("exit lock").is_none() {
            self.backend.write(bytes);
        }
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
        self.term
            .lock()
            .resize(TermSize::new(cols as usize, rows as usize));
        self.backend.resize(cols, rows, px_width, px_height);
    }

    pub fn hangup(&self) {
        if !self.hung_up.swap(true, Ordering::SeqCst) {
            self.backend.hangup();
        }
    }

    pub fn scroll_lines(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
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
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::TermMode;

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

    fn row_text(session: &Session, row: i32) -> String {
        let term = session.term.lock();
        let mut text = String::new();
        for column in 0..term.columns() {
            text.push(term.grid()[Line(row)][Column(column)].c);
        }
        text.trim_end().to_owned()
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
        assert_eq!(session.term.lock().columns(), 120);
        assert_eq!(session.term.lock().screen_lines(), 40);
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
    fn alternate_screen_and_title_follow_the_stream() {
        let session = Session::new(Box::new(Recorder::default()));
        session.output(b"\x1b]0;vim\x07\x1b[?1049h");
        assert_eq!(session.title(), "vim");
        assert!(session.term.lock().mode().contains(TermMode::ALT_SCREEN));
    }
}
