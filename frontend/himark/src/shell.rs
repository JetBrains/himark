// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Navigation into the installed [`crate::himark::Shell`] — the
//! boundary between app logic and placement. The workbench's
//! command vocabulary lives in the `workbench-shell` crate; this
//! module only forwards.

use imba::store::Store;
use imba::ui::UiCtx;

/// Hand a place to the installed shell, addressed by the origin
/// view. Public: plugin commands landing on a mounted origin
/// (hicode's navigation) call it too.
pub fn navigate_origin(
    store: &mut Store,
    ui: &UiCtx,
    origin: crate::bridge::ViewBridgeId,
    place: impl hikit::navigation::Place,
    fx: &mut imba::command::Fx<'_>,
) {
    let Some(installed) = store.get::<crate::himark::InstalledShell>().cloned() else {
        eprintln!("[shell] no shell installed — navigation dropped");
        return;
    };
    let place = hikit::navigation::NavigationLocation::new(place);
    if !installed.0.navigate(origin, place, store, ui, fx) {
        eprintln!("[shell] navigation declined by every shell — dropped");
    }
}
