// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use documents::diff_views::DiffSideInput;

use imba::effect::{AnyEffect, Effect};
use imba::store::Store;

use crate::app::{AppCommand, AppFx};
use editor::location::ResourceLocation;


pub struct OpenByLocationEffect {
    pub window: ::workbench::window::WindowId,
    /// The collection the open lands into — stamped at launch.
    pub documents: imba::store::Id<documents::OpenDocuments>,
    pub location: ResourceLocation,
    pub primary: bool,

    pub target: Option<std::ops::Range<documents::text_ext::LineCol>>,

    /// Focus the opened editor (a deliberate jump) or just show it.
    pub focus: bool,
}

impl std::fmt::Display for OpenByLocationEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "open by location /{}", self.location.path().join("/"))
    }
}

impl Effect for OpenByLocationEffect {
    type Result = AppCommand;
}

/// The pane road's boundary type (himark → hiahp): the changes view's
/// "Open Diff" and the diff navigator resolve the two sides on the UI
/// thread and launch this; the handler opens both sides and lands the
/// pane-open command.
pub struct OpenDiffByLocationsEffect {
    pub window: ::workbench::window::WindowId,
    /// The collection both sides register into — stamped at launch.
    pub documents: imba::store::Id<documents::OpenDocuments>,
    pub old: DiffSideInput,
    pub new: DiffSideInput,
}

impl std::fmt::Display for OpenDiffByLocationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("open diff by locations")
    }
}

impl Effect for OpenDiffByLocationsEffect {
    type Result = AppCommand;
}


pub fn open_by_location_effect(
    window: ::workbench::window::WindowId,
    documents: imba::store::Id<documents::OpenDocuments>,
    location: ResourceLocation,
    primary: bool,
    focus: bool,
    target: Option<std::ops::Range<documents::text_ext::LineCol>>,
) -> crate::effects::AppEffect {
    AnyEffect::new(OpenByLocationEffect {
        window,
        documents,
        location,
        primary,
        target,
        focus,
    })
}

pub fn open_locations(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    window: ::workbench::window::WindowId,
    locations: &[ResourceLocation],
    fx: &mut AppFx<'_>,
) {
    let folders = ::workbench::window::Windows::window_ref(store, window)
        .map(|entity| ahp_session::session::folders::session_folders(store, &crate::workspace::entity_session(&entity)))
        .unwrap_or_default();
    let locations: Vec<ResourceLocation> = locations
        .iter()
        .map(|location| {
            if location.authority().as_str() != "local" {
                return location.clone();
            }
            match folders
                .iter()
                .find(|folder| location.path().starts_with(folder.path()))
            {
                Some(folder) => ResourceLocation::new(
                    location.kind().clone(),
                    folder.authority().clone(),
                    location.path().to_vec(),
                ),
                None => location.clone(),
            }
        })
        .collect();
    let mut primary = true;
    let Some(documents) =
        crate::workspace::session_state(store, window).map(|state| state.documents())
    else {
        return;
    };
    for location in &locations {
        if let Some(document) = documents::OpenDocuments::by_location(store, documents, location) {
            if primary {
                if let Some(mut window_entity) = ::workbench::window::Windows::window(store, window) {
                    fx.scope(AppCommand::Verb, |fx| {
                        window_entity.show_document(store, ui, window, document, None, false, fx);
                    });
                    ::workbench::window::Windows::put(store, window, window_entity);
                }
            }
            primary = false;
            continue;
        }
        fx.push(open_by_location_effect(
            window,
            documents,
            location.clone(),
            primary,
            false,
            None,
        ));
        primary = false;
    }
}

pub(crate) struct EditorNavigator;

impl ::workbench::navigation::WindowedNavigator for EditorNavigator {
    type Place = hikit::navigation::EditorPlace;

    fn navigate(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        place: &hikit::navigation::EditorPlace,
        fx: &mut imba::command::Fx<'_>,
    ) -> Option<::workbench::workbench_node::Panel> {
        let documents = crate::workspace::session_state(store, window)
            .expect("navigation runs in a window with a session")
            .documents();
        let Some(id) = documents::OpenDocuments::by_location(store, documents, &place.location) else {
            // The fetch-then-open lands an AppCommand into the window —
            // the shell escape carries it over the Verb lane.
            fx.push(
                open_by_location_effect(
                    window,
                    documents,
                    place.location.clone(),
                    true,
                    false,
                    None,
                )
                .map(crate::app::shell_verb),
            );
            return None;
        };
        let mut document = documents::OpenDocuments::document(store, documents, id)?;
        let width = ::workbench::window::Windows::window_ref(store, window)
            .and_then(|entity| {
                ::workbench::workbench::panel_width(store, entity.workbench().root.focused_pane())
            })
            .unwrap_or_else(|| ::workbench::workbench::fallback_pane_editor_width(store));
        let editor = fx.scope(
            move |command| {
                imba::command::Verb::at(documents, documents::DocumentsCommand::Editor(id, command))
            },
            |fx| {
                let editor = documents::lifecycle::mount_editor(store, ui, &mut document, width, None, fx);

                if place.caret > 0 {
                    let fonts = ::editor::env::Fonts::of(store)();
                    let theme = ::editor::env::Themes::of(store);
                    document.reveal_at_instant(editor, place.caret, store, ui, &fonts, &theme, fx);
                }
                editor
            },
        );
        documents::scroll_stripes::enable_scroll_stripes(
            store,
            documents,
            id,
            &mut document,
            editor,
        );
        documents::OpenDocuments::put_document(store, documents, id, document);
        documents::OpenDocuments::touch(store, documents, id);
        let mut pane = imba::scroll::ScrollView::new(
            documents::entity_view::EditorIdView::new(documents, id, editor).with_gutter(),
        );
        pane.set_scroll_y(place.scroll_y);
        Some(::workbench::workbench_node::Panel::Editor(pane))
    }
}

/// --- The SESSION AS WORKSPACE ---------------------------------------
/// What a window shows is a session: here the workbench's opaque
/// `Workspace` learns to say so. The bundle pairs the session's NAME
/// (the `SessionId` — rekeys move it) with its id bundle (the
/// `SessionState` — wired at session entry, stable for the session's
/// life). Identity is the name: two workspaces are the same when
/// their sessions match.

use std::any::Any;

use ahp_session::session::state::SessionState;
use ahp_wire::SessionId;
use workbench::window::{Window, WindowId, Windows, Workspace};

pub struct SessionWorkspace {
    pub session: SessionId,
    pub state: SessionState,
}

impl SessionWorkspace {
    /// The one constructor every window boards through.
    pub fn boxed(session: SessionId, state: SessionState) -> Box<dyn Workspace> {
        Box::new(SessionWorkspace { session, state })
    }
}

/// The local-host rekey payload (`Window::adopt_workspaces`).
pub struct AdoptLocalHost(pub ahp_wire::client::HostId);

impl Workspace for SessionWorkspace {
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

    fn same(&self, other: &dyn Workspace) -> bool {
        other
            .as_any()
            .downcast_ref::<SessionWorkspace>()
            .is_some_and(|other| other.session == self.session)
    }

    fn clone_workspace(&self) -> Box<dyn Workspace> {
        Box::new(SessionWorkspace {
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
        .workspace()
        .as_any()
        .downcast_ref::<SessionWorkspace>()
        .expect("every himark window grips a session")
        .session
        .clone()
}

/// The id bundle of the session a window shows — read from the
/// window's OWN record (docs/entities.md law 3): the bundle is wired
/// at session entry, so no catalog consult and no ambient scope.
pub fn entity_state(entity: &Window) -> SessionState {
    entity
        .workspace()
        .as_any()
        .downcast_ref::<SessionWorkspace>()
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
            window.workspaces().any(|workspace| {
                workspace.as_any()
                    .downcast_ref::<SessionWorkspace>()
                    .is_some_and(|workspace| workspace.session == *session)
            })
        })
    })
}
