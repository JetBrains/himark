// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use himark::test_document::plain_document;
use himark::{AppExt, AppFonts, Application};

#[derive(Clone)]
struct StubWidget(&'static str);

impl imba::View for StubWidget {
    type Command = imba::DynCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        _store: &'a Store,
        _ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            leaf(constraints.max.width, constraints.max.height)
        })
    }
}

impl himark::PanelView for StubWidget {
    type Place = himark::NoPlace;
    fn title(&self, _store: &Store) -> String {
        self.0.to_owned()
    }
    fn dismantle(&mut self, _store: &mut Store) {}
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn mounted_titles(app: &Application) -> Vec<String> {
    let mut titles = Vec::new();
    app.for_each_plugin_panel(&mut |panel| titles.push(panel.title(app.store())));
    titles
}

/// A PTY-less terminal session: the cheap seedable family row now
/// that result lists live in the session feed, not a family.
fn seed_terminal_row(app: &mut Application, channel: &str, title: &str) -> String {
    struct NullBackend;
    impl himark::terminal::TerminalBackend for NullBackend {
        fn write(&self, _bytes: &[u8]) {}
        fn resize(&self, _cols: u16, _rows: u16, _w: f32, _h: f32) {}
        fn hangup(&self) {}
    }
    let session = himark::terminal::Session::new(Box::new(NullBackend));
    session.set_channel(channel.to_owned());
    let _ = session.output(format!("\x1b]0;{title}\x07").as_bytes());
    himark::terminal::Terminals::put(&mut app.store_mut(), channel.to_owned(), session);
    channel.to_owned()
}

#[test]
fn displaced_handles_drop_and_their_rows_survive() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let alpha = seed_terminal_row(&mut app, "test-terminal:alpha", "alpha results");
    assert!(app.open_panel(
        app.sole_window(),
        Box::new(himark::terminal::TerminalView::new(alpha.clone()))
    ));
    assert!(app.open_panel(app.sole_window(), Box::new(StubWidget("beta widget"))));
    assert_eq!(mounted_titles(&app), vec!["beta widget"]);
    assert!(
        himark::terminal::Terminals::session_ref(app.store(), &alpha).is_some(),
        "the displaced handle dropped; the family row survived"
    );
}

#[test]
fn the_peeker_lists_previews_and_selects_widgets() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    app.register_command(std::sync::Arc::new(TogglePeeker));
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(app.add_document(
        app.sole_window(),
        plain_document("alpha body"),
        "alpha-doc".to_owned(),
        false
    ));
    let alpha = seed_terminal_row(&mut app, "test-terminal:alpha", "alpha results");
    assert!(app.open_panel(app.sole_window(), Box::new(StubWidget("beta widget"))));

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let listed = labels(&app).expect("the peeker is up");
    assert!(
        listed.contains(&"beta widget".to_owned()) && listed.contains(&"alpha results".to_owned()),
        "held widgets and family rows list beside documents: {listed:?}"
    );
    assert!(himark::test_driver::key(
        &mut app,
        imba::event::Key::Escape,
        Default::default()
    ));
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert_eq!(mounted_titles(&app), vec!["beta widget"], "pane restored");
    assert!(
        himark::terminal::Terminals::session_ref(app.store(), &alpha).is_some(),
        "the row survived the dismissal"
    );

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(himark::test_driver::type_text(&mut app, "alpha results"));
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let listed = labels(&app).expect("still up");
    assert_eq!(listed, vec!["alpha results"], "the filter narrowed to it");
    assert_eq!(
        previewed_widget(&app).as_deref(),
        Some("alpha results"),
        "the preview MOUNTED the minted handle"
    );
    assert!(himark::test_driver::key(
        &mut app,
        imba::event::Key::Enter,
        Default::default()
    ));
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(labels(&app).is_none(), "picking closes the peeker");
    assert_eq!(
        mounted_titles(&app),
        vec!["alpha results"],
        "the picked handle took the focused pane"
    );
}
