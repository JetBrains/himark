# AHP Extension: Commit History

This document specifies the Commit History extension to the Agent
Host Protocol (AHP): a **session-level channel** carrying a working
directory's commit log, where every commit **references the
changeset** that is its content.

The design rule carried over from changesets: **history answers
"what happened"; changesets answer "what changed".** The history
channel never carries diffs, files, or content — a commit points at
a changeset URI, and the existing changeset machinery
(`ChangesetState`, `changeset/*` actions, `resourceRead` content
refs) serves the rest. The extension adds no second way to describe
change.

Types are defined in `protocol/ahp-ext-types` (crate
`himark-ahp-ext-types`, `history` module); the reference server is
`backend/agent-host` (`src/history.rs` and the history surface of
`src/server.rs`).

The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be
interpreted as described in RFC 2119.

## 1. Availability

There is no capability advertisement. `initialize` negotiates only
the protocol version string (`"0.7.0"`); absence is the gate: a
server that does not serve history answers subscriptions of unknown
channels with `-32001`, and a client arms its history features on
the discovery entries below, which a server without the extension
simply never publishes.

## 2. Discovery: the changeset catalog

The session's EXISTING changeset catalog (`SessionState.changesets`)
carries the history entries: one per `file://` working directory,
beside that directory's `uncommitted` entry:

```json
{ "label": "History", "uriTemplate": "hihost-history:/<abs>",
  "description": "<abs>", "changeKind": "history" }
```

The template contains no variables — it is itself the subscribable
URI (the static shape the core spec already defines). The catalog is
re-derived whenever the working-directory set moves and rides the
standard `session/changesetsChanged` action; clients ignore unknown
`changeKind` values, exactly as the core spec instructs. Clients
never mint history URIs; the scheme is host-defined and opaque.

There is no `{commitId}` template variable and no
`changeKind: "commit"` catalog entry: a commit's content is
addressed by the already-expanded `Commit.changeset` URI (§4), so a
history client never touches template machinery.

## 3. The channel

Subscribing a history URI answers a `HistoryState` snapshot and a
stream of `history/*` actions:

```json
{ "snapshot": { "resource": "hihost-history:/…",
                "state": { "status": "computing", "head": {},
                            "commits": [] },
                "fromSeq": 42 } }
```

Subscribing a URI that is not a history URI answers `-32001` (NO
SUCH CHANNEL). The first subscribe lazily creates the channel entry
(initial state: `status: "computing"`, empty window), arms a
content-comparing poll watch (750 ms interval) on the repository's
reflog signal, and kicks an asynchronous recompute; the entry and
its watch persist for the server's lifetime once created. Every
recompute — the initial one, a watcher tick, a commit — lands as a
fresh `history/reset` (§3.1): rewrites are re-emits, never patches.
A directory that is not a git repository computes to
`status: "error"` with `error: "not a git repository"`.

History is unbounded, so the state is an explicit **window**: the
newest commits plus a cursor. The window starts at 100 commits; a
recompute preserves the depth the window has grown to.

```typescript
interface HistoryState {
  /** Computation lifecycle. */
  status: "computing" | "ready" | "error";
  /** Present iff status is "error". A plain message string. */
  error?: string;
  /** Where the repository stands. */
  head: {
    /** Current branch name; absent when detached. */
    branch?: string;
    /** Upstream ref name, when one is configured. */
    upstream?: string;
    /** Commits ahead of / behind the upstream. Absent without one. */
    ahead?: number;
    behind?: number;
  };
  /**
   * The window: newest-first, contiguous from HEAD. A client lays
   * out graph lanes from `parents` alone.
   */
  commits: Commit[];
  /**
   * Continuation cursor: the decimal spelling of the window's depth
   * (the skip offset for the next page). Present iff the last page
   * came back full; absent when the root is inside the window.
   */
  more?: string;
}

interface Commit {
  /** Full commit id (hash). Stable identity across re-emits. */
  id: string;
  /** Parent ids, first-parent first — the graph's edges. */
  parents: string[];
  /** The message's first line. */
  summary: string;
  /** The full message, when it is more than the summary. */
  message?: string;
  author: { name: string; email?: string; timestamp: number };
  /**
   * Decorations pointing here. `kind` is an open string; the
   * reference server emits "branch", "head", "remote", "tag".
   * Omitted when empty.
   */
  refs?: { name: string; kind: string }[];
  /**
   * True for commits not reachable from the upstream (unpushed).
   * Omitted when false.
   */
  outgoing?: boolean;
  /**
   * THE REFERENCE: a subscribable changeset URI whose
   * `ChangesetState` is this commit's content (§4).
   */
  changeset: string;
}
```

### 3.1 Server → client actions

Extension actions ride `StateAction::Unknown`: the body travels with
a `"type"` field beside it naming the action.

| Action              | Payload              | Meaning |
| ------------------- | -------------------- | ------- |
| `history/reset`     | `{ state }`          | The full fresh `HistoryState` — emitted for EVERY change: the initial compute, every watcher tick (a commit landed, a rebase, a checkout, a fetch), and after `history/commit`. Reducer: replace the state whole. |
| `history/appended`  | `{ commits, more? }` | Older commits at the BOTTOM of the window — the answer to `history/grow`. Reducer: extend `commits`, replace `more`. |

`history/prepended` (`{ commits, head }` — new commits on top of the
window) exists in the shared types and clients fold it, but the
server never emits it: growth at the top always arrives as a
`history/reset`. The shape is reserved.

### 3.2 Client → server dispatches

Dispatches travel as `dispatchAction` **notifications** on the
history channel; there is no response, and a dispatch the server
cannot honor is silently dropped.

| Dispatch         | Payload               | Meaning |
| ---------------- | --------------------- | ------- |
| `history/grow`   | `{ before, limit? }`  | Extend the window: `before` is the state's `more` cursor (the decimal skip offset), `limit` the page size (clamped to 1..=1000, default 100). Answered by `history/appended`. Stale-cursor guard: the server appends only if the window's length still equals the skip when the page is ready — any interleaving reset invalidates the page, and the client re-grows from the fresh `more`. |
| `history/commit` | `{ message }`         | Commit the working directory's changes, staged and unstaged alike, with `message`. An empty or whitespace-only message is ignored. The server recomputes: a fresh `history/reset` on this channel, and a recompute of the folder's `uncommitted` changeset, which empties through its own `changeset/*` actions. There is no amend. |

`history/commit` is deliberately on the history channel, not a
changeset operation: committing MOVES REFS — it is history's
mutation, and the uncommitted changeset merely observes the
consequence.

## 4. Commit changesets

`Commit.changeset` is a standard changeset channel URI of the form:

```
hihost-changes:/<abs-folder>?commit=<id>
```

— the same scheme as the folder's `uncommitted` changeset, with the
commit pinned by the query. It is served by the standard changeset
road: subscribe, render `files[].edit`, fetch sides via
`resourceRead`. The changeset diffs the commit against its first
parent; entries outside the subscribed folder are filtered, as in
the uncommitted computation.

A commit changeset's content refs address blobs (`hihost-git:/…`),
on BOTH sides, so `resourceRead` answers are immutable and cacheable
— unlike the uncommitted changeset, whose refs the client must treat
as live. A commit changeset is IMMUTABLE: same URI, same content,
forever — clients MAY cache and MUST expect no `changeset/*`
mutations after `ready`. The compute is asynchronous like every
changeset: subscribe, observe `computing` → `ready`, unsubscribe
when done.

Nothing else changes: `ChangesetState`, `changeset/*`, and
`resourceRead` are used exactly as the base protocol specifies.

## 5. Errors

History defines no requests; its error surface is the channel's.

| condition | outcome |
|---|---|
| subscribe on a non-history URI under the scheme | JSON-RPC error `-32001` (no such channel) |
| the folder is not a git repository | **not an error** — `status: "error"`, `error: "not a git repository"` in the state |
| `history/grow` with a stale cursor | silently dropped — a later `history/reset` already superseded the window |
| `history/commit` with a blank message | silently ignored |
