use super::*;
use himark::app::AppFonts;
use himark::app::Application;
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
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    let (source, document) = outlined_document();
    assert!(app.perform_command(himark::app::AppCommand::Opened(
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
    let mut app = Application::new(AppFonts::embedded());
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

    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    let (_source, document) = outlined_document();
    assert!(app.perform_command(himark::app::AppCommand::Opened(
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

    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    let (_source, document) = outlined_document();
    assert!(app.perform_command(himark::app::AppCommand::Opened(
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
        EventResult::Command(::toc::OutlineCommand::List(command)) => {
            use imba::list::ListOps;
            type Search = hikit::list_keyboard::ListKeyboardController<
                hikit::forest::ForestList<::toc::OutlineKey>,
                hikit::forest::ForestSearcher<::toc::OutlineKey>,
            >;
            Search::selected_index(command).is_some()
        }
        _ => false,
    };
    assert!(
        stepped,
        "Down while searching must answer the matched-step Select"
    );

    if let EventResult::Command(command) = result {
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
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    let (_source, document) = outlined_document();
    assert!(app.perform_command(himark::app::AppCommand::Opened(
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
