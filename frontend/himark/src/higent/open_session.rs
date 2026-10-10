// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::app::AppCommand;
use crate::commands::WindowedCommand;
use ::workbench::window::Windows;
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
pub struct OpenNewSession {
    pub host: Option<HostId>,
}

impl WindowedCommand for OpenNewSession {
    fn id(&self) -> &'static str {
        "session.new"
    }

    fn name(&self) -> String {
        "New Session".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let seed = Windows::window_ref(store, window)
            .map(crate::workspace::entity_session)
            .filter(|session| session.names_session())
            .filter(|session| self.host.is_none_or(|host| host == session.host));
        let Some(host) = self
            .host
            .or(seed.as_ref().map(|session| session.host))
            .or_else(|| {
                store
                    .get::<ahp_wire::client::LocalHost>()
                    .and_then(|local| local.0)
            })
        else {
            eprintln!("[new-session] no host to create on");
            return;
        };
        let Some(client) = ahp_wire::client::Servers::client(store, host) else {
            eprintln!("[new-session] unregistered host {host:?}");
            return;
        };

        let mut working_directories = Vec::new();
        let mut options = ahp_wire::client::SessionOptions {
            provider: None,
            config: None,
            model: None,
        };
        if let Some(session) = &seed {
            if let Some(uris) = ahp_session::session::state::Hosts::uris(store, session.host) {
                for folder in ahp_session::session::folders::session_folders(store, session) {
                    working_directories.push(uris.uri_of(&folder).as_str().to_owned());
                }
            }
            if let Some(channel) = ahp_session::session::agents::Agents::channel(store, session) {
                if !channel.provider.is_empty() {
                    options.provider = Some(channel.provider.clone());
                }
                options.config = channel.config.as_ref().map(|config| config.values.clone());
            }
        }

        let server = host;
        fx.push(
            AnyEffect::new(ahp_wire::effects::CreateSessionEffect {
                client: client.session.clone(),
                working_directories,
                options,
            })
            .map(move |result| {
                AppCommand::Windowed(
                    window,
                    Arc::new(OpenCreatedSession {
                        server,
                        open_chat: true,
                        initial_prompt: None,
                        result,
                    }),
                )
            }),
        );
    }
}

pub fn open_session(
    store: &mut Store,
    window: ::workbench::window::WindowId,
    server: HostId,
    session: SessionUri,
    open_chat: bool,
    fx: &mut crate::app::AppFx<'_>,
) {
    open_session_with(store, window, server, session, open_chat, None, fx)
}

pub(crate) fn open_session_with(
    store: &mut Store,
    window: ::workbench::window::WindowId,
    server: HostId,
    session: SessionUri,
    open_chat: bool,
    initial_prompt: Option<String>,
    fx: &mut crate::app::AppFx<'_>,
) {
    let Some(client) = ahp_wire::client::Servers::client(store, server) else {
        eprintln!("[higent] open-session: unregistered server {server:?}");
        return;
    };
    let landing = session.clone();
    fx.push(
        AnyEffect::new(SubscribeSessionEffect {
            client: client.session.clone(),
            session,
        })
        .map(move |result| {
            AppCommand::Windowed(
                window,
                Arc::new(OpenSubscribedSession {
                    server,
                    session: landing.clone(),
                    open_chat,
                    initial_prompt: initial_prompt.clone(),
                    result,
                }),
            )
        }),
    );
}

pub(crate) struct OpenSubscribedSession {
    pub server: HostId,
    pub session: SessionUri,

    pub open_chat: bool,

    pub initial_prompt: Option<String>,
    pub result: Result<ahp_types::state::SessionState, String>,
}

impl WindowedCommand for OpenSubscribedSession {
    fn id(&self) -> &'static str {
        "agent.open-session"
    }

    fn name(&self) -> String {
        "Open Agent Session".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
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
        crate::app::switch_session(store, window, key.clone(), fx);

        fx.follow_up(AppCommand::Windowed(
            window,
            Arc::new(EnterSessionWork {
                server: self.server,
                session: self.session.clone(),
                open_chat: self.open_chat,
                initial_prompt: self.initial_prompt.clone(),
                default_chat: state.default_chat.clone().map(ChatUri::new),
            }),
        ));
    }
}

struct EnterSessionWork {
    server: HostId,
    session: SessionUri,
    open_chat: bool,
    initial_prompt: Option<String>,
    default_chat: Option<ChatUri>,
}

impl WindowedCommand for EnterSessionWork {
    fn id(&self) -> &'static str {
        "agent.enter-session"
    }

    fn name(&self) -> String {
        "Enter Agent Session".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = ui;
        let key = SessionId {
            host: self.server,
            session: self.session.clone(),
        };
        let folders = ahp_session::session::folders::session_folders(store, &key);
        if let Some(state) = crate::workspace::session_state(store, window) {
            fx.scope(crate::app::AppCommand::Verb, |fx| {
                ahp_changes::changes::ensure(store, state.changes_wire(), folders.clone(), fx)
            });
            fx.scope(crate::app::AppCommand::Verb, |fx| {
                for folder in folders {
                    ahp_comments::ensure(store, state.comments_wire(), &folder, fx);
                    ahp_lsp::diagnostics::ensure(store, state.diagnostics_wire(), &folder, fx);
                }
            });
        }
        // The poll is the session MIRROR's lifeline, not the chat's:
        // launched before the chat-open block so no early return in
        // it can orphan the channel mirror for good.
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            ahp_session::session::driver::relaunch_session_poll(store, key.clone(), fx)
        });
        if let Some(chat) = self.default_chat.clone().filter(|_| self.open_chat) {
            let prompt = self
                .initial_prompt
                .clone()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty());
            let mut entity = Windows::window(store, window).expect("the window entity");
            // The window switched to this session a batch ago; if it
            // has moved on since, the entry is abandoned — the chat
            // must not be filed into whatever session is there now.
            if crate::workspace::entity_session(&entity) != key {
                Windows::put(store, window, entity);
                return;
            }
            let pane = ahp_chat::chats::Chats::open_with(
                store,
                ui,
                crate::workspace::entity_state(&entity).chats(),
                self.server,
                self.session.clone(),
                chat,
                prompt,
            );

            let _ = entity.open_chat_panel(store, ui, pane, fx);
            Windows::put(store, window, entity);
        }
    }
}

pub struct OpenCreatedSession {
    pub server: HostId,

    pub open_chat: bool,

    pub initial_prompt: Option<String>,
    pub result: Result<SessionUri, String>,
}

impl WindowedCommand for OpenCreatedSession {
    fn id(&self) -> &'static str {
        "agent.open-created-session"
    }

    fn name(&self) -> String {
        "Open Created Session".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        match &self.result {
            Ok(session) => open_session_with(
                store,
                window,
                self.server,
                session.clone(),
                self.open_chat,
                self.initial_prompt.clone(),
                fx,
            ),
            Err(error) => eprintln!("[higent] createSession failed: {error}"),
        }
    }
}

pub(crate) struct OpenSessionRow {
    pub(crate) server: HostId,
    pub(crate) session: SessionUri,
}

impl WindowedCommand for OpenSessionRow {
    fn id(&self) -> &'static str {
        "agent.open-session-row"
    }

    fn name(&self) -> String {
        "Open Session".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        open_session(store, window, self.server, self.session.clone(), true, fx);
    }
}
