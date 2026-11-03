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
use std::sync::Arc;

use himark::app::AppCommand;

const SCAN_SECONDS: u64 = 15;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SavedHost {
    name: String,
    url: String,
}

fn himark_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".himark"))
}

fn hosts_path() -> Option<PathBuf> {
    Some(himark_dir()?.join("hosts.json"))
}

/// Must agree with the desktop connector's node state dir — its
/// existence is the "tailnet is enabled" latch.
fn client_state_dir() -> Option<PathBuf> {
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
}

impl Scanner {
    async fn run(mut self) {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(SCAN_SECONDS)).await;
            let Some(state_dir) = client_state_dir() else {
                return;
            };
            if !state_dir.exists() {
                // Not enabled: no node has ever logged in here. Keep
                // a lazy eye out — the first ts.net Add Host flips it.
                tokio::time::sleep(std::time::Duration::from_secs(45)).await;
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
                    continue;
                }
            };
            if !status.running() {
                continue;
            }
            self.digest(&status);
        }
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
