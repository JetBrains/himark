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
fn host_is_local(store: &Store, server: HostId) -> bool {
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
pub(crate) fn folder_pick(
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
pub(crate) struct OpenFolderPicker {
    pub server: HostId,
    pub session: SessionUri,
}

impl crate::commands::HimarkCommand for OpenFolderPicker {
    fn id(&self) -> &'static str {
        "session.open-folder-picker"
    }

    fn name(&self) -> String {
        "Add Session Folder…".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        _bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        use crate::higent::folder_picker::{FolderPicker, FolderPickerCommand};
        let target = ahp_wire::SessionId {
            host: self.server,
            session: self.session.clone(),
        };

        // A window showing the session: the picker sheets there.
        let window = store
            .get::<::workbench::window::Windows>()
            .and_then(|windows| {
                windows.ids().into_iter().find(|id| {
                    windows
                        .entity(*id)
                        .is_some_and(|entity| crate::workspace::entity_session(entity) == target)
                })
            });
        if let Some(window) = window {
            let pick = folder_pick(store, self.server, self.session.clone());
            let modal: Box<dyn hikit::modal::ModalView> = fx.scope(
                move |command| {
                    crate::shell::content(
                        window,
                        ::workbench::window::WindowCommand::Modal(command),
                    )
                },
                |fx| {
                    fx.scope(imba::dyn_view::DynCommand::new::<FolderPickerCommand>, |fx| {
                        Box::new(FolderPicker::open(store, ui, pick, fx))
                            as Box<dyn hikit::modal::ModalView>
                    })
                },
            );
            let mut entity = ::workbench::window::Windows::window(store, window)
                .expect("the window entity");
            fx.scope(
                move |command| crate::app::content(window, command),
                |fx| entity.show_modal(store, modal, fx),
            );
            ::workbench::window::Windows::put(store, window, entity);
            return;
        }
        // No window holds it — a mounted shell: navigate the place;
        // the native shell presents it (iOS: a sheet).
        if let Some(origin) = crate::mounts::origin_of(store, &target) {
            fx.scope(crate::app::AppCommand::Verb, |fx| {
                crate::shell::navigate_origin(
                    store,
                    ui,
                    origin,
                    crate::places::FolderPickerPlace(target),
                    fx,
                )
            });
            return;
        }
        eprintln!(
            "[higent] folder picker DROPPED: nothing shows {} on {:?}",
            self.session.as_str(),
            self.server
        );
    }
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

impl crate::commands::HimarkCommand for AddSessionFolders {
    fn id(&self) -> &'static str {
        "session.add-folder"
    }

    fn name(&self) -> String {
        "Add Session Folder…".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // A remote host: the system chooser browses the WRONG machine
        // — the in-app picker shows the session host's tree instead.
        if !host_is_local(store, self.server) {
            return OpenFolderPicker {
                server: self.server,
                session: self.session.clone(),
            }
            .perform(store, ui, bound, fx);
        }
        // The picker needs no window — the host sheets it where it
        // likes; the ORIGIN rides along as the anchor id.
        let anchor = crate::commands::bound_origin(self.id(), bound)
            .map(|origin| origin.raw())
            .unwrap_or(0);
        let server = self.server;
        let session = self.session.clone();
        fx.push(
            AnyEffect::new(PickFoldersEffect { window: anchor }).map(move |locations| {
                crate::shell::bound(
                    imba::focus::Bindings::new(),
                    Arc::new(SessionFoldersPicked {
                        server,
                        session: session.clone(),
                        locations,
                    }),
                )
            }),
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

    fn perform(&self, store: &mut Store, ui: &imba::ui::UiCtx, fx: &mut imba::command::Fx<'_>) {
        let target = ahp_wire::SessionId {
            host: self.server,
            session: self.session.clone(),
        };
        // A remote host: the in-app picker. A window showing the
        // session takes it as a sheet (the app lane drains the ask
        // this same batch); a mounted shell navigates the place and
        // the native host presents it (iOS: a sheet).
        if !host_is_local(store, self.server) {
            if let Some(origin) = crate::mounts::origin_of(store, &target) {
                return crate::shell::navigate_origin(
                    store,
                    ui,
                    origin,
                    crate::places::FolderPickerPlace(target),
                    fx,
                );
            }
            return crate::commands::AppRequests::push(
                store,
                Arc::new(OpenFolderPicker {
                    server: self.server,
                    session: self.session.clone(),
                }),
            );
        }
        // The local host: the system chooser (on macOS it is also the
        // TCC grant). The picker needs no window — requiring one
        // killed this road on every mounted shell. A window SHOWING
        // the session anchors the sheet when one exists; 0 sheets
        // app-modally.
        let anchor = store
            .get::<::workbench::window::Windows>()
            .and_then(|windows| {
                windows.ids().into_iter().find(|id| {
                    windows
                        .entity(*id)
                        .is_some_and(|entity| crate::workspace::entity_session(entity) == target)
                })
            })
            .map(|window| window.raw())
            .unwrap_or(0);
        let (server, session) = (self.server, self.session.clone());
        fx.push(
            AnyEffect::new(PickFoldersEffect { window: anchor }).map(move |locations| {
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

impl crate::commands::HimarkCommand for SessionFoldersPicked {
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
        _bound: &imba::focus::Bindings,
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
                    crate::shell::bound(imba::focus::Bindings::new(), Arc::new(GrantAck { result }))
                }),
            );
        }
    }
}

struct GrantAck {
    result: Result<(), String>,
}

impl crate::commands::HimarkCommand for GrantAck {
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
        _bound: &imba::focus::Bindings,
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
