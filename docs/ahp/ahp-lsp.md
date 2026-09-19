# AHP Extension: Language Services

This document specifies the Language Services extension to the Agent
Host Protocol (AHP). It gives clients access to Language Server
Protocol (LSP) features — definition, references, hover, completion,
and the rest — for a session's resources, with the server owning the
language servers and every stateful half of LSP.

The design principle: **LSP requests and responses pass through
unchanged; LSP state does not pass through at all.** The server is
the single LSP client of each language server it runs. It owns
lifecycle (`initialize`, capability negotiation, shutdown) and text
synchronization (`didOpen`/`didChange`, watched files) — none of
which ever appear on this wire. What crosses the wire is LSP's
stateless request/response surface, verbatim, multiplexed across any
number of protocol clients. Because the calls that cross are
stateless, multiplexing them requires nothing beyond routing.

This document does not restate LSP. Method names, parameter and
result shapes, and error semantics of forwarded calls are those of
the LSP specification, unmodified except where §4 says otherwise.

The reference server is `backend/agent-host` (`src/lsp.rs` and the
`lsp/` surface of `src/server.rs`); the reference client route is
`frontend/hiahp/src/lsproute.rs`.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be
interpreted as described in RFC 2119.

## 1. Availability

There is no capability advertisement. `initialize` negotiates only
the protocol version string (`"0.7.0"`); absence is the gate: a
server that does not serve the `lsp/` method space answers the
standard JSON-RPC *Method not found* error, and a client arms its
language features on the resource authority it connects to, not on
an advertised flag.

## 2. The forwarding surface

### 2.1 Method space

Every forwardable LSP request or notification is exposed as a
protocol method named:

```
lsp/{LSP method name}
```

e.g. `lsp/textDocument/definition`, `lsp/textDocument/completion`,
`lsp/textDocument/hover`, `lsp/textDocument/references`. The names
`lsp/capabilities` and `lsp/diagnostics` are reserved by this
extension (§5, §6) and are not LSP methods.

### 2.2 Envelope

Forwarded calls wrap their LSP params in a routing envelope:

```typescript
interface LspRequestParams {
  /** The session's channel URI. */
  channel: string;
  /** The LSP request's params, verbatim. */
  params: unknown;
}
```

An envelope without `params` answers *Invalid params* (`-32602`); an
unknown session channel answers `-32602`.

The result of a forwarded request is the LSP result, **verbatim** —
no envelope. An error from the language server passes through as a
JSON-RPC error with the language server's code and message. Each
forwarded request is served on its own task: a slow language-server
answer never blocks the connection.

### 2.3 Routing

The routing target is `params.textDocument.uri` when the forwarded
params carry one; otherwise the placeholder `<first working
directory>/_`, so workspace-scoped calls route by the session alone.
The server chooses the language server by matching the target's file
extension against its language-server configuration, rooted at the
longest session working directory containing the target (the
reference server's default configuration runs rust-analyzer for
`.rs`). When no configuration row matches, no working directory
contains the target, or the chosen server cannot be started, the
call answers `NoLanguageServer` (`-33002`) — distinct from a
language server answering an empty result.

### 2.4 Excluded methods

The following LSP methods are the server's own and MUST NOT be sent
by clients; the server refuses them with `MethodNotAllowed`
(`-33001`), without forwarding:

- lifecycle: `initialize`, `initialized`, `shutdown`, `exit`,
  `$/setTrace`
- text synchronization: `textDocument/didOpen`,
  `textDocument/didChange`, `textDocument/didClose`,
  `textDocument/didSave`, `textDocument/willSave`,
  `textDocument/willSaveWaitUntil`
- workspace state: `workspace/didChangeWatchedFiles`,
  `workspace/didChangeConfiguration`,
  `workspace/didChangeWorkspaceFolders`

All other LSP requests and client-to-server notifications are
forwardable.

## 3. Text: the server's single truth

Language servers see one text truth, maintained by the server:

- A resource open as a document under the Documents extension is
  synchronized from its **document channel**: the server sends
  `textDocument/didOpen` when the document opens and
  `textDocument/didChange` for every applied operation, with the
  operation's replacements sent in reverse order (last range first),
  so each LSP change addresses text untouched by the changes before
  it in the same notification. Document versions are opaque
  identifiers while LSP versions are integers, so the server
  maintains its own monotonic integer version per synchronized URI
  and keeps the mapping; anything version-tagged coming back to
  clients (§6) carries the DOCUMENT VERSION IDENTIFIER, directly
  comparable to document channel state.
- A forwarded request naming a URI not yet synchronized causes the
  server to `didOpen` that file from disk (LSP version 0) before
  forwarding.

Clients never send text through this extension. A client whose edits
should be visible to language services makes them through the
Documents extension; the server carries them onward.

## 4. Positions and URIs

- **Positions**: the server initializes every language server with
  `general.positionEncodings: ["utf-8"]` and applies no conversion
  layer — positions in forwarded params and results are UTF-8 code
  unit offsets within the line, by negotiation. This is the same
  unit the Documents extension pins for `TextPosition.character`.
- **URIs**: resource URIs inside forwarded params and results are
  the protocol's resource URIs — the same URI space `resourceRead`
  and the session's `workingDirectories` use.

## 5. `lsp/capabilities`

Answers the negotiated capabilities of the language server
responsible for a resource, so clients can gate their features the
way an LSP client would after `initialize`.

```typescript
interface LspCapabilitiesParams {
  /** The session's channel URI. */
  channel: string;
  /** The resource whose language server is being asked about. */
  uri: string;
}

type LspCapabilitiesResult = {
  /** LSP ServerCapabilities, verbatim. */
  capabilities: unknown;
} | null;
```

An unknown session channel answers `-32602`. `null` means no
language server serves the resource (or its handshake has not
completed). The answer MAY change over a session's lifetime; clients
SHOULD treat it as a hint, and treat `NoLanguageServer` on a later
call as the authoritative signal.

## 6. Diagnostics

Diagnostics are pushed by language servers, not requested; they are
therefore **state**, and travel as a channel with the protocol's
standard contract: subscribe answers a snapshot; updates arrive as
actions applied by a pure reducer. Diagnostics are the extension's
one server-push surface — everything else a language server
initiates stays server-side (§7.2).

### 6.1 `lsp/diagnostics`

```typescript
interface LspDiagnosticsParams {
  /** The session's channel URI. */
  channel: string;
}

interface LspDiagnosticsResult {
  /** The session's diagnostics channel URI. */
  channel: string; // "ahp-lsp-diagnostics:/<uuid>"
}
```

One diagnostics channel per session; the request is idempotent — a
repeat call answers the existing channel. An unknown session channel
answers `-32602`.

### 6.2 Subscribe and snapshot

Subscribing the diagnostics channel answers:

```json
{ "snapshot": { "resource": "ahp-lsp-diagnostics:/…",
                "state": { "items": { "<uri>": { "version": "…",
                                                  "diagnostics": [] } } },
                "fromSeq": 42 } }
```

```typescript
interface DiagnosticsState {
  /** Keyed by resource URI. Resources without diagnostics are absent. */
  items: { [uri: string]: PublishedDiagnostics };
}

interface PublishedDiagnostics {
  /**
   * The document channel VERSION IDENTIFIER the diagnostics were
   * computed against (§3). Absent when the resource is not
   * synchronized from a document, or when the language server's
   * reported version does not match the synchronized one.
   */
  version?: string;
  /** LSP Diagnostic[], verbatim (positions per §4). */
  diagnostics: unknown[];
}
```

Subscribing an unknown diagnostics channel answers `-32001` (NO SUCH
CHANNEL).

### 6.3 Action: `lspDiagnostics/published`

Extension actions ride `StateAction::Unknown`: the body travels with
a `"type"` field beside it naming the action.

```typescript
interface LspDiagnosticsPublishedAction {
  // "type": "lspDiagnostics/published"
  uri: string;
  /** The document version identifier, as in the snapshot (§6.2). */
  version?: string;
  diagnostics: unknown[];
}
```

Reducer: **replacement** — the entry for `uri` is replaced whole,
and removed when `diagnostics` is empty, mirroring LSP
`textDocument/publishDiagnostics` semantics. There are no diagnostic
deltas.

## 7. Cancellation and server-initiated traffic

### 7.1 Cancellation

The base protocol defines no request cancellation; this extension
supplies one for its own request space, mirroring the LSP
notification it maps to. A client cancels an in-flight forwarded
request with the notification:

```
lsp/$/cancelRequest   params: { id: number }
```

where `id` is the JSON-RPC id of the forwarded request being
cancelled, on the same connection. The server maps it to
`$/cancelRequest` toward the language server through an
overtake-proof in-flight map: a cancellation that arrives before the
request has reached its language server is remembered and delivered
the moment it does. Per LSP, cancellation is best-effort: the
forwarded request still completes, either with its result or with
the LSP `RequestCancelled` error (`-32800`) passed through. Servers
ignore the notification for unknown ids.

### 7.2 Server-initiated LSP traffic

Nothing a language server initiates crosses this wire, and clients
MUST NOT expect otherwise:

- `textDocument/publishDiagnostics` feeds the diagnostics channel
  (§6) — the one server-push surface.
- `$/progress` is dropped. There is no progress forwarding; clients
  do not mint progress tokens.
- Every server-initiated **request** (`workspace/configuration`,
  `client/registerCapability`, `window/workDoneProgress/create`,
  `workspace/applyEdit`, …) is answered `{"result": null}` by the
  server itself.

This is deliberate: the forwarding surface stays stateless, and
state reaches clients only as channels. Diagnostics earned a channel
because they are the one push stream clients render.

## 8. Errors

Extension-defined error codes:

| code | name | meaning |
|---|---|---|
| `-33001` | `MethodNotAllowed` | an excluded LSP method (§2.4) was sent |
| `-33002` | `NoLanguageServer` | no language server serves the call's target |

Other conditions:

| condition | answer |
|---|---|
| unknown session channel | JSON-RPC error `-32602` (Invalid params) |
| envelope without `params` | JSON-RPC error `-32602` |
| language server answered an LSP error | that error, passed through verbatim |
| forwarded request cancelled | LSP `RequestCancelled` (`-32800`), passed through |
| subscribe on an unknown diagnostics channel | JSON-RPC error `-32001` (no such channel) |

## 9. Client notes (non-normative)

The reference client currently forwards `textDocument/completion`,
`textDocument/hover`, `textDocument/definition`, and
`textDocument/references`, and renders the diagnostics channel.
