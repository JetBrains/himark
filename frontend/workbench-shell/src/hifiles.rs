// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The Files dock tab's CHROME: the toggle and the toolbar button.
//! The panel builder (`himark::hifiles::open_panel`) stays with the
//! engine — the mounted files place mints through it too.

use himark::hifiles::open_panel;

use ::filetree::TreeCommand;

use std::sync::Arc;

use imba::store::Store;
use skia_safe::{Paint, PathBuilder};

/// The files feature as a plugin: the tree toggle and its toolbar
/// button.
pub struct ChromePlugin;

impl himark::plugin::Plugin for ChromePlugin {
    fn install(&self, app: &mut himark::app::Himark, _cx: &himark::plugin::PluginContext) {
        use himark::app_ext::AppExt;
        app.register_command(std::sync::Arc::new(ToggleSessionTree));
        app.register_toolbar_button(toolbar_button());
    }
}

pub struct ToggleSessionTree;

impl himark::commands::HimarkCommand for ToggleSessionTree {
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
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
        // The focused location is a state walk over the views now —
        // nothing is laid to answer it.
        let reveal = {
            himark::focus::window_focus_data(store, &ui, window)
                .and_then(|mut data| himark::focus::focused_location(&mut data))
        };
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            ::workbench::window::Windows::put(store, window, entity);
            return;
        }

        fx.scope(
            move |command| himark::shell::content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );

        let workspace = himark::workspace::entity_session(&entity);
        let trees = himark::workspace::entity_state(&entity).trees();
        let panel = fx.scope(crate::modal::dock_scope(window), |fx| {
            // The dock modal is the SCROLLED wrapper now: the
            // landings wrap into its content layer.
            fx.scope(
                |command: TreeCommand| {
                    imba::dyn_view::DynCommand::new(imba::scroll::ScrollCommand::Content(command))
                },
                |fx| open_panel(store, ui, workspace, trees, reveal, fx),
            )
        });
        let owner = self.id();
        fx.scope(
            move |command| himark::shell::content(window, command),
            |fx| {
                entity.show_dock(
                    store,
                    Box::new(hikit::modal::Scrolled::new(panel)),
                    owner,
                    fx,
                )
            },
        );
        ::workbench::window::Windows::put(store, window, entity);
    }
}

pub fn toolbar_button() -> ::workbench::toolbar::ToolbarButton {
    ::workbench::toolbar::ToolbarButton {
        command: "files.tree",
        label: "Files",
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
        let session = himark::test_support::seed_session_folders(
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
