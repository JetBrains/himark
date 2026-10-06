use super::plain_document;
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

fn app() -> (Application, ::workbench::window::WindowId) {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let window = app.sole_window();
    (app, window)
}

fn land(
    app: &mut Application,
    window: ::workbench::window::WindowId,
    name: &str,
    text: &str,
    col: Option<u32>,
) {
    assert!(app.perform_command(himark::app::AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: name.to_owned(),
            document: plain_document(text),
            location: Some(located(name)),
            primary: true,
            target: col.map(|col| {
                documents::text_ext::LineCol { line: 0, col }..documents::text_ext::LineCol {
                    line: 0,
                    col: col + 1,
                }
            }),
            focus: false,
        },
    )));
}

fn depths(app: &Application) -> (usize, usize) {
    ::workbench::window::Windows::window_ref(app.store(), app.sole_window())
        .expect("the window")
        .focused_history_depths()
}

fn focused_text(app: &Application) -> String {
    app.focused_document_text().expect("an editor pane")
}

fn caret(app: &Application) -> u32 {
    let (document, editor) = app.focused_editor_id();
    documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), document)
        .expect("document")
        .caret_byte(editor)
}

fn registered(app: &Application, name: &str) -> bool {
    documents::OpenDocuments::by_location(app.store(), app.sole_documents(), &located(name))
        .is_some()
}

#[test]
fn back_into_a_released_document_completes_the_walk_on_landing() {
    let (mut app, window) = app();
    land(&mut app, window, "a.md", "alpha\n", None);
    land(&mut app, window, "b.md", "beta\n", None);
    assert_eq!(focused_text(&app), "beta\n");
    assert_eq!(depths(&app), (1, 0), "the displaced place recorded");
    assert!(
        !registered(&app, "a.md"),
        "clean + displaced = released (the everyday case)"
    );

    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(focused_text(&app), "beta\n", "still parked");
    assert_eq!(depths(&app), (1, 0), "the entry stays for the landing");

    land(&mut app, window, "a.md", "alpha\n", None);
    assert_eq!(focused_text(&app), "alpha\n", "the walk came home");
    assert_eq!(depths(&app), (0, 1), "back popped; b.md on forward");

    assert!(app.perform_registered(window, "navigation.forward"));
    assert_eq!(focused_text(&app), "alpha\n", "parked again");
    land(&mut app, window, "b.md", "beta\n", None);
    assert_eq!(focused_text(&app), "beta\n", "forward came home");
    assert_eq!(depths(&app), (1, 0), "the round trip restored the shape");
}

#[test]
fn back_and_forward_walk_a_three_deep_chain() {
    let (mut app, window) = app();
    for (name, text) in [("a.md", "alpha\n"), ("b.md", "beta\n"), ("c.md", "gamma\n")] {
        land(&mut app, window, name, text, None);

        assert!(himark::test_driver::type_text(&mut app, "x"));
    }
    assert_eq!(focused_text(&app), "xgamma\n");
    assert_eq!(depths(&app), (2, 0));

    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(focused_text(&app), "xbeta\n");
    assert_eq!(depths(&app), (1, 1));
    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(focused_text(&app), "xalpha\n");
    assert_eq!(depths(&app), (0, 2));

    // At the bottom of the stack: the command may refuse, but the
    // place must not move.
    let _ = app.perform_registered(window, "navigation.back");
    assert_eq!(focused_text(&app), "xalpha\n");
    assert_eq!(depths(&app), (0, 2));

    assert!(app.perform_registered(window, "navigation.forward"));
    assert_eq!(focused_text(&app), "xbeta\n");
    assert_eq!(depths(&app), (1, 1));
    assert!(app.perform_registered(window, "navigation.forward"));
    assert_eq!(focused_text(&app), "xgamma\n");
    assert_eq!(depths(&app), (2, 0));
    assert_eq!(focused_text(&app), "xgamma\n");
    assert_eq!(depths(&app), (2, 0), "the far end no-ops");
}

#[test]
fn back_restores_the_recorded_caret_across_a_release() {
    let (mut app, window) = app();
    land(&mut app, window, "a.md", "alpha\n", None);

    land(&mut app, window, "a.md", "alpha\n", Some(3));
    assert_eq!(caret(&app), 3);
    assert_eq!(depths(&app), (1, 0), "the jump recorded where it left");

    land(&mut app, window, "b.md", "beta\n", None);
    assert_eq!(depths(&app), (2, 0), "displacement recorded A@3");
    assert!(!registered(&app, "a.md"), "released clean");

    assert!(app.perform_registered(window, "navigation.back"));
    land(&mut app, window, "a.md", "alpha\n", None);
    assert_eq!(focused_text(&app), "alpha\n");
    assert_eq!(caret(&app), 3, "the walk restored the recorded caret");
    assert_eq!(depths(&app), (1, 1));

    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(caret(&app), 0, "the same-file step walked the caret");
    assert_eq!(depths(&app), (0, 2));
}

#[test]
fn back_walks_carets_within_one_document() {
    let (mut app, window) = app();
    land(&mut app, window, "a.md", "alpha beta\n", None);
    land(&mut app, window, "a.md", "alpha beta\n", Some(3));
    land(&mut app, window, "a.md", "alpha beta\n", Some(7));
    assert_eq!(caret(&app), 7);
    assert_eq!(depths(&app), (2, 0));

    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(caret(&app), 3);
    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(caret(&app), 0);
    assert_eq!(depths(&app), (0, 2));
    assert!(app.perform_registered(window, "navigation.forward"));
    assert!(app.perform_registered(window, "navigation.forward"));
    assert_eq!(caret(&app), 7);
    assert_eq!(depths(&app), (2, 0));
}

#[test]
fn reopening_the_focused_document_is_a_no_op() {
    let (mut app, window) = app();
    land(&mut app, window, "a.md", "alpha beta\n", None);
    land(&mut app, window, "a.md", "alpha beta\n", Some(7));
    land(&mut app, window, "b.md", "beta\n", None);
    assert!(app.perform_registered(window, "navigation.back"));
    land(&mut app, window, "a.md", "alpha beta\n", None);
    assert_eq!(caret(&app), 7);
    let shape = depths(&app);
    assert_eq!(shape.1, 1, "forward holds b.md");

    land(&mut app, window, "a.md", "alpha beta\n", None);
    assert_eq!(caret(&app), 7, "the caret did not jump to 0");
    assert_eq!(depths(&app), shape, "nothing recorded, forward intact");
}

#[test]
fn a_new_navigation_clears_forward() {
    let (mut app, window) = app();
    for (name, text) in [("a.md", "alpha\n"), ("b.md", "beta\n")] {
        land(&mut app, window, name, text, None);
        assert!(himark::test_driver::type_text(&mut app, "x"));
    }
    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(depths(&app), (0, 1), "b.md on forward");

    land(&mut app, window, "c.md", "gamma\n", None);
    assert_eq!(
        depths(&app),
        (1, 0),
        "the branch recorded a.md and forgot redo"
    );
    // Past the top of the stack: the command may refuse, but the
    // place must not move.
    let _ = app.perform_registered(window, "navigation.forward");
    assert_eq!(focused_text(&app), "gamma\n", "no forward to walk");
}

#[test]
fn close_reveals_the_previous_place_with_its_caret() {
    let (mut app, window) = app();
    land(&mut app, window, "a.md", "alpha\n", None);
    land(&mut app, window, "a.md", "alpha\n", Some(3));
    land(&mut app, window, "b.md", "beta\n", None);
    assert_eq!(depths(&app), (2, 0));
    assert!(!registered(&app, "a.md"), "released clean");

    assert!(app.perform_registered(window, "workbench.close"));
    assert!(!registered(&app, "b.md"), "closed for real");

    land(&mut app, window, "a.md", "alpha\n", None);
    assert_eq!(focused_text(&app), "alpha\n");
    assert_eq!(caret(&app), 3, "the reveal restored the recorded caret");
    assert_eq!(depths(&app), (1, 0), "popped at close; nothing recorded");
}

#[test]
fn close_reveals_a_dirty_previous_place_instantly() {
    let (mut app, window) = app();
    land(&mut app, window, "a.md", "alpha\n", None);
    assert!(himark::test_driver::type_text(&mut app, "zz"));
    land(&mut app, window, "b.md", "beta\n", None);
    assert!(app.perform_registered(window, "workbench.close"));
    assert_eq!(focused_text(&app), "zzalpha\n", "instant remount");
    assert_eq!(caret(&app), 2, "at the recorded caret");
    assert_eq!(depths(&app), (0, 0));
}

#[test]
fn split_copies_history_and_walks_independently() {
    let (mut app, window) = app();
    for (name, text) in [("a.md", "alpha\n"), ("b.md", "beta\n")] {
        land(&mut app, window, name, text, None);
        assert!(himark::test_driver::type_text(&mut app, "x"));
    }
    assert_eq!(depths(&app), (1, 0));
    assert!(app.perform_registered(window, "workbench.split-pane"));
    assert_eq!(depths(&app), (1, 0), "the focused (new) pane inherited");
    assert!(app.perform_registered(window, "navigation.back"));
    assert_eq!(focused_text(&app), "xalpha\n", "the copy walks");
    assert_eq!(depths(&app), (0, 1));
}

#[test]
fn a_new_navigation_cancels_a_parked_walk() {
    let (mut app, window) = app();
    land(&mut app, window, "a.md", "alpha\n", None);
    land(&mut app, window, "b.md", "beta\n", None);
    assert!(app.perform_registered(window, "navigation.back"));
    land(&mut app, window, "c.md", "gamma\n", None);
    assert_eq!(focused_text(&app), "gamma\n");

    land(&mut app, window, "a.md", "alpha\n", None);
    assert_eq!(focused_text(&app), "alpha\n");
    assert_eq!(caret(&app), 0, "a fresh navigation, not the stale walk");
    assert!(
        app.perform_registered(window, "navigation.back"),
        "back returns to c.md — the fresh record"
    );
    land(&mut app, window, "c.md", "gamma\n", None);
    assert_eq!(focused_text(&app), "gamma\n");
}

#[test]
fn exhausted_stacks_no_op() {
    let (mut app, window) = app();
    land(&mut app, window, "a.md", "alpha\n", None);
    assert_eq!(depths(&app), (0, 0));
    let _ = app.perform_registered(window, "navigation.back");
    let _ = app.perform_registered(window, "navigation.forward");
    assert_eq!(focused_text(&app), "alpha\n");
    assert_eq!(depths(&app), (0, 0));
}
