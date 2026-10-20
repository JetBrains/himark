// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The chat's WINDOW roads: the open-working-copy ask needs the
//! window's documents, so the shell installs the road at boot and
//! keeps the command here.

use std::sync::Arc;

use imba::store::Store;

use ahp_chat::chat::OpenEditedRoad;

/// Install the SHELL roads the protocol crate asks through — the
/// window hold for the session sweep, the catalog-actions apply, and
/// the chat's open-working-copy ask. Called once at boot.
pub(crate) fn install_shell_roads(store: &mut Store) {
    crate::higent::folder_grant::install_chat_road(store);
    // The chat ROW's minter: the row carries its collection, a pane
    // is minted off the id — and a dismantled chat has no home to
    // walk back to, so the holds-check gates the mint.
    ::workbench::rows::RowMinters::register(
        store,
        Arc::new(|store, row| {
            let ahp_chat::chats::ChatRow(chats, chat) = row.row::<ahp_chat::chats::ChatRow>()?;
            store
                .entity(*chats)
                .filter(|rows| rows.holds(chat))
                .map(|_| {
                    Box::new(ahp_chat::chats::ChatPane::new(*chats, chat.clone()))
                        as Box<dyn hikit::panel::DynPanelView>
                })
        }),
    );
    store.put(ahp_session::session::state::WindowGrip(Arc::new(
        |store, scope| crate::workspace::any_window_holds(store, scope),
    )));
    store.put(ahp_wire::ChannelActionsRoad(Arc::new(
        |store, home, actions| {
            crate::commands::AppRequests::push(
                store,
                Arc::new(ApplyChannelActions {
                    home: home.clone(),
                    actions,
                }),
            );
        },
    )));
    OpenEditedRoad::install(
        store,
        OpenEditedRoad(Arc::new(|store, server, session, uri| {
            crate::commands::AppRequests::push(
                store,
                Arc::new(OpenEditedFile {
                    server,
                    session,
                    uri,
                }),
            );
        })),
    );
}

/// Open the working copy behind a chat diff: resolve the wire uri
/// against the session's client authority and open the location —
/// the same road a search hit or a changes row takes.
pub(crate) struct OpenEditedFile {
    server: ahp_wire::client::HostId,
    session: ahp_wire::client::SessionUri,
    uri: String,
}

impl crate::commands::HimarkCommand for OpenEditedFile {
    fn id(&self) -> &'static str {
        "chat.open-edited-file"
    }

    fn name(&self) -> String {
        "Open Edited File".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // The SEATED SHELL opens the file — the chat's edit chips
        // work from any shell.
        let Some(origin) = crate::commands::bound_origin(self.id(), bound) else {
            return;
        };
        let Some(uris) = ahp_session::session::state::Hosts::uris(store, self.server) else {
            return;
        };
        let authority = editor::location::Authority::new(ahp_wire::client::authority(
            self.server,
            &self.session,
        ));
        let Some(location) = uris.location_of(
            &ahp_wire::client::ResourceUri::new(self.uri.as_str()),
            editor::location::ResourceType::document(),
            &authority,
        ) else {
            return;
        };
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            crate::shell::navigate_origin(
                store,
                ui,
                origin,
                crate::places::DocumentPlace {
                    location,
                    target: None,
                    focus: true,
                },
                fx,
            )
        });
    }
}

/// The catalog batch, applied in whichever window the drain runs —
/// the session channel's actions open and close against windows.
struct ApplyChannelActions {
    home: ahp_wire::SessionId,
    actions: Vec<ahp_types::actions::StateAction>,
}

impl crate::commands::HimarkCommand for ApplyChannelActions {
    fn id(&self) -> &'static str {
        "higent.apply-channel-actions"
    }
    fn name(&self) -> String {
        "Apply Session Catalog".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
        let _ = window;
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            ahp_session::session::channel::apply_channel_actions(
                store,
                &self.home,
                &self.actions,
                fx,
            )
        });
    }
}
