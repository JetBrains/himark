# AHP Extension: Location Lists

This document specifies the Location Lists extension to the Agent
Host Protocol (AHP). It defines one shared vocabulary — a document
location carrying its own display context — and one channel scheme,
`ahp-locations:/`, over which location-producing requests stream
their results. Two producers are specified here: streaming content
search (`searchLocations`, a rider on the Search extension) and the
location-answering LSP asks (`lsp/locations`, a rider on the
Language Services extension). Both answer a channel URI immediately;
results arrive as channel actions; **unsubscribing cancels the
producer**.

Types are defined in `protocol/ahp-ext-types` (crate
`himark-ahp-ext-types`, module `locations`); the reference server is
`backend/agent-host`; the reference client surface is
`frontend/hiahp`.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be
interpreted as described in RFC 2119.

## 1. Availability

There is no capability advertisement. `initialize` negotiates only
the protocol version string (`"0.7.0"`); absence is the gate: a
server that does not serve these methods answers the standard
JSON-RPC *Method not found* error.

## 2. The channel

### 2.1 Types

```typescript
interface Location {
  /** Resource URI of the document. */
  uri: string;
  /** 0-based line. */
  line: number;
  /**
   * 0-based column, in UTF-8 bytes within the line — the unit the
   * Documents extension pins for TextPosition.character and the
   * LSP utf-8 negotiation forwards.
   */
  column: number;
  /** Match length in bytes; MAY run past the line end (a multi-line
   *  match); consumers clamp for display. Default 0. */
  length?: number;
  /**
   * A slice of the match's line for display: usually the whole hard
   * line; for huge lines a bounded window around the match.
   */
  context: string;
  /** The column (same unit) where `context` begins — 0 unless the
   *  line was too long to ship whole. The match renders inside
   *  `context` at `column - contextColumnStart`. Default 0. */
  contextColumnStart?: number;
}

interface LocationList {
  locations: Location[];
  /** The producer finished. Latches true. */
  done?: boolean;
  /** The producer stopped early — a limit or a cancellation cut the
   *  result off. Latches true. */
  truncated?: boolean;
}
```

### 2.2 State, action, reducer

Both the channel state and the channel's one action are a
`LocationList`. The action rides `StateAction::Unknown` with the
`"type"` field `"locations/extend"` beside the body. The reducer is
**concatenation**: `locations` append in action order, `done` and
`truncated` latch by OR. There are no deltas, no replacement, no
reordering vocabulary — the producer's emission order is the list
order, and a subscriber's snapshot is always the folded state.

### 2.3 Lifecycle

Channels are host-minted (`"ahp-locations:/<uuid>"`) and
**per-request** — every minting request answers a fresh channel;
nothing is idempotent. The producer starts at the request, not at
subscribe: results produced before the subscribe arrive in the
snapshot (the standard snapshot-then-actions contract absorbs the
race). Subscribing an unknown locations channel answers `-32001`
(NO SUCH CHANNEL).

**Unsubscribe is the cancel.** When a channel's last subscriber
leaves — by `unsubscribe` or by its connection closing — the server
cancels the producer and disposes the channel. Disposal emits no
action. Cancellation is best-effort and prompt, not instantaneous; a
producer past its last check simply finishes into a disposed channel.
A request whose channel is never subscribed is reaped when its
connection closes.

**Positions are a display snapshot, not live coordinates.** Unlike
the Search extension's deliberate positions-never-cross-the-wire
rule, a `Location` carries line/column/context — captured against
the text the producer read. Consumers MUST treat them as advisory:
render from `context`, and resolve `line`/`column` against live text
at navigation time, clamping when the document has moved.

### 2.4 Honesty

A producer MUST terminate its channel's story: its final action
carries `done: true`, with `truncated: true` when a limit or a
cancellation cut the walk short — a subscriber can always tell a
complete answer from a cut-off one. (The final action of a producer
cancelled by disposal is delivered to no one; that is fine — the
state dies with the channel.)

## 3. Methods

### 3.1 `searchLocations`

Streaming content search over the session's resources. Scope, walk
rules, and matching semantics are the Search extension's
(docs/ahp/ahp-search.md §2.2–§2.4): folders default to the session's
working directories and MUST be `file:` URIs under a root; ignore
rules respected, symlinks skipped, binaries skipped, the Rust regex
dialect; stored content only — no document-channel overlay.

```typescript
interface SearchLocationsParams {
  /** The session's channel URI. */
  channel: string;
  folders?: string[];
  query: string;
  /** "text" (default) or "regex". "fuzzy" answers Invalid params —
   *  this is content search. */
  kind?: "text" | "regex";
  /** Default false. */
  caseSensitive?: boolean;
  /** Total location cap; the server caps the effective limit. */
  limit?: number;
}

interface LocationsChannelResult { channel: string; }  // ahp-locations:/…
```

The server answers the channel, then walks; matches stream as
`locations/extend` actions, one action per matched file (a server MAY
split a file with very many matches across several actions). An
empty `query` answers a channel that immediately resolves
`{done: true}`. Per-file emission order within a folder is path
order; across folders, folder order.

### 3.2 `lsp/locations`

The location-answering LSP asks, streamed. Routing, text truth, and
position/URI semantics are the Language Services extension's
(docs/ahp/ahp-lsp.md §2.3, §3, §4), unchanged.

```typescript
interface LspLocationsParams {
  /** The session's channel URI. */
  channel: string;
  /** Whitelisted: "textDocument/references",
   *  "textDocument/implementation". Anything else answers
   *  Invalid params. */
  method: string;
  /** The LSP request's params, verbatim. */
  params: unknown;
}
// answers LocationsChannelResult
```

The server answers the channel, forwards the LSP request as
`lsp/{method}` would, converts the verbatim `Location[] |
LocationLink[]` answer (a `LocationLink` contributes its
`targetSelectionRange`) into this extension's `Location`s — deriving
`context` from the text truth it already owns: the synchronized
document channel when one exists (unflushed edits ARE observed,
unlike content search), the stored resource otherwise — and emits
one `locations/extend` per distinct result URI, then the `done`
action. Channel cancellation maps to `$/cancelRequest` toward the
language server through the in-flight map.

An LSP error, `NoLanguageServer`, or a null result after the channel
was minted resolves the channel with `{done: true, truncated: true}`
(empty on error, the converted results otherwise); the request
itself errors only when it fails before minting (unknown session,
excluded method).

## 4. Errors

| condition | answer |
|---|---|
| unknown session channel | JSON-RPC error `-32602` (Invalid params) |
| `kind: "fuzzy"` on `searchLocations` | `-32602` |
| invalid regular expression | `-32602` |
| a `folders` entry that is not a `file:` URI, or outside the roots | `-32602` |
| `lsp/locations` with a non-whitelisted `method` | `-32602` |
| subscribe on an unknown locations channel | `-32001` (no such channel) |
| no matches / no references | **not an error** — the channel resolves `{locations: [], done: true}` |

## 5. Client notes (non-normative)

The reference client subscribes the answered channel immediately and
treats supersession as lifecycle: replacing a query or closing the
consuming surface unsubscribes, which is the cancel — no serials
cross the wire (stale streams are still generation-gated client-side
against late landings). The Search extension's `search` method is
unaffected and remains the quick-open path lane
(`kind: "fuzzy"`, `target: "path"`).
