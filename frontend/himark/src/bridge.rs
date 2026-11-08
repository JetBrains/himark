// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The view bridge: imba's pipeline, mirrored into user space
//! (docs/ui/UI.md, docs/ui/imba-ffi.md). The host drives the same
//! curries the engine's own windows ride —
//!
//! ```text
//! ViewBridge --layout(store, constraints)--> BridgeLayout   (sized)
//! BridgeLayout --realize(viewport)--------> Frame           (materialized)
//! Frame --handle(event)-------------------> commands
//! ```
//!
//! — and each stage is a VALUE the host holds and re-derives when
//! its own inputs move: the store changed → lay out again; the
//! viewport moved → realize again; nothing here caches, keys or
//! invalidates. Caching is a client's composition over the pure
//! stages (`Mount` below is the engine's), never the pipeline's
//! business.

use std::rc::Rc;
use std::sync::Arc;

use gfx::{Rect, Size};
use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult},
    store::Store,
    ui::UiCtx,
    Thunk, ThunkBox, Widget, WidgetBox,
};

use crate::perform::AppCommand;

/// The engine-side identity of one mounted view.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ViewBridgeId(u64);

/// Where MOUNTED ids start: the id spaces must not collide, because
/// a window's bridge shares the window's own id and the workbench
/// tail reaps bridge slots by window liveness — a mounted id below
/// this line would be reaped at the first batch tail.
pub const MOUNT_BASE: u64 = 1 << 32;

impl ViewBridgeId {
    pub fn from_raw(raw: u64) -> ViewBridgeId {
        ViewBridgeId(raw)
    }

    pub fn raw(self) -> u64 {
        self.0
    }

    /// A host-minted mount, as opposed to a window's bridge.
    pub fn is_mounted(self) -> bool {
        self.0 >= MOUNT_BASE
    }
}

/// The retained view: imba's display/layout curries, erased the
/// way `DynView` erases them. This trait IS `imba::dyn_view::
/// DynView`'s layout half with the app's concrete command type; it
/// collapses into `DynView` itself once commands erase too
/// (docs/himark-core.md, commands section).
pub trait BridgeView: Send + Sync {
    /// The snapshot a frame realizes against. A view that stamps
    /// frame-scoped environment does it here — the workbench
    /// stamps its window's focused seat (`FrameFocus`), so editors
    /// derive their selections-visible bit from STATE at build
    /// time.
    fn frame_store(&self, store: &Store, ui: &UiCtx) -> Store;

    /// Read the state, name the structure — the only stage with
    /// the store in scope (docs/ui/UI.md). Borrows from the view
    /// and the store ride the returned thunk for the frame.
    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> ThunkBox<'a, AppCommand>;

    /// The view's semantic focus chain — a STATE walk, no tree
    /// (docs/ui/UI.md): who holds the keyboard inside this
    /// mounted view, and the handlers a keystroke runs. `None` is
    /// a view with no focusable state.
    fn focus<'a>(
        &'a self,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> Option<imba::focus::FocusData<'a, AppCommand>> {
        let _ = (store, ui);
        None
    }

    /// Route one command to the retained view (`AppCommand::
    /// ToView`). The default is the store-routed view's answer:
    /// nothing should address it here (`WorkbenchView` routes over
    /// `At` instead), so a delivery is a wiring bug, dropped
    /// loudly.
    fn perform(
        &mut self,
        command: imba::dyn_view::DynCommand,
        origin: ViewBridgeId,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut crate::perform::AppFx<'_>,
    ) {
        let _ = (origin, store, ui, fx);
        eprintln!("[bridge] a view command reached a view with no perform — dropped: {command}");
    }

    /// ENGINE PLUMBING, not vocabulary: the engine reaches the
    /// mounts it minted itself (the popover mount) concretely
    /// through these. A library view leaves them `None`.
    #[doc(hidden)]
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }

    #[doc(hidden)]
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }

    /// Whether the engine clears the canvas to TRANSPARENT before
    /// painting this view — a mounted island composes over the
    /// host's backdrop; a window root paints its own ground and
    /// keeps the surface opaque.
    fn clears_backdrop(&self) -> bool {
        true
    }
}

/// One mounted view: the state carrier, retained. Everything else
/// is a stage derived from it.
pub struct ViewBridge {
    view: Arc<dyn BridgeView>,
}

impl ViewBridge {
    pub(crate) fn new(view: Arc<dyn BridgeView>) -> ViewBridge {
        ViewBridge { view }
    }

    /// The retained view, shared — the focus walk's read door.
    pub(crate) fn view(&self) -> &dyn BridgeView {
        self.view.as_ref()
    }

    /// The retained view by handle — the popover mint holds the
    /// parent alive through it.
    pub(crate) fn view_rc(&self) -> Arc<dyn BridgeView> {
        Arc::clone(&self.view)
    }

    /// The retained view, mutably — available only while NOTHING
    /// borrows it: no standing frame, no stage out with a host.
    /// The `ToView` road quiesces first, so `None` here is a
    /// foreign stage still alive (the caller drops the command
    /// loudly rather than mutate under it).
    pub(crate) fn view_mut(&mut self) -> Option<&mut (dyn BridgeView + 'static)> {
        Arc::get_mut(&mut self.view)
    }

    /// Strong holders of the view right now — the borrow guard's
    /// diagnostic twin.
    pub(crate) fn view_refs(&self) -> usize {
        Arc::strong_count(&self.view)
    }

    /// display → layout: cut the view's frame snapshot, read the
    /// state, name the structure, size it. The returned stage is
    /// self-contained — the caller hands the arena in (warm from a
    /// recycled frame, or fresh) and gets it back at `recycle`.
    pub(crate) fn layout(
        &self,
        store: &Store,
        ui: &Rc<UiCtx>,
        constraints: Constraints,
        arena: Box<Arena>,
    ) -> BridgeLayout {
        let store = Box::new(self.view.frame_store(store, ui.as_ref()));
        let ui = Rc::clone(ui);
        let view = Arc::clone(&self.view);
        // SAFETY: the erased-to-'static borrows below point into
        // the box and Rc contents moved into the stage right after
        // — heap addresses stable, owners live in the same struct,
        // declared to drop AFTER the thunk. See `BridgeLayout`'s
        // invariants.
        let thunk = {
            let arena_ref: &'static Arena = unsafe { &*(arena.as_ref() as *const Arena) };
            let store_ref: &'static Store = unsafe { &*(store.as_ref() as *const Store) };
            let ui_ref: &'static UiCtx = unsafe { &*(Rc::as_ptr(&ui)) };
            let view_ref: &'static dyn BridgeView = unsafe { &*Arc::as_ptr(&view) };
            view_ref.layout(arena_ref, store_ref, ui_ref, constraints)
        };
        BridgeLayout {
            thunk,
            arena,
            store,
            ui,
            view,
        }
    }
}

/// The SIZED stage: display and layout have run, nothing is
/// realized. `size` is the measure answer — the native scroller's
/// question — and `realize` cuts the viewport's slice.
///
/// SAFETY: `thunk`'s `'static` is a lie, erased in
/// `ViewBridge::layout`. What makes it sound:
/// - the thunk borrows only `arena`, `store`, `ui` and `view`
///   below — boxes and `Rc`s whose heap addresses are stable,
///   moved but never replaced while the thunk lives;
/// - the thunk dies first: fields drop in declaration order, and
///   `realize` consumes in the same order;
/// - the arena is never reset and the store never mutated under a
///   live thunk;
/// - no `'static` borrow escapes: every accessor reborrows at the
///   caller's own lifetime.
pub struct BridgeLayout {
    thunk: ThunkBox<'static, AppCommand>,
    arena: Box<Arena>,
    store: Box<Store>,
    ui: Rc<UiCtx>,
    view: Arc<dyn BridgeView>,
}

impl BridgeLayout {
    /// The content size at the laid-out constraints — a READ on
    /// this stage, nothing derived, nothing mutated.
    pub fn size(&self) -> Size {
        self.thunk.size()
    }

    /// layout → realize: materialize the viewport's slice. The
    /// stage is consumed — a new viewport is a new realize off a
    /// fresh layout, exactly as inside the engine.
    pub fn realize(self, viewport: Rect) -> Frame {
        let BridgeLayout {
            thunk,
            arena,
            store,
            ui,
            view,
        } = self;
        // SAFETY: same owners, same invariants — the tree replaces
        // the thunk as the one borrower and moves into `Frame`
        // with the same fields in the same order.
        let tree = {
            let arena_ref: &'static Arena = unsafe { &*(arena.as_ref() as *const Arena) };
            Thunk::realize(thunk, arena_ref, viewport)
        };
        Frame {
            tree,
            arena,
            store,
            ui,
            view,
        }
    }
}

/// The REALIZED stage: one widget tree, routing events between
/// store batches. See `BridgeLayout`'s invariants — they carry
/// over whole.
pub struct Frame {
    tree: WidgetBox<'static, AppCommand>,
    arena: Box<Arena>,
    store: Box<Store>,
    ui: Rc<UiCtx>,
    #[allow(dead_code)] // the tree may borrow the view; kept alive here
    view: Arc<dyn BridgeView>,
}

impl Frame {
    pub fn handle(&self, event: &Event<'_>, viewport: Rect) -> EventResult<AppCommand> {
        self.tree.handle_event(&self.arena, event, viewport)
    }

    /// Drain the overlays that bubbled to this frame's root uncaught
    /// — on the mounted road nobody plays the window or the top
    /// scroller, and handling them is the BRIDGE's job: their
    /// anchors become the shell's layers, their contents re-derived
    /// by each layer's own bridge. EVERY uncaught overlay rides out
    /// — the shell plays whatever host didn't stand — and each
    /// carries its own `Attachment`, so this layer knows no host
    /// names. Draining consumes; once per frame; bubble order is
    /// the index contract with `OverlayMount`.
    pub(crate) fn exported_overlays(
        &mut self,
    ) -> Vec<(imba::overlay::Attachment, imba::overlay::OverlayHost, Rect)> {
        self.tree
            .overlays()
            .into_iter()
            .map(|overlay| (overlay.attachment, overlay.host, overlay.anchor))
            .collect()
    }

    pub(crate) fn layout_data(
        &mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'_, AppCommand> {
        self.tree.layout_data(target)
    }

    /// The store snapshot the tree was built against.
    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    /// Tear down in the safe order and hand the arena back warm.
    pub fn recycle(self) -> Box<Arena> {
        let Frame {
            tree,
            mut arena,
            store,
            ui,
            view,
        } = self;
        drop(tree);
        drop(store);
        drop(ui);
        drop(view);
        arena.reset();
        arena
    }
}

/// Quiesce one mounted view: drop its standing frame and anything
/// staged for the C shell, recycling what can be recycled — after
/// this, the view has exactly one owner and may be mutated
/// (`ToView`). The shell never holds staged objects across a call,
/// so there is nothing of its to invalidate.
pub(crate) fn quiesce(slot: &mut crate::himark::Mounted) {
    if let Some(frame) = slot.frame.take() {
        slot.spare_arena = Some(frame.recycle());
    }
    slot.staged_layout = None;
    if let Some(frame) = slot.staged_frame.take() {
        slot.spare_arena = Some(frame.recycle());
    }
}
