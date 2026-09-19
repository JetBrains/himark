# Remote hosts

himark's remoting unit is the **agent host** (docs/ahp/agent-host.md):
himark's workspace-services process, standing next to the working
tree, outliving the editor. Sessions, chats, terminals, documents,
vcs, search and lsp are all served over AHP on one socket — so
"remote" is not a feature layered onto the editor, it is the same
host reached over a different wire.

## Why the host is the remoting unit

The services must run where the files are: the agents mutate the
working tree, git runs against it, search walks it, language servers
index it, PTYs fork beside it. Everything the user feels per
keystroke — documents, markup, layout, paint — stays in the engine.
**The latency rule: nothing on the keystroke path crosses the wire.**
What crosses is occasional and stream-shaped: channel actions,
resource reads and writes, watch batches, terminal bytes, search
results. Collaborative documents keep edits local and converge
through their channels (docs/ahp/ahp-documents.md); reconnect replays the
missed action tail from the host's ring or falls back to fresh
snapshots (docs/ahp/agent-host.md §3).

Because every service is already a method or channel family on the
AHP socket, remoting adds no second protocol: there is nothing
terminal-specific, vcs-specific or search-specific about the network
story.

## The off-machine road: the web face

`httpServe {enabled, bind?, webRoot?}` starts a token-gated HTTP +
WebSocket face on the host:

- binds a port and mints a random token;
- advertises `http://<lan-ip>:<port>/?tkn=…` and writes the URL into
  the lockfile's `http` field;
- serves the web build as static files with cross-origin-isolation
  headers (COOP/COEP — the wasm build needs them);
- injects the AHP URL into the served page
  (`window.HIMARK_AHP_URL`), sets the token as an `HttpOnly` cookie;
- upgrades `/` to a WebSocket serving the same AHP server.

Auth is the `tkn` query parameter or the `himark_tkn` cookie; a
missing or wrong token answers 403. The unix socket itself carries no
auth — it lives under the user's home; the token gate exists exactly
where the network does.

The client-side affordance is the "Share Host over HTTP" command:
`ShareHostEffect` asks the local seat for `httpServe`, and the minted
URL lands on the clipboard.

## One seat, three transports

The client's transports sit behind one `Connector` trait
(`hiahp::transport`): ndjson over a unix socket on desktop, WebSocket
for remote faces, and a `BrowserConnector` in the web build. A remote
host is therefore the same `WireHost` seat with a different dial
string — the add-host-by-URL row mints one (`WireHost::at`,
docs/ahp/agents.md), and any send/recv failure latches the connection
dead so reconnect can take over.

The design consequence is the whole point: **remote and local differ
only in transport.** The seat trait, the effects, the drawer, the
panels, the state shape — none of them know the difference
(docs/ui/app-state.md). A session on a host across the network is a row
in the same drawer, a place to work like any other.
