// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use super::{Agents, SessionChannel};
use crate::higent::{HostId, PollSessionEffect, SubscribeSessionEffect};
use crate::{AppCommand, DynamicCommand, SessionId, Windows};
use ahp_types::actions::StateAction;
use ahp_types::common::Uri;
use ahp_types::state::ChatSummary;
use imba::effect::AnyEffect;
use imba::store::Store;

pub fn open_session(
    store: &mut Store,
    window: crate::WindowId,
    server: HostId,
    session: Uri,
    open_chat: bool,
    fx: &mut crate::AppFx<'_>,
) {
    open_session_with(store, window, server, session, open_chat, None, fx)
}

pub fn open_session_with(
    store: &mut Store,
    window: crate::WindowId,
    server: HostId,
    session: Uri,
    open_chat: bool,
    initial_prompt: Option<String>,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(seat) = crate::higent::Servers::seat(store, server) else {
        eprintln!("[higent] open-session: unregistered server {server:?}");
        return;
    };
    let landing = session.clone();
    fx.push(
        AnyEffect::new(SubscribeSessionEffect { seat, session }).map(move |result| {
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
    pub session: Uri,

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
                default_chat: state.default_chat.clone(),
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

        crate::commands::BatchRequests::push(
            store,
            window,
            Arc::new(EnterSessionWork {
                server: self.server,
                session: self.session.clone(),
                open_chat: self.open_chat,
                initial_prompt: self.initial_prompt.clone(),
                default_chat: state.default_chat.clone(),
            }),
        );
    }
}

struct EnterSessionWork {
    server: HostId,
    session: Uri,
    open_chat: bool,
    initial_prompt: Option<String>,
    default_chat: Option<Uri>,
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
        crate::hichanges::Changes::ensure(store, window, key.clone(), fx);
        for folder in crate::higent::session_folders(store, &key) {
            crate::hicomments::Comments::ensure(store, window, &folder, fx);
        }
        if let Some(chat) = self.default_chat.clone().filter(|_| self.open_chat) {
            let prompt = self
                .initial_prompt
                .clone()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty());
            let pane = crate::higent::Chats::open_with(
                store,
                ui,
                self.server,
                self.session.clone(),
                chat,
                prompt,
            );
            let mut entity = Windows::window(store, window).expect("the window entity");

            let _ = entity.open_panel(store, ui, pane, fx);
            Windows::put(store, window, entity);
        }

        relaunch_session_poll(store, window, self.server, self.session.clone(), fx);
    }
}

fn relaunch_session_poll(
    store: &Store,
    window: crate::WindowId,
    server: HostId,
    session: Uri,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(seat) = crate::higent::Servers::seat(store, server) else {
        return;
    };
    let landing = session.clone();
    fx.push(
        AnyEffect::new(PollSessionEffect { seat, session }).map(move |actions| {
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
    session: Uri,
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
        let mut channel = Agents::channel(store, &key).unwrap_or_default();
        for action in &self.actions {
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
                _ => {}
            }
        }
        Agents::set_channel(store, &key, channel);

        if Agents::live_session(store, &key).is_some() {
            relaunch_session_poll(store, window, self.server, self.session.clone(), fx);
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
    pub result: Result<Uri, String>,
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
    pub(crate) session: Uri,
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
