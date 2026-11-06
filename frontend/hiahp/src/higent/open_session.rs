// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use himark::commands::HimarkCommand;
use ahp_wire::client::ChatUri;
use ahp_wire::client::HostId;
use ahp_wire::client::SessionUri;
use ahp_wire::effects::SubscribeSessionEffect;
use ahp_wire::SessionId;
use imba::effect::AnyEffect;
use imba::store::Store;

/// ⌘N: a NEW SESSION is a regular chat. The session is created
/// immediately — inheriting the current session's host, folders and
/// agent config when one is showing — and the landing fronts the
/// ordinary chat panel on it; the first message is a normal send.
/// Model, effort and permission mode live in the session toolbar,
/// folders in the add-folder road; `Hosts::sweep_empty` reaps a
/// session that never gets a word. (This replaced the composer
/// form — new_session.rs, 2.4k lines — 2026-10-10.)
pub struct CreateHeadless {
    pub host: Option<HostId>,
}

impl imba::command::DynamicOnceCommand for CreateHeadless {
    fn perform(
        self: Box<Self>,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) {
        let Some(host) = self.host.or_else(|| {
            store
                .get::<ahp_wire::client::LocalHost>()
                .and_then(|local| local.0)
        }) else {
            eprintln!("[new-session] no host to create on");
            return;
        };
        let Some(client) = ahp_wire::client::Servers::client(store, host) else {
            eprintln!("[new-session] unregistered host {host:?}");
            return;
        };
        fx.push(
            AnyEffect::new(ahp_wire::effects::CreateSessionEffect {
                client: client.session.clone(),
                working_directories: Vec::new(),
                options: ahp_wire::client::SessionOptions {
                    provider: None,
                    config: None,
                    model: None,
                },
            })
            .map(move |result| match result {
                Ok(session) => imba::command::Verb::once(EnterHeadless {
                    server: host,
                    session,
                }),
                Err(error) => imba::command::Verb::once(HeadlessFailed { error }),
            }),
        );
    }
}

/// The HEADLESS session entry — subscribe, adopt, ensure the
/// wires, relaunch the poll. No window switches, no chat opens:
/// the shell mounts the session's views itself.
pub struct EnterHeadless {
    pub server: HostId,
    pub session: SessionUri,
}

impl imba::command::DynamicOnceCommand for EnterHeadless {
    fn perform(
        self: Box<Self>,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) {
        let Some(client) = ahp_wire::client::Servers::client(store, self.server) else {
            eprintln!(
                "[higent] enter-session: unregistered server {:?}",
                self.server
            );
            return;
        };
        let server = self.server;
        let session = self.session.clone();
        fx.push(
            AnyEffect::new(SubscribeSessionEffect {
                client: client.session.clone(),
                session: self.session,
            })
            .map(move |result| {
                imba::command::Verb::once(EnteredHeadless {
                    server,
                    session: session.clone(),
                    result,
                })
            }),
        );
    }
}

struct EnteredHeadless {
    server: HostId,
    session: SessionUri,
    result: Result<ahp_types::state::SessionState, String>,
}

impl imba::command::DynamicOnceCommand for EnteredHeadless {
    fn perform(
        self: Box<Self>,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) {
        let state = match &self.result {
            Ok(state) => state,
            Err(error) => {
                eprintln!("[higent] subscribe session failed: {error}");
                return;
            }
        };
        let key = SessionId {
            host: self.server,
            session: self.session.clone(),
        };
        ahp_session::session::channel::adopt_subscribed(store, &key, state);
        let row = ahp_session::session::state::Hosts::ensure_state(store, &key);
        let folders = ahp_session::session::folders::session_folders(store, &key);
        ahp_changes::changes::ensure(store, row.changes_wire(), folders.clone(), fx);
        for folder in folders {
            ahp_comments::ensure(store, row.comments_wire(), &folder, fx);
            ahp_lsp::diagnostics::ensure(store, row.diagnostics_wire(), &folder, fx);
        }
        ahp_session::session::driver::relaunch_session_poll(store, key, fx);
    }
}

struct HeadlessFailed {
    error: String,
}

impl imba::command::DynamicOnceCommand for HeadlessFailed {
    fn perform(
        self: Box<Self>,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _fx: &mut imba::command::Fx<'_>,
    ) {
        eprintln!("[higent] createSession failed: {}", self.error);
    }
}

pub struct ShowSession {
    pub origin: himark::bridge::ViewBridgeId,
    pub server: ahp_wire::client::HostId,
    pub session: ahp_wire::client::SessionUri,
}

impl imba::command::DynamicOnceCommand for ShowSession {
    fn perform(
        self: Box<Self>,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) {
        let key = ahp_wire::SessionId {
            host: self.server,
            session: self.session.clone(),
        };
        Box::new(EnterHeadless {
            server: self.server,
            session: self.session,
        })
        .perform(store, ui, fx);
        himark::shell::navigate_origin(store, ui, self.origin, crate::places::ChatPlace(key), fx);
    }
}

