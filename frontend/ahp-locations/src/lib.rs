// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The locations protocol domain: the streaming `ahp-locations:/…`
//! channels (content search, the location-answering LSP asks), the
//! quick-open path find, their route handlers, and the driver over
//! the `LocationLists` collection.

pub mod driver;
pub mod find;
pub mod routes;

/// The quick-open path find: fuzzy over names, capped, one answer.
/// Content search is the streaming locations channel
/// (`SearchLocationsEffect`), not a Find target.
pub struct FindEffect {
    pub folders: Vec<editor::location::ResourceLocation>,
    pub term: String,
}

impl std::fmt::Display for FindEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "find paths {}", self.term)
    }
}

impl imba::effect::Effect for FindEffect {
    type Result = Vec<editor::location::ResourceLocation>;
}

/// A live `ahp-locations:/…` result stream, as the ask effects
/// answer it: the client and channel to subscribe/poll/unsubscribe
/// (docs/ahp/ahp-locations.md), plus the route's way back from the
/// stream's resource URIs to locations — the shell never parses URIs.
#[derive(Clone)]
pub struct LocationsChannel {
    pub client: std::sync::Arc<dyn ahp_wire::client::LocationsClient>,
    pub channel: ahp_wire::client::ChannelUri,
    pub resolve:
        std::sync::Arc<dyn Fn(&str) -> Option<editor::location::ResourceLocation> + Send + Sync>,
}

/// The streaming content search ask. Answers the channel; results
/// stream as `LocationList` batches; unsubscribing cancels the walk.
pub struct SearchLocationsEffect {
    pub folders: Vec<editor::location::ResourceLocation>,
    pub query: String,
    /// Literal by default; the query as a regular expression when set.
    pub regex: bool,
    pub case_sensitive: bool,
    pub limit: usize,
}

impl std::fmt::Display for SearchLocationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "search locations {}", self.query)
    }
}

impl imba::effect::Effect for SearchLocationsEffect {
    type Result = Result<LocationsChannel, String>;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LspLocationsKind {
    References,
    Implementations,
}

/// The location-answering LSP asks, streamed over the same channel
/// shape as the content search.
pub struct LspLocationsEffect {
    pub location: editor::location::ResourceLocation,
    pub position: documents::text_ext::LineCol,
    pub kind: LspLocationsKind,
}

impl std::fmt::Display for LspLocationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "lsp locations {:?} /{}",
            self.kind,
            self.location.path().join("/")
        )
    }
}

impl imba::effect::Effect for LspLocationsEffect {
    type Result = Result<LocationsChannel, String>;
}
