#![allow(unused_imports)]
use super::*;

#[test]
fn find_bar_rescans_in_the_background_after_document_edits() {
    use himark::app::AppFonts;
    use himark::app::Application;
    use std::sync::{mpsc, Arc};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let settle = |app: &mut Application| {
        for _ in 0..3 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
        }
    };

    assert!(himark::test_driver::type_text(
        &mut app,
        "alpha one\nalpha two\n"
    ));
    assert!(app.perform_registered(window, "find.open"));
    assert!(himark::test_driver::type_text(&mut app, "alpha"));
    settle(&mut app);

    let matches_now = |app: &Application| -> Vec<std::ops::Range<u32>> {
        let (document, editor) = app.focused_editor_id();
        documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), document)
            .and_then(|document| document.find(editor).map(|find| find.matches().to_vec()))
            .expect("the bar is open")
    };
    assert_eq!(matches_now(&app), vec![0..5, 10..15]);

    {
        let (document_id, editor) = app.focused_editor_id();
        let documents = app.sole_documents();
        let mut document =
            documents::OpenDocuments::document(app.store(), documents, document_id)
                .expect("the document");
        document.find_mut(editor).expect("the bar is open").focused = false;
        documents::OpenDocuments::put_document(
            &mut app.store_mut(),
            documents,
            document_id,
            document,
        );
    }

    assert!(himark::test_driver::type_text(&mut app, "alpha"));
    settle(&mut app);
    assert_eq!(matches_now(&app), vec![0..5, 10..15, 20..25]);

    for _ in 0..5 {
        assert!(himark::test_driver::key(
            &mut app,
            imba::event::Key::Backspace,
            Default::default()
        ));
    }
    settle(&mut app);
    assert_eq!(matches_now(&app), vec![0..5, 10..15]);
}

#[test]
fn find_bar_highlights_and_walks_occurrences() {
    use himark::app::AppFonts;
    use himark::app::Application;
    use std::sync::{mpsc, Arc};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    assert!(himark::test_driver::type_text(
        &mut app,
        "alpha beta\nalpha gamma\nALPHA tail\n"
    ));
    let text_before = app.focused_document_text();

    assert!(app.perform_registered(window, "find.open"));
    assert!(himark::test_driver::type_text(&mut app, "alpha"));

    for _ in 0..3 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }

    assert_eq!(
        app.focused_document_text(),
        text_before,
        "typing lands in the bar's input"
    );

    app.draw_window(app.sole_window(), surface.canvas());
    let find = |app: &Application| -> Option<::editor::find::FindBar> {
        let (document, editor) = app.focused_editor_id();
        documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), document)
            .and_then(|document| document.find(editor).cloned())
    };
    {
        let find = find(&app).expect("the bar is open");
        assert_eq!(find.query(), "alpha");

        assert_eq!(find.matches().len(), 3, "{:?}", find.matches());
    }

    let (document_id, editor) = app.focused_editor_id();
    let document =
        documents::OpenDocuments::document(app.store(), app.sole_documents(), document_id)
            .expect("the document");
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    let extras = document.extras_keyed(editor);
    ::editor::markup::OverlaidMarkup::new(document.markup(), &extras).marks_inline_hidden_in(
        0..12,
        &mut inline,
        &mut hidden,
    );
    assert!(
        inline
            .iter()
            .any(|interval| interval.id == ::editor::theme::StyleId::Match),
        "occurrences tint like the global search"
    );
    drop(document);

    assert!(app.perform_registered(window, "find.next"));
    let selection = |app: &Application| {
        let document =
            documents::OpenDocuments::document(app.store(), app.sole_documents(), document_id)
                .expect("the document");
        let caret = document.carets(editor).primary();
        caret.selection()
    };
    assert_eq!(selection(&app), 0..5, "the first occurrence is selected");
    assert!(app.perform_registered(window, "find.next"));
    assert_eq!(selection(&app), 11..16, "the walk moves forward");
    assert!(app.perform_registered(window, "find.previous"));
    assert_eq!(selection(&app), 0..5, "and back");

    assert!(himark::test_driver::key(
        &mut app,
        imba::event::Key::Escape,
        imba::event::Modifiers::default()
    ));
    assert!(find(&app).is_none(), "Escape closed the bar");
    let document =
        documents::OpenDocuments::document(app.store(), app.sole_documents(), document_id)
            .expect("the document");
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    let extras = document.extras_keyed(editor);
    ::editor::markup::OverlaidMarkup::new(document.markup(), &extras).marks_inline_hidden_in(
        0..12,
        &mut inline,
        &mut hidden,
    );
    assert!(
        inline
            .iter()
            .all(|interval| interval.id != ::editor::theme::StyleId::Match),
        "the tints unwound with the bar"
    );

    {
        let mut document =
            documents::OpenDocuments::document(app.store(), app.sole_documents(), document_id)
                .expect("the document");
        document.set_carets(
            editor,
            ::editor::caret::MultiCaret::one(::editor::caret::Caret::selecting(6, 10)),
        );
        let documents = app.sole_documents();
        documents::OpenDocuments::put_document(
            &mut app.store_mut(),
            documents,
            document_id,
            document,
        );
    }
    assert!(app.perform_registered(window, "find.open"));

    for _ in 0..3 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }
    {
        let find = find(&app).expect("re-opened");
        assert_eq!(find.query(), "beta", "the selection seeded the query");
        assert_eq!(find.matches().len(), 1);
    }
}

#[test]
fn keymap_backspace_edits_the_find_bar_query() {
    use himark::app::AppFonts;
    use himark::app::Application;
    use imba::event::{Key, Modifiers};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    assert!(himark::test_driver::type_text(&mut app, "hello"));
    let cmd = Modifiers {
        command: true,
        ..Default::default()
    };
    assert!(
        himark::test_driver::key(&mut app, Key::Char('f'), cmd),
        "cmd-F opens the find bar"
    );
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(himark::test_driver::type_text(&mut app, "ab"));

    let query = |app: &Application| -> Option<String> {
        let (document, editor) = app.focused_editor_id();
        documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), document)?
            .find(editor)
            .map(|find| find.query())
    };
    assert_eq!(
        query(&app).as_deref(),
        Some("ab"),
        "the query took the typing"
    );
    assert!(
        himark::test_driver::backspace(&mut app),
        "backspace resolves through the keymap"
    );
    assert_eq!(query(&app).as_deref(), Some("a"), "the QUERY shrank");
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("hello"),
        "the document behind the bar never moved"
    );
}
