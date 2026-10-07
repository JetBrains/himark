#![allow(unused_imports)]
use super::*;

#[test]
fn the_at_completion_opens_finds_and_picks() {
    use himark::app::AppFonts;
    use himark::app::Application;
    use himark::test_driver;
    use imba::event::{Key, Modifiers};

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();
    let folder = editor::location::ResourceLocation::new(
        editor::location::ResourceType::directory(),
        editor::location::Authority::new("test"),
        vec!["proj".to_owned()],
    );
    let session = himark::test_support::seed_session_folders(&mut app.store_mut(), &[folder]);

    struct StubFind(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    impl imba::effect::EffectHandler<ahp_locations::FindEffect> for StubFind {
        async fn handle(
            &self,
            effect: ahp_locations::FindEffect,
        ) -> Vec<editor::location::ResourceLocation> {
            self.0.lock().expect("terms").push(effect.term.clone());
            let file = |path: &[&str]| {
                editor::location::ResourceLocation::new(
                    editor::location::ResourceType::document(),
                    editor::location::Authority::new("test"),
                    path.iter().map(|s| s.to_string()).collect::<Vec<String>>(),
                )
            };
            vec![
                file(&["proj", "src", "main.rs"]),
                file(&["proj", "README.md"]),
            ]
        }
    }
    let terms = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    app.register_handler::<ahp_locations::FindEffect>(StubFind(std::sync::Arc::clone(&terms)));

    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        std::sync::Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        std::sync::Arc::new(|| {}),
    );

    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/completion");
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &session).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        session.host,
        session.session.clone(),
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    // Retire the launch scratch: at this width the chat is visible
    // only over a vacant tree, and the composer needs the screen.
    assert!(app.perform_registered(window, "workbench.close"));
    {
        let ui = app.ui_ctx();
        let mut store = app.store_mut();
        let mut entity = ::workbench::window::Windows::window(&store, window).expect("window");
        let mut batch = imba::effect::Batch::<himark::app::AppCommand>::new();
        let _ = entity.open_panel(
            &mut store,
            &ui,
            Box::new(ahp_chat::chats::ChatPane::new(chats, uri.clone())),
            &mut batch.effects(),
        );
        ::workbench::window::Windows::put(&mut store, window, entity);
    }
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    app.draw_window(window, surface.canvas());

    let panel = |app: &Application| -> ahp_chat::chat::ChatPanel {
        ahp_chat::chats::Chats::chat(app.store(), chats, &uri).expect("the panel")
    };
    let pump = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for tick in 0..30 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            app.draw_window(window, surface.canvas());
            let _ = test_driver::animate(
                app,
                imba::anim::AnimationClock::from_millis(tick as f64 * 16.0),
            );
        }
    };

    assert!(test_driver::type_text(&mut app, "check "));
    assert!(!panel(&app).completion_open(), "no popup before the @");
    assert!(test_driver::type_text(&mut app, "@"));
    assert!(panel(&app).completion_open(), "the @ opened the popup");

    assert!(test_driver::type_text(&mut app, "ma"));
    pump(&mut app, &mut surface);
    assert_eq!(
        terms.lock().expect("terms").last().map(String::as_str),
        Some("ma"),
        "the find asked with the typed query"
    );
    let rows = panel(&app).completion_rows();
    assert_eq!(
        rows,
        vec!["main.rs".to_owned(), "README.md".to_owned()],
        "the stub's files listed"
    );

    assert!(test_driver::key(&mut app, Key::Down, Modifiers::default()));
    assert!(test_driver::key(&mut app, Key::Enter, Modifiers::default()));
    let after = panel(&app);
    assert!(!after.completion_open(), "the pick closed the popup");
    assert_eq!(
        after.composer_text(),
        "check @README.md ",
        "the inline spelling replaced the query, newline-free"
    );
    assert_eq!(after.completion_picked(), vec!["README.md".to_owned()]);

    assert!(test_driver::type_text(&mut app, "@"));
    assert!(panel(&app).completion_open());
    assert!(test_driver::key(
        &mut app,
        Key::Escape,
        Modifiers::default()
    ));
    assert!(!panel(&app).completion_open(), "Escape closed the popup");

    assert!(test_driver::type_text(&mut app, "x "));
    assert!(
        !panel(&app).completion_open(),
        "a pasted 'x ' must not open"
    );
    assert!(test_driver::type_text(&mut app, "@"));
    assert!(panel(&app).completion_open());
    assert!(test_driver::key(
        &mut app,
        Key::Backspace,
        Modifiers::default()
    ));
    assert!(
        !panel(&app).completion_open(),
        "deleting the @ closed the popup"
    );
}

#[test]
fn the_at_completion_serves_markdown_panes() {
    use himark::app::AppCommand;
    use himark::app::AppFonts;
    use himark::app::Application;
    use himark::app::OpenedDocument;
    use himark::test_driver;
    use imba::event::{Key, Modifiers};
    use std::sync::Arc;

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();
    let folder = editor::location::ResourceLocation::new(
        editor::location::ResourceType::directory(),
        editor::location::Authority::new("test"),
        vec!["proj".to_owned()],
    );
    let session = himark::test_support::seed_session_folders(&mut app.store_mut(), &[folder]);

    struct StubFind(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    impl imba::effect::EffectHandler<ahp_locations::FindEffect> for StubFind {
        async fn handle(
            &self,
            effect: ahp_locations::FindEffect,
        ) -> Vec<editor::location::ResourceLocation> {
            self.0.lock().expect("terms").push(effect.term.clone());
            vec![editor::location::ResourceLocation::new(
                editor::location::ResourceType::document(),
                editor::location::Authority::new("test"),
                vec!["proj".to_owned(), "notes".to_owned(), "ideas.md".to_owned()],
            )]
        }
    }
    let terms = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    app.register_handler::<ahp_locations::FindEffect>(StubFind(std::sync::Arc::clone(&terms)));

    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        std::sync::Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        std::sync::Arc::new(|| {}),
    );

    struct EnterSeeded(ahp_wire::SessionId);
    impl himark::commands::WindowedCommand for EnterSeeded {
        fn id(&self) -> &'static str {
            "test.enter-seeded"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            himark::app::switch_session(store, window, self.0.clone(), fx)
        }
    }
    assert!(app.perform_command(AppCommand::Windowed(
        window,
        Arc::new(EnterSeeded(session.clone()))
    )));

    let markdown = editor::document::Document::new(
        text::text::Text::from_string_exact("hello world "),
        editor::markup::Markup::new(),
    )
    .with_syntax(
        editor::markup::Syntax::new("markdown", None, editor::markup::Markup::new()),
        &[],
    );
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "notes.md".to_owned(),
            document: markdown,
            location: Some(editor::location::ResourceLocation::new(
                editor::location::ResourceType::document(),
                editor::location::Authority::new("test"),
                vec!["proj".to_owned(), "notes.md".to_owned()],
            )),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    app.draw_window(window, surface.canvas());

    let completion_open = |app: &Application| -> bool {
        himark::editor_accessories::Seats::seat(app.store(), app.focused_editor_id().1)
            .is_some_and(|seat| seat.completion.open())
    };
    let rows = |app: &Application| -> Vec<String> {
        himark::editor_accessories::Seats::seat(app.store(), app.focused_editor_id().1)
            .map(|seat| seat.completion.row_labels())
            .unwrap_or_default()
    };
    let pane_text = |app: &Application| -> String {
        let (_, held) = documents::OpenDocuments::list(app.store(), app.sole_documents())
            .into_iter()
            .find(|(_, held)| held.name() == "notes.md")
            .expect("the pane document");
        let document = held.document();
        let mut view = document.text().view();
        let end = view.byte_count();
        view.byte_string(0, end)
    };
    let pump = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for tick in 0..30 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            app.draw_window(window, surface.canvas());
            let _ = test_driver::animate(
                app,
                imba::anim::AnimationClock::from_millis(tick as f64 * 16.0),
            );
        }
    };

    assert!(test_driver::key(
        &mut app,
        Key::Down,
        Modifiers {
            command: true,
            ..Default::default()
        }
    ));
    assert!(test_driver::type_text(&mut app, "@"));
    assert!(completion_open(&app), "the @ opened the pane popup");

    assert!(test_driver::type_text(&mut app, "id"));
    pump(&mut app, &mut surface);
    assert_eq!(
        terms.lock().expect("terms").last().map(String::as_str),
        Some("id"),
        "the find asked with the typed query"
    );
    assert_eq!(rows(&app), vec!["ideas.md".to_owned()]);

    assert!(test_driver::key(&mut app, Key::Enter, Modifiers::default()));
    assert!(!completion_open(&app), "the pick closed the popup");
    assert_eq!(
        pane_text(&app),
        "hello world @notes/ideas.md ",
        "the pick wrote the folder-relative path inline"
    );

    assert!(test_driver::type_text(&mut app, "@"));
    assert!(completion_open(&app));
    assert!(test_driver::key(
        &mut app,
        Key::Escape,
        Modifiers::default()
    ));
    assert!(!completion_open(&app), "Escape closed the popup");

    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "main.rs".to_owned(),
            document: plain_document("fn main() {}\n"),
            location: Some(editor::location::ResourceLocation::new(
                editor::location::ResourceType::document(),
                editor::location::Authority::new("test"),
                vec!["proj".to_owned(), "main.rs".to_owned()],
            )),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    app.draw_window(window, surface.canvas());
    assert!(test_driver::type_text(&mut app, "@"));
    assert!(
        !completion_open(&app),
        "a non-markdown pane must not path-complete"
    );
}

#[test]
fn lsp_completion_serves_code_panes() {
    use himark::app::AppCommand;
    use himark::app::AppFonts;
    use himark::app::Application;
    use himark::app::OpenedDocument;
    use himark::test_driver;
    use imba::event::{Key, Modifiers};
    use std::sync::Arc;

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let window = app.add_window();

    struct StubLsp(Arc<std::sync::Mutex<Vec<documents::text_ext::LineCol>>>);
    impl imba::effect::EffectHandler<ahp_lsp::LspCompletionEffect> for StubLsp {
        async fn handle(&self, effect: ahp_lsp::LspCompletionEffect) -> Option<ahp_lsp::LspAnswer> {
            self.0.lock().expect("asks").push(effect.position);
            Some(ahp_lsp::LspAnswer {
                items: vec![
                    ahp_lsp::LspItem {
                        label: "insert".to_owned(),
                        detail: Some("fn insert(k, v)".to_owned()),
                        filter_text: None,
                        sort_text: Some("0".to_owned()),
                        edit: Some((
                            documents::text_ext::LineCol {
                                line: effect.position.line,
                                col: effect.position.col.saturating_sub(1),
                            }..effect.position,
                            "insert($0)".to_owned(),
                        )),
                        insert_text: None,
                    },
                    ahp_lsp::LspItem {
                        label: "push".to_owned(),
                        detail: None,
                        filter_text: None,
                        sort_text: Some("1".to_owned()),
                        edit: None,
                        insert_text: Some("push".to_owned()),
                    },
                ],
                incomplete: false,
            })
        }
    }
    let asks = Arc::new(std::sync::Mutex::new(Vec::new()));
    app.register_handler::<ahp_lsp::LspCompletionEffect>(StubLsp(Arc::clone(&asks)));

    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        std::sync::Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        std::sync::Arc::new(|| {}),
    );

    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "main.rs".to_owned(),
            document: plain_document("value "),
            location: Some(editor::location::ResourceLocation::new(
                editor::location::ResourceType::document(),
                editor::location::Authority::new("test"),
                vec!["proj".to_owned(), "main.rs".to_owned()],
            )),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    app.draw_window(window, surface.canvas());

    let completion_open = |app: &Application| -> bool {
        himark::editor_accessories::Seats::seat(app.store(), app.focused_editor_id().1)
            .is_some_and(|seat| seat.completion.open())
    };
    let rows = |app: &Application| -> Vec<String> {
        himark::editor_accessories::Seats::seat(app.store(), app.focused_editor_id().1)
            .map(|seat| seat.completion.row_labels())
            .unwrap_or_default()
    };
    let pane_text = |app: &Application| -> String {
        let (_, held) = documents::OpenDocuments::list(app.store(), app.sole_documents())
            .into_iter()
            .find(|(_, held)| held.name() == "main.rs")
            .expect("the pane document");
        let document = held.document();
        let mut view = document.text().view();
        let end = view.byte_count();
        view.byte_string(0, end)
    };
    let pump = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for tick in 0..30 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            app.draw_window(window, surface.canvas());
            let _ = test_driver::animate(
                app,
                imba::anim::AnimationClock::from_millis(tick as f64 * 16.0),
            );
        }
    };

    assert!(test_driver::key(
        &mut app,
        Key::Down,
        Modifiers {
            command: true,
            ..Default::default()
        }
    ));
    assert!(test_driver::type_text(&mut app, "i"));
    assert!(completion_open(&app), "the word start opened");
    pump(&mut app, &mut surface);
    assert_eq!(asks.lock().expect("asks").len(), 1, "one ask at open");
    assert_eq!(
        rows(&app),
        vec!["insert".to_owned()],
        "the query 'i' already filters"
    );

    assert!(test_driver::type_text(&mut app, "n"));
    pump(&mut app, &mut surface);
    assert_eq!(
        asks.lock().expect("asks").len(),
        1,
        "the growth filtered locally"
    );
    assert_eq!(
        rows(&app),
        vec!["insert".to_owned()],
        "subsequence-filtered"
    );

    assert!(test_driver::key(&mut app, Key::Enter, Modifiers::default()));
    assert!(!completion_open(&app), "the pick closed the popup");
    assert!(
        pane_text(&app).contains("insert()"),
        "the textEdit applied: {:?}",
        pane_text(&app)
    );

    assert!(test_driver::type_text(&mut app, "."));
    assert!(completion_open(&app), "the trigger char opened");
    pump(&mut app, &mut surface);
    assert_eq!(asks.lock().expect("asks").len(), 2, "the trigger re-asked");
    assert!(test_driver::key(
        &mut app,
        Key::Escape,
        Modifiers::default()
    ));
    assert!(!completion_open(&app));

    let trigger = himark::commands::palette_commands(app.store(), &app.ui_handle(), window)
        .into_iter()
        .find(|presentable| presentable.id == "completion.trigger")
        .expect("the trigger command registered")
        .command;
    assert!(app.perform_command(trigger));
    assert!(completion_open(&app), "the explicit ask opened");
    pump(&mut app, &mut surface);
    assert_eq!(
        asks.lock().expect("asks").len(),
        3,
        "the explicit ask launched"
    );
    assert!(
        !rows(&app).is_empty(),
        "the app-road landing filled the rows"
    );
}
