// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench's PICKER landings: the Open… command and what a
//! picked set becomes — panes and folder sessions. The C marshaling
//! (the effects, the host bridge) lives in `himark-ffi`.

use std::sync::Arc;

use himark::app::AppFx;
use himark::app::AppCommand;
use himark::commands::HimarkCommand;
use himark_ffi::host::FilePickerEffect;
use imba::effect::AnyEffect;
use imba::store::Store;

use editor::location::ResourceLocation;

use crate::app::bound_window;
use crate::workspace::open_locations;
use crate::app::WorkbenchShell as _;
use crate::app_ext::AppExt as _;

pub struct OpenFilePicker;

impl HimarkCommand for OpenFilePicker {
    fn id(&self) -> &'static str {
        "file.open"
    }
    fn name(&self) -> String {
        "Open…".to_owned()
    }
    fn perform(
        &self,
        _store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(window) = bound_window(self.id(), bound) else {
            return;
        };
        let _ = fx.push(
            AnyEffect::new(FilePickerEffect { origin: himark::bridge::ViewBridgeId::from_raw(window.raw()) }).map(move |locations| {
                crate::shell::windowed(window, Arc::new(OpenPicked { locations }))
            }),
        );
    }
}

struct OpenPicked {
    locations: Vec<ResourceLocation>,
}

fn open_folder_session(
    store: &mut Store,
    window: workbench::window::WindowId,
    folders: &[ResourceLocation],
    fx: &mut AppFx<'_>,
) -> bool {
    let Some((host, client, _)) = ahp_wire::client::route_client(store, "local") else {
        return false;
    };
    let Some(uris) = ahp_session::session::state::Hosts::uris(store, host) else {
        return false;
    };
    let dirs: Vec<String> = folders
        .iter()
        .map(|folder| uris.uri_of(folder).into_string())
        .collect();
    let existing = ahp_session::session::agents::Agents::record(store, host).and_then(|record| {
        record
            .sessions
            .iter()
            .find(|summary| {
                summary.working_directories.as_ref().is_some_and(|held| {
                    held.len() == dirs.len() && dirs.iter().all(|dir| held.contains(dir))
                })
            })
            .map(|summary| ahp_wire::client::SessionUri::new(summary.resource.clone()))
    });
    match existing {
        Some(session) => {
            crate::higent::open_session::open_session(store, window, host, session, false, fx)
        }
        None => {
            let _ = fx.push(
                AnyEffect::new(ahp_wire::effects::CreateSessionEffect {
                    options: ahp_wire::client::SessionOptions::default(),
                    client: client.session.clone(),
                    working_directories: dirs,
                })
                .map(move |result| {
                    crate::shell::windowed(
                        window,
                        Arc::new(crate::higent::open_session::OpenCreatedSession {
                            initial_prompt: None,
                            server: host,
                            open_chat: false,
                            result,
                        }),
                    )
                }),
            );
        }
    }
    true
}

impl HimarkCommand for OpenPicked {
    fn id(&self) -> &'static str {
        "file.open-picked"
    }
    fn name(&self) -> String {
        "Open Picked".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(window) = bound_window(self.id(), bound) else {
            return;
        };
        let ui = ui;
        let (folders, documents): (Vec<_>, Vec<_>) = self
            .locations
            .iter()
            .cloned()
            .partition(|location| location.kind().is_directory());
        if !folders.is_empty() {
            let workspace = workbench::window::Windows::window_ref(store, window)
                .map(|entity| crate::workspace::entity_session(&entity))
                .unwrap_or_else(|| ahp_wire::SessionId::local_default(store));
            let state = crate::workspace::session_state(store, window);
            if let (true, Some(state)) = (workspace.names_session(), state) {
                let (wire, comments_wire) = (state.changes_wire(), state.comments_wire());
                let diagnostics_wire = state.diagnostics_wire();
                for folder in folders {
                    let spelled = editor::location::ResourceLocation::new(
                        folder.kind().clone(),
                        editor::location::Authority::new(ahp_wire::client::authority(
                            workspace.host,
                            &workspace.session,
                        )),
                        folder.path().to_vec(),
                    );
                    fx.scope(AppCommand::Verb, |fx| {
                        ahp_changes::changes::ensure_folder(store, wire, spelled.clone(), fx)
                    });
                    fx.scope(AppCommand::Verb, |fx| {
                        ahp_comments::ensure(store, comments_wire, &spelled, fx)
                    });
                    fx.scope(AppCommand::Verb, |fx| {
                        ahp_lsp::diagnostics::ensure(store, diagnostics_wire, &spelled, fx)
                    });
                }
            } else if !open_folder_session(store, window, &folders, fx) {
                eprintln!("[host] folder pick dropped: no local agent host to session it");
            }
        }
        if !documents.is_empty() {
            open_locations(store, ui, window, &documents, fx);
        }
    }
}
