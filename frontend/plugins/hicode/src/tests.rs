// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

use ::editor::test_document::plain_document;
use himark::app::AppCommand;
use himark::app_ext::AppExt;
use himark::app::AppFonts;
use himark::app::Application;
use himark::app::OpenedDocument;
use std::sync::{mpsc, Arc};

fn document_location(name: &str) -> ResourceLocation {
    ResourceLocation::new(
        editor::location::ResourceType::document(),
        editor::location::Authority::new("local"),
        vec!["project".to_owned(), name.to_owned()],
    )
}

fn lc(line: u32, col: u32) -> LineCol {
    LineCol { line, col }
}

#[test]
fn identifier_at_clips_the_word_under_the_caret() {
    let text = text::text::Text::from_string("let frob_nicate2 = 7;");
    let mut view = text.view();
    assert_eq!(identifier_at(&mut view, 6), "frob_nicate2");
    assert_eq!(identifier_at(&mut view, 4), "frob_nicate2");
    assert_eq!(identifier_at(&mut view, 17), "");
    let empty = text::text::Text::from_string("");
    assert_eq!(identifier_at(&mut empty.view(), 0), "");
}

struct StubNavigation {
    targets: Option<Vec<CodeTarget>>,
    built: Vec<(ResourceLocation, Document)>,
}

impl imba::effect::EffectHandler<CodeNavigationEffect> for StubNavigation {
    async fn handle(&self, effect: CodeNavigationEffect) -> NavigationOutcome {
        NavigationOutcome {
            title: effect.title,
            targets: self.targets.clone(),
            built: self.built.clone(),
        }
    }
}

fn app_with_located_document(source: &str) -> (Application, workbench::window::WindowId) {
    let mut app = Application::new(AppFonts::embedded());
    app.register_document_command(Arc::new(GoDefinition));
    app.register_document_command(Arc::new(GoReferences));
    let window = app.add_window();
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "a.rs".to_owned(),
            document: plain_document(source),
            location: Some(document_location("a.rs")),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    (app, window)
}

fn invoke(app: &mut Application, window: workbench::window::WindowId, id: &str) {
    let command = himark::commands::palette_commands(app.store(), &app.ui_handle(), window)
        .into_iter()
        .find(|presentable| presentable.id == id)
        .expect("the located editor offers the command")
        .command;
    assert!(app.perform_command(command));
}

#[test]
fn located_editors_offer_the_commands_on_the_focus_path() {
    let mut app = Application::new(AppFonts::embedded());
    app.register_document_command(Arc::new(GoDefinition));
    let window = app.add_window();
    let listed = |app: &Application| {
        himark::commands::palette_commands(app.store(), &app.ui_handle(), window)
            .iter()
            .any(|presentable| presentable.id == "code.definition")
    };
    assert!(!listed(&app), "the startup scratch has no location");
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "a.rs".to_owned(),
            document: plain_document("hello"),
            location: Some(document_location("a.rs")),
            primary: true,
            target: None,
            focus: false,
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

/// A seat that serves ONLY the locations trio: subscribe answers a
/// canned stream snapshot, poll parks, everything else is
/// unreachable in these tests.
struct StreamSeat {
    snapshot: himark_ahp_ext_types::locations::LocationList,
}

impl ahp_wire::client::LocationsClient for StreamSeat {
    fn subscribe_locations(
        &self,
        _channel: ahp_wire::client::ChannelUri,
    ) -> ahp_wire::client::ClientFuture<Result<himark_ahp_ext_types::locations::LocationList, String>> {
        let snapshot = self.snapshot.clone();
        Box::pin(std::future::ready(Ok(snapshot)))
    }

    // poll_locations keeps its default: parked — the stream is done
    // in the snapshot. unsubscribe_locations keeps its default no-op.
}

/// The registry's thin seat handlers, test-side (hiahp registers
/// these in production).
struct SeatSubscribeLocations;

impl imba::effect::EffectHandler<ahp_wire::effects::SubscribeLocationsEffect> for SeatSubscribeLocations {
    async fn handle(
        &self,
        effect: ahp_wire::effects::SubscribeLocationsEffect,
    ) -> Result<himark_ahp_ext_types::locations::LocationList, String> {
        effect.client.subscribe_locations(effect.channel).await
    }
}

struct SeatPollLocations;

impl imba::effect::EffectHandler<ahp_wire::effects::PollLocationsEffect> for SeatPollLocations {
    async fn handle(
        &self,
        effect: ahp_wire::effects::PollLocationsEffect,
    ) -> Vec<himark_ahp_ext_types::locations::LocationList> {
        effect.client.poll_locations(effect.channel).await
    }
}

struct SeatUnsubscribeLocations;

impl imba::effect::EffectHandler<ahp_wire::effects::UnsubscribeLocationsEffect>
    for SeatUnsubscribeLocations
{
    async fn handle(&self, effect: ahp_wire::effects::UnsubscribeLocationsEffect) {
        effect.client.unsubscribe_locations(&effect.channel);
    }
}

struct StubLspLocations {
    snapshot: himark_ahp_ext_types::locations::LocationList,
}

impl imba::effect::EffectHandler<ahp_locations::LspLocationsEffect> for StubLspLocations {
    async fn handle(
        &self,
        _effect: ahp_locations::LspLocationsEffect,
    ) -> Result<ahp_locations::LocationsChannel, String> {
        Ok(ahp_locations::LocationsChannel {
            client: Arc::new(StreamSeat {
                snapshot: self.snapshot.clone(),
            }),
            channel: ahp_wire::client::ChannelUri::new("ahp-locations:/test"),
            resolve: Arc::new(|uri| {
                let name = uri.strip_prefix("test:/")?;
                Some(document_location(name))
            }),
        })
    }
}

fn wire_location(
    uri: &str,
    line: u32,
    column: u32,
    context: &str,
) -> himark_ahp_ext_types::locations::Location {
    himark_ahp_ext_types::locations::Location {
        uri: uri.to_owned(),
        line,
        column,
        length: 4,
        context: context.to_owned(),
        context_column_start: 0,
    }
}

#[test]
fn references_stream_into_the_search_dock() {
    let (mut app, window) = app_with_located_document(&"line one two\n".repeat(30));
    app.register_handler::<ahp_wire::effects::SubscribeLocationsEffect>(SeatSubscribeLocations);
    app.register_handler::<ahp_wire::effects::PollLocationsEffect>(SeatPollLocations);
    app.register_handler::<ahp_wire::effects::UnsubscribeLocationsEffect>(SeatUnsubscribeLocations);
    app.register_handler::<ahp_locations::LspLocationsEffect>(StubLspLocations {
        snapshot: himark_ahp_ext_types::locations::LocationList {
            locations: vec![
                wire_location("test:/a.rs", 0, 5, "line one two"),
                wire_location("test:/a.rs", 20, 5, "line one two"),
                wire_location("test:/far.rs", 0, 0, "fee fie foe"),
            ],
            done: true,
            truncated: false,
        },
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
        let _ = app.draw_window(window, surface.canvas());
    }

    let entity = workbench::window::Windows::window_ref(app.store(), window).expect("the window");
    assert_eq!(
        entity.dock_owner(),
        Some(himark::hisearch::OWNER),
        "the Search tab activated"
    );
    let lists = entity.state().lists();
    let feed = locations::LocationLists::search(app.store(), lists)
        .expect("the session fronts the feed");
    let row =
        locations::LocationLists::row(app.store(), lists, feed).expect("the feed row");
    assert_eq!(row.title, "References to `line`");
    assert!(row.done && !row.truncated);
    assert_eq!(row.locations.len(), 3, "the stream landed, resolved");
    assert_eq!(
        row.locations
            .iter()
            .filter(|found| found.location.name().ends_with("far.rs"))
            .count(),
        1
    );
    assert_eq!(
        app.document_count(),
        documents_before,
        "NOTHING was fetched or registered before navigation"
    );
}
