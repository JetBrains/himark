use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

use himark_ahp_ext_types::{TextOperation, Uid};
use serde_json::{json, Value};

const INIT_ID: i64 = 0;

pub(crate) enum LsEvent {
    Diagnostics {
        uri: String,

        version: Option<i64>,
        diagnostics: Value,
    },
    Progress {
        token: Value,
        value: Value,
    },
}

pub fn discover_rust_analyzer() -> String {
    if let Ok(command) = std::env::var("HIMARK_RUST_ANALYZER") {
        return command;
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("rust-analyzer");
            if crate::claude::is_executable(&candidate) {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    for known in [
        format!("{home}/.cargo/bin/rust-analyzer"),
        "/opt/homebrew/bin/rust-analyzer".to_owned(),
        "/usr/local/bin/rust-analyzer".to_owned(),
    ] {
        if crate::claude::is_executable(std::path::Path::new(&known)) {
            return known;
        }
    }
    if let Ok(toolchains) = std::fs::read_dir(format!("{home}/.rustup/toolchains")) {
        for toolchain in toolchains.flatten() {
            let candidate = toolchain.path().join("bin").join("rust-analyzer");
            if crate::claude::is_executable(&candidate) {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    "rust-analyzer".to_owned()
}

fn extended_path() -> std::ffi::OsString {
    let home = std::env::var("HOME").unwrap_or_default();
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    for known in [
        format!("{home}/.cargo/bin"),
        "/opt/homebrew/bin".to_owned(),
        "/usr/local/bin".to_owned(),
    ] {
        let known = std::path::PathBuf::from(known);
        if !dirs.contains(&known) {
            dirs.push(known);
        }
    }
    std::env::join_paths(dirs).unwrap_or(current)
}

pub(crate) struct Pool {
    servers: Mutex<HashMap<PathBuf, Slot>>,
    events: Arc<dyn Fn(&Arc<Server>, LsEvent) + Send + Sync>,
}

enum Slot {
    Live(Arc<Server>),
    Dead,
}

impl Pool {
    pub(crate) fn new(events: Arc<dyn Fn(&Arc<Server>, LsEvent) + Send + Sync>) -> Pool {
        Pool {
            servers: Mutex::new(HashMap::new()),
            events,
        }
    }

    pub(crate) fn ensure(&self, root: &Path, command: &str) -> Option<Arc<Server>> {
        let mut servers = self.servers.lock().expect("lsp pool");
        if let Some(slot) = servers.get(root) {
            return match slot {
                Slot::Live(server) => Some(Arc::clone(server)),
                Slot::Dead => None,
            };
        }
        match Server::spawn(command, root, Arc::clone(&self.events)) {
            Some(server) => {
                servers.insert(root.to_path_buf(), Slot::Live(Arc::clone(&server)));
                Some(server)
            }
            None => {
                eprintln!("[lsp] {command} failed to spawn for {}", root.display());
                servers.insert(root.to_path_buf(), Slot::Dead);
                None
            }
        }
    }

    pub(crate) fn live(&self, root: &Path) -> Option<Arc<Server>> {
        match self.servers.lock().expect("lsp pool").get(root) {
            Some(Slot::Live(server)) => Some(Arc::clone(server)),
            _ => None,
        }
    }
}

pub(crate) struct Server {
    outbox: Mutex<Outbox>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, PendingSlot>>,
    ready: Mutex<ReadyState>,

    synced: Mutex<HashMap<String, SyncedDocument>>,

    capabilities: Mutex<Option<Value>>,
    _child: Mutex<std::process::Child>,
}

#[derive(Clone)]
pub(crate) struct SyncedDocument {
    pub(crate) version: i64,
    pub(crate) uid: Option<Uid>,
}

struct Outbox {
    stdin: std::process::ChildStdin,
    ready: bool,
    buffered: Vec<Vec<u8>>,
}

enum ReadyState {
    Waiting(Vec<Waker>),
    Ready,
    Failed,
}

#[derive(Default)]
struct PendingSlot {
    waker: Option<Waker>,
    answer: Option<Value>,
}

impl Server {
    fn spawn(
        command: &str,
        root: &Path,
        events: Arc<dyn Fn(&Arc<Server>, LsEvent) + Send + Sync>,
    ) -> Option<Arc<Self>> {
        let mut parts = command.split_whitespace();
        let binary = parts.next()?;
        let mut child = std::process::Command::new(binary)
            .args(parts)

            .env("PATH", extended_path())
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let server = Arc::new(Server {
            outbox: Mutex::new(Outbox {
                stdin,
                ready: false,
                buffered: Vec::new(),
            }),
            next_id: AtomicI64::new(INIT_ID + 1),
            pending: Mutex::new(HashMap::new()),
            ready: Mutex::new(ReadyState::Waiting(Vec::new())),
            synced: Mutex::new(HashMap::new()),
            capabilities: Mutex::new(None),
            _child: Mutex::new(child),
        });
        let root_uri = crate::uris::encoded_file_uri(root);
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": INIT_ID,
            "method": "initialize",
            "params": {
                "processId": Value::Null,
                "rootUri": root_uri.clone(),
                "workspaceFolders": [ { "uri": root_uri, "name": root.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "root".to_owned()) } ],
                "capabilities": {
                    "general": { "positionEncodings": ["utf-8"] },
                    "textDocument": {
                        "definition": {},
                        "references": {},
                        "hover": {},
                        "publishDiagnostics": {},
                        "synchronization": {},

                        "completion": { "completionItem": {} }
                    },
                },
            },
        });
        {
            let mut outbox = server.outbox.lock().expect("lsp outbox");
            let bytes = frame(&initialize);
            let _ = outbox.stdin.write_all(&bytes);
            let _ = outbox.stdin.flush();
        }
        let reader_server = Arc::clone(&server);
        std::thread::Builder::new()
            .name("lsp-reader".to_owned())
            .spawn(move || reader_loop(reader_server, stdout, events))
            .ok()?;
        Some(server)
    }

    pub(crate) fn capabilities(&self) -> Option<Value> {
        self.capabilities.lock().expect("lsp capabilities").clone()
    }

    pub(crate) fn uid_of(&self, uri: &str, version: Option<i64>) -> Option<Uid> {
        let synced = self.synced.lock().expect("lsp synced");
        let document = synced.get(uri)?;
        match version {
            Some(version) if version == document.version => document.uid,
            None => document.uid,
            _ => None,
        }
    }

    pub(crate) fn document_opened(&self, uri: &str, text: &str, uid: Uid) {
        let mut synced = self.synced.lock().expect("lsp synced");
        if synced.contains_key(uri) {
            return;
        }
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri,
                "languageId": language_of(uri),
                "version": 1,
                "text": text,
            }}),
        );
        synced.insert(
            uri.to_owned(),
            SyncedDocument {
                version: 1,
                uid: Some(uid),
            },
        );
    }

    pub(crate) fn document_changed(&self, uri: &str, operation: &TextOperation, uid: Uid) {
        let mut synced = self.synced.lock().expect("lsp synced");
        let Some(document) = synced.get_mut(uri) else {
            return;
        };
        document.version += 1;
        document.uid = Some(uid);
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
                "textDocument": { "uri": uri, "version": document.version },
                "contentChanges": changes,
            }),
        );
    }

    pub(crate) fn ensure_open_from_disk(&self, uri: &str, path: &Path) {
        let mut synced = self.synced.lock().expect("lsp synced");
        if synced.contains_key(uri) {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri,
                "languageId": language_of(uri),
                "version": 0,
                "text": text,
            }}),
        );
        synced.insert(
            uri.to_owned(),
            SyncedDocument {
                version: 0,
                uid: None,
            },
        );
    }

    fn notify(&self, method: &str, params: Value) {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let bytes = frame(&message);
        let mut outbox = self.outbox.lock().expect("lsp outbox");
        if outbox.ready {
            let _ = outbox.stdin.write_all(&bytes);
            let _ = outbox.stdin.flush();
        } else {
            outbox.buffered.push(bytes);
        }
    }

    pub(crate) fn forward_notification(&self, method: &str, params: Value) {
        self.notify(method, params);
    }

    pub(crate) fn request(self: &Arc<Self>, method: &str, params: Value) -> (i64, RequestFuture) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .expect("lsp pending")
            .insert(id, PendingSlot::default());
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let bytes = frame(&message);
        {
            let mut outbox = self.outbox.lock().expect("lsp outbox");
            if outbox.ready {
                let _ = outbox.stdin.write_all(&bytes);
                let _ = outbox.stdin.flush();
            } else {
                outbox.buffered.push(bytes);
            }
        }
        (
            id,
            RequestFuture {
                server: Arc::clone(self),
                id,
            },
        )
    }

    pub(crate) fn ready(self: &Arc<Self>) -> ReadyFuture {
        ReadyFuture {
            server: Arc::clone(self),
        }
    }

    fn finish_handshake(self: &Arc<Self>, response: &Value) {
        *self.capabilities.lock().expect("lsp capabilities") =
            response.pointer("/result/capabilities").cloned();
        let initialized =
            frame(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
        {
            let mut outbox = self.outbox.lock().expect("lsp outbox");
            let _ = outbox.stdin.write_all(&initialized);
            for buffered in outbox.buffered.drain(..).collect::<Vec<_>>() {
                let _ = outbox.stdin.write_all(&buffered);
            }
            let _ = outbox.stdin.flush();
            outbox.ready = true;
        }
        let waiting = {
            let mut ready = self.ready.lock().expect("lsp ready");
            std::mem::replace(&mut *ready, ReadyState::Ready)
        };
        if let ReadyState::Waiting(wakers) = waiting {
            for waker in wakers {
                waker.wake();
            }
        }
    }

    fn fail(self: &Arc<Self>) {
        eprintln!("[lsp] a language server connection closed");
        let waiting = {
            let mut ready = self.ready.lock().expect("lsp ready");
            std::mem::replace(&mut *ready, ReadyState::Failed)
        };
        if let ReadyState::Waiting(wakers) = waiting {
            for waker in wakers {
                waker.wake();
            }
        }
        let mut pending = self.pending.lock().expect("lsp pending");
        for (_, slot) in pending.iter_mut() {
            slot.answer = Some(Value::Null);
            if let Some(waker) = slot.waker.take() {
                waker.wake();
            }
        }
    }
}

pub(crate) struct ReadyFuture {
    server: Arc<Server>,
}

impl std::future::Future for ReadyFuture {
    type Output = bool;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<bool> {
        let mut ready = self.server.ready.lock().expect("lsp ready");
        match &mut *ready {
            ReadyState::Ready => Poll::Ready(true),
            ReadyState::Failed => Poll::Ready(false),
            ReadyState::Waiting(wakers) => {
                wakers.push(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

pub(crate) struct RequestFuture {
    server: Arc<Server>,
    id: i64,
}

impl std::future::Future for RequestFuture {
    type Output = Value;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Value> {
        let mut pending = self.server.pending.lock().expect("lsp pending");
        let Some(slot) = pending.get_mut(&self.id) else {
            return Poll::Ready(Value::Null);
        };
        match slot.answer.take() {
            Some(response) => {
                pending.remove(&self.id);
                Poll::Ready(response)
            }
            None => {
                slot.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl Drop for RequestFuture {
    fn drop(&mut self) {
        self.server
            .pending
            .lock()
            .expect("lsp pending")
            .remove(&self.id);
    }
}

fn reader_loop(
    server: Arc<Server>,
    stdout: std::process::ChildStdout,
    events: Arc<dyn Fn(&Arc<Server>, LsEvent) + Send + Sync>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let Some(message) = read_message(&mut reader) else {
            server.fail();
            return;
        };
        match (message.get("id"), message.get("method")) {
            (Some(id), Some(_)) => {
                let response = json!({ "jsonrpc": "2.0", "id": id.clone(), "result": Value::Null });
                let bytes = frame(&response);
                let mut outbox = server.outbox.lock().expect("lsp outbox");
                let _ = outbox.stdin.write_all(&bytes);
                let _ = outbox.stdin.flush();
            }
            (Some(id), None) => match id.as_i64() {
                Some(INIT_ID) => server.finish_handshake(&message),
                Some(id) => {
                    let mut pending = server.pending.lock().expect("lsp pending");
                    if let Some(slot) = pending.get_mut(&id) {
                        slot.answer = Some(message.clone());
                        if let Some(waker) = slot.waker.take() {
                            waker.wake();
                        }
                    }
                }
                None => {}
            },
            (None, Some(method)) => match method.as_str() {
                Some("textDocument/publishDiagnostics") => {
                    let params = &message["params"];
                    events(
                        &server,
                        LsEvent::Diagnostics {
                            uri: params["uri"].as_str().unwrap_or_default().to_owned(),
                            version: params["version"].as_i64(),
                            diagnostics: params["diagnostics"].clone(),
                        },
                    );
                }
                Some("$/progress") => {
                    let params = &message["params"];
                    events(
                        &server,
                        LsEvent::Progress {
                            token: params["token"].clone(),
                            value: params["value"].clone(),
                        },
                    );
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse().ok();
        }
    }
    let mut body = vec![0u8; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

fn language_of(uri: &str) -> &'static str {
    match uri.rsplit('.').next() {
        Some("rs") => "rust",
        Some("md") => "markdown",
        Some("py") => "python",
        Some("ts") => "typescript",
        Some("js") => "javascript",
        _ => "plaintext",
    }
}

#[cfg(test)]
mod discovery_tests {
    #[test]
    fn discovery_honors_the_override_then_finds_a_binary() {
        std::env::set_var("HIMARK_RUST_ANALYZER", "python3 /tmp/ra-stub.py");
        assert_eq!(super::discover_rust_analyzer(), "python3 /tmp/ra-stub.py");
        std::env::remove_var("HIMARK_RUST_ANALYZER");
        assert!(!super::discover_rust_analyzer().is_empty());
    }
}
