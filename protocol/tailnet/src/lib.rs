// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The embedded tailnet node (docs/ahp/tailnet.md). Wraps
//! libtailscale — Tailscale's Go core as a c-archive — so a host or
//! client joins the user's tailnet as its own device: no Tailscale
//! app, no tokens, identity comes from the tailnet itself.
//!
//! The node is a process-lifetime singleton. Status, identity and
//! WhoIs go through the node's LocalAPI on a loopback socket; dials
//! and listens produce socketpair fds that surface here as tokio
//! streams.

#![cfg(unix)]

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// One side of a tailnet connection. The fds libtailscale hands out
/// are socketpairs (AF_UNIX) — the tunnel itself lives inside the Go
/// core — so a UnixStream is their honest tokio shape.
pub type TailnetStream = tokio::net::UnixStream;

pub struct Tailnet {
    /// Leaked on purpose: the node lives as long as the process, and
    /// libtailscale's `Listener<'a>` borrows it.
    ts: &'static libtailscale::Tailscale,
    localapi_address: String,
    localapi_auth: String,
}

impl Tailnet {
    /// Create (or resume) the node: state in `state_dir`, device
    /// named `hostname`. Returns as soon as the node is started —
    /// it may still be waiting for a login; poll [`Tailnet::status`]
    /// for `AuthURL` / `Running`.
    pub fn start(state_dir: &Path, hostname: &str) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(state_dir)
            .map_err(|error| format!("tailnet state dir {}: {error}", state_dir.display()))?;
        let dir = state_dir
            .to_str()
            .ok_or_else(|| "tailnet state dir is not utf-8".to_owned())?;
        let mut ts = libtailscale::Tailscale::new();
        ts.set_dir(dir)?;
        ts.set_hostname(hostname)?;
        ts.start()?;
        let loopback = ts.loopback()?;
        let auth = base64::engine::general_purpose::STANDARD
            .encode(format!(":{}", loopback.credential));
        Ok(Arc::new(Self {
            ts: Box::leak(Box::new(ts)),
            localapi_address: loopback.address,
            localapi_auth: auth,
        }))
    }

    pub async fn status(&self) -> Result<Status, String> {
        let body = self.localapi_get("/localapi/v0/status").await?;
        serde_json::from_str(&body).map_err(|error| format!("status json: {error}"))
    }

    /// Who is on the far end of a connection from `remote`? Answers
    /// from the tailnet's coordination state — this is the identity
    /// that replaces tokens.
    pub async fn whois(&self, remote: IpAddr) -> Result<WhoIs, String> {
        let body = self
            .localapi_get(&format!("/localapi/v0/whois?addr={remote}"))
            .await?;
        serde_json::from_str(&body).map_err(|error| format!("whois json: {error}"))
    }

    /// Dial `address` ("host:port" — MagicDNS names and tailnet IPs
    /// both resolve inside the node).
    pub async fn dial(self: &Arc<Self>, address: String) -> Result<TailnetStream, String> {
        let node = Arc::clone(self);
        let stream = tokio::task::spawn_blocking(move || node.ts.dial("tcp", &address))
            .await
            .map_err(|join| format!("tailnet dial task: {join}"))??;
        to_tokio(stream)
    }

    /// Listen on `port` on the node's tailnet address.
    pub fn listen(self: &Arc<Self>, port: u16) -> Result<TailnetListener, String> {
        let listener = self.ts.listen("tcp", &format!(":{port}"))?;
        Ok(TailnetListener {
            _node: Arc::clone(self),
            inner: Arc::new(listener),
        })
    }

    async fn localapi_get(&self, path: &str) -> Result<String, String> {
        let mut stream = tokio::net::TcpStream::connect(&self.localapi_address)
            .await
            .map_err(|error| format!("localapi connect: {error}"))?;
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nSec-Tailscale: localapi\r\nAuthorization: Basic {auth}\r\nConnection: close\r\n\r\n",
            host = self.localapi_address,
            auth = self.localapi_auth,
        );
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|error| format!("localapi write: {error}"))?;
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .await
            .map_err(|error| format!("localapi read: {error}"))?;
        let text = String::from_utf8_lossy(&raw);
        let (head, body) = text
            .split_once("\r\n\r\n")
            .ok_or_else(|| "localapi: no header/body split".to_owned())?;
        let status_line = head.lines().next().unwrap_or("");
        if !status_line.contains(" 200 ") {
            return Err(format!("localapi {path}: {status_line}: {body}"));
        }
        // Connection: close — chunked bodies still arrive whole;
        // strip the framing if present.
        if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
            return Ok(unchunk(body));
        }
        Ok(body.to_owned())
    }
}

pub struct TailnetListener {
    _node: Arc<Tailnet>,
    inner: Arc<libtailscale::Listener<'static>>,
}

impl TailnetListener {
    /// Accept the next connection and its remote tailnet address.
    pub async fn accept(&self) -> Result<(TailnetStream, IpAddr), String> {
        let inner = Arc::clone(&self.inner);
        let (stream, addr) = tokio::task::spawn_blocking(move || inner.accept_with_addr())
            .await
            .map_err(|join| format!("tailnet accept task: {join}"))??;
        Ok((to_tokio(stream)?, addr))
    }
}

fn to_tokio(stream: std::net::TcpStream) -> Result<TailnetStream, String> {
    use std::os::fd::{FromRawFd, IntoRawFd};
    let fd = stream.into_raw_fd();
    let unix = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
    unix.set_nonblocking(true)
        .map_err(|error| format!("tailnet stream nonblocking: {error}"))?;
    tokio::net::UnixStream::from_std(unix)
        .map_err(|error| format!("tailnet stream register: {error}"))
}

fn unchunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, tail)) = rest.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if size == 0 || size > tail.len() {
            break;
        }
        out.push_str(&tail[..size]);
        rest = tail[size..].trim_start_matches("\r\n");
    }
    out
}

/// The device name a HOST enrolls under: `himark-<machine>`,
/// lowercased to tailnet hostname rules. The prefix is also how
/// clients spot agent hosts among tailnet peers.
pub const HOSTNAME_PREFIX: &str = "himark-";

/// Client devices (a phone, a laptop that only connects out) enroll
/// under this prefix so host discovery never lists them.
pub const CLIENT_PREFIX: &str = "himark-client-";

pub fn default_client_hostname() -> String {
    let machine = default_hostname();
    format!(
        "{CLIENT_PREFIX}{}",
        machine.trim_start_matches(HOSTNAME_PREFIX)
    )
}

pub fn default_hostname() -> String {
    let mut buf = [0u8; 256];
    let raw = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    let machine = match raw {
        0 => {
            let end = buf.iter().position(|byte| *byte == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..end]).into_owned()
        }
        _ => "host".to_owned(),
    };
    let machine = machine.strip_suffix(".local").unwrap_or(&machine);
    let mut name = HOSTNAME_PREFIX.to_owned();
    for ch in machine.chars().take(50) {
        match ch.to_ascii_lowercase() {
            ch if ch.is_ascii_alphanumeric() || ch == '-' => name.push(ch),
            '.' | '_' | ' ' => name.push('-'),
            _ => {}
        }
    }
    name.trim_end_matches('-').to_owned()
}

/// LocalAPI JSON writes absent collections as explicit `null`s —
/// every container field tolerates both.
fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::Deserialize<'de>,
{
    let value = <Option<T> as serde::Deserialize>::deserialize(deserializer)?;
    Ok(value.unwrap_or_default())
}

#[derive(Debug, serde::Deserialize)]
pub struct Status {
    #[serde(rename = "BackendState", default)]
    pub backend_state: String,
    #[serde(rename = "AuthURL", default, deserialize_with = "null_default")]
    pub auth_url: String,
    #[serde(rename = "Self", default)]
    pub self_node: Option<NodeStatus>,
    #[serde(rename = "User", default, deserialize_with = "null_default")]
    pub users: HashMap<String, UserProfile>,
    #[serde(rename = "Peer", default, deserialize_with = "null_default")]
    pub peers: HashMap<String, NodeStatus>,
}

impl Status {
    pub fn running(&self) -> bool {
        self.backend_state == "Running"
    }

    /// The tailnet login that enrolled THIS node — the owner every
    /// caller is checked against.
    pub fn owner_login(&self) -> Option<&str> {
        let user_id = self.self_node.as_ref()?.user_id;
        let profile = self.users.get(&user_id.to_string())?;
        Some(&profile.login_name)
    }

    /// Peers that look like agent hosts owned by the same login.
    pub fn agent_host_peers(&self) -> impl Iterator<Item = &NodeStatus> {
        let owner = self.self_node.as_ref().map(|own| own.user_id);
        self.peers.values().filter(move |peer| {
            Some(peer.user_id) == owner
                && peer.host_name.starts_with(HOSTNAME_PREFIX)
                && !peer.host_name.starts_with(CLIENT_PREFIX)
        })
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct NodeStatus {
    #[serde(rename = "HostName", default)]
    pub host_name: String,
    #[serde(rename = "DNSName", default)]
    pub dns_name: String,
    #[serde(rename = "TailscaleIPs", default, deserialize_with = "null_default")]
    pub ips: Vec<IpAddr>,
    #[serde(rename = "UserID", default)]
    pub user_id: u64,
    #[serde(rename = "Online", default)]
    pub online: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct UserProfile {
    #[serde(rename = "LoginName", default)]
    pub login_name: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct WhoIs {
    #[serde(rename = "UserProfile")]
    pub user_profile: Option<UserProfile>,
    #[serde(rename = "Node")]
    pub node: Option<NodeStatus>,
}

impl WhoIs {
    pub fn login(&self) -> Option<&str> {
        let login = &self.user_profile.as_ref()?.login_name;
        (!login.is_empty()).then_some(login.as_str())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn status_parses_the_fields_we_route_on() {
        let json = r#"{
            "BackendState": "Running",
            "AuthURL": "",
            "Self": {"HostName": "himark-mini", "DNSName": "himark-mini.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.1"], "UserID": 7, "Online": true},
            "User": {"7": {"LoginName": "jet@example.com"}},
            "Peer": {
                "k1": {"HostName": "himark-studio", "DNSName": "himark-studio.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.2"], "UserID": 7, "Online": true},
                "k2": {"HostName": "printer", "TailscaleIPs": ["100.64.0.3"], "UserID": 7, "Online": true},
                "k3": {"HostName": "himark-other", "TailscaleIPs": ["100.64.0.4"], "UserID": 9, "Online": false},
                "k4": {"HostName": "himark-client-phone", "TailscaleIPs": ["100.64.0.5"], "UserID": 7, "Online": true}
            }
        }"#;
        let status: super::Status = serde_json::from_str(json).expect("parses");
        assert!(status.running());
        assert_eq!(status.owner_login(), Some("jet@example.com"));
        let hosts: Vec<_> = status
            .agent_host_peers()
            .map(|peer| peer.host_name.as_str())
            .collect();
        assert_eq!(hosts, ["himark-studio"], "same owner, himark- prefix only");
    }

    #[test]
    fn status_tolerates_the_nulls_of_a_logged_out_node() {
        let json = r#"{
            "BackendState": "NeedsLogin",
            "AuthURL": null,
            "Self": {"HostName": "", "TailscaleIPs": null, "UserID": 0},
            "User": null,
            "Peer": null
        }"#;
        let status: super::Status = serde_json::from_str(json).expect("parses");
        assert!(!status.running());
        assert!(status.auth_url.is_empty());
        assert_eq!(status.agent_host_peers().count(), 0);
    }

    #[test]
    fn whois_login_is_none_when_unidentified() {
        let whois: super::WhoIs =
            serde_json::from_str(r#"{"UserProfile": {"LoginName": ""}}"#).expect("parses");
        assert_eq!(whois.login(), None);
        let whois: super::WhoIs = serde_json::from_str("{}").expect("parses");
        assert_eq!(whois.login(), None);
    }

    #[test]
    fn default_hostname_is_prefixed_and_clean() {
        let name = super::default_hostname();
        assert!(name.starts_with(super::HOSTNAME_PREFIX));
        assert!(name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'));
    }

    #[test]
    fn unchunk_reassembles() {
        assert_eq!(super::unchunk("5\r\nhello\r\n1\r\n!\r\n0\r\n\r\n"), "hello!");
    }
}
