// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub type Uri = String;

pub mod documents;
pub mod history;
pub mod locations;
pub mod search;
pub mod text;

pub use documents::{
    DocumentApplied, DocumentClosed, DocumentState, OpenDocumentParams, OpenDocumentResult,
    Replacement, StoreDocumentParams, StoreDocumentResult, TextOperation, TextPosition, TextRange,
    Uid, DOCUMENT_APPLIED, DOCUMENT_CLOSED,
};
pub use locations::{
    Location, LocationList, LocationsChannelResult, LspLocationsParams, SearchLocationsParams,
    LOCATIONS_EXTEND,
};
pub use search::{SearchKind, SearchParams, SearchResult, SearchTarget};
