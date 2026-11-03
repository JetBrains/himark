// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The tailnet face (docs/ahp/tailnet.md): the host joins the user's
//! tailnet as its own device and serves the same HTTP + WebSocket
//! face as `http.rs` — but with NO token. Identity replaces
//! credentials: every accepted connection is answered for by the
//! tailnet (WhoIs), and only the login that enrolled this node gets
//! through. Plain ws on a fixed port; WireGuard is the encryption.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use crate::server::Host;

pub(crate) const PORT: u16 = 7297;

pub(crate) struct Options {
    pub hostname: Option<String>,
}

/// Start the node and hand the rest (login watch, listener, serving)
/// to a background task. Must run inside the tokio runtime.
pub(crate) fn start(
    host: &Arc<Host>,
    options: Options,
    web_root: Option<PathBuf>,
) -> Result<(), String> {
    let dir = host_discovery::default_dir()
        .ok_or_else(|| "no HOME — nowhere to keep tailnet state".to_owned())?
        .join("tailscale");
    let hostname = options.hostname.unwrap_or_else(tailnet::default_hostname);
    let node = tailnet::Tailnet::start(&dir, &hostname)?;
    eprintln!("[agent-host] tailnet node starting as {hostname}");
    let host = Arc::clone(host);
    tokio::spawn(serve(host, node, web_root));
    Ok(())
}

async fn serve(host: Arc<Host>, node: Arc<tailnet::Tailnet>, web_root: Option<PathBuf>) {
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
                    tracing::error!(
                        target: "ahp_tailnet",
                        "tailnet node has no owning login — not serving"
                    );
                    eprintln!("[agent-host] tailnet node has no owning login — not serving");
                    return;
                }
            },
            Ok(status) => {
                if !status.auth_url.is_empty() && status.auth_url != last_url {
                    last_url = status.auth_url.clone();
                    tracing::info!(target: "ahp_tailnet", url = %last_url, "tailnet login needed");
                    eprintln!("[agent-host] tailnet login: open {last_url}");
                }
            }
            Err(error) => {
                tracing::warn!(target: "ahp_tailnet", %error, "tailnet status");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    };

    let listener = match node.listen(PORT) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(target: "ahp_tailnet", %error, "tailnet listen");
            eprintln!("[agent-host] tailnet listen :{PORT}: {error}");
            return;
        }
    };
    let name = node
        .status()
        .await
        .ok()
        .and_then(|status| status.self_node)
        .map(|own| own.dns_name.trim_end_matches('.').to_owned())
        .unwrap_or_default();
    tracing::info!(target: "ahp_tailnet", %name, %owner, port = PORT, "serving tailnet");
    eprintln!("[agent-host] serving tailnet on ws://{name}:{PORT}/ (owner {owner})");

    let app = crate::http::router(crate::http::HttpState {
        host,
        token: None,
        web_root: web_root.map(Into::into),
    });
    let gated = GatedListener {
        listener,
        node,
        owner,
    };
    if let Err(error) = axum::serve(gated, app).await {
        tracing::error!(target: "ahp_tailnet", %error, "tailnet server stopped");
    }
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
