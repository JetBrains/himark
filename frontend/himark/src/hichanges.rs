// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The shell half of the changes feature: the dock toggle, the
//! palette refetch, the diff-for-pair open — everything that holds a
//! window. The collection and the tree live in the `changesview`
//! crate.


use changesview::hichanges::*;

use std::sync::Arc;

use editor::location::ResourceLocation;
use imba::{effect::AnyEffect, store::Store};


pub struct OpenDiffForPair {
    pub old: ResourceLocation,
    pub new: ResourceLocation,
}

impl crate::commands::DynamicCommand for OpenDiffForPair {
    fn id(&self) -> &'static str {
        "changes.open-diff"
    }
    fn name(&self) -> String {
        "Open Diff".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::app::Application,
        store: &mut Store,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // Resolve both sides on the UI thread — an open side hands over
        // its live registry snapshot, so the diff is against the live
        // buffer and rebases if it moves (docs/no-diff-on-ui-thread).
        let documents = ::workbench::window::Windows::session_state(store, window)
            .expect("a diff opens from a window with a session")
            .documents();
        let old = documents::diff_views::DiffSideInput::resolve(store, documents, self.old.clone());
        let new = documents::diff_views::DiffSideInput::resolve(store, documents, self.new.clone());
        let _ = fx.push(AnyEffect::new(crate::workspace::OpenDiffByLocationsEffect {
            window,
            documents,
            old,
            new,
        }));
    }
}

pub struct ToggleChangesView;

impl crate::commands::DynamicCommand for ToggleChangesView {
    fn id(&self) -> &'static str {
        "changes.view"
    }
    fn name(&self) -> String {
        "Changes".to_owned()
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
        // The pane closes over the window's session: its change sets
        // by id, the session only as the catalog's name for the folders.
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
        let view = Changes::mint_view(
            store,
            changes,
            changesview::changes_view::ChangesView::open(
                store,
                &_app.ui_ctx(),
                changes,
                changesview::changes_view::ViewSets::WorkingCopies,
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

/// Refetch changesets — one repository's when `folder` names it, every
/// riding folder's otherwise (the palette / test road).
#[derive(Default)]
pub struct RefetchChanges {
    /// The wire to refetch through. `None` means "the window's
    /// session's", resolved when the command performs — a command
    /// registered into the palette holds no id at registration.
    pub wire: Option<imba::store::Id<ahp_changes::changes::ChangesWire>>,
    pub folder: Option<ResourceLocation>,
}

impl crate::commands::DynamicCommand for RefetchChanges {
    fn id(&self) -> &'static str {
        "changes.refetch"
    }
    fn name(&self) -> String {
        "Refresh Changes".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::app::Application,
        store: &mut Store,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let _ = fx;
        let Some(changes) = ::workbench::window::Windows::session_state(store, window)
            .map(|state| state.changes())
            .or_else(|| {
                // An addressed refetch (the view's chip) names its
                // wire's collection directly.
                self.wire
                    .and_then(|wire| ahp_changes::changes::changes_of(store, wire))
            })
        else {
            return;
        };
        Changes::ask_refetch(store, changes, self.folder.clone());
    }
}

pub fn toolbar_button() -> ::workbench::toolbar::ToolbarButton {
    ::workbench::toolbar::ToolbarButton {
        command: "changes.view",
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

#[cfg(test)]
mod tests;
