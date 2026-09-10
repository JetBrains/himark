use super::*;
use himark::AppExt;
use himark::{test_document::plain_document, AppFonts, Application};
use std::sync::{mpsc, Arc};

#[test]
fn a_broad_landing_installs_within_a_keystroke_budget() {
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

    for index in 0..12 {
        let body = "prefix needle suffix\nanother needle line\n".repeat(40);
        assert!(app.add_document(
            app.sole_window(),
            plain_document(&body),
            format!("doc-{index}"),
            false
        ));
    }

    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(app.open_panel(app.sole_window(), Box::new(SearchView::new())));
    let _ = himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    assert!(himark::test_driver::type_text(&mut app, "needle"));
    runner.run();
    while let Ok(command) = arriving.try_recv() {
        app.perform_batch(vec![command]);
    }

    assert!(himark::test_driver::backspace(&mut app));
    runner.run();
    let mut landings = Vec::new();
    while let Ok(command) = arriving.try_recv() {
        let started = std::time::Instant::now();
        app.perform_batch(vec![command]);
        landings.push(started.elapsed());
    }
    let worst = landings.iter().copied().max().unwrap_or_default();
    imba::perf::record("search-landing", "worst_ms", worst.as_secs_f64() * 1000.0);

    assert!(
        worst < std::time::Duration::from_millis(75),
        "a landing must apply within a keystroke budget: worst {worst:?} of {:?}",
        landings
    );
}
