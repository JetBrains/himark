use super::*;
use crate::AppExt;

#[derive(Clone, Default)]
struct Recorder {
    written: Arc<Mutex<Vec<u8>>>,
    resizes: Arc<Mutex<Vec<(u16, u16)>>>,
    hangups: Arc<Mutex<usize>>,
}

impl TerminalBackend for Recorder {
    fn write(&self, bytes: &[u8]) {
        self.written.lock().unwrap().extend_from_slice(bytes);
    }

    fn resize(&self, cols: u16, rows: u16, _px_width: f32, _px_height: f32) {
        self.resizes.lock().unwrap().push((cols, rows));
    }

    fn hangup(&self) {
        *self.hangups.lock().unwrap() += 1;
    }
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
fn dismantle_hangs_up_exactly_once() {
    let recorder = Recorder::default();
    let session = Session::new(Box::new(recorder.clone()));
    let mut store = Store::new();
    Terminals::put(&mut store, "test-term:1".to_owned(), session.clone());
    let mut panel = TerminalView::new("test-term:1".to_owned());
    PanelView::dismantle(&mut panel, &mut store);
    session.hangup();
    assert_eq!(*recorder.hangups.lock().unwrap(), 1);
    assert!(
        Terminals::list(&store).is_empty(),
        "the row left the family"
    );
}

#[test]
fn keys_encode_like_xterm() {
    let plain = Modifiers::default();
    let ctrl = Modifiers {
        control: true,
        ..Default::default()
    };
    assert_eq!(encode_key(Key::Enter, plain, false), Some(b"\r".to_vec()));
    assert_eq!(encode_key(Key::Up, plain, false), Some(b"\x1b[A".to_vec()));
    assert_eq!(encode_key(Key::Up, plain, true), Some(b"\x1bOA".to_vec()));
    assert_eq!(encode_key(Key::Char('c'), ctrl, false), Some(vec![0x03]));
    assert_eq!(encode_key(Key::Char('d'), ctrl, false), Some(vec![0x04]));
    assert_eq!(
        encode_key(Key::F(5), plain, false),
        Some(b"\x1b[15~".to_vec())
    );
    assert_eq!(encode_key(Key::Char('x'), plain, false), None);
}

#[test]
fn alternate_screen_and_title_follow_the_stream() {
    let session = Session::new(Box::new(Recorder::default()));
    session.output(b"\x1b]0;vim\x07\x1b[?1049h");
    assert_eq!(session.title.lock().unwrap().as_str(), "vim");
    assert!(session.term.lock().mode().contains(TermMode::ALT_SCREEN));
}

#[test]
fn the_panel_reconciles_its_grid_and_routes_focused_input() {
    let mut app = crate::Application::new(crate::AppFonts::embedded());
    let _ = app.add_window();
    let recorder = Recorder::default();
    let session = Session::new(Box::new(recorder.clone()));

    let mut surface = skia_safe::surfaces::raster_n32_premul((1600, 900)).expect("surface");
    assert!(app.new_scratch(app.sole_window()));
    let _ = crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    Terminals::put(
        &mut app.store_mut(),
        "test-term:g".to_owned(),
        session.clone(),
    );
    assert!(app.open_panel(
        app.sole_window(),
        Box::new(TerminalView::new("test-term:g".to_owned()))
    ));
    session.output(b"$ echo himark\r\n\x1b[32mhimark\x1b[0m\r\n$ ");

    let _ = crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let _ = crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let resizes = recorder.resizes.lock().unwrap().clone();
    assert_eq!(
        resizes.len(),
        1,
        "one resize to the laid-out grid: {resizes:?}"
    );
    let (cols, rows) = resizes[0];
    assert_eq!(session.term.lock().columns(), cols as usize);
    assert_eq!(session.term.lock().screen_lines(), rows as usize);
    assert!(
        (cols, rows) != (DEFAULT_COLS, DEFAULT_ROWS),
        "the pane's grid differs from the default"
    );

    assert!(crate::test_driver::type_text(&mut app, "ls"));
    assert!(crate::test_driver::key(
        &mut app,
        imba::event::Key::Char('c'),
        Modifiers {
            control: true,
            ..Default::default()
        }
    ));
    let written = recorder.written.lock().unwrap().clone();
    assert_eq!(written.as_slice(), b"ls\x03");
}

#[test]
#[ignore = "writes a screenshot into HIMARK_SHOT (a directory) for visual inspection"]
fn dump_terminal_screenshot() {
    let Some(dir) = std::env::var_os("HIMARK_SHOT") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("shot dir");
    let mut app = crate::Application::new(crate::AppFonts::embedded());
    let _ = app.add_window();
    let session = Session::new(Box::new(Recorder::default()));
    assert!(app.new_scratch(app.sole_window()));
    let mut surface = skia_safe::surfaces::raster_n32_premul((1600, 900)).expect("surface");
    let _ = crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    Terminals::put(
        &mut app.store_mut(),
        "test-term:s".to_owned(),
        session.clone(),
    );
    assert!(app.open_panel(
        app.sole_window(),
        Box::new(TerminalView::new("test-term:s".to_owned()))
    ));
    let _ = crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    session.output(
        b"$ cargo test -p terminal\r\n\
\x1b[1;32m   Compiling\x1b[0m terminal v0.1.0\r\n\
\x1b[1;36m    Finished\x1b[0m dev profile in 0.42s\r\n\
\x1b[7m inverse \x1b[0m \x1b[33myellow\x1b[0m \x1b[34mblue\x1b[0m \x1b[45m magenta bg \x1b[0m\r\n\
$ \xf0\x9f\x91\xbb wide \xe6\xbc\xa2\xe5\xad\x97 chars\r\n$ ",
    );
    let _ = crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let _ = crate::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let image = surface.image_snapshot();
    let data = image
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png encode");
    std::fs::write(dir.join("terminal-panel.png"), data.as_bytes()).expect("write screenshot");
}
