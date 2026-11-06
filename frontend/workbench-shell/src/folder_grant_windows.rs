// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The folder-grant WINDOW half: the picker modal command, the
//! add-folders landing and the chat road. The effect and the pick
//! launcher live in the engine (`crate::higent::folder_grant`).

use std::sync::Arc;

use ahp_session::session::state::Hosts;
use ahp_types::actions::{SessionWorkingDirectorySetAction, StateAction};
use ahp_wire::client::{HostId, SessionUri};
use himark::higent::folder_grant::*;
use imba::effect::AnyEffect;
use imba::store::Store;

pub(crate) struct OpenFolderPicker {
    pub server: HostId,
    pub session: SessionUri,
}

impl himark::commands::HimarkCommand for OpenFolderPicker {
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
        use himark::higent::folder_picker::{FolderPicker, FolderPickerCommand};
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
            let pick = himark::higent::folder_grant::folder_pick(store, self.server, self.session.clone());
            let modal: Box<dyn hikit::modal::ModalView> = fx.scope(
                move |command| {
                    crate::shell::content(
                        window,
                        ::workbench::window::WindowCommand::Modal(command),
                    )
                },
                |fx| {
                    fx.scope(
                        imba::dyn_view::DynCommand::new::<FolderPickerCommand>,
                        |fx| {
                            Box::new(FolderPicker::open(store, ui, pick, fx))
                                as Box<dyn hikit::modal::ModalView>
                        },
                    )
                },
            );
            let mut entity =
                ::workbench::window::Windows::window(store, window).expect("the window entity");
            fx.scope(
                move |command| crate::app::content(window, command),
                |fx| entity.show_modal(store, modal, fx),
            );
            ::workbench::window::Windows::put(store, window, entity);
            return;
        }
        // No window holds it — a mounted shell: navigate the place;
        // the native shell presents it (iOS: a sheet).
        if let Some(origin) = himark::mounts::origin_of(store, &target) {
            fx.scope(crate::app::AppCommand::Verb, |fx| {
                himark::shell::navigate_origin(
                    store,
                    ui,
                    origin,
                    himark::places::FolderPickerPlace(target),
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

pub(crate) struct AddSessionFolders {
    pub server: HostId,
    pub session: SessionUri,
}

impl himark::commands::HimarkCommand for AddSessionFolders {
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
        let anchor = himark::commands::bound_origin(self.id(), bound)
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

pub fn install_chat_road(store: &mut Store) {
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
            if let Some(origin) = himark::mounts::origin_of(store, &target) {
                return himark::shell::navigate_origin(
                    store,
                    ui,
                    origin,
                    himark::places::FolderPickerPlace(target),
                    fx,
                );
            }
            return himark::commands::AppRequests::push(
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



struct SessionFoldersPicked {
    server: HostId,
    session: SessionUri,
    locations: Vec<editor::location::ResourceLocation>,
}

impl himark::commands::HimarkCommand for SessionFoldersPicked {
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

impl himark::commands::HimarkCommand for GrantAck {
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

