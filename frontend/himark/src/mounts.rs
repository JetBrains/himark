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
                // No window impersonation: WINDOW overlays BUBBLE
                // to the frame root, the bridge records them, the
                // shell promotes them to popovers.
                .map(move |command| AppCommand::ToView(id, command)),
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
                // No window impersonation: WINDOW overlays BUBBLE
                // to the frame root, the bridge records them, the
                // shell promotes them to popovers.
                .map(move |command| AppCommand::ToView(id, DynCommand::new(command))),
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

/// The palette's view, built by the plugin that owns it from the
/// commands the engine walked (`Himark::session_commands`).
pub type PaletteFactory = std::sync::Arc<
    dyn Fn(
            &mut Store,
            &UiCtx,
            Vec<imba::PresentableCommand<crate::app::AppCommand>>,
        ) -> Box<dyn hikit::modal::ModalView>
        + Send
        + Sync,
>;

/// An EXTERNAL BRIDGE arm: a plugin answers a place with a mounted
/// view of ANY shape — it mints through `Himark::mount` itself
/// (the AHP plugin's whole session vocabulary registers here).
/// Registered at boot (`Himark::register_bridge_arm`);
/// `mount_place` consults these before the modal arms.
pub type BridgePlaceArm = std::sync::Arc<
    dyn Fn(
        &mut crate::himark::Himark,
        &hikit::navigation::NavigationLocation,
    ) -> Option<ViewBridgeId>,
>;

/// An erased PANEL as a mounted view — the wrapper a bridge arm
/// answers with for panel-shaped content (the chat pane).
pub fn panel_mount(id: ViewBridgeId, panel: Box<dyn hikit::panel::DynPanelView>) -> Arc<dyn BridgeView> {
    Arc::new(PanelMount { id, panel })
}

/// A typed dock view (`View` + `ModalView`) as a mounted view.
pub fn modal_mount<V>(id: ViewBridgeId, view: V) -> Arc<dyn BridgeView>
where
    V: imba::View + hikit::modal::ModalView + 'static,
{
    Arc::new(ModalMount { id, view })
}

/// An erased MODAL as a mounted view (the palette's factory
/// answers one).
pub fn dyn_modal_mount(
    id: ViewBridgeId,
    view: Box<dyn hikit::modal::ModalView>,
) -> Arc<dyn BridgeView> {
    Arc::new(DynModalMount { id, view })
}

/// A BARE single-document editor as a mounted view — the stage
/// road's pushed "file" screen (the host's scroller owns the
/// viewport).
pub fn editor_mount(
    id: ViewBridgeId,
    view: ::documents::entity_view::EditorIdView,
) -> Arc<dyn BridgeView> {
    Arc::new(EditorMount { id, view })
}

/// The POPOVER's bridge: re-derives the parent's frame in its own
/// arena and stands overlay #index's content alone. The parent's
/// pixels are a byproduct, discarded; the content's commands were
/// mapped into the parent's `ToView` space by the parent's own
/// display, so events from the popover land exactly where the
/// composited overlay's would have. Its OWN bubbled overlays
/// export like any bridge's — recursive popovers are normal.
pub(crate) struct OverlayMount {
    /// The strong pin the standing frame's borrows ride on. No
    /// Weak lives here — `Rc::get_mut` refuses a view with ANY
    /// weak holder, so even an unpinned Weak would block every
    /// perform on the parent. The ENGINE refills the pin (both
    /// layout roads) right before laying this view; the ToView
    /// release road empties it AFTER the frames quiesce — then
    /// the parent performs exclusively.
    pub(crate) pinned: std::sync::OnceLock<Arc<dyn BridgeView>>,
    /// The parent's standing BAND — the viewport its own stage
    /// road last realized. The re-derivation realizes exactly
    /// that slice: realizing the parent's whole content rect made
    /// every popover layout, IME ask and clock tick walk a 10k-line
    /// document end to end (popup-causes-freezes.trace).
    pub(crate) band: std::sync::Mutex<Option<gfx::Rect>>,
    /// The overlay's stable identity: the host it was minted FOR
    /// and which of that host's overlays it stands (the exported
    /// set reorders as views mount and scroll; a positional index
    /// would swap this popover's content under the shell).
    pub(crate) host: imba::overlay::OverlayHost,
    pub(crate) within: usize,
    /// The union of the overlay's NON-host-covering pieces, in the
    /// parent's coordinates — the slice the shell's popover shows.
    /// Stashed at layout; read through the content-rect door.
    pub(crate) content: std::sync::Mutex<Option<gfx::Rect>>,
}

impl OverlayMount {
    fn parent_ref(&self) -> Option<&Arc<dyn BridgeView>> {
        self.pinned.get()
    }

    /// Refill the pin before a layout road runs the popover — the
    /// engine calls this concretely (`as_any`), never through the
    /// `BridgeView` vocabulary.
    pub(crate) fn pin_parent(&self, parent: Arc<dyn BridgeView>, band: Option<gfx::Rect>) {
        let _ = self.pinned.set(parent);
        *self.band.lock().unwrap() = band;
    }

    /// Drop the strong pin (after the popover's frames quiesced),
    /// so the parent can be taken mutably. `&mut` on purpose: the
    /// exclusivity is the proof no standing borrow rides the pin.
    pub(crate) fn release_parent(&mut self) {
        self.pinned.take();
    }

    /// The overlay's content slice, as the last layout stashed it.
    pub(crate) fn content_rect(&self) -> Option<gfx::Rect> {
        *self.content.lock().unwrap()
    }
}

impl BridgeView for OverlayMount {
    fn frame_store(&self, store: &Store, ui: &UiCtx) -> Store {
        match self.parent_ref() {
            Some(parent) => parent.frame_store(store, ui),
            None => store.clone(),
        }
    }

    /// Keystrokes landing on the popover walk the PARENT'S chain —
    /// the combo's Escape and the menu's search live there, and
    /// the chain is a state walk, identical from either window.
    fn focus<'a>(
        &'a self,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> Option<imba::focus::FocusData<'a, AppCommand>> {
        self.parent_ref()?.focus(store, ui)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn layout<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: imba::constraints::Constraints,
    ) -> imba::ThunkBox<'a, AppCommand> {
        use imba::Thunk as _;
        let Some(parent) = self.parent_ref() else {
            // The parent unmounted under the popover — stand
            // empty; the shell closes on the next epoch.
            *self.content.lock().unwrap() = None;
            return imba::ThunkBox::new(arena, imba::leaf::leaf::<AppCommand>(1.0, 1.0));
        };
        let inner = parent.layout(arena, store, ui, constraints);
        let host_size = inner.size();
        let band = self
            .band
            .lock()
            .unwrap()
            .unwrap_or_else(|| gfx::Rect::from_size(host_size));
        let mut stacked = {
            let mut base = imba::container::Container::new(arena, host_size);
            base.place(0.0, 0.0, inner);
            base.realize_into(band)
        };
        // The stable identity pick: the nth overlay OF THIS HOST,
        // however the rest of the exported set reordered.
        let picked = imba::Widget::overlays(&mut stacked)
            .into_iter()
            .filter(|overlay| overlay.host == self.host)
            .nth(self.within);
        let Some(overlay) = picked else {
            // The overlay left between frames — the shell's next
            // reconciliation closes this popover; stand empty.
            *self.content.lock().unwrap() = None;
            return imba::ThunkBox::new(arena, imba::leaf::leaf::<AppCommand>(1.0, 1.0));
        };
        let placed = overlay.content.layout(arena, host_size, overlay.anchor);
        // EVERY piece stays, at its TRUE position — including the
        // host-covering DISMISS SCRIM. This frame shares the
        // parent's coordinate space: the parent forwards outside
        // clicks here untranslated, the scrim turns them into the
        // dismissal the widget wired — shell-app's composited
        // road, verbatim. The shell's popover shows only the
        // CONTENT slice: the union of the non-host-covering
        // pieces, stashed here for the content-rect door.
        let mut content: Option<gfx::Rect> = None;
        for (point, thunk) in &placed {
            let size = thunk.size();
            let host_covering =
                size.width >= host_size.width - 0.5 && size.height >= host_size.height - 0.5;
            if !host_covering {
                let rect = gfx::Rect::from_xywh(point.x, point.y, size.width, size.height);
                content = Some(match content {
                    Some(acc) => acc.union(&rect),
                    None => rect,
                });
            }
        }
        *self.content.lock().unwrap() = content;
        if content.is_none() {
            return imba::ThunkBox::new(arena, imba::leaf::leaf::<AppCommand>(1.0, 1.0));
        }
        let mut full = imba::container::container(arena, host_size);
        for (point, thunk) in placed {
            full.place(point.x, point.y, thunk);
        }
        imba::ThunkBox::new(arena, full)
    }

}

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
                // No window impersonation: WINDOW overlays BUBBLE
                // to the frame root, the bridge records them, the
                // shell promotes them to popovers.
                .map(move |command| AppCommand::ToView(id, command)),
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
        ModalRequest::Close => {
            // The mounted modal's own dismissal — the native face
            // hosting this origin folds away.
            navigate_origin(store, ui, origin, crate::places::DismissPlace, fx)
        }
        ModalRequest::SelectWidget(_) => {}
    });
}

/// Mint the view a place names — the ONE minting door
/// (docs/ui/himark-ffi.md). `None` is an unknown place kind or a
/// session the catalog cannot serve yet.
pub fn mount_place(
    app: &mut crate::himark::Himark,
    place: &hikit::navigation::NavigationLocation,
) -> Option<ViewBridgeId> {
    // The EXTERNAL BRIDGE arms — plugins answering with any mount
    // shape (the AHP plugin's session vocabulary).
    let bridge_arms = app.bridge_place_arms.clone();
    for arm in bridge_arms {
        let minted = arm(app, place);
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
/// The LIBRARY consumer's editor mount: a SCROLLED editor over a
/// registered document — no session, no window, and the mount owns
/// its scroll physics (the place mounts ride the stage road
/// instead, where the host's native scroller owns the viewport).
/// This is the one-call door an embedding shell mounts a document
/// editor through.
pub fn mount_scrolling_editor(
    app: &mut crate::himark::Himark,
    documents: imba::store::Id<::documents::OpenDocuments>,
    document: ::documents::DocumentId,
) -> Option<ViewBridgeId> {
    app.mount(move |store, ui, id, fx| {
        let mut held = ::documents::OpenDocuments::document(store, documents, document)?;
        let width = fallback_editor_width(store);
        let editor = fx.scope(
            move |command: ::editor::editor_view::EditorCommand| {
                AppCommand::Verb(imba::command::Verb::at(
                    documents,
                    ::documents::DocumentsCommand::Editor(document, command),
                ))
            },
            |fx| ::documents::lifecycle::mount_editor(store, ui, &mut held, width, None, fx),
        );
        ::documents::OpenDocuments::put_document(store, documents, document, held);
        ::documents::OpenDocuments::touch(store, documents, document);
        let view = imba::scroll::ScrollView::new(
            ::documents::entity_view::EditorIdView::new(documents, document, editor).with_gutter(),
        );
        Some(Arc::new(ScrolledEditorMount { id, view }) as Arc<dyn BridgeView>)
    })
}

/// A scrolled editor pane as a mounted view — the workbench's
/// `EditorPane` shape (`ScrollView<EditorIdView>`), minted without
/// a window.
struct ScrolledEditorMount {
    id: ViewBridgeId,
    view: imba::scroll::ScrollView<::documents::entity_view::EditorIdView>,
}

type ScrolledEditorCommand = imba::scroll::ScrollCommand<::editor::editor_view::EditorCommand>;

impl BridgeView for ScrolledEditorMount {
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
        // The scroller fills the HOST's constraints — the viewport
        // is the mount's size, the content scrolls inside it (no
        // loose-measure container like the stage-road mounts).
        let content = imba::layout::Layout::layout(
            imba::View::display(&self.view, arena, store, ui),
            arena,
            constraints,
        );
        ThunkBox::new(
            arena,
            content.map(move |command| AppCommand::ToView(id, DynCommand::new(command))),
        )
    }

    fn perform(
        &mut self,
        command: DynCommand,
        _origin: ViewBridgeId,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut AppFx<'_>,
    ) {
        let Some(command) = command.downcast::<ScrolledEditorCommand>() else {
            eprintln!("[mount] a foreign command was dropped by the scrolled editor mount");
            return;
        };
        let origin = self.id;
        fx.scope(
            move |command: ScrolledEditorCommand| {
                AppCommand::ToView(origin, DynCommand::new(command))
            },
            |fx| imba::View::perform(&mut self.view, store, ui, command, fx),
        );
    }
}

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
                // No window impersonation: WINDOW overlays BUBBLE
                // to the frame root, the bridge records them, the
                // shell promotes them to popovers.
                .map(move |command| AppCommand::ToView(id, DynCommand::new(command))),
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

/// The width a mounted editor wraps at before its host measures it
/// — the same theme-derived fallback a workbench pane uses, read
/// from the window chrome part (a THEME fact, no window needed).
pub fn fallback_editor_width(store: &imba::store::Store) -> f32 {
    let theme = ::editor::env::Themes::of(store);
    let window = theme.part(::editor::theme::WindowChrome::KEY);
    let gutter = theme.part(::editor::theme::EditorGutterChrome::KEY).width;
    (window.first_pane_width - gutter).max(window.min_editor_width)
}
