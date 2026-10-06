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
    window: Option<::workbench::window::WindowId>,
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
    match window {
        Some(window) => panel.following(follow_window(window)),
        None => panel,
    }
}

/// The focus-follow probe: the window's focused location and the
/// focus generation it stands at, read per paint.
fn follow_window(
    window: ::workbench::window::WindowId,
) -> Arc<dyn Fn(&Store) -> Option<(ResourceLocation, u64)> + Send + Sync> {
    Arc::new(move |store| {
        let entity = ::workbench::window::Windows::window_ref(store, window)?;
        entity
            .focused_location()
            .cloned()
            .map(|location| (location, entity.focus_generation()))
    })
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

pub struct ToggleSessionTree;

impl crate::commands::WindowedCommand for ToggleSessionTree {
    fn id(&self) -> &'static str {
        "files.tree"
    }
    fn name(&self) -> String {
        "File Tree".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // The focused location is a state walk over the views now —
        // nothing is laid to answer it.
        let reveal = {
            crate::focus::window_focus_data(store, &ui, window)
                .and_then(|mut data| crate::focus::focused_location(&mut data))
        };
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            ::workbench::window::Windows::put(store, window, entity);
            return;
        }

        fx.scope(
            move |command| crate::app::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );

        let workspace = crate::workspace::entity_session(&entity);
        let trees = crate::workspace::entity_state(&entity).trees();
        let panel = fx.scope(crate::modal::dock_scope(window), |fx| {
            fx.scope(imba::dyn_view::DynCommand::new::<TreeCommand>, |fx| {
                open_panel(store, ui, Some(window), workspace, trees, reveal, fx)
            })
        });
        let owner = self.id();
        fx.scope(
            move |command| crate::app::AppCommand::Content(window, command),
            |fx| entity.show_dock(store, Box::new(panel), owner, fx),
        );
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub fn toolbar_button() -> ::workbench::toolbar::ToolbarButton {
    ::workbench::toolbar::ToolbarButton {
        command: "files.tree",
        order: 0.0,
        side: ::workbench::toolbar::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());
            let rows = [t + h * 0.14, t + h * 0.5, t + h * 0.86];
            let mut path = PathBuilder::new();

            path.move_to((l, rows[0]));
            path.line_to((l + w, rows[0]));

            let trunk = l + w * 0.12;
            path.move_to((trunk, rows[0] + h * 0.14));
            path.line_to((trunk, rows[2]));
            for row in &rows[1..] {
                path.move_to((trunk, *row));
                path.line_to((l + w * 0.28, *row));
                path.move_to((l + w * 0.42, *row));
                path.line_to((l + w, *row));
            }
            canvas.draw_path(&path.detach(), &paint);
        }),
    }
}

#[cfg(test)]
mod tests {
    use imba::store::Store;

    fn directory(path: &[&str]) -> editor::location::ResourceLocation {
        editor::location::ResourceLocation::new(
            editor::location::ResourceType::directory(),
            editor::location::Authority::new("test"),
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
    }

    /// The ceremony's folder dedup — shell semantics; the tree's own
    /// behavior tests live in the `filetree` crate.
    #[test]
    fn opened_roots_join_the_workspace_once() {
        let mut store = Store::new();
        let root = directory(&["project"]);
        let session = crate::test_support::seed_session_folders(
            &mut store,
            &[root.clone(), root.clone(), directory(&["other"])],
        );
        let folders = ahp_session::session::folders::session_folders(&store, &session);
        let unique: std::collections::HashSet<_> = folders
            .iter()
            .map(|folder| folder.path().to_vec())
            .collect();
        assert_eq!(unique.len(), 2);
        assert_eq!(folders[0].path(), root.path());
    }
}
