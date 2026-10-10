// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The add-session-folder road: the native folder picker needs the
//! WINDOW, so the ask lives shell-side; the grant dispatch rides the
//! session's agent channel.

use std::sync::Arc;

use ahp_types::actions::{SessionWorkingDirectorySetAction, StateAction};
use imba::effect::AnyEffect;
use imba::store::Store;

use ahp_session::session::state::Hosts;
use ahp_wire::client::HostId;
use ahp_wire::client::SessionUri;

/// The native folder picker, asked from the SHELL (it owns the
/// window): the effect's handler lives host-side.
pub struct PickFoldersEffect {
    pub window: ::workbench::window::WindowId,
}

impl std::fmt::Display for PickFoldersEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("pick folders")
    }
}

impl imba::effect::Effect for PickFoldersEffect {
    type Result = Vec<editor::location::ResourceLocation>;
}

pub(crate) struct AddSessionFolders {
    pub server: HostId,
    pub session: SessionUri,
}

impl crate::commands::WindowedCommand for AddSessionFolders {
    fn id(&self) -> &'static str {
        "session.add-folder"
    }

    fn name(&self) -> String {
        "Add Session Folder…".to_owned()
    }

    fn perform(
        &self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let server = self.server;
        let session = self.session.clone();
        fx.push(
            AnyEffect::new(PickFoldersEffect { window }).map(
                move |locations| {
                    crate::app::AppCommand::Windowed(
                        window,
                        Arc::new(SessionFoldersPicked {
                            server,
                            session: session.clone(),
                            locations,
                        }),
                    )
                },
            ),
        );
    }
}


/// Boot: the chat toolbar's add-folder road. The toolbar (ahp-chat)
/// cannot name windows or the picker effect; it hands the ask back
/// through `imba::command::Requests`, and this command resolves the
/// WINDOW showing the session and runs the same road as
/// `session.add-folder`.
pub(crate) fn install_chat_road(store: &mut Store) {
    store.put(ahp_chat::session_toolbar::AddFolderRoad(Arc::new(
        |server, session| Arc::new(AddFolderFromChat { server, session }),
    )));
}

struct AddFolderFromChat {
    server: HostId,
    session: SessionUri,
}

impl imba::command::DynamicCommand for AddFolderFromChat {
    fn id(&self) -> &'static str {
        "session.add-folder-from-chat"
    }

    fn name(&self) -> String {
        "Add Session Folder…".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) {
        let Some(windows) = store.get::<::workbench::window::Windows>() else {
            return;
        };
        let target = ahp_wire::SessionId {
            host: self.server,
            session: self.session.clone(),
        };
        let ids = windows.ids();
        let Some(window) = ids
            .iter()
            .copied()
            .find(|id| {
                windows
                    .entity(*id)
                    .is_some_and(|entity| crate::workspace::entity_session(entity) == target)
            })
            .or(ids.first().copied())
        else {
            return;
        };
        let (server, session) = (self.server, self.session.clone());
        fx.push(
            AnyEffect::new(PickFoldersEffect { window }).map(move |locations| {
                imba::command::Verb::Dynamic(Arc::new(ChatFoldersPicked {
                    server,
                    session: session.clone(),
                    locations,
                }))
            }),
        );
    }
}

struct ChatFoldersPicked {
    server: HostId,
    session: SessionUri,
    locations: Vec<editor::location::ResourceLocation>,
}

impl imba::command::DynamicCommand for ChatFoldersPicked {
    fn id(&self) -> &'static str {
        "session.folder-granted-from-chat"
    }

    fn name(&self) -> String {
        "Session Folder Granted".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) {
        let Some(client) = ahp_wire::client::Servers::client(store, self.server) else {
            eprintln!(
                "[higent] chat folder grant DROPPED: no client for {:?} ({})",
                self.server,
                self.session.as_str()
            );
            return;
        };
        let Some(uris) = Hosts::uris(store, self.server) else {
            eprintln!(
                "[higent] chat folder grant DROPPED: no uri map for {:?} ({})",
                self.server,
                self.session.as_str()
            );
            return;
        };
        for location in &self.locations {
            let directory = uris.uri_of(location).as_str().to_owned();
            let channel = self.session.as_channel();
            eprintln!("[higent] granting folder {directory} to {channel}");
            fx.push(
                AnyEffect::new(ahp_wire::effects::DispatchChatActionEffect {
                    client: client.session.clone(),
                    channel,
                    action: StateAction::SessionWorkingDirectorySet(
                        SessionWorkingDirectorySetAction { directory },
                    ),
                })
                .map(|result| imba::command::Verb::Dynamic(Arc::new(ChatGrantAck { result }))),
            );
        }
    }
}

struct ChatGrantAck {
    result: Result<(), String>,
}

impl imba::command::DynamicCommand for ChatGrantAck {
    fn id(&self) -> &'static str {
        "session.folder-grant-ack-from-chat"
    }

    fn name(&self) -> String {
        "Session Folder Grant Acknowledged".to_owned()
    }

    fn perform(&self, _store: &mut Store, _ui: &imba::ui::UiCtx, _fx: &mut imba::command::Fx<'_>) {
        if let Err(error) = &self.result {
            eprintln!("[higent] chat folder grant dispatch FAILED: {error}");
        }
    }
}

struct SessionFoldersPicked {
    server: HostId,
    session: SessionUri,
    locations: Vec<editor::location::ResourceLocation>,
}

impl crate::commands::WindowedCommand for SessionFoldersPicked {
    fn id(&self) -> &'static str {
        "session.folder-granted"
    }

    fn name(&self) -> String {
        "Session Folder Granted".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(client) = ahp_wire::client::Servers::client(store, self.server) else {
            eprintln!(
                "[higent] folder grant DROPPED: no client for {:?} ({})",
                self.server,
                self.session.as_str()
            );
            return;
        };
        let Some(uris) = Hosts::uris(store, self.server) else {
            eprintln!(
                "[higent] folder grant DROPPED: no uri map for {:?} ({})",
                self.server,
                self.session.as_str()
            );
            return;
        };
        for location in &self.locations {
            let directory = uris.uri_of(location).as_str().to_owned();
            let channel = self.session.as_channel();
            eprintln!("[higent] granting folder {directory} to {channel}");
            fx.push(
                AnyEffect::new(ahp_wire::effects::DispatchChatActionEffect {
                    client: client.session.clone(),
                    channel,
                    action: StateAction::SessionWorkingDirectorySet(
                        SessionWorkingDirectorySetAction { directory },
                    ),
                })
                .map(move |result| {
                    crate::app::AppCommand::Windowed(window, Arc::new(GrantAck { result }))
                }),
            );
        }
    }
}

struct GrantAck {
    result: Result<(), String>,
}

impl crate::commands::WindowedCommand for GrantAck {
    fn id(&self) -> &'static str {
        "session.folder-grant-ack"
    }

    fn name(&self) -> String {
        "Session Folder Grant Acknowledged".to_owned()
    }

    fn perform(
        &self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _window: ::workbench::window::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        // The dispatch is a wire notification: this result is the
        // only trace the grant left the client. A swallowed error
        // here is a folder the user granted and the session never
        // gained — loud beats lost.
        if let Err(error) = &self.result {
            eprintln!("[higent] folder grant dispatch FAILED: {error}");
        }
    }
}
