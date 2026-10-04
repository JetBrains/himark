// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The contents drawer's SHELL half: the toggle, the toolbar button
//! and the windowed jump — the views are the `toc` crate's (the UI
//! lives with its machinery, docs/entities.md).

use imba::store::Store;
use skia_safe::Paint;

pub use ::toc::{
    OutlineCommand, OutlineEffect, OutlineHandler, OutlineKey, OutlineRows, OutlineView,
    TocCommand, TocView,
};

pub struct NavigateToPlace {
    pub place: crate::EditorPlace,
}

impl crate::DynamicCommand for NavigateToPlace {
    fn id(&self) -> &'static str {
        "toc.jump"
    }
    fn name(&self) -> String {
        "Jump to Symbol".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };
        let _ = entity.navigate(
            store,
            ui,
            window,
            &crate::NavigationLocation::new(self.place.clone()),
            fx,
        );
        crate::Windows::put(store, window, entity);
    }
}

pub fn toolbar_button() -> crate::ToolbarButton {
    crate::ToolbarButton {
        command: "toc.toggle",
        order: 2.0,
        side: crate::ToolbarSide::Left,
        glyph: std::sync::Arc::new(|canvas, rect, color| {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());
            let rows = [t + h * 0.14, t + h * 0.38, t + h * 0.62, t + h * 0.86];
            let mut path = skia_safe::PathBuilder::new();
            path.move_to((l, rows[0]));
            path.line_to((l + w, rows[0]));
            path.move_to((l + w * 0.28, rows[1]));
            path.line_to((l + w, rows[1]));
            path.move_to((l + w * 0.28, rows[2]));
            path.line_to((l + w, rows[2]));
            path.move_to((l, rows[3]));
            path.line_to((l + w, rows[3]));
            canvas.draw_path(&path.detach(), &paint);
        }),
    }
}

pub struct ToggleToc;

impl crate::DynamicCommand for ToggleToc {
    fn id(&self) -> &'static str {
        "toc.toggle"
    }
    fn name(&self) -> String {
        "Table of Contents".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };
        if entity.side_panel().is_some_and(|panel| {
            panel.as_any().is::<TocView>() || panel.as_any().is::<OutlineView>()
        }) {
            entity.roll_away_side_panel();
            crate::Windows::put(store, window, entity);
            return;
        }
        let Some(panel) =
            entity
                .workbench()
                .root
                .focused_pane()
                .drawer_view(store, &_app.ui_ctx(), window)
        else {
            crate::Windows::put(store, window, entity);
            return;
        };
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.show_side_panel(store, panel, fx),
        );
        crate::Windows::put(store, window, entity);
    }
}
