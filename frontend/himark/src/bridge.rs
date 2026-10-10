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

use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult},
    store::Store,
    ui::UiCtx,
    Thunk, ThunkBox, Widget, WidgetBox,
};
use skia_safe::{Rect, Size};

use crate::perform::AppCommand;

/// The engine-side identity of one mounted view.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ViewBridgeId(u64);

impl ViewBridgeId {
    pub fn from_raw(raw: u64) -> ViewBridgeId {
        ViewBridgeId(raw)
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

/// The retained view: imba's display/layout curries, erased the
/// way `DynView` erases them. This trait IS `imba::dyn_view::
/// DynView`'s layout half with the app's concrete command type; it
/// collapses into `DynView` itself once commands erase too
/// (docs/himark-core.md, commands section).
pub(crate) trait BridgeView {
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
}

/// One mounted view: the state carrier, retained. Everything else
/// is a stage derived from it.
pub struct ViewBridge {
    view: Rc<dyn BridgeView>,
}

impl ViewBridge {
    pub(crate) fn new(view: Rc<dyn BridgeView>) -> ViewBridge {
        ViewBridge { view }
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
        let view = Rc::clone(&self.view);
        // SAFETY: the erased-to-'static borrows below point into
        // the box and Rc contents moved into the stage right after
        // — heap addresses stable, owners live in the same struct,
        // declared to drop AFTER the thunk. See `BridgeLayout`'s
        // invariants.
        let thunk = {
            let arena_ref: &'static Arena = unsafe { &*(arena.as_ref() as *const Arena) };
            let store_ref: &'static Store = unsafe { &*(store.as_ref() as *const Store) };
            let ui_ref: &'static UiCtx = unsafe { &*(Rc::as_ptr(&ui)) };
            let view_ref: &'static dyn BridgeView = unsafe { &*Rc::as_ptr(&view) };
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
    view: Rc<dyn BridgeView>,
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
    view: Rc<dyn BridgeView>,
}

impl Frame {
    pub fn handle(&self, event: &Event<'_>, viewport: Rect) -> EventResult<AppCommand> {
        self.tree.handle_event(&self.arena, event, viewport)
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
