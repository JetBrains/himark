// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

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
            focus: false,
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
    snapshot: himark_ahp_ext_types::LocationList,
}

impl himark::higent::AhpServer for StreamSeat {
    fn connect(&self) -> himark::higent::SeatFuture<Result<himark::higent::RootInfo, String>> {
        unreachable!()
    }
    fn list_sessions(
        &self,
        _cursor: Option<String>,
    ) -> himark::higent::SeatFuture<Result<himark::higent::SessionsPage, String>> {
        unreachable!()
    }
    fn poll_root(&self) -> himark::higent::SeatFuture<Vec<himark::higent::ServerEvent>> {
        unreachable!()
    }
    fn create_session(
        &self,
        _working_directories: Vec<String>,
        _options: himark::higent::SessionOptions,
    ) -> himark::higent::SeatFuture<Result<String, String>> {
        unreachable!()
    }
    fn resolve_session_config(
        &self,
        _working_directory: Option<String>,
        _config: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> himark::higent::SeatFuture<
        Result<himark::higent::ahp_types::commands::ResolveSessionConfigResult, String>,
    > {
        unreachable!()
    }
    fn dispose_session(&self, _session: String) -> himark::higent::SeatFuture<Result<(), String>> {
        unreachable!()
    }
    fn subscribe_session(
        &self,
        _session: String,
    ) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::state::SessionState, String>>
    {
        unreachable!()
    }
    fn poll_session(
        &self,
        _session: String,
    ) -> himark::higent::SeatFuture<Vec<himark::higent::ahp_types::actions::StateAction>> {
        unreachable!()
    }
    fn create_chat(&self, _session: String) -> himark::higent::SeatFuture<Result<String, String>> {
        unreachable!()
    }
    fn subscribe_chat(
        &self,
        _chat: String,
    ) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::state::ChatState, String>>
    {
        unreachable!()
    }
    fn fetch_turns(
        &self,
        _chat: String,
        _cursor: Option<String>,
    ) -> himark::higent::SeatFuture<Result<himark::higent::TurnsPage, String>> {
        unreachable!()
    }
    fn start_turn(
        &self,
        _chat: String,
        _text: String,
        _attachments: Option<Vec<himark::higent::ahp_types::state::MessageAttachment>>,
        _model: Option<himark::higent::ahp_types::state::ModelSelection>,
    ) -> himark::higent::SeatFuture<Result<(), String>> {
        unreachable!()
    }
    fn poll_chat(
        &self,
        _chat: String,
    ) -> himark::higent::SeatFuture<Vec<himark::higent::ahp_types::actions::StateAction>> {
        unreachable!()
    }
    fn cancel_turn(&self, _chat: String, _turn_id: String) -> himark::higent::SeatFuture<()> {
        unreachable!()
    }
    fn dispatch_action(
        &self,
        _channel: String,
        _action: himark::higent::ahp_types::actions::StateAction,
    ) -> himark::higent::SeatFuture<Result<(), String>> {
        unreachable!()
    }
    fn read_file_edit(
        &self,
        _before: Option<String>,
        _after: Option<String>,
    ) -> himark::higent::SeatFuture<Result<himark::higent::FileEditContents, String>> {
        unreachable!()
    }
    fn resource_read(
        &self,
        _session: String,
        _uri: himark::higent::ResourceUri,
    ) -> himark::higent::SeatFuture<Option<String>> {
        unreachable!()
    }
    fn resource_write(
        &self,
        _session: String,
        _uri: himark::higent::ResourceUri,
        _text: String,
    ) -> himark::higent::SeatFuture<bool> {
        unreachable!()
    }
    fn resource_list(
        &self,
        _session: String,
        _uri: himark::higent::ResourceUri,
    ) -> himark::higent::SeatFuture<Option<Vec<(String, bool)>>> {
        unreachable!()
    }
    fn resource_watch(
        &self,
        _session: String,
        _uri: himark::higent::ResourceUri,
        _events: Arc<dyn Fn() + Send + Sync>,
    ) -> himark::higent::SeatFuture<Option<himark::higent::WatchHandle>> {
        unreachable!()
    }
    fn resource_unwatch(
        &self,
        _handle: himark::higent::WatchHandle,
    ) -> himark::higent::SeatFuture<()> {
        unreachable!()
    }
    fn search(
        &self,
        _session: String,
        _ask: himark::higent::SearchAsk,
    ) -> himark::higent::SeatFuture<Option<himark::higent::SearchResult>> {
        unreachable!()
    }
    fn terminal_open(
        &self,
        _session: String,
        _channel: String,
        _cwd: Option<String>,
        _cols: u16,
        _rows: u16,
        _events: Arc<dyn Fn(himark::higent::TerminalEvent) + Send + Sync>,
    ) -> himark::higent::SeatFuture<Option<himark::higent::TerminalHandle>> {
        unreachable!()
    }
    fn terminal_input(&self, _channel: &String, _data: String) {
        unreachable!()
    }
    fn terminal_resize(&self, _channel: &String, _cols: u16, _rows: u16) {
        unreachable!()
    }
    fn terminal_dispose(&self, _channel: &String) {
        unreachable!()
    }
    fn subscribe_changeset(
        &self,
        _channel: String,
    ) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::state::ChangesetState, String>>
    {
        unreachable!()
    }
    fn poll_changeset(
        &self,
        _channel: String,
    ) -> himark::higent::SeatFuture<Vec<himark::higent::ahp_types::actions::StateAction>> {
        unreachable!()
    }
    fn unsubscribe_changeset(&self, _channel: &String) {
        unreachable!()
    }
    fn subscribe_annotations(
        &self,
        _session: String,
    ) -> himark::higent::SeatFuture<
        Result<himark::higent::ahp_types::state::AnnotationsState, String>,
    > {
        unreachable!()
    }
    fn poll_annotations(
        &self,
        _session: String,
    ) -> himark::higent::SeatFuture<Vec<himark::higent::ahp_types::actions::StateAction>> {
        unreachable!()
    }
    fn dispatch_annotations(
        &self,
        _session: &String,
        _action: himark::higent::ahp_types::actions::StateAction,
    ) {
        unreachable!()
    }
    fn unsubscribe_annotations(&self, _session: &String) {
        unreachable!()
    }
    fn open_document(
        &self,
        _session: String,
        _uri: Option<himark::higent::ResourceUri>,
        _text: Option<String>,
    ) -> himark::higent::SeatFuture<Result<himark_ahp_ext_types::OpenDocumentResult, String>> {
        unreachable!()
    }
    fn subscribe_document(
        &self,
        _channel: String,
    ) -> himark::higent::SeatFuture<Result<himark_ahp_ext_types::DocumentState, String>> {
        unreachable!()
    }
    fn poll_document(
        &self,
        _channel: String,
    ) -> himark::higent::SeatFuture<Vec<himark_ahp_ext_types::DocumentApplied>> {
        unreachable!()
    }
    fn dispatch_document(&self, _channel: &String, _action: himark_ahp_ext_types::DocumentApplied) {
        unreachable!()
    }
    fn unsubscribe_document(&self, _channel: &String) -> himark::higent::SeatFuture<()> {
        Box::pin(std::future::ready(()))
    }
    fn lsp(
        &self,
        _session: String,
        _method: String,
        _params: serde_json::Value,
    ) -> himark::higent::SeatFuture<Result<serde_json::Value, String>> {
        unreachable!()
    }

    fn subscribe_locations(
        &self,
        _channel: String,
    ) -> himark::higent::SeatFuture<Result<himark_ahp_ext_types::LocationList, String>> {
        let snapshot = self.snapshot.clone();
        Box::pin(std::future::ready(Ok(snapshot)))
    }
    // poll_locations keeps its default: parked — the stream is done
    // in the snapshot. unsubscribe_locations keeps its default no-op.
}

/// The registry's thin seat handlers, test-side (hiahp registers
/// these in production).
struct SeatSubscribeLocations;

impl himark::EffectHandler<himark::higent::SubscribeLocationsEffect> for SeatSubscribeLocations {
    async fn handle(
        &self,
        effect: himark::higent::SubscribeLocationsEffect,
    ) -> Result<himark_ahp_ext_types::LocationList, String> {
        effect.seat.subscribe_locations(effect.channel).await
    }
}

struct SeatPollLocations;

impl himark::EffectHandler<himark::higent::PollLocationsEffect> for SeatPollLocations {
    async fn handle(
        &self,
        effect: himark::higent::PollLocationsEffect,
    ) -> Vec<himark_ahp_ext_types::LocationList> {
        effect.seat.poll_locations(effect.channel).await
    }
}

struct SeatUnsubscribeLocations;

impl himark::EffectHandler<himark::higent::UnsubscribeLocationsEffect>
    for SeatUnsubscribeLocations
{
    async fn handle(&self, effect: himark::higent::UnsubscribeLocationsEffect) {
        effect.seat.unsubscribe_locations(&effect.channel);
    }
}

struct StubLspLocations {
    snapshot: himark_ahp_ext_types::LocationList,
}

impl himark::EffectHandler<himark::LspLocationsEffect> for StubLspLocations {
    async fn handle(
        &self,
        _effect: himark::LspLocationsEffect,
    ) -> Result<himark::LocationsChannel, String> {
        Ok(himark::LocationsChannel {
            seat: Arc::new(StreamSeat {
                snapshot: self.snapshot.clone(),
            }),
            channel: "ahp-locations:/test".to_owned(),
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
) -> himark_ahp_ext_types::Location {
    himark_ahp_ext_types::Location {
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
    app.register_handler::<himark::higent::SubscribeLocationsEffect>(SeatSubscribeLocations);
    app.register_handler::<himark::higent::PollLocationsEffect>(SeatPollLocations);
    app.register_handler::<himark::higent::UnsubscribeLocationsEffect>(SeatUnsubscribeLocations);
    app.register_handler::<himark::LspLocationsEffect>(StubLspLocations {
        snapshot: himark_ahp_ext_types::LocationList {
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
        let _ = himark::Window::draw(window, &mut app, surface.canvas());
    }

    let entity = himark::Windows::window_ref(app.store(), window).expect("the window");
    assert_eq!(
        entity.dock_owner(),
        Some(himark::hisearch::OWNER),
        "the Search tab activated"
    );
    let session = entity.current_session();
    let feed = himark::locations::SessionSearchFeeds::feed(app.store(), &session)
        .expect("the session fronts the feed");
    let row = himark::locations::LocationsFeeds::row(app.store(), feed).expect("the feed row");
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
