# Agents

himark integrates coding agents through the **Agent Host Protocol**
(AHP): a JSON-RPC protocol in the LSP/DAP lineage where a standalone
**agent host** process owns the authoritative state and any number of
clients subscribe to it. himark is a CLIENT — the server side is
himark's own agent host (docs/ahp/agent-host.md), which terminates the
agent protocols natively: Claude over the Claude Code CLI's
stream-json wire, Codex over its app-server protocol. The engine
stays AHP-blind the way it is LSP-blind and git-blind — a plugin
declares what the feature *means*, a native crate implements what it
*does*, and the distribution wires them.

This is not just a chat. The integration is a HIERARCHY, and each
level of it is a himark surface:

```
server (agent host)          →  a row in the AGENTS DRAWER
 └─ session                  →  the PLACE YOU WORK (folders come from it)
     └─ chat                 →  a ChatPanel inside that session
         └─ turn / cell      →  the transcript
```

The user's mental model: an AHP server is a thing you HAVE (like a
git remote); its sessions are things you OPEN and work IN (the
session IS the workspace — docs/ui/app-state.md §6); chats happen
INSIDE the session you are in.

The load-bearing decisions, up front:

1. **AHP types pass through undigested.** `ChatState`, `Turn`,
   `ResponsePart`, `SessionSummary`, `AgentInfo`, `StateAction` are
   client-facing state types with stable ids and reducer semantics,
   from the vendored `ahp-types` crate (serde-only, no I/O). `higent`
   renders them directly. Inventing a parallel vocabulary would be a
   translation layer with no consumer.
2. **Servers are first-class.** A registry of seats
   (`higent::Servers`), a host id on every effect, no uri-prefix
   routing. Adding a server is just adding a server.
3. **A session IS the place to work.** Opening a session switches
   the window to it; its folders DERIVE from the session's
   `workingDirectories` (`higent::session_folders`). There is no
   binding: `SessionId { host, session }` is the identity join
   (docs/ui/app-state.md §6), and everything app-side the session owns
   lives in its row under its `Host`.
4. **Everything is reactive, per protocol.** The session list is
   `listSessions` + the root notifications (`root/sessionAdded`,
   `root/sessionRemoved`, `root/sessionSummaryChanged`); session and
   chat channels are subscribe-snapshot-then-actions. One long-poll
   recipe (park on a feed, land, relaunch) at every level. No
   refresh buttons, no polling timers, no corners.
5. **The transcript is turns, and turns are the lazy-loading unit.**
   `ChatState.turns` is a TAIL WINDOW; a standing `turns_next_cursor`
   means older turns exist. Scrolling near the top launches
   `fetchTurns`; the host dispatches `chat/turnsLoaded` before
   responding. History is never loaded whole, per protocol.
6. **The chat component is
   `Col<ScrollView<ListView<TurnView>>, EditorView>`.** One row per
   turn, keyed by turn id; a `TurnView` is itself a `ListView` of
   cells. Under the list, the composer.
7. **Every markdown-bearing cell is a real markdown editor over an
   anonymous document** (the comment-card recipe). Streamed deltas
   APPEND through the ordinary edit door; stateful parts REWRITE via
   a Myers diff (`myersdiff::diff` — a patch wants the minimal exact
   edit, docs/editor/structural-diff.md decision 3). Styling, layout,
   heights and repairs ride
   exactly the lanes typing rides.

## Placement — the higit split, again

| crate | role |
|---|---|
| `himark::higent` | the domain and the UI: the seat trait (`AhpServer`), the effects, the `AgentsPanel` drawer, `ChatPanel`, `TurnView`, cells, the `Agents`/`Hosts` stores, the commands |
| `hiahp` | AHP client glue: the wire seat (`wire::WireHost`), the stateless effect handlers (`registry`), the fs/find/lsp routing over seats, docsync |
| `frontend-host` | registers the handlers and commands, seeds the built-in seats, owns the terminal fork |

`editor` carries the theme sections (`ChatChrome`; the drawer reuses
`TreeChrome`) — 2x-tuned in both theme files, the chrome-scale rule.
The wire seat runs on hiahp's own tokio runtime (host-territory code,
the reader-thread rule).

## Servers: hosts are first-class

`higent::Servers` is the seat registry. A seat is a live object (it
owns sockets and runtimes), so the registry is OWNED BY `Application`
(docs/ui/app-state.md §2) and INJECTED into every gathered store as a
read-only component — launch sites resolve `HostId → seat` against
the store, but registration is `Application::register_seat`, never a
store write.

One SEAT per server implements the whole effect vocabulary below —
every seat is a `WireHost`, differing only in how it dials. Effects
CARRY their seat: the launch site (a command, a panel perform —
always on the main thread, store in hand) resolves the host id and
embeds the handle; the ONE registered handler per effect type (the
hard rule) is a stateless call on it. The worker never sees the
store, the store never sees the worker, and landings correlate by
whatever the launch captured.

**The no-kind rule.** "A session must not know which server minted
it" stands — nothing anywhere branches on which host answered. But a
session's SERVER is not a kind: it is an ADDRESS — the connection
that answers its channels, its parent in the drawer's tree. Addresses
are carried (the host id on every effect, the tree's structure),
never dispatched on by variety. The seat trait is the uniformity
guarantee: the panel cannot tell which seat answered.

The distribution registers two built-in seats at construction
(`Application::register_seat` + `higent::Agents::seed` +
`Hosts::install_uris`, frontend-host):

- **"himark Agent Host"** — the PRIMARY wire: our own daemon over a
  unix domain socket, ndjson JSON-RPC, discovered through its
  lockfile (with an autostart fallback gated on
  `$HIMARK_HOST_AUTOSTART`). This seat is also designated the LOCAL
  host: plain file workspaces route to its permanent
  `hihost-fs:/local` session, and it serves files, watches, vcs,
  search, lsp, documents and terminals beside the agents
  (docs/ahp/agent-host.md, docs/ahp/ahp-ext.md).
- **"VS Code Agent Host"** — the secondary COMPATIBILITY seat: a
  WebSocket client discovered via `HIMARK_AHP_URL`, else that host's
  lockfile (host, port, `?tkn=` connection token).

**Add server by URL exists**: the drawer's add-host row takes an
address, and the installed flow (`Agents::install_add_host`) mints a
`WireHost::at(url)` seat, registers and seeds it like the built-ins.
Transports sit behind one `Connector` trait, so a pinned URL is just
a different dial string (docs/ahp/remote.md).

Discovery is PER SEAT, never a precedence chain: every drawer row
finds only ITS host; a row whose host is down shows offline and never
silently borrows another host.

## The protocol, as much as the UI needs

- **Channels are URIs**; every command/notification carries
  `channel`. The root (`ahp-root://`) carries the agent catalog and
  config; sessions live at server-minted uris; chats at
  `ahp-chat:/<cid>`. Subscribing answers a snapshot; updates arrive
  as `action` notifications (`ActionEnvelope` with `server_seq`),
  applied by pure per-channel reducers.
- **The session CATALOG is not channel state** — `RootState` carries
  the agents list. The catalog is `listSessions` (paginated,
  most-recently-modified first, opaque cursors) plus three
  root-channel NOTIFICATIONS that keep a cached list live:
  `root/sessionAdded { summary }`, `root/sessionRemoved { session }`,
  `root/sessionSummaryChanged { session, changes }` — `changes` is a
  PARTIAL summary: present fields replace, omitted fields stand.
  Notifications are ephemeral (never replayed): reconnect means
  re-`listSessions`. This is exactly the drawer's reactive contract.
- **`SessionSummary`** (the row's substance): `resource` (the uri —
  the identity), `provider`, `title`, `status`, `activity`,
  `workingDirectories`, `createdAt`/`modifiedAt`, aggregate diff
  stats. **`SessionState`** (the channel's substance) adds the
  `chats` catalog, `defaultChat` (a routing hint), `lifecycle`,
  `config` (schema + values), `changesets`, `inputNeeded`.
- **`ChatState`**: `turns: Vec<Turn>` (the window),
  `turns_next_cursor: Option<String>`, `active_turn`, `draft`,
  `queued_messages`, plus catalog fields (title, status, activity).
- **`Turn`**: `id`, `message: Message` (text + origin kind),
  `response_parts: Vec<ResponsePart>`, `usage`, `state:
  complete|cancelled|error`, `error`.
- **`ResponsePart`** (tagged by `kind`): `markdown`, `reasoning`,
  `toolCall` (a status-tagged lifecycle), `systemNotification`,
  `inputRequest`, `contentRef` (large content by reference —
  `resourceRead` fetches it). Every union has an `Unknown` tail —
  forward compatibility is the wire's own promise; unknown parts
  render as a quiet notice.
- **Streaming** is create-then-append: `chat/responsePart` creates a
  part, `chat/delta` / `chat/reasoning` append text by part id; tool
  calls advance by `chat/toolCall*`; `chat/turnComplete` /
  `chat/turnCancelled` / `chat/error` retire `active_turn`.
- **Sending** is write-ahead: the client dispatches
  `chat/turnStarted` via `dispatchAction`, applies it optimistically,
  and the server echoes it into the ordered stream.
- **Lazy loading**: `fetchTurns { channel, cursor }` — the host
  dispatches `chat/turnsLoaded` (older turns, oldest-first) before
  responding; the result itself is empty.
- **Session creation**: `createSession { channel, provider,
  workingDirectories, config }`. `disposeSession` removes a session
  (and broadcasts `root/sessionRemoved` to every client).
- **Session config**: `resolveSessionConfig` answers the pickers'
  schema (mode, permission mode, worktree) and current values;
  `sessionConfigCompletions` exists beside it — the himark host
  answers completions with an empty list. Two values are worth
  stating plainly: `worktree: true` is accepted into the session
  manifest but NOT honored — no git-worktree bootstrap happens; and
  `config: {isolation: "folder"}` matters only to the VS Code host
  (without it that host bootstraps a git worktree and an `agents/*`
  branch) — the himark host ignores `isolation` and always works
  directly in the session's directories.

## The effects (declared in higent, handled statelessly in hiahp)

Every effect CARRIES its seat (`seat: Arc<dyn AhpServer>`, resolved
at the launch site). Ask-answer effects never conflate (the standing
rule); the polls park (the external-handler contract). Result
payloads with wire vectors are MOVED into their landings, never
cloned.

```rust
// ---- the server level (the drawer's substance) -------------------
ConnectServerEffect { seat }                  // → Result<RootInfo, String>
ListSessionsEffect  { seat, cursor }          // → Result<SessionsPage, String>
PollServerEffect    { seat }                  // → Vec<ServerEvent>
CreateSessionEffect { seat, working_directories, options }
                                              // → Result<Uri, String>
ResolveSessionConfigEffect { seat, working_directory, config }
DisposeSessionEffect { seat, session }        // → Result<(), String>
ShareHostEffect      { seat }                 // → Result<String, String>

// ---- the session level (the place-to-work's substance) -----------
SubscribeSessionEffect { seat, session }      // → Result<SessionState, String>
PollSessionEffect      { seat, session }      // → Vec<StateAction>
CreateChatEffect       { seat, session }      // → Result<Uri, String>
SubscribeAnnotationsEffect / PollAnnotationsEffect { seat, session }
SubscribeChangesetEffect / PollChangesetEffect { seat, channel }
SubscribeHistoryEffect { seat, channel }

// ---- the chat level (the panel's substance) -----------------------
SubscribeChatEffect  { seat, chat }           // → Result<ChatState, String>
FetchTurnsEffect     { seat, chat, cursor }   // → Result<TurnsPage, String>
StartTurnEffect      { seat, chat, text, attachments, model }
                                              // → Result<(), String>  (ack; the turn streams)
PollChatActionsEffect{ seat, chat }           // → Vec<StateAction>
CancelTurnEffect     { seat, chat, turn_id }
DispatchChatActionEffect { seat, channel, action }
FetchFileEditEffect  { seat, before, after }  // → Result<FileEditContents, String>
```

The seat trait (`higent::AhpServer`, ~45 methods) mirrors this
vocabulary one method per effect and carries the rest of the
workspace surface beside it — `resource_read/write/list/watch`,
`search`, the terminal quartet (docs/ahp/terminal.md), changeset,
history, annotation and document channels, and the `lsp` envelope
(docs/ahp/ahp-ext.md). `hiahp::registry` holds the stateless handlers. An
unsubscribed wire channel's poll re-checks on a short interval
instead of parking forever — a poll arm can never wedge on
subscription order.

## The agents drawer

`higent::AgentsPanel` — a drawer on the RepositoryView recipe
(docs/ahp/vcs.md): toggle command `agent.toggle-agents`, a toolbar
button, `TreeItemView` rows. The tree:

```
▾ himark Agent Host                    server row: name + status
    ▸ himark — Reply with the single…  title · status glyph · activity
    ▸ docs cleanup                     +12 −4      (diff stats badge)
    ＋ New Session…                     the action row
▾ VS Code Agent Host
    …
＋ Add host…                            the add-by-URL row
```

- **Server rows** carry the seat's display name and its connection
  status (idle / connecting / connected / failed-with-message). A
  failed server row shows the error and re-asks `ConnectServerEffect`
  on activation — reconnect is a click, not a restart.
- **Boot is the drawer's reconcile** (the mount-reconcile rule):
  first paint answers `Boot`; for each registered server the panel
  launches `ConnectServerEffect`, then `ListSessionsEffect` walking
  cursors (bounded pages — the catalog is small), then holds ONE
  `PollServerEffect` parked per server, relaunched per landing (the
  long-poll recipe, drawer edition). Landings MERGE into the store:
  added prepends, removed drops, changed partial-merges onto the
  cached summary (protocol semantics: present fields replace).
  Reconnect re-lists — notifications are not replayed.
- **Session rows** render the summary: title, a status glyph
  (`SessionStatus`), the activity line while the agent works, the
  aggregate `+N −M` badge when the server supplies diff stats. All of
  it updates live from `root/sessionSummaryChanged`. The himark
  host's catalog includes the user's terminal-created CLI sessions
  (`~/.claude/projects`, `~/.codex/sessions`) — opening one resumes
  it (docs/ahp/agent-host.md §4).
- **The "+" action row** sits under each server's sessions: `＋ New
  Session…`. Activation opens THE NEW-SESSION COMPOSER (below) with
  the row's host preselected. An installed `NewSessionFlow` (a
  `HostId → command` factory in the `Agents` store) OVERRIDES the
  composer — the e2e batteries stub the whole create road through
  it; production installs nothing.
- **The add-host row** turns into an inline editor on activation:
  Enter submits the URL through the installed add-host flow, Escape
  cancels.
- Sessions are never silently disposed by the drawer;
  `DisposeSessionEffect` is in the vocabulary, activation only opens.

## The new-session composer

The screen a session BEGINS in:
`himark::new_session::NewSessionView`, an ordinary full-bleed PANEL.
The production shells open it on every fresh window (the ENGINE's
`compose_new_windows` — the shell's choice, not the app's; the test
batteries keep the bare scratch boot), the drawer's "+" row opens it
host-named, and `session.new` ("New Session") opens it from the
palette. A full-window markdown prompt editor (line numbers, the
"What are you building?" placeholder) sits over a controls row of
COMBO selectors — HOST, DIR, MODE, MODEL, EFFORT, EDITS — a "New
worktree" checkbox, the ⌘⏎/⏎ hints and START SESSION.

- **The combos are anchored-overlay consumers** (docs/ui/UI.md,
  Overlays; `himark::combo`): a cell click drops the option menu as a
  `WINDOW`-host popup — UP over the bottom row — with
  Up/Down/Enter/Escape on the popup's own keymap and a backdrop that
  closes on outside clicks.
- **Nothing invents choices.** MODE/EDITS render the host's
  `resolveSessionConfig` schema (`enum` + `enumLabels`; the `worktree`
  boolean renders as the checkbox); MODEL groups every advertised
  model under its provider heading, selecting both provider and model
  in one pick, and EFFORT comes from the picked model's
  `configSchema` (`thinkingLevel`). HOST lists the registered seats;
  DIR leads with "No folder" (the default — a FOLDERLESS start is
  legitimate), then the picked host's catalog folders, then "Choose
  folder…" (the standing picker capability). Start needs only a
  connected host and a non-empty prompt; a dirless session runs its
  agent in a scratch cwd and gains folders later through the chat's
  session toolbar.
- **The store is the composer's bus.** The panel launches NO effects:
  it files ONE app-level ask (`ComposerAsk` — connect + catalog walk
  + schema resolve, superseded by a token in the `ComposerFeed`
  mailbox) and every landing writes the STORE (`Hosts` rows, the
  mailbox, `PendingFolderPick`); a paint over a moved fingerprint
  answers `Sync`, which re-derives the combos and drains the mail. A
  displaced or closed composer costs nothing — no landing ever
  routes to a pane position.
- **START SESSION** (or ⌘⏎ over the prompt): the spent composer
  closes (the cmd-w road restores what it displaced), `createSession`
  goes out with the picked provider, resolved `config`, and `model`,
  and the standard open road runs — subscribe, switch, chat panel.
  The prompt rides INTO the chat panel (`Chats::open_with`) and goes
  out through its own write-ahead send the moment the link is ready,
  so the user's first words show like any typed send.

Backend side (docs/ahp/agent-host.md): each agent card advertises its
models with a thinking-level schema, and `createSession` persists the
provider and options into the manifest — `--model` and
`--permission-mode` at spawn, `MAX_THINKING_TOKENS` for the effort.
Sessions created by other roads carry no options and keep the
defaults.

## The session toolbar

The composer's combo row carried into the LIVE chat
(`higent::SessionToolbar`, a strip under the composer band): MODEL,
EFFORT and EDITS combos plus an ＋ FOLDER action — the session keeps
being configurable after it starts.

- **Substance from the live session.** The session channel mirror
  (`SessionChannel`) keeps the protocol's `config` (schema + values,
  folded from the snapshot and `session/configChanged` echoes); the
  toolbar seeds MODEL/EFFORT from the agent card + the published
  `values.model` / `values.thinkingLevel`, EDITS from the
  `permissionMode` schema (`sessionMutable: true`). A paint over a
  moved fingerprint re-syncs (the composer's Sync recipe); made picks
  survive by id.
- **MODEL/EFFORT ride the send.** Every send threads the pick as the
  turn's `Message.model` (`ModelSelection {id, config.thinkingLevel}`)
  — the protocol's designed per-turn slot. The host makes the pick
  the session's NEW default and respawns the CLI under it, so this
  very turn runs on the fresh model.
- **EDITS dispatches** `session/configChanged {permissionMode}` on
  the SESSION channel (the write-ahead road); the echo folds home
  through the session poll into every subscriber's mirror, and the
  host retires and respawns the agent process under the fresh flags
  (docs/ahp/agent-host.md §4c).
- **＋ FOLDER** runs the OS picker and dispatches one
  `session/workingDirectorySet` per picked directory — the grant
  reaches the manifest, the CLI (next spawn's `--add-dir`), the
  changeset catalog, and the workspace tree through the standard
  folds. `workingDirectoryRemoved` is served by the host; the chat
  has no remove affordance.

## Sessions ARE the places to work

`higent::Hosts` holds one row per host; `higent::Agents` keeps the
residual app-side state (the new-session and add-host flows, the
latest-turn stamps). The protocol's session uri IS the identity
(server-minted, stable) — `SessionId { host, session }` is the full
address, and there are NO binding maps (docs/ui/app-state.md §6: the
identity is the join; liveness is the channel mirror's membership).

A `Host` row carries: the display name, the connection status
(idle / connecting / connected / failed-with-message), the agent
catalog, the session catalog (protocol order, live-merged), the
subscribed channel mirrors (`SessionChannel` per session), the
per-session app-side families row (everything a session OWNS
app-side), and the host's uri codec (`ResourceUriMap` — installed at
registration). One seat is additionally designated `LocalHost` — the
fallback address for plain file workspaces and folder picks.

Rules:

- **Opening a session** (a row pick, a fresh creation, a folder pick
  — one flow, `open_session`): `SubscribeSessionEffect` FIRST — the
  channel snapshot is the freshest truth for directories and the
  default chat — then the landing: mirror the channel into the host's
  states, `switch_session` the window there, and file the ENTER-WORK
  as a batch follow-up (docs/ui/app-state.md §4 — it must run after a
  first visit's fresh workbench installs, under the entered session's
  projection): the changes/comments ensures per folder, the
  `ChatPanel` over `defaultChat` (the drawer flows; a folder pick
  stays quiet), and the SESSION POLL LOOP — rooted at the app: each
  drain merges catalog changes (`session/chatAdded`/`Removed`/
  `Updated`) into the mirror and relaunches WHILE the mirror stands;
  a dropped subscription ends the loop at its next landing.
  `root/sessionRemoved` drops the catalog row and the channel; the
  session's families row (and the window's workbench for it)
  survive — they are the user's, not the protocol's.
- **Opening a folder** in a plain workspace sessions it: the picked
  directories match an existing local-host session or create one
  (`CreateSessionEffect` against the local seat) — the local
  filesystem is just another session, served by the himark host.
- **Chats exist inside the session.** The `ChatPanel` VALUE lives in
  the session's families row (its landings dispatch session-addressed
  — `AppCommand::InSession` — so a stashed session's feed keeps
  folding); the workbench holds only a `ChatPane` handle, displaced
  and restored through the ordinary widget mount model.
  `agent.new-chat` creates a chat IN THE CURRENT SESSION —
  `CreateChatEffect` against the session's seat, quietly nothing
  without one. The session channel's own `chats` catalog is the truth
  the store mirrors.

## The chat panel

`ChatPanel` is an ordinary `PanelView`:

```
ChatPanel                      (server, chat) — the address
 ├─ ScrollView<ListView<TurnView, String>>     rows: one per turn (+ active turn tail)
 │    TurnView = ListView<PartCell, String>    cells: message, parts, footer
 └─ composer: EditorView::input + status strip
```

- **Mount = boot** (the mount-reconcile): an idle panel's first
  paint answers `Boot` → `SubscribeChatEffect`; the snapshot landing
  builds the turn rows from `ChatState.turns`, remembers
  `turns_next_cursor`, and starts the poll.
- **`TurnView`** builds its inner list from the `Turn`:
  - the MESSAGE cell — a markdown editor over the message text,
    washed as the user bubble (`user` origin) or plain
    (agent/tool/system origins render as notices);
  - one cell per response part: markdown/reasoning → editors
    (reasoning washed dim, the thought card), `toolCall` → a row of
    the RUN's cell (see **Tool runs collapse** below),
    `systemNotification` → a dim notice,
    `contentRef` → a stub notice naming size/type, unknown → quiet
    notice;
  - a completed tool carrying **`FileEdit` result content yields DIFF
    CELLS** — one per edited file, after the title cell. AHP ships no
    patches: the sides come BY REFERENCE (`FileEditRefs`, the typed
    parse over the wire's opaque `before`/`after` — stashed
    `ahp-content:/` refs the host serves over `resourceRead`), so the
    cell is born PENDING — header band with the file name and the
    advisory `+N -M` badges — while `FetchFileEditEffect` (the two
    `resourceRead`s, composed) fetches the contents. The landing
    builds both texts, DIFFS them client-side (the installed
    `env::Differ` policy — edit-sized inline; the worker offload is
    the monster escape),
    tracks the operation on the AFTER document (`Document::add_diff`)
    and joins the base (`EditorView.base` + the gutter width) — so
    the cell is a REAL diff view: language-routed syntax colors, line
    numbers, gutter stripes classified against the operation, and the
    stripe click's before-card, all riding the standing machinery
    (docs/editor/diff-stripes.md §5–6) with zero registry involvement;
  - the FOOTER cell when the turn did not complete cleanly:
    cancelled / error banner.
  Cells append/rewrite exactly like the anonymous-document recipe
  (decision 7); a cell laid at a stale width reports its rewrap once
  on paint.
- **Lazy loading is a LOADER ROW** — row 0 of the transcript while a
  cursor stands: a "Loading older turns…" band that ACTIVATES ON
  APPEARING. The list's paint pass walks visible rows only, so a Paint
  event reaching the loader IS the appearance signal (the
  paint-reconcile discipline, docs/ui/UI.md); it answers `LoadOlder`, and
  the panel — the state owner — dedups (one fetch in flight, the
  lane token is the guard) and launches `FetchTurnsEffect`. The
  landing seals the page into a `ListSlice` (one `TurnView` per turn,
  keyed by id, oldest-first, the loader re-minted at its head while a
  cursor remains) and splices it OVER the loader row, adjusting
  `scroll_y` by the grown extent — prepending must not move what the
  user is reading. An exhausted cursor splices the loader away for
  good.
- **Landings route by TURN ID, never by row index.** A prepend shifts
  every row below it, so an in-flight cell landing addressed by index
  would come home to the wrong turn. Cell editors' effect scopes lift
  to `ChatPanelCommand::Cell { turn, cell, … }`; the panel resolves
  the turn's CURRENT row through the outer list's structure keys
  (`row_range`, O(log n)) at perform time and routes down the ordinary
  child dance (which re-measures both levels' heights). A landing for
  a replaced turn (the write-ahead placeholder) resolves to nothing
  and drops — the guard rule as ever.
- **Follow-tail**: appends keep the scroll pinned to the bottom only
  while it was already near it — reading back never fights the
  stream.
- **The composer is a bordered input** (idle/focused border colors,
  placeholder over an empty draft) that GROWS with its content — the
  editor's measured `content_height` clamps into
  `[one line, input_max_height]`, and past the max the editor scrolls
  internally (the pane recipe). Its bottom row holds the status text
  and THE BUTTON, three states: **Send** (idle; dim while empty),
  **Queue** (busy + text — the send queues), **Stop** (busy + empty —
  `CancelTurnEffect`, the red square). Cmd-Enter sends/queues; plain
  Enter is the editor's newline (never a send); the editor re-lays on
  width changes (the cell rewrap recipe).
- **The STACK sits between transcript and composer**:
  - the PERMISSION card — `chat/toolCallReady` at
    pending-confirmation raises it: confirmation title, invocation
    message, the `ToolInput::Inline` command preview in a well, and
    the server's numbered options (a default approve/deny pair when
    it offers none). Click or DIGIT answers; Enter over an empty
    composer approves option 1; Escape picks the first deny. The
    answer is a write-ahead `chat/toolCallConfirmed` carrying
    `selectedOptionId`; the widget clears at once and the tool cell's
    face follows the ECHO. Non-digit typing still reaches the
    composer.
  - the PROMPT QUEUE — `chat/pendingMessageSet` write-aheads mirror
    here (snapshot seeds from `ChatState.queuedMessages`); a
    collapsible header (`PROMPT QUEUE n`), one row per prompt with a
    drawn ✕ (`chat/pendingMessageRemoved`). The SERVER drains the
    queue on natural completion — `pendingMessageRemoved` + the next
    `turnStarted` carrying `queuedMessageId`; a Stop leaves the queue
    paused (docs/ahp/agent-host.md §4c).
- **The stream applies at CELL granularity**: `chat/turnStarted`
  replaces the write-ahead placeholder with the echoed turn, each
  `chat/responsePart` / `chat/usage` grows the SAME row by one built
  cell (`RowCommand::Append` through the list dance), and
  `chat/turnComplete`/`turnCancelled`/`error` close it — releasing
  the send gate. `chat/delta` / `chat/reasoning` append TEXT into a
  part's cell through the edit door (the part-id → cell map). The
  `chat/toolCall*` LIFECYCLE moves faces inside the RUN's cell:
  `toolCallStart` joins the run (preparing…), `toolCallReady` flips
  the row to waiting (and raises the ask), the `toolCallConfirmed`
  echo flips it to running/denied, `toolCallComplete` lands the
  result face — plus one DIFF CELL per `FileEdit` in the result. Each
  is addressed by `tool_call_id`, never by row index: rows move under
  expansion, ids do not.
- **Focus is the search-panel recipe**: a `ChatArea`
  (Transcript | Composer) picks where position-less events route and
  whose editor's presentable commands the panel offers — the keymap
  resolves every chord against exactly that offer, so `commands()`
  forwarding runs panel → row → turn → cell.
- **Clone carries the bookkeeping** (the LocationList rule): the turn
  row index map, the cursor, the fetch/poll tokens ride every clone.
  `destroy` cancels the lanes and forwards to rows and composer.

### Tool runs collapse

A transcript is unreadable when every tool call prints its whole
output. Consecutive calls are therefore ONE cell — a RUN — and a run
is a little tree over the unified list (docs/ui/list-tree.md §4), in
`higent/tool_group.rs`:

- **the group row** counts what ran, by display name, in first-run
  order — `4 × Bash, 1 × Grep`. It stands only for a run; a lone call
  speaks for itself;
- **a call row** is one line: the name and what it did (the completed
  `past_tense_message`, the running invocation — elided at the
  reading width);
- **a body row** is the full form — the fenced invocation over the
  output — a markdown `Cell` like any other.

Rules that hold the design up:

- **Bodies are LAZY.** A closed call owns no document, no editor and
  no syntax tree — it holds its full form as a `Text` VALUE, minted
  once at the format boundary (`turn.rs`) and handed to
  `Cell::build_text` as it stands when a reader opens it; closing
  destroys the row and gives all of it back. A hundred calls cost a
  hundred painted lines.
- **Anything else closes the run.** A diff cell, a reply, a notice
  between two calls means two runs — the transcript keeps its order.
- **The work steers the disclosure until the reader takes over.** A
  live call (running, awaiting approval) holds its run open; when the
  last one settles the run closes again — unless the reader has
  toggled it, and from then on the reader's choice stands.
- **Rows are keyed** (`Group` / `Face(id)` / `Body(id)`): landings
  ride keys through `CellCommand::ToolRow`, the `lift_rows_command`
  recipe one level down. Toggles ride the ordinary child dance, which
  re-measures the row — that is how expansion changes heights.
- **A toggle splices only what changed, ANIMATED** — the head and
  tail stand, the disclosure row rides along, so the calls unroll
  from under it and shrink back into it (`splice_slice_animated`, the
  forest's toggle shape). The group hands the list's own words
  (`Animate`, `SetHeight`, reveals) straight back to its list, and a
  click arrives FOCUS-wrapped: the group seats the row, then acts on
  what the row said.

## Chrome

`ui.chat` (`ChatChrome`) in `editor`'s theme, 2x-tuned in both theme
files: `pad`, `gap`, `radius`, `min_cell_height`, `max_content_width`
(cells center in wide panels), `user_surface`, `user_border`,
`thought_surface`, `tool_surface`, `notice_color`, `error_surface`,
`title_size`, `text_color`, `input_height`, `input_fill`,
`divider_color`, the loader and added/removed colors. The drawer
reuses `TreeChrome`. No constant in code.

## The session as a filesystem — and everything else

A session's workspace services ride the same seat as its chats: files
list, read, store and watch through the session's AHP server
(docs/ahp/agents-fs.md); changesets and history channels, host-side
search and the LSP envelope are the himark-owned extension vocabulary
(docs/ahp/ahp-ext.md); terminals are core vocabulary (docs/ahp/terminal.md);
collaborative documents converge over their own channels
(docs/ahp/ahp-documents.md). The drawer, the panel and every effect above
speak to all of it through one trait — which host serves it is an
address, never a kind.
