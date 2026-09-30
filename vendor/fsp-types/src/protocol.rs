//! Payload types for every FSP method, matching PROTOCOL.md field-for-field.
//! All structs use camelCase on the wire.
//!
//! Every type derives both `Serialize` and `Deserialize`: the same structs
//! serve servers (deserialize params, serialize results) and clients
//! (serialize params, deserialize results).

use serde::{Deserialize, Serialize};

use crate::uri::Uri;
use serde_json::Value;

// ---------------------------------------------------------------- basics §2

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    /// UTF-16 code units by default (negotiable via `positionEncoding`).
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProgressToken {
    Int(i64),
    Str(String),
}

// ------------------------------------------------------------- lifecycle §3

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub process_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<ClientInfo>,
    pub capabilities: ClientCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_folders: Option<Vec<SearchFolder>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_done_token: Option<ProgressToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_encodings: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_results: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub capabilities: ServerCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    pub position_encoding: String,
    pub text_document_sync: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_search_provider: Option<TextSearchOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_search_provider: Option<FileSearchOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceServerCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceServerCapabilities {
    pub search_folders: SearchFoldersServerCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFoldersServerCapabilities {
    pub supported: bool,
    pub change_notifications: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSearchOptions {
    pub reg_exp: bool,
    pub multiline: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSearchOptions {
    pub fuzzy: bool,
}

// ---------------------------------------------------------------- folders §4

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFolder {
    pub uri: Uri,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excludes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub respect_ignore_files: Option<bool>,
}

impl SearchFolder {
    pub fn respect_ignore_files(&self) -> bool {
        self.respect_ignore_files.unwrap_or(true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DidChangeSearchFoldersParams {
    pub event: SearchFoldersChangeEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFoldersChangeEvent {
    pub added: Vec<SearchFolder>,
    pub removed: Vec<SearchFolder>,
}

// ----------------------------------------------------------- doc sync §5

pub mod text_document_sync_kind {
    pub const NONE: u8 = 0;
    pub const FULL: u8 = 1;
    pub const INCREMENTAL: u8 = 2;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidOpenTextDocumentParams {
    pub text_document: TextDocumentItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentItem {
    pub uri: Uri,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_id: Option<String>,
    pub version: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeTextDocumentParams {
    pub text_document: VersionedTextDocumentIdentifier,
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedTextDocumentIdentifier {
    pub uri: Uri,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentContentChangeEvent {
    /// Absent range = full-content replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidCloseTextDocumentParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: Uri,
}

// ---------------------------------------------------------- text search §6

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSearchParams {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_reg_exp: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_case_sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_word_match: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_multiline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirs: Option<Vec<Uri>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub includes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excludes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextLines>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results_per_file: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<BinaryMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_result_token: Option<ProgressToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_done_token: Option<ProgressToken>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ContextLines {
    #[serde(default)]
    pub before: u32,
    #[serde(default)]
    pub after: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BinaryMode {
    #[default]
    Skip,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSearchResult {
    pub results: Vec<TextSearchFileResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_hit: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSearchFileResult {
    pub uri: Uri,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    pub matches: Vec<TextSearchMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_hit: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextSearchMatch {
    pub range: Range,
    pub lines: Vec<ContextLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextLine {
    pub line: u32,
    pub text: String,
    pub is_match: bool,
}

// ---------------------------------------------------------- file search §7

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchParams {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_case_sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirs: Option<Vec<Uri>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub includes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excludes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_result_token: Option<ProgressToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_done_token: Option<ProgressToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchResult {
    pub results: Vec<FileMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_hit: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMatch {
    pub uri: Uri,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_matches: Option<Vec<Range>>,
}

// --------------------------------------------------- progress + cancel §8-10

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressParams {
    pub token: ProgressToken,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelParams {
    pub id: crate::jsonrpc::Id,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WorkDoneProgress {
    Begin {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Report {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    End {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_search_params_from_protocol_example() {
        // The exact request from PROTOCOL.md §6.1.
        let raw = r#"{
          "query":"fn\\s+main",
          "isRegExp":true,
          "isCaseSensitive":true,
          "includes":["**/*.rs"],
          "context":{"before":1,"after":1},
          "maxResults":1000,
          "partialResultToken":"tok-7"
        }"#;
        let p: TextSearchParams = serde_json::from_str(raw).unwrap();
        assert_eq!(p.query, "fn\\s+main");
        assert_eq!(p.is_reg_exp, Some(true));
        assert_eq!(p.context.unwrap().before, 1);
        assert_eq!(
            p.partial_result_token,
            Some(ProgressToken::Str("tok-7".into()))
        );
    }

    #[test]
    fn match_serialization_matches_protocol_example() {
        let m = TextSearchMatch {
            range: Range {
                start: Position { line: 12, character: 0 },
                end: Position { line: 12, character: 7 },
            },
            lines: vec![ContextLine {
                line: 12,
                text: "fn main() {".into(),
                is_match: true,
            }],
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["range"]["start"]["line"], 12);
        assert_eq!(v["lines"][0]["isMatch"], true);
    }

    #[test]
    fn params_round_trip_client_to_server() {
        // The client half: params SERIALIZE, absent options stay off the
        // wire, and the server-side deserialization reads them back.
        let p = TextSearchParams {
            query: "needle".into(),
            is_reg_exp: None,
            is_case_sensitive: Some(true),
            is_word_match: None,
            is_multiline: None,
            dirs: None,
            includes: None,
            excludes: None,
            context: None,
            max_results: Some(10),
            max_results_per_file: None,
            max_file_size: None,
            binary: None,
            encoding: None,
            partial_result_token: Some(ProgressToken::Int(7)),
            work_done_token: None,
        };
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("isRegExp").is_none(), "None options stay off the wire");
        assert_eq!(v["isCaseSensitive"], true);
        let back: TextSearchParams = serde_json::from_value(v).unwrap();
        assert_eq!(back.max_results, Some(10));
        assert_eq!(back.partial_result_token, Some(ProgressToken::Int(7)));
    }

    #[test]
    fn results_round_trip_server_to_client() {
        let r = FileSearchResult {
            results: vec![FileMatch {
                uri: Uri::from("file:///repo/src/main.rs"),
                score: Some(2.5),
                path_matches: None,
            }],
            limit_hit: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        let back: FileSearchResult = serde_json::from_value(v).unwrap();
        assert_eq!(back.results[0].uri.as_str(), "file:///repo/src/main.rs");
        assert_eq!(back.results[0].score, Some(2.5));
        assert_eq!(back.limit_hit, None);
    }
}
