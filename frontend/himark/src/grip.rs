// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The shell's workspace grip: what a window SHOWS is a session, and
//! this is where the workbench's opaque `Grip` learns to say so. The
//! grip bundles the session's NAME (the `SessionId` — rekeys move it)
//! with its id bundle (the `SessionState` — wired at session entry,
//! stable for the session's life). Identity is the name: two grips are
//! the same workspace when their sessions match.

use std::any::Any;

use ahp_session::session::state::SessionState;
use ahp_wire::SessionId;
use imba::store::Store;
use workbench::window::{Grip, Window, WindowId, Windows};

pub struct WorkspaceGrip {
    pub session: SessionId,
    pub state: SessionState,
}

/// The one constructor every window boards through.
pub fn grip(session: SessionId, state: SessionState) -> Box<dyn Grip> {
    Box::new(WorkspaceGrip { session, state })
}

/// The local-host rekey payload (`Window::adopt_workspaces`).
pub struct AdoptLocalHost(pub ahp_wire::client::HostId);

impl Grip for WorkspaceGrip {
    fn documents(&self) -> imba::store::Id<documents::OpenDocuments> {
        self.state.documents()
    }

    fn touched(&self, store: &mut Store, target: &hikit::navigation::NavigationLocation) {
        if let Some(place) = target.place::<hikit::navigation::EditorPlace>() {
            recents::RecentLocations::touch(store, self.state.recents(), &place.location);
        }
    }

    fn front_chat_pane(
        &self,
        store: &mut Store,
    ) -> Option<Box<dyn hikit::panel::DynPanelView>> {
        let chats = self.state.chats();
        let chat = ahp_chat::chats::Chats::list(store, chats).into_iter().next()?;
        let pane = ::workbench::rows::mint(
            store,
            &hikit::pane_row::PaneRow::new(ahp_chat::chats::ChatRow(chats, chat.clone())),
        )?;
        boot_chat_feed(store, chats, chat);
        Some(pane)
    }

    fn is_chat_pane(&self, pane: &dyn hikit::panel::DynPanelView) -> bool {
        pane.pane_row()
            .is_some_and(|row| row.row::<ahp_chat::chats::ChatRow>().is_some())
    }

    fn boot_chat(&self, store: &mut Store, pane: &dyn hikit::panel::DynPanelView) {
        if let Some(ahp_chat::chats::ChatRow(chats, chat)) = pane
            .pane_row()
            .as_ref()
            .and_then(|row| row.row::<ahp_chat::chats::ChatRow>())
        {
            boot_chat_feed(store, *chats, chat.clone());
        }
    }

    fn same(&self, other: &dyn Grip) -> bool {
        other
            .as_any()
            .downcast_ref::<WorkspaceGrip>()
            .is_some_and(|other| other.session == self.session)
    }

    fn clone_grip(&self) -> Box<dyn Grip> {
        Box::new(WorkspaceGrip {
            session: self.session.clone(),
            state: self.state.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn adopt(&mut self, payload: &dyn Any) -> bool {
        let Some(AdoptLocalHost(host)) = payload.downcast_ref::<AdoptLocalHost>() else {
            return false;
        };
        if self.session.session.as_str() == ahp_wire::LOCAL_FS_SESSION && self.session.host != *host
        {
            self.session.host = *host;
            return true;
        }
        false
    }
}

/// The chat connects the moment it is OPEN, painted or not — its
/// feed is addressed by id, like any panel's commands.
fn boot_chat_feed(
    store: &mut Store,
    chats: imba::store::Id<ahp_chat::chats::Chats>,
    chat: ahp_wire::client::ChatUri,
) {
    imba::command::Requests::push(
        store,
        std::sync::Arc::new(ahp_chat::chats::BootChat { chats, chat }),
    );
}

/// The session a window shows.
pub fn entity_session(entity: &Window) -> SessionId {
    entity
        .grip()
        .as_any()
        .downcast_ref::<WorkspaceGrip>()
        .expect("every himark window grips a session")
        .session
        .clone()
}

/// The id bundle of the session a window shows — read from the
/// window's OWN record (docs/entities.md law 3): the bundle is wired
/// at session entry, so no catalog consult and no ambient scope.
pub fn entity_state(entity: &Window) -> SessionState {
    entity
        .grip()
        .as_any()
        .downcast_ref::<WorkspaceGrip>()
        .expect("every himark window grips a session")
        .state
        .clone()
}

pub fn session_state(store: &Store, window: WindowId) -> Option<SessionState> {
    Some(entity_state(Windows::window_ref(store, window)?))
}

pub fn window_session(store: &Store, window: WindowId) -> Option<SessionId> {
    Some(entity_session(Windows::window_ref(store, window)?))
}

/// Does any window hold this session — showing it or stashing it?
/// What the all-empty sweep spares.
pub fn any_window_holds(store: &Store, session: &SessionId) -> bool {
    let Some(windows) = store.get::<Windows>() else {
        return false;
    };
    windows.ids().into_iter().any(|id| {
        windows.entity(id).is_some_and(|window| {
            window.grips().any(|grip| {
                grip.as_any()
                    .downcast_ref::<WorkspaceGrip>()
                    .is_some_and(|grip| grip.session == *session)
            })
        })
    })
}
