// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The changes feature's CHROME: the dock toggle and its toolbar
//! button. The collection and the tree live in `changesview`; the
//! engine-half commands stay in himark.

use changesview::hichanges::*;

use std::sync::Arc;

use imba::store::Store;

/// The chrome plugin — registered by workbench shells.
pub struct ChromePlugin;

impl himark::plugin::Plugin for ChromePlugin {
    fn install(&self, app: &mut himark::app::Himark, _cx: &himark::plugin::PluginContext) {
        use himark::app_ext::AppExt;
        app.register_command(Arc::new(ToggleChangesView));
        app.register_toolbar_button(toolbar_button());
    }
}

pub struct ToggleChangesView;

impl himark::commands::HimarkCommand for ToggleChangesView {
    fn id(&self) -> &'static str {
        "changes.view"
    }
    fn name(&self) -> String {
        "Changes".to_owned()
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
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            ::workbench::window::Windows::put(store, window, entity);
            return;
        }
        // The pane closes over the window's session: its change sets
        // by id, the session only as the catalog's name for the folders.
        let workspace = himark::workspace::entity_session(&entity);
        let changes = himark::workspace::entity_state(&entity).changes();
        let wire = himark::workspace::entity_state(&entity).changes_wire();
        let folders = ahp_session::session::folders::session_folders(store, &workspace);
        fx.scope(himark::app::AppCommand::Verb, |fx| {
            ahp_changes::changes::ensure(store, wire, folders, fx)
        });
        // The canvas-open verb the tree emits — the window rides in
        // the closure; the view never holds one.
        let open_canvas: changesview::changes_view::CanvasOpener =
            Arc::new(move |source, reveal| {
                himark::app::shell_verb(himark::shell::windowed(
                    window,
                    Arc::new(himark::diff_canvas::OpenDiffCanvas {
                        changes,
                        source,
                        reveal,
                    }),
                ))
            });

        fx.scope(
            move |command| himark::shell::content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let view = Changes::mint_view(
            store,
            changes,
            changesview::changes_view::ChangesView::open(
                store,
                ui,
                changes,
                changesview::changes_view::ViewSets::WorkingCopies,
                open_canvas,
            ),
        );
        let owner = self.id();
        fx.scope(
            move |command| himark::shell::content(window, command),
            |fx| {
                entity.show_dock(
                    store,
                    Box::new(hikit::modal::Scrolled::new(
                        changesview::changes_view::ChangesPane::new(changes, view),
                    )),
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
        command: "changes.view",
        label: "Changes",
        order: 1.0,
        side: ::workbench::toolbar::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());
            let mut path = skia_safe::PathBuilder::new();

            let (px, py, arm) = (l + w * 0.32, t + h * 0.32, w * 0.17);
            path.move_to((px - arm, py));
            path.line_to((px + arm, py));
            path.move_to((px, py - arm));
            path.line_to((px, py + arm));

            let (mx, my) = (l + w * 0.68, t + h * 0.74);
            path.move_to((mx - arm, my));
            path.line_to((mx + arm, my));
            canvas.draw_path(&path.detach(), &paint);
        }),
    }
}
