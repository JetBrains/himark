// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Dials over the EMBEDDED tailnet node (docs/ahp/tailnet.md): a
//! `ws://…ts.net` or CGNAT (`100.64/10`) destination goes through
//! the node's tunnel instead of the OS network; everything else
//! falls through to the plain [`crate::connector::DesktopConnector`]
//! paths. The node is one per process, started on first tailnet
//! dial; while it still needs a Tailscale login, dials fail with the
//! AuthURL in the error, which the reconnect loop surfaces and
//! retries.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use ahp_wire::transport::{Connector, DeadLatched, Dialing};

pub struct TailnetConnector {
    inner: Arc<Inner>,
}

struct Inner {
    fallback: crate::connector::DesktopConnector,
    node: tokio::sync::OnceCell<Arc<tailnet::Tailnet>>,
}

impl TailnetConnector {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                fallback: crate::connector::DesktopConnector,
                node: tokio::sync::OnceCell::new(),
            }),
        }
    }
}

impl Inner {
    async fn node(&self) -> Result<&Arc<tailnet::Tailnet>, String> {
        self.node
            .get_or_try_init(|| async {
                tokio::task::spawn_blocking(|| {
                    let home = std::env::var_os("HOME")
                        .ok_or_else(|| "no HOME for the tailnet state".to_owned())?;
                    let dir = std::path::PathBuf::from(home)
                        .join(".himark")
                        .join("tailnet-client");
                    tailnet::Tailnet::start(&dir, &tailnet::default_client_hostname())
                })
                .await
                .map_err(|join| format!("tailnet start task: {join}"))?
            })
            .await
    }
}

impl Default for TailnetConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl Connector for TailnetConnector {
    fn dial(&self, url: String, tag: String, dead: Arc<AtomicBool>) -> Dialing {
        let Some(destination) = tailnet_destination(&url) else {
            return self.inner.fallback.dial(url, tag, dead);
        };
        let this = Arc::clone(&self.inner);
        Box::pin(async move {
            let node = this.node().await?;
            // Not Running yet usually means the login is pending —
            // put the AuthURL in the error so the host row shows the
            // way in; the redial loop picks it up once logged in.
            let status = node.status().await?;
            if !status.running() {
                return Err(match status.auth_url.is_empty() {
                    false => format!("tailnet login needed — open {}", status.auth_url),
                    true => format!("tailnet is {}", status.backend_state),
                });
            }
            let stream = node.dial(destination).await?;
            // Plain ws over the tunnel: WireGuard already encrypts
            // and authenticates the path (docs/ahp/tailnet.md).
            let handshake_url = url.replacen("wss://", "ws://", 1);
            let (socket, _response) = tokio_tungstenite::client_async(&handshake_url, stream)
                .await
                .map_err(|error| format!("agent host unreachable at {url}: {error}"))?;
            Ok(ahp::transport::BoxedTransport::new(DeadLatched {
                inner: WsOverTailnet { inner: socket },
                dead,
            }))
        })
    }
}

/// `host:port` when the URL points into the tailnet, else None.
fn tailnet_destination(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))?;
    let authority = rest.split(['/', '?']).next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|ch| ch.is_ascii_digit()) => {
            (host, port.parse().unwrap_or(7297u16))
        }
        _ => (authority, 7297u16),
    };
    let magic_dns = host
        .to_ascii_lowercase()
        .trim_end_matches('.')
        .ends_with(".ts.net");
    let cgnat = host
        .parse::<std::net::Ipv4Addr>()
        .is_ok_and(|ip| ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]));
    (magic_dns || cgnat).then(|| format!("{host}:{port}"))
}

/// The ahp transport over a websocket on a tailnet stream — the same
/// framing `ahp_ws` uses, which is tied to plain TcpStreams and so
/// cannot wrap this one.
struct WsOverTailnet {
    inner: tokio_tungstenite::WebSocketStream<tailnet::TailnetStream>,
}

impl ahp::transport::Transport for WsOverTailnet {
    async fn send(
        &mut self,
        msg: ahp::transport::TransportMessage,
    ) -> Result<(), ahp::TransportError> {
        use futures_util::SinkExt as _;
        use tokio_tungstenite::tungstenite::Message;
        let frame = match msg {
            ahp::transport::TransportMessage::Parsed(message) => {
                let text = serde_json::to_string(&message)
                    .map_err(|error| ahp::TransportError::Protocol(error.to_string()))?;
                Message::Text(text.into())
            }
            ahp::transport::TransportMessage::Text(text) => Message::Text(text.into()),
            ahp::transport::TransportMessage::Binary(bytes) => Message::Binary(bytes.into()),
        };
        self.inner
            .send(frame)
            .await
            .map_err(|error| ahp::TransportError::Io(error.to_string()))
    }

    async fn recv(
        &mut self,
    ) -> Result<Option<ahp::transport::TransportMessage>, ahp::TransportError> {
        use futures_util::StreamExt as _;
        use tokio_tungstenite::tungstenite::Message;
        loop {
            match self.inner.next().await {
                None => return Ok(None),
                Some(Err(error)) => return Err(ahp::TransportError::Io(error.to_string())),
                Some(Ok(Message::Text(text))) => {
                    return Ok(Some(ahp::transport::TransportMessage::Text(
                        text.to_string(),
                    )))
                }
                Some(Ok(Message::Binary(bytes))) => {
                    return Ok(Some(ahp::transport::TransportMessage::Binary(
                        bytes.into(),
                    )))
                }
                Some(Ok(Message::Close(_))) => return Ok(None),
                // Ping/Pong/raw frames: tungstenite answers these.
                Some(Ok(_)) => continue,
            }
        }
    }

    async fn close(&mut self) -> Result<(), ahp::TransportError> {
        self.inner
            .close(None)
            .await
            .map_err(|error| ahp::TransportError::Io(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::tailnet_destination;

    #[test]
    fn ts_net_names_and_cgnat_ips_route_through_the_node() {
        assert_eq!(
            tailnet_destination("ws://himark-studio.tail1234.ts.net:7297/"),
            Some("himark-studio.tail1234.ts.net:7297".to_owned())
        );
        assert_eq!(
            tailnet_destination("ws://himark-studio.tail1234.ts.net/"),
            Some("himark-studio.tail1234.ts.net:7297".to_owned()),
            "the fixed face port is the default"
        );
        assert_eq!(
            tailnet_destination("wss://a.b.TS.NET./x?y=1"),
            Some("a.b.TS.NET.:7297".to_owned())
        );
        assert_eq!(
            tailnet_destination("ws://100.64.0.2:7297/"),
            Some("100.64.0.2:7297".to_owned())
        );
    }

    #[test]
    fn everything_else_falls_through() {
        assert_eq!(tailnet_destination("ws://192.168.1.4:8000/?tkn=x"), None);
        assert_eq!(tailnet_destination("ws://localhost:9000/"), None);
        assert_eq!(tailnet_destination("unix:/tmp/host.sock"), None);
        assert_eq!(tailnet_destination("ws://100.200.0.1:7297/"), None);
        assert_eq!(tailnet_destination("ws://mytsnet.example.com/"), None);
    }
}
