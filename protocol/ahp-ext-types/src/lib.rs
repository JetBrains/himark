pub type Uri = String;

pub mod documents;
pub mod history;
pub mod search;
pub mod text;

pub use documents::{
    DocumentApplied, DocumentClosed, DocumentState, OpenDocumentParams, OpenDocumentResult,
    Replacement, TextOperation, TextPosition, TextRange, Uid, DOCUMENT_APPLIED, DOCUMENT_CLOSED,
};
pub use search::{SearchKind, SearchParams, SearchResult, SearchTarget};
