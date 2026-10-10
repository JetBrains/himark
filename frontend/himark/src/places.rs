// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The place vocabulary the SHELL road speaks (docs/himark-core.md):
//! what the user asked to SEE, content-addressed — never a pane, a
//! screen or a split. The workbench shell places these in panes; a
//! native shell pushes screens and mints bridges for the same
//! places.

use hikit::navigation::Place;

/// Open a located document, optionally at a target range —
/// focused (a deliberate jump) or just shown.
#[derive(Clone, PartialEq)]
pub struct DocumentPlace {
    pub location: editor::location::ResourceLocation,
    pub target: Option<std::ops::Range<documents::text_ext::LineCol>>,
    pub focus: bool,
}

impl Place for DocumentPlace {}

/// Show a document ALREADY OPEN in its collection — an open's
/// landing half, and a modal's "show this one".
#[derive(Clone, PartialEq)]
pub struct OpenDocumentPlace {
    pub document: documents::DocumentId,
    pub target: Option<std::ops::Range<documents::text_ext::LineCol>>,
    pub focus: bool,
}

impl Place for OpenDocumentPlace {}

/// A diff of two located sides.
#[derive(Clone, PartialEq)]
pub struct DiffPlace {
    pub old: editor::location::ResourceLocation,
    pub new: editor::location::ResourceLocation,
}

impl Place for DiffPlace {}

/// A batch of locations — search results, a multi-open.
#[derive(Clone, PartialEq)]
pub struct LocationsPlace(pub Vec<editor::location::ResourceLocation>);

impl Place for LocationsPlace {}

/// A session's CHAT — transcript and the square composer, whole.
#[derive(Clone, PartialEq)]
pub struct ChatPlace(pub ahp_wire::SessionId);

impl Place for ChatPlace {}

/// A session's CHANGES tree.
#[derive(Clone, PartialEq)]
pub struct ChangesPlace(pub ahp_wire::SessionId);

impl Place for ChangesPlace {}

/// A session's FILE tree.
#[derive(Clone, PartialEq)]
pub struct FilesPlace(pub ahp_wire::SessionId);

impl Place for FilesPlace {}
