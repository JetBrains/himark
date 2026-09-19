# The himark Agent Host

himark owns its agent host: a standalone process that holds the
authoritative agent state, speaks **AHP** to himark over a **unix
domain socket**, and terminates the AGENT protocols on the other
side. The flagship agents ride their NATIVE wires: **Claude over the
Claude Code CLI's stream-json protocol** (the Agent SDK wire itself)
and **Codex over its app-server protocol**.

Why native wires instead of an adapter: an adapter hides the session
identity and the CLI-shaped surface behind a smaller vocabulary.
Talking to the CLI directly buys full **CLI interop** — himark
sessions are `claude --resume`-able from a terminal and terminal
sessions appear in himark's drawer (the session catalog lists
`~/.claude/projects`); thinking levels, permission modes, hooks,
skills, CLAUDE.md and MCP config all ride the CLI's own loading; and
no third-party release schedule sits between us and the agent.

Why an owned host at all: a host we do not own auto-updates
underneath us — protocol bumps and auth redesigns land on someone
else's schedule. A host we own moves when WE move it, and sessions
survive editor restarts because agents are transient subprocesses of
a long-lived daemon, not of the editor.

Credentials: the host spawns the `claude` CLI locally with the
user's own environment — it resolves credentials exactly the way
terminal Claude Code does (the keychain / `~/.claude` login, i.e.
the user's claude.ai subscription). No credential sniffing, no
gating, and the host never injects `ANTHROPIC_*` /
`CLAUDE_CODE_OAUTH_TOKEN` overrides into the agent's environment.

```
┌────────── himark ──────────┐      AHP over UDS       ┌───── himark-agent-host ─────┐   native wires, spawned per chat
│ higent (drawer, ChatPanel) │ ◄──────────────────────► │ AHP server: channels, seqs, │ ◄──► claude CLI (stream-json + control)
│ hiahp::wire (AHP client)   │   ndjson JSON-RPC        │ reducers, persistence, fs,  │ ◄──► codex app-server
│ fs-over-AHP (fsroute)      │   lockfile discovery     │ vcs, search, lsp, pty, docs │
└────────────────────────────┘                          └─────────────────────────────┘
```

## 1. The shape

- **himark** is an AHP CLIENT. The wire seat (`hiahp::wire::WireHost`)
  discovers per seat and shows one drawer row PER HOST, each
  connected or offline on its own terms (§6).
- **himark-agent-host** is a standalone binary (`backend/agent-host`,
  package `agent-host`). It is the AHP SERVER: it owns sessions,
  chats, turns, the action streams, the AHP-shaped transcript
  history — and the session's WORKSPACE SERVICES: files, watches,
  vcs changesets, history, search, LSP, PTYs, collaborative
  documents (§5). Toward agents it drives the native provider wires.
- The house rule applies to the host itself: the crate is a LIBRARY
  with a thin `main` (`run_with` parses `--socket`/`--http`/
  `--web-root`) — engine tests run the whole host in-process over a
  socketpair; the daemon is the same library behind a real socket.

The host is not "the agent host" so much as **himark's
workspace-services process**: it stands where the services must run
— next to the working tree the agent mutates — and every service is
a method or channel family on the same socket.

## 2. Transport, discovery, lifecycle

- **Socket**: `~/.himark/agent-host/host.sock` (`$HIMARK_HOST_HOME`
  overrides the directory). Framing is newline-delimited JSON-RPC —
  what the `ahp` SDK's `Transport` trait abstracts over.
- **Lockfile**: `~/.himark/agent-host/host.lock` —
  `{pid, socket, protocolVersion, startedAt, build?, http?}`
  (camelCase). Liveness is pid-probing (`kill(pid, 0)`); a booting
  host that finds a live lock simply exits 0 — the singleton is the
  lockfile plus liveness, and the loser of a race connects to the
  winner. `http` is filled in by the running host when its web face
  is up (§7).
- **Discovery is PER SEAT, never a precedence chain**: every drawer
  row finds only ITS host — the himark row reads our lockfile; a
  pinned row (`WireHost::at`) uses its address. Hosts come and go at
  will; a row whose host is down shows offline, it never silently
  borrows another host.
- **The daemon binary** ships in the bundle BESIDE the app
  executable — ~70 crates, no rendering, a fraction of the app
  binary. `host_discovery::resolve_daemon_binary` resolves it via
  `$HIMARK_AGENT_HOST_BIN`, then as `current_exe`'s sibling, then on
  `PATH`.
- **Autostart + takeover**: the engine calls
  `host_discovery::autostart()` at boot. A live host whose lockfile
  `build` stamp (`path:len:mtime` of the daemon binary) matches the
  resolved daemon is adopted; a stale one is SIGTERMed and awaited
  (a host that refuses to die is adopted anyway — never a boot
  hang), then the fresh daemon spawns detached and the lockfile is
  polled. An app-only rebuild does not restart the host: the stamp
  compares against the daemon binary, not the app. The host outlives
  himark — sessions keep streaming while no client is attached,
  which is the property that makes it a separate process at all.
- **Auth on the socket**: none — the socket lives under the user's
  home. The HTTP/WebSocket face is token-gated (§7).
- **The watchdog**: each host tags itself `host#N`, tracks inflight
  requests, and a watchdog thread logs "RUNTIME DEAF" if the tokio
  heartbeat lags beyond 3s — the wedge tripwire.

## 3. The AHP server half

Version policy: **the host speaks the version our crates speak** —
0.7.0 via the vendored `ahp-types`; the initialize handshake
intersects the client's offer (himark offers `["0.8.0", "0.7.0"]`
and lands on 0.7.0). Version moves are our commits on both ends at
once; nobody can bump the protocol under us.

State model:

- **THE state is one persistent VALUE** — the same discipline the
  client's Store lives by, on both sides of the wire. `State` is
  built of `rpds` persistent maps (`sessions`, `chats`,
  `subscribers`, `terminals`, `changesets`, `histories`,
  `lsp_diagnostics`, `watches`, `contents`, `searches`, the replay
  queue); entries are `Clone`, live ends (agents, ptys, watchers,
  outboxes) behind `Arc`s. Readers take an O(1) snapshot
  (`Host::snapshot`) they may hold across anything, awaits included;
  writers derive the next value copy-on-write and swap it atomically
  (`Host::update`). The `Mutex` is a swap latch, never a region to
  think inside — a panicked update never bricks the state. Truly
  live OBJECTS (the lsp pool, an agent's turn plumbing) stay beside
  the state, never in it.
- **Requests are INDEPENDENT**: every request answers on its own
  task — one that parks (a language server thinking, a search
  walking a big root) never wedges the connection's read loop. The
  publish-before-response contract is per-request (a handler's
  broadcasts and its answer leave the same task through the one
  outbox, in order).
- **Channels**: `ahp-root://`; `ahp-session:/<id>` per session;
  `ahp-chat:/<id>` per chat; `<session>/annotations`;
  `ahp-terminal:/<id>`; `hihost-changes:/<folder>` (and
  `?commit=<sha>`); `hihost-history:/<folder>`; `ahp-document:/…`;
  `ahp-lsp-diagnostics:/<id>`. Subscribe answers a snapshot; actions
  stream after it; `serverSeq` orders everything per connection.
- **Reducers**: the host maintains authoritative state with the
  OFFICIAL pure reducers published in the `ahp` crate
  (`apply_action_to_root/session/chat/terminal/changeset/annotations`
  — line-for-line ports of the spec's `reducers.ts`). Providers
  author actions; the reducers fold them; a subscriber's snapshot is
  always the folded state. State semantics come from the spec, not
  from us. (The himark-owned extensions — documents, history — fold
  their own state; their vocabulary is ours, docs/ahp/ahp-ext.md.)
- **One update carries everything**: `apply` bumps `serverSeq`,
  folds the action, appends the chat's ndjson log, fans out to the
  channel's subscribers, and pushes onto the bounded replay ring —
  that serialization is what makes per-channel order real on the
  wire and in the log.
- **Reconnect replays**: `reconnect(clientId, lastSeenServerSeq,
  subscriptions)` re-subscribes and answers the missed action tail
  from the replay ring when it still covers the gap, or fresh
  snapshots when it does not.
- **Write-ahead contract**: client-dispatched actions
  (`chat/turnStarted`, `chat/toolCallConfirmed`,
  `chat/pendingMessageSet/Removed`, `chat/turnCancelled`) validate,
  apply, ECHO to all subscribers, and trigger the provider — the
  contract `higent` banks on (`dispatchAction` is a NOTIFICATION,
  not a request).
- **Persistence — two stores, two jobs**:
  `~/.himark/agent-host/sessions/<native-id>/` holds the session
  manifest (`session.json`: dirs, provider, native/provider session
  ids, title, config, annotations) and the AHP-SHAPED transcript
  (append-only ndjson of actions per chat — what `fetchTurns` pages,
  what boot replays through the reducers so transcripts survive
  restarts; an unlisted session with no turns is reaped at boot).
  The provider's NATIVE store (`~/.claude/projects`,
  `~/.codex/sessions`) stays untouched — the source of truth for
  RESUME and CLI interop; the host only reads catalogs from it.
- **A permanent local session** (`hihost-fs:/local`, provider
  `hihost`) is always present: the seatless "Local Files" world —
  the fs/search root when no agent session is in play. It is
  filtered out of `listSessions`.

The served surface: `initialize`, `subscribe`/`unsubscribe`, `ping`,
`reconnect`, `listSessions`, `createSession`, `disposeSession`,
`createChat`, `fetchTurns`, `dispatchAction`,
`resolveSessionConfig`, `sessionConfigCompletions`,
`resourceRead/Write/List`, `createResourceWatch`, `search`,
`openDocument`, `storeDocument`, `lsp/*`, `createTerminal`,
`disposeTerminal`, `httpServe`.

## 4. The providers (the termination)

Two providers exist — Claude and Codex — and the seam is
deliberately AHP-shaped: a provider EMITS `StateAction`s through one
sink (`AgentSink { action, stash }`); the host folds them through
the reducers and fans them out. No third vocabulary in between (the
no-parallel-vocabulary rule, applied inside the host). One agent
process serves one chat: the claude CLI is one conversation per
process, so a second chat in the same session spawns a second
process — `createChat` = spawn, cheap.

### 4a. Claude, native

The provider drives the `claude` CLI itself — resolved via
`$HIMARK_CLAUDE_BIN`, then `PATH`, then the usual install spots —
with `--print --input-format stream-json --output-format stream-json
--verbose --include-partial-messages --permission-prompt-tool stdio`,
ndjson over the child's stdio. Identity: a fresh chat spawns with
`--session-id <native-id>`; a chat that has spoken respawns with
`--resume=<native-id>` — the CLI's session id IS the storage id, and
transcripts live in `~/.claude/projects/<cwd>/` where the terminal
reads them. Config rides the spawn: `--permission-mode`, `--model`,
`--add-dir` per extra working directory, and `MAX_THINKING_TOKENS`
for the thinking level (low/medium/high tiers).

Event mapping: streamed content blocks → `chat/responsePart` +
`chat/delta` (markdown) and `chat/reasoning` deltas (thinking);
`assistant` tool_use snapshots → `chat/toolCallStart`; the CONTROL
channel's `can_use_tool` ask → `chat/toolCallReady` with a pending
confirmation (allow/deny options; the answer returns as a
`control_response`, deny carries "The user declined."); tool results
→ `chat/toolCallComplete`; the terminal `result` → usage +
`chat/turnComplete` (or `turnCancelled`/`error`). Interrupt is a
`control_request {subtype: interrupt}`. A dying CLI fails every
pending turn with `chat/error {errorType: "agentGone"}` carrying the
last stderr lines.

File-edit tools (`Write`/`Edit`/`MultiEdit`/`NotebookEdit`) get
their before-text captured at the ask, and both sides travel BY
REFERENCE as stashed `ahp-content:/<id>` refs served by
`resourceRead` — AHP's diff-by-reference rule holds regardless of
provider.

The drawer lists the user's EXISTING terminal sessions:
`~/.claude/projects/*/<session-id>.jsonl` is scanned into catalog
rows (title = the first real user message), and opening one resumes
it with `--resume`. Backfill reconstructs the user/assistant text
turns.

### 4b. Codex, native

The provider drives `codex app-server` over its native ndjson
JSON-RPC: `initialize`/`initialized`, then `thread/start` (or
`thread/resume {threadId}` when a native thread id is known). Turns
ride `turn/start {threadId, input, approvalPolicy, sandboxPolicy,
model?, effort?}`; the permission mode maps onto the sandbox policy
(`bypassPermissions` → full access, `plan` → read-only, else
workspace-write with the session's directories as writable roots);
approvals answer `{decision: accept|decline}`; interrupt is
`turn/interrupt`. Item/delta/usage notifications map onto the same
AHP actions as Claude's blocks.

The AHP session keeps its own stable storage id while the returned
Codex thread id is persisted separately in the manifest — host
restarts resume the same native thread without changing the AHP
channel or its transcript. Terminal-created threads are cataloged
from `~/.codex/sessions` (subagent sessions and injected context
messages filtered), list in the drawer with text backfilled, and
resume through their original thread id. himark-created threads use
the same native store and remain available to the CLI.

### 4c. Host-owned semantics (both providers)

- **The prompt queue**: `pendingMessageSet` mirrors into host state;
  on natural turn completion the host drains the head into the next
  prompt and emits `pendingMessageRemoved` +
  `turnStarted {queuedMessageId}`. Stop leaves the queue paused.
- **Sessions mutate mid-flight.** The uniform lever: mutate the
  manifest, persist, RETIRE the chat's agent process — an idle one
  now, an active one after its turn — and the next turn respawns
  under the fresh flags. `session/workingDirectorySet/Removed`
  mirror into the manifest and re-grant at the next spawn;
  `session/configChanged {permissionMode}` re-permissions; a turn's
  `Message.model` (id + thinking level) becomes the session's new
  default and respawns for THAT turn. The session state publishes
  the current values in `config.values` so clients seed their
  pickers. (`worktree` is accepted and persisted but not acted on;
  `isolation` is accepted and ignored — folder semantics are the
  only session bootstrapping the host performs.)
- **Turn ids, part ids, timestamps**: minted by the host, persisted
  in the AHP transcript; providers keep their native ids in `_meta`
  for interop.
- **Titles**: first-prompt retitle (the prompt, truncated) +
  `sessionSummaryChanged` on the root feed; a session's first turn
  also promotes it into the listed catalog (`sessionAdded`).
- **Annotations** (docs/ahp/comments.md) expand into the prompt text at
  dispatch, so agents see the anchored conversation.

## 5. Workspace services

Beyond agents, the host serves the session's workspace over the same
socket — each service either a method or a channel family
(docs/ahp/ahp-ext.md for the wire vocabulary):

- **fs**: `resourceRead/Write/List` straight off the local
  filesystem, and `createResourceWatch` — one-level, non-recursive,
  batch events on the watch channel; unsubscribe releases. Path
  authority: the session's working directories bound the servable
  tree (docs/ahp/agents-fs.md).
- **vcs**: the `higit` git-CLI backend runs host-side against the
  session's directories. Changeset channels
  (`hihost-changes:/<folder>`, `?commit=<sha>` for a revision's
  files) recompute on a polled repo signal and on the host's own
  `resourceWrite`s; blob sides travel as `hihost-git:/` refs served
  by `resourceRead`. History channels (`hihost-history:/<folder>`)
  page the revision log (docs/ahp/ahp-history.md).
- **search**: the `search` method walks the session's directories
  with the ripgrep stack on a per-connection LEASH — the next search
  supersedes the running one, a dead connection raises the cancel
  flag, a cut answer says `truncated` (docs/ahp/ahp-search.md).
- **lsp**: the host supervises language servers per workspace root
  (`lsp/*` envelope methods forwarded verbatim; capabilities and
  diagnostics host-synthesized; diagnostics stream on their own
  channels) — which also deduplicates servers between himark and
  anything else that wants language intelligence, and survives
  editor restarts the way sessions do (docs/ahp/ahp-lsp.md).
- **documents**: collaborative text over AHP —
  `openDocument`/`storeDocument` + `ahp-document:/` channels with
  rebase-based convergence and host-owned disk reload
  (docs/ahp/ahp-documents.md).
- **terminals**: `createTerminal`/`disposeTerminal` + terminal
  channels over a real PTY (docs/ahp/terminal.md).

## 6. The client half

`hiahp::wire::WireHost` is the seat. Discovery is a per-seat enum:
our host (lockfile → `unix:<socket>`, with an autostart fallback
gated on `$HIMARK_HOST_AUTOSTART`), a pinned URL (`WireHost::at` —
the add-host-by-URL row), or a VS Code agent host (its lockfile /
`HIMARK_AHP_URL`, WebSocket with `?tkn=`) — a compatibility seat
that predates our host and remains selectable. Transports sit behind
one `Connector` trait: unix-socket ndjson on desktop, WebSocket for
remote faces and the browser build; any send/recv failure latches
the connection dead. Connect dials with a deadline, initializes with
`["0.8.0", "0.7.0"]`, and keeps the link alive with pings; reconnect
carries the subscriptions forward and takes the host's replay or
falls back to snapshots. `poll_channel` waits for a
not-yet-subscribed feed on a short re-check interval instead of
parking forever — a poll arm can never wedge on subscription order.

Everything above the wire — the seat trait, effects, registry,
drawer, panel — speaks AHP vocabulary and does not know which host
serves it (docs/ahp/agents.md).

## 7. The web face

`httpServe {enabled, bind?, webRoot?}` starts a token-gated HTTP +
WebSocket face on the host: it binds a port, mints a token,
advertises `http://<lan-ip>:<port>/?tkn=…` (also written into the
lockfile's `http` field), serves the web build as static files with
cross-origin-isolation headers, injects the AHP URL into the page,
and upgrades to a WebSocket serving the same AHP server. This is the
remote road that exists today: the same host, reached over the
network instead of the socket (docs/ahp/remote.md).

## 8. Decisions taken (and their why)

- **Keep AHP between himark and the host** — all of higent/hiahp
  and the fs seam are host-agnostic; the host is swappable; AHP's
  channel/reducer model is genuinely good and its reducers are
  published in rust.
- **Terminate agent protocols in the host, not in himark** — agents
  are transient subprocesses of a long-lived host, so sessions
  survive editor restarts; himark stays one protocol simpler.
- **Flagships get native wires** — the adapter route costs CLI
  interop (session identity, resumability, terminal sessions in the
  drawer) and the full config surface (permission modes, thinking,
  hooks, skills, MCP), and adds a third-party release schedule.
- **Providers emit AHP actions directly** — the provider seam's
  vocabulary IS `StateAction` (native ids in `_meta`); no third
  internal vocabulary.
- **ndjson everywhere** — one framing for every wire in the
  building; no Content-Length ceremony.
- **Official reducers for state** — server semantics match the spec
  by construction rather than by care.
- **History lives in the host** — whoever terminates the agent
  protocol owns the transcript. Append-only ndjson per chat, paged
  by `fetchTurns`, replayed at boot.
- **A standalone tiny daemon beside the app binary** — the host
  must outlive the editor, and the build-stamp takeover keeps
  client and host from skewing: a host update is "the app noticed
  and restarted the daemon", never a version negotiation.
