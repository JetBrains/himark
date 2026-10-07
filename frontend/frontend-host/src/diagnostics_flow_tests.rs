// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Diagnostics e2e over a SCRIPTED host through the REAL session-open
//! road: `open_session` → subscribe → `EnterSessionWork` dials the
//! diagnostics channel; a document opened under the session's folder
//! gets dressed with the snapshot's squiggles at the batch tail.

use std::sync::Arc;

use ahp_types::state::{SessionLifecycle, SessionState};
use ahp_wire::client::{ChannelUri, ClientFuture, LspClient, SessionClient, SessionUri};
use editor::location::{Authority, ResourceLocation, ResourceType};
use editor::theme::StyleId;
use himark_ahp_ext_types::lsp::{DiagnosticsPublished, DiagnosticsState, PublishedDiagnostics};

use crate::{AppFonts, HimarkEngine};

const SESSION: &str = "ahp-session:/diagnosed";
const CHANNEL: &str = "ahp-lsp-diagnostics:/one";
const FOLDER: &str = "file:///tmp/diagproj";
const FILE: &str = "file:///tmp/diagproj/main.rs";

macro_rules! unreached {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $out:ty;)*) => {
        $(fn $name(&self, $($arg: $ty),*) -> $out {
            $(let _ = $arg;)*
            unreachable!("the diagnostics flow never reaches this seat road")
        })*
    };
}

struct Seat;

impl SessionClient for Seat {
    fn subscribe_session(&self, session: SessionUri) -> ClientFuture<Result<SessionState, String>> {
        assert_eq!(session.as_str(), SESSION);
        Box::pin(std::future::ready(Ok(SessionState {
            provider: "scripted".to_owned(),
            title: "diagnosed session".to_owned(),
            status: 0,
            activity: None,
            project: None,
            origin: None,
            working_directories: Some(vec![FOLDER.to_owned()]),
            annotations: None,
            lifecycle: SessionLifecycle::Ready,
            creation_error: None,
            server_tools: None,
            active_clients: Vec::new(),
            changesets: None,
            config: None,
            customizations: None,
            input_needed: None,
            chats: Vec::new(),
            default_chat: None,
            meta: None,
        })))
    }

    fn poll_session(
        &self,
        _session: SessionUri,
    ) -> ClientFuture<Vec<ahp_types::actions::StateAction>> {
        Box::pin(std::future::pending())
    }

    fn connect(&self) -> ClientFuture<Result<ahp_wire::client::RootInfo, String>> {
        Box::pin(std::future::ready(Ok(ahp_wire::client::RootInfo {
            agents: Vec::new(),
        })))
    }

    fn list_sessions(
        &self,
        _cursor: Option<String>,
    ) -> ClientFuture<Result<ahp_wire::client::SessionsPage, String>> {
        Box::pin(std::future::ready(Ok(ahp_wire::client::SessionsPage {
            sessions: Vec::new(),
            next_cursor: None,
        })))
    }

    fn poll_root(&self) -> ClientFuture<Vec<ahp_wire::client::ServerEvent>> {
        Box::pin(std::future::pending())
    }

    unreached! {
        create_session(dirs: Vec<String>, options: ahp_wire::client::SessionOptions) -> ClientFuture<Result<SessionUri, String>>;
        resolve_session_config(working_directory: Option<String>, config: Option<serde_json::Map<String, serde_json::Value>>) -> ClientFuture<Result<ahp_types::commands::ResolveSessionConfigResult, String>>;
        dispose_session(session: SessionUri) -> ClientFuture<Result<(), String>>;
    }

    fn dispatch_action(
        &self,
        _channel: ChannelUri,
        _action: ahp_types::actions::StateAction,
    ) -> ClientFuture<Result<(), String>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

/// The sibling wires the session-open road arms — comments and
/// changes — park: their stories are not this test's.
struct Parked;

impl ahp_wire::client::AnnotationsClient for Parked {
    fn subscribe_annotations(
        &self,
        _session: SessionUri,
    ) -> ClientFuture<Result<ahp_types::state::AnnotationsState, String>> {
        Box::pin(std::future::pending())
    }
    fn poll_annotations(
        &self,
        _session: SessionUri,
    ) -> ClientFuture<Vec<ahp_types::actions::StateAction>> {
        Box::pin(std::future::pending())
    }
    fn dispatch_annotations(
        &self,
        _session: &SessionUri,
        _action: ahp_types::actions::StateAction,
    ) {
    }
    fn unsubscribe_annotations(&self, _session: &SessionUri) {}
}

impl ahp_wire::client::ChangesClient for Parked {
    fn subscribe_changeset(
        &self,
        _channel: ChannelUri,
    ) -> ClientFuture<Result<ahp_types::state::ChangesetState, String>> {
        Box::pin(std::future::pending())
    }
    fn poll_changeset(
        &self,
        _channel: ChannelUri,
    ) -> ClientFuture<Vec<ahp_types::actions::StateAction>> {
        Box::pin(std::future::pending())
    }
    fn unsubscribe_changeset(&self, _channel: &ChannelUri) {}
}

impl ahp_wire::client::ResourceClient for Parked {
    fn resource_read(
        &self,
        _session: SessionUri,
        _uri: ahp_wire::client::ResourceUri,
    ) -> ClientFuture<Option<String>> {
        Box::pin(std::future::ready(None))
    }
    fn resource_write(
        &self,
        _session: SessionUri,
        _uri: ahp_wire::client::ResourceUri,
        _text: String,
    ) -> ClientFuture<bool> {
        Box::pin(std::future::ready(false))
    }
    fn resource_list(
        &self,
        _session: SessionUri,
        _uri: ahp_wire::client::ResourceUri,
    ) -> ClientFuture<Option<Vec<(String, bool)>>> {
        Box::pin(std::future::ready(None))
    }
    fn resource_watch(
        &self,
        _session: SessionUri,
        _uri: ahp_wire::client::ResourceUri,
        _events: Arc<dyn Fn() + Send + Sync>,
    ) -> ClientFuture<Option<ahp_wire::client::WatchHandle>> {
        Box::pin(std::future::ready(None))
    }
    fn resource_unwatch(&self, _handle: ahp_wire::client::WatchHandle) -> ClientFuture<()> {
        Box::pin(std::future::ready(()))
    }
    fn search(
        &self,
        _session: SessionUri,
        _ask: ahp_wire::client::SearchAsk,
    ) -> ClientFuture<Option<himark_ahp_ext_types::search::SearchResult>> {
        Box::pin(std::future::ready(None))
    }
}

impl ahp_wire::client::DocumentsClient for Parked {
    fn open_document(
        &self,
        _session: SessionUri,
        _uri: Option<ahp_wire::client::ResourceUri>,
        _text: Option<String>,
    ) -> ClientFuture<Result<himark_ahp_ext_types::documents::OpenDocumentResult, String>> {
        Box::pin(std::future::pending())
    }
    fn subscribe_document(
        &self,
        _channel: ChannelUri,
    ) -> ClientFuture<Result<himark_ahp_ext_types::documents::DocumentState, String>> {
        Box::pin(std::future::pending())
    }
    fn poll_document(
        &self,
        _channel: ChannelUri,
    ) -> ClientFuture<Vec<himark_ahp_ext_types::documents::DocumentApplied>> {
        Box::pin(std::future::pending())
    }
    fn dispatch_document(
        &self,
        _channel: &ChannelUri,
        _action: himark_ahp_ext_types::documents::DocumentApplied,
    ) {
    }
    fn unsubscribe_document(&self, _channel: &ChannelUri) -> ClientFuture<()> {
        Box::pin(std::future::ready(()))
    }
}

struct Lsp {
    snapshot: DiagnosticsState,
    /// The pass-through answers by method; an unscripted method is
    /// refused, the way a host without that language feature would.
    answers: std::collections::HashMap<&'static str, serde_json::Value>,
    legend: Vec<&'static str>,
}

impl Lsp {
    fn quiet(snapshot: DiagnosticsState) -> Self {
        Self {
            snapshot,
            answers: std::collections::HashMap::new(),
            legend: Vec::new(),
        }
    }
}

impl LspClient for Lsp {
    fn lsp(
        &self,
        _session: SessionUri,
        method: String,
        _params: serde_json::Value,
    ) -> ClientFuture<Result<serde_json::Value, String>> {
        let answer = self
            .answers
            .get(method.as_str())
            .cloned()
            .ok_or_else(|| format!("{method} not scripted"));
        Box::pin(std::future::ready(answer))
    }

    fn lsp_capabilities(
        &self,
        _session: SessionUri,
        _uri: ahp_wire::client::ResourceUri,
    ) -> ClientFuture<Result<Option<serde_json::Value>, String>> {
        let capabilities = (!self.legend.is_empty()).then(|| {
            serde_json::json!({ "semanticTokensProvider": { "legend": {
                "tokenTypes": self.legend, "tokenModifiers": []
            }}})
        });
        Box::pin(std::future::ready(Ok(capabilities)))
    }

    fn lsp_diagnostics(&self, session: SessionUri) -> ClientFuture<Result<ChannelUri, String>> {
        assert_eq!(session.as_str(), SESSION);
        Box::pin(std::future::ready(Ok(ChannelUri::new(CHANNEL))))
    }

    fn subscribe_lsp_diagnostics(
        &self,
        channel: ChannelUri,
    ) -> ClientFuture<Result<DiagnosticsState, String>> {
        assert_eq!(channel.as_str(), CHANNEL);
        Box::pin(std::future::ready(Ok(self.snapshot.clone())))
    }

    fn poll_lsp_diagnostics(
        &self,
        _channel: ChannelUri,
    ) -> ClientFuture<Vec<DiagnosticsPublished>> {
        Box::pin(std::future::pending())
    }
}

struct OpenScripted {
    host: ahp_wire::client::HostId,
}

impl himark::commands::WindowedCommand for OpenScripted {
    fn id(&self) -> &'static str {
        "test.open-diagnosed-session"
    }
    fn name(&self) -> String {
        "Open Diagnosed".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        himark::higent::open_session::open_session(
            store,
            window,
            self.host,
            SessionUri::new(SESSION),
            false,
            fx,
        );
    }
}

/// Open `main.rs` under the session's folder the way the file tree
/// does: a document location spelled with the session's authority.
struct OpenMain;

impl himark::commands::WindowedCommand for OpenMain {
    fn id(&self) -> &'static str {
        "test.open-main"
    }
    fn name(&self) -> String {
        "Open Main".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let key = himark::workspace::window_session(store, window).expect("a session");
        let state = himark::workspace::session_state(store, window).expect("a state");
        let location = ResourceLocation::new(
            ResourceType::document(),
            Authority::new(ahp_wire::client::authority(key.host, &key.session)),
            vec![
                "tmp".to_owned(),
                "diagproj".to_owned(),
                "main.rs".to_owned(),
            ],
        );
        fx.follow_up(himark::app::AppCommand::Opened(
            window,
            himark::app::OpenedDocument {
                documents: state.documents(),
                name: "main.rs".to_owned(),
                document: editor::test_document::plain_document("fn main() {}\n"),
                location: Some(location),
                primary: true,
                target: None,
                focus: false,
            },
        ));
    }
}

fn settle(engine: &mut HimarkEngine) {
    for _ in 0..4 {
        engine.worker().run_pending();
        engine.drain();
    }
}

fn settle_until(
    engine: &mut HimarkEngine,
    window: u64,
    what: &str,
    mut done: impl FnMut(&HimarkEngine) -> bool,
) {
    let started = std::time::Instant::now();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    while !done(engine) {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "never settled: {what}"
        );
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        settle(engine);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn squiggles(engine: &HimarkEngine, window: u64) -> Option<Vec<(std::ops::Range<u32>, StyleId)>> {
    let store = engine.app.store();
    let state = himark::workspace::session_state(store, crate::wid(window))?;
    let location = {
        let key = himark::workspace::window_session(store, crate::wid(window))?;
        ResourceLocation::new(
            ResourceType::document(),
            Authority::new(ahp_wire::client::authority(key.host, &key.session)),
            vec![
                "tmp".to_owned(),
                "diagproj".to_owned(),
                "main.rs".to_owned(),
            ],
        )
    };
    let id = documents::OpenDocuments::by_location(store, state.documents(), &location)?;
    let document = documents::OpenDocuments::document_ref(store, state.documents(), id)?;
    let markup = document.feature_markup(ahp_lsp::diagnostics::diagnostics_markup())?;
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    markup.marks_inline_hidden_in(0..u32::MAX, &mut inline, &mut hidden);
    let mut found: Vec<_> = inline
        .into_iter()
        .map(|interval| (interval.range, interval.id))
        .collect();
    found.sort_by_key(|(range, _)| range.start);
    Some(found)
}

fn error_at(line: u64, from: u64, to: u64) -> serde_json::Value {
    serde_json::json!({
        "range": {
            "start": { "line": line, "character": from },
            "end": { "line": line, "character": to },
        },
        "severity": 1,
        "message": "boom",
    })
}

#[test]
fn the_session_open_road_dials_diagnostics_and_squiggles_the_document() {
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let mut items = std::collections::HashMap::new();
    items.insert(
        FILE.to_owned(),
        PublishedDiagnostics {
            version: None,
            diagnostics: vec![error_at(0, 3, 7)],
        },
    );
    let host = engine.register_agent_server(
        "diagnosed",
        ahp_wire::client::Client {
            session: Arc::new(Seat),
            annotations: Arc::new(Parked),
            changes: Arc::new(Parked),
            resources: Arc::new(Parked),
            documents: Arc::new(Parked),
            lsp: Arc::new(Lsp::quiet(DiagnosticsState { items })),
            ..ahp_wire::client::inert()
        },
    );
    engine.set_local_backend(host);

    assert!(engine
        .app
        .perform_batch(vec![himark::app::AppCommand::Windowed(
            crate::wid(window),
            Arc::new(OpenScripted { host }),
        )]));
    settle_until(&mut engine, window, "the session came up", |engine| {
        himark::workspace::window_session(engine.app.store(), crate::wid(window))
            .is_some_and(|key| key.session.as_str() == SESSION)
    });

    assert!(engine
        .app
        .perform_batch(vec![himark::app::AppCommand::Windowed(
            crate::wid(window),
            Arc::new(OpenMain),
        )]));
    settle_until(&mut engine, window, "the document opened", |engine| {
        squiggles(engine, window).is_some()
    });
    settle_until(&mut engine, window, "the squiggle landed", |engine| {
        squiggles(engine, window).is_some_and(|found| !found.is_empty())
    });
    assert_eq!(
        squiggles(&engine, window).unwrap(),
        vec![(3..7, StyleId::DiagnosticError)]
    );

    // And the WINDOW paints it: the pane's bounded editor repairs its
    // damaged line and the wavy underline reaches the pixels.
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let started = std::time::Instant::now();
    let mut painted = 0;
    while started.elapsed() < std::time::Duration::from_secs(10) {
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        settle(&mut engine);
        painted = squiggle_pixels(&mut surface);
        if painted > 10 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        painted > 10,
        "the window painted the squiggle: {painted} pixels"
    );
}

/// Pixels in the embedded theme's diagnostic_error squiggle color
/// (#ff5370), tolerant to channel order.
fn squiggle_pixels(surface: &mut skia_safe::Surface) -> usize {
    let image = surface.image_snapshot();
    let pixmap = image.peek_pixels().expect("raster pixels");
    let bytes = pixmap.bytes().expect("pixel bytes");
    let row_bytes = image.width() as usize * 4;
    (0..image.height() as usize)
        .flat_map(|y| (0..image.width() as usize).map(move |x| (x, y)))
        .filter(|(x, y)| {
            let px = &bytes[y * row_bytes + x * 4..y * row_bytes + x * 4 + 4];
            let (a, b, c) = (px[0], px[1], px[2]);
            let red = a.max(c);
            let blue = a.min(c);
            red > 200 && b < 140 && b > 40 && blue < 160
        })
        .count()
}

/// Opens `main.rs`-style paths the way the file tree does — a `local`
/// spelling that `open_locations` re-authors under the session folder.
struct OpenLivePath {
    path: Vec<String>,
}

impl himark::commands::WindowedCommand for OpenLivePath {
    fn id(&self) -> &'static str {
        "test.open-live-path"
    }
    fn name(&self) -> String {
        "Open Live Path".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let location = ResourceLocation::new(
            ResourceType::document(),
            Authority::new("local"),
            self.path.clone(),
        );
        himark::workspace::open_locations(store, ui, window, &[location], fx);
    }
}

struct OpenLiveSession {
    host: ahp_wire::client::HostId,
    session: String,
}

impl himark::commands::WindowedCommand for OpenLiveSession {
    fn id(&self) -> &'static str {
        "test.open-live-session"
    }
    fn name(&self) -> String {
        "Open Live Session".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        himark::higent::open_session::open_session(
            store,
            window,
            self.host,
            SessionUri::new(self.session.clone()),
            false,
            fx,
        );
    }
}

fn live_squiggles(
    engine: &HimarkEngine,
    window: u64,
    path: &[String],
) -> Option<Vec<(std::ops::Range<u32>, StyleId)>> {
    let store = engine.app.store();
    let state = himark::workspace::session_state(store, crate::wid(window))?;
    let key = himark::workspace::window_session(store, crate::wid(window))?;
    let location = ResourceLocation::new(
        ResourceType::document(),
        Authority::new(ahp_wire::client::authority(key.host, &key.session)),
        path.to_vec(),
    );
    let id = documents::OpenDocuments::by_location(store, state.documents(), &location)?;
    let document = documents::OpenDocuments::document_ref(store, state.documents(), id)?;
    let markup = document.feature_markup(ahp_lsp::diagnostics::diagnostics_markup())?;
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    markup.marks_inline_hidden_in(0..u32::MAX, &mut inline, &mut hidden);
    Some(
        inline
            .into_iter()
            .map(|interval| (interval.range, interval.id))
            .collect(),
    )
}

/// The PRODUCTION engine against the LIVE agent host at `~/.himark`:
/// `HIMARK_LIVE_SESSION` names the session, `HIMARK_LIVE_FILE` an
/// absolute path under one of its folders.
#[test]
#[ignore = "talks to the live agent host; set HIMARK_LIVE_SESSION and HIMARK_LIVE_FILE"]
fn live_host_squiggles_the_opened_file() {
    let session = std::env::var("HIMARK_LIVE_SESSION").expect("HIMARK_LIVE_SESSION");
    let file = std::env::var("HIMARK_LIVE_FILE").expect("HIMARK_LIVE_FILE");
    let path: Vec<String> = file
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();

    let mut engine = HimarkEngine::new();
    let window = engine.add_window();
    let host = ahp_wire::SessionId::local_default(engine.app.store()).host;
    eprintln!("[live] local host {host:?}");

    assert!(engine
        .app
        .perform_batch(vec![himark::app::AppCommand::Windowed(
            crate::wid(window),
            Arc::new(OpenLiveSession {
                host,
                session: session.clone(),
            }),
        )]));
    let started = std::time::Instant::now();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    while !himark::workspace::window_session(engine.app.store(), crate::wid(window))
        .is_some_and(|key| key.session.as_str() == session)
    {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "session never came up"
        );
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        settle(&mut engine);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    eprintln!("[live] session entered after {:?}", started.elapsed());

    assert!(engine
        .app
        .perform_batch(vec![himark::app::AppCommand::Windowed(
            crate::wid(window),
            Arc::new(OpenLivePath { path: path.clone() }),
        )]));
    let started = std::time::Instant::now();
    let mut last = None;
    while started.elapsed() < std::time::Duration::from_secs(20) {
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        settle(&mut engine);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let now = live_squiggles(&engine, window, &path);
        if now != last {
            eprintln!("[live] +{:?} squiggles: {now:?}", started.elapsed());
            last = now;
        }
        if last.as_ref().is_some_and(|found| !found.is_empty()) {
            break;
        }
    }
    assert!(
        last.as_ref().is_some_and(|found| !found.is_empty()),
        "the live host's diagnostics squiggled the file: {last:?}"
    );
    // Let the pull layers land too, then time a theme switch over
    // the dressed document.
    for _ in 0..40 {
        settle(&mut engine);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for round in 0..3 {
        let started = std::time::Instant::now();
        assert!(engine.perform_command(window, "theme.toggle"));
        let performed = started.elapsed();
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        let drawn = started.elapsed();
        // The repairs the toggle owes: drain the worker until quiet.
        let mut rounds = 0;
        loop {
            engine.worker().run_pending();
            let landed = engine.drain();
            let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
            rounds += 1;
            if !landed && engine.queued_landings() == 0 && rounds > 3 {
                break;
            }
            if rounds > 400 {
                break;
            }
        }
        eprintln!(
            "[live] theme toggle {round}: perform {performed:?}, first draw {drawn:?}, settled {:?} after {rounds} rounds",
            started.elapsed()
        );
    }
}

/// The pull layers through the real session-open road: a document
/// opened under the session asks its semantic tokens and inlay hints,
/// and both land as document-scoped markups that reach the pixels.
#[test]
fn semantic_tokens_and_inlay_hints_dress_the_open_document() {
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let mut answers = std::collections::HashMap::new();
    // "main" at 0:3, four long, token type 0 = function.
    answers.insert(
        "textDocument/semanticTokens/full",
        serde_json::json!({ "data": [0, 3, 4, 0, 0] }),
    );
    answers.insert(
        "textDocument/inlayHint",
        serde_json::json!([{
            "position": { "line": 0, "character": 9 },
            "label": "-> ()",
            "paddingLeft": true
        }]),
    );
    let host = engine.register_agent_server(
        "enriched",
        ahp_wire::client::Client {
            session: Arc::new(Seat),
            annotations: Arc::new(Parked),
            changes: Arc::new(Parked),
            resources: Arc::new(Parked),
            documents: Arc::new(Parked),
            lsp: Arc::new(Lsp {
                snapshot: DiagnosticsState::default(),
                answers,
                legend: vec!["function"],
            }),
            ..ahp_wire::client::inert()
        },
    );
    engine.set_local_backend(host);

    assert!(engine
        .app
        .perform_batch(vec![himark::app::AppCommand::Windowed(
            crate::wid(window),
            Arc::new(OpenScripted { host }),
        )]));
    settle_until(&mut engine, window, "the session came up", |engine| {
        himark::workspace::window_session(engine.app.store(), crate::wid(window))
            .is_some_and(|key| key.session.as_str() == SESSION)
    });
    assert!(engine
        .app
        .perform_batch(vec![himark::app::AppCommand::Windowed(
            crate::wid(window),
            Arc::new(OpenMain),
        )]));

    let layer = |engine: &HimarkEngine,
                 markup: editor::markup::MarkupId|
     -> Option<editor::markup::Markup> {
        let store = engine.app.store();
        let state = himark::workspace::session_state(store, crate::wid(window))?;
        let key = himark::workspace::window_session(store, crate::wid(window))?;
        let location = ResourceLocation::new(
            ResourceType::document(),
            Authority::new(ahp_wire::client::authority(key.host, &key.session)),
            vec![
                "tmp".to_owned(),
                "diagproj".to_owned(),
                "main.rs".to_owned(),
            ],
        );
        let id = documents::OpenDocuments::by_location(store, state.documents(), &location)?;
        let document = documents::OpenDocuments::document_ref(store, state.documents(), id)?;
        document.feature_markup(markup).cloned()
    };
    settle_until(&mut engine, window, "both layers landed", |engine| {
        layer(engine, ahp_lsp::enrich::semantic_tokens_markup()).is_some_and(|m| !m.is_empty())
            && layer(engine, ahp_lsp::enrich::inlay_hints_markup()).is_some_and(|m| !m.is_empty())
    });

    let tokens = layer(&engine, ahp_lsp::enrich::semantic_tokens_markup()).expect("tokens");
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    tokens.marks_inline_hidden_in(0..u32::MAX, &mut inline, &mut hidden);
    let spans: Vec<_> = inline.iter().map(|i| (i.range.clone(), i.id)).collect();
    assert_eq!(
        spans,
        vec![(3..7, StyleId::Function)],
        "`main` restyled as a function"
    );

    // The window paints with the hint chip in place: a frame with the
    // layer and a frame after clearing it differ.
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let frame = |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface| -> Vec<u8> {
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        settle(engine);
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        surface
            .image_snapshot()
            .encode(None, skia_safe::EncodedImageFormat::PNG, None)
            .expect("png")
            .as_bytes()
            .to_vec()
    };
    let with_hint = frame(&mut engine, &mut surface);
    if let Some(dir) = std::env::var_os("HIMARK_SHOT") {
        std::fs::write(
            std::path::PathBuf::from(dir).join("inlay-hints.png"),
            &with_hint,
        )
        .expect("the shot");
    }

    struct ClearHints;
    impl himark::commands::WindowedCommand for ClearHints {
        fn id(&self) -> &'static str {
            "test.clear-hints"
        }
        fn name(&self) -> String {
            "Clear Hints".to_owned()
        }
        fn perform(
            &self,
            store: &mut imba::store::Store,
            ui: &imba::ui::UiCtx,
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let state = himark::workspace::session_state(store, window).expect("a state");
            let key = himark::workspace::window_session(store, window).expect("a session");
            let location = ResourceLocation::new(
                ResourceType::document(),
                Authority::new(ahp_wire::client::authority(key.host, &key.session)),
                vec![
                    "tmp".to_owned(),
                    "diagproj".to_owned(),
                    "main.rs".to_owned(),
                ],
            );
            let documents = state.documents();
            let id =
                documents::OpenDocuments::by_location(store, documents, &location).expect("open");
            let mut document =
                documents::OpenDocuments::document(store, documents, id).expect("doc");
            let markup = ahp_lsp::enrich::inlay_hints_markup();
            let replacement = editor::markup::Markup::new();
            let changed = editor::markup::set_diff(document.feature_markup(markup), &replacement);
            let fonts = editor::env::ui_collection(store, ui);
            let theme = editor::env::Themes::of(store);
            fx.scope(
                move |command| {
                    himark::app::AppCommand::Verb(imba::command::Verb::at(
                        documents,
                        documents::DocumentsCommand::Editor(id, command),
                    ))
                },
                |fx| {
                    document.replace_markup(
                        markup,
                        replacement,
                        &changed,
                        store,
                        ui,
                        &fonts,
                        &theme,
                        fx,
                    )
                },
            );
            documents::OpenDocuments::put_document(store, documents, id, document);
        }
    }
    assert!(engine
        .app
        .perform_batch(vec![himark::app::AppCommand::Windowed(
            crate::wid(window),
            Arc::new(ClearHints),
        )]));
    let without_hint = frame(&mut engine, &mut surface);
    assert_ne!(with_hint, without_hint, "the hint chip reaches the pixels");
}
