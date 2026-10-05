use super::*;
use ahp_locations::driver::DisposeFeed;
use locations::FeedId;
use locations::FoundLocation;
use locations::LocationLists;
use locations::LocationsFeedRow;
use himark::app::AppCommand;
use himark::app::AppFonts;
use himark::app::Application;
use himark::app::OpenedDocument;
use std::sync::Arc;

fn located(name: &str) -> editor::location::ResourceLocation {
    editor::location::ResourceLocation::new(
        editor::location::ResourceType::document(),
        editor::location::Authority::new("local"),
        vec!["work".to_owned(), name.to_owned()],
    )
}

fn state_lists(app: &Application) -> imba::store::Id<LocationLists> {
    let window = app.sole_window();
    himark::workspace::entity_state(
        ::workbench::window::Windows::window_ref(app.store(), window)
            .expect("the window entity"),
    )
    .lists()
}

fn state_wire(app: &Application) -> imba::store::Id<ahp_locations::driver::LocationsWire> {
    let window = app.sole_window();
    himark::workspace::entity_state(
        ::workbench::window::Windows::window_ref(app.store(), window)
            .expect("the window entity"),
    )
    .locations_wire()
}

fn seeded_feed(app: &mut Application, name: &str) -> (imba::store::Id<LocationLists>, FeedId) {
    let lists = state_lists(app);
    let feed = FeedId::mint();
    let mut row = LocationsFeedRow {
        title: "Search: needle".to_owned(),
        generation: 1,
        done: true,
        ..Default::default()
    };
    for (line, column) in [(0u32, 0u32), (1, 4)] {
        row.locations.push_back_mut(FoundLocation {
            location: located(name),
            line,
            column,
            length: 6,
            context: "needle".to_owned(),
            context_column_start: 0,
        });
    }
    row.locations.push_back_mut(FoundLocation {
        location: located("other.md"),
        line: 0,
        column: 0,
        length: 6,
        context: "needle".to_owned(),
        context_column_start: 0,
    });
    LocationLists::put(&mut app.store_mut(), lists, feed, row);
    (lists, feed)
}

#[test]
fn a_search_pick_washes_the_opened_editor() {
    let mut app = Application::new(AppFonts::embedded());
    let window = app.add_window();
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "hit.md".to_owned(),
            document: ::editor::test_document::plain_document("needle one\nfour needle\n"),
            location: Some(located("hit.md")),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    let (lists, feed) = seeded_feed(&mut app, "hit.md");

    // The already-open pick path: the wash lands through the
    // drained request, resolved against live text — the pick
    // pushes it view-side now.
    let washed = documents::OpenDocuments::by_location(
        app.store(),
        app.sole_documents(),
        &located("hit.md"),
    )
    .expect("the document is open");
    assert!(
        app.perform_command(AppCommand::Verb(imba::command::Verb::Dynamic(Arc::new(
            locations::views::WashDocument {
                lists,
                feed,
                document: washed,
            }
        ),)))
    );
    // Requests drain on the next content tick, as in the live app.
    assert!(app.perform_command(AppCommand::Content(
        window,
        ::workbench::window::WindowCommand::Focus(::workbench::window::LayerFocus::Content),
    )));
    let row = LocationLists::row(app.store(), lists, feed).expect("the feed");
    assert_eq!(row.washes.size(), 1, "the opened document is washed");
    let (document, (_, pushed)) = row.washes.iter().next().expect("the wash");
    let ranges: Vec<(u32, u32)> = pushed.iter().copied().collect();
    assert_eq!(
        ranges,
        [(0, 6), (15, 21)],
        "every occurrence in the file, byte-resolved"
    );

    // Disposal removes the wash and survives the walk.
    let wire = state_wire(&app);
    let _ = window;
    assert!(
        app.perform_command(AppCommand::Verb(imba::command::Verb::Dynamic(Arc::new(
            DisposeFeed { wire, feed }
        ),)))
    );
    assert!(LocationLists::row(app.store(), lists, feed).is_none());
    assert!(
        documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), *document)
            .is_some(),
        "the document stays; only the wash left"
    );
}

#[test]
fn a_pick_before_the_open_washes_at_registration() {
    let mut app = Application::new(AppFonts::embedded());
    let window = app.add_window();
    let (lists, feed) = seeded_feed(&mut app, "late.md");

    // The async-open path: the pick notes the pending wash; the
    // document hook converts it when registration lands.
    LocationLists::note_wash(&mut app.store_mut(), lists, located("late.md"), feed);
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "late.md".to_owned(),
            document: ::editor::test_document::plain_document("needle one\nfour needle\n"),
            location: Some(located("late.md")),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    assert!(app.perform_command(AppCommand::Content(
        window,
        ::workbench::window::WindowCommand::Focus(::workbench::window::LayerFocus::Content),
    )));
    let row = LocationLists::row(app.store(), lists, feed).expect("the feed");
    assert_eq!(
        row.washes.size(),
        1,
        "the hook washed the registration: {:?}",
        row.washes.size()
    );
}
