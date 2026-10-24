use super::*;
use himark::app::AppFonts;
use himark::app::Himark;
use himark::app::OpenedDocument;
use himark::app_ext::AppExt;

fn located(name: &str) -> editor::location::ResourceLocation {
    editor::location::ResourceLocation::new(
        editor::location::ResourceType::document(),
        editor::location::Authority::new("test"),
        vec!["project".to_owned(), name.to_owned()],
    )
}

fn outlined_document() -> (String, editor::document::Document) {
    let source = "# One\ntext\n## Two\nmore\n".to_owned();
    let len = source.len() as u32;
    let mut syntax = editor::markup::Syntax::new("toy", None, editor::markup::Markup::new());
    syntax.push_outline_item(
        0..len,
        editor::markup::OutlineItem {
            title: "One".to_owned(),
        },
    );
    syntax.push_outline_item(
        11..len,
        editor::markup::OutlineItem {
            title: "Two".to_owned(),
        },
    );
    let document = editor::document::Document::new(
        text::text::Text::from_string_exact(&source),
        editor::markup::Markup::new(),
    )
    .with_syntax(syntax, &[]);
    (source, document)
}

fn run_outline(batch: imba::effect::Batch<::toc::OutlineCommand>) -> Option<::toc::OutlineRows> {
    use imba::effect::{block_on, EffectHandler, Message};
    for message in batch.drain() {
        let (Message::Launch(_, effect) | Message::Relaunch(_, _, effect)) = message else {
            continue;
        };
        let (value, _) = effect.into_payload().split();
        if let Ok(effect) = value.downcast::<::toc::OutlineEffect>() {
            return Some(block_on(Box::pin(async move {
                ::toc::OutlineHandler.handle(*effect).await
            })));
        }
    }
    None
}

#[test]
fn the_outline_derives_lands_and_jumps() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    let (source, document) = outlined_document();
    assert!(app.perform_command(himark::app::opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "toc.md".to_owned(),
            document,
            location: Some(located("toc.md")),
            primary: true,
            target: None,
            focus: false,
        },
    )));

    assert!(app.perform_registered(window, "toc.toggle"));
    let mut view = ::workbench::window::Windows::window_ref(app.store(), window)
        .expect("window")
        .side_panel()
        .expect("the drawer is up")
        .as_any()
        .downcast_ref::<::toc::OutlineView>()
        .expect("the drawer holds the outline")
        .clone();
    assert!(view.rows().is_empty(), "nothing landed yet");

    let ui = ::editor::test_document::test_ui();
    let mut store = app.store().clone();
    let mut batch = imba::effect::Batch::new();
    view.perform(
        &mut store,
        &ui,
        ::toc::OutlineCommand::Refresh,
        &mut batch.effects(),
    );
    let landed = run_outline(batch).expect("the refresh launched the derivation");
    view.perform(
        &mut store,
        &ui,
        ::toc::OutlineCommand::Landed(landed),
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        view.rows(),
        vec![(0, "One".to_owned()), (1, "Two".to_owned())],
        "containment nests the sections"
    );

    assert!(app.perform_registered(window, "toc.toggle"));

    let _ = himark::test_driver::animate(&mut app, imba::anim::AnimationClock::from_millis(0.0));
    let _ =
        himark::test_driver::animate(&mut app, imba::anim::AnimationClock::from_millis(1_000.0));
    assert!(himark::test_driver::type_text(&mut app, "x"));
    let mut store = app.store().clone();
    {
        use imba::list::{ActivateTrigger, ListOps};
        let step = view.search().step_index(1).expect("a stepped row");
        let select = ::toc::OutlineCommand::List(view.search().select_command(step));
        view.perform(
            &mut store,
            &ui,
            select,
            &mut imba::effect::Batch::new().effects(),
        );
        let at = view.search().cursor_index().expect("a cursor row");
        let pick =
            ::toc::OutlineCommand::List(view.search().activate_command(at, ActivateTrigger::Enter));
        view.perform(
            &mut store,
            &ui,
            pick,
            &mut imba::effect::Batch::new().effects(),
        );
    }
    let Some(hikit::modal::ModalRequest::Perform(verb)) =
        hikit::modal::ModalView::take_request(&mut view)
    else {
        panic!("the pick performs the jump");
    };
    let command = himark::app::verb_command(app.sole_window(), verb).expect("a performable verb");
    assert!(app.perform_command(command));
    let (document_id, editor_id) = app.focused_editor_id();
    let caret =
        documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), document_id)
            .expect("document")
            .caret_byte(editor_id);
    assert_eq!(
        caret,
        source.find("## Two").unwrap() as u32 + 1,
        "the address resolved LIVE — shifted by the typed byte"
    );
    let depths = ::workbench::window::Windows::window_ref(app.store(), window)
        .expect("window")
        .focused_history_depths();
    assert_eq!(depths, (1, 0), "the jump recorded where it left");
}

#[test]
fn a_structureless_pane_offers_no_toc() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    // The toggle may refuse on a structureless pane — what matters
    // is that no panel opens.
    let _ = app.perform_registered(window, "toc.toggle");
    assert!(
        ::workbench::window::Windows::window_ref(app.store(), window)
            .expect("window")
            .side_panel()
            .is_none(),
        "no structure, no drawer"
    );
}

#[test]
fn locations_group_into_a_results_forest() {
    let store = Store::new();
    let _window = ::workbench::window::WindowId::from_raw(1);
    let at = |dir: &str, name: &str| {
        editor::location::ResourceLocation::new(
            editor::location::ResourceType::document(),
            editor::location::Authority::new("test"),
            vec![dir.to_owned(), name.to_owned()],
        )
    };
    let toc = ::toc::TocView::for_locations(
        &store,
        ::editor::test_document::test_ui(),
        &[at("src", "b.rs"), at("docs", "a.md"), at("src", "a.rs")],
    )
    .expect("rows");
    assert_eq!(
        toc.rows(),
        vec![
            (0, "docs".to_owned(), false),
            (1, "a.md".to_owned(), true),
            (0, "src".to_owned(), false),
            (1, "a.rs".to_owned(), true),
            (1, "b.rs".to_owned(), true),
        ],
        "dirs band their files; files sort within"
    );

    let mut toc = toc;
    let ui = ::editor::test_document::test_ui();
    let mut scratch = Store::new();
    {
        use imba::list::{ActivateTrigger, ListOps};
        let at = toc.search().cursor_index().expect("a cursor row");
        let pick =
            ::toc::TocCommand::List(toc.search().activate_command(at, ActivateTrigger::Enter));
        toc.perform(
            &mut scratch,
            &ui,
            pick,
            &mut imba::effect::Batch::new().effects(),
        );
    }
    let Some(hikit::modal::ModalRequest::OpenLocations(locations)) =
        hikit::modal::ModalView::take_request(&mut toc)
    else {
        panic!("a file pick opens");
    };
    assert_eq!(locations, vec![at("docs", "a.md")]);

    let mut band = ::toc::TocView::for_locations(
        &store,
        ::editor::test_document::test_ui(),
        &[at("src", "x.rs")],
    )
    .expect("rows");
    {
        use imba::list::{ActivateTrigger, ListOps};
        // The band row is the only cursor stop; Up clamps onto it.
        if let Some(step) = band.search().step_index(-1) {
            let select = ::toc::TocCommand::List(band.search().select_command(step));
            band.perform(
                &mut scratch,
                &ui,
                select,
                &mut imba::effect::Batch::new().effects(),
            );
        }
        let at = band.search().cursor_index().expect("a cursor row");
        let pick =
            ::toc::TocCommand::List(band.search().activate_command(at, ActivateTrigger::Enter));
        band.perform(
            &mut scratch,
            &ui,
            pick,
            &mut imba::effect::Batch::new().effects(),
        );
    }
    assert!(
        hikit::modal::ModalView::take_request(&mut band).is_none(),
        "band rows never pick"
    );
}

#[test]
fn locations_nest_into_a_directory_tree() {
    let store = Store::new();
    let _window = ::workbench::window::WindowId::from_raw(1);
    let at = |path: &[&str]| {
        editor::location::ResourceLocation::new(
            editor::location::ResourceType::document(),
            editor::location::Authority::new("test"),
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
    };
    let toc = ::toc::TocView::for_locations(
        &store,
        ::editor::test_document::test_ui(),
        &[
            at(&["src", "ui", "widgets", "c.rs"]),
            at(&["src", "a.rs"]),
            at(&["src", "ui", "b.rs"]),
        ],
    )
    .expect("rows");
    assert_eq!(
        toc.rows(),
        vec![
            (0, "src".to_owned(), false),
            (1, "ui".to_owned(), false),
            (2, "widgets".to_owned(), false),
            (3, "c.rs".to_owned(), true),
            (2, "b.rs".to_owned(), true),
            (1, "a.rs".to_owned(), true),
        ],
        "real nesting, folders before files"
    );

    let chain = ::toc::TocView::for_locations(
        &store,
        ::editor::test_document::test_ui(),
        &[at(&["a", "b", "c", "x.rs"])],
    )
    .expect("rows");
    assert_eq!(
        chain.rows(),
        vec![(0, "a/b/c".to_owned(), false), (1, "x.rs".to_owned(), true),],
        "single-child directory chains compact"
    );
}

#[test]
fn result_directories_fold_and_unfold() {
    let store = Store::new();
    let _window = ::workbench::window::WindowId::from_raw(1);
    let at = |dir: &str, name: &str| {
        editor::location::ResourceLocation::new(
            editor::location::ResourceType::document(),
            editor::location::Authority::new("test"),
            vec![dir.to_owned(), name.to_owned()],
        )
    };
    let mut toc = ::toc::TocView::for_locations(
        &store,
        ::editor::test_document::test_ui(),
        &[at("src", "a.rs"), at("src", "b.rs")],
    )
    .expect("rows");
    assert_eq!(toc.visible_rows(), 3, "the band and both files");

    let ui = ::editor::test_document::test_ui();
    let mut scratch = Store::new();
    let mut drive = |toc: &mut ::toc::TocView, command| {
        toc.perform(
            &mut scratch,
            &ui,
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    };

    use imba::list::{ActivateTrigger, ListOps};
    if let Some(step) = toc.search().step_index(-1) {
        let select = ::toc::TocCommand::List(toc.search().select_command(step));
        drive(&mut toc, select);
    }
    let fold = ::toc::TocCommand::List(hikit::list_keyboard::ListKeyCommand::Fold {
        index: toc.search().cursor_index().expect("a cursor row"),
        expand: false,
    });
    drive(&mut toc, fold);
    assert_eq!(toc.visible_rows(), 1, "the fold hides the subtree");
    assert!(
        hikit::modal::ModalView::take_request(&mut toc).is_none(),
        "folding is not a pick"
    );
    let pick = ::toc::TocCommand::List(toc.search().activate_command(
        toc.search().cursor_index().expect("a cursor row"),
        ActivateTrigger::Enter,
    ));
    drive(&mut toc, pick);
    assert_eq!(toc.visible_rows(), 3, "Enter on a directory unfolds");
}

#[test]
fn outline_speedsearch_filters_steps_and_clears() {
    use imba::effect::{block_on, EffectHandler, Message};

    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    let (_source, document) = outlined_document();
    assert!(app.perform_command(himark::app::opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "toc.md".to_owned(),
            document,
            location: Some(located("toc.md")),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    assert!(app.perform_registered(window, "toc.toggle"));
    let mut view = ::workbench::window::Windows::window_ref(app.store(), window)
        .expect("window")
        .side_panel()
        .expect("drawer")
        .as_any()
        .downcast_ref::<::toc::OutlineView>()
        .expect("outline")
        .clone();

    let ui = ::editor::test_document::test_ui();
    let mut store = app.store().clone();
    let mut batch = imba::effect::Batch::new();
    view.perform(
        &mut store,
        &ui,
        ::toc::OutlineCommand::Refresh,
        &mut batch.effects(),
    );
    let landed = run_outline(batch).expect("derivation");
    let mut drive = |view: &mut ::toc::OutlineView, command| -> imba::effect::Batch<_> {
        let mut batch = imba::effect::Batch::new();
        view.perform(&mut store, &ui, command, &mut batch.effects());
        batch
    };
    let _ = drive(&mut view, ::toc::OutlineCommand::Landed(landed));

    let typing = drive(
        &mut view,
        ::toc::OutlineCommand::List(hikit::list_keyboard::ListKeyCommand::Input(
            editor::editor_view::EditorCommand::InsertText {
                text: "two".to_owned(),
            },
        )),
    );
    let mut matches = None;
    for message in typing.drain() {
        let (Message::Launch(_, effect) | Message::Relaunch(_, _, effect)) = message else {
            continue;
        };
        let (value, _) = effect.into_payload().split();
        if let Ok(effect) = value.downcast::<hikit::list_keyboard::SpeedSearchEffect>() {
            matches = Some(block_on(Box::pin(async move {
                hikit::list_keyboard::SpeedSearchHandler
                    .handle(*effect)
                    .await
            })));
        }
    }
    let matches = matches.expect("typing launched the filter");
    let landing = drive(
        &mut view,
        ::toc::OutlineCommand::List(hikit::list_keyboard::ListKeyCommand::Landed(matches)),
    );
    // The first-match jump rides the announce round trip.
    if let Some(select) = crate::drain_announced(landing) {
        let _ = drive(&mut view, select);
    }
    assert_eq!(view.match_count(), 1, "only 'Two' matches");
    assert_eq!(
        view.cursor_title().as_deref(),
        Some("Two"),
        "the cursor jumped"
    );

    {
        use imba::list::ListOps;
        let step = view
            .search()
            .matched_step_index(1)
            .expect("a match to step");
        let select = ::toc::OutlineCommand::List(view.search().select_command(step));
        let _ = drive(&mut view, select);
    }
    assert_eq!(
        view.cursor_title().as_deref(),
        Some("Two"),
        "wrapped in place"
    );
    let _ = drive(
        &mut view,
        ::toc::OutlineCommand::List(hikit::list_keyboard::ListKeyCommand::Clear),
    );
    assert_eq!(view.match_count(), 0, "cleared");
}

#[test]
fn speedsearch_arrows_step_the_matches() {
    use imba::constraints::Constraints;
    use imba::effect::{block_on, EffectHandler, Message};
    use imba::event::{Event, EventResult, Key, Modifiers};
    use imba::Widget as _;

    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    let (_source, document) = outlined_document();
    assert!(app.perform_command(himark::app::opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "toc.md".to_owned(),
            document,
            location: Some(located("toc.md")),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    assert!(app.perform_registered(window, "toc.toggle"));
    let mut view = ::workbench::window::Windows::window_ref(app.store(), window)
        .expect("window")
        .side_panel()
        .expect("drawer")
        .as_any()
        .downcast_ref::<::toc::OutlineView>()
        .expect("outline")
        .clone();

    let ui = ::editor::test_document::test_ui();
    let mut store = app.store().clone();
    let mut batch = imba::effect::Batch::new();
    view.perform(
        &mut store,
        &ui,
        ::toc::OutlineCommand::Refresh,
        &mut batch.effects(),
    );
    let landed = run_outline(batch).expect("derivation");
    view.perform(
        &mut store,
        &ui,
        ::toc::OutlineCommand::Landed(landed),
        &mut imba::effect::Batch::new().effects(),
    );

    let mut typing = imba::effect::Batch::new();
    view.perform(
        &mut store,
        &ui,
        ::toc::OutlineCommand::List(hikit::list_keyboard::ListKeyCommand::Input(
            editor::editor_view::EditorCommand::InsertText {
                text: "o".to_owned(),
            },
        )),
        &mut typing.effects(),
    );
    let mut matches = None;
    for message in typing.drain() {
        let (Message::Launch(_, effect) | Message::Relaunch(_, _, effect)) = message else {
            continue;
        };
        let (value, _) = effect.into_payload().split();
        if let Ok(effect) = value.downcast::<hikit::list_keyboard::SpeedSearchEffect>() {
            matches = Some(block_on(Box::pin(async move {
                hikit::list_keyboard::SpeedSearchHandler
                    .handle(*effect)
                    .await
            })));
        }
    }
    let mut landing = imba::effect::Batch::new();
    view.perform(
        &mut store,
        &ui,
        ::toc::OutlineCommand::List(hikit::list_keyboard::ListKeyCommand::Landed(
            matches.expect("filter launched"),
        )),
        &mut landing.effects(),
    );
    // The first-match jump rides the announce round trip.
    if let Some(select) = crate::drain_announced(landing) {
        view.perform(
            &mut store,
            &ui,
            select,
            &mut imba::effect::Batch::new().effects(),
        );
    }
    assert_eq!(view.match_count(), 2, "One and Two match 'o'");
    assert_eq!(view.cursor_title().as_deref(), Some("One"));

    let result = {
        let arena = imba::arena::Arena::default();
        let widget = imba::layout::Layout::layout(
            imba::View::display(&view, &arena, &store, &ui),
            &arena,
            Constraints::tight(skia_safe::Size::new(800.0, 600.0)),
        );
        let widget = imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_wh(800.0, 600.0));
        let result = widget.handle_event(
            &arena,
            &Event::KeyDown {
                key: Key::Down,
                mods: Modifiers::default(),
            },
            skia_safe::Rect::from_wh(800.0, 600.0),
        );
        drop(widget);
        result
    };
    let stepped = match &result {
        imba::event::EventResult::Handled(handled) => {
            handled
                .commands
                .first()
                .is_some_and(|command| match command {
                    ::toc::OutlineCommand::List(command) => {
                        use imba::list::ListOps;
                        ::toc::OutlineSearch::selected_index(command).is_some()
                    }
                    _ => false,
                })
        }
        _ => false,
    };
    assert!(
        stepped,
        "Down while searching must answer the matched-step Select"
    );

    if let Some(command) = result.into_command() {
        view.perform(
            &mut store,
            &ui,
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    }
    assert_eq!(
        view.cursor_title().as_deref(),
        Some("Two"),
        "stepped to the next match"
    );
}

#[test]
fn outline_folds_survive_relandings() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    let (_source, document) = outlined_document();
    assert!(app.perform_command(himark::app::opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "toc.md".to_owned(),
            document,
            location: Some(located("toc.md")),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    assert!(app.perform_registered(window, "toc.toggle"));
    let mut view = ::workbench::window::Windows::window_ref(app.store(), window)
        .expect("window")
        .side_panel()
        .expect("drawer")
        .as_any()
        .downcast_ref::<::toc::OutlineView>()
        .expect("outline")
        .clone();

    let ui = ::editor::test_document::test_ui();
    let mut store = app.store().clone();
    let mut batch = imba::effect::Batch::new();
    view.perform(
        &mut store,
        &ui,
        ::toc::OutlineCommand::Refresh,
        &mut batch.effects(),
    );
    let landed = run_outline(batch).expect("derivation");
    let mut drive = |view: &mut ::toc::OutlineView, command| {
        view.perform(
            &mut store,
            &ui,
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    };
    drive(&mut view, ::toc::OutlineCommand::Landed(landed.clone()));
    assert_eq!(view.visible_rows(), 2, "One with Two nested");

    {
        use imba::list::ListOps;
        let fold = ::toc::OutlineCommand::List(hikit::list_keyboard::ListKeyCommand::Fold {
            index: view.search().cursor_index().expect("a cursor row"),
            expand: false,
        });
        drive(&mut view, fold);
    }
    assert_eq!(view.visible_rows(), 1, "folded");

    let mut relanded = landed;
    relanded.stamp = (relanded.stamp.0 + 1, relanded.stamp.1);
    drive(&mut view, ::toc::OutlineCommand::Landed(relanded));
    assert_eq!(view.visible_rows(), 1, "the fold survived the swap");
}

/// A Rust-like `server.rs`: the declarations at the mock's lines,
/// banded into `mod`s so the drawer heads them as groups.
fn server_outline() -> (editor::document::Document, u32) {
    // (depth, title, first line, last line)
    let items: &[(u8, &str, u32, u32)] = &[
        (0, "mod annotations", 38, 60),
        (1, "pub fn annotations_session", 39, 42),
        (1, "pub fn annotations_channel", 43, 46),
        (1, "pub fn annotations_summary", 47, 59),
        (0, "mod configuration", 70, 400),
        (1, "pub struct HostConfig", 72, 99),
        (1, "pub struct LanguageServer", 100, 105),
        (1, "impl Default for HostConfig", 106, 140),
        (2, "fn default", 107, 139),
        (1, "fn claude_card", 141, 153),
        (1, "fn claude_models", 154, 180),
        (1, "fn codex_card", 181, 192),
        (1, "fn codex_models", 193, 235),
        (1, "fn thinking_schema", 236, 280),
        (1, "pub fn session_config_schema", 281, 347),
        (1, "pub fn session_config_values", 348, 399),
        (0, "mod agents", 405, 500),
        (1, "pub struct SessionEntry", 407, 423),
        (1, "pub struct ChatEntry", 424, 444),
        (1, "pub enum LiveAgent", 445, 449),
        (1, "impl LiveAgent", 450, 499),
        (2, "pub fn prompt", 458, 470),
        (2, "pub fn interrupt", 471, 490),
        (0, "mod state", 505, 700),
        (1, "pub struct TerminalEntry", 508, 520),
        (1, "pub struct LspDiagnostics", 521, 526),
        (1, "pub struct ChangesetEntry", 527, 558),
        (1, "pub struct State", 559, 611),
        (1, "pub enum DocumentFeed", 612, 634),
        (1, "pub struct Host", 635, 660),
        (1, "impl Host", 661, 699),
        (2, "pub fn new", 662, 670),
        (2, "fn fsp_engine", 671, 690),
    ];
    let mut lines: Vec<String> = (1..=720).map(|line| format!("// line {line}")).collect();
    for (_, title, first, _) in items {
        lines[*first as usize - 1] = format!("{title} {{");
    }
    let source = lines.join("\n") + "\n";
    let starts: Vec<u32> = std::iter::once(0)
        .chain(
            source
                .match_indices('\n')
                .map(|(at, _)| at as u32 + 1),
        )
        .collect();
    let mut syntax = editor::markup::Syntax::new("rust", None, editor::markup::Markup::new());
    for (_, title, first, last) in items {
        let start = starts[*first as usize - 1];
        let end = starts[*last as usize];
        syntax.push_outline_item(
            start..end,
            editor::markup::OutlineItem {
                title: title.to_string(),
            },
        );
    }
    let caret = starts[615];
    let document = editor::document::Document::new(
        text::text::Text::from_string_exact(&source),
        editor::markup::Markup::new(),
    )
    .with_syntax(syntax, &[]);
    (document, caret)
}

/// The Contents drawer over a seeded outline, 2880×1800 —
/// `HIMARK_SHOT=<dir> cargo test -p himark-tests --test shell dump_contents_drawer_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_contents_drawer_screenshot() {
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((2880, 1800)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    let (document, caret) = server_outline();
    assert!(app.perform_command(himark::app::opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "server.rs".to_owned(),
            document,
            location: Some(located("server.rs")),
            primary: true,
            target: None,
            focus: true,
        },
    )));
    {
        let (document_id, editor) = app.focused_editor_id();
        let mut document =
            documents::OpenDocuments::document(app.store(), app.sole_documents(), document_id)
                .expect("the document");
        document.set_carets(
            editor,
            ::editor::caret::MultiCaret::one(::editor::caret::Caret::at(caret)),
        );
        let documents = app.sole_documents();
        documents::OpenDocuments::put_document(&mut app.store_mut(), documents, document_id, document);
    }
    assert!(app.perform_registered(window, "toc.toggle"));
    let landed = land_drawer_outline(&mut app);
    super::dock::settle(&mut app, &mut surface);
    eprintln!("{} entries landed", landed.entries().len());
    let data = surface
        .image_snapshot()
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::write(format!("{dir}/contents-drawer.png"), data.as_bytes()).expect("write");
}

/// Derive the open drawer's outline in place (the test app runs no
/// effect lane) and stand the landed drawer back in the window.
fn land_drawer_outline(app: &mut Himark) -> ::toc::OutlineView {
    let window = app.sole_window();
    let mut view = ::workbench::window::Windows::window_ref(app.store(), window)
        .expect("window")
        .side_panel()
        .expect("the drawer is up")
        .as_any()
        .downcast_ref::<::toc::OutlineView>()
        .expect("the drawer holds the outline")
        .clone();
    let ui = ::editor::test_document::test_ui();
    let mut store = app.store().clone();
    let mut batch = imba::effect::Batch::new();
    view.perform(
        &mut store,
        &ui,
        ::toc::OutlineCommand::Refresh,
        &mut batch.effects(),
    );
    let landed = run_outline(batch).expect("the refresh launched the derivation");
    view.perform(
        &mut store,
        &ui,
        ::toc::OutlineCommand::Landed(landed),
        &mut imba::effect::Batch::new().effects(),
    );

    struct StandDrawer(::toc::OutlineView);
    impl himark::commands::HimarkCommand for StandDrawer {
        fn id(&self) -> &'static str {
            "test.stand-drawer"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let Some(window) = himark::commands::bound_window(self.id(), bound) else {
                return;
            };
            let mut entity = ::workbench::window::Windows::window(store, window).expect("window");
            fx.scope(
                move |command| himark::shell::content(window, command),
                |fx| entity.show_side_panel(store, Box::new(self.0.clone()), fx),
            );
            ::workbench::window::Windows::put(store, window, entity);
        }
    }
    assert!(app.perform_command(himark::app::windowed(
        window,
        std::sync::Arc::new(StandDrawer(view.clone())),
    )));
    view
}

#[test]
fn the_contents_drawer_reads_kinds_groups_and_the_caret() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    let (document, caret) = server_outline();
    assert!(app.perform_command(himark::app::opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "server.rs".to_owned(),
            document,
            location: Some(located("server.rs")),
            primary: true,
            target: None,
            focus: true,
        },
    )));
    {
        let (document_id, editor) = app.focused_editor_id();
        let mut document =
            documents::OpenDocuments::document(app.store(), app.sole_documents(), document_id)
                .expect("the document");
        document.set_carets(
            editor,
            ::editor::caret::MultiCaret::one(::editor::caret::Caret::at(caret)),
        );
        let documents = app.sole_documents();
        documents::OpenDocuments::put_document(&mut app.store_mut(), documents, document_id, document);
    }
    assert!(app.perform_registered(window, "toc.toggle"));
    let view = land_drawer_outline(&mut app);
    let entries = view.entries();
    assert_eq!(entries[0].place, ::toc::Place::Group { first: true }, "a top-level mod heads a group");
    assert_eq!(entries[0].name, "annotations");
    assert_eq!(
        (entries[1].kind.as_deref(), entries[1].name.as_str(), entries[1].line),
        (Some("fn"), "annotations_session", 39),
        "the keyword is the kind, the rest the name"
    );
    let current: Vec<&str> = entries
        .iter()
        .filter(|entry| entry.current)
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(current, vec!["DocumentFeed"], "the innermost entry holding the caret");
    assert_eq!(view.cursor_title().as_deref(), Some("pub enum DocumentFeed"), "the cursor opens on it");
    let member = entries.iter().find(|entry| entry.name == "default").expect("the member");
    assert_eq!(member.place, ::toc::Place::Entry { strong: false, indent: 1 });
}
