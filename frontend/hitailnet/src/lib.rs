// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Tailnet host discovery, the saved host list, and the tailnet
//! face (docs/ahp/tailnet.md phases 2–3). The lockfile law extends
//! here: discovery is the EMBEDDER's — the wire sees names and
//! URLs, himark sees rows, nothing below this module sees tailscale.
//!
//! State management is the store's, nothing else's:
//! - the shell-visible state is [`TailnetFaceState`], a store
//!   component written ONLY by the posted [`TailnetChanged`]
//!   command and read over the C ABI between batches — the same
//!   road every catalog read takes, and the batch that writes it is
//!   the wake;
//! - the shell's connect action is [`TailnetFlows`], a flow closure
//!   installed in the store (the `AgentFlows` precedent) that sends
//!   on a channel the scanner owns the other end of;
//! - everything the scanner tracks is a plain owned field.
//!
//! `hosts.json` under the data root keeps the discovered hosts
//! (keyed by URL — HostIds are per-process), so rows appear at boot
//! before the node is up and survive an expired login. The data
//! root is THREADED from `himark_create`; the shell that knows the
//! right place (the iOS container) passes it there.

#[cfg(feature = "capi")]
pub mod capi;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use himark::app::AppCommand;

const SCAN_SECONDS: u64 = 15;

/// What a shell renders of the tailnet. Immutable value, replaced
/// whole.
#[derive(Clone, PartialEq, Default, Debug)]
pub enum TailnetState {
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
    pub fn code(&self) -> u32 {
        match self {
            TailnetState::Off => 0,
            TailnetState::Starting => 1,
            TailnetState::NeedsLogin { .. } => 2,
            TailnetState::WaitingApproval => 3,
            TailnetState::Running => 4,
        }
    }
}

/// The store component the C face reads. Written only by
/// [`TailnetChanged`].
#[derive(Clone, PartialEq, Default, Debug)]
pub struct TailnetFaceState {
    pub state: TailnetState,
}

/// The shell's tailnet verbs, installed in the store at boot —
/// the closures carry what they captured there (the scanner's
/// channel, the connector, the poster).
#[derive(Clone)]
pub struct TailnetFlows {
    pub connect: Arc<dyn Fn() + Send + Sync>,
    /// The foregrounding kick: revive a suspension-wedged engine
    /// and nudge the Failed rows to redial.
    pub resume: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SavedHost {
    name: String,
    url: String,
}

/// The data root: the shell's choice, threaded from `himark_create`;
/// the desktop default is the dotdir.
pub fn data_root(from_shell: Option<PathBuf>) -> Option<PathBuf> {
    if from_shell.is_some() {
        return from_shell;
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".himark"))
}

/// The client node's state dir under the root — handed to the
/// connector at boot; its existence is the "tailnet is enabled"
/// latch.
pub fn client_node_dir(root: &Option<PathBuf>) -> Option<PathBuf> {
    Some(root.as_ref()?.join("tailnet-client"))
}

fn hosts_path(root: &Option<PathBuf>) -> Option<PathBuf> {
    Some(root.as_ref()?.join("hosts.json"))
}

fn load(root: &Option<PathBuf>) -> Vec<SavedHost> {
    let Some(path) = hosts_path(root) else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save(root: &Option<PathBuf>, hosts: &[SavedHost]) {
    let Some(path) = hosts_path(root) else { return };
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

/// Register the saved rows (between batches, before any window),
/// install the flows, and start the discovery scanner.
pub fn install(
    app: &mut himark::app::Himark,
    clients: &Arc<ahp_wire::fs::ClientDirectory>,
    tailnet_connector: Arc<desktop::tailnet_connector::TailnetConnector>,
    connector: Arc<dyn ahp_wire::transport::Connector>,
    root: Option<PathBuf>,
) {
    let saved = load(&root);
    let mut known: HashSet<String> = HashSet::new();
    for host in &saved {
        if !known.insert(host.url.clone()) {
            continue;
        }
        let seat =
            ahp_wire::wire::WireHost::at(app.runtime(), Arc::clone(&connector), host.url.clone());
        hiahp::engine::register_agent_server(
            app,
            clients,
            &host.name,
            ahp_wire::client::Client::of(Arc::new(seat)),
        );
    }

    let (connect, connect_requests) = tokio::sync::mpsc::unbounded_channel::<()>();
    let resume = {
        let connector = Arc::clone(&tailnet_connector);
        let runtime = app.runtime();
        let poster = app.poster();
        Arc::new(move || {
            connector.resume(&runtime);
            // Failed rows never redial on their own — the return
            // from background is exactly the moment to retry.
            poster.post(AppCommand::Verb(imba::command::Verb::Dynamic(Arc::new(
                ahp_session::session::driver::RetryHosts,
            ))));
        })
    };
    app.with_store(|store| {
        store.put(TailnetFaceState::default());
        store.put(TailnetFlows {
            connect: Arc::new(move || {
                let _ = connect.send(());
            }),
            resume,
        });
    });

    let scanner = Scanner {
        poster: app.poster(),
        runtime: app.runtime(),
        clients: Arc::clone(clients),
        node_source: tailnet_connector,
        connector,
        root,
        known,
        saved,
        online: HashMap::new(),
        published: TailnetState::Off,
        connect_requests,
        connect_requested: false,
    };
    app.runtime().spawn(scanner.run());
}

struct Scanner {
    poster: himark::effects::Poster,
    runtime: tokio::runtime::Handle,
    clients: Arc<ahp_wire::fs::ClientDirectory>,
    node_source: Arc<desktop::tailnet_connector::TailnetConnector>,
    connector: Arc<dyn ahp_wire::transport::Connector>,
    root: Option<PathBuf>,
    known: HashSet<String>,
    saved: Vec<SavedHost>,
    online: HashMap<String, bool>,
    published: TailnetState,
    connect_requests: tokio::sync::mpsc::UnboundedReceiver<()>,
    connect_requested: bool,
}

impl Scanner {
    async fn run(mut self) {
        // Let the first poster land after the host attaches.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        loop {
            if !self.enabled() {
                self.announce(TailnetState::Off);
                // Not enabled: no node has ever logged in here. The
                // first ts.net Add Host flips the dir latch; a
                // shell's Connect Tailscale arrives on the channel.
                let lull = tokio::time::sleep(std::time::Duration::from_secs(45));
                tokio::select! {
                    _ = lull => {}
                    request = self.connect_requests.recv() => {
                        self.connect_requested |= request.is_some();
                    }
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

    fn enabled(&mut self) -> bool {
        while let Ok(()) = self.connect_requests.try_recv() {
            self.connect_requested = true;
        }
        if self.connect_requested {
            return true;
        }
        client_node_dir(&self.root).is_some_and(|dir| dir.exists())
    }

    /// On a change, post the command that writes the store — the
    /// batch is what wakes the shell, which reads the store back.
    fn announce(&mut self, state: TailnetState) {
        if self.published == state {
            return;
        }
        self.published = state.clone();
        self.poster
            .post(AppCommand::Verb(imba::command::Verb::Dynamic(Arc::new(
                TailnetChanged { state },
            ))));
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
                save(&self.root, &self.saved);
                tracing::info!(target: "tailnet_hosts", %label, %url, "discovered");
                self.poster
                    .post(AppCommand::Verb(imba::command::Verb::Dynamic(Arc::new(
                        HostDiscovered {
                            name: label,
                            url: url.clone(),
                            runtime: self.runtime.clone(),
                            connector: Arc::clone(&self.connector),
                            clients: Arc::clone(&self.clients),
                        },
                    ))));
            } else if peer.online && !self.online.get(&url).copied().unwrap_or(false) {
                // A known host came back — nudge the Failed rows,
                // which never retry on their own.
                wake_failed_rows = true;
            }
            self.online.insert(url, peer.online);
        }
        if wake_failed_rows {
            self.poster
                .post(AppCommand::Verb(imba::command::Verb::Dynamic(Arc::new(
                    ahp_session::session::driver::RetryHosts,
                ))));
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

/// The scanner's state landing: writes the component the C face
/// reads.
struct TailnetChanged {
    state: TailnetState,
}

impl imba::command::DynamicCommand for TailnetChanged {
    fn id(&self) -> &'static str {
        "tailnet.state-changed"
    }

    fn name(&self) -> String {
        format!("Tailnet: {:?}", self.state)
    }

    fn perform(
        &self,
        store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        _fx: &mut imba::command::Fx<'_>,
    ) {
        store.update::<TailnetFaceState>(|held| held.state = self.state.clone());
    }
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
        hiahp::higent::flows::mint_agent_server(
            store,
            &self.clients,
            &self.name,
            ahp_wire::client::Client::of(Arc::new(seat)),
        );
    }
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
