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
/// window): the effect's handler lives host-side. The LOCAL host's
/// road only — the system chooser cannot browse a remote machine
/// (and on macOS it is also the TCC grant, so local keeps it).
pub struct PickFoldersEffect {
    /// The anchor id the host may sheet the picker to — a window's
    /// or a mounted origin's raw id, 0 for none.
    pub window: u64,
}

/// Whether the session's host is the DESIGNATED local one — the one
/// machine the system chooser can actually browse.
pub fn host_is_local(store: &Store, server: HostId) -> bool {
    store
        .get::<ahp_wire::client::LocalHost>()
        .and_then(|local| local.0)
        == Some(server)
}

/// The session host's filesystem as locations: the authority that
/// routes `resourceList` to the host's whole-fs world ("local" for
/// the designated local host, the scoped `ahp:<host>:hihost-fs:/local`
/// otherwise).
fn host_fs_authority(store: &Store, server: HostId) -> editor::location::Authority {
    if host_is_local(store, server) {
        editor::location::Authority::new("local")
    } else {
        editor::location::Authority::new(ahp_wire::client::authority(
            server,
            &SessionUri::new(ahp_wire::LOCAL_FS_SESSION),
        ))
    }
}

/// The picker's roots on a host: home (the connect's
/// `defaultDirectory`) when the host offered one, else the
/// filesystem root.
fn host_roots(store: &Store, server: HostId) -> Vec<editor::location::ResourceLocation> {
    let authority = host_fs_authority(store, server);
    let home = Hosts::host(store, server)
        .and_then(|host| host.home)
        .and_then(|home| {
            ahp_wire::client::ResourceUriMap::location_of(
                &ahp_wire::uris::FileUris,
                &ahp_wire::client::ResourceUri::new(home),
                editor::location::ResourceType::directory(),
                &authority,
            )
        });
    match home {
        Some(home) => vec![home],
        // No metadata (an older host): the filesystem root — an
        // empty-path location lists "/" and names itself by its
        // authority.
        None => vec![editor::location::ResourceLocation::new(
            editor::location::ResourceType::directory(),
            authority,
            Vec::new(),
        )],
    }
}

/// What the picker is opened over for a session's host — the roots,
/// the caption and the grant; shared by the windowed opener and the
/// mounted place arm.
pub fn folder_pick(
    store: &Store,
    server: HostId,
    session: SessionUri,
) -> crate::higent::folder_picker::FolderPick {
    crate::higent::folder_picker::FolderPick {
        roots: host_roots(store, server),
        caption: match Hosts::host(store, server) {
            Some(host) if !host.name.is_empty() => {
                format!("ADD FOLDER · {}", host.name.to_uppercase())
            }
            _ => "ADD FOLDER".to_owned(),
        },
        grant: Arc::new(move |folder| {
            Arc::new(ChatFoldersPicked {
                server,
                session: session.clone(),
                locations: vec![folder],
            })
        }),
    }
}

/// Open the in-app folder picker for a session: the windowed shell
/// sheets it on the window showing the session; a mounted shell
/// navigates `FolderPickerPlace` (iOS presents it natively). Queued
/// through `AppRequests` so the verb-world roads can ask too.
impl std::fmt::Display for PickFoldersEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("pick folders")
    }
}

impl imba::effect::Effect for PickFoldersEffect {
    type Result = Vec<editor::location::ResourceLocation>;
}


pub struct ChatFoldersPicked {
    pub server: HostId,
    pub session: SessionUri,
    pub locations: Vec<editor::location::ResourceLocation>,
}

impl imba::command::DynamicCommand for ChatFoldersPicked {
    fn id(&self) -> &'static str {
        "session.folder-granted-from-chat"
    }

    fn name(&self) -> String {
        "Session Folder Granted".to_owned()
    }

    fn perform(&self, store: &mut Store, _ui: &imba::ui::UiCtx, fx: &mut imba::command::Fx<'_>) {
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

pub struct ChatGrantAck {
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

