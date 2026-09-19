# AHP Extension: Documents

This document specifies the Documents extension to the Agent Host
Protocol (AHP). It defines shared, versioned text buffers —
**documents** — synchronized between the server and any number of
clients through the protocol's standard channel mechanism.

A document is a buffer of text. It may mirror a resource, or exist on
its own. Every change to a document — a client's edit, the server
reconciling mirrored content, an edit produced by a tool — is
expressed uniformly as an **operation** applied to a specific version
of the buffer.

Types are defined in `protocol/ahp-ext-types` (crate
`himark-ahp-ext-types`); the reference server is
`backend/agent-host`; the reference client synchronizer is
`frontend/hiahp/src/docsync.rs`.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be
interpreted as described in RFC 2119.

## 1. Availability

There is no capability advertisement. `initialize` negotiates only
the protocol version string (`"0.7.0"`); absence is the gate: a
server that does not serve `openDocument` or `storeDocument` answers
the standard JSON-RPC *Method not found* error, and a client arms its
document features on the resource authority it connects to, not on an
advertised flag.

## 2. Concepts

### 2.1 Document

A document is identified by a server-minted channel URI:

```
ahp-document:/<seq>
```

where `<seq>` is a server-assigned sequence number. The scheme is
server-defined and opaque; clients never mint document URIs. A
document's state is a text and a version. A document is
session-scoped: it is created within a session and lives no longer
than that session.

### 2.2 Version

A document's version is an opaque **identifier** — a `Uid` — unique
within the document, not a counter. On the wire a `Uid` is a string
of 32 lowercase hex digits (a 128-bit value); parsers accept and
strip `-`, so a dashed UUID spelling is a valid input. The version at
creation is minted by the server; every applied operation carries the
identifier of the version it PRODUCES (its action `id`, §4.2), so
"the version" at any moment is the `id` of the last applied action
(or the creation version when none has). Versions are per-document
and totally ordered by the channel's action stream; equality of
identifiers is the only comparison.

Identifiers are what make a claimed base **unforgeable** — this is
the design rationale for not using integers. An integer version
cannot ground a discard: a client that dispatches two operations back
to back — B built on its own not-yet-confirmed A, claiming base
version `v+1` — would have B's claim *numerically* satisfied even if
a foreign operation, not A, produced `v+1`, and B would silently
apply against text its positions never addressed. With identifiers,
the version produced by the foreign action is a different id than A's
mint, B's claimed base names A's, and the server can tell.

### 2.3 Text and lines

Document text is Unicode. On the wire it travels as JSON strings,
stored and delivered **verbatim** — the extension never normalizes
line terminators or any other content.

For the purpose of positions, the text divides into **lines**:

- A line terminator is `"\n"`, optionally preceded by `"\r"` — a
  `"\r"` immediately before a `"\n"` belongs to the terminator, not
  to the line's content. A `"\r"` not followed by `"\n"` is ordinary
  line content.
- A line's **content** is its text excluding its terminator.
- A terminator ends a line and begins the next; text ending in a
  terminator therefore has a final empty line. A document always has
  at least one line (the empty text has one empty line).

### 2.4 Positions and ranges

Positions address the text as **line/character coordinates** — never
as absolute offsets into the whole text — using the shared types:

```typescript
interface TextPosition {
  line: number;      // zero-based line index
  character: number; // zero-based offset within the line's content
}

interface TextRange {
  start: TextPosition;
  end: TextPosition;  // end >= start, (line, character) lexicographic
}
```

`character` counts UTF-8 code units of the line's content, MUST fall
on a Unicode scalar-value boundary, and MUST NOT exceed the line
content's length. `line` MUST be a valid line index of the addressed
text (§2.3's line model, final empty line included).

- Positions address **line content only**; the inside of a line
  terminator is not addressable. A range covers a terminator by
  spanning lines: the range from the end of line *L*'s content to
  `{ line: L + 1, character: 0 }` covers line *L*'s terminator,
  whole. Deleting or replacing a line break is expressed exactly
  that way.

Line/character addressing is deliberate: a disagreement about line
terminator representation (`"\r\n"` versus `"\n"`) perturbs only the
terminators themselves — line indices and character offsets within
lines are identical either way, where absolute offsets into the
whole text would drift by one per preceding terminator.

### 2.5 Operation

An operation is an atomic set of replacements against one specific
version of the text:

```typescript
interface TextOperation {
  replacements: Replacement[];
}

interface Replacement {
  /** The replaced range, addressing the operation's base version. */
  range: TextRange;
  /**
   * The replacement text, verbatim (it may contain terminators).
   * Empty string deletes; an empty range inserts.
   */
  text: string;
}
```

Constraints — an operation violating any of them is malformed; a
malformed operation is never applied and never counted as a version
(§3.3):

- `replacements` MUST be sorted by `range.start`, ascending in
  (line, character) lexicographic order.
- Ranges MUST NOT overlap. Touching is permitted
  (`replacements[i].range.end == replacements[i+1].range.start`),
  except that two empty ranges at the same position MUST NOT appear
  in one operation.
- All positions address the text **at the operation's base version**
  and MUST be valid in it (§2.4). Applying an operation replaces all
  ranges simultaneously; earlier replacements do not shift the
  positions of later ones.
- An operation MAY be empty (`replacements: []`); it still installs
  its `id` as the version when applied.

## 3. Requests and editing

The extension defines exactly two requests: `openDocument` and
`storeDocument`. There is no fetch, close, or apply request — reads
travel as channel snapshots and edits as dispatched actions.

### 3.1 `openDocument`

Opens a document within a session.

```typescript
interface OpenDocumentParams {
  /** The session's channel URI. */
  channel: string;
  /** Resource to mirror. Mutually exclusive with `text`. */
  uri?: string;
  /** Initial content for a standalone document. Default "". */
  text?: string;
}

interface OpenDocumentResult {
  /** The document's channel URI (`ahp-document:/<seq>`). */
  document: string;
  /** The document's current version identifier (§2.2). */
  version: string;
}
```

- The session MUST exist; an unknown session channel answers
  *Invalid params* (`-32602`).
- Supplying both `uri` and `text` answers `-32602`.
- With `uri`: the server resolves the URI through its URI codec — an
  unservable URI answers `-32602` (`"unservable uri …"`) — and seeds
  the document from the resource's current content; a resource that
  cannot be read answers `-32602`. Opening the same `(session, uri)`
  pair again while the document lives is **idempotent**: the server
  answers the existing document's channel URI and its current
  version, not a second document.
- Without `uri`: the server creates a new standalone document seeded
  with `text` (empty by default). Every such call creates a distinct
  document.

Opening a mirrored document also makes the server the mirror's reload
authority (§6): it arms a content-comparing poll watch (400 ms
interval) on the mirrored file and announces the document to the
session's language services (`textDocument/didOpen`, see the LSP
extension).

Opening does not subscribe. To receive state and updates, the client
subscribes to the returned channel (§4).

### 3.2 `storeDocument`

Writes a document's current text to a resource. This is the client's
ONLY save path for a mirrored document: the client flushes its edit
queue (dispatches every pending operation, §3.3), then requests the
store; saving through the server keeps the write ordered behind the
document's committed edits.

```typescript
interface StoreDocumentParams {
  /** The DOCUMENT's channel URI (`ahp-document:/<seq>`). */
  channel: string;
  /** The resource to write. */
  uri: string;
}

interface StoreDocumentResult {
  /** The document version whose text was written. */
  version: string;
}
```

- An unservable `uri` answers `-32602`; an unknown document channel
  answers `-32001` (NO SUCH CHANNEL); a failed write answers an
  internal error.
- The server writes the document's current text to the resource
  (creating parent directories as needed), records the written text
  as the mirror's last reconciled disk content — so its own file
  watch does not read the write back as a foreign change (§6) — and
  recomputes any changeset channels the written path falls under.
- The answer carries the version whose text was written.

### 3.3 Submitting edits

Submitting an edit is not a request. A client submits an operation by
dispatching the channel's `document/applied` action (§4.2) on the
document's channel through the protocol's standard `dispatchAction`
**notification**. Extension actions ride `StateAction::Unknown`: the
action body travels with a `"type"` field beside it naming the
action.

A dispatched `document/applied` action carries `base`, `operation`,
and `id` (§4.2); the client MUST NOT set `origin`. `id` is minted by
the dispatcher and names the version the edit PRODUCES; `base` names
the version the edit was built on — the `id` of the action that
produced it (or the creation version, §2.2). When re-dispatching the
same logical edit transformed onto a newer base (§5), the client
SHOULD keep the same `id`, so echo recognition is stable.

Server behavior — the concurrency contract:

- The server processes dispatched operations for a given document in
  a total order.
- If the action's `base` equals the document's current version
  identifier and the operation applies cleanly (§2.5) to the current
  text, the server applies it — the text becomes the operation's
  result, the version becomes the action's `id` — and broadcasts the
  action to every subscriber of the channel, including the
  dispatching client, in the channel's ordered stream. The applied
  action also feeds the session's language services
  (`textDocument/didChange`, see the LSP extension).
- If `base` does not match, the server **silently discards** the
  dispatch. It MUST NOT transform, queue, or partially apply it. No
  response exists — `dispatchAction` is a notification, and there is
  no rejected-dispatch answer of any kind — and none is needed: the
  discard is observable on the stream, because the version the
  dispatch targeted is (or will be) superseded by an action whose
  `id` differs from the discarded dispatch's `base` lineage.
- A malformed operation (§2.5), or a dispatch against an unknown or
  disposed document channel, is likewise silently discarded. Every
  §2.5 constraint is checkable by the client against the base text it
  addressed; dispatching a malformed operation is a client defect.

The lineage check (`base` against the current version) is the entire
gate — there is no replay-dedup by `id`.

**Pipelining.** Because validation is by lineage, a client need not
wait for echoes: it MAY dispatch its entire pending queue at once,
each operation's `base` naming the previous one's `id`, the head
naming the last version it has folded. If every link holds, the
operations apply in order, each installing its `id` as the version.
If a foreign action wins the head's slot, the whole in-flight chain
is dead — every link names a version that never came to be, so the
server discards them all, and nothing misapplies.

**Outcome detection.** Exactly one action occupies each version, and
delivery is gapless and ordered (§4.2), so a client learns each
dispatch's outcome from the action that arrives with the `base` its
dispatch claimed: its own `id` means applied (and its chained
successors remain live); a foreign `id` means its chain from that
link on was discarded — the client folds the foreign action in,
transforms its pending operations across it, and re-dispatches the
chain from the new tip (§5). The loop needs no timeouts and no
negative acknowledgements.

## 4. The document channel

The document channel follows the protocol's standard contract:
subscribing answers a snapshot; subsequent updates arrive as actions
in order, applied by a pure reducer.

### 4.1 Subscribe and snapshot

Subscribing a document channel answers:

```json
{ "snapshot": { "resource": "ahp-document:/7",
                "state": { "text": "…", "version": "…", "uri": "…" },
                "fromSeq": 42 } }
```

with the state:

```typescript
interface DocumentState {
  /** The mirrored resource, if the document was opened with `uri`. */
  uri?: string;
  text: string;
  /**
   * The current version identifier (§2.2) — the chain anchor for the
   * subscriber's first dispatch (§3.3).
   */
  version: string;
}
```

Subscribing an unknown document channel answers `-32001` (NO SUCH
CHANNEL).

Document channels are **not** covered by the server's reconnect
replay set: a `reconnect` naming a document channel answers it in the
result's `missing` list. The client re-establishes the document —
`openDocument` (idempotent for a living mirror) and resubscribe —
then treats the fresh snapshot as a new stable and rebases its
pending tail onto it (§5).

### 4.2 Action: `document/applied`

The channel's single state action — both the shape a client
dispatches (§3.3) and the shape the server broadcasts. Reducer:
`text = apply(text, operation); version = id` (the server's
validation guarantees `base == version` for every broadcast action).

```typescript
interface DocumentAppliedAction {
  // "type": "document/applied"
  /**
   * The version the operation applies to (its positions' text) —
   * the lineage claim the server validates (§3.3).
   */
  base: string;
  operation: TextOperation;
  /**
   * The version this action PRODUCES. Minted by the dispatcher: by
   * the client for dispatched operations (§3.3), by the server for
   * server-originated ones. MUST be unique within the document;
   * UUIDs are RECOMMENDED.
   */
  id: string;
  /**
   * Reserved. The server currently emits `null` on every
   * server-originated action; clients MUST NOT set it and MUST
   * tolerate any value.
   */
  origin?: string;
}
```

Delivery rules:

- Actions are delivered to each subscriber in version order, gapless:
  a subscriber that received the snapshot at version *v* next
  receives the action with `base == v`, then the action whose `base`
  is that action's `id`, and so on. Equivalently, each delivered
  action's `base` equals the subscriber's current `version`.
- The dispatching client receives its own applied action like any
  other subscriber; `id` identifies it. Discarded dispatches (§3.3)
  produce no action.

### 4.3 Server-originated operations

The server MAY apply operations of its own at any time, at its
current version, through the same reducer — they broadcast as
`document/applied` with a server-minted `id`. This is the extension's
only mechanism for content change from above: if a mirrored
resource's content changes on disk, the server expresses the
difference as an operation ("a reload is just an operation"). There
is no snapshot-replacement action and no reload event.

### 4.4 Lifecycle

A document lives at least as long as it has subscribers. When the
last subscriber unsubscribes (or disconnects), the server disposes
the document: the channel state, the mirror association, and the
mirror's file watch are removed. Disposal emits no action (there is
nobody to hear it); a subsequent `openDocument` creates a new
document at a fresh channel URI. There is no close action on the
channel.

## 5. Client synchronization (non-normative)

The contract in §3.3 and §4 is the protocol's write-ahead pattern
applied to text. The reference client
(`frontend/hiahp/src/docsync.rs`):

- keeps **stable** — the last text confirmed by the channel, at its
  version identifier — plus a **pending** tail of its own operations
  not yet confirmed, and displays `stable` with `pending` applied
  (the write-ahead: its own edits show instantly);
- dispatches pending operations as they are made, without waiting —
  each `base` naming its predecessor's `id`, the first naming
  `stable.version` (§3.3's pipelining: the chain, not an ack, is
  what keeps this safe);
- on an echo (an action with an `id` it minted): folds it into
  `stable`, drops it from `pending`;
- on a foreign action: folds it into `stable`, **transforms** each
  pending operation across the foreign one (shifting each range by
  the foreign replacements' line and in-line character deltas;
  resolving genuine overlaps by a deterministic policy of its
  choosing), and re-derives its display text by replaying the
  transformed tail over the new stable. The foreign action also
  means every dispatch still in flight is dead (its chain names an
  ancestor that lost): re-dispatch the transformed tail as a fresh
  chain from the new `stable.version`, same `id`s.

There is no rejection to wait for: a discarded submit is superseded
by the foreign action that caused the discard, and that action's
arrival on the stream is what triggers the rebase and resubmit. The
loop converges: the server's total order admits exactly one winner
per version, so each re-dispatched chain either applies or trails
exactly one more foreign operation. All transformation is the
client's; the server never transforms (§3.3), which keeps server
semantics trivial.

## 6. Relationship to resources

A document opened with `uri` *mirrors* that resource: it is seeded
from the resource's content, and the server reflects later external
changes to the resource into the document as server-originated
operations (§4.3). Writing document content back to the resource is
the client's explicit act, through `storeDocument` (§3.2).

**The reload division of labor.** Agents edit FILES; clients edit
DOCUMENTS; the server is where the two meet. While a document mirrors
a resource, the server is the resource's ONLY reload authority: it
watches the file itself (§3.1), and a client that holds the document
channel MUST NOT independently reload the mirrored resource into the
document — one buffer fed from two reload pipelines applies the same
change twice or rewinds to stale content.

**Reload semantics.** The server keeps, per mirror, the disk content
it last reconciled (updated at open, at `storeDocument`, and at every
reload). On a disk change, when the document carries no unflushed
client operations relative to that content the reload is the plain
difference; when it does, the server three-way merges — base: the
last reconciled disk content; ours: the document; theirs: the fresh
disk — taking shared hunks once and keeping both sides' bytes on
genuine conflict. Either way the result broadcasts as ONE
server-originated `document/applied` chained off the document's
current version, exactly like a client's edit would (§4.3); racing
client dispatches lose by the ordinary §3.3 discard and re-chain. A
reload that changes nothing emits nothing.

Language services observe the document channel's text as the
mirrored resource's current content (see the LSP extension). Search
does not: `search` reads stored content only (see the Search
extension).

## 7. Errors

`openDocument` and `storeDocument` are requests and answer errors;
dispatches are notifications and cannot — their failure modes are
defined as silent discards (§3.3).

| condition | outcome |
|---|---|
| unknown session channel (`openDocument`) | JSON-RPC error `-32602` (Invalid params) |
| both `uri` and `text` in `openDocument` | JSON-RPC error `-32602` |
| unservable or unreadable `uri` (`openDocument`) | JSON-RPC error `-32602` |
| unservable `uri` (`storeDocument`) | JSON-RPC error `-32602` |
| unknown document channel (`storeDocument`) | JSON-RPC error `-32001` (no such channel) |
| write failure (`storeDocument`) | JSON-RPC internal error |
| subscribe on an unknown document channel | JSON-RPC error `-32001` (no such channel) |
| dispatched `base` not the document's current version (lost the race) | **not a fault** — silently discarded; outcome observed on the stream (§3.3) |
| malformed operation | silently discarded — a client defect; every constraint is client-checkable |
| dispatch against an unknown / disposed document | silently discarded |
