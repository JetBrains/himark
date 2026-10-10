// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! One mounted view (docs/himark-core.md, docs/ui/imba-ffi.md).
//! The engine stores VIEW BRIDGES, not windows: a workbench window
//! is just one kind of mounted view, adapted behind `BridgeView`,
//! and its window id is that adapter's private business. A bridge
//! retains two things — the view (the state carrier) and its
//! realized frame — and nothing of the host's geometry: size
//! arrives with every call, and the frame is a memo over
//! (store revision × size), rebuilt from the retained view when
//! either moves.

use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult},
    store::Store,
    ui::UiCtx,
    Thunk, Widget, WidgetBox,
};
use skia_safe::{Rect, Size};

use crate::app::AppCommand;

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
    ) -> imba::ThunkBox<'a, AppCommand>;
}

/// One bridge's REALIZED widget tree, kept between command batches.
/// The tree is a pure function of (store revision, size): every
/// event, hit-test and paint in between replays against this one
/// realization instead of laying the view out again.
///
/// SAFETY: `tree`'s `'static` is a lie, erased in `build`. What
/// makes it sound:
/// - the tree borrows only `arena`, `store` and `ui` below (boxes
///   and an `Rc`, heap addresses stable, moved but never replaced
///   while the tree lives) and the bridge's retained VIEW — boxed
///   in `ViewBridge`, dropped after the frame (field order there),
///   and never mutated under a live tree;
/// - the tree dies first: fields drop in declaration order, and
///   `recycle` repeats that order by hand;
/// - the arena is never reset and the store never mutated under a
///   live tree;
/// - no `'static` view escapes: every accessor reborrows at the
///   caller's own lifetime.
pub(crate) struct Frame {
    tree: WidgetBox<'static, AppCommand>,
    arena: Box<Arena>,
    store: Box<Store>,
    ui: std::rc::Rc<UiCtx>,
    revision: u64,
    size: Size,
}

impl Frame {
    fn build(
        view: &dyn BridgeView,
        revision: u64,
        size: Size,
        store: Store,
        ui: std::rc::Rc<UiCtx>,
        arena: Box<Arena>,
    ) -> Frame {
        let store = Box::new(store);
        let tree = {
            let thunk = view.layout(&arena, &store, ui.as_ref(), Constraints::tight(size));
            Thunk::realize(thunk, &arena, Rect::from_size(size))
        };
        // SAFETY: the tree borrows the box and Rc contents moved
        // into the frame right below, plus the bridge-owned view;
        // see the type's invariants.
        let tree = unsafe {
            std::mem::transmute::<WidgetBox<'_, AppCommand>, WidgetBox<'static, AppCommand>>(tree)
        };
        Frame {
            tree,
            arena,
            store,
            ui,
            revision,
            size,
        }
    }

    pub(crate) fn handle(&self, event: &Event<'_>, viewport: Rect) -> EventResult<AppCommand> {
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
    fn recycle(self) -> Box<Arena> {
        let Frame {
            tree,
            mut arena,
            store,
            ui,
            revision: _,
            size: _,
        } = self;
        drop(tree);
        drop(store);
        drop(ui);
        arena.reset();
        arena
    }
}

/// One mounted view: the retained view and its retained frame.
///
/// Field order is load-bearing (see `Frame`'s invariants): the
/// frame drops before the view its tree may borrow.
pub(crate) struct ViewBridge {
    frame: Option<Frame>,
    view: Box<dyn BridgeView>,
    spare_arena: Option<Box<Arena>>,
}

impl ViewBridge {
    pub(crate) fn new(view: Box<dyn BridgeView>) -> ViewBridge {
        ViewBridge {
            frame: None,
            view,
            spare_arena: None,
        }
    }

    /// The realized frame, current for (store revision × size) —
    /// rebuilt from the retained view when either moved. The
    /// caller hands the LIVE store; the view cuts its own frame
    /// snapshot (`BridgeView::frame_store`).
    pub(crate) fn frame(
        &mut self,
        store: &Store,
        ui: &std::rc::Rc<UiCtx>,
        revision: u64,
        size: Size,
    ) -> &mut Frame {
        let fresh = self
            .frame
            .as_ref()
            .is_some_and(|frame| frame.revision == revision && frame.size == size);
        if !fresh {
            if let Some(frame) = self.frame.take() {
                self.spare_arena = Some(frame.recycle());
            }
            let arena = self.spare_arena.take().unwrap_or_default();
            let snapshot = self.view.frame_store(store, ui.as_ref());
            self.frame = Some(Frame::build(
                self.view.as_ref(),
                revision,
                size,
                snapshot,
                ui.clone(),
                arena,
            ));
        }
        self.frame.as_mut().expect("the frame just built")
    }
}
