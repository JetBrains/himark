// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The shell half of comments: the feature gate (a workbench
//! registry flag), the dock toggle, its toolbar button and the
//! comment-pick navigation drain. Everything else — the collection,
//! the cards, the dock panel — lives in the `comments` crate.

use ::comments::panel::CommentsView;

use std::sync::Arc;

use imba::store::Store;

#[cfg(test)]
mod tests;

pub struct ToggleCommentsView;

impl crate::commands::WindowedCommand for ToggleCommentsView {
    fn id(&self) -> &'static str {
        "comments.view"
    }
    fn name(&self) -> String {
        "Comments".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
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
        let comments = entity.state().comments();
        let wire = entity.state().comments_wire();
        let folders = ahp_session::session::folders::session_folders(store, &workspace);
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            for folder in folders {
                ahp_comments::ensure(store, wire, &folder, fx);
            }
        });

        fx.scope(
            move |command| crate::app::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let panel = CommentsView::open(store, ui, comments);
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
        command: "comments.view",
        order: 1.5,
        side: ::workbench::toolbar::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());

            let body = skia_safe::Rect::from_xywh(l + w * 0.18, t + h * 0.2, w * 0.64, h * 0.44);
            let radius = h * 0.12;
            canvas.draw_round_rect(body, radius, radius, &paint);
            let mut path = skia_safe::PathBuilder::new();
            path.move_to((l + w * 0.34, t + h * 0.64));
            path.line_to((l + w * 0.30, t + h * 0.8));
            path.line_to((l + w * 0.46, t + h * 0.64));
            canvas.draw_path(&path.detach(), &paint);
        }),
    }
}
