// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Tailnet host discovery and the saved host list
//! (docs/ahp/tailnet.md phase 2). The lockfile law extends here:
//! discovery is the EMBEDDER's — the wire sees names and URLs,
//! himark sees rows, nothing below this module sees tailscale.
//!
//! `~/.himark/hosts.json` keeps the discovered hosts (keyed by URL —
//! HostIds are per-process), so rows appear at boot before the node
//! is even up, and survive an expired login. Discovery itself runs
//! only when the client node's state dir already exists: logging in
//! once (by adding a ts.net host, which starts the node) is the
//! whole enablement — no setting.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use himark::app::AppCommand;

const SCAN_SECONDS: u64 = 15;

/// What a native shell renders of the tailnet. One IMMUTABLE value:
/// the scanner swaps the whole thing, readers clone the whole thing,
/// no field is ever poked in place.
#[derive(Clone, PartialEq, Default, Debug)]
pub(crate) enum TailnetState {
    #[default]
    Off,
    Starting,
    NeedsLogin {
        auth_url: String,
    },
    WaitingApproval,
    Running,
}

impl TailnetState {
    /// The C ABI encoding — integers exist ONLY at this edge.
    /// Append-only once a header ships.
    pub(crate) fn code(&self) -> u32 {
        match self {
            TailnetState::Off => 0,
            TailnetState::Starting => 1,
            TailnetState::NeedsLogin { .. } => 2,
            TailnetState::WaitingApproval => 3,
            TailnetState::Running => 4,
        }
    }
}

/// The shell's window onto the scanner: a state atom plus the
/// login-request latch. A static, because the C reads are sync and
/// a shell process runs one engine.
pub(crate) struct TailnetFace {
    state: Mutex<TailnetState>,
    /// One-way latch: a shell asked to connect. Input TO the
    /// scanner, never published state.
    login_requested: AtomicBool,
    wake: tokio::sync::Notify,
}

pub(crate) fn face() -> &'static TailnetFace {
    static FACE: std::sync::OnceLock<TailnetFace> = std::sync::OnceLock::new();
    FACE.get_or_init(|| TailnetFace {
        state: Mutex::new(TailnetState::Off),
        login_requested: AtomicBool::new(false),
        wake: tokio::sync::Notify::new(),
    })
}

impl TailnetFace {
    pub(crate) fn state(&self) -> TailnetState {
        self.state
            .lock()
            .map(|held| held.clone())
            .unwrap_or_default()
    }

    /// The shell's "Connect Tailscale" action: starts the node even
    /// before any state dir exists and wakes the scanner for the
    /// fast login cadence.
    pub(crate) fn request_login(&self) {
        self.login_requested.store(true, Ordering::Relaxed);
        self.wake.notify_waiters();
    }

    fn publish(&self, state: TailnetState) {
        if let Ok(mut held) = self.state.lock() {
            *held = state;
        }
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SavedHost {
    name: String,
    url: String,
}

/// The data root. A shell may place it (the iOS container's
/// Application Support — set BEFORE the engine exists); desktop
/// defaults to the dotdir.
static DATA_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub(crate) fn set_data_dir(path: PathBuf) -> bool {
    DATA_DIR.set(path).is_ok()
}

fn himark_dir() -> Option<PathBuf> {
    if let Some(dir) = DATA_DIR.get() {
        return Some(dir.clone());
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".himark"))
}

fn hosts_path() -> Option<PathBuf> {
    Some(himark_dir()?.join("hosts.json"))
}

/// The client node's state dir — handed to the connector at boot;
/// its existence is the "tailnet is enabled" latch.
pub(crate) fn client_node_dir() -> Option<PathBuf> {
    Some(himark_dir()?.join("tailnet-client"))
}

fn load() -> Vec<SavedHost> {
    let Some(path) = hosts_path() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save(hosts: &[SavedHost]) {
    let Some(path) = hosts_path() else { return };
    let Ok(body) = serde_json::to_vec_pretty(hosts) else {
        return;
    };
    let Some(dir) = path.parent() else { return };
    let _ = std::fs::create_dir_all(dir);
    let scratch = path.with_extension("json.new");
    if std::fs::write(&scratch, body).is_ok() {
        let _ = std::fs::rename(&scratch, &path);
    }
}

/// Register the saved rows (between batches, before any window) and
/// start the discovery scanner.
pub(crate) fn install(
    engine: &mut crate::HimarkEngine,
    tailnet_connector: Arc<desktop::tailnet_connector::TailnetConnector>,
    connector: Arc<dyn ahp_wire::transport::Connector>,
) {
    let saved = load();
    let mut known: HashSet<String> = HashSet::new();
    for host in &saved {
        if !known.insert(host.url.clone()) {
            continue;
        }
        let seat = ahp_wire::wire::WireHost::at(
            engine.runtime(),
            Arc::clone(&connector),
            host.url.clone(),
        );
        engine.register_agent_server(&host.name, ahp_wire::client::Client::of(Arc::new(seat)));
    }

    let scanner = Scanner {
        poster: engine.app.poster(),
        runtime: engine.runtime(),
        clients: engine.clients(),
        node_source: tailnet_connector,
        connector,
        known,
        saved,
        online: HashMap::new(),
        published: TailnetState::Off,
    };
    engine.runtime().spawn(scanner.run());
}

struct Scanner {
    poster: himark::effects::Poster,
    runtime: tokio::runtime::Handle,
    clients: Arc<ahp_wire::fs::ClientDirectory>,
    node_source: Arc<desktop::tailnet_connector::TailnetConnector>,
    connector: Arc<dyn ahp_wire::transport::Connector>,
    known: HashSet<String>,
    saved: Vec<SavedHost>,
    online: HashMap<String, bool>,
    published: TailnetState,
}

impl Scanner {
    async fn run(mut self) {
        // Let the first poster land after the host attaches.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        loop {
            let enabled = self.enabled();
            if !enabled {
                self.announce(TailnetState::Off);
                // Not enabled: no node has ever logged in here. The
                // first ts.net Add Host (or a shell's Connect
                // Tailscale, which wakes us) flips it.
                let lull = tokio::time::sleep(std::time::Duration::from_secs(45));
                tokio::select! {
                    _ = lull => {}
                    _ = face().wake.notified() => {}
                }
                continue;
            }
            let status = match self.node_source.node().await {
                Ok(node) => node.status().await,
                Err(error) => Err(error),
            };
            let status = match status {
                Ok(status) => status,
                Err(error) => {
                    tracing::warn!(target: "tailnet_hosts", %error, "scan");
                    tokio::time::sleep(std::time::Duration::from_secs(SCAN_SECONDS)).await;
                    continue;
                }
            };
            let pace = if status.running() {
                self.announce(TailnetState::Running);
                self.digest(&status);
                SCAN_SECONDS
            } else if status.needs_machine_auth() {
                self.announce(TailnetState::WaitingApproval);
                5
            } else if !status.auth_url.is_empty() {
                // The login cadence is fast: a shell is likely
                // showing this URL right now.
                self.announce(TailnetState::NeedsLogin {
                    auth_url: status.auth_url.clone(),
                });
                3
            } else {
                self.announce(TailnetState::Starting);
                2
            };
            tokio::time::sleep(std::time::Duration::from_secs(pace)).await;
        }
    }

    fn enabled(&self) -> bool {
        if face().login_requested.load(Ordering::Relaxed) {
            return true;
        }
        client_node_dir().is_some_and(|dir| dir.exists())
    }

    /// Publish to the face; on a CHANGE, also post a (no-op) command
    /// so the shell's wake-drain cycle runs and re-reads the face —
    /// native chrome has no other pulse.
    fn announce(&mut self, state: TailnetState) {
        if self.published == state {
            return;
        }
        self.published = state.clone();
        face().publish(state);
        self.poster.post(AppCommand::Verb(imba::command::Verb::Dynamic(
            Arc::new(TailnetStateChanged),
        )));
    }

    fn digest(&mut self, status: &tailnet::Status) {
        let mut wake_failed_rows = false;
        for peer in status.agent_host_peers() {
            let Some((label, url)) = candidate(peer) else {
                continue;
            };
            if self.known.insert(url.clone()) {
                self.saved.push(SavedHost {
                    name: label.clone(),
                    url: url.clone(),
                });
                save(&self.saved);
                tracing::info!(target: "tailnet_hosts", %label, %url, "discovered");
                self.poster.post(AppCommand::Verb(imba::command::Verb::Dynamic(
                    Arc::new(HostDiscovered {
                        name: label,
                        url: url.clone(),
                        runtime: self.runtime.clone(),
                        connector: Arc::clone(&self.connector),
                        clients: Arc::clone(&self.clients),
                    }),
                )));
            } else if peer.online && !self.online.get(&url).copied().unwrap_or(false) {
                // A known host came back — nudge the Failed rows,
                // which never retry on their own.
                wake_failed_rows = true;
            }
            self.online.insert(url, peer.online);
        }
        if wake_failed_rows {
            self.poster.post(AppCommand::Verb(imba::command::Verb::Dynamic(
                Arc::new(ahp_session::session::driver::RetryHosts),
            )));
        }
    }
}

/// The drawer label and dial URL for a discovered peer. MagicDNS
/// names dial (they survive IP churn); the label is the machine
/// name with our prefix shed.
fn candidate(peer: &tailnet::NodeStatus) -> Option<(String, String)> {
    let dns = peer.dns_name.trim_end_matches('.');
    if dns.is_empty() {
        return None;
    }
    let url = format!("ws://{dns}:7297/");
    let label = peer
        .host_name
        .strip_prefix(tailnet::HOSTNAME_PREFIX)
        .filter(|short| !short.is_empty())
        .unwrap_or(&peer.host_name)
        .to_owned();
    Some((label, url))
}

/// Wakes the shell, nothing else: the drain after any batch is when
/// native chrome re-reads the face.
struct TailnetStateChanged;

impl imba::command::DynamicCommand for TailnetStateChanged {
    fn id(&self) -> &'static str {
        "tailnet.state-changed"
    }

    fn name(&self) -> String {
        "Tailnet state changed".to_owned()
    }

    fn perform(
        &self,
        _store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        _fx: &mut imba::command::Fx<'_>,
    ) {
    }
}

// ---- The C surface (hand-declared in himark's cbindgen trailer) --

/// 0 off · 1 starting · 2 needs login · 3 waiting approval ·
/// 4 running. Append-only.
#[no_mangle]
pub extern "C" fn himark_tailnet_state() -> u32 {
    face().state().code()
}

/// UTF-8, no NUL; returns the byte length, copies when it fits.
/// Non-empty only in the needs-login state.
#[no_mangle]
pub unsafe extern "C" fn himark_tailnet_auth_url(
    out: *mut std::os::raw::c_char,
    cap: usize,
) -> usize {
    let url = match face().state() {
        TailnetState::NeedsLogin { auth_url } => Some(auth_url),
        _ => None,
    };
    himark::ffi::copy_optional_utf8(url, out, cap)
}

/// The shell's "Connect Tailscale" action.
#[no_mangle]
pub extern "C" fn himark_tailnet_login() {
    face().request_login();
}

/// Place the data root (hosts.json, the node state dir). Call
/// BEFORE himark_create; returns false once the root is set.
#[no_mangle]
pub unsafe extern "C" fn himark_tailnet_set_data_dir(
    path: *const std::os::raw::c_char,
    len: usize,
) -> bool {
    if path.is_null() || len == 0 {
        return false;
    }
    let bytes = std::slice::from_raw_parts(path as *const u8, len);
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    set_data_dir(PathBuf::from(text))
}

/// The posted mint: runs store-in-hand on the UI thread; all the
/// scanning stayed on the runtime.
struct HostDiscovered {
    name: String,
    url: String,
    runtime: tokio::runtime::Handle,
    connector: Arc<dyn ahp_wire::transport::Connector>,
    clients: Arc<ahp_wire::fs::ClientDirectory>,
}

#[cfg(test)]
mod tests {
    fn peer(host_name: &str, dns_name: &str) -> tailnet::NodeStatus {
        serde_json::from_value(serde_json::json!({
            "HostName": host_name,
            "DNSName": dns_name,
            "TailscaleIPs": ["100.64.0.9"],
            "UserID": 1,
            "Online": true
        }))
        .expect("a peer parses")
    }

    #[test]
    fn candidates_dial_magicdns_and_shed_the_prefix() {
        let found = super::candidate(&peer("himark-studio", "himark-studio.tail1.ts.net."));
        assert_eq!(
            found,
            Some((
                "studio".to_owned(),
                "ws://himark-studio.tail1.ts.net:7297/".to_owned()
            ))
        );
    }

    #[test]
    fn degenerate_peers_are_skipped_or_kept_whole() {
        assert_eq!(
            super::candidate(&peer("himark-mini", "")),
            None,
            "no MagicDNS name, no row"
        );
        let found = super::candidate(&peer("himark-", "himark-.tail1.ts.net"));
        assert_eq!(
            found.expect("a row").0,
            "himark-",
            "an empty shed keeps the full name"
        );
    }
}

impl imba::command::DynamicCommand for HostDiscovered {
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
            Arc::clone(&self.connector),
            self.url.clone(),
        );
        himark::higent::flows::mint_agent_server(
            store,
            &self.clients,
            &self.name,
            ahp_wire::client::Client::of(Arc::new(seat)),
        );
    }
}
