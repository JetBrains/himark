# File search: the indexed engine (FSP) and the naive fallback

Status: **LANDED** (2026-09-29). The wire truths this design rides
are `docs/ahp/ahp-search.md` and `docs/ahp/ahp-locations.md` —
neither changed shape; this was a host-internal engine swap. The
engine's own protocol is the File Search Protocol (FSP), specified
in the JetBrains-internal `file-search-protocol` repository
(`PROTOCOL.md` there; server design in its `docs/design.md`). The
himark half: `backend/agent-host/src/fsp.rs` (the client),
`vendor/fsp-types` (the vendored wire crate), the engine seams in
`backend/agent-host/src/server.rs`.

## 1. What is wrong today

Every search the host serves is a cold parallel walk. `hifind`
(`backend/find/src/lib.rs`) is one `ignore::WalkBuilder` fan-out per
request: `scan` answers the `search` method (quick-open path find,
content find), `scan_locations` streams the `searchLocations`
producer (`backend/agent-host/src/server.rs:2538`). It is honest and
simple, and it re-reads the working tree on **every keystroke**:

- Quick-open (`search` with `kind: "fuzzy"`, `target: "path"`) walks
  the whole tree to list file names — the one query class users type
  fastest and expect instant.
- Content search greps every file, every time. On a large tree the
  dock search streams for seconds; the FSP reference server answers
  the same queries from a trigram index in single-digit
  milliseconds.
- Nothing watches. There is no state between requests, so there is
  nothing to keep warm and no way to get faster.
- Content search reads **stored content only** — unflushed edits in
  document channels are invisible (ahp-locations.md §3.1 says so out
  loud), while `lsp/locations` results DO observe them. Two
  producers of the same channel disagree about text truth.

The host is the right place — search runs where the files live
(docs/ahp/agent-host.md §5) — but the engine underneath it is the
naive one.

## 2. The design in one paragraph

The agent host grows an **FSP client** and treats the FSP server as
a supervised sidecar, exactly the way it already treats language
servers (`backend/agent-host/src/lsp.rs`): spawn, LSP-style framing,
a pending map, document sync fed from the document channels it
already owns. The AHP wire does not change — `search` and
`searchLocations` keep their shapes, channels, and error tables; the
frontend is untouched. The engine is chosen **once, at host boot,
on binary presence**: with the server discovered, FSP is THE
engine — the host eagerly registers every session's working
directories as search folders (the indexing ask) and routes every
mappable request to the server, which answers not-yet-indexed
scopes by scanning on its own (§3); without the binary, `hifind`
is the engine, exactly as today. The fallback is permanent because
himark is public while the FSP server is JetBrains-internal: a
public build without the binary must work unchanged. The
types crate is not shared across the repo boundary either — a copy
of `fsp-types` (~740 lines: protocol structs, Content-Length
framing, jsonrpc, uri) is vendored into `vendor/fsp-types` and
synced by hand.

## 3. What FSP brings

FSP mirrors LSP: JSON-RPC with Content-Length framing, an
`initialize`/`shutdown` lifecycle, `textDocument/didOpen/didChange/
didClose` document sync, `$/progress` partial results,
`$/cancelRequest`. Two requests over a set of registered *search
folders*:

- `workspace/textSearch` — grep-like content search (literal or
  regex, case folding, includes/excludes, limits), streamed per
  file via partial results.
- `workspace/fileSearch` — fuzzy path search with scores and match
  ranges, the quick-open shape.

The reference server keeps a trigram index (candidate filter only —
a grep verify stage makes results exact), a RocksDB file-tree
snapshot with its own watcher, and gitignore handling equivalent to
the `ignore` crate's. Two properties matter for the mapping:

- **Overlays win over disk.** An open document synced via
  `didOpen`/`didChange` is searched from the client's content, not
  the file. This is the text-truth fix for §1's last bullet.
- **Search is valid while indexing — and without an index.** A
  server still building its index answers by scanning, and a query
  the index cannot filter (a short literal, a regex with no
  required literal) runs as a scan too; it never errors "not
  ready". So the host never gates requests on index state: it
  registers folders and asks, and warmth is the server's problem.

## 4. The boundary: nothing changes on the AHP wire

The engine swap is invisible to clients:

- `search` keeps `{hits, truncated}`, path-ordered (ahp-search.md
  §2.5 — the client ranks; FSP's scores are dropped for now, a
  wire extension is future work).
- `searchLocations` keeps the `ahp-locations:/` channel contract:
  per-file `locations/extend` actions, the done/truncated honesty
  latch, unsubscribe-is-cancel. FSP's per-file
  `TextSearchFileResult` batches map one-to-one onto the actions
  `emit_locations` (`server.rs:3636`) already fans out.
- Error tables hold: invalid regex is `-32602` whichever engine
  refuses it (FSP answers `InvalidParams` for a bad pattern; the
  naive path has `hifind::validate`).

One SEMANTIC delta, stated in the specs rather than hidden: with the
FSP engine, content search observes synchronized document content
(unflushed edits included), because the host feeds overlays (§6).
The naive fallback stays disk-only. ahp-search.md §2.3 and
ahp-locations.md §3.1 change from "stored content only" to "stored
content; a server MAY observe synchronized document content" — the
client already treats positions as advisory display snapshots, so
nothing on the consuming side cares which truth produced them.

## 5. The FSP client in the host

New module `backend/agent-host/src/fsp.rs`, modeled on `lsp.rs`
(outbox + pending map + ready future + reader thread), but speaking
FSP via the vendored `fsp-types` (its `framing`/`jsonrpc` modules
replace the hand-rolled `frame`/`read_message` of `lsp.rs`).

**Process model: an stdio child, persistent data root.** The host
spawns `fsp-server` in stdio mode — one FSP session, dying with the
host — with `FSP_DATA_DIR=~/.himark/agent-host/fsp` so shards
persist across restarts (a respawn hot-starts from the RocksDB
store; no cold re-index). The host is already a lockfile singleton
(docs/ahp/agent-host.md §2), so no two instances contend for the
store. FSP's daemon mode (UDS, multi-session, shared shards) is a
deliberate later switch, worth taking only when a second client
exists; stdio pins the binary per spawn and removes socket
discovery, daemon liveness, and client/daemon version skew from the
failure table.

**Discovery**: `$HIMARK_FSP_BIN`, then a sibling of the daemon
binary (the `resolve_daemon_binary` recipe — internal bundles ship
the server beside the host), then `fsp-server` on `PATH`, then the
sibling repository's build output — a
`file-search-protocol/target/{release,debug}/fsp-server` beside ANY
ancestor of the running binary, so `<repo>/target/<profile>/` runs
and the Xcode bundle under `apps/himark-apple/build/` resolve the
checkout alike. RELEASE wins whenever it exists: the sidecar is
latency-critical — a debug fileSearch over a monorepo takes
seconds where release takes ~0.4s draining and milliseconds warm,
and quick-open cancels per keystroke, so a debug server reads as
"no results" (2026-09-30, ultimate) — and `cargo test` in the
checkout must not shadow it with a fresh debug build; an fsp
developer iterating on the server pins `$HIMARK_FSP_BIN`. A dev
gets the server picked up with no env var, the feedback loop the
`HIMARK_RUST_ANALYZER`-style overrides exist for but without the
setup. No binary anywhere → the engine is simply absent, logged
once, and the naive engine serves the host's lifetime.
`HIMARK_FSP=0` forces it absent.

**One FSP session for the whole host; OPEN sessions' GIT folders
register.** Search folders are the working directories that are
(inside) git repositories, of sessions with a live subscriber on
their session channel — reconciled at session AND subscription
lifecycle (`createSession`/`disposeSession`/
`session/workingDirectorySet/Removed`,
subscribe/unsubscribe/connection close →
`workspace/didChangeSearchFolders`). Two deliberate narrowings:

- NOT the whole catalog: a host boots with every manifest it ever
  stored, and registering that union would be an ask to index and
  WATCH a user's entire session history (the 2026-09-30 lesson: a
  months-old session naming `~/Downloads` had macOS prompting for
  it at boot).
- ONLY git repositories (`fsp::indexable` — a `.git` in the folder
  or an ancestor): what fits git fits the index — bounded,
  ignore-ruled, worth watching. Anything else is either explodable
  (a home directory, `~/Downloads`) or too small to index, and the
  server scan-serves it per request anyway.

Opening a session is the indexing ask: the server starts watching
and indexing its repositories at subscribe, so the first search is
already warm; closing it releases them. Registration is ONLY the warmth
ask — it never gates scope: FSP serves every request, and a
`dirs` entry outside all registered folders is served by the
server as an ad-hoc scan scope with default folder rules, no
registration, watcher or index implied (PROTOCOL §6, an upstream
change made for this). The local-fs session (`hihost-fs:/local`)
therefore rides FSP like everything else; its no-folder default
(`/`) becomes a scan of `/` — exactly the walk `hifind` performs
for it today, behind the same cancellation. Folders register with
defaults (`respectIgnoreFiles: true`, no excludes) — the same
rules the `hifind` walk applies.

**State discipline.** The engine follows the host's own rule: ONE
persistent value (`EngineState` — rpds folder set, overlay
versions, the connection behind an `Arc`, the demotion counter)
behind a swap latch; readers snapshot, writers derive the next
value; spawns happen OUTSIDE the latch, installed first-wins. The
connection's plumbing is channels and oneshots — a writer thread
owns the child's stdin (buffering until the handshake `Ready`s
it), and the reader resolves requests through the one
live-mutable end, an in-flight table on the `lsp_inflight`
precedent.

**Overlays ride the existing hooks.** `lsp_feed_open`/
`lsp_feed_change` get FSP siblings (`fsp_feed_open/change`):
document channel opens/ops mirror into `textDocument/
didOpen`/`didChange` (replacements in reverse order, the `lsp.rs`
recipe); `disposeSession` closes its mirrors' overlays. The host
initializes with `positionEncodings: ["utf-8"]` — AHP's pinned
unit (ahp-lsp.md §4) — and the reference server grew the
negotiation as part of this work (`Encoding` in its `textutil.rs`;
utf-16 stays its default). Against a server that lands on `utf-16`
anyway, the host downgrades overlay sync to whole-text changes and
converts result columns using the match line's text, which FSP
ships in every match — conversion is always possible, never
guessed.

**Cancellation.** The locations leash (`leash_locations`,
`server.rs:3618`) flips on disposal exactly as today; the FSP
producer maps the flip to `$/cancelRequest` for its in-flight
request. `search` keeps its per-connection supersession token
(`state.searches`); superseding cancels the FSP request the same
way. FSP keeps streamed partial results valid after a cancel, which
is precisely the channel contract (partials stay, `truncated`
latches).

## 6. Request mapping

| AHP request | FSP request | notes |
|---|---|---|
| `searchLocations` (text/regex content) | `workspace/textSearch`, `partialResultToken` set | streamed `TextSearchFileResult[]` batches → one `locations/extend` per file; `range.start` → line/column, match length from the range against the shipped line text; `hifind::context_window` (`backend/find/src/lib.rs:324`) reslices the matched line so windows look identical across engines; `maxResults` = the limit; `limitHit` → `truncated` |
| `search`, `target: "path"`, `kind: "fuzzy"` | `workspace/fileSearch` | quick-open; the server's score order IS the wire and presentation order (ahp-search.md §2.5 — ranking lives in fsp-server: name-position ladder, 2026-09-30); score values stay off the wire |
| `search`, `target: "content"`, `kind: "text"/"regex"` | `workspace/textSearch` | `maxResultsPerFile: 1`, hits are the matched files, path-ordered |

Scope never routes: FSP serves every folder set, registered or
not (§5). What remains on the in-process walk under either engine
are the classes with no FSP vocabulary — fuzzy content (per-line
subsequence matching no index can filter) and regex/substring
path matching (fileSearch's query interpretation is a server
capability, not a per-request choice) — all consumerless today.
If one ever matters, the answer is FSP vocabulary, not host-side
dispatch. The mapping is **static**: the table and the boot-time
engine presence; never server health, index state, or scope per
request.

## 7. The failure ladder

The engine decision is made once, at boot; failures are handled by
keeping the chosen engine alive, not by re-deciding per request:

- **No binary / `HIMARK_FSP=0`**: the naive engine, for the host's
  lifetime; one log line.
- **Crash mid-flight**: respawn with backoff, then REPLAY — folders
  re-register from the refcount table, open documents re-`didOpen`
  from the document channel state the host owns; both are replays
  of state the host already holds, nothing is asked of clients.
  In-flight work at the moment of death: a `searchLocations`
  producer that already streamed batches resolves its channel
  `{done: true, truncated: true}` (the honesty latch — the user's
  next keystroke re-asks against the respawned server); one that
  emitted nothing, and any in-flight `search`, simply re-runs after
  the respawn — searches are cheap to restart and idempotent.
- **The binary is present but will not serve** (spawn or handshake
  fails repeatedly — a broken install): after N respawn attempts
  the host demotes to the naive engine for its lifetime and logs
  loudly. A one-time demotion at the same seam as the boot
  decision — still never a per-request choice.

## 8. The vendored types

`vendor/fsp-types` — a manual copy of the internal repo's
`crates/fsp-types` (protocol structs, framing, jsonrpc, uri; serde +
url deps only), workspace member, used by `agent-host` alone. The
provenance header lives in its `lib.rs`; syncing is copying the
crate's `src/` over and re-reading the diff. (The crate was made
client-usable UPSTREAM as part of this work: every type derives
both `Serialize` and `Deserialize`, and `jsonrpc` gained
`RequestMessage` — the copy stays verbatim.) Copying beats
reimplementing (the structs encode spec subtleties —
optional-field defaults, the progress envelope) and
beats a git dependency himark's public builds could not fetch. The
protocol is versioned (`0.1.0`) and all requests flow client →
server, so skew between the vendored types and a newer server
degrades to `MethodNotFound`/`InvalidParams` — which the failure
ladder (§7) already turns into the naive path.

## 9. Testing (as landed)

- **Hermetic** (`tests/host.rs`, the `fsp_*` tests, over
  `testing::FAKE_FSP` — a scripted python server speaking real
  Content-Length framing, logging every message it hears):
  streaming `$/progress` batches → `locations/extend` with exact
  positions/context; both `search` lanes, including the
  score-order → path-order re-sort; folder registration observed
  on the wire; didOpen REPLAY at first spawn (document opened
  before the engine ever ran) and live didChange after;
  kill-mid-request (`die-now`) → the walk serves the crash window
  and the respawned server serves the next request (two
  `initialize`s in the log); unsubscribe → `$/cancelRequest`
  reaches the server (`slow` parks until it does). Engine unit
  tests in `fsp.rs` cover discovery, column conversion, and result
  mapping.
- **Real binary** (`fsp_real_server_parity`, gated on
  `HIMARK_FSP_E2E=<path>`): a seeded tree with a multibyte line,
  positions asserted EQUAL to a `hifind::scan_locations` oracle —
  the drift guard; an unflushed overlay edit found at its live
  utf-8 position; fuzzy path search. CI has no internal binary and
  skips.

Verification: `cargo test -p agent-host -p fsp-types -p hifind`
plus `apps/web/tools/build-web.sh --check` (the house gate; the
bare cargo pipe false-greens on the web target).

## 10. What landed where

1. **Upstream** (`file-search-protocol`): symmetric serde on
   `fsp-types` + `RequestMessage`; position-encoding negotiation
   (`textutil::Encoding`) through overlays, textSearch and
   fileSearch — utf-16 stays the default, utf-8 negotiable;
   PROTOCOL §6 `dirs` outside search folders served as ad-hoc scan
   scopes (`resolve_scopes` synthesizes a default `FolderCfg`).
2. **`vendor/fsp-types`**: the verbatim copy, workspace member.
3. **`backend/agent-host/src/fsp.rs`**: discovery ladder; the
   engine as one persistent `EngineState` under a swap latch
   (folder set, overlay versions, connection, demotion counter);
   the connection as a writer thread over the child's stdin (frames
   buffered until the handshake), a reader thread resolving oneshot
   answers through the in-flight table, overlay replay queued at
   spawn; request mapping onto `hifind::LineMatch`.
4. **`backend/agent-host/src/server.rs`**: `fsp` beside the `lsp`
   pool; `fsp_sync_folders` at boot/create/dispose/directory-set;
   `fsp_feed_*` beside the `lsp_feed_*` hooks; the engine branches
   in `search` (fileSearch / per-file-capped textSearch +
   `path_ordered`) and `searchLocations` (`fsp_search_locations`)
   with the `Gone` crash-window fallback; the shared
   `locations_of` conversion.
5. **Docs**: the overlay sentences in ahp-search.md §2.3 /
   ahp-locations.md §3.1; agent-host.md §5's search bullet; this
   record.

## 11. Decisions taken (and their why)

- **Engine behind the host, not a new wire** — the AHP surface and
  the whole frontend stay untouched; the fallback and the engine
  are one seam apart, so public builds lose nothing.
- **One engine per host lifetime, decided at boot** — dispatch by
  binary presence, never per request: the server scan-serves
  anything unindexed, so index state is not the host's business,
  and a static engine keeps semantics (overlays, ignore handling)
  consistent within a run instead of flapping under load.
- **Keep `hifind` forever, not transitionally** — himark is public,
  the server is internal; a build without the binary must be whole,
  and the `search` classes FSP has no vocabulary for need the
  in-process walk regardless of packaging.
- **Scope routes nothing; the server walks what it doesn't index**
  — rather than host-side folder-coverage rules or on-demand
  registration plumbing, FSP itself serves out-of-folder `dirs` by
  scanning (the upstream §6 change). One engine, every scope; the
  walk fallback exists for missing binaries and missing
  vocabulary, never for scope.
- **Indexing is opt-in by evidence, not by attachment** — open
  sessions only, git repositories only. Registration costs a
  watcher, an index, and (on macOS) possibly a permission prompt;
  a folder earns that by being a repo someone is working in right
  now. Search over everything else still works — it scans.
- **stdio child over daemon mode, for now** — one client exists;
  pinning the binary per spawn deletes discovery, liveness, and
  version-skew failure modes. `FSP_DATA_DIR` keeps the index warm
  across restarts, which is the property that matters. Daemon mode
  is a config change away when a second client appears.
- **Copy `fsp-types`, don't rewrite or depend** — the repo boundary
  is a publishing boundary, not a design one; a hand-synced copy of
  740 lines is the cheapest correct bridge.
- **Overlays sync into the engine** — it heals the text-truth split
  between `searchLocations` and `lsp/locations`, and the host
  already owns every piece: the document channels, the feed hooks,
  the reverse-order change recipe.
- **utf-8 negotiated, conversion as the net** — AHP pins utf-8 byte
  columns; FSP negotiates encodings per LSP 3.17; the shipped match
  line makes conversion exact when negotiation lands elsewhere.
