// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The diagnostics lane end to end (docs/ahp/ahp-lsp.md §6): a
//! session dials its diagnostics channel, the snapshot lands, and the
//! batch-tail lane dresses the open document with severity squiggles.

#![allow(unused_imports)]
use super::*;
use ahp_wire::client::{ChannelUri, Client, ClientFuture, LspClient, SessionClient, SessionUri};
use editor::location::{Authority, ResourceLocation, ResourceType};
use editor::markup::Markup;
use editor::theme::StyleId;
use himark::app::{AppCommand, AppFonts, Application};
use himark_ahp_ext_types::lsp::{DiagnosticsPublished, DiagnosticsState, PublishedDiagnostics};
use imba::command::{DynamicCommand, Fx, Verb};
use imba::ui::UiCtx;
use std::sync::Arc;

const SESSION: &str = "test-session:/diagnostics";
const CHANNEL: &str = "ahp-lsp-diagnostics:/one";

macro_rules! off_road {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $out:ty;)*) => {
        $(fn $name(&self, $($arg: $ty),*) -> $out {
            $(let _ = $arg;)*
            unreachable!("the diagnostics lane never takes this road")
        })*
    };
}

struct Seat;

impl SessionClient for Seat {
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
            sessions: vec![ahp_types::state::SessionSummary {
                origin: None,
                provider: "test".to_owned(),
                title: "squiggled".to_owned(),
                status: 0,
                activity: None,
                project: None,
                working_directories: Some(vec!["file:///project".to_owned()]),
                annotations: None,
                resource: SESSION.to_owned(),
                created_at: String::new(),
                modified_at: String::new(),
                changes: None,
                meta: None,
            }],
            next_cursor: None,
        })))
    }

    fn poll_root(&self) -> ClientFuture<Vec<ahp_wire::client::ServerEvent>> {
        Box::pin(std::future::pending())
    }

    off_road! {
        create_session(dirs: Vec<String>, options: ahp_wire::client::SessionOptions) -> ClientFuture<Result<SessionUri, String>>;
        resolve_session_config(working_directory: Option<String>, config: Option<serde_json::Map<String, serde_json::Value>>) -> ClientFuture<Result<ahp_types::commands::ResolveSessionConfigResult, String>>;
        dispose_session(session: SessionUri) -> ClientFuture<Result<(), String>>;
        subscribe_session(session: SessionUri) -> ClientFuture<Result<ahp_types::state::SessionState, String>>;
        poll_session(session: SessionUri) -> ClientFuture<Vec<ahp_types::actions::StateAction>>;
        dispatch_action(channel: ChannelUri, action: ahp_types::actions::StateAction) -> ClientFuture<Result<(), String>>;
    }
}

/// The scripted language-services facet: one channel, one snapshot
/// naming one resource, a poll that parks.
struct Lsp {
    snapshot: DiagnosticsState,
}

impl LspClient for Lsp {
    fn lsp(
        &self,
        _session: SessionUri,
        method: String,
        _params: serde_json::Value,
    ) -> ClientFuture<Result<serde_json::Value, String>> {
        // The pull layers ask on open; this host serves none of them.
        Box::pin(std::future::ready(Err(format!("{method} not served"))))
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

struct Dial {
    wire: imba::store::Id<ahp_lsp::diagnostics::DiagnosticsWire>,
    folder: ResourceLocation,
}

impl DynamicCommand for Dial {
    fn id(&self) -> &'static str {
        "test.diagnostics.dial"
    }
    fn name(&self) -> String {
        "Dial Diagnostics".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &UiCtx, fx: &mut Fx<'_>) {
        ahp_lsp::diagnostics::ensure(store, self.wire, &self.folder, fx);
    }
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

fn squiggles(document: &Document) -> Vec<(std::ops::Range<u32>, StyleId)> {
    let Some(markup) = document.feature_markup(ahp_lsp::diagnostics::diagnostics_markup()) else {
        return Vec::new();
    };
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    markup.marks_inline_hidden_in(0..u32::MAX, &mut inline, &mut hidden);
    let mut found: Vec<_> = inline
        .into_iter()
        .map(|interval| (interval.range, interval.id))
        .collect();
    found.sort_by_key(|(range, _)| range.start);
    found
}

#[test]
fn the_snapshot_squiggles_the_open_document() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    himark::hiahp::register_all(&mut app);

    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );

    let mut items = std::collections::HashMap::new();
    items.insert(
        "file:///project/main.rs".to_owned(),
        PublishedDiagnostics {
            version: None,
            diagnostics: vec![error_at(0, 3, 7)],
        },
    );
    let host = app.register_client(Client {
        session: Arc::new(Seat),
        lsp: Arc::new(Lsp {
            snapshot: DiagnosticsState { items },
        }),
        ..ahp_wire::client::inert()
    });
    ahp_session::session::agents::Agents::seed(&mut app.store_mut(), host, "Diagnostics Host");
    ahp_session::session::state::Hosts::install_uris(
        &mut app.store_mut(),
        host,
        Arc::new(ahp_wire::uris::FileUris),
    );
    let key = ahp_wire::SessionId {
        host,
        session: SessionUri::new(SESSION),
    };
    let state = ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &key);

    let authority = Authority::new(ahp_wire::client::authority(host, &key.session));
    let folder = ResourceLocation::new(
        ResourceType::directory(),
        authority.clone(),
        vec!["project".to_owned()],
    );
    let file = folder.child(ResourceType::document(), "main.rs");
    let document = Document::new(
        text::text::Text::from_string_exact("fn main() {}\n"),
        Markup::new(),
    );
    let document_id = documents::OpenDocuments::register(
        &mut app.store_mut(),
        state.documents(),
        document,
        Some(file.clone()),
        "main.rs".to_owned(),
        0,
    );

    assert!(
        app.perform_batch(vec![AppCommand::Verb(Verb::Dynamic(Arc::new(Dial {
            wire: state.diagnostics_wire(),
            folder,
        })))])
    );
    for _ in 0..20 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }

    let document =
        documents::OpenDocuments::document_ref(app.store(), state.documents(), document_id)
            .expect("the document stays open");
    assert_eq!(
        squiggles(document),
        vec![(3..7, StyleId::DiagnosticError)],
        "the snapshot's error squiggles `main`"
    );
}
