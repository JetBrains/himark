// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The Files dock tab's SHELL half: the toggle, the toolbar button
//! and the session closures the windowless tree rides — the view
//! itself is the `filetree` crate's (the UI lives with its model,
//! docs/entities.md).

use ::filetree::{SessionTree, SessionTreeView, TreeCommand};

use std::sync::Arc;

use imba::effect::AnyEffect;
use imba::store::Store;
use skia_safe::{Paint, PathBuilder};

use editor::location::ResourceLocation;

/// Build the shell closures and open the panel — the one door the
/// dock toggle uses.
pub fn open_panel(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    workspace: ahp_wire::SessionId,
    trees: imba::store::Id<SessionTree>,
    reveal: Option<ResourceLocation>,
    fx: &mut imba::effect::Effects<'_, TreeCommand>,
) -> SessionTreeView {
    let folders = ahp_session::session::folders::session_folders(store, &workspace);
    let mirror: Arc<dyn Fn(&Store) -> Vec<ResourceLocation> + Send + Sync> = {
        let workspace = workspace.clone();
        Arc::new(move |store: &Store| {
            ahp_session::session::folders::session_folders(store, &workspace)
        })
    };
    let panel = SessionTreeView::open(
        store,
        ui,
        trees,
        &folders,
        mirror,
        remove_from_session(workspace),
        reveal,
        fx,
    );
    // No follower probe: BOTH shells push `TreeCommand::Follow`
    // explicitly at their navigation spot — the workbench from its
    // batch-tail focus walk, the native host through
    // `himark_tree_follow` as a landing arrives.
    panel
}

/// The remove-from-session dispatch: the ask rides the session's
/// agent channel; the row itself leaves via the stale-roots gate
/// when the echo lands.
fn remove_from_session(
    workspace: ahp_wire::SessionId,
) -> Arc<dyn Fn(&Store, ResourceLocation) -> Option<AnyEffect<TreeCommand>> + Send + Sync> {
    Arc::new(move |store, target| {
        let client = ahp_wire::client::Servers::client(store, workspace.host)?;
        let uris = ahp_session::session::state::Hosts::uris(store, workspace.host)?;
        use ahp_types::actions as wire;
        let directory = uris.uri_of(&target).as_str().to_owned();
        Some(
            AnyEffect::new(ahp_wire::effects::DispatchChatActionEffect {
                client: client.session.clone(),
                channel: workspace.session.as_channel(),
                action: wire::StateAction::SessionWorkingDirectoryRemoved(
                    wire::SessionWorkingDirectoryRemovedAction { directory },
                ),
            })
            .map(TreeCommand::Dispatched),
        )
    })
}

