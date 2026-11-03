// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The tailnet face (docs/ahp/tailnet.md): the host joins the user's
//! tailnet as its own device and serves the same HTTP + WebSocket
//! face as `http.rs` — but with NO token. Identity replaces
//! credentials: every accepted connection is answered for by the
//! tailnet (WhoIs), and only the login that enrolled this node gets
//! through. Plain ws on a fixed port; WireGuard is the encryption.
//!
//! Enabled three ways, all meeting in [`TailnetServer::enable`]:
//! the `--tailscale` flag, a state dir left by an earlier enrollment
//! (a host that joined once keeps serving across restarts), and the
//! `tailnetServe` RPC (the drawer's "Share Host on Tailnet"). The
//! enrollment's progress is ONE immutable [`TailnetServing`] value
//! under a swap latch — the serve task replaces it whole, the RPC
//! answers a snapshot.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use crate::server::Host;

pub(crate) const PORT: u16 = 7297;

pub(crate) struct Options {
    pub hostname: Option<String>,
}

/// Where the enrollment stands. Immutable; replaced whole.
#[derive(Clone, Debug)]
pub(crate) enum TailnetServing {
    Dormant,
    Starting,
    NeedsLogin { auth_url: String },
    WaitingApproval,
    Running { url: String, owner: String },
    Failed { error: String },
}

impl TailnetServing {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        match self {
            TailnetServing::Dormant => serde_json::json!({ "state": "dormant" }),
            TailnetServing::Starting => serde_json::json!({ "state": "starting" }),
            TailnetServing::NeedsLogin { auth_url } => {
                serde_json::json!({ "state": "needsLogin", "authUrl": auth_url })
            }
            TailnetServing::WaitingApproval => {
                serde_json::json!({ "state": "waitingApproval" })
            }
            TailnetServing::Running { url, owner } => {
                serde_json::json!({ "state": "running", "url": url, "owner": owner })
            }
            TailnetServing::Failed { error } => {
                serde_json::json!({ "state": "failed", "error": error })
            }
        }
    }
}

/// The host's seat for the face: one swap latch over the snapshot —
/// readers clone the Arc, the serve task swaps whole values.
pub(crate) struct TailnetServer {
    snapshot: std::sync::Mutex<Arc<TailnetServing>>,
}

impl TailnetServer {
    pub(crate) fn new() -> Self {
        Self {
            snapshot: std::sync::Mutex::new(Arc::new(TailnetServing::Dormant)),
        }
    }

    pub(crate) fn snapshot(&self) -> Arc<TailnetServing> {
        self.snapshot
            .lock()
            .map(|held| Arc::clone(&held))
            .unwrap_or_else(|_| Arc::new(TailnetServing::Dormant))
    }

    fn swap(&self, next: TailnetServing) {
        if let Ok(mut held) = self.snapshot.lock() {
            *held = Arc::new(next);
        }
    }

    /// Idempotent: dormant and failed faces start; a face already
    /// underway just keeps going. Must run inside the runtime.
    pub(crate) fn enable(host: &Arc<Host>, options: Options, web_root: Option<PathBuf>) {
        {
            let Ok(mut held) = host.tailnet.snapshot.lock() else {
                return;
            };
            match **held {
                TailnetServing::Dormant | TailnetServing::Failed { .. } => {
                    *held = Arc::new(TailnetServing::Starting);
                }
                _ => return,
            }
        }
        let host = Arc::clone(host);
        tokio::spawn(async move {
            if let Err(error) = serve(&host, options, web_root).await {
                tracing::error!(target: "ahp_tailnet", %error, "tailnet face");
                eprintln!("[agent-host] tailnet: {error}");
                host.tailnet.swap(TailnetServing::Failed { error });
            }
        });
    }
}

/// A state dir from an earlier enrollment — the boot-time reason to
/// enable without the flag.
pub(crate) fn enrolled_state_dir() -> Option<PathBuf> {
    let dir = host_discovery::default_dir()?.join("tailscale");
    dir.exists().then_some(dir)
}

async fn serve(
    host: &Arc<Host>,
    options: Options,
    web_root: Option<PathBuf>,
) -> Result<(), String> {
    let dir = host_discovery::default_dir()
        .ok_or_else(|| "no HOME — nowhere to keep tailnet state".to_owned())?
        .join("tailscale");
    let hostname = options.hostname.unwrap_or_else(tailnet::default_hostname);
    let node = tokio::task::spawn_blocking({
        let dir = dir.clone();
        let hostname = hostname.clone();
        move || tailnet::Tailnet::start(&dir, &hostname)
    })
    .await
    .map_err(|join| format!("tailnet start task: {join}"))??;
    eprintln!("[agent-host] tailnet node starting as {hostname}");

    // Wait out the login. The AuthURL is the WHOLE enrollment UX:
    // the user opens it, logs into Tailscale, and the node's state
    // dir keeps the login from then on.
    let mut last_url = String::new();
    let owner = loop {
        match node.status().await {
            Ok(status) if status.running() => match status.owner_login() {
                Some(owner) => break owner.to_owned(),
                None => {
                    // A tagged node has no owner; owner-only is the
                    // only policy so far (docs/ahp/tailnet.md).
                    return Err("tailnet node has no owning login — not serving".to_owned());
                }
            },
            Ok(status) if status.needs_machine_auth() => {
                host.tailnet.swap(TailnetServing::WaitingApproval);
            }
            Ok(status) => {
                if !status.auth_url.is_empty() && status.auth_url != last_url {
                    last_url = status.auth_url.clone();
                    tracing::info!(target: "ahp_tailnet", url = %last_url, "tailnet login needed");
                    eprintln!("[agent-host] tailnet login: open {last_url}");
                    host.tailnet.swap(TailnetServing::NeedsLogin {
                        auth_url: last_url.clone(),
                    });
                }
            }
            Err(error) => {
                tracing::warn!(target: "ahp_tailnet", %error, "tailnet status");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    };

    let listener = node.listen(PORT)?;
    let name = node
        .status()
        .await
        .ok()
        .and_then(|status| status.self_node)
        .map(|own| own.dns_name.trim_end_matches('.').to_owned())
        .unwrap_or_default();
    let url = format!("ws://{name}:{PORT}/");
    tracing::info!(target: "ahp_tailnet", %name, %owner, port = PORT, "serving tailnet");
    eprintln!("[agent-host] serving tailnet on {url} (owner {owner})");
    host.tailnet.swap(TailnetServing::Running {
        url,
        owner: owner.clone(),
    });

    let app = crate::http::router(crate::http::HttpState {
        host: Arc::clone(host),
        token: None,
        web_root: web_root.map(Into::into),
    });
    let gated = GatedListener {
        listener,
        node,
        owner,
    };
    axum::serve(gated, app)
        .await
        .map_err(|error| format!("tailnet server stopped: {error}"))
}

/// The gate itself: connections are admitted by WHO they are from,
/// before a byte of HTTP is read. Anyone the tailnet cannot name as
/// the owner is dropped here.
struct GatedListener {
    listener: tailnet::TailnetListener,
    node: Arc<tailnet::Tailnet>,
    owner: String,
}

impl axum::serve::Listener for GatedListener {
    type Io = tailnet::TailnetStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, remote) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(target: "ahp_tailnet", %error, "tailnet accept");
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    continue;
                }
            };
            let login = match self.node.whois(remote).await {
                Ok(whois) => whois.login().map(str::to_owned),
                Err(error) => {
                    tracing::warn!(target: "ahp_tailnet", %remote, %error, "tailnet whois");
                    None
                }
            };
            match login {
                Some(login) if login == self.owner => {
                    tracing::info!(target: "ahp_tailnet", %remote, %login, "tailnet caller admitted");
                    return (stream, SocketAddr::new(remote, 0));
                }
                refused => {
                    tracing::warn!(
                        target: "ahp_tailnet",
                        %remote,
                        caller = refused.as_deref().unwrap_or("<unidentified>"),
                        "tailnet caller is not the owner — refused"
                    );
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), PORT))
    }
}
