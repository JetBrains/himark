// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The shell half of the history feature: the dock toggle and its
//! toolbar button. The collection and the commit graph live in the
//! `changesview` crate.


use changesview::hihistory::*;

use std::sync::Arc;

use imba::store::Store;


pub struct ToggleHistoryView;

impl crate::commands::DynamicCommand for ToggleHistoryView {
    fn id(&self) -> &'static str {
        "history.view"
    }
    fn name(&self) -> String {
        "History".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::app::Application,
        store: &mut Store,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let mut entity = ::workbench::window::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            ::workbench::window::Windows::put(store, window, entity);
            return;
        }
        let workspace = entity.current_session();
        let changes = entity.state().changes();
        let wire = entity.state().changes_wire();
        let folders = ahp_session::session::folders::session_folders(store, &workspace);
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            ahp_changes::changes::ensure(store, wire, folders, fx)
        });
        // The canvas-open verb the tree emits — the window rides in
        // the closure; the view never holds one.
        let open_canvas: changesview::changes_view::CanvasOpener = Arc::new(move |source, reveal| {
            crate::app::shell_verb(crate::app::AppCommand::Dynamic(
                window,
                Arc::new(crate::diff_canvas::OpenDiffCanvas {
                    changes,
                    source,
                    reveal,
                }),
            ))
        });
        fx.scope(
            move |command| crate::app::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let view = changesview::hichanges::Changes::mint_view(
            store,
            changes,
            changesview::changes_view::ChangesView::open(
                store,
                &_app.ui_ctx(),
                changes,
                changesview::changes_view::ViewSets::History,
                open_canvas,
            ),
        );
        let owner = self.id();
        fx.scope(
            move |command| crate::app::AppCommand::Content(window, command),
            |fx| {
                entity.show_dock(
                    store,
                    Box::new(changesview::changes_view::ChangesPane::new(changes, view)),
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
        command: "history.view",
        order: 1.1,
        side: ::workbench::toolbar::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());

            let x = l + w * 0.35;
            let radius = w * 0.10;
            let dots = [t + h * 0.22, t + h * 0.5, t + h * 0.78];
            let mut path = skia_safe::PathBuilder::new();
            path.move_to((x, dots[0] + radius));
            path.line_to((x, dots[2] - radius));
            canvas.draw_path(&path.detach(), &paint);
            for (index, y) in dots.iter().enumerate() {
                canvas.draw_circle((x, *y), radius, &paint);
                if index == 1 {
                    let mut branch = skia_safe::PathBuilder::new();
                    branch.move_to((x + radius, *y - radius * 0.4));
                    branch.line_to((l + w * 0.72, t + h * 0.32));
                    canvas.draw_path(&branch.detach(), &paint);
                }
            }
        }),
    }
}

#[cfg(test)]
mod tests;
