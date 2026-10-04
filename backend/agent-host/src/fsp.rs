// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The FSP client: the host's indexed search engine
//! (docs/file-search.md). One supervised `fsp-server` child on stdio,
//! LSP-framed JSON-RPC via the vendored `fsp-types` crate. Folders
//! register from session lifecycle (the indexing ask); overlays feed
//! from the document channels, so FSP search observes unsaved edits.
//! A crashed server respawns with folder and overlay replay; repeated
//! stillborn spawns demote to the naive engine for the host's
//! lifetime.
//!
//! State discipline: THE engine state is one persistent VALUE behind
//! a swap latch, exactly like `Host` — readers snapshot, writers
//! derive the next value. The one live-mutable end is a connection's
//! in-flight table (the `lsp_inflight` precedent).

use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use fsp_types::jsonrpc::RawMessage;
use fsp_types::protocol as wire;
use serde_json::{json, Value};

const INIT_ID: i64 = 0;
/// Consecutive stillborn connections (spawn failed, or died before the
/// handshake) after which the engine demotes for the host's lifetime.
const DEMOTE_AFTER: u32 = 5;
const RESPAWN_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

// ------------------------------------------------------------- discovery

/// The discovery ladder (docs/file-search.md §5): env override, a
/// sibling of the running binary, PATH, then the sibling checkout's
/// build output — both profiles probed, the newer build wins.
pub fn discover_binary() -> Option<String> {
    if std::env::var("HIMARK_FSP").is_ok_and(|flag| flag == "0") {
        return None;
    }
    if let Ok(command) = std::env::var("HIMARK_FSP_BIN") {
        if !command.is_empty() {
            return Some(command);
        }
    }
    let exe = std::env::current_exe().ok();
    if let Some(dir) = exe.as_deref().and_then(Path::parent) {
        let sibling = dir.join("fsp-server");
        if crate::claude::is_executable(&sibling) {
            return Some(sibling.to_string_lossy().into_owned());
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("fsp-server");
            if crate::claude::is_executable(&candidate) {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    sibling_checkout(exe.as_deref()?)
}

/// Dev feedback loop: a `file-search-protocol` checkout beside ANY
/// ancestor of the running binary — `<repo>/target/<profile>/` runs
/// and the Xcode bundle under `apps/himark-apple/build/` alike both
/// live under the directory that also holds the server checkout.
/// RELEASE wins over debug whenever it exists: the sidecar is
/// latency-critical (a debug fileSearch over a monorepo takes
/// seconds where release takes milliseconds — quick-open cancels
/// per keystroke and nothing ever lands), and an fsp developer
/// iterating on the server pins `HIMARK_FSP_BIN` instead.
fn sibling_checkout(exe: &Path) -> Option<String> {
    ["release", "debug"].iter().find_map(|profile| {
        exe.ancestors()
            .skip(1)
            .map(|ancestor| {
                ancestor
                    .join("file-search-protocol")
                    .join("target")
                    .join(profile)
                    .join("fsp-server")
            })
            .find(|candidate| crate::claude::is_executable(candidate))
            .map(|candidate| candidate.to_string_lossy().into_owned())
    })
}

/// Only git repositories register as search folders — a `.git` in
/// the directory or an ancestor. What fits git fits the index:
/// bounded, ignore-ruled, worth watching. Anything else is either
/// explodable (`~/Downloads`, a home directory — the 2026-09-30 TCC
/// prompt) or too small to index; the server scan-serves those
/// scopes per request.
pub(crate) fn indexable(dir: &Path) -> bool {
    dir.ancestors()
        .any(|ancestor| ancestor.join(".git").exists())
}

// ------------------------------------------------------------- the engine

/// Snapshot of every open document overlay: (resource uri, full text).
/// Read at (re)spawn to replay `didOpen`s from state the host owns.
pub(crate) type OverlaySource = Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

#[derive(Debug)]
pub(crate) enum FspFailure {
    /// The connection died (or could not be established). The caller
    /// retries against a respawned server or falls back for this request.
    Gone,
    /// The server refused the request: bad pattern, bad params.
    Invalid(String),
}

/// A file's positioned matches in the host vocabulary —
/// `hifind::LineMatch` is the shared match shape, so FSP and the naive
/// walk convert to `Location` identically.
pub(crate) struct FileMatches {
    pub(crate) path: PathBuf,
    pub(crate) matches: Vec<hifind::LineMatch>,
}

/// The engine state: a persistent value swapped under the latch.
#[derive(Clone)]
struct EngineState {
    /// Registered search folders — the union of the listed sessions'
    /// working directories, reconciled whole (docs/file-search.md §5).
    folders: rpds::HashTrieSetSync<PathBuf>,
    /// Overlay versions on the LIVE connection (fsp uri → version);
    /// rebuilt from the replay at every spawn.
    overlays: rpds::HashTrieMapSync<String, i64>,
    connection: Option<Arc<Connection>>,
    last_spawn: Option<std::time::Instant>,
    stillborn: u32,
    demoted: bool,
}

pub(crate) struct Engine {
    binary: String,
    data_dir: PathBuf,
    overlay_source: OverlaySource,
    /// The swap latch — never a region to think inside.
    state: Mutex<Arc<EngineState>>,
}

impl Engine {
    pub(crate) fn new(
        binary: String,
        data_dir: PathBuf,
        overlay_source: OverlaySource,
    ) -> Arc<Engine> {
        Arc::new(Engine {
            binary,
            data_dir,
            overlay_source,
            state: Mutex::new(Arc::new(EngineState {
                folders: rpds::HashTrieSetSync::new_sync(),
                overlays: rpds::HashTrieMapSync::new_sync(),
                connection: None,
                last_spawn: None,
                stillborn: 0,
                demoted: false,
            })),
        })
    }

    fn snapshot(&self) -> Arc<EngineState> {
        Arc::clone(
            &self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn update<R>(&self, mutate: impl FnOnce(&mut EngineState) -> R) -> R {
        let mut held = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut next = EngineState::clone(&held);
        let result = mutate(&mut next);
        *held = Arc::new(next);
        result
    }

    pub(crate) fn available(&self) -> bool {
        !self.snapshot().demoted
    }

    /// Reconcile the folder set from session lifecycle. The diff rides
    /// `workspace/didChangeSearchFolders` on a live connection; the
    /// registry seeds `initialize` at the next spawn otherwise.
    pub(crate) fn sync_folders(&self, desired: Vec<PathBuf>) {
        self.update(|state| {
            let added: Vec<PathBuf> = desired
                .iter()
                .filter(|folder| !state.folders.contains(*folder))
                .cloned()
                .collect();
            let removed: Vec<PathBuf> = state
                .folders
                .iter()
                .filter(|folder| !desired.contains(folder))
                .cloned()
                .collect();
            for folder in &removed {
                state.folders.remove_mut(folder);
            }
            for folder in &added {
                state.folders.insert_mut(folder.clone());
            }
            if added.is_empty() && removed.is_empty() {
                return;
            }
            if let Some(connection) = live(state) {
                connection.change_folders(&added, &removed);
            }
        });
    }

    /// The live connection, respawned when the previous one died.
    /// `None` while demoted or inside the respawn backoff window.
    pub(crate) fn ensure(self: &Arc<Self>) -> Option<Arc<Connection>> {
        {
            let state = self.snapshot();
            if state.demoted {
                return None;
            }
            if let Some(connection) = live(&state) {
                return Some(connection);
            }
            if state.connection.is_some()
                && state
                    .last_spawn
                    .is_some_and(|at| at.elapsed() < RESPAWN_BACKOFF)
            {
                return None;
            }
        }
        // Spawn OUTSIDE the latch; install first-wins — a losing spawn
        // drops its senders, the child reads EOF and exits.
        let folders: Vec<PathBuf> = self.snapshot().folders.iter().cloned().collect();
        let overlays = (self.overlay_source)();
        let spawned = Connection::spawn(self, &folders, &overlays);
        self.update(|state| match spawned {
            Some((connection, versions)) => {
                state.last_spawn = Some(std::time::Instant::now());
                if state.demoted {
                    return None;
                }
                if let Some(held) = live(state) {
                    return Some(held);
                }
                state.connection = Some(Arc::clone(&connection));
                state.overlays = versions;
                Some(connection)
            }
            None => {
                state.last_spawn = Some(std::time::Instant::now());
                count_stillborn(state, &self.binary);
                None
            }
        })
    }

    // ---------------------------------------------------------- overlays

    pub(crate) fn feed_open(&self, uri: &str, text: &str) {
        let Some(fsp_uri) = fsp_uri(uri) else { return };
        self.update(|state| {
            let Some(connection) = live(state) else {
                return;
            };
            if state.overlays.contains_key(&fsp_uri) {
                return;
            }
            state.overlays.insert_mut(fsp_uri.clone(), 1);
            connection.overlay_open(&fsp_uri, 1, text);
        });
    }

    pub(crate) fn feed_change(&self, uri: &str, operation: &himark_ahp_ext_types::documents::TextOperation) {
        let Some(fsp_uri) = fsp_uri(uri) else { return };
        // Non-utf-8 servers get whole-text sync: incremental ranges in
        // byte columns would land wrong (docs/file-search.md §5).
        let full = |connection: &Connection| {
            (self.overlay_source)()
                .into_iter()
                .find(|(open, _)| open == uri)
                .map(|(_, text)| text)
                .filter(|_| !connection.utf8())
        };
        self.update(|state| {
            let Some(connection) = live(state) else {
                return;
            };
            let Some(version) = state.overlays.get(&fsp_uri).map(|held| held + 1) else {
                return;
            };
            state.overlays.insert_mut(fsp_uri.clone(), version);
            match full(&connection) {
                Some(text) => connection.overlay_replace(&fsp_uri, version, &text),
                None => connection.overlay_change(&fsp_uri, version, operation),
            }
        });
    }

    pub(crate) fn feed_close(&self, uri: &str) {
        let Some(fsp_uri) = fsp_uri(uri) else { return };
        self.update(|state| {
            let Some(connection) = live(state) else {
                return;
            };
            if state.overlays.contains_key(&fsp_uri) {
                state.overlays.remove_mut(&fsp_uri);
                connection.overlay_close(&fsp_uri);
            }
        });
    }

    // ---------------------------------------------------------- searches

    /// `workspace/textSearch`, streamed: per-file batches through
    /// `sink`, the answer carries the truncation flag. The `leash`
    /// maps to `$/cancelRequest` while the request runs.
    pub(crate) async fn text_search(
        self: &Arc<Self>,
        dirs: &[PathBuf],
        query: &str,
        regex: bool,
        case_sensitive: bool,
        max_results: usize,
        max_results_per_file: Option<u32>,
        leash: Arc<AtomicBool>,
        sink: Arc<dyn Fn(Vec<FileMatches>) + Send + Sync>,
    ) -> Result<bool, FspFailure> {
        let connection = self.ensure().ok_or(FspFailure::Gone)?;
        let mut params = json!({
            "query": query,
            "isRegExp": regex,
            "isCaseSensitive": case_sensitive,
            "dirs": dirs.iter().map(|dir| encoded_uri(dir)).collect::<Vec<_>>(),
            "maxResults": max_results as u32,
        });
        if let Some(per_file) = max_results_per_file {
            params["maxResultsPerFile"] = json!(per_file);
        }
        let progress = Arc::new({
            // The encoding is read at MAPPING time, not at ask time: a
            // batch can only follow the handshake that settles it, while
            // the ask may race a fresh spawn. (The cycle through the
            // caller table breaks when the response removes the entry.)
            let connection = Arc::clone(&connection);
            move |value: Value| {
                let Ok(batch) = serde_json::from_value::<Vec<wire::TextSearchFileResult>>(value)
                else {
                    return;
                };
                let mapped: Vec<FileMatches> = batch
                    .iter()
                    .filter_map(|file| map_file_result(file, connection.utf8()))
                    .collect();
                if !mapped.is_empty() {
                    sink(mapped);
                }
            }
        });
        match call(
            &connection,
            "workspace/textSearch",
            params,
            Some(progress),
            leash,
        )
        .await?
        {
            Value::Null => Ok(true), // cancelled
            result => {
                let result: wire::TextSearchResult =
                    serde_json::from_value(result).map_err(|_| FspFailure::Gone)?;
                debug_assert!(result.results.is_empty(), "everything streams");
                Ok(result.limit_hit.unwrap_or(false))
            }
        }
    }

    /// `workspace/fileSearch`: fuzzy path search. Answers matched paths
    /// (score-ordered — the caller re-sorts to the wire's path order).
    pub(crate) async fn file_search(
        self: &Arc<Self>,
        dirs: &[PathBuf],
        query: &str,
        case_sensitive: bool,
        max_results: usize,
        leash: Arc<AtomicBool>,
    ) -> Result<(Vec<PathBuf>, bool), FspFailure> {
        let connection = self.ensure().ok_or(FspFailure::Gone)?;
        let params = json!({
            "query": query,
            "isCaseSensitive": case_sensitive,
            "dirs": dirs.iter().map(|dir| encoded_uri(dir)).collect::<Vec<_>>(),
            "maxResults": max_results as u32,
        });
        match call(&connection, "workspace/fileSearch", params, None, leash).await? {
            Value::Null => Ok((Vec::new(), true)), // cancelled
            result => {
                let result: wire::FileSearchResult =
                    serde_json::from_value(result).map_err(|_| FspFailure::Gone)?;
                let paths = result
                    .results
                    .iter()
                    .filter_map(|found| found.uri.to_file_path().ok())
                    .collect();
                Ok((paths, result.limit_hit.unwrap_or(false)))
            }
        }
    }
}

fn live(state: &EngineState) -> Option<Arc<Connection>> {
    state
        .connection
        .as_ref()
        .filter(|connection| !connection.dead.load(Ordering::Relaxed))
        .cloned()
}

fn count_stillborn(state: &mut EngineState, binary: &str) {
    state.stillborn += 1;
    if state.stillborn >= DEMOTE_AFTER && !state.demoted {
        state.demoted = true;
        eprintln!(
            "[fsp] {} stillborn spawns of {binary} — demoting to the naive search engine for this host's lifetime",
            state.stillborn
        );
    }
}

/// One request with leash-driven cancellation. Answers `Value::Null`
/// for a server-side `RequestCancelled` — the callers' truncated case.
async fn call(
    connection: &Arc<Connection>,
    method: &str,
    params: Value,
    progress: Option<ProgressSink>,
    leash: Arc<AtomicBool>,
) -> Result<Value, FspFailure> {
    let (id, answer) = connection.request(method, params, progress);
    let watcher = tokio::spawn({
        let connection = Arc::clone(connection);
        async move {
            loop {
                if leash.load(Ordering::Relaxed) {
                    connection.notify("$/cancelRequest", json!({ "id": id }));
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    });
    let outcome = answer.await.unwrap_or(Answer::Gone);
    watcher.abort();
    match outcome {
        Answer::Result(value) => Ok(value),
        Answer::Error(-32800, _) => Ok(Value::Null),
        Answer::Error(_, message) => Err(FspFailure::Invalid(message)),
        Answer::Gone => Err(FspFailure::Gone),
    }
}

// ------------------------------------------------------- the connection

enum Answer {
    Result(Value),
    Error(i64, String),
    Gone,
}

type ProgressSink = Arc<dyn Fn(Value) + Send + Sync>;

struct Caller {
    answer: tokio::sync::oneshot::Sender<Answer>,
    progress: Option<ProgressSink>,
}

enum Out {
    Frame(Vec<u8>),
    /// The handshake finished: flush what buffered behind it.
    Ready,
}

pub(crate) struct Connection {
    out: std::sync::mpsc::Sender<Out>,
    next_id: AtomicI64,
    /// The in-flight table — the one live-mutable end, held only for
    /// insert/remove (the `lsp_inflight` precedent).
    callers: Mutex<std::collections::HashMap<i64, Caller>>,
    utf8: AtomicBool,
    dead: AtomicBool,
}

impl Connection {
    /// Spawns the server, writes `initialize` (current folders), queues
    /// the overlay replay behind the handshake, and starts the writer,
    /// reaper and reader threads. Answers the connection and the
    /// replayed overlay versions.
    fn spawn(
        engine: &Arc<Engine>,
        folders: &[PathBuf],
        overlays: &[(String, String)],
    ) -> Option<(Arc<Connection>, rpds::HashTrieMapSync<String, i64>)> {
        let _ = std::fs::create_dir_all(&engine.data_dir);
        // The command may carry an interpreter ("python3 stub.py"), as
        // language-server commands do.
        let mut parts = engine.binary.split_whitespace();
        let mut child = std::process::Command::new(parts.next()?)
            .args(parts)
            .env("FSP_DATA_DIR", &engine.data_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mut stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        std::thread::spawn(move || {
            let _ = child.wait(); // reap; the child exits on stdin EOF
        });

        // The handshake bypasses the writer's buffering; everything
        // else queues behind `Ready`.
        let initialize = frame(&json!({
            "jsonrpc": "2.0", "id": INIT_ID, "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "clientInfo": { "name": crate::SERVER_NAME, "version": crate::SERVER_VERSION },
                "capabilities": { "positionEncodings": ["utf-8"], "partialResults": true },
                "searchFolders": folders
                    .iter()
                    .map(|folder| json!({ "uri": encoded_uri(folder) }))
                    .collect::<Vec<_>>(),
            },
        }));
        stdin.write_all(&initialize).ok()?;
        stdin.flush().ok()?;

        let (out, frames) = std::sync::mpsc::channel::<Out>();
        std::thread::Builder::new()
            .name("fsp-writer".to_owned())
            .spawn(move || {
                let mut buffered: Vec<Vec<u8>> = Vec::new();
                let mut ready = false;
                for next in frames {
                    let held = match next {
                        Out::Ready => {
                            ready = true;
                            std::mem::take(&mut buffered)
                        }
                        Out::Frame(bytes) if ready => vec![bytes],
                        Out::Frame(bytes) => {
                            buffered.push(bytes);
                            continue;
                        }
                    };
                    for bytes in held {
                        if stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err() {
                            return; // the reader hears the death
                        }
                    }
                }
            })
            .ok()?;

        let connection = Arc::new(Connection {
            out,
            next_id: AtomicI64::new(INIT_ID + 1),
            callers: Mutex::new(std::collections::HashMap::new()),
            utf8: AtomicBool::new(false),
            dead: AtomicBool::new(false),
        });
        let mut versions = rpds::HashTrieMapSync::new_sync();
        for (uri, text) in overlays {
            if let Some(fsp_uri) = fsp_uri(uri) {
                if !versions.contains_key(&fsp_uri) {
                    connection.overlay_open(&fsp_uri, 1, text);
                    versions.insert_mut(fsp_uri, 1);
                }
            }
        }
        std::thread::Builder::new()
            .name("fsp-reader".to_owned())
            .spawn({
                let connection = Arc::clone(&connection);
                let engine = Arc::downgrade(engine);
                move || reader_loop(&connection, engine, stdout)
            })
            .ok()?;
        Some((connection, versions))
    }

    pub(crate) fn notify(&self, method: &str, params: Value) {
        let _ = self.out.send(Out::Frame(frame(
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        )));
    }

    fn request(
        &self,
        method: &str,
        mut params: Value,
        progress: Option<ProgressSink>,
    ) -> (i64, tokio::sync::oneshot::Receiver<Answer>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if progress.is_some() {
            params["partialResultToken"] = json!(id);
        }
        let (answer, receiver) = tokio::sync::oneshot::channel();
        self.callers
            .lock()
            .expect("fsp callers")
            .insert(id, Caller { answer, progress });
        let _ = self.out.send(Out::Frame(frame(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )));
        // A dead connection's reader already drained the table; a slot
        // inserted after that drain resolves here instead.
        if self.dead.load(Ordering::Relaxed) {
            if let Some(caller) = self.callers.lock().expect("fsp callers").remove(&id) {
                let _ = caller.answer.send(Answer::Gone);
            }
        }
        (id, receiver)
    }

    fn change_folders(&self, added: &[PathBuf], removed: &[PathBuf]) {
        let folder = |path: &PathBuf| json!({ "uri": encoded_uri(path) });
        self.notify(
            "workspace/didChangeSearchFolders",
            json!({ "event": {
                "added": added.iter().map(folder).collect::<Vec<_>>(),
                "removed": removed.iter().map(folder).collect::<Vec<_>>(),
            }}),
        );
    }

    fn overlay_open(&self, fsp_uri: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": fsp_uri, "version": version, "text": text } }),
        );
    }

    fn overlay_change(
        &self,
        fsp_uri: &str,
        version: i64,
        operation: &himark_ahp_ext_types::documents::TextOperation,
    ) {
        // Replacements in reverse order: each LSP-style change addresses
        // text untouched by the changes before it (the lsp.rs recipe).
        let changes: Vec<Value> = operation
            .replacements
            .iter()
            .rev()
            .map(|replacement| {
                json!({
                    "range": {
                        "start": { "line": replacement.range.start.line,
                                    "character": replacement.range.start.character },
                        "end": { "line": replacement.range.end.line,
                                  "character": replacement.range.end.character },
                    },
                    "text": replacement.text,
                })
            })
            .collect();
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": fsp_uri, "version": version },
                "contentChanges": changes,
            }),
        );
    }

    fn overlay_replace(&self, fsp_uri: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": fsp_uri, "version": version },
                "contentChanges": [ { "text": text } ],
            }),
        );
    }

    fn overlay_close(&self, fsp_uri: &str) {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": fsp_uri } }),
        );
    }

    fn utf8(&self) -> bool {
        self.utf8.load(Ordering::Relaxed)
    }
}

fn reader_loop(
    connection: &Arc<Connection>,
    engine: std::sync::Weak<Engine>,
    stdout: std::process::ChildStdout,
) {
    let mut reader = BufReader::new(stdout);
    let mut ready = false;
    loop {
        let Ok(Some(body)) = fsp_types::framing::read_message(&mut reader) else {
            break;
        };
        let Ok(raw) = serde_json::from_slice::<RawMessage>(&body) else {
            continue;
        };
        match (raw.id, raw.method) {
            (Some(fsp_types::jsonrpc::Id::Int(INIT_ID)), None) => {
                if raw.error.is_some() {
                    break;
                }
                let encoding = raw
                    .result
                    .as_ref()
                    .and_then(|result| result.pointer("/capabilities/positionEncoding"))
                    .and_then(Value::as_str)
                    .unwrap_or("utf-16");
                connection
                    .utf8
                    .store(encoding == "utf-8", Ordering::Relaxed);
                connection.notify("initialized", json!({}));
                let _ = connection.out.send(Out::Ready);
                ready = true;
                if let Some(engine) = engine.upgrade() {
                    engine.update(|state| state.stillborn = 0);
                }
            }
            (Some(fsp_types::jsonrpc::Id::Int(id)), None) => {
                let caller = connection.callers.lock().expect("fsp callers").remove(&id);
                if let Some(caller) = caller {
                    let _ = caller.answer.send(match raw.error {
                        Some(error) => Answer::Error(error.code, error.message),
                        None => Answer::Result(raw.result.unwrap_or(Value::Null)),
                    });
                }
            }
            (None, Some(method)) if method == "$/progress" => {
                let Some(params) = raw.params else { continue };
                let Some(token) = params.get("token").and_then(Value::as_i64) else {
                    continue;
                };
                let sink = connection
                    .callers
                    .lock()
                    .expect("fsp callers")
                    .get(&token)
                    .and_then(|caller| caller.progress.clone());
                if let Some(sink) = sink {
                    sink(params.get("value").cloned().unwrap_or(Value::Null));
                }
            }
            _ => {}
        }
    }
    // Death: every waiter answers Gone; a pre-handshake death counts
    // toward demotion.
    connection.dead.store(true, Ordering::Relaxed);
    eprintln!("[fsp] the search server connection closed");
    let drained: Vec<Caller> = {
        let mut callers = connection.callers.lock().expect("fsp callers");
        callers.drain().map(|(_, caller)| caller).collect()
    };
    for caller in drained {
        let _ = caller.answer.send(Answer::Gone);
    }
    if !ready {
        if let Some(engine) = engine.upgrade() {
            engine.update(|state| count_stillborn(state, &engine.binary));
        }
    }
}

// ------------------------------------------------------------- mapping

/// FSP file result → the shared match vocabulary. Multiline is never
/// requested, so ranges stay on one line; the match line's text ships
/// in the result, and the shared `context_window` reslices it so
/// windows look identical whichever engine produced them.
fn map_file_result(file: &wire::TextSearchFileResult, utf8: bool) -> Option<FileMatches> {
    let path = file.uri.to_file_path().ok()?;
    let matches: Vec<hifind::LineMatch> = file
        .matches
        .iter()
        .filter_map(|found| {
            let line_text = &found
                .lines
                .iter()
                .find(|line| line.is_match && line.line == found.range.start.line)?
                .text;
            let start = byte_column(line_text, found.range.start.character, utf8);
            let end = byte_column(line_text, found.range.end.character, utf8).max(start);
            let (context, context_column_start) = hifind::context_window(line_text, start);
            Some(hifind::LineMatch {
                line: found.range.start.line,
                column: start as u32,
                length: (end - start) as u32,
                context,
                context_column_start: context_column_start as u32,
            })
        })
        .collect();
    if matches.is_empty() {
        return None;
    }
    Some(FileMatches { path, matches })
}

/// A protocol column → byte offset within the line's text.
fn byte_column(line: &str, character: u32, utf8: bool) -> usize {
    if utf8 {
        let mut offset = (character as usize).min(line.len());
        while offset > 0 && !line.is_char_boundary(offset) {
            offset -= 1;
        }
        return offset;
    }
    let mut units_left = character;
    for (i, c) in line.char_indices() {
        if units_left == 0 {
            return i;
        }
        let width = c.len_utf16() as u32;
        if width > units_left {
            return i;
        }
        units_left -= width;
    }
    line.len()
}

/// himark resource uri → a properly percent-encoded FSP uri.
fn fsp_uri(uri: &str) -> Option<String> {
    Some(encoded_uri(&crate::uris::file_path(uri)?))
}

fn encoded_uri(path: &Path) -> String {
    crate::uris::encoded_file_uri(path)
}

fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_honors_the_override_and_the_off_switch() {
        std::env::set_var("HIMARK_FSP_BIN", "/tmp/fsp-stub");
        assert_eq!(discover_binary().as_deref(), Some("/tmp/fsp-stub"));
        std::env::set_var("HIMARK_FSP", "0");
        assert_eq!(discover_binary(), None);
        std::env::remove_var("HIMARK_FSP");
        std::env::remove_var("HIMARK_FSP_BIN");
    }

    #[test]
    fn the_checkout_probe_walks_every_ancestor() {
        let dir = std::env::temp_dir().join(format!("fsp-probe-{}", std::process::id()));
        let checkout = dir.join("Projects/file-search-protocol/target/debug");
        std::fs::create_dir_all(&checkout).expect("mkdir");
        let server = checkout.join("fsp-server");
        std::fs::write(&server, b"#!/bin/sh\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        // The Xcode bundle shape — nowhere near <repo>/target/<profile>.
        let exe = dir.join(
            "Projects/himark-jb/apps/himark-apple/build/Build/Products/Release/\
             himark-macOS.app/Contents/MacOS/himark-agent-host",
        );
        assert_eq!(
            sibling_checkout(&exe).as_deref(),
            Some(server.to_string_lossy().as_ref()),
        );

        // Release wins whenever it exists — the sidecar is
        // latency-critical, and `cargo test` in the checkout must not
        // shadow it with a fresh debug build.
        let release = dir.join("Projects/file-search-protocol/target/release");
        std::fs::create_dir_all(&release).expect("mkdir");
        let fast = release.join("fsp-server");
        std::fs::write(&fast, b"#!/bin/sh\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fast, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        assert_eq!(
            sibling_checkout(&exe).as_deref(),
            Some(fast.to_string_lossy().as_ref()),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn byte_columns_convert_both_encodings() {
        // "𐐷bb": 𐐷 = 4 bytes, 2 utf-16 units.
        let line = "𐐷bb";
        assert_eq!(byte_column(line, 4, true), 4);
        assert_eq!(byte_column(line, 2, false), 4);
        assert_eq!(byte_column(line, 3, false), 5);
        assert_eq!(byte_column(line, 99, true), line.len());
        assert_eq!(byte_column(line, 99, false), line.len());
        // A utf-8 column inside 𐐷 snaps to the character start.
        assert_eq!(byte_column(line, 2, true), 0);
    }

    #[test]
    fn file_results_map_to_the_shared_match_shape() {
        let file = wire::TextSearchFileResult {
            uri: fsp_types::uri::Uri::from_file_path(Path::new("/tmp/a.rs")),
            version: None,
            limit_hit: None,
            matches: vec![wire::TextSearchMatch {
                range: wire::Range {
                    start: wire::Position {
                        line: 3,
                        character: 4,
                    },
                    end: wire::Position {
                        line: 3,
                        character: 10,
                    },
                },
                lines: vec![wire::ContextLine {
                    line: 3,
                    text: "the needle here".to_owned(),
                    is_match: true,
                }],
            }],
        };
        let mapped = map_file_result(&file, true).expect("mapped");
        assert_eq!(mapped.path, PathBuf::from("/tmp/a.rs"));
        let found = &mapped.matches[0];
        assert_eq!((found.line, found.column, found.length), (3, 4, 6));
        assert_eq!(found.context, "the needle here");
        assert_eq!(found.context_column_start, 0);
    }

    #[test]
    fn folder_reconciliation() {
        let engine = Engine::new(
            "false".to_owned(),
            std::env::temp_dir().join("fsp-none"),
            Arc::new(Vec::new),
        );
        let a = PathBuf::from("/w/a");
        let b = PathBuf::from("/w/b");
        engine.sync_folders(vec![a.clone(), b.clone()]);
        assert!(engine.snapshot().folders.contains(&a));
        assert!(engine.snapshot().folders.contains(&b));
        engine.sync_folders(vec![a.clone()]);
        assert!(!engine.snapshot().folders.contains(&b));
        assert!(engine.snapshot().folders.contains(&a));
    }
}
