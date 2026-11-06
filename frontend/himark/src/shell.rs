// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The engine's SHELL DOOR: hand a place to the seated shell
//! (`crate::himark::Shell`) — the knife between app logic and
//! placement. The workbench's command vocabulary lives in the
//! `workbench-shell` crate; this module is only the seat.

use imba::store::Store;
use imba::ui::UiCtx;

/// The windowless door: any road holding the verb lane can hand
/// the seated shell a place. Public: plugin commands landing on a
/// MOUNTED origin (hicode's navigation) take it too.
pub fn navigate_origin(
    store: &mut Store,
    ui: &UiCtx,
    origin: crate::bridge::ViewBridgeId,
    place: impl hikit::navigation::Place,
    fx: &mut imba::command::Fx<'_>,
) {
    let Some(seat) = store.get::<crate::himark::ShellSeat>().cloned() else {
        eprintln!("[shell] no shell seated — navigation dropped");
        return;
    };
    let place = hikit::navigation::NavigationLocation::new(place);
    if !seat.0.navigate(origin, place, store, ui, fx) {
        eprintln!("[shell] navigation declined by every shell — dropped");
    }
}

