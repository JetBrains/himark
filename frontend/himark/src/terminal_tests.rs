// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The PANE's tests — the keymap, the grid reconcile, the dismantle
//! road. The emulator's own tests live with the `terminals` crate.

use std::sync::{Arc, Mutex};

use ::terminals::pane::*;
use ::terminals::*;
use crate::app_ext::AppExt;
use hikit::panel::PanelView;
use alacritty_terminal::grid::Dimensions;
use imba::event::Modifiers;
use imba::store::Store;

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

#[test]
fn dismantle_hangs_up_exactly_once() {
    let recorder = Recorder::default();
    let session = Session::new(Box::new(recorder.clone()));
    let mut store = Store::new();
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("test-session:1"),
    };
    let terminals = ahp_session::session::state::Hosts::ensure_state(&mut store, &home).terminals();
    let id = TerminalId::mint();
    Terminals::put(&mut store, terminals, id, session.clone());
    let mut panel = TerminalView::new(terminals, id);
    PanelView::dismantle(&mut panel, &mut store);
    session.hangup();
    assert_eq!(*recorder.hangups.lock().unwrap(), 1);
    assert!(
        Terminals::list(&store, terminals).is_empty(),
        "the row left the state"
    );
}

#[test]
fn the_panel_reconciles_its_grid_and_routes_focused_input() {
    let mut app = crate::app::Application::new(crate::app::AppFonts::embedded());
    let _ = app.add_window();
    let recorder = Recorder::default();
    let session = Session::new(Box::new(recorder.clone()));

    let mut surface = skia_safe::surfaces::raster_n32_premul((1600, 900)).expect("surface");
    assert!(app.new_scratch(app.sole_window()));
    let _ = app.draw_window(app.sole_window(), surface.canvas());
    let home = app.sole_window_session();
    let terminals = ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).terminals();
    let id = TerminalId::mint();
    Terminals::put(&mut app.store_mut(), terminals, id, session.clone());
    assert!(app.open_panel(
        app.sole_window(),
        Box::new(TerminalView::new(terminals, id))
    ));
    session.output(b"$ echo himark\r\n\x1b[32mhimark\x1b[0m\r\n$ ");

    let _ = app.draw_window(app.sole_window(), surface.canvas());
    let _ = app.draw_window(app.sole_window(), surface.canvas());
    let resizes = recorder.resizes.lock().unwrap().clone();
    assert_eq!(
        resizes.len(),
        1,
        "one resize to the laid-out grid: {resizes:?}"
    );
    let (cols, rows) = resizes[0];
    assert_eq!(session.term().lock().columns(), cols as usize);
    assert_eq!(session.term().lock().screen_lines(), rows as usize);
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
    let mut app = crate::app::Application::new(crate::app::AppFonts::embedded());
    let _ = app.add_window();
    let session = Session::new(Box::new(Recorder::default()));
    assert!(app.new_scratch(app.sole_window()));
    let mut surface = skia_safe::surfaces::raster_n32_premul((1600, 900)).expect("surface");
    let _ = app.draw_window(app.sole_window(), surface.canvas());
    let home = app.sole_window_session();
    let terminals = ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).terminals();
    let id = TerminalId::mint();
    Terminals::put(&mut app.store_mut(), terminals, id, session.clone());
    assert!(app.open_panel(
        app.sole_window(),
        Box::new(TerminalView::new(terminals, id))
    ));
    let _ = app.draw_window(app.sole_window(), surface.canvas());
    session.output(
        b"$ cargo test -p terminal\r\n\
\x1b[1;32m   Compiling\x1b[0m terminal v0.1.0\r\n\
\x1b[1;36m    Finished\x1b[0m dev profile in 0.42s\r\n\
\x1b[7m inverse \x1b[0m \x1b[33myellow\x1b[0m \x1b[34mblue\x1b[0m \x1b[45m magenta bg \x1b[0m\r\n\
$ \xf0\x9f\x91\xbb wide \xe6\xbc\xa2\xe5\xad\x97 chars\r\n$ ",
    );
    let _ = app.draw_window(app.sole_window(), surface.canvas());
    let _ = app.draw_window(app.sole_window(), surface.canvas());
    let image = surface.image_snapshot();
    let data = image
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png encode");
    std::fs::write(dir.join("terminal-panel.png"), data.as_bytes()).expect("write screenshot");
}
