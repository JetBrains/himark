// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The pane-row vocabulary, ERASED: a row names one re-mintable
//! pane of a session session (a terminal, a chat, a tracked pair, a
//! canvas) by ids and private keys. Each feature defines its own row
//! type; the workbench compares rows by value and re-mints panes
//! through the registered minters — it never learns the row's shape.

use std::any::{Any, TypeId};
use std::sync::Arc;

/// A feature's pane-row payload — plain ids and keys, compared by
/// value.
pub trait Row: Clone + PartialEq + Send + Sync + 'static {}

trait ErasedRow: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn same(&self, other: &dyn Any) -> bool;
}

impl<R: Row> ErasedRow for R {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn same(&self, other: &dyn Any) -> bool {
        other.downcast_ref::<R>().is_some_and(|other| other == self)
    }
}

#[derive(Clone)]
pub struct PaneRow {
    row_type: TypeId,
    payload: Arc<dyn ErasedRow>,
}

impl PaneRow {
    pub fn new<R: Row>(row: R) -> Self {
        Self {
            row_type: TypeId::of::<R>(),
            payload: Arc::new(row),
        }
    }

    pub fn row<R: Row>(&self) -> Option<&R> {
        self.payload.as_any().downcast_ref::<R>()
    }
}

impl PartialEq for PaneRow {
    fn eq(&self, other: &Self) -> bool {
        self.row_type == other.row_type && self.payload.same(other.payload.as_any())
    }
}

impl std::fmt::Debug for PaneRow {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("state row")
    }
}
