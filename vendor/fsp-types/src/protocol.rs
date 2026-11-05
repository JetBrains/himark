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
    /// Present iff the server supports `workspace/symbolSearch` (§13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_search_provider: Option<SymbolSearchOptions>,
}

/// §13.2: what the declaration index offers. Declarations only — there is
/// no references or implementations method (owner decision 2026-11-03:
/// resolution is the IDE's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolSearchOptions {
    pub work_done_progress: bool,
    /// The `matchMode`s accepted: `exact`, `prefix`, `fuzzy`.
    pub match_modes: Vec<MatchMode>,
    /// `kinds` filtering is supported.
    pub kinds: bool,
    /// `sourceLines` is supported (the declaration's text in the answer).
    pub source_lines: bool,
    /// The languages a parser reads (`SymbolInformation.language` values);
    /// a file of any other language declares nothing to the index.
    pub languages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceServerCapabilities {
    pub search_folders: SearchFoldersServerCapabilities,
    /// True iff the server answers `workspace/indexStatus` and sends
    /// `workspace/indexReady` (§4.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_status: Option<bool>,
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
    // Stage-2 feature flags (PROTOCOL §3.1): absent means false. A slice's
    // implementer flips its flag to `Some(true)` when the slice is complete;
    // a client sends the matching request flag only when it reads `true`.
    /// `TextSearchParams.merge_windows` / `TextSearchMatch.ranges` (§6.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_windows: Option<bool>,
    /// The total order of §6.4 and `TextSearchResult.cut`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordered_cut: Option<bool>,
    /// `TextSearchResult.explain` and the `note` grammar of §6.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explain: Option<bool>,
    /// `TextSearchParams.files_only` / `count_matches`, `TextSearchFileResult.count` (§6.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_only: Option<bool>,
    /// `TextSearchParams.enclosing` / `TextSearchMatch.in` (§6.7): the
    /// declaration around a match, from the declaration index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enclosing: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchOptions {
    pub fuzzy: bool,
    /// `FileMatch.match_kind` and the exact-first order (§7.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_kind: Option<bool>,
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
    /// URI of another folder this one is a copy of (PROTOCOL §4): a seeding
    /// hint — the server may start this folder's index from that folder's
    /// instead of building cold. Not part of the folder's configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_of: Option<Uri>,
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

// --------------------------------------------------- index status §4.2

/// `workspace/indexStatus` result: one entry per folder the session has
/// registered, in registration order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexStatusResult {
    pub folders: Vec<FolderIndexStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderIndexStatus {
    pub uri: Uri,
    pub state: IndexState,
    /// Whether this session of the folder started from a prior store
    /// (`hot`) or built it from nothing (`cold`).
    pub start_mode: StartMode,
    /// Files whose content is in the index.
    pub files: u64,
    /// Change events queued for the index writer and not yet applied.
    /// During a cold build: the files still to index.
    pub pending_ops: u64,
    /// On-disk size of the folder's shard, in bytes.
    pub data_bytes: u64,
    /// When the folder entered `state` (RFC 3339, UTC, milliseconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// The last index build error; the state stays `building` and the
    /// server retries on the next change round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The folder's declaration index (§13.1), when the server builds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_index: Option<SymbolIndexStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexState {
    /// The initial inventory or the content index is still being built;
    /// queries are answered by scanning.
    Building,
    /// Inventory and index are complete and published: queries are served
    /// from the index.
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartMode {
    Cold,
    Hot,
}

/// §13.1 `SymbolIndexStatus`: the declaration index is built in the same
/// pass as the text index, so its state is the folder's (`building` until
/// the first full drain, then `ready`); `files` counts the documents with
/// a declaration record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolIndexStatus {
    pub state: IndexState,
    pub files: u64,
}

/// `workspace/indexReady` notification params: sent once per folder per
/// session when the folder's index becomes `ready`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexReadyParams {
    pub uri: Uri,
    /// Files in the index at that moment.
    pub files: u64,
    /// Time from the folder's registration in this server process to
    /// readiness.
    pub elapsed_ms: u64,
    pub start_mode: StartMode,
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
    /// §6.3: one `TextSearchMatch` per block of overlapping or touching
    /// context windows, with `ranges`. Default false (one per match).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_windows: Option<bool>,
    /// §6.6 (`grep -l`): files, not matches; `max_results` counts files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_only: Option<bool>,
    /// §6.6 (`grep -c`): `TextSearchFileResult.count` on every reported file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count_matches: Option<bool>,
    /// §6.7: every match carries `in`, the innermost declaration around
    /// it, when the folder has a declaration index. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enclosing: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_result_token: Option<ProgressToken>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_done_token: Option<ProgressToken>,
}

impl TextSearchParams {
    /// The stage-2 request flags as booleans (absent = false).
    pub fn merge_windows(&self) -> bool {
        self.merge_windows.unwrap_or(false)
    }
    pub fn files_only(&self) -> bool {
        self.files_only.unwrap_or(false)
    }
    pub fn count_matches(&self) -> bool {
        self.count_matches.unwrap_or(false)
    }
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
    /// The explanation printed verbatim after the footer: why the result
    /// set is incomplete when `limitHit` alone does not say (the size cap),
    /// or the prose of `explain` for an answer with no match (§6.5 grammar).
    /// At most [`NOTE_MAX_LINES`] lines and [`NOTE_MAX_BYTES`] bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// What `maxResults` cut off (§6.4); present iff the limit stopped the
    /// search. Needs `textSearchProvider.orderedCut`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut: Option<Cut>,
    /// The structured explanation of an answer with no match (§6.5), always
    /// with its `note`. Needs `textSearchProvider.explain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explain: Option<Explain>,
    /// How the answer was produced (§6.2); on the final response when streamed.
    pub completeness: Completeness,
}

/// The size of a `note` (PROTOCOL §6): three lines, 240 bytes of UTF-8.
/// Every byte of it is replayed on every later turn of the reader.
pub const NOTE_MAX_LINES: usize = 3;
pub const NOTE_MAX_BYTES: usize = 240;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSearchFileResult {
    pub uri: Uri,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    /// Matches, or blocks with `mergeWindows` (§6.3); empty with `filesOnly`.
    pub matches: Vec<TextSearchMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_hit: Option<bool>,
    /// Matches in the whole file (§6.6); present iff `countMatches` was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextSearchMatch {
    /// The match, or with `mergeWindows` the block's first match (`ranges[0]`).
    pub range: Range,
    /// §6.3: every match of the block in position order, `ranges[0] == range`.
    /// Absent without `mergeWindows`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranges: Option<Vec<Range>>,
    pub lines: Vec<ContextLine>,
    /// §6.7: the innermost declaration whose lines hold `range.start`, from
    /// the folder's declaration index; only with `enclosing: true`, and
    /// absent when no declaration covers the match.
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub enclosing: Option<EnclosingDeclaration>,
}

/// §6.7 `TextSearchMatch.in`: a declaration by name, kind and range, in the
/// shape of `SymbolInformation` minus the file (it is the match's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnclosingDeclaration {
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    pub range: Range,
}

// ------------------------------------------------------ symbol search §13

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    /// The name as written (the default).
    Exact,
    /// Names starting with the query.
    Prefix,
    /// The fileSearch scorer's subsequence / camel-hump rules over names
    /// (§7.1), case-insensitive unless `isCaseSensitive`.
    Fuzzy,
}

/// `SymbolSearchParams.sourceLines`: `N` lines of the declaration's text
/// (header and body, from its first line) or `"all"` of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SourceLines {
    Lines(u32),
    Word(SourceLinesWord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceLinesWord {
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolSearchParams {
    /// A declaration name, optionally qualified by its container
    /// (`Owner.member`, `Owner#member`, `a::b::Name`): the last segment is
    /// matched by `matchMode`, the ones before must end the container chain.
    pub query: String,
    /// Default `exact`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_mode: Option<MatchMode>,
    /// Only declarations of these kinds (the `kinds` vocabulary of §13.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    /// Stop after this many declarations. Default 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
    /// Restrict to these directories (inside a search folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirs: Option<Vec<Uri>>,
    /// Include declarations in files that look like tests (§6.4). Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_tests: Option<bool>,
    /// `exact`/`prefix` compare bytes by default (true); `fuzzy` folds case
    /// by default (false). Explicit, never smart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_case_sensitive: Option<bool>,
    /// The declaration's text in `SymbolInformation.source`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_lines: Option<SourceLines>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_done_token: Option<ProgressToken>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolSearchResult {
    pub results: Vec<SymbolInformation>,
    /// Declarations the server matched: the whole set when `limitHit` is
    /// absent, otherwise those it examined before the limit stopped it.
    pub total: u32,
    /// True if `maxResults` stopped the search with candidates remaining.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_hit: Option<bool>,
    /// `indexed` when every folder's index is ready, `partial` while one is
    /// still building (the answer covers what has been indexed so far; a
    /// declaration lookup never scans).
    pub completeness: Completeness,
    /// What a 0-hit answer offers instead (§13.3 note grammar).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub index: SymbolIndexStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInformation {
    pub name: String,
    /// The declaration's kind (`kinds` of §13.3).
    pub kind: String,
    /// The enclosing declaration's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    pub uri: Uri,
    /// The whole declaration: start of its first line to the end of its last.
    pub range: Range,
    /// The name within the declaration, when the source writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_range: Option<Range>,
    pub language: String,
    /// The declaration's first line, trimmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// With `sourceLines`: the declaration's text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SymbolSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolSource {
    /// 0-based line of `text`'s first line (`range.start.line`).
    pub start_line: u32,
    /// The lines, joined by `\n`, without a trailing newline.
    pub text: String,
    /// True when `sourceLines: N` cut the declaration short of `range.end`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

// ------------------------------------------------------ order and cut §6.4

/// What `maxResults` left behind (PROTOCOL §6.4). The files of the answer
/// are a prefix of the total order; `cut` describes the rest of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cut {
    /// Files in the answer with at least one reported match.
    pub files_with_hits: u32,
    /// Files the search did not open because the limit came first.
    pub files_left: u32,
    /// What `files_left` counts.
    pub left_kind: LeftKind,
    /// The directories (a file's own parent) holding the most files left:
    /// at most [`CUT_MAX_DIRS`], largest first, ties by `uri` bytes.
    pub dirs: Vec<CutDir>,
    /// Directories with files left that `dirs` does not list.
    pub dirs_left: u32,
}

/// The cap on `Cut::dirs`: eight directories cover the common shape of a
/// limit hit (one or two modules) and cost ~500 rendered bytes at most.
pub const CUT_MAX_DIRS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeftKind {
    /// `completeness == indexed`: the files left hold every trigram the
    /// query needs — an upper bound of the files with matches.
    Candidates,
    /// Any other completeness: every file of the scope the walk had not
    /// reached, matching or not.
    Files,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutDir {
    pub uri: Uri,
    /// Files left in exactly this directory.
    pub files: u32,
}

// ------------------------------------------------- empty answers §6.5

/// The structured explanation of an answer with no match (PROTOCOL §6.5).
/// The search never widens: this describes the scope, it does not search
/// outside it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Explain {
    /// Per literal of the query (the whole literal, then its words of ≥ 3
    /// characters; for a regex the planner's required literals), at most
    /// [`CENSUS_MAX_LITERALS`] entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub census: Option<Vec<LiteralCensus>>,
    /// `dirs` entries that are not directories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_dirs: Option<Vec<Uri>>,
    /// Names near the query, at most [`DID_YOU_MEAN_MAX`], best first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub did_you_mean: Option<Vec<Suggestion>>,
}

pub const CENSUS_MAX_LITERALS: usize = 5;
pub const DID_YOU_MEAN_MAX: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiteralCensus {
    pub literal: String,
    /// Files in scope whose trigram set admits the literal (case-folded):
    /// an upper bound of the files containing it in any case — exact when
    /// the server verified the candidates (fsp-server: ≤ 1 000 per side).
    pub in_scope: u32,
    /// The same over the scope's search folders, minus `in_scope`.
    pub outside_scope: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub name: String,
    pub kind: SuggestionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<Uri>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SuggestionKind {
    /// A file-name stem within edit distance of the query (§6.5).
    File,
    /// A declared name from the declaration index (§13); stage 3, never
    /// emitted before it.
    Declaration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextLine {
    pub line: u32,
    /// The line's text without the trailing newline — the whole line unless
    /// it is longer than the server's limit, in which case a window of it
    /// (`start`/`truncated` say so).
    pub text: String,
    /// Offset of `text[0]` within the full line, in the negotiated position
    /// encoding; absent means 0 (the text starts at the line start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
    /// True when `text` is a window of a longer line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    pub is_match: bool,
}

// ------------------------------------------------------- completeness §6.2
// (HANDOVER 1c; kept as one block — 1d adds the index-status types nearby.)

/// How a `textSearch`/`fileSearch` answer was produced. A scan is as exact
/// as an index lookup: the label tells the client what the answer cost and
/// whether the folder's index was in play yet, never whether matches are
/// missing (`limitHit` says that).
///
/// The same values label one scope: for a scope, `Partial` means it was
/// scanned because its folder's index was still building.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Completeness {
    /// Every scope was served from a ready index.
    Indexed,
    /// Every scope was walked, and no index was pending for any of them:
    /// the folder has none (a `dirs` entry outside the search folders) or
    /// the query cannot use one (no literal of three characters).
    Scan,
    /// A mix of indexed and scanned scopes, or a scope walked because its
    /// folder's index was still building.
    Partial,
}

impl Completeness {
    pub fn as_str(self) -> &'static str {
        match self {
            Completeness::Indexed => "indexed",
            Completeness::Scan => "scan",
            Completeness::Partial => "partial",
        }
    }

    /// The request's value from its scopes': the value they all share, or
    /// `Partial` when they differ. No scopes at all counts as `Indexed`
    /// (nothing was walked).
    pub fn of_scopes(scopes: impl IntoIterator<Item = Completeness>) -> Completeness {
        let mut scopes = scopes.into_iter();
        let Some(first) = scopes.next() else {
            return Completeness::Indexed;
        };
        if scopes.all(|c| c == first) {
            first
        } else {
            Completeness::Partial
        }
    }
}

impl std::fmt::Display for Completeness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
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
    /// How the answer was produced (§6.2); on the final response when streamed.
    pub completeness: Completeness,
    /// §7.2: on an answer with no result, what the scope holds — `missingDirs`
    /// and `didYouMean` (names the fuzzy scorer admits), never `census`.
    /// Requires `textSearchProvider.explain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explain: Option<Explain>,
    /// §7.2: the prose of `explain`, in its fixed grammar (≤ 240 bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMatch {
    pub uri: Uri,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_matches: Option<Vec<Range>>,
    /// How the path matched (§7.1). Needs `fileSearchProvider.matchKind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_kind: Option<FileMatchKind>,
}

/// The ladder of a path match (PROTOCOL §7.1), best first. The variant
/// order is the ladder, so `Ord` ranks `Exact` first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileMatchKind {
    /// The query equals the file name, a stem of it, or (with a `/`) the
    /// folder-relative path.
    Exact,
    /// The name starts with the query as one contiguous run.
    Prefix,
    /// The query is one contiguous run inside the name, not at its start.
    Basename,
    /// The query's characters occur in the name in order, not contiguously.
    Fuzzy,
    /// The match needs characters of the directory part.
    Path,
}

impl FileMatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileMatchKind::Exact => "exact",
            FileMatchKind::Prefix => "prefix",
            FileMatchKind::Basename => "basename",
            FileMatchKind::Fuzzy => "fuzzy",
            FileMatchKind::Path => "path",
        }
    }
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
            enclosing: None,
            range: Range {
                start: Position {
                    line: 12,
                    character: 0,
                },
                end: Position {
                    line: 12,
                    character: 7,
                },
            },
            ranges: None,
            lines: vec![ContextLine {
                line: 12,
                text: "fn main() {".into(),
                start: None,
                truncated: None,
                is_match: true,
            }],
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["range"]["start"]["line"], 12);
        assert_eq!(v["lines"][0]["isMatch"], true);
        assert!(v.get("ranges").is_none(), "no ranges without mergeWindows");
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
            merge_windows: None,
            files_only: None,
            enclosing: None,
            count_matches: None,
            partial_result_token: Some(ProgressToken::Int(7)),
            work_done_token: None,
        };
        let v = serde_json::to_value(&p).unwrap();
        assert!(
            v.get("isRegExp").is_none(),
            "None options stay off the wire"
        );
        for absent in ["mergeWindows", "filesOnly", "countMatches"] {
            assert!(v.get(absent).is_none(), "{absent} stays off the wire");
        }
        assert!(!p.merge_windows() && !p.files_only() && !p.count_matches());
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
                match_kind: None,
            }],
            limit_hit: None,
            completeness: Completeness::Indexed,
            explain: None,
            note: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        let back: FileSearchResult = serde_json::from_value(v).unwrap();
        assert_eq!(back.results[0].uri.as_str(), "file:///repo/src/main.rs");
        assert_eq!(back.results[0].score, Some(2.5));
        assert_eq!(back.limit_hit, None);
    }

    /// Golden wire shapes for `completeness` (PROTOCOL §6.2): one fixture
    /// per value on each result type, byte-exact, so a renamed variant or
    /// a changed serde attribute fails here before it reaches a client.
    #[test]
    fn completeness_golden() {
        let cases = [
            (Completeness::Indexed, "indexed"),
            (Completeness::Scan, "scan"),
            (Completeness::Partial, "partial"),
        ];
        for (value, wire) in cases {
            let text = TextSearchResult {
                results: vec![],
                limit_hit: None,
                note: None,
                cut: None,
                explain: None,
                completeness: value,
            };
            assert_eq!(
                serde_json::to_string(&text).unwrap(),
                format!(r#"{{"results":[],"completeness":"{wire}"}}"#)
            );
            let file = FileSearchResult {
                results: vec![],
                limit_hit: Some(true),
                completeness: value,
                explain: None,
                note: None,
            };
            assert_eq!(
                serde_json::to_string(&file).unwrap(),
                format!(r#"{{"results":[],"limitHit":true,"completeness":"{wire}"}}"#)
            );
            let back: TextSearchResult =
                serde_json::from_str(&format!(r#"{{"results":[],"completeness":"{wire}"}}"#))
                    .unwrap();
            assert_eq!(back.completeness, value);
            assert_eq!(value.as_str(), wire);
            assert_eq!(value.to_string(), wire);
        }
        // The field is not optional: an answer without it is not a result.
        assert!(serde_json::from_str::<TextSearchResult>(r#"{"results":[]}"#).is_err());
        assert!(serde_json::from_str::<FileSearchResult>(
            r#"{"results":[],"completeness":"complete"}"#
        )
        .is_err());
    }

    /// The multi-scope rule (PROTOCOL §6.2): the value every scope shares,
    /// else `partial`; no scopes is `indexed`.
    #[test]
    fn completeness_of_scopes() {
        use Completeness::*;
        assert_eq!(Completeness::of_scopes([]), Indexed);
        assert_eq!(Completeness::of_scopes([Indexed, Indexed]), Indexed);
        assert_eq!(Completeness::of_scopes([Scan, Scan]), Scan);
        assert_eq!(Completeness::of_scopes([Partial]), Partial);
        assert_eq!(Completeness::of_scopes([Indexed, Scan]), Partial);
        assert_eq!(Completeness::of_scopes([Scan, Partial]), Partial);
        assert_eq!(
            Completeness::of_scopes([Indexed, Partial, Indexed]),
            Partial
        );
    }

    // ------------------------------------------- stage 2: §6.3–§6.6, §7.1

    /// The stage-1 wire shape is unchanged when no stage-2 flag is set: a
    /// stage-1 client (idecli's `answer.go`) reads a stage-2 server's
    /// answers field for field. Golden, byte-exact.
    #[test]
    fn stage2_fields_are_absent_by_default() {
        let r = TextSearchResult {
            results: vec![TextSearchFileResult {
                uri: Uri::from("file:///repo/a.rs"),
                version: None,
                matches: vec![TextSearchMatch {
                    enclosing: None,
                    range: Range {
                        start: Position {
                            line: 1,
                            character: 2,
                        },
                        end: Position {
                            line: 1,
                            character: 5,
                        },
                    },
                    ranges: None,
                    lines: vec![ContextLine {
                        line: 1,
                        text: "x foo".into(),
                        start: None,
                        truncated: None,
                        is_match: true,
                    }],
                }],
                limit_hit: None,
                count: None,
            }],
            limit_hit: None,
            note: None,
            cut: None,
            explain: None,
            completeness: Completeness::Indexed,
        };
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"results":[{"uri":"file:///repo/a.rs","matches":[{"range":{"start":{"line":1,"character":2},"end":{"line":1,"character":5}},"lines":[{"line":1,"text":"x foo","isMatch":true}]}]}],"completeness":"indexed"}"#
        );
        let caps = ServerCapabilities {
            position_encoding: "utf-16".into(),
            text_document_sync: text_document_sync_kind::INCREMENTAL,
            text_search_provider: Some(TextSearchOptions {
                reg_exp: true,
                multiline: true,
                merge_windows: None,
                ordered_cut: None,
                explain: None,
                enclosing: None,
                files_only: None,
            }),
            file_search_provider: Some(FileSearchOptions {
                fuzzy: true,
                match_kind: None,
            }),
            workspace: None,
            symbol_search_provider: None,
        };
        let v = serde_json::to_value(&caps).unwrap();
        assert_eq!(
            v["textSearchProvider"],
            serde_json::json!({"regExp": true, "multiline": true})
        );
        assert_eq!(v["fileSearchProvider"], serde_json::json!({"fuzzy": true}));
    }

    /// The merged-window block of PROTOCOL §6.3: `ranges[0] == range`, each
    /// line once; the client reads `ranges` as camelCase `Range[]`.
    #[test]
    fn merged_block_round_trip() {
        let raw = r#"{
          "range":{"start":{"line":178,"character":15},"end":{"line":178,"character":23}},
          "ranges":[
            {"start":{"line":178,"character":15},"end":{"line":178,"character":23}},
            {"start":{"line":181,"character":67},"end":{"line":181,"character":75}}
          ],
          "lines":[
            {"line":178,"text":"    public int hashCode() {","isMatch":true},
            {"line":179,"text":"        final int prime = 31;","isMatch":false},
            {"line":180,"text":"        int result = 1;","isMatch":false},
            {"line":181,"text":"        result = prime * result + ((filename == null) ? 0 : filename.hashCode());","isMatch":true}
          ]
        }"#;
        let m: TextSearchMatch = serde_json::from_str(raw).unwrap();
        let ranges = m.ranges.as_ref().unwrap();
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0], m.range);
        assert_eq!(m.lines.iter().filter(|l| l.is_match).count(), 2);
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["ranges"][1]["start"]["line"], 181);
    }

    /// `cut` (§6.4) and `explain` (§6.5) golden shapes: camelCase names,
    /// lowercase enum values, optional members off the wire when `None`.
    #[test]
    fn cut_and_explain_golden() {
        let cut = Cut {
            files_with_hits: 48,
            files_left: 174,
            left_kind: LeftKind::Candidates,
            dirs: vec![CutDir {
                uri: Uri::from("file:///repo/src/main"),
                files: 61,
            }],
            dirs_left: 52,
        };
        assert_eq!(
            serde_json::to_string(&cut).unwrap(),
            r#"{"filesWithHits":48,"filesLeft":174,"leftKind":"candidates","dirs":[{"uri":"file:///repo/src/main","files":61}],"dirsLeft":52}"#
        );
        let back: Cut = serde_json::from_str(&serde_json::to_string(&cut).unwrap()).unwrap();
        assert_eq!(back, cut);
        assert_eq!(
            serde_json::to_string(&LeftKind::Files).unwrap(),
            r#""files""#
        );

        let explain = Explain {
            census: Some(vec![
                LiteralCensus {
                    literal: "XStreamPersistor".into(),
                    in_scope: 0,
                    outside_scope: 0,
                },
                LiteralCensus {
                    literal: "XStream".into(),
                    in_scope: 339,
                    outside_scope: 0,
                },
            ]),
            missing_dirs: None,
            did_you_mean: Some(vec![Suggestion {
                name: "XStreamPersister".into(),
                kind: SuggestionKind::File,
                uri: Some(Uri::from(
                    "file:///repo/src/main/src/main/java/org/geoserver/config/util/XStreamPersister.java",
                )),
            }]),
        };
        let v = serde_json::to_value(&explain).unwrap();
        assert_eq!(v["census"][1]["inScope"], 339);
        assert_eq!(v["census"][0]["outsideScope"], 0);
        assert!(v.get("missingDirs").is_none());
        assert_eq!(v["didYouMean"][0]["kind"], "file");
        let back: Explain = serde_json::from_value(v).unwrap();
        assert_eq!(back, explain);
        assert_eq!(
            serde_json::to_string(&SuggestionKind::Declaration).unwrap(),
            r#""declaration""#
        );
        assert_eq!(
            Explain::default(),
            Explain {
                census: None,
                missing_dirs: None,
                did_you_mean: None
            }
        );
    }

    /// `matchKind` (§7.1): lowercase on the wire, the variant order is the
    /// ladder (exact first), `count` and the request flags round-trip.
    #[test]
    fn match_kind_and_files_only_golden() {
        use FileMatchKind::*;
        for (k, wire) in [
            (Exact, "exact"),
            (Prefix, "prefix"),
            (Basename, "basename"),
            (Fuzzy, "fuzzy"),
            (Path, "path"),
        ] {
            assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{wire}\""));
            assert_eq!(k.as_str(), wire);
            let back: FileMatchKind = serde_json::from_str(&format!("\"{wire}\"")).unwrap();
            assert_eq!(back, k);
        }
        assert!(Exact < Prefix && Prefix < Basename && Basename < Fuzzy && Fuzzy < Path);
        let m: FileMatch = serde_json::from_str(
            r#"{"uri":"file:///repo/LayerInfoImpl.java","score":50.1,"matchKind":"exact"}"#,
        )
        .unwrap();
        assert_eq!(m.match_kind, Some(Exact));

        let p: TextSearchParams = serde_json::from_str(
            r#"{"query":"LinkedHashSet","filesOnly":true,"countMatches":true,"maxResults":200}"#,
        )
        .unwrap();
        assert!(p.files_only() && p.count_matches() && !p.merge_windows());
        let f: TextSearchFileResult =
            serde_json::from_str(r#"{"uri":"file:///repo/a.java","matches":[],"count":2}"#)
                .unwrap();
        assert_eq!(f.count, Some(2));
        assert!(f.matches.is_empty());
        assert_eq!(
            serde_json::to_string(&f).unwrap(),
            r#"{"uri":"file:///repo/a.java","matches":[],"count":2}"#
        );
    }

    /// The constants the renderer and the server share.
    #[test]
    fn stage2_limits() {
        assert_eq!(NOTE_MAX_LINES, 3);
        assert_eq!(NOTE_MAX_BYTES, 240);
        assert_eq!(CUT_MAX_DIRS, 8);
        assert_eq!(CENSUS_MAX_LITERALS, 5);
        assert_eq!(DID_YOU_MEAN_MAX, 3);
    }

    // ------------------------------------------------- index status §4.2

    #[test]
    fn index_status_result_matches_protocol_example() {
        // The exact response from PROTOCOL.md §4.2.
        let r = IndexStatusResult {
            folders: vec![FolderIndexStatus {
                uri: Uri::from("file:///home/user/repo"),
                state: IndexState::Building,
                start_mode: StartMode::Cold,
                files: 18_204,
                pending_ops: 1_796,
                data_bytes: 73_400_320,
                since: Some("2026-11-03T14:02:11.418Z".into()),
                error: None,
                symbol_index: Some(SymbolIndexStatus {
                    state: IndexState::Building,
                    files: 11_904,
                }),
            }],
        };
        let v = serde_json::to_value(&r).unwrap();
        let expected = serde_json::json!({
            "folders": [{
                "uri": "file:///home/user/repo",
                "state": "building",
                "startMode": "cold",
                "files": 18204,
                "pendingOps": 1796,
                "dataBytes": 73400320,
                "since": "2026-11-03T14:02:11.418Z",
                "symbolIndex": {"state": "building", "files": 11904}
            }]
        });
        assert_eq!(v, expected);
        let back: IndexStatusResult = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn index_status_optional_fields_stay_off_the_wire() {
        let f = FolderIndexStatus {
            uri: Uri::from("file:///repo"),
            state: IndexState::Ready,
            start_mode: StartMode::Hot,
            files: 1,
            pending_ops: 0,
            data_bytes: 0,
            since: None,
            error: None,
            symbol_index: None,
        };
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["state"], "ready");
        assert_eq!(v["startMode"], "hot");
        for absent in ["since", "error", "symbolIndex"] {
            assert!(v.get(absent).is_none(), "{absent} must be absent");
        }
    }

    #[test]
    fn index_ready_params_from_protocol_example() {
        // The exact notification params from PROTOCOL.md §4.2.
        let raw = r#"{
          "uri": "file:///home/user/repo",
          "files": 20000,
          "elapsedMs": 1180,
          "startMode": "cold"
        }"#;
        let p: IndexReadyParams = serde_json::from_str(raw).unwrap();
        assert_eq!(p.files, 20_000);
        assert_eq!(p.elapsed_ms, 1180);
        assert_eq!(p.start_mode, StartMode::Cold);
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["elapsedMs"], 1180);
        assert_eq!(v["startMode"], "cold");
    }

    // ------------------------------------------------- symbol search §13

    #[test]
    fn symbol_search_params_and_result_match_protocol_example() {
        let raw = r#"{"query": "Catalog.getLayer", "matchMode": "prefix", "kinds": ["method"],
                      "maxResults": 20, "sourceLines": 12}"#;
        let p: SymbolSearchParams = serde_json::from_str(raw).unwrap();
        assert_eq!(p.match_mode, Some(MatchMode::Prefix));
        assert_eq!(p.source_lines, Some(SourceLines::Lines(12)));
        let all: SymbolSearchParams =
            serde_json::from_str(r#"{"query": "X", "sourceLines": "all"}"#).unwrap();
        assert_eq!(
            all.source_lines,
            Some(SourceLines::Word(SourceLinesWord::All))
        );
        assert!(serde_json::from_str::<SymbolSearchParams>(
            r#"{"query": "X", "sourceLines": "some"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<SymbolSearchParams>(
            r#"{"query": "X", "matchMode": "regex"}"#
        )
        .is_err());

        let r = SymbolSearchResult {
            results: vec![SymbolInformation {
                name: "getLayer".into(),
                kind: "method".into(),
                container: Some("Catalog".into()),
                uri: Uri::from("file:///repo/src/Catalog.java"),
                range: Range { start: Position { line: 41, character: 0 }, end: Position { line: 44, character: 5 } },
                selection_range: Some(Range { start: Position { line: 41, character: 21 }, end: Position { line: 41, character: 29 } }),
                language: "java".into(),
                signature: Some("public LayerInfo getLayer(String id) {".into()),
                source: Some(SymbolSource { start_line: 41, text: "    public LayerInfo getLayer(String id) {\n        return layers.get(id);\n    }".into(), truncated: None }),
            }],
            total: 1,
            limit_hit: None,
            completeness: Completeness::Indexed,
            note: None,
            index: SymbolIndexStatus { state: IndexState::Ready, files: 11_904 },
        };
        let v = serde_json::to_value(&r).unwrap();
        let expected = serde_json::json!({
            "results": [{
                "name": "getLayer", "kind": "method", "container": "Catalog",
                "uri": "file:///repo/src/Catalog.java",
                "range": {"start": {"line": 41, "character": 0}, "end": {"line": 44, "character": 5}},
                "selectionRange": {"start": {"line": 41, "character": 21}, "end": {"line": 41, "character": 29}},
                "language": "java",
                "signature": "public LayerInfo getLayer(String id) {",
                "source": {"startLine": 41, "text": "    public LayerInfo getLayer(String id) {\n        return layers.get(id);\n    }"}
            }],
            "total": 1,
            "completeness": "indexed",
            "index": {"state": "ready", "files": 11904}
        });
        assert_eq!(v, expected);
        let back: SymbolSearchResult = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn enclosing_declaration_is_named_in_on_the_wire() {
        let m = TextSearchMatch {
            range: Range {
                start: Position {
                    line: 3,
                    character: 1,
                },
                end: Position {
                    line: 3,
                    character: 4,
                },
            },
            ranges: None,
            lines: Vec::new(),
            enclosing: Some(EnclosingDeclaration {
                name: "run".into(),
                kind: "method".into(),
                container: Some("Job".into()),
                range: Range {
                    start: Position {
                        line: 1,
                        character: 0,
                    },
                    end: Position {
                        line: 9,
                        character: 1,
                    },
                },
            }),
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["in"]["name"], "run");
        assert_eq!(v["in"]["container"], "Job");
        let plain: TextSearchMatch = serde_json::from_value(serde_json::json!({
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "lines": []
        })).unwrap();
        assert!(plain.enclosing.is_none());
    }
}
