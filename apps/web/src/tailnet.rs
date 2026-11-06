// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The browser's tailnet road (docs/ahp/tailnet.md, §The browser).
//! The page runs Tailscale's node as a Go wasm module (ephemeral,
//! same-login — the host's owner gate admits it unchanged); this
//! module is the Rust half of the bridge: dials become newline-JSON
//! AHP transports over the node's synthetic TCP streams, and a
//! polling driver turns the node's netmap into drawer host rows.
//!
//! The JS half lives in tools/tailnet-library.js (the emscripten
//! js-library, proxied to the main thread where the Go module runs)
//! and tailnet-boot.js (lazy load + login + netmap).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::task::Waker;

extern "C" {
    /// Lazy-load and start the in-page node; opens the Tailscale
    /// login when one is needed. Idempotent.
    fn himark_tailnet_start();
    /// A JSON snapshot {state, authUrl, hosts:[{name,dns}]} — NULL
    /// before the node ever started. Caller frees.
    fn himark_tailnet_poll_json() -> *mut std::ffi::c_char;
    fn himark_tailnet_dial(conn: usize, host_port: *const u8, len: usize);
    /// `bytes` ownership moves to JS (it frees after the write).
    fn himark_tailnet_send(conn: usize, bytes: *mut u8, len: usize);
    fn himark_tailnet_close(conn: usize);
    fn malloc(size: usize) -> *mut u8;
    fn free(ptr: *mut std::ffi::c_void);
}

#[derive(Default)]
struct Shared {
    open: bool,
    closed: Option<String>,
    /// The close ask has been proxied — never ask twice, and never
    /// ask after `closed` (a conn id must not outlive its anchor).
    close_sent: bool,
    inbox: VecDeque<Vec<u8>>,
    waker: Option<Waker>,
}

impl Shared {
    fn wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

/// The conn id the JS side hands back on every callback IS the
/// anchor: a raw `Arc<Mutex<Shared>>`, freed exactly once, in
/// `himark_tailnet_closed` — the library guarantees that callback
/// fires last on every road (refused dial included).
fn anchor(shared: &Arc<Mutex<Shared>>) -> usize {
    Arc::into_raw(Arc::clone(shared)) as usize
}

unsafe fn anchored(conn: usize) -> Arc<Mutex<Shared>> {
    let raw = conn as *const Mutex<Shared>;
    Arc::increment_strong_count(raw);
    Arc::from_raw(raw)
}

#[no_mangle]
pub unsafe extern "C" fn himark_tailnet_opened(conn: usize) {
    let shared = anchored(conn);
    let mut state = shared.lock().expect("tailnet shared");
    state.open = true;
    state.wake();
}

/// JS frees `ptr` after this returns — copy out.
#[no_mangle]
pub unsafe extern "C" fn himark_tailnet_data(conn: usize, ptr: *const u8, len: usize) {
    let shared = anchored(conn);
    let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
    let mut state = shared.lock().expect("tailnet shared");
    state.inbox.push_back(bytes);
    state.wake();
}

/// The LAST callback for a conn: consumes the anchor.
#[no_mangle]
pub unsafe extern "C" fn himark_tailnet_closed(conn: usize, reason: *const std::ffi::c_char) {
    let shared = Arc::from_raw(conn as *const Mutex<Shared>);
    let reason = match reason.is_null() {
        true => String::new(),
        false => std::ffi::CStr::from_ptr(reason)
            .to_string_lossy()
            .into_owned(),
    };
    let mut state = shared.lock().expect("tailnet shared");
    state.closed = Some(reason);
    state.wake();
}

/// One wait on the shared state — the browser transport's idiom.
struct SharedWait<R: Fn(&mut Shared) -> Option<Result<(), String>>> {
    shared: Arc<Mutex<Shared>>,
    ready: R,
}

impl<R: Fn(&mut Shared) -> Option<Result<(), String>> + Unpin> std::future::Future
    for SharedWait<R>
{
    type Output = Result<(), String>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut state = self.shared.lock().expect("tailnet shared");
        match (self.ready)(&mut state) {
            Some(result) => std::task::Poll::Ready(result),
            None => {
                state.waker = Some(context.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}

/// Newline-JSON AHP over the in-page node's synthetic TCP stream —
/// the raw face's protocol (agent-host port 7298).
pub(crate) struct TailnetTransport {
    conn: usize,
    shared: Arc<Mutex<Shared>>,
    /// Carry-over bytes of an unfinished line.
    pending: Vec<u8>,
    /// Whole lines ready to hand out.
    lines: VecDeque<String>,
}

impl TailnetTransport {
    pub(crate) async fn connect(host_port: &str) -> Result<Self, String> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let conn = anchor(&shared);
        unsafe { himark_tailnet_dial(conn, host_port.as_ptr(), host_port.len()) };
        SharedWait {
            shared: Arc::clone(&shared),
            ready: |state: &mut Shared| match (&state.closed, state.open) {
                (Some(reason), _) => Some(Err(match reason.is_empty() {
                    true => "tailnet dial refused".to_owned(),
                    false => reason.clone(),
                })),
                (None, true) => Some(Ok(())),
                (None, false) => None,
            },
        }
        .await?;
        Ok(Self {
            conn,
            shared,
            pending: Vec::new(),
            lines: VecDeque::new(),
        })
    }

    fn digest(&mut self, chunk: &[u8]) {
        self.pending.extend_from_slice(chunk);
        while let Some(at) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=at).collect();
            let line = &line[..line.len() - 1];
            self.lines
                .push_back(String::from_utf8_lossy(line).into_owned());
        }
    }
}

impl Drop for TailnetTransport {
    fn drop(&mut self) {
        ask_close(self.conn, &self.shared);
        // The anchor itself dies in `himark_tailnet_closed`.
    }
}

fn ask_close(conn: usize, shared: &Arc<Mutex<Shared>>) {
    let mut state = shared.lock().expect("tailnet shared");
    if state.closed.is_some() || state.close_sent {
        return;
    }
    state.close_sent = true;
    drop(state);
    unsafe { himark_tailnet_close(conn) };
}

impl ahp::transport::Transport for TailnetTransport {
    async fn send(
        &mut self,
        msg: ahp::transport::TransportMessage,
    ) -> Result<(), ahp::TransportError> {
        let text = match msg {
            ahp::transport::TransportMessage::Parsed(message) => serde_json::to_string(&message)
                .map_err(|error| ahp::TransportError::Protocol(error.to_string()))?,
            ahp::transport::TransportMessage::Text(text) => text,
            ahp::transport::TransportMessage::Binary(_) => {
                return Err(ahp::TransportError::Protocol(
                    "the raw tailnet face is a line protocol".to_owned(),
                ))
            }
        };
        unsafe {
            let len = text.len() + 1;
            let buffer = malloc(len);
            if buffer.is_null() {
                return Err(ahp::TransportError::Io("malloc failed".to_owned()));
            }
            std::ptr::copy_nonoverlapping(text.as_ptr(), buffer, text.len());
            *buffer.add(text.len()) = b'\n';
            himark_tailnet_send(self.conn, buffer, len);
        }
        Ok(())
    }

    async fn recv(
        &mut self,
    ) -> Result<Option<ahp::transport::TransportMessage>, ahp::TransportError> {
        loop {
            if let Some(line) = self.lines.pop_front() {
                return Ok(Some(ahp::transport::TransportMessage::Text(line)));
            }
            SharedWait {
                shared: Arc::clone(&self.shared),
                ready: |state: &mut Shared| match (!state.inbox.is_empty(), &state.closed) {
                    (true, _) => Some(Ok(())),
                    (false, Some(_)) => Some(Ok(())),
                    (false, None) => None,
                },
            }
            .await
            .map_err(ahp::TransportError::Io)?;
            let chunk = self
                .shared
                .lock()
                .expect("tailnet shared")
                .inbox
                .pop_front();
            match chunk {
                Some(chunk) => self.digest(&chunk),
                // Closed and drained.
                None => return Ok(None),
            }
        }
    }

    async fn close(&mut self) -> Result<(), ahp::TransportError> {
        ask_close(self.conn, &self.shared);
        Ok(())
    }
}

// ─── the driver: start command + discovery poll ───

#[derive(serde::Deserialize)]
struct Snapshot {
    #[serde(default)]
    state: String,
    #[serde(rename = "authUrl", default)]
    auth_url: String,
    #[serde(default)]
    hosts: Vec<SnapshotHost>,
}

#[derive(serde::Deserialize)]
struct SnapshotHost {
    name: String,
    dns: String,
}

fn poll_snapshot() -> Option<Snapshot> {
    unsafe {
        let raw = himark_tailnet_poll_json();
        if raw.is_null() {
            return None;
        }
        let text = std::ffi::CStr::from_ptr(raw).to_string_lossy().into_owned();
        free(raw as *mut std::ffi::c_void);
        serde_json::from_str(&text).ok()
    }
}

/// The drawer command: load the node, log in, and let discovery
/// fill the rows.
pub struct ConnectTailnet;

impl himark::commands::HimarkCommand for ConnectTailnet {
    fn id(&self) -> &'static str {
        "tailnet.connect"
    }

    fn name(&self) -> String {
        "Connect Tailscale".to_owned()
    }

    fn perform(
        &self,
        _store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        _bound: &imba::focus::Bindings,
        _fx: &mut himark::app::AppFx<'_>,
    ) {
        unsafe { himark_tailnet_start() };
    }
}

/// The posted mint — the frontend-host discovery command, the web
/// edition: dial strings are `tailnet://`, which the browser
/// connector routes through the in-page node.
struct TailnetHostDiscovered {
    name: String,
    url: String,
    runtime: tokio::runtime::Handle,
    clients: Arc<ahp_wire::fs::ClientDirectory>,
}

impl imba::command::DynamicCommand for TailnetHostDiscovered {
    fn id(&self) -> &'static str {
        "tailnet.host-discovered"
    }

    fn name(&self) -> String {
        format!("Tailnet host: {}", self.name)
    }

    fn perform(
        &self,
        store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        _fx: &mut imba::command::Fx<'_>,
    ) {
        let seat = ahp_wire::wire::WireHost::at(
            self.runtime.clone(),
            Arc::new(crate::browser::BrowserConnector),
            self.url.clone(),
        );
        hiahp::higent::flows::mint_agent_server(
            store,
            &self.clients,
            &self.name,
            ahp_wire::client::Client::of(Arc::new(seat)),
        );
    }
}

/// Watch the node and mint discovered hosts; one task on the wire
/// runtime, results ride the posted-command inbox.
pub fn spawn_discovery(
    handle: tokio::runtime::Handle,
    posted: std::sync::mpsc::Sender<himark::app::AppCommand>,
    clients: Arc<ahp_wire::fs::ClientDirectory>,
) {
    handle.clone().spawn(async move {
        let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut last_state = String::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let Some(snapshot) = poll_snapshot() else {
                continue;
            };
            if snapshot.state != last_state {
                last_state = snapshot.state.clone();
                web_sys_log(&match snapshot.auth_url.is_empty() {
                    false => format!(
                        "[tailnet] {} — login: {}",
                        snapshot.state, snapshot.auth_url
                    ),
                    true => format!("[tailnet] {}", snapshot.state),
                });
            }
            for host in &snapshot.hosts {
                let dns = host.dns.trim_end_matches('.');
                if dns.is_empty() {
                    continue;
                }
                let url = format!("tailnet://{dns}:7298");
                if !known.insert(url.clone()) {
                    continue;
                }
                let label = host
                    .name
                    .strip_prefix("himark-")
                    .filter(|short| !short.is_empty())
                    .unwrap_or(&host.name)
                    .to_owned();
                let _ = posted.send(himark::app::AppCommand::Verb(imba::command::Verb::Dynamic(
                    Arc::new(TailnetHostDiscovered {
                        name: label,
                        url,
                        runtime: handle.clone(),
                        clients: Arc::clone(&clients),
                    }),
                )));
            }
        }
    });
}

fn web_sys_log(line: &str) {
    eprintln!("{line}");
}
