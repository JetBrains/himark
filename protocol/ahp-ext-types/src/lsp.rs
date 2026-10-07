// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Language Services extension types (docs/ahp/ahp-lsp.md): the
//! diagnostics channel — LSP's one server-push surface, traveling as
//! AHP state (snapshot + replacement actions) because any number of
//! clients share one language server.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Uri;

/// The action type beside `lspDiagnostics/published` bodies
/// (ahp-lsp.md §6.3).
pub const LSP_DIAGNOSTICS_PUBLISHED: &str = "lspDiagnostics/published";

/// `lsp/diagnostics` params (§6.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsParams {
    /// The session's channel URI.
    pub channel: Uri,
}

/// `lsp/diagnostics` result: the session's diagnostics channel
/// (idempotent — a repeat call answers the existing one).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsChannelResult {
    pub channel: Uri,
}

/// The channel's snapshot state (§6.2), keyed by resource URI;
/// resources without diagnostics are absent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsState {
    #[serde(default)]
    pub items: HashMap<Uri, PublishedDiagnostics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedDiagnostics {
    /// The DOCUMENT CHANNEL version identifier the diagnostics were
    /// computed against; absent when the resource is not synchronized
    /// or the language server's version does not match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// LSP `Diagnostic[]`, verbatim (utf-8 positions, ahp-lsp.md §4).
    #[serde(default)]
    pub diagnostics: Vec<serde_json::Value>,
}

/// The `lspDiagnostics/published` action body (§6.3). Reducer:
/// REPLACEMENT — the entry for `uri` swaps whole, and leaves when
/// `diagnostics` is empty, mirroring LSP publishDiagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsPublished {
    pub uri: Uri,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    #[serde(default)]
    pub diagnostics: Vec<serde_json::Value>,
}
