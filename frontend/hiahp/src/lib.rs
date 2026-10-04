// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The protocol crate: the AHP adapter (transport, session wire,
//! fs/doc/LSP routes, docsync), the HIGENT catalog (hosts, clients,
//! sessions, the chat) and the DRIVERS — every coroutine between a
//! himark shell and its agent hosts, with no window anywhere.

pub mod completion;
pub mod docsync;
pub mod drivers;
pub mod higent;

pub use completion::{
    Completion, CompletionCommand, CompletionFound, CompletionPopupView, PickedFile,
};
pub mod fsroute;
pub mod lsproute;
pub mod open;

pub use ahp_locations::{find, routes as locations, FindEffect, LocationsChannel, LspLocationsEffect, LspLocationsKind, SearchLocationsEffect};
pub use ahp_wire::{fs, registry, transport, uris, uuid_v4, wire};
pub use ahp_wire::{SessionId, LOCAL_FS_SESSION};

#[derive(Clone, Debug)]
pub struct LspAnswer {
    pub items: Vec<LspItem>,

    pub incomplete: bool,
}

#[derive(Clone, Debug)]
pub struct LspItem {
    pub label: String,
    pub detail: Option<String>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,

    pub edit: Option<(std::ops::Range<documents::LineCol>, String)>,
    pub insert_text: Option<String>,
}

pub struct LspCompletionEffect {
    pub location: editor::ResourceLocation,
    pub position: documents::LineCol,
}

impl std::fmt::Display for LspCompletionEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "lsp completion /{}", self.location.path().join("/"))
    }
}

impl imba::effect::Effect for LspCompletionEffect {
    type Result = Option<LspAnswer>;
}
