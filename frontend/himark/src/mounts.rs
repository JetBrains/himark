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

/// Which SESSION each mounted bridge belongs to — the windowless
/// twin of "the window's session": the navigate resolver reads it
/// to bind a mounted origin's opens to the right collections.
#[derive(Clone, Default)]
pub(crate) struct MountSessions(pub(crate) rpds::HashTrieMapSync<u64, ahp_wire::SessionId>);

pub(crate) fn remember_session(store: &mut Store, id: ViewBridgeId, session: ahp_wire::SessionId) {
    let mut map = store.get::<MountSessions>().cloned().unwrap_or_default();
    map.0.insert_mut(id.raw(), session);
    store.put(map);
}

pub(crate) fn session_of(store: &Store, id: ViewBridgeId) -> Option<ahp_wire::SessionId> {
    store.get::<MountSessions>()?.0.get(&id.raw()).cloned()
}

pub(crate) fn forget_session(store: &mut Store, id: ViewBridgeId) {
    let Some(mut map) = store.get::<MountSessions>().cloned() else {
        return;
    };
    map.0.remove_mut(&id.raw());
    store.put(map);
}

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
        let content = self.panel.layout_dyn(arena, store, ui, constraints);
        // The CONTENT's size, not the ceiling: under the native
        // scroller's loose measure the ceiling is infinite, and
        // the content's own height IS the answer. Tight
        // constraints land on the same size either way.
        let size = imba::Thunk::size(&content);
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
        let content =
            imba::layout::Layout::layout(self.view.display(arena, store, ui), arena, constraints);
        // The CONTENT's size, not the ceiling — under the native
        // scroller's loose measure the ceiling is infinite, and an
        // infinite container poisons the slot's size: the measure
        // never reaches the scroller and hit-testing dies against
        // an infinite rect. Tight constraints land the same.
        let size = imba::Thunk::size(&content);
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

/// An EXTERNAL place arm: a crate ABOVE this one (a plugin)
/// answers a place with a modal view. Registered at boot
/// (`Himark::register_place_arm`); `mount_place` consults these
/// after its own arms.
pub type ModalPlaceArm = std::sync::Arc<
    dyn Fn(
            &mut Store,
            &UiCtx,
            ViewBridgeId,
            &mut AppFx<'_>,
            &hikit::navigation::NavigationLocation,
        ) -> Option<Box<dyn hikit::modal::ModalView>>
        + Send
        + Sync,
>;

/// The dyn twin of `ModalMount`, for externally-answered places.
pub(crate) struct DynModalMount {
    id: ViewBridgeId,
    view: Box<dyn hikit::modal::ModalView>,
}

impl BridgeView for DynModalMount {
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
            self.view
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
        let content = self.view.layout_dyn(arena, store, ui, constraints);
        let size = imba::Thunk::size(&content);
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
            |fx| self.view.perform_dyn(store, ui, command, fx),
        );
        while let Some(request) = hikit::modal::ModalView::take_request(&mut *self.view) {
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
        PanelRequest::OpenLocations(locations) => navigate_origin(
            store,
            ui,
            origin,
            crate::places::LocationsPlace(locations),
            fx,
        ),
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
        PanelRequest::Perform(command) => imba::command::Verb::Dynamic(command).run(store, ui, fx),
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
        ModalRequest::OpenLocations(locations) => navigate_origin(
            store,
            ui,
            origin,
            crate::places::LocationsPlace(locations),
            fx,
        ),
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
            remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let chat = ahp_session::session::agents::Agents::channel(store, &key)?.default_chat?;
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
    if let Some(place) = place.place::<crate::places::ChatTranscriptPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let chat = ahp_session::session::agents::Agents::channel(store, &key)?.default_chat?;
            ahp_chat::chats::Chats::ensure_chat(
                store,
                ui,
                state.chats(),
                key.host,
                key.session.clone(),
                chat.clone(),
                None,
            );
            Some(Rc::new(PanelMount {
                id,
                panel: Box::new(ahp_chat::transcript::TranscriptPane::new(
                    state.chats(),
                    chat,
                )),
            }) as Rc<dyn BridgeView>)
        });
    }
    if let Some(place) = place.place::<crate::places::ChatComposerPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let chat = ahp_session::session::agents::Agents::channel(store, &key)?.default_chat?;
            ahp_chat::chats::Chats::ensure_chat(
                store,
                ui,
                state.chats(),
                key.host,
                key.session.clone(),
                chat.clone(),
                None,
            );
            Some(Rc::new(PanelMount {
                id,
                panel: Box::new(ahp_chat::composer::ComposerPane::new(state.chats(), chat)),
            }) as Rc<dyn BridgeView>)
        });
    }
    if let Some(place) = place.place::<crate::places::ChangesPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let changes = state.changes();
            // A row tap opens THAT ROW'S DIFF — no canvas on the
            // mounted road (the desktop's multi-diff canvas stays a
            // workbench face); the shell pushes one split-diff.
            let row_key = key.clone();
            let opener: changesview::changes_view::CanvasOpener =
                Arc::new(move |source, location| {
                    imba::command::Verb::once(OpenDiffFromChanges {
                        origin: id,
                        session: row_key.clone(),
                        changes,
                        source,
                        reveal: location,
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
    if let Some(place) = place.place::<crate::places::ChangesCanvasPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let changes = state.changes();
            // The DESKTOP flavor: a row opens the MULTI-DIFF CANVAS
            // (user law — iOS keeps one inline diff per row). The
            // windowed verb forks windowless at the missing entity
            // and hands the shell a CanvasPlace.
            let opener: changesview::changes_view::CanvasOpener =
                Arc::new(move |source, reveal| {
                    crate::app::shell_verb(crate::shell::windowed(
                        ::workbench::window::WindowId::from_raw(id.raw()),
                        Arc::new(crate::diff_canvas::OpenDiffCanvas {
                            changes,
                            source,
                            reveal,
                        }),
                    ))
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
    if let Some(place) = place.place::<crate::places::HistoryPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let changes = state.changes();
            let opener: changesview::changes_view::CanvasOpener =
                Arc::new(move |source, reveal| {
                    crate::app::shell_verb(crate::shell::windowed(
                        ::workbench::window::WindowId::from_raw(id.raw()),
                        Arc::new(crate::diff_canvas::OpenDiffCanvas {
                            changes,
                            source,
                            reveal,
                        }),
                    ))
                });
            let view = changesview::changes_view::ChangesView::open(
                store,
                ui,
                changes,
                changesview::changes_view::ViewSets::History,
                opener,
            );
            let view = changesview::hichanges::Changes::mint_view(store, changes, view);
            Some(Rc::new(ModalMount {
                id,
                view: changesview::changes_view::ChangesPane::new(changes, view),
            }) as Rc<dyn BridgeView>)
        });
    }
    if let Some(place) = place.place::<crate::places::SearchPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let lists = state.lists();
            let folders = ahp_session::session::folders::session_folders(store, &key);
            ::locations::LocationLists::adopt_folders(store, lists, &folders);
            let view = ::locations::search::SearchView::open(store, ui, lists);
            Some(Rc::new(ModalMount { id, view }) as Rc<dyn BridgeView>)
        });
    }
    if let Some(place) = place.place::<crate::places::CommentsPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, fx| {
            remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let comments = state.comments();
            let wire = state.comments_wire();
            let folders = ahp_session::session::folders::session_folders(store, &key);
            fx.scope(crate::perform::AppCommand::Verb, |fx| {
                for folder in folders {
                    ahp_comments::ensure(store, wire, &folder, fx);
                }
            });
            let view = ::comments::panel::CommentsView::open(store, ui, comments);
            Some(Rc::new(ModalMount { id, view }) as Rc<dyn BridgeView>)
        });
    }
    if let Some(place) = place.place::<::canvas::diff_canvas::CanvasPlace>() {
        let place = place.clone();
        return app.mount(move |store, ui, id, fx| {
            // The canvas mints through ITS navigator — the same
            // road a workbench pane rides.
            let panel = fx.scope(crate::perform::AppCommand::Verb, |fx| {
                hikit::navigation::Navigator::navigate(
                    &::canvas::canvas::CanvasNavigator,
                    store,
                    ui,
                    &place,
                    fx,
                )
            })?;
            Some(Rc::new(PanelMount { id, panel }) as Rc<dyn BridgeView>)
        });
    }
    if let Some(place) = place.place::<crate::places::FilesPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, fx| {
            remember_session(store, id, key.clone());
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
    if let Some(place) = place.place::<crate::places::OpenDocumentPlace>() {
        let document = place.document;
        return app.mount(move |store, ui, id, fx| {
            // Mounted places carry no window to ask for a session: a
            // registered document lives in exactly one collection —
            // find it across the (few) session states.
            let collection = ahp_session::session::state::Hosts::states(store)
                .into_iter()
                .map(|state| state.documents())
                .collect::<Vec<_>>()
                .into_iter()
                .find(|collection| {
                    ::documents::OpenDocuments::document(store, *collection, document).is_some()
                })?;
            let mut held = ::documents::OpenDocuments::document(store, collection, document)?;
            let width = ::workbench::workbench::fallback_pane_editor_width(store);
            let editor = fx.scope(
                move |command: ::editor::editor_view::EditorCommand| {
                    AppCommand::Verb(imba::command::Verb::at(
                        collection,
                        ::documents::DocumentsCommand::Editor(document, command),
                    ))
                },
                |fx| ::documents::lifecycle::mount_editor(store, ui, &mut held, width, None, fx),
            );
            ::documents::scroll_stripes::enable_scroll_stripes(
                store, collection, document, &mut held, editor,
            );
            ::documents::OpenDocuments::put_document(store, collection, document, held);
            ::documents::OpenDocuments::touch(store, collection, document);
            // BARE — no imba scroller: the editor screen rides the
            // stage road, and the native scroller owns the physics
            // (docs/ui/imba-ffi.md, touch section).
            let view = ::documents::entity_view::EditorIdView::new(collection, document, editor)
                .with_gutter();
            Some(Rc::new(EditorMount { id, view }) as Rc<dyn BridgeView>)
        });
    }
    if let Some(place) = place.place::<crate::places::DiffViewPlace>() {
        let documents = place.documents;
        let view = place.view;
        return app.mount(move |store, ui, id, fx| {
            let mut panel = ::canvas::diff_pane::DiffPanelView::over(documents, view);
            // One file, one column: the phone shows diffs INLINE —
            // the pane's own layout road, persisted in the view
            // state (a no-op when already inline).
            fx.scope(
                move |command: ::editor::unified_diff::UnifiedDiffCommand| {
                    AppCommand::ToView(id, DynCommand::new(command))
                },
                |fx| {
                    imba::View::perform(
                        &mut panel,
                        store,
                        ui,
                        ::editor::unified_diff::UnifiedDiffCommand::SetLayout(
                            ::editor::unified_diff::DiffLayout::Inline,
                        ),
                        fx,
                    )
                },
            );
            Some(Rc::new(PanelMount {
                id,
                panel: Box::new(panel),
            }) as Rc<dyn BridgeView>)
        });
    }
    // The EXTERNAL arms — plugins registered above this crate.
    let arms = app.modal_place_arms.clone();
    for arm in arms {
        let place = place.clone();
        let minted = app.mount(move |store, ui, id, fx| {
            let view = arm(store, ui, id, fx, &place)?;
            Some(Rc::new(DynModalMount { id, view }) as Rc<dyn BridgeView>)
        });
        if minted.is_some() {
            return minted;
        }
    }
    eprintln!("[mount] no view answers this place — refused");
    None
}

/// A single editor over a registered document, as a mounted view —
/// the pushed "file" screen. The view is BARE: the host's scroller
/// owns the viewport (the stage road); the document and its
/// editors live in the collection, exactly as a workbench editor
/// pane holds them.
pub(crate) struct EditorMount {
    id: ViewBridgeId,
    view: ::documents::entity_view::EditorIdView,
}

impl BridgeView for EditorMount {
    fn frame_store(&self, store: &Store, _ui: &UiCtx) -> Store {
        store.clone()
    }

    fn focus<'a>(
        &'a self,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> Option<imba::focus::FocusData<'a, AppCommand>> {
        let id = self.id;
        // `focus_data_dyn` ERASES already — wrapping its DynCommand
        // in another DynCommand breaks the perform downcast.
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
        let content = imba::layout::Layout::layout(
            imba::View::display(&self.view, arena, store, ui),
            arena,
            constraints,
        );
        // The container takes the CONTENT's size, not the ceiling:
        // under the native scroller's loose measure the ceiling is
        // infinite, and the content's own height IS the answer.
        let size = imba::Thunk::size(&content);
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
        _origin: ViewBridgeId,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut AppFx<'_>,
    ) {
        type PaneCommand = ::editor::editor_view::EditorCommand;
        let Some(command) = command.downcast::<PaneCommand>() else {
            eprintln!("[mount] a foreign command was dropped by the editor mount");
            return;
        };
        let origin = self.id;
        fx.scope(
            move |command: PaneCommand| AppCommand::ToView(origin, DynCommand::new(command)),
            |fx| imba::View::perform(&mut self.view, store, ui, command, fx),
        );
    }
}

/// The changes tree asked to open a row: find THAT ROW'S pair in
/// the set's listing (the same normalization the canvas rows use)
/// and launch the pair-open with the MOUNTED origin — the landing
/// installs the view and hands the shell a ready `DiffViewPlace`.
struct OpenDiffFromChanges {
    origin: ViewBridgeId,
    session: ahp_wire::SessionId,
    changes: imba::store::Id<changesview::hichanges::ChangeSets>,
    source: changesview::hichanges::CanvasSource,
    reveal: Option<editor::location::ResourceLocation>,
}

impl imba::command::DynamicOnceCommand for OpenDiffFromChanges {
    fn perform(self: Box<Self>, store: &mut Store, _ui: &UiCtx, fx: &mut imba::command::Fx<'_>) {
        let Some(reveal) = self.reveal else {
            eprintln!("[mount] changes open: no reveal (a folder row?)");
            return;
        };
        let (_, listing) = ::canvas::diff_canvas::canvas_files(store, self.changes, &self.source);
        let ::canvas::diff_canvas::CanvasListing::Ready(files) = listing else {
            eprintln!("[mount] changes open: listing not ready");
            return;
        };
        let Some(file) = files.into_iter().find(|file| file.new == reveal) else {
            eprintln!("[mount] a changes row vanished before its diff opened — dropped");
            return;
        };
        let documents =
            ahp_session::session::state::Hosts::ensure_state(store, &self.session).documents();
        let old = documents::diff_views::DiffSideInput::resolve(store, documents, file.old);
        let new = documents::diff_views::DiffSideInput::resolve(store, documents, file.new);
        fx.push(
            imba::effect::AnyEffect::new(crate::workspace::OpenDiffByLocationsEffect {
                window: ::workbench::window::WindowId::from_raw(self.origin.raw()),
                documents,
                old,
                new,
            })
            .map(|command| imba::command::Verb::shell(Box::new(command))),
        );
    }
}
