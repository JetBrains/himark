// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The SESSION place vocabulary — what the AHP half of the app can
//! ask to see, keyed by the session. The engine's content places
//! (documents, diffs, locations) stay in `himark::places`.

use hikit::navigation::Place;

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

/// The FOLDER PICKER — the session host's file tree in a modal (the
/// Files tab's view, rooted at the host's home), for granting a
/// workspace folder when the system chooser cannot reach the
/// session's machine (a remote host; every iOS session).
#[derive(Clone, PartialEq)]
pub struct FolderPickerPlace(pub ahp_wire::SessionId);

impl Place for FolderPickerPlace {}

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

/// A session's FILE tree.
#[derive(Clone, PartialEq)]
pub struct FilesPlace(pub ahp_wire::SessionId);

impl Place for FilesPlace {}

