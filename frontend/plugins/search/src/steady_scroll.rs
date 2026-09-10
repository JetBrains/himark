use super::*;
use himark::AppExt;
use himark::{test_document::plain_document, AppFonts, Application};
use std::sync::{mpsc, Arc};

fn test_theme() -> himark::Theme {
    himark::Theme::embedded()
}

#[test]
fn scrolling_steady_results_reveals_rows_without_reconciling() {
    let fonts = { AppFonts::embedded() };
    let mut app = Application::new(fonts);
    register_handlers(&mut app);
    let _ = app.add_window();
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let sample = "# section needle\n\n            | alpha | needle beta |\n|---|---|\n| needle one | two |\n\n            plain needle prose here\n\n"
        .repeat(10);
    let markdown_fonts = himark::embedded_fonts::source()();
    for index in 0..8 {
        assert!(app.add_document(
            app.sole_window(),
            himarkdown::document_from_markdown(&sample, &markdown_fonts, &test_theme()),
            format!("doc-{index}.md"),
            false
        ));
    }
    let _ = plain_document;
    let size = skia_safe::Size::new(900.0, 700.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    assert!(app.open_panel(app.sole_window(), Box::new(SearchView::new())));
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    assert!(himark::test_driver::type_text(&mut app, "needle"));
    runner.run();
    while let Ok(command) = arriving.try_recv() {
        app.perform_batch(vec![command]);
    }

    for _ in 0..4 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    }

    let mut reconciled = 0;
    let mut steps = 0;
    for _ in 0..300 {
        if !himark::test_driver::scroll(&mut app, 96.0) {
            break;
        }
        steps += 1;
        if himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size) {
            reconciled += 1;
        }
    }
    assert!(steps > 5, "the result list must be long enough to scroll");
    assert_eq!(
        reconciled, 0,
        "revealing rows of a steady result set must not reconcile              ({reconciled}/{steps} scroll frames did)"
    );
}
