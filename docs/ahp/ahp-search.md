# AHP Extension: Search

This document specifies the Search extension to the Agent Host
Protocol (AHP). It lets a client search the contents and names of a
session's resources server-side, where the resources live.

The result is deliberately minimal: a search answers **which
resources matched**, never where inside them. Match positions
against live content are the client's to derive from content it
fetches — positions carried across the wire would be stale by the
time they were used. This keeps the result shape stable, small, and
immune to concurrent edits.

Types are defined in `protocol/ahp-ext-types` (crate
`himark-ahp-ext-types`); the reference server is
`backend/agent-host` over the search engine `backend/find`
(`hifind`); the reference client surface is
`frontend/hiahp/src/find.rs`.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be
interpreted as described in RFC 2119.

## 1. Availability

There is no capability advertisement. `initialize` negotiates only
the protocol version string (`"0.7.0"`); absence is the gate: a
server that does not serve `search` answers the standard JSON-RPC
*Method not found* error, and a client arms its search features on
the resource authority it connects to, not on an advertised flag.

## 2. Method

### 2.1 `search`

```typescript
interface SearchParams {
  /** The session's channel URI. */
  channel: string;
  /**
   * Folder resource URIs to search, each a `file:` URI within the
   * session's working directories. Default: all of the session's
   * working directories.
   */
  folders?: string[];
  /** The search term. */
  query: string;
  /**
   * How `query` is interpreted:
   *  - "text"  — literal substring (default)
   *  - "regex" — regular expression
   *  - "fuzzy" — subsequence match: the query's characters must
   *              occur in the target in order, not necessarily
   *              adjacent (quick-open matching)
   */
  kind?: "text" | "regex" | "fuzzy";
  /** Default false. */
  caseSensitive?: boolean;
  /**
   * What is searched:
   *  - "content" — the stored text content of resources (default)
   *  - "path"    — resource names and paths; `query` matches
   *                against the path relative to the searched
   *                folder, the name included
   */
  target?: "content" | "path";
  /**
   * Maximum number of hits to answer. Default 128; the server caps
   * the effective limit at 1024.
   */
  limit?: number;
}

interface SearchResult {
  /**
   * Resource URIs of matching resources, deduplicated — one entry
   * per resource regardless of how many times the query matches
   * inside it. Ordered by (searched folder, relative path).
   */
  hits: string[];
  /** True iff the limit or a cancellation cut the result off. */
  truncated: boolean;
}
```

### 2.2 Scope

The searchable roots are the session's `workingDirectories`; for the
server's local-filesystem session the root is `/`. Every entry of
`folders` MUST be a `file:` URI — anything else answers *Invalid
params* (`-32602`, `"folders must be file uris"`) — and MUST lie
under a root (`-32602`, `"folder outside the session"`). An unknown
session channel answers `-32001` (NO SUCH CHANNEL).

### 2.3 Matching

- `kind: "regex"` uses the Rust regex dialect; a syntactically
  invalid expression answers `-32602`.
- `caseSensitive: false` (the default) folds case on both sides for
  every kind.
- `target: "path"` matches over the path relative to the searched
  folder (file name included); no content is read.
- `target: "content"` reads **stored resource content only**. There
  is no document-channel overlay: unflushed edits held in a mirrored
  document (see the Documents extension) are not observed — a client
  that wants live-buffer hits searches its own buffers.

### 2.4 The walk

The server walks each searched folder in parallel:

- version-control ignore rules are respected;
- symbolic links are not followed; only regular files are
  considered;
- files with a known binary extension (images, archives, media,
  object code, databases, fonts, …) are skipped for every target;
- for `kind: "fuzzy"` with `target: "content"`, only files of at
  most 2 MiB are read (fuzzy content matching runs per line);
- content that cannot be decoded as text does not match.

An empty `query`, or `limit: 0`, answers
`{ hits: [], truncated: false }` without walking.

### 2.5 Ordering

Hits are sorted by (index of the searched folder, relative path) —
**path order**. Ranking by match quality is not performed; a client
that wants relevance ordering (e.g. name-over-directory or
subsequence density for a quick-open surface) ranks the returned
paths itself.

### 2.6 Cancellation and truncation

The server holds one cancellation token per **connection**: a new
`search` request on a connection cancels that connection's previous
search, and closing the connection cancels its in-flight search. A
cancelled search still answers, with the hits collected so far and
`truncated: true`.

`truncated: true` is the honesty contract — it is set both when the
limit cut the walk short and when cancellation did; a client MUST be
able to tell a complete answer from a cut-off one.

## 3. Errors

| condition | answer |
|---|---|
| unknown session channel | JSON-RPC error `-32001` (no such channel) |
| a `folders` entry that is not a `file:` URI | JSON-RPC error `-32602` (Invalid params) |
| a folder outside the session's roots | JSON-RPC error `-32602` |
| invalid regular expression | JSON-RPC error `-32602` |
| no matches | **not an error** — `{ hits: [], truncated: false }` |

## 4. Client notes (non-normative)

The reference client maps its find surfaces onto the method as:

- path find → `kind: "fuzzy"`, `target: "path"`, overall cap 128;
- text find → `kind: "text"`, `target: "content"`, overall cap 64;

issuing one request per session working directory with a limit that
shrinks by the hits already collected. A client superseding a search
(the user kept typing) simply issues the next request — the server's
per-connection token cancels the old one — and discards the earlier
answer on arrival.
