#![allow(unused_imports)]
use super::*;

#[test]
fn two_windows_edit_independently() {
    use himark::app::AppFonts;
    use himark::app::Application;
    use imba::event::Event;
    use skia_safe::Size;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let first = app.add_window();
    let second = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window_sized(first, surface.canvas(), Size::new(800.0, 600.0));
    app.draw_window_sized(second, surface.canvas(), Size::new(500.0, 400.0));

    let focused_text =
        |app: &Application, window: ::workbench::window::WindowId| -> Option<String> {
            let store = &app.window_store(window);
            let session = himark::workspace::window_session(store, window)?;
            let documents = ahp_session::session::state::Hosts::state(store, &session)?.documents();
            let document = documents::OpenDocuments::document_ref(
                store,
                documents,
                ::workbench::window::Windows::window_ref(store, window)?.focused_document_id()?,
            )?;
            let end = document.text().byte_count().min(u32::MAX as usize) as u32;
            Some(document.text().view().substring(0..end))
        };

    assert!(app.dispatch_timed(
        first,
        Event::TextInput { text: "one" },
        Size::new(800.0, 600.0),
        0.0,
    ));
    assert!(app.dispatch_timed(
        second,
        Event::TextInput { text: "two" },
        Size::new(500.0, 400.0),
        0.0,
    ));
    assert_eq!(focused_text(&app, first).as_deref(), Some("one"));
    assert_eq!(focused_text(&app, second).as_deref(), Some("two"));

    let viewport = |app: &Application, window: ::workbench::window::WindowId| {
        app.window_viewport(window).expect("the window entity")
    };
    assert_eq!(viewport(&app, first).width, 800.0);
    assert_eq!(viewport(&app, second).width, 500.0);

    assert!(app.perform_registered(second, "workbench.split-pane"));
    let panes = |app: &Application, window: ::workbench::window::WindowId| {
        let mut count = 0;
        let store = app.window_store(window);
        ::workbench::window::Windows::window_ref(&store, window)
            .expect("the window entity")
            .workbench()
            .root
            .for_each_pane(&mut |_| count += 1);
        count
    };
    assert_eq!(panes(&app, second), 2, "the second window split");
    assert_eq!(panes(&app, first), 1, "the first window did not");
}

#[test]
fn closing_a_split_pane_collapses_to_the_sibling() {
    use himark::app::AppFonts;
    use himark::app::Application;
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let start = app.pane_count();

    assert!(
        app.perform_registered(app.sole_window(), "workbench.split-pane"),
        "splitting the focused pane"
    );
    let _ = app.draw_window(app.sole_window(), surface.canvas());
    assert_eq!(app.pane_count(), start + 1);

    assert!(
        app.perform_registered(app.sole_window(), "workbench.close-pane"),
        "closing the focused pane"
    );
    let _ = app.draw_window(app.sole_window(), surface.canvas());
    assert_eq!(
        app.pane_count(),
        start,
        "the split collapsed to its sibling"
    );
    while app.pane_count() > 1 {
        assert!(
            app.perform_registered(app.sole_window(), "workbench.close-pane"),
            "closing down to one pane"
        );
        let _ = app.draw_window(app.sole_window(), surface.canvas());
    }
    let _ = app.perform_registered(app.sole_window(), "workbench.close-pane");
    assert_eq!(app.pane_count(), 1, "the last pane refuses to close");
    assert!(
        himark::test_driver::type_text(&mut app, "x"),
        "the surviving pane still types"
    );
}

#[test]
fn open_documents_list_recently_opened_first() {
    use documents::OpenDocuments;
    let mut store = Store::new();
    let alpha = OpenDocuments::register(
        &mut store,
        test_docs(),
        plain_document("alpha"),
        None,
        "alpha".to_owned(),
        0,
    );
    let _beta = OpenDocuments::register(
        &mut store,
        test_docs(),
        plain_document("beta"),
        None,
        "beta".to_owned(),
        0,
    );

    let recent_names = |store: &Store| -> Vec<String> {
        OpenDocuments::list_recent(store, test_docs())
            .into_iter()
            .map(|(_, info)| info.name())
            .collect()
    };
    assert_eq!(recent_names(&store), ["beta", "alpha"]);

    OpenDocuments::touch(&mut store, test_docs(), alpha);
    assert_eq!(recent_names(&store), ["alpha", "beta"]);

    let open_order: Vec<String> = OpenDocuments::list(&store, test_docs())
        .into_iter()
        .map(|(_, info)| info.name())
        .collect();
    assert_eq!(open_order, ["alpha", "beta"], "open order stays stable");
}

#[test]
fn navigation_back_and_forward_walk_pane_history() {
    use himark::app::AppFonts;
    use himark::app::Application;
    use himark::app::OpenedDocument;
    fn located(name: &str) -> editor::location::ResourceLocation {
        editor::location::ResourceLocation::new(
            editor::location::ResourceType::document(),
            editor::location::Authority::new("test"),
            vec!["project".to_owned(), name.to_owned()],
        )
    }
    fn editor_count(app: &Application, location: &editor::location::ResourceLocation) -> usize {
        let id = documents::OpenDocuments::by_location(app.store(), app.sole_documents(), location)
            .expect("registered");
        documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), id)
            .expect("document")
            .editor_ids()
            .count()
    }

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();

    let open = |app: &mut Application, name: &str, text: &str| {
        let document = plain_document(text);
        assert!(app.perform_command(himark::app::AppCommand::Opened(
            window,
            OpenedDocument {
                documents: app.sole_documents(),
                name: name.to_owned(),
                document,
                location: Some(located(name)),
                primary: true,
                target: None,
                focus: false,
            },
        )));
    };
    open(&mut app, "a.md", "alpha\n");
    assert_eq!(app.focused_document_text().as_deref(), Some("alpha\n"));

    assert!(himark::test_driver::type_text(&mut app, "x"));
    assert_eq!(app.focused_document_text().as_deref(), Some("xalpha\n"));

    open(&mut app, "b.md", "beta\n");
    assert_eq!(app.focused_document_text().as_deref(), Some("beta\n"));
    assert!(himark::test_driver::type_text(&mut app, "y"));
    assert_eq!(
        editor_count(&app, &located("a.md")),
        0,
        "displacement retracted the editor; dirty, the document stays"
    );

    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("xalpha\n"),
        "back returns to the recorded place — an instant remount, edits intact"
    );
    assert_eq!(editor_count(&app, &located("a.md")), 1);
    assert_eq!(
        editor_count(&app, &located("b.md")),
        0,
        "b displaced in turn — retracted, dirty, kept"
    );

    assert!(app.perform_registered(window, "navigation.forward"));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("ybeta\n"),
        "forward walks up again"
    );

    assert!(app.perform_registered(window, "navigation.forward"));
    assert_eq!(app.focused_document_text().as_deref(), Some("ybeta\n"));

    let caret = |app: &Application| -> u32 {
        let (document, editor) = app.focused_editor_id();
        documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), document)
            .expect("document")
            .caret_byte(editor)
    };
    let before_jump = caret(&app);
    assert!(app.perform_command(himark::app::AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "b.md".to_owned(),
            document: plain_document("beta\n"),
            location: Some(located("b.md")),
            primary: true,
            target: Some(
                documents::text_ext::LineCol { line: 0, col: 3 }..documents::text_ext::LineCol {
                    line: 0,
                    col: 4
                }
            ),
            focus: false,
        },
    )));
    assert_eq!(app.focused_document_text().as_deref(), Some("ybeta\n"));
    assert_eq!(caret(&app), 3, "the jump moved the caret in place");
    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(
        caret(&app),
        before_jump,
        "back returns to where the same-file jump left"
    );
}

#[test]
fn close_widget_walks_the_pane_history() {
    use himark::app::AppFonts;
    use himark::app::Application;
    use himark::app::OpenedDocument;
    fn located(name: &str) -> editor::location::ResourceLocation {
        editor::location::ResourceLocation::new(
            editor::location::ResourceType::document(),
            editor::location::Authority::new("test"),
            vec!["project".to_owned(), name.to_owned()],
        )
    }
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    let open = |app: &mut Application, name: &str, text: &str| {
        assert!(app.perform_command(himark::app::AppCommand::Opened(
            window,
            OpenedDocument {
                documents: app.sole_documents(),
                name: name.to_owned(),
                document: plain_document(text),
                location: Some(located(name)),
                primary: true,
                target: None,
                focus: false,
            },
        )));
    };

    open(&mut app, "a.md", "alpha\n");
    assert!(himark::test_driver::type_text(&mut app, "x"));
    open(&mut app, "b.md", "beta\n");
    assert_eq!(
        {
            let session = app.sole_window_session();
            ahp_session::session::state::Hosts::state(app.store(), &session)
                .map(|state| recents::RecentLocations::list(app.store(), state.recents()))
                .unwrap_or_default()
        }[..2],
        [located("b.md"), located("a.md")],
        "every visited place on the recents list, newest first (the startup scratch behind)"
    );

    assert!(app.perform_registered(window, "workbench.close"));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("xalpha\n"),
        "the last place in the pane's history took over"
    );
    assert!(
        documents::OpenDocuments::by_location(app.store(), app.sole_documents(), &located("b.md"))
            .is_none(),
        "the closed clean document left the registry"
    );

    assert!(app.perform_registered(window, "workbench.close"));
    assert!(
        app.focused_document_text().is_none(),
        "history spent — the blank stays (a pristine scratch is not a place)"
    );
    let a =
        documents::OpenDocuments::by_location(app.store(), app.sole_documents(), &located("a.md"))
            .expect("dirty: unsaved edits are never released");
    assert_eq!(
        documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), a)
            .expect("document")
            .editor_ids()
            .count(),
        0
    );
    assert!(
        documents::OpenDocuments::list(app.store(), app.sole_documents())
            .iter()
            .any(|(_, entity)| entity.location().is_some_and(documents::is_scratch)),
        "the startup scratch survives the walk-past — listed, not lost"
    );

    assert!(app.perform_registered(window, "workbench.close"));
    assert!(app.focused_document_text().is_none());
    let recents = {
        let session = app.sole_window_session();
        ahp_session::session::state::Hosts::state(app.store(), &session)
            .map(|state| recents::RecentLocations::list(app.store(), state.recents()))
            .unwrap_or_default()
    };
    assert!(
        recents.contains(&located("a.md")) && recents.contains(&located("b.md")),
        "closing forgets documents, never places: {recents:?}"
    );
}

#[test]
fn scroll_stripes_follow_the_diff_through_the_app() {
    let ui = ::editor::test_document::test_ui();
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
        for _ in 0..5 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
        }
    };

    assert!(app.add_document(
        app.sole_window(),
        plain_document("one\ntwo\nthree\n"),
        "target.md".to_owned(),
        true
    ));
    let (target, editor) = app.focused_editor_id();
    let documents = app.sole_documents();
    let base = documents::OpenDocuments::register(
        &mut app.store_mut(),
        documents,
        plain_document("one\nTWO\nthree\n"),
        None,
        "base.md".to_owned(),
        0,
    );
    let documents = app.sole_documents();
    let diff =
        documents::OpenDocuments::track_diff(&mut app.store_mut(), documents, base, target, true)
            .expect("tracked");
    let _ = window;

    // A benign entity command: deliver drops it, the batch tails run.
    let home = app.sole_window_session();
    let documents =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).documents();
    let tick = move |app: &mut Application| {
        app.perform_batch(vec![himark::app::AppCommand::at(
            documents,
            documents::DocumentsCommand::Editor(target, EditorCommand::ApplyRepair(Vec::new())),
        )]);
    };
    let stripes = |app: &Application| -> Vec<u32> {
        documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), target)
            .and_then(|document| document.scroll_stripes(editor))
            .map(|landed| landed.segments.iter().map(|segment| segment.byte).collect())
            .unwrap_or_default()
    };

    tick(&mut app);
    settle(&mut app);
    let born = stripes(&app);
    assert!(!born.is_empty(), "the tracked hunk projects onto the track");

    // The diff CHANGES: the first line gains its own hunk.
    {
        let fonts = ::editor::env::Fonts::of(app.store())();
        let theme = ::editor::env::Themes::of(app.store());
        let documents = app.sole_documents();
        let mut store = app.store_mut();
        let mut document =
            documents::OpenDocuments::document(&store, documents, target).expect("open");
        let len = document.text().byte_count().min(u32::MAX as usize) as u32;
        document.edit(
            &::operation::operation::Operation::insert_in(len, 0, "zero\n"),
            &store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
        documents::OpenDocuments::put_document(&mut store, documents, target, document);
    }
    tick(&mut app);
    settle(&mut app);
    let moved = stripes(&app);
    assert_ne!(moved, born, "an edit that moves the hunks moves the track");
    assert!(
        moved.contains(&0),
        "the typed line's own hunk lands on the track after the normalize: {moved:?}"
    );

    // The COMMIT road: the base catches up with the target — the diff
    // empties and the track must clear.
    {
        let fonts = ::editor::env::Fonts::of(app.store())();
        let theme = ::editor::env::Themes::of(app.store());
        let target_text =
            documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), target)
                .map(|document| {
                    let mut view = document.text().view();
                    let count = view.byte_count();
                    view.byte_string(0, count)
                })
                .expect("open");
        let documents = app.sole_documents();
        let mut store = app.store_mut();
        let mut document =
            documents::OpenDocuments::document(&store, documents, base).expect("open");
        let catch_up = myersdiff::diff(
            document.text(),
            &::text::text::Text::from_string_exact(&target_text),
        );
        document.edit(
            &catch_up,
            &store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
        documents::OpenDocuments::put_document(&mut store, documents, base, document);
    }
    tick(&mut app);
    settle(&mut app);
    assert_eq!(
        stripes(&app),
        Vec::<u32>::new(),
        "the committed diff leaves no marks on the track"
    );

    // The REAL commit road: HEAD moves, the base ask answers a NEW
    // location — adopt unTRACKS the old diff and retracks against the
    // fresh base. First give the track marks again...
    {
        let fonts = ::editor::env::Fonts::of(app.store())();
        let theme = ::editor::env::Themes::of(app.store());
        let mut store = app.store_mut();
        let mut document =
            documents::OpenDocuments::document(&store, documents, base).expect("open");
        let len = document.text().byte_count().min(u32::MAX as usize) as u32;
        document.edit(
            &::operation::operation::Operation::insert_in(len, 0, "gone\n"),
            &store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
        documents::OpenDocuments::put_document(&mut store, documents, base, document);
    }
    tick(&mut app);
    settle(&mut app);
    assert!(
        !stripes(&app).is_empty(),
        "the diverged base marks the track again"
    );

    // ...then land the new HEAD: a base equal to the target's text,
    // under its own location.
    let target_text =
        documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), target)
            .map(|document| {
                let mut view = document.text().view();
                let count = view.byte_count();
                view.byte_string(0, count)
            })
            .expect("open");
    let head = ::editor::location::ResourceLocation::new(
        ::editor::location::ResourceType::document(),
        ::editor::location::Authority::new("local"),
        vec!["head-v2.md".to_owned()],
    );
    let documents = app.sole_documents();
    let _head_id = documents::OpenDocuments::register(
        &mut app.store_mut(),
        documents,
        plain_document(&target_text),
        Some(head.clone()),
        "head-v2.md".to_owned(),
        0,
    );
    {
        let home = app.sole_window_session();
        let documents =
            ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home)
                .documents();
        app.perform_batch(vec![himark::app::AppCommand::at(
            documents,
            documents::DocumentsCommand::BaseLocated {
                document: target,
                base: Some(head),
            },
        )]);
    }
    settle(&mut app);
    assert_eq!(
        stripes(&app),
        Vec::<u32>::new(),
        "the retracked (empty) diff clears the track"
    );

    // And a fresh edit AFTER the retrack stripes again — through the
    // real typing road.
    assert!(himark::test_driver::type_text(&mut app, "typed"));
    settle(&mut app);
    assert!(
        !stripes(&app).is_empty(),
        "typing into the retracked pane marks the track"
    );

    // A split-diff PANEL tracks its own diff on the same document
    // (stripes=false). Its hunks must NOT leak onto the pane's track:
    // when the pane's own stripes diff clears, the track clears too,
    // panel or no panel.
    let documents = app.sole_documents();
    let snapshot = documents::OpenDocuments::register(
        &mut app.store_mut(),
        documents,
        plain_document(
            "something
entirely
unrelated
",
        ),
        None,
        "panel-base.md".to_owned(),
        0,
    );
    let panel_diff = documents::OpenDocuments::track_diff(
        &mut app.store_mut(),
        documents,
        snapshot,
        target,
        false,
    )
    .expect("the panel road tracks");
    // The pane's own diff empties: HEAD catches up again.
    let target_text =
        documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), target)
            .map(|document| {
                let mut view = document.text().view();
                let count = view.byte_count();
                view.byte_string(0, count)
            })
            .expect("open");
    let head3 = ::editor::location::ResourceLocation::new(
        ::editor::location::ResourceType::document(),
        ::editor::location::Authority::new("local"),
        vec!["head-v3.md".to_owned()],
    );
    let documents = app.sole_documents();
    let _ = documents::OpenDocuments::register(
        &mut app.store_mut(),
        documents,
        plain_document(&target_text),
        Some(head3.clone()),
        "head-v3.md".to_owned(),
        0,
    );
    {
        let home = app.sole_window_session();
        let documents =
            ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home)
                .documents();
        app.perform_batch(vec![himark::app::AppCommand::at(
            documents,
            documents::DocumentsCommand::BaseLocated {
                document: target,
                base: Some(head3),
            },
        )]);
    }
    settle(&mut app);
    assert_eq!(
        stripes(&app),
        Vec::<u32>::new(),
        "the committed pane clears its track even while a diff panel holds its own diff"
    );
    let _ = (diff, panel_diff);
}
