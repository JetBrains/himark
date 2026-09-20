# AHP extensions: documents, search, LSP, history

> **Wire truth lives elsewhere.** The normative protocol
> specifications are **docs/ahp/ahp-documents.md**, **docs/ahp/ahp-search.md**,
> **docs/ahp/ahp-lsp.md**, **docs/ahp/ahp-history.md**,
> **docs/ahp/ahp-locations.md** — self-contained,
> spec-style, and authoritative. This document is the himark-side
> design rationale: which seams route where, and why.

A session workspace is served whole by the agent host
(docs/ahp/agent-host.md): fs-over-AHP (docs/ahp/agents-fs.md) routes
list/read/store/watch through the seat, and the capabilities beyond
files — documents, search, language intelligence, vcs change, history
— ride the same socket as protocol extensions. The services run IN
the host, next to the working tree the agent mutates. We own both
ends of the wire, so extending it is a method away.

The foundation is text: **you cannot get LSP right without syncing
documents correctly**, and once documents sync correctly, LSP needs
no vocabulary of its own at all. So documents are a protocol surface
— shared text buffers, synced by operation — and LSP is a thin
multiplexed pass-through above them. The sync model is the
distributed-transaction shape (an ordering authority + a speculating
client + replay-in-final-order) reduced to the one state we share:
text.

The load-bearing decisions, up front:

1. **The effects stay THE seam; handlers fork on authority.** No new
   client-side effect types: `FindEffect`, the base-resolution chain,
   the LSP asks each route by the location's AUTHORITY — the fs
   precedent, verbatim. Nothing downstream can tell; the door is the
   authority, not the caller.
2. **Absence is the gate.** There is no capability negotiation: the
   initialize handshake checks the protocol version string and
   nothing else. A seat that does not serve an extension answers the
   error its default method answers; an unknown method answers
   JSON-RPC method-not-found; the client's extension arms are gated
   by resource authority (an `ahp`-scoped location implies a seat
   that serves the surface). Both ends live in one workspace and move
   in one commit — the version that matters is the protocol
   version, pinned and intersected at initialize.
3. **A document is a CHANNEL.** State = `{text, version}`; the fold
   has ONE action kind — an operation, a set of replaces — and
   folding it is applying it. Every client of a session shares its
   buffers through these channels: himark editors, the host's
   language servers, a second frontend, the reconciled edits of the
   agent itself. This is the single source of text truth the rest of
   the design stands on.
4. **The host ORDERS; the client TRANSFORMS.** The host is a dumb
   serializer: an operation whose `base` names the channel's current
   version applies and broadcasts; any other lineage is discarded —
   the host never transforms anything. The client applies its own
   edits speculatively (typing never waits on the wire), holds the
   host-confirmed STABLE text plus its unconfirmed tail, and on a
   foreign operation time-travels: re-point at the stable snapshot
   (persistent ropes make this O(1)), fold in the foreign operation,
   REPLAY its own tail transformed over it — git rebase, per
   keystroke. Convergence is the host's total order; the transform
   lives in exactly one place, the client.
5. **LSP is the LSP protocol, minus what the host now owns.** With
   documents synced, the host is the ONE LSP client per language
   server and owns everything lifecycle- and text-shaped: spawn,
   `initialize`, capabilities, `didOpen`/`didChange` (fed straight
   from document channels), watched-files, shutdown. What crosses the
   wire is the REST of LSP, verbatim: requests/responses pass through
   multiplexed, cancellation rides a notification mapped to
   `$/cancelRequest`, diagnostics fan out on a channel. Raw-LSP
   tunneling of a whole connection (id collisions, capability
   intersection, didChange ownership) is exactly what this avoids —
   the host terminates the protocol's stateful half and forwards its
   stateless half.
6. **VCS needs no extension.** The core protocol already speaks
   change the direct way: a session carries CHANGESETS — the
   aggregated diff of the session's work, with per-file before/after
   refs readable via `resourceRead`. himark's change-facing surfaces
   consume that core state; the himark-added parts are only the
   channel URI schemes and the git computation behind them. History
   — the revision log — is the one vcs surface that earned its own
   extension (docs/ahp/ahp-history.md), and a commit's files ride the
   SAME changeset shape, addressed by revision.
7. **Versions are the correctness story; connection order is only
   thrift.** Versioned operations subsume FIFO: a stale submit is
   discarded, a gap is impossible by construction, a reconnect is a
   resubscribe (the snapshot carries `{text, version}`) plus a
   rebase of the unconfirmed tail. Ordering still keeps the common
   case one-round-trip cheap; it just is not load-bearing.

## Extension mechanics

- **Methods are flat names in the host's dispatch table**, beside the
  base protocol's: `openDocument`, `storeDocument`, `search`,
  `searchLocations`, `httpServe`, and the `lsp/` prefix family
  (`lsp/<method>` envelopes plus the reserved `lsp/capabilities`,
  `lsp/diagnostics`, `lsp/locations`, and the `lsp/$/cancelRequest`
  notification). An unknown method answers method-not-found.
- **Actions ride the base protocol's untagged escape hatch**
  (`StateAction::Unknown`) with a `type` string beside the body:
  `document/applied`, `history/reset`, `history/appended`,
  `history/grow`, `history/commit`, `lspDiagnostics/published`,
  `locations/extend`.
  Reducing them is the channel owner's business on each end.
- **Channels are host-minted URI schemes**, routed by prefix at
  subscribe and dispatch: `ahp-document:/…`, `hihost-history:/…`,
  `ahp-lsp-diagnostics:/…`, `ahp-locations:/…` (per-request result
  streams; last unsubscribe cancels the producer,
  docs/ahp/ahp-locations.md), `hihost-changes:/<folder>` (and
  `?commit=<sha>`), plus the content-ref schemes `hihost-git:/…`
  (git blobs) and `ahp-content:/…` (stashed in-memory content), both
  served by `resourceRead`.
- **Session scope**: every extension request names its session (or a
  channel the session owns), and the session's working directories
  bound what the host serves. Host-minted uris (document channels,
  content refs) are the one principled exception: they are the
  host's own tokens, served wherever the host minted them.
- **The types are a shared crate** (`protocol/ahp-ext-types`) —
  serde shapes only; client and host share the PROTOCOL, never each
  other's code. The search scan (`backend/find`) and the git backend
  (`backend/git`) are library cuts both sides could link; today the
  host links them and the client consumes the wire.

## Documents

One channel per open resource, host-minted and idempotent per
`(session, uri)`: `openDocument` answers `{channel, version}`,
`subscribe` answers `{text, version, uri}`, edits travel as
`document/applied {base, operation, id}` dispatches, and
`storeDocument` writes the channel text to disk host-side. The full
contract — the lineage gate, the client's stable/tail/in-flight
machine, the disk reconciler — is docs/ahp/ahp-documents.md.

What the shape buys:

- **disk writes reconcile as operations.** The host watches a live
  channel's resource; a foreign change diffs against channel state
  (three-way when the buffer has unflushed edits) and applies through
  the host's own door as one more operation — never rejected, the
  host authors at its own current version. An agent's tool edit
  therefore MERGES into the user's open buffer live, and the user's
  unsaved edits rebase over it — the conflict dialog dissolves into
  an ordinary rebase tick.
- **save is a channel affair**: `storeDocument` writes host-side; the
  host knows its own write, so the watch echo is suppressed by
  construction — the save-echo rule, airtight instead of heuristic.
- **channels are the agent's window too**: anything that wants the
  buffer the user actually sees (unsaved edits included) reads the
  channel, not the disk — the language servers already do.

Client-side, a session document's `Document` is **channel-backed**
(`hiahp::docsync`): the edit door forwards every committed operation
to the sync client, and foreign operations enter as programmatic
edits — markup, layouts, carets and repairs shift exactly as for any
other edit. Undo stays LOCAL and survives multi-client by shape:
himark's undo is inverse-as-a-fresh-edit, and a fresh edit is just
the next operation to sync. The `EditLog` and document revisions stay
what they are (local mechanics); the channel `version` is the sync
layer's own lineage and never leaks into engine semantics.

The authority line this draws, deliberately: for SESSION documents
the host owns the ordered operation log and the client holds a
speculating replica; local files keep local authority untouched. Two
himark frontends on one session converge by construction —
collaboration falls out of the shape.

## VCS: no extension, deliberately

A session carries **changesets** — per-file before/after content refs
served by `resourceRead` — advertised through the standard catalog
(`SessionState.changesets`: one `uncommitted` entry per working
directory, one `history` entry beside it). That is precisely the
"what changed here" a session workspace's surfaces need:

- **Stripes**: the base-resolution chain's `ahp` arm resolves a
  document's base to its changeset before-ref (a file untouched by
  the session has no entry and no stripes — honest: the session's
  change IS the interesting diff). The answered location enters the
  standing base chain (fetch → diff → sweep) unchanged.
- **The changes drawer** renders the session's aggregate diff from
  the changeset channel — core protocol state, no extension methods.
- **History** (commit log, per-revision file lists) is its own
  extension because it is genuinely new vocabulary — a channel of
  commits whose `changeset` fields point back at commit-addressed
  changeset channels (docs/ahp/ahp-history.md).

## Search

The find seam's answer shape is the design's luck: it returns BARE
LOCATIONS, and every consumer — the location lists, the peeker
preview — re-derives spans against the text it actually fetched. So
the wire carries no offsets to go stale: `search` takes the session
channel, optional folders, the query (`text`/`regex`/`fuzzy` over
`content`/`path`) and a limit; it answers `{hits, truncated}`. The
host walks the session's directories with the ripgrep stack on a
per-connection leash — a new search supersedes the running one, a
dead connection cancels it. `target: "path"` with `kind: "fuzzy"` is
the quick-open form: the peeker asks the seat instead of crawling the
tree. Spec: docs/ahp/ahp-search.md.

## LSP

`lsp/<method>` envelopes (`{channel, params}` — params and results
are LSP shapes VERBATIM, utf-8 positions) forward to a host-owned
language server chosen by file extension and workspace root.
Lifecycle and text-sync methods are refused at the door — they are
the host's own; text truth comes from the document channels
(`didOpen` from channel state, every folded operation forwarded as an
incremental `didChange`). `lsp/capabilities` answers the server's
capabilities; `lsp/diagnostics` names a per-session diagnostics
channel that streams per-document replacements;
`lsp/$/cancelRequest` maps onto the real server's cancel. One server
per resolved root, supervised by the host — shared between sessions
over the same directory, warm across editor restarts. Spec:
docs/ahp/ahp-lsp.md.

Client-side the LSP routes are thin: the effect handlers serialize
the ask through the seat and convert locations at the client edge.
The completion, hover, definition and references surfaces ride it
today; the diagnostics channel ships host-side so the rendering arc
finds the pipe standing.

## Testing

- **doc convergence** (hermetic): interleaved operation chains
  through one host — every schedule converges to identical text; a
  discarded chain rebases and re-lands whole; a disk-reconcile
  operation rebases a client's pending tail.
- **host service tests** (socketpair, the standing harness): search
  against a seeded tree; lsp against a scripted fake server —
  pass-through round-trips, lifecycle methods refused, cancel
  reaches the server, didChange versions track channel operations;
  history/grow paging and commit against a real temp repo.
- **engine e2e** (at the effect seams): a session document types
  locally while the host injects a concurrent operation — the editor
  converges with markup and caret sane; stripes appear on a session
  document from its changeset before-ref; the peeker's name lookup
  answers seat-side hits.
