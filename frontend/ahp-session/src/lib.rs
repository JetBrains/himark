// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The session catalog: hosts and their session states, the mint/
//! dispose ceremony that wires every collection and driver row, the
//! fs-effect routes' watch border — the one crate that knows the
//! whole protocol family, sitting on top of the domain crates.

pub mod drivers;
pub mod higent;

pub use ahp_chat::completion::{
    Completion, CompletionCommand, CompletionFound, CompletionPopupView, PickedFile,
};
pub use ahp_chat::{completion, open};
pub mod fsroute;

pub use ahp_docsync as docsync;
pub use ahp_lsp as lsproute;
pub use ahp_lsp::{LspAnswer, LspCompletionEffect, LspItem};
pub use ahp_locations::{find, routes as locations, FindEffect, LocationsChannel, LspLocationsEffect, LspLocationsKind, SearchLocationsEffect};
pub use ahp_wire::{fs, registry, transport, uris, uuid_v4, wire};
pub use ahp_wire::{SessionId, LOCAL_FS_SESSION};

