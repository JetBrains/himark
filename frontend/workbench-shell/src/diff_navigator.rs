// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The diff place's WINDOWED navigator — the workbench's claim on
//! `DiffPlace`; the engine half (handlers, landings) is
//! `himark::hiahp`.


use imba::effect::AnyEffect;
use imba::store::Store;

pub struct DiffNavigator;

impl ::workbench::navigation::WindowedNavigator for DiffNavigator {
    type Place = canvas::diff_pane::DiffPlace;

    fn navigate(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        place: &canvas::diff_pane::DiffPlace,
        fx: &mut imba::command::Fx<'_>,
    ) -> Option<::workbench::workbench_node::Panel> {
        // Resolve both sides on the UI thread — an open side hands over
        // its live snapshot; the prep runs off-thread and the landing
        // opens the dressed pane. Diffing never runs here.
        let documents = crate::workspace::session_state(store, window)
            .expect("a diff opens from a window with a session")
            .documents();
        let old =
            documents::diff_views::DiffSideInput::resolve(store, documents, place.old.clone());
        let new =
            documents::diff_views::DiffSideInput::resolve(store, documents, place.new.clone());
        fx.push(
            AnyEffect::new(crate::workspace::OpenDiffByLocationsEffect {
                origin: himark::bridge::ViewBridgeId::from_raw(window.raw()),
                documents,
                old,
                new,
            })
            .map(crate::app::shell_verb),
        );
        None
    }
}
