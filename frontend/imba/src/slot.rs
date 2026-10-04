// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! View slots: a MODEL row stores a view value without knowing its
//! type — the views-on-collections pattern across crate lines
//! (docs/model-view.md). The collection holds slots and the join
//! bookkeeping; the view's own crate puts values in and reads them
//! back by downcast. A misread type answers `None`, the gone-row
//! convention.

use std::any::Any;

pub trait ViewSlot: Send + Sync + 'static {
    fn clone_slot(&self) -> Box<dyn ViewSlot>;
    fn as_any(&self) -> &dyn Any;
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

impl<V: Clone + Send + Sync + 'static> ViewSlot for V {
    fn clone_slot(&self) -> Box<dyn ViewSlot> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl Clone for Box<dyn ViewSlot> {
    fn clone(&self) -> Self {
        self.clone_slot()
    }
}
