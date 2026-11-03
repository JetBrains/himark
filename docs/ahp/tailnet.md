# Tailnet remoting (design)

Status: design, nothing landed. Companion to docs/ahp/remote.md,
which describes the remoting model and today's web face.

## Problem

Every transport exists (unix socket, HTTP/WebSocket face, `wss`-capable
client wire), but reaching an arbitrary host is still hostile: the web
face advertises a guessed LAN IP with an in-memory token that dies on
every restart (`backend/agent-host/src/http.rs`), there is no TLS, no
caller identity, no saved host list, and no stable host identity —
`HostId` is minted per app run (`frontend/ahp-wire/src/client.rs`).
"Connect your phone to your desk machine" means pasting a
`ws://…?tkn=` URL that will be dead by tomorrow.

## Model

The host joins the user's Tailscale tailnet as its own device, and
**identity replaces credentials**: the server asks Tailscale who a
connection is from and admits the owner. (This is the Rex model —
Superlogical's terminal does exactly this with tsnet.)

**The zero-config law: the user has a Tailscale account and clicks
Log in once per machine. Nothing else** — no URLs, no ports, no admin
console, no ACL edits, no cert toggles. Every design choice below is
subordinate to this law.

Tailscale is embedded via libtailscale (Go core compiled to a static
archive, C API; `rsnet`/`libtailscale` crates wrap it for tokio) — in
the host **and in every client**. We do not depend on the Tailscale
app anywhere.

## Enrollment

`himark-agent-host --tailscale`, or a drawer action on desktop. The
embedded node generates its keys locally; we poll `status_json` until
an `AuthURL` appears, open it (browser on desktop, printed link for
headless, `ASWebAuthenticationSession` on iOS), and wait for state
`Running`. The login persists in a state dir under
`~/.himark/agent-host/tailscale/` (host) or the app container
(clients); libtailscale owns its contents (`tailscale_set_dir` is the
only storage hook its C API exposes).

We set the device hostname ourselves: `himark-<machinename>`.
Non-ephemeral nodes (ephemeral ones are dropped when offline and
force re-login). No auth keys — the URL flow covers headless too.

What's stored is a locally generated private keypair, not a fetched
token: it never crosses the network, it is revocable per device from
the Tailscale console, and it expires (≤180 days, tailnet default).
Expiry is handled as UX, never as required console config: the drawer
shows a countdown from `status_json`, clients nudge ahead of time,
re-auth is the same login sheet.

## Listener, transport

A tailnet listener on a fixed port (7297), accepting the same
JSON-RPC-over-WebSocket the HTTP face speaks today
(`Host::serve_ws`, `backend/agent-host/src/server.rs`). Plain `ws://`:
WireGuard already encrypts and authenticates the path, so no ts.net
HTTPS certs (enabling them is a console step — forbidden by the law)
and no token (`?tkn=` survives only on the legacy LAN share path).
The listener binds the tailnet only — never `0.0.0.0`, and the
8.8.8.8 LAN-IP guess in `http.rs` does not apply here.

## Identity and authorization

Per connection the host resolves the caller via WhoIs on the embedded
node's LocalAPI (`/localapi/v0/whois?addr=…` over `tailscale_loopback`;
the C API has no direct WhoIs call). Policy:

- **Default: owner-only.** The caller's tailnet login must equal the
  login that enrolled this host. Same account on both ends, fresh
  personal tailnets default to allow-all ACLs — so this passes with
  zero configuration.
- Sharing with other tailnet members is a later, explicit opt-in
  (Rex's `--tailscale-allow-tailnet-members` shape). No multi-account
  login-server machinery: one agent-host per user stands.

## Second factor: paired client identity

Tailnet identity alone means a stolen client state dir commands every
host. Two app-level layers close that:

- **Hardware-bound client key.** Each client holds a P-256 keypair —
  Secure Enclave on iOS/macOS (non-exportable by construction), key
  file elsewhere. At `initialize` the host sends a nonce; the client
  signs it. Tailscale's Curve25519 node key must live on disk; our
  challenge key doesn't have to.
- **TOFU pairing.** A client key's first appearance requires one-tap
  approval on the host machine or an already-paired device; approvals
  persist. This also neutralizes a phished Tailscale account
  enrolling a fresh "you" device.

A stolen host state dir only lets a thief impersonate that one host
(a decoy), not command others; host fingerprints in the drawer, no
heavier machinery.

## Discovery and the host list

No address entry. The client's embedded node already sees the
tailnet: `status_json` lists peers with hostnames and owners. The
drawer shows peers named `himark-*` owned by the same login, as rows
labeled by machine name — hosts appear everywhere the moment they
enroll. Dial by tailnet IP from status (MagicDNS names also work;
default-on since 2022, but we don't depend on it).

The tailnet node ID becomes the stable host identity that today's
runtime-only `HostId` lacks; the saved host list (label, node ID,
pairing state) persists across launches. Last-seen `serverSeq` and
reconnect semantics are unchanged.

## Client wiring

A `TailnetConnector` implementing `Connector::dial`
(`frontend/ahp-wire/src/transport.rs`) — the seam already exists.
`tailscale_dial` returns a pipe fd; wrap it for tokio (kqueue/epoll
poll fine; `rsnet` demonstrates this) and hand the stream to
tungstenite. `DesktopConnector` keeps `unix:` and legacy URLs. All
reconnect machinery (`WireHost`, replay ring, `reconnect(clientId,
lastSeen)`) is transport-agnostic and untouched.

iOS: state dir in the app container with
`NSFileProtectionCompleteUntilFirstUserAuthentication` (reconnects
must work locked-in-pocket). Userspace networking — no VPN
entitlement. App deletion loses the node key: user re-logs-in, a dead
device lingers in their console; accepted.

The browser is the one client that cannot embed a node. The web face
stays as-is (served by the host, reachable when the browsing machine
is on the tailnet by other means); not part of the zero-config story.

## Phasing

1. **Host node + owner check.** Embedded node, tailnet `ws` listener,
   WhoIs owner-only policy. Desktop client dials via embedded node.
2. **Discovery + persistence.** Peer-scan drawer rows, saved host
   list keyed by node ID, expiry countdown UX.
3. **iOS.** Login sheet, file protection, device-approval
   ("waiting for admin") state for work tailnets.
4. **Pairing.** Enclave challenge at `initialize`, TOFU approvals.

## Risks

- `rsnet`/`libtailscale` crates are community-maintained; eval spike
  before committing. Go toolchain joins the build (CI already
  source-builds skia).
- C API gaps: no WhoIs (LocalAPI detour), no custom state store (no
  Keychain-backed node state without forking; sandbox + file
  protection is the accepted posture).
- Embedded node per client app = one tailnet device per install;
  fine for personal tailnets, inventory noise at worst.
- Work tailnets may enforce device approval or tailnet lock; render
  the pending state, don't fight it.
