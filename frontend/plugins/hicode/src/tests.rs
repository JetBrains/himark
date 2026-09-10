use super::*;

use himark::test_document::plain_document;
use himark::{AppCommand, AppExt, AppFonts, Application, OpenedDocument};
use std::sync::{mpsc, Arc};

fn document_location(name: &str) -> ResourceLocation {
    ResourceLocation::new(
        himark::ResourceType::document(),
        himark::Authority::new("local"),
        vec!["project".to_owned(), name.to_owned()],
    )
}

fn lc(line: u32, col: u32) -> LineCol {
    LineCol { line, col }
}

#[test]
fn preview_spans_expand_context_and_merge_overlaps() {
    let text = text::Text::from_string(&"a\n".repeat(12));
    let mut view = text.view();

    let spans = preview_spans(
        &mut view,
        &[lc(2, 0)..lc(2, 1), lc(3, 0)..lc(3, 1), lc(10, 0)..lc(10, 1)],
    );
    assert_eq!(spans.marks, vec![4..5, 6..7, 20..21]);
    assert_eq!(
        spans.ranges,
        vec![0..12, 16..24],
        "context 2 lines each way, overlapping windows merged"
    );
}

#[test]
fn identifier_at_clips_the_word_under_the_caret() {
    let text = text::Text::from_string("let frob_nicate2 = 7;");
    let mut view = text.view();
    assert_eq!(identifier_at(&mut view, 6), "frob_nicate2");
    assert_eq!(identifier_at(&mut view, 4), "frob_nicate2");
    assert_eq!(identifier_at(&mut view, 17), "");
    let empty = text::Text::from_string("");
    assert_eq!(identifier_at(&mut empty.view(), 0), "");
}

struct StubNavigation {
    targets: Option<Vec<CodeTarget>>,
    built: Vec<(ResourceLocation, Document)>,
}

impl himark::EffectHandler<CodeNavigationEffect> for StubNavigation {
    async fn handle(&self, effect: CodeNavigationEffect) -> NavigationOutcome {
        NavigationOutcome {
            title: effect.title,
            targets: self.targets.clone(),
            built: self.built.clone(),
        }
    }
}

fn app_with_located_document(source: &str) -> (Application, himark::WindowId) {
    let mut app = Application::new(AppFonts::embedded());
    app.register_editor_command(Arc::new(GoDefinition));
    app.register_editor_command(Arc::new(GoReferences));
    let window = app.add_window();
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            name: "a.rs".to_owned(),
            document: plain_document(source),
            location: Some(document_location("a.rs")),
            primary: true,
            target: None,
        },
    )));
    (app, window)
}

fn invoke(app: &mut Application, window: himark::WindowId, id: &str) {
    let command = himark::palette_commands(app.store(), &app.ui_handle(), window)
        .into_iter()
        .find(|presentable| presentable.id == id)
        .expect("the located editor offers the command")
        .command;
    assert!(app.perform_command(command));
}

#[test]
fn located_editors_offer_the_commands_on_the_focus_path() {
    let mut app = Application::new(AppFonts::embedded());
    app.register_editor_command(Arc::new(GoDefinition));
    let window = app.add_window();
    let listed = |app: &Application| {
        himark::palette_commands(app.store(), &app.ui_handle(), window)
            .iter()
            .any(|presentable| presentable.id == "code.definition")
    };
    assert!(!listed(&app), "the startup scratch has no location");
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            name: "a.rs".to_owned(),
            document: plain_document("hello"),
            location: Some(document_location("a.rs")),
            primary: true,
            target: None,
        },
    )));
    assert!(listed(&app), "the located editor offers it");
}

#[test]
fn a_single_open_target_navigates_with_the_caret_revealed() {
    let (mut app, window) = app_with_located_document("hello\nworld\nagain");
    app.register_handler::<CodeNavigationEffect>(StubNavigation {
        targets: Some(vec![CodeTarget {
            location: document_location("a.rs"),
            range: lc(1, 2)..lc(1, 5),
        }]),
        built: Vec::new(),
    });
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );

    invoke(&mut app, window, "code.definition");
    runner.run();
    while let Ok(command) = arriving.try_recv() {
        app.perform_batch(vec![command]);
    }
    assert_eq!(app.focused_caret_byte(), Some(8), "line 1 col 2");
    assert_eq!(app.focused_reveal_pending(), Some(true));
}

#[test]
fn a_single_unopened_target_registers_its_prefetched_build() {
    let (mut app, window) = app_with_located_document("hello");
    let far = document_location("far.rs");
    app.register_handler::<CodeNavigationEffect>(StubNavigation {
        targets: Some(vec![CodeTarget {
            location: far.clone(),
            range: lc(1, 0)..lc(1, 4),
        }]),
        built: vec![(far, plain_document("alpha\nbeta\ngamma"))],
    });
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let documents_before = app.document_count();

    invoke(&mut app, window, "code.definition");
    runner.run();
    while let Ok(command) = arriving.try_recv() {
        app.perform_batch(vec![command]);
    }
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("alpha\nbeta\ngamma"),
        "the prefetched build opened"
    );
    assert_eq!(app.focused_caret_byte(), Some(6), "line 1 col 0");
    assert_eq!(app.focused_reveal_pending(), Some(true));
    assert_eq!(
        app.document_count(),
        documents_before,
        "the target registered; the jumped-from clean document released"
    );
}

#[test]
fn many_targets_open_the_references_panel_with_groups() {
    let (mut app, window) = app_with_located_document(&"line one two\n".repeat(30));
    let open_location = document_location("a.rs");
    let far = document_location("far.rs");
    app.register_handler::<CodeNavigationEffect>(StubNavigation {
        targets: Some(vec![
            CodeTarget {
                location: open_location.clone(),
                range: lc(0, 5)..lc(0, 8),
            },
            CodeTarget {
                location: open_location,
                range: lc(20, 5)..lc(20, 8),
            },
            CodeTarget {
                location: far.clone(),
                range: lc(0, 0)..lc(0, 3),
            },
        ]),
        built: vec![(far, plain_document("fee fie foe\n"))],
    });
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let documents_before = app.document_count();

    invoke(&mut app, window, "code.references");

    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    for _ in 0..4 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let _ = himark::Window::draw(window, &mut app, surface.canvas());
    }

    let mut panel_list = None;
    app.for_each_plugin_panel(&mut |panel| {
        if let Some(references) = panel.as_any().downcast_ref::<himark::ListPanel>() {
            panel_list = Some(references.list_id());
        }
    });
    let panel_entry = panel_list.and_then(|id| himark::LocationLists::entry_ref(app.store(), id));
    let panel_groups = panel_entry.map(|entry| entry.list.content().group_sizes());
    let panel_title = panel_entry.map(|entry| entry.title.clone());
    assert_eq!(
        panel_groups,
        Some(vec![2, 1]),
        "two far-apart rows in the open document, one in the temp"
    );
    assert_eq!(panel_title.as_deref(), Some("References to `line`"));
    assert_eq!(
        app.document_count(),
        documents_before + 1,
        "the prefetched preview registers under its location like any open"
    );
}

#[test]
fn cmd_enter_opens_the_focused_group_in_full() {
    let (mut app, window) = app_with_located_document(&"line one two\n".repeat(30));
    let open_location = document_location("a.rs");
    let far = document_location("far.rs");
    app.register_handler::<CodeNavigationEffect>(StubNavigation {
        targets: Some(vec![
            CodeTarget {
                location: open_location.clone(),
                range: lc(0, 5)..lc(0, 8),
            },
            CodeTarget {
                location: far.clone(),
                range: lc(0, 0)..lc(0, 3),
            },
        ]),
        built: vec![(far, plain_document("fee fie foe\n"))],
    });
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    invoke(&mut app, window, "code.references");
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    for _ in 0..4 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let _ = himark::Window::draw(window, &mut app, surface.canvas());
    }

    let list_id = {
        let mut panel_list = None;
        app.for_each_plugin_panel(&mut |panel| {
            if let Some(references) = panel.as_any().downcast_ref::<himark::ListPanel>() {
                panel_list = Some(references.list_id());
            }
        });
        panel_list.expect("the references panel stands")
    };
    let focused_group = |app: &Application| {
        himark::LocationLists::entry_ref(app.store(), list_id)
            .map(|entry| entry.list.content().focused_group())
    };

    'scan: for x in [250, 480, 700] {
        for y in (120..680).step_by(24) {
            let _ = himark::test_driver::click(&mut app, x as f32, y as f32, 900.0, 700.0);
            if focused_group(&app) == Some(Some(1)) {
                break 'scan;
            }
        }
    }
    assert_eq!(
        focused_group(&app),
        Some(Some(1)),
        "a click lands the focus on the far group"
    );

    let handled = himark::test_driver::key(
        &mut app,
        imba::event::Key::Enter,
        imba::event::Modifiers {
            command: true,
            ..Default::default()
        },
    );
    assert!(handled, "cmd-enter resolves on the focused group");

    runner.run();
    while let Ok(command) = arriving.try_recv() {
        app.perform_batch(vec![command]);
    }
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("fee fie foe\n"),
        "the far group's document opened in full"
    );
}
