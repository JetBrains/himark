// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The diff FACES: the embedded split-diff pane, the diff canvas
//! (rows, header, banner, the canvases' At router) and their
//! navigation places and session rows. Built over the changesview
//! collection, the documents diff machinery and the kit — no window,
//! no session, no wire.

pub mod canvas;
pub mod diff_canvas;
pub mod diff_header;
pub mod diff_pane;

/// A tracked diff pair's row: the documents collection and the view.
#[derive(Clone, PartialEq)]
pub struct PairRow(
    pub imba::store::Id<documents::OpenDocuments>,
    pub documents::diffs::DiffViewId,
);

impl hikit::pane_row::Row for PairRow {}

/// A diff canvas's row: its source names it.
#[derive(Clone, PartialEq)]
pub struct CanvasRow(pub changesview::hichanges::CanvasSource);

impl hikit::pane_row::Row for CanvasRow {}
