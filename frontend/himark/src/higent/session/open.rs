// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::higent::{Agents, SessionChannel};
use crate::higent::{ChatUri, SessionUri};
use crate::higent::{HostId, PollSessionEffect, SubscribeSessionEffect};
use crate::{AppCommand, DynamicCommand, SessionId, Windows};
use ahp_types::actions::StateAction;
use ahp_types::state::ChatSummary;
use imba::effect::AnyEffect;
use imba::store::Store;

pub fn open_session(
    store: &mut Store,
    window: crate::WindowId,
    server: HostId,
    session: SessionUri,
    open_chat: bool,
    fx: &mut crate::AppFx<'_>,
) {
    open_session_with(store, window, server, session, open_chat, None, fx)
}

pub fn open_session_with(
    store: &mut Store,
    window: crate::WindowId,
    server: HostId,
    session: SessionUri,
    open_chat: bool,
    initial_prompt: Option<String>,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(client) = crate::higent::Servers::client(store, server) else {
        eprintln!("[higent] open-session: unregistered server {server:?}");
        return;
    };
    let landing = session.clone();
    fx.push(
        AnyEffect::new(SubscribeSessionEffect { client: client.session.clone(), session }).map(move |result| {
            AppCommand::Dynamic(
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

pub struct OpenSubscribedSession {
    pub server: HostId,
    pub session: SessionUri,

    pub open_chat: bool,

    pub initial_prompt: Option<String>,
    pub result: Result<ahp_types::state::SessionState, String>,
}

impl DynamicCommand for OpenSubscribedSession {
    fn id(&self) -> &'static str {
        "agent.open-session"
    }

    fn name(&self) -> String {
        "Open Agent Session".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
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

        Agents::set_channel(
            store,
            &key,
            SessionChannel {
                provider: state.provider.clone(),
                chats: state.chats.iter().cloned().collect(),
                default_chat: state.default_chat.clone().map(ChatUri::new),
                working_directories: state
                    .working_directories
                    .iter()
                    .flatten()
                    .cloned()
                    .collect(),
                config: state.config.clone().map(Arc::new),
            },
        );
        crate::switch_session(store, window, key.clone(), fx);

        fx.follow_up(AppCommand::Dynamic(
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

impl DynamicCommand for EnterSessionWork {
    fn id(&self) -> &'static str {
        "agent.enter-session"
    }

    fn name(&self) -> String {
        "Enter Agent Session".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let key = SessionId {
            host: self.server,
            session: self.session.clone(),
        };
        let folders = crate::higent::session_folders(store, &key);
        if let Some(family) = Windows::session_family(store, window) {
            fx.scope(crate::AppCommand::Verb, |fx| {
                crate::drivers::changes::ensure(store, family.changes_wire(), folders.clone(), fx)
            });
            fx.scope(crate::AppCommand::Verb, |fx| {
                for folder in folders {
                    crate::drivers::comments::ensure(store, family.comments_wire(), &folder, fx);
                }
            });
        }
        // The poll is the session MIRROR's lifeline, not the chat's:
        // launched before the chat-open block so no early return in
        // it can orphan the channel mirror for good.
        relaunch_session_poll(store, window, self.server, self.session.clone(), fx);
        if let Some(chat) = self.default_chat.clone().filter(|_| self.open_chat) {
            let prompt = self
                .initial_prompt
                .clone()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty());
            let mut entity = Windows::window(store, window).expect("the window entity");
            // The window switched to this session a batch ago; if it
            // has moved on since, the entry is abandoned — the chat
            // must not be filed into whatever family is there now.
            if entity.current_session() != key {
                Windows::put(store, window, entity);
                return;
            }
            let pane = crate::higent::Chats::open_with(
                store,
                ui,
                entity.family().chats(),
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

fn relaunch_session_poll(
    store: &Store,
    window: crate::WindowId,
    server: HostId,
    session: SessionUri,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(client) = crate::higent::Servers::client(store, server) else {
        return;
    };
    let landing = session.clone();
    fx.push(
        AnyEffect::new(PollSessionEffect { client: client.session.clone(), session }).map(move |actions| {
            AppCommand::Dynamic(
                window,
                Arc::new(ApplySessionActions {
                    server,
                    session: landing.clone(),
                    actions,
                }),
            )
        }),
    );
}

struct ApplySessionActions {
    server: HostId,
    session: SessionUri,
    actions: Vec<StateAction>,
}

impl DynamicCommand for ApplySessionActions {
    fn id(&self) -> &'static str {
        "agent.session-actions"
    }

    fn name(&self) -> String {
        "Apply Session Actions".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let key = SessionId {
            host: self.server,
            session: self.session.clone(),
        };
        apply_channel_actions(store, window, &key, &self.actions, fx);

        if Agents::live_session(store, &key).is_some() {
            relaunch_session_poll(store, window, self.server, self.session.clone(), fx);
        } else {
            // The chain's one legitimate end — a disposed session.
            // Anything else parked here is a mirror frozen for good,
            // so the retirement leaves a trace.
            eprintln!(
                "[higent] session poll retired: {} is no longer live",
                self.session.as_str()
            );
        }
    }
}

/// Applies a drained session-channel batch to the local mirror and
/// fans out its side effects. TWO standing pollers drain the one
/// shared wire feed for a session — this module's and hichanges' —
/// and whichever wakes first takes the whole batch, so both must
/// route every action kind through here; a partial handler silently
/// loses the rest of the batch for everyone.
pub(crate) fn apply_channel_actions(
    store: &mut Store,
    _window: crate::WindowId,
    key: &SessionId,
    actions: &[StateAction],
    fx: &mut crate::AppFx<'_>,
) {
    let mut channel = Agents::channel(store, key).unwrap_or_default();
    let mut folders_grew = false;
    for action in actions {
        match action {
            StateAction::SessionChatAdded(added) => {
                let kept: rpds::VectorSync<ChatSummary> = channel
                    .chats
                    .iter()
                    .filter(|held| held.resource != added.summary.resource)
                    .cloned()
                    .collect();
                let mut chats = kept;
                chats.push_back_mut(added.summary.clone());
                channel.chats = chats;
            }
            StateAction::SessionChatRemoved(removed) => {
                channel.chats = channel
                    .chats
                    .iter()
                    .filter(|held| held.resource != removed.chat)
                    .cloned()
                    .collect();
            }
            StateAction::SessionChatUpdated(updated) => {
                channel.chats = channel
                    .chats
                    .iter()
                    .map(|held| {
                        let mut held = held.clone();
                        if held.resource == updated.chat {
                            merge_chat_summary(&mut held, updated);
                        }
                        held
                    })
                    .collect();
            }
            StateAction::SessionWorkingDirectorySet(set) => {
                if !channel
                    .working_directories
                    .iter()
                    .any(|held| held == &set.directory)
                {
                    channel
                        .working_directories
                        .push_back_mut(set.directory.clone());
                    folders_grew = true;
                }
            }
            StateAction::SessionWorkingDirectoryRemoved(removed) => {
                channel.working_directories = channel
                    .working_directories
                    .iter()
                    .filter(|held| **held != removed.directory)
                    .cloned()
                    .collect();
            }

            StateAction::SessionConfigChanged(changed) => {
                if let Some(held) = &channel.config {
                    let mut config = (**held).clone();
                    if changed.replace.unwrap_or(false) {
                        config.values = changed.config.clone();
                    } else {
                        for (key, value) in &changed.config {
                            config.values.insert(key.clone(), value.clone());
                        }
                    }
                    channel.config = Some(Arc::new(config));
                }
            }
            StateAction::SessionChangesetsChanged(changed) => {
                // The session channel's catalog names the session it
                // serves; its family's DRIVER takes the entries.
                if let Some(wire) =
                    crate::higent::Hosts::family(store, key).map(|family| family.changes_wire())
                {
                    fx.scope(crate::AppCommand::Verb, |fx| {
                        crate::drivers::changes::adopt_session_catalog(
                            store, key, wire, changed, fx,
                        )
                    });
                }
            }
            _ => {}
        }
    }
    Agents::set_channel(store, key, channel);

    if folders_grew {
        // The attach path (`EnterSessionWork`) arms comments for
        // every folder; one added mid-session gets the same here.
        // The channel names the session it serves; the family's
        // collection takes the folder.
        if let Some(wire) =
            crate::higent::Hosts::family(store, key).map(|family| family.comments_wire())
        {
            let folders = crate::higent::session_folders(store, key);
            fx.scope(crate::AppCommand::Verb, |fx| {
                for folder in folders {
                    crate::drivers::comments::ensure(store, wire, &folder, fx);
                }
            });
        }
    }
}

fn merge_chat_summary(
    summary: &mut ChatSummary,
    updated: &ahp_types::actions::SessionChatUpdatedAction,
) {
    let changes = &updated.changes;
    if let Some(title) = &changes.title {
        summary.title = title.clone();
    }
    if let Some(status) = changes.status {
        summary.status = status;
    }
    if let Some(activity) = &changes.activity {
        summary.activity = Some(activity.clone());
    }
    if let Some(modified) = &changes.modified_at {
        summary.modified_at = modified.clone();
    }
}

pub struct OpenCreatedSession {
    pub server: HostId,

    pub open_chat: bool,

    pub initial_prompt: Option<String>,
    pub result: Result<SessionUri, String>,
}

impl DynamicCommand for OpenCreatedSession {
    fn id(&self) -> &'static str {
        "agent.open-created-session"
    }

    fn name(&self) -> String {
        "Open Created Session".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
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

impl DynamicCommand for OpenSessionRow {
    fn id(&self) -> &'static str {
        "agent.open-session-row"
    }

    fn name(&self) -> String {
        "Open Session".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        open_session(store, window, self.server, self.session.clone(), true, fx);
    }
}
