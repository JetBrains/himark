//! Wire-level types for the File Search Protocol (FSP).
//!
//! Mirrors PROTOCOL.md: JSON-RPC 2.0 messages with LSP-style
//! `Content-Length` framing, plus serde structs for every request,
//! response and notification payload.
//!
//! VENDORED: a hand-synced copy of `crates/fsp-types` from the
//! JetBrains-internal `file-search-protocol` repository
//! (github.com/JetBrains/file-search-protocol) — himark is public and
//! cannot depend on it directly (docs/file-search.md §8). To sync, copy
//! the crate's `src/` over this one and re-read the diff; do not edit
//! these files here.

pub mod framing;
pub mod jsonrpc;
pub mod protocol;
pub mod uri;
