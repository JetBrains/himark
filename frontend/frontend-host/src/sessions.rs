// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The sessions drawer's MOUNTED asks. The panel itself is
//! `ahp_session::session::drawer` — the same one the workbench
//! stands in a side slot; only the answers to its gestures differ,
//! because a mounted shell has no window to act in.

use ahp_wire::client::HostId;
use ahp_wire::client::SessionUri;
use imba::command::DynamicOnceCommand;
use imba::store::Store;
use imba::ui::UiCtx;

/// A row was picked. Entering the session is the engine's half
/// (subscribe, adopt, relaunch the poll); the other half is the
/// shell's, which has to re-point its views — so the place naming
/// the session goes to the shell through the one door it has.
pub(crate) struct ShowSession {
    pub(crate) origin: himark::bridge::ViewBridgeId,
    pub(crate) server: HostId,
    pub(crate) session: SessionUri,
}

impl DynamicOnceCommand for ShowSession {
    fn perform(self: Box<Self>, store: &mut Store, ui: &UiCtx, fx: &mut imba::command::Fx<'_>) {
        let key = ahp_wire::SessionId {
            host: self.server,
            session: self.session.clone(),
        };
        Box::new(himark::higent::open_session::EnterHeadless {
            server: self.server,
            session: self.session,
        })
        .perform(store, ui, fx);
        himark::shell::navigate_origin(store, ui, self.origin, himark::places::ChatPlace(key), fx);
    }
}
