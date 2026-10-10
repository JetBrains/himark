// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Session views mounted as standalone bridges — the library's
//! view-minting half (docs/ui/himark-ffi.md): given a place, a
//! view, with the workbench nowhere in sight. Two adapters cover
//! what exists: `PanelMount` for the erased panels (the chat
//! pane), `ModalMount` for the typed dock views (changes, files).
//! Commands ride `AppCommand::ToView`; the requests a view raises
//! become PLACES handed to the seated shell, stamped with this
//! bridge as the origin.

use std::rc::Rc;
use std::sync::Arc;

use imba::arena::Arena;
use imba::constraints::Constraints;
use imba::dyn_view::DynCommand;
use imba::store::Store;
use imba::thunk_ext::ThunkExt;
use imba::ui::UiCtx;
use imba::ThunkBox;

use crate::bridge::{BridgeView, ViewBridgeId};
use crate::perform::{AppCommand, AppFx};
use crate::shell::navigate_origin;

/// An erased panel (`DynPanelView`) as a mounted view.
pub(crate) struct PanelMount {
    id: ViewBridgeId,
    panel: Box<dyn hikit::panel::DynPanelView>,
}

impl BridgeView for PanelMount {
    fn frame_store(&self, store: &Store, _ui: &UiCtx) -> Store {
        store.clone()
    }

    fn focus<'a>(
        &'a self,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> Option<imba::focus::FocusData<'a, AppCommand>> {
        let id = self.id;
        Some(
            self.panel
                .focus_data_dyn(store, ui)
                .map(move |command| AppCommand::ToView(id, command)),
        )
    }

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> ThunkBox<'a, AppCommand> {
        let id = self.id;
        let size = constraints.max;
        let content = self.panel.layout_dyn(arena, store, ui, constraints);
        let mut container = imba::container::container(arena, size);
        container.place(
            0.0,
            0.0,
            content
                .map(move |command| AppCommand::ToView(id, command))
                .overlay_host(imba::overlay::WINDOW),
        );
        ThunkBox::new(arena, container)
    }

    fn perform(
        &mut self,
        command: DynCommand,
        origin: ViewBridgeId,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut AppFx<'_>,
    ) {
        fx.scope(
            move |command: DynCommand| AppCommand::ToView(origin, command),
            |fx| self.panel.perform_dyn(store, ui, command, fx),
        );
        while let Some(request) = self.panel.take_request() {
            route_panel_request(request, origin, store, ui, fx);
        }
    }
}

/// A typed dock view (`View` + `ModalView`) as a mounted view.
pub(crate) struct ModalMount<V> {
    id: ViewBridgeId,
    view: V,
}

impl<V> BridgeView for ModalMount<V>
where
    V: imba::View + hikit::modal::ModalView + 'static,
{
    fn frame_store(&self, store: &Store, _ui: &UiCtx) -> Store {
        store.clone()
    }

    fn focus<'a>(
        &'a self,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> Option<imba::focus::FocusData<'a, AppCommand>> {
        let id = self.id;
        Some(
            imba::dyn_view::DynView::focus_data_dyn(&self.view, store, ui)
                .map(move |command| AppCommand::ToView(id, command)),
        )
    }

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> ThunkBox<'a, AppCommand> {
        let id = self.id;
        let size = constraints.max;
        let content =
            imba::layout::Layout::layout(self.view.display(arena, store, ui), arena, constraints);
        let mut container = imba::container::container(arena, size);
        container.place(
            0.0,
            0.0,
            content
                .map(move |command| AppCommand::ToView(id, DynCommand::new(command)))
                .overlay_host(imba::overlay::WINDOW),
        );
        ThunkBox::new(arena, container)
    }

    fn perform(
        &mut self,
        command: DynCommand,
        origin: ViewBridgeId,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut AppFx<'_>,
    ) {
        let Some(command) = command.downcast::<V::Command>() else {
            eprintln!("[mount] a command of another view's type was dropped");
            return;
        };
        fx.scope(
            move |command: V::Command| AppCommand::ToView(origin, DynCommand::new(command)),
            |fx| self.view.perform(store, ui, command, fx),
        );
        while let Some(request) = hikit::modal::ModalView::take_request(&mut self.view) {
            route_modal_request(request, origin, store, ui, fx);
        }
    }
}

fn route_panel_request(
    request: hikit::panel::PanelRequest,
    origin: ViewBridgeId,
    store: &mut Store,
    ui: &UiCtx,
    fx: &mut AppFx<'_>,
) {
    use hikit::panel::PanelRequest;
    fx.scope(AppCommand::Verb, |fx| match request {
        PanelRequest::OpenLocations(locations) => {
            navigate_origin(store, ui, origin, crate::places::LocationsPlace(locations), fx)
        }
        PanelRequest::OpenAt(location, target) => navigate_origin(
            store,
            ui,
            origin,
            crate::places::DocumentPlace {
                location,
                target,
                focus: true,
            },
            fx,
        ),
        PanelRequest::OpenDiff(old, new) => {
            navigate_origin(store, ui, origin, crate::places::DiffPlace { old, new }, fx)
        }
        PanelRequest::Perform(command) => {
            imba::command::Verb::Dynamic(command).run(store, ui, fx)
        }
        PanelRequest::Shell(_) => {
            eprintln!("[mount] a window-taking panel ask surfaced windowless — dropped")
        }
    });
}

fn route_modal_request(
    request: hikit::modal::ModalRequest,
    origin: ViewBridgeId,
    store: &mut Store,
    ui: &UiCtx,
    fx: &mut AppFx<'_>,
) {
    use hikit::modal::ModalRequest;
    fx.scope(AppCommand::Verb, |fx| match request {
        ModalRequest::OpenAt {
            location,
            target,
            focus,
        } => navigate_origin(
            store,
            ui,
            origin,
            crate::places::DocumentPlace {
                location,
                target,
                focus,
            },
            fx,
        ),
        ModalRequest::ShowDocument(document) => navigate_origin(
            store,
            ui,
            origin,
            crate::places::OpenDocumentPlace {
                document,
                target: None,
                focus: false,
            },
            fx,
        ),
        ModalRequest::OpenLocations(locations) => {
            navigate_origin(store, ui, origin, crate::places::LocationsPlace(locations), fx)
        }
        ModalRequest::Perform(verb) => match verb {
            imba::command::Verb::Shell(_) => {
                eprintln!("[mount] a window-taking ask surfaced windowless — dropped")
            }
            verb => verb.run(store, ui, fx),
        },
        ModalRequest::Close | ModalRequest::SelectWidget(_) => {}
    });
}

/// Mint the view a place names — the ONE minting door
/// (docs/ui/himark-ffi.md). `None` is an unknown place kind or a
/// session the catalog cannot serve yet.
pub fn mount_place(
    app: &mut crate::himark::Himark,
    place: &hikit::navigation::NavigationLocation,
) -> Option<ViewBridgeId> {
    if let Some(place) = place.place::<crate::places::ChatPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let chat = ahp_session::session::agents::Agents::channel(store, &key)?
                .default_chat?;
            let panel = ahp_chat::chats::Chats::open_with(
                store,
                ui,
                state.chats(),
                key.host,
                key.session.clone(),
                chat,
                None,
            );
            Some(Rc::new(PanelMount { id, panel }) as Rc<dyn BridgeView>)
        });
    }
    if let Some(place) = place.place::<crate::places::ChangesPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let changes = state.changes();
            // The row tap opens the FILE for now; the diff screen
            // takes this seat when the canvas mounts land.
            let opener: changesview::changes_view::CanvasOpener =
                Arc::new(move |_source, location| {
                    imba::command::Verb::once(OpenFromChanges {
                        origin: id,
                        location,
                    })
                });
            let view = changesview::changes_view::ChangesView::open(
                store,
                ui,
                changes,
                changesview::changes_view::ViewSets::WorkingCopies,
                opener,
            );
            let view = changesview::hichanges::Changes::mint_view(store, changes, view);
            Some(Rc::new(ModalMount {
                id,
                view: changesview::changes_view::ChangesPane::new(changes, view),
            }) as Rc<dyn BridgeView>)
        });
    }
    if let Some(place) = place.place::<crate::places::FilesPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, fx| {
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let tree = fx.scope(
                move |command: filetree::TreeCommand| {
                    AppCommand::ToView(id, DynCommand::new(command))
                },
                |fx| {
                    crate::hifiles::open_panel(
                        store,
                        ui,
                        None,
                        key.clone(),
                        state.trees(),
                        None,
                        fx,
                    )
                },
            );
            Some(Rc::new(ModalMount { id, view: tree }) as Rc<dyn BridgeView>)
        });
    }
    eprintln!("[mount] no view answers this place — refused");
    None
}

/// The changes tree asked to open a row.
struct OpenFromChanges {
    origin: ViewBridgeId,
    location: Option<editor::location::ResourceLocation>,
}

impl imba::command::DynamicOnceCommand for OpenFromChanges {
    fn perform(
        self: Box<Self>,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut imba::command::Fx<'_>,
    ) {
        let Some(location) = self.location else {
            return;
        };
        navigate_origin(
            store,
            ui,
            self.origin,
            crate::places::DocumentPlace {
                location,
                target: None,
                focus: true,
            },
            fx,
        );
    }
}
