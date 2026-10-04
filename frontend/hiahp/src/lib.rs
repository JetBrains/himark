// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The protocol crate: the AHP adapter (transport, session wire,
//! fs/doc/LSP routes, docsync), the HIGENT catalog (hosts, clients,
//! sessions, the chat) and the DRIVERS — every coroutine between a
//! himark shell and its agent hosts, with no window anywhere.

pub mod completion;
pub mod drivers;
pub mod higent;

pub use completion::{
    Completion, CompletionCommand, CompletionFound, CompletionPopupView, PickedFile,
};
pub mod fsroute;
pub mod open;

pub use ahp_docsync as docsync;
pub use ahp_lsp as lsproute;
pub use ahp_lsp::{LspAnswer, LspCompletionEffect, LspItem};
pub use ahp_locations::{find, routes as locations, FindEffect, LocationsChannel, LspLocationsEffect, LspLocationsKind, SearchLocationsEffect};
pub use ahp_wire::{fs, registry, transport, uris, uuid_v4, wire};
pub use ahp_wire::{SessionId, LOCAL_FS_SESSION};

