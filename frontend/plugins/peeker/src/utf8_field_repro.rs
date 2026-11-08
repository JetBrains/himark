// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

#[allow(unused_imports)]
use workbench_shell::app::WorkbenchShell as _;
#[allow(unused_imports)]
use workbench_shell::app_ext::AppExt as _;
#[cfg(any(test, feature = "test-support"))]
#[allow(unused_imports)]
use workbench_shell::test_support::TestSupport as _;

use super::*;
use himark::app::AppFonts;
use himark::app::Himark;
use workbench_shell::app_ext::AppExt;
use std::sync::{mpsc, Arc};

#[test]
fn browsing_the_peeker_over_the_docs_keeps_text_ranges_valid() {
    let fonts = AppFonts::embedded();
    let mut app = workbench_shell::app::workbench_app(fonts);
    let _ = app.add_window();
    app.register_command(std::sync::Arc::new(TogglePeeker));
    let mut languages = editor::reparse::SyntaxLanguages::new();
    hirust::register(&mut languages);
    app.register_syntax_languages(himarkdown::markdown_languages(languages));
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = editor::theme::embedded();
    for (name, source) in [
        (
            "search.md",
            include_str!("../../himarkdown/fixtures/utf8-repro.md"),
        ),
        (
            "commands.md",
            include_str!("../../himarkdown/fixtures/utf8-repro.md"),
        ),
        (
            "list-view.md",
            include_str!("../../himarkdown/fixtures/utf8-repro.md"),
        ),
    ] {
        assert!(app.add_document(
            app.sole_window(),
            himarkdown::document_from_markdown(source, app.store(), &app.ui_ctx(), &theme,),
            name.to_owned(),
            false,
        ));
    }

    let size = gfx::Size::new(1200.0, 900.0);
    let mut surface = gfx_skia::raster_tuple((1200, 900)).expect("surface");
    let settle = |app: &mut Himark| {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    };
    settle(&mut app);
    let _ = app.draw_window_sized(
        app.sole_window(),
        &gfx_skia::SkiaCanvas::new(surface.canvas()),
        size,
    );

    assert!(
        app.perform_registered(app.sole_window(), "peeker.toggle"),
        "the peeker opens"
    );
    let _ = app.draw_window_sized(
        app.sole_window(),
        &gfx_skia::SkiaCanvas::new(surface.canvas()),
        size,
    );

    for round in 0..40u32 {
        let key = match round % 5 {
            0 | 1 | 2 => imba::event::Key::Down,
            _ => imba::event::Key::Up,
        };
        let _ = workbench_shell::test_driver::key(&mut app, key, Default::default());
        let _ = app.draw_window_sized(
            app.sole_window(),
            &gfx_skia::SkiaCanvas::new(surface.canvas()),
            size,
        );
        if round % 3 == 0 {
            settle(&mut app);
        }
    }

    for _ in 0..8 {
        settle(&mut app);
        let _ = app.draw_window_sized(
            app.sole_window(),
            &gfx_skia::SkiaCanvas::new(surface.canvas()),
            size,
        );
    }

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    for _ in 0..12 {
        let _ = workbench_shell::test_driver::key(&mut app, imba::event::Key::Down, Default::default());
        let _ = app.draw_window_sized(
            app.sole_window(),
            &gfx_skia::SkiaCanvas::new(surface.canvas()),
            size,
        );
        settle(&mut app);
    }
}
