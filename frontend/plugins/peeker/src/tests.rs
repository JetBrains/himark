// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

#[allow(unused_imports)]
use workbench_shell::app::WorkbenchShell as _;
#[allow(unused_imports)]
use workbench_shell::app_ext::AppExt as _;
#[allow(unused_imports)]
use workbench_shell::test_support::TestSupport as _;

use super::*;
use editor::test_document::plain_document;
use himark::app::AppFonts;
use himark::app::Himark;
use workbench_shell::app_ext::AppExt;

fn boot() -> Himark {
    let mut app = workbench_shell::app::workbench_app(AppFonts::embedded());
    let _ = app.add_window();
    app.register_command(std::sync::Arc::new(TogglePeeker));
    app
}

#[test]
fn the_peeker_toggles_filters_and_picks_through_the_registry() {
    let mut app = boot();
    let mut surface = gfx_skia::raster_tuple((900, 700)).expect("surface");
    app.draw_window(
        app.sole_window(),
        &gfx_skia::SkiaCanvas::new(surface.canvas()),
    );
    let located = |name: &str| {
        editor::location::ResourceLocation::new(
            editor::location::ResourceType::document(),
            editor::location::Authority::new("test"),
            vec!["project".to_owned(), name.to_owned()],
        )
    };
    let open = |app: &mut Himark, name: &str, text: &str| {
        assert!(app.perform_command(workbench_shell::app::opened(
            app.sole_window(),
            workbench_shell::app::OpenedDocument {
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

    open(&mut app, "beta", "beta body");
    assert!(workbench_shell::test_driver::type_text(&mut app, "x"));
    open(&mut app, "alpha", "alpha body");

    assert!(
        app.perform_registered(app.sole_window(), "peeker.toggle"),
        "the peeker opens"
    );
    app.draw_window(
        app.sole_window(),
        &gfx_skia::SkiaCanvas::new(surface.canvas()),
    );
    let listed = labels(&app).expect("the peeker is the modal");
    assert!(
        listed.iter().any(|label| label == "alpha"),
        "open documents list under their names: {listed:?}"
    );
    assert!(
        preview_height(&app, app.store()).is_some(),
        "the selection previews"
    );

    assert!(workbench_shell::test_driver::type_text(&mut app, "beta"));
    let listed = labels(&app).expect("still open");
    assert_eq!(listed, vec!["beta"], "the filter narrowed the list");

    assert!(workbench_shell::test_driver::key(
        &mut app,
        imba::event::Key::Enter,
        Default::default()
    ));
    app.draw_window(
        app.sole_window(),
        &gfx_skia::SkiaCanvas::new(surface.canvas()),
    );
    assert!(labels(&app).is_none(), "picking closes the peeker");
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("xbeta body"),
        "the picked document took the focused pane, unsaved edit intact"
    );

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    assert!(labels(&app).is_some());
    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    assert!(labels(&app).is_none());

    // The sigil lanes: '#' swaps the master to the hits tree and the
    // sheet keeps drawing (the caption and counts read the feed).
    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    assert!(workbench_shell::test_driver::type_text(&mut app, "#lay"));
    app.draw_window(
        app.sole_window(),
        &gfx_skia::SkiaCanvas::new(surface.canvas()),
    );
    assert!(
        labels(&app).is_some(),
        "the peeker stands in the symbol lane"
    );
    assert!(workbench_shell::test_driver::key(
        &mut app,
        imba::event::Key::Escape,
        Default::default()
    ));
    assert!(labels(&app).is_none(), "escape closes the hits lane too");
}

#[test]
fn a_query_keystroke_cancels_the_in_flight_find() {
    use imba::effect::{Batch, Message};
    let mut store = Store::new();
    let ui = ::editor::test_document::test_ui();
    let home = ahp_wire::SessionId::local_default(&store);
    let state = ahp_session::session::state::Hosts::ensure_state(&mut store, &home);
    let (documents, lists) = (state.documents(), state.lists());
    let mut peeker = Peeker::open(
        &mut store,
        &ui,
        documents,
        lists,
        Size::new(800.0, 600.0),
        Vec::new(),
        Vec::new(),
        vec![ResourceLocation::new(
            editor::location::ResourceType::directory(),
            editor::location::Authority::new("test"),
            vec!["project".to_owned()],
        )],
        &mut Batch::new().effects(),
    );

    let find_tokens = |peeker: &mut Peeker, query: &str| {
        let mut batch: Batch<PeekerCommand> = Batch::new();
        peeker.launch_find(&store, &ui, query, &mut batch.effects());
        let mut cancels = Vec::new();
        let mut finds = Vec::new();
        for message in batch.drain() {
            match message {
                Message::Launch(token, effect) if effect.is::<FindEffect>() => finds.push(token),
                Message::Relaunch(previous, token, effect) if effect.is::<FindEffect>() => {
                    cancels.push(previous);
                    finds.push(token);
                }
                Message::Cancel(token) => cancels.push(token),
                _ => {}
            }
        }
        (cancels, finds)
    };

    let (cancels, finds) = find_tokens(&mut peeker, "readme");
    assert!(cancels.is_empty(), "nothing in flight yet");
    assert_eq!(finds.len(), 1, "one find holds the lane");

    let (cancels, next) = find_tokens(&mut peeker, "readmes");
    assert_eq!(cancels, finds, "the keystroke cancels the in-flight find");
    assert_eq!(next.len(), 1, "the lane refills with one");
}

#[test]
fn the_list_caps_at_two_hundred_rows_and_counts_the_rest() {
    use imba::effect::Batch;
    let mut store = Store::new();
    let ui = ::editor::test_document::test_ui();
    let home = ahp_wire::SessionId::local_default(&store);
    let state = ahp_session::session::state::Hosts::ensure_state(&mut store, &home);
    let (documents, lists) = (state.documents(), state.lists());
    let mut peeker = Peeker::open(
        &mut store,
        &ui,
        documents,
        lists,
        Size::new(800.0, 600.0),
        Vec::new(),
        Vec::new(),
        vec![ResourceLocation::new(
            editor::location::ResourceType::directory(),
            editor::location::Authority::new("test"),
            vec!["project".to_owned()],
        )],
        &mut Batch::new().effects(),
    );

    let mut batch: Batch<PeekerCommand> = Batch::new();
    peeker.launch_find(&store, &ui, "file", &mut batch.effects());
    let locations: Vec<ResourceLocation> = (0..250)
        .map(|index| {
            ResourceLocation::new(
                editor::location::ResourceType::document(),
                editor::location::Authority::new("test"),
                vec!["project".to_owned(), format!("file-{index}.md")],
            )
        })
        .collect();
    let ui = ::editor::test_document::test_ui();
    imba::View::perform(
        &mut peeker,
        &mut store,
        &ui,
        PeekerCommand::Found {
            serial: 1,
            locations,
        },
        &mut Batch::new().effects(),
    );

    assert_eq!(
        peeker.labels(&store).len(),
        200,
        "the list stops at the cap"
    );
    assert_eq!(peeker.hidden_count(), 50, "the note counts the rest");
}

/// The jump sheet over a window, a query typed — the redesign's
/// dialog (Jump.dc.html) at the mock's 2x size.
/// `HIMARK_SHOT=<dir> cargo test -p peeker dump_jump_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_jump_screenshot() {
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = boot();
    let mut surface = gfx_skia::raster_tuple((2880, 1800)).expect("surface");
    app.draw_window(
        app.sole_window(),
        &gfx_skia::SkiaCanvas::new(surface.canvas()),
    );
    let files: &[&[&str]] = &[
        &["backend", "agent-host", "src", "main.rs"],
        &["frontend", "ahp-chat", "src", "transcript.rs"],
        &["frontend", "hikit", "src", "panel_tests.rs"],
        &["frontend", "ahp-chat", "src", "composer.rs"],
        &["frontend", "himark", "src", "startup_profile.rs"],
        &["frontend", "ahp-chat", "src", "main.rs"],
        &["frontend", "himark", "src", "app_ext.rs"],
        &["tools", "grammar-pack", "src", "main.rs"],
        &["frontend", "himark", "src", "main.rs"],
        &["frontend", "himark", "src", "app.rs"],
    ];
    let body = "// Copyright © 2026 JetBrains s.r.o.\n// SPDX-License-Identifier: Apache-2.0\n\nfn main() {\n    let out = std::env::args()\n        .nth(1)\n        .expect(\"usage: grammar-pack <out-dir>\");\n    let out = std::path::PathBuf::from(out);\n    std::fs::create_dir_all(&out).expect(\"out dir\");\n}\n";
    for path in files {
        let location = editor::location::ResourceLocation::new(
            editor::location::ResourceType::document(),
            editor::location::Authority::new("test"),
            path.iter()
                .map(|segment| segment.to_string())
                .collect::<Vec<_>>(),
        );
        assert!(app.perform_command(workbench_shell::app::opened(
            app.sole_window(),
            workbench_shell::app::OpenedDocument {
                documents: app.sole_documents(),
                name: path.last().unwrap().to_string(),
                document: plain_document(body),
                location: Some(location),
                primary: true,
                target: None,
                focus: false,
            },
        )));
    }
    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    app.draw_window(
        app.sole_window(),
        &gfx_skia::SkiaCanvas::new(surface.canvas()),
    );
    assert!(workbench_shell::test_driver::type_text(&mut app, "ar"));
    for tick in 0..30 {
        app.draw_window(
            app.sole_window(),
            &gfx_skia::SkiaCanvas::new(surface.canvas()),
        );
        let busy = workbench_shell::test_driver::animate(
            &mut app,
            imba::anim::AnimationClock::from_millis(tick as f64 * 32.0),
        );
        if !busy && tick > 1 {
            break;
        }
    }
    let data = surface
        .image_snapshot()
        .encode(None, gfx_skia::skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::create_dir_all(&dir).expect("the shot dir");
    std::fs::write(format!("{dir}/jump.png"), data.as_bytes()).expect("write");
}

#[test]
fn sigil_queries_name_their_lane() {
    assert_eq!(sniff("#Catalog"), (QueryMode::Symbols, "Catalog"));
    assert_eq!(sniff("# Catalog "), (QueryMode::Symbols, "Catalog"));
    assert_eq!(sniff("%needle words"), (QueryMode::Text, "needle words"));
    assert_eq!(sniff("main.rs"), (QueryMode::Files, "main.rs"));
    assert_eq!(sniff(""), (QueryMode::Files, ""));
}

#[test]
fn the_hits_lanes_ride_a_peeker_owned_feed() {
    use imba::effect::Batch;
    let mut store = Store::new();
    let ui = ::editor::test_document::test_ui();
    let home = ahp_wire::SessionId::local_default(&store);
    let state = ahp_session::session::state::Hosts::ensure_state(&mut store, &home);
    let (documents, lists) = (state.documents(), state.lists());
    let mut peeker = Peeker::open(
        &mut store,
        &ui,
        documents,
        lists,
        Size::new(800.0, 600.0),
        Vec::new(),
        Vec::new(),
        vec![ResourceLocation::new(
            editor::location::ResourceType::directory(),
            editor::location::Authority::new("test"),
            vec!["project".to_owned()],
        )],
        &mut Batch::new().effects(),
    );
    let _ = locations::LocationLists::take_asks(&mut store, lists);

    // The symbol lane swaps the master in, mints a feed and NOTES
    // the ask once; a caret move with the same term re-asks nothing.
    peeker.enter_hits(&mut store, &ui, HitsKind::Symbols, "Catalog");
    let feed = peeker
        .hits_lane()
        .expect("the hits master stands")
        .feed
        .expect("a feed fronts the master");
    let asks = locations::LocationLists::take_asks(&mut store, lists);
    assert!(
        matches!(asks.as_slice(),
            [locations::LocationsAsk::Symbols { feed: asked, query }]
                if *asked == feed && query == "Catalog"),
        "one symbols ask"
    );
    peeker.enter_hits(&mut store, &ui, HitsKind::Symbols, "Catalog");
    assert!(
        locations::LocationLists::take_asks(&mut store, lists).is_empty(),
        "the dedup holds"
    );

    // Switching sigils disposes the standing feed and asks the stream.
    peeker.enter_hits(&mut store, &ui, HitsKind::Text, "Catalog");
    let asks = locations::LocationLists::take_asks(&mut store, lists);
    assert!(
        matches!(asks.as_slice(),
            [locations::LocationsAsk::Dispose(disposed), locations::LocationsAsk::Search { query, .. }]
                if *disposed == feed && query == "Catalog"),
        "dispose, then the text ask"
    );

    // A landed hit renders in the shared results tree, and picking it
    // opens AT the hit's span.
    let feed = peeker
        .hits_lane()
        .expect("still the hits master")
        .feed
        .expect("the text feed");
    let hit = ResourceLocation::new(
        editor::location::ResourceType::document(),
        editor::location::Authority::new("test"),
        vec!["project".to_owned(), "hit.md".to_owned()],
    );
    locations::LocationLists::fold_locations(
        &mut store,
        lists,
        feed,
        vec![locations::FoundLocation {
            location: hit.clone(),
            line: 3,
            column: 2,
            length: 6,
            context: "a needle here".to_owned(),
            context_column_start: 0,
        }],
        true,
        false,
    );
    let mut batch: Batch<PeekerCommand> = Batch::new();
    imba::View::perform(
        &mut peeker,
        &mut store,
        &ui,
        PeekerCommand::RefreshResults,
        &mut batch.effects(),
    );
    assert!(
        !peeker.hits_lane().expect("hits").results.is_empty(),
        "the hit landed in the tree"
    );

    let mut batch: Batch<PeekerCommand> = Batch::new();
    peeker.open_hit(
        &mut store,
        &ui,
        locations::LocationKey::Hit(hit.clone(), 3, 2),
        &mut batch.effects(),
    );
    let request = hikit::modal::ModalView::take_request(&mut peeker);
    match request {
        Some(hikit::modal::ModalRequest::OpenAt {
            location,
            target: Some(target),
            focus,
        }) => {
            assert_eq!(location, hit);
            assert!(focus, "a pick is the deliberate jump");
            assert_eq!((target.start.line, target.start.col), (3, 2));
            assert_eq!(target.end.col, 8, "start + length");
        }
        _ => panic!("expected OpenAt"),
    }
    // The pick disposed the feed with the overlay's end.
    let asks = locations::LocationLists::take_asks(&mut store, lists);
    assert!(
        matches!(asks.as_slice(), [locations::LocationsAsk::Dispose(disposed)] if *disposed == feed),
        "the feed dies with the pick"
    );
}
