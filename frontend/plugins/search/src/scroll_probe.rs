use super::*;
use himark::AppExt;
use himark::{test_document::plain_document, AppFonts, Application};
use std::sync::{mpsc, Arc};

fn test_theme() -> himark::Theme {
    himark::Theme::embedded()
}

#[test]
fn scrolling_search_results_probe() {
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

    let sample = "# section needle\n\n            | alpha | needle beta |\n|---|---|\n| needle one | two |\n\n            plain needle prose here\n\n            ```rust\nlet needle = 42;\n```\n\n"
        .repeat(12);
    let markdown_fonts = himark::embedded_fonts::source()();
    for index in 0..6 {
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
    assert!(himark::test_driver::type_text(&mut app, "needle"));
    runner.run();
    while let Ok(command) = arriving.try_recv() {
        app.perform_batch(vec![command]);
    }
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);

    let mut unsettled = 0;
    for _ in 0..30 {
        if himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size) {
            unsettled += 1;
        }
    }
    eprintln!("[probe] unsettled mount frames at rest: {unsettled}/30");

    let mut scrolls = Vec::new();
    let mut paints = Vec::new();
    for _ in 0..300 {
        let started = std::time::Instant::now();
        if !himark::test_driver::scroll(&mut app, 48.0) {
            break;
        }
        scrolls.push(started.elapsed());
        paints.push(himark::Window::draw_profiled(
            app.sole_window(),
            &mut app,
            surface.canvas(),
            size,
        ));
    }
    let p = |mut samples: Vec<std::time::Duration>| {
        if samples.is_empty() {
            return (0.0, 0.0);
        }
        samples.sort();
        (
            samples[samples.len() / 2].as_secs_f64() * 1000.0,
            samples[samples.len() * 95 / 100].as_secs_f64() * 1000.0,
        )
    };
    let frames = scrolls.len();
    let (s50, s95) = p(scrolls);
    let (p50, p95) = p(paints);
    eprintln!(
        "[probe] search scroll: frames={frames} scroll p50={s50:.3} p95={s95:.3} | paint p50={p50:.3} p95={p95:.3}"
    );
    imba::perf::record("search-scroll", "paint_p50_ms", p50);
    imba::perf::record("search-scroll", "paint_p95_ms", p95);
}
