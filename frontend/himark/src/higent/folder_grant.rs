// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The add-session-folder road: the native folder picker needs the
//! WINDOW, so the ask lives shell-side; the grant dispatch rides the
//! session's agent channel.

use std::sync::Arc;

use ahp_types::actions::{SessionWorkingDirectorySetAction, StateAction};
use imba::effect::AnyEffect;
use imba::store::Store;

use crate::higent::{HostId, Hosts, SessionUri};

pub struct AddSessionFolders {
    pub server: HostId,
    pub session: SessionUri,
}

impl crate::DynamicCommand for AddSessionFolders {
    fn id(&self) -> &'static str {
        "session.add-folder"
    }

    fn name(&self) -> String {
        "Add Session Folder…".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let server = self.server;
        let session = self.session.clone();
        fx.push(
            AnyEffect::new(crate::new_session::PickFoldersEffect { window }).map(
                move |locations| {
                    crate::app::AppCommand::Dynamic(
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

struct SessionFoldersPicked {
    server: HostId,
    session: SessionUri,
    locations: Vec<editor::location::ResourceLocation>,
}

impl crate::DynamicCommand for SessionFoldersPicked {
    fn id(&self) -> &'static str {
        "session.folder-granted"
    }

    fn name(&self) -> String {
        "Session Folder Granted".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(client) = crate::higent::Servers::client(store, self.server) else {
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
                AnyEffect::new(crate::higent::DispatchChatActionEffect {
                    client: client.session.clone(),
                    channel,
                    action: StateAction::SessionWorkingDirectorySet(
                        SessionWorkingDirectorySetAction { directory },
                    ),
                })
                .map(move |result| {
                    crate::app::AppCommand::Dynamic(window, Arc::new(GrantAck { result }))
                }),
            );
        }
    }
}

struct GrantAck {
    result: Result<(), String>,
}

impl crate::DynamicCommand for GrantAck {
    fn id(&self) -> &'static str {
        "session.folder-grant-ack"
    }

    fn name(&self) -> String {
        "Session Folder Grant Acknowledged".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        _window: crate::WindowId,
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
