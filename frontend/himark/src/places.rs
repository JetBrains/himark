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

/// An INSTALLED split-diff view — the ready half of `DiffPlace`,
/// minted by the open landing once both sides are registered and
/// the pair installed. A native shell mounts it synchronously.
#[derive(Clone, PartialEq)]
pub struct DiffViewPlace {
    pub documents: imba::store::Id<documents::OpenDocuments>,
    pub view: documents::diffs::DiffViewId,
    /// The NEW side's name — the pushed screen's title, computed
    /// at install where the pair is in hand.
    pub title: String,
}

impl Place for DiffViewPlace {}

/// A session's CHAT — transcript and the square composer, whole.
#[derive(Clone, PartialEq)]
pub struct ChatPlace(pub ahp_wire::SessionId);

impl Place for ChatPlace {}

/// A session chat's TRANSCRIPT alone — a native scroller's content.
#[derive(Clone, PartialEq)]
pub struct ChatTranscriptPlace(pub ahp_wire::SessionId);

impl Place for ChatTranscriptPlace {}

/// A session chat's COMPOSER BAND alone, at its natural height.
#[derive(Clone, PartialEq)]
pub struct ChatComposerPlace(pub ahp_wire::SessionId);

impl Place for ChatComposerPlace {}

/// A session's CHANGES tree.
#[derive(Clone, PartialEq)]
pub struct ChangesPlace(pub ahp_wire::SessionId);

impl Place for ChangesPlace {}

/// The session's changes tree whose rows open the DIFF CANVAS —
/// the desktop flavor (iOS opens one inline diff per row).
#[derive(Clone, PartialEq)]
pub struct ChangesCanvasPlace(pub ahp_wire::SessionId);

impl Place for ChangesCanvasPlace {}

/// The session's commit HISTORY (the changes tree, History set),
/// rows opening commit canvases.
#[derive(Clone, PartialEq)]
pub struct HistoryPlace(pub ahp_wire::SessionId);

impl Place for HistoryPlace {}

/// The session's search face — the location lists.
#[derive(Clone, PartialEq)]
pub struct SearchPlace(pub ahp_wire::SessionId);

impl Place for SearchPlace {}

/// The session's comments panel.
#[derive(Clone, PartialEq)]
pub struct CommentsPlace(pub ahp_wire::SessionId);

impl Place for CommentsPlace {}

/// The go-to-file face — the peeker, whole: query, results,
/// preview, the FSP search behind them.
#[derive(Clone, PartialEq)]
pub struct PeekerPlace(pub ahp_wire::SessionId);

impl Place for PeekerPlace {}

/// The COMMAND PALETTE — the registry walk, the query and the
/// filtered list. The shell mounts the face and paints the glass
/// around it; the palette itself is the engine's, once.
#[derive(Clone, PartialEq)]
pub struct PalettePlace(pub ahp_wire::SessionId);

impl Place for PalettePlace {}

/// The SESSIONS browser — hosts, their folders and their sessions,
/// with the speed search over them. One list, both roads.
#[derive(Clone, PartialEq)]
pub struct SessionsPlace(pub ahp_wire::SessionId);

impl Place for SessionsPlace {}

/// A mounted modal asked to CLOSE (escape, a pick completing) —
/// the shell dismisses whatever face hosts the origin.
#[derive(Clone, PartialEq)]
pub struct DismissPlace;

impl Place for DismissPlace {}

/// A session's FILE tree.
#[derive(Clone, PartialEq)]
pub struct FilesPlace(pub ahp_wire::SessionId);

impl Place for FilesPlace {}
