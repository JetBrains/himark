use super::*;
use himark::AppExt;
use himark::{AppFonts, Application};
use std::sync::{mpsc, Arc};

#[test]
fn searching_the_search_doc_and_editing_rows_keeps_boundaries() {
    let fonts = AppFonts::embedded();
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
    let source = include_str!("../../himarkdown/fixtures/utf8-repro.md");
    let markdown_fonts = himark::embedded_fonts::source()();

    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(source, &markdown_fonts, &himark::Theme::embedded()),
        "search.md".to_owned(),
        false,
    ));

    let size = skia_safe::Size::new(1200.0, 900.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1200, 900)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for _ in 0..6 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface);
    assert!(
        app.open_panel(app.sole_window(), Box::new(SearchView::new())),
        "search opens"
    );
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);

    for query in ["assemble", "bounded", "deliver"] {
        for ch in query.chars() {
            assert!(himark::test_driver::type_text(&mut app, &ch.to_string()));
        }
        settle(&mut app, &mut surface);

        himark::test_driver::click(&mut app, 200.0, 320.0, 1200.0, 900.0);
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
        let document_text = |app: &Application| -> String {
            let info = himark::OpenDocuments::list(app.store())
                .into_iter()
                .find(|(_, info)| info.name() == "search.md")
                .expect("the document is open");
            let document =
                himark::OpenDocuments::document_ref(app.store(), info.0).expect("document");
            let count = document.text().byte_count();
            document.text().view().byte_string(0, count)
        };
        let before = document_text(&app);
        for _ in 0..3 {
            let _ = himark::test_driver::type_text(&mut app, "—");
            settle(&mut app, &mut surface);
        }
        assert_ne!(
            document_text(&app),
            before,
            "the row edit must write through (the click hit a row)"
        );
        for _ in 0..6 {
            let _ = himark::test_driver::backspace(&mut app);
            settle(&mut app, &mut surface);
        }

        himark::test_driver::click(&mut app, 200.0, 30.0, 1200.0, 900.0);
        for _ in 0..query.len() + 1 {
            let _ = himark::test_driver::backspace(&mut app);
        }
        settle(&mut app, &mut surface);
    }
}
