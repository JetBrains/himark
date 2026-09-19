# Terminals over AHP

Every terminal is an AHP terminal. There is no platform-PTY path: the
shell that renders himark never forks a process for a terminal panel.
The PTY lives in the himark agent host (docs/ahp/agent-host.md) — the
process that already stands next to the working tree and serves the
session's files, vcs, search and agents — and the panel talks to it
through the same seat every other workspace service rides. The design
consequence is the point: a terminal opens in the place the session's
work actually happens, it survives the editor (the host outlives
himark), and a remote host serves terminals over the identical wire
with nothing terminal-specific added (docs/ahp/remote.md).

Two facts shape the design:

1. **Terminals are CORE AHP vocabulary, not an extension.** `ahp-types`
   carries the whole surface and the `ahp` crate ships the official
   reducer (`ahp::reducers::apply_action_to_terminal`). Methods:
   `createTerminal` (`CreateTerminalParams { channel, claim, name?,
   cwd?, cols?, rows? }` — the CLIENT mints the channel uri; result
   `null`) and `disposeTerminal { channel }` → `null`. The terminal
   channel's snapshot is `TerminalState { title, cwd?, cols?, rows?,
   content: Vec<TerminalContentPart>, exit_code?, claim, is_pty?,
   supports_command_detection? }`. Actions: `terminal/data`
   (server-authoritative output), `terminal/input` (client→server,
   SIDE-EFFECT-ONLY — the reducer is a NoOp; the spec's own rationale:
   no write-ahead reconciliation for pty i/o, a pty is a stateful
   process), `terminal/resized`, `terminal/exited`;
   `root/terminalsChanged { terminals: Vec<TerminalInfo> }` on the root
   channel.
2. **The host stays minimal-deps.** `rustix-openpty` is already in the
   lock via alacritty's dependency graph, and alacritty's unix tty
   module is a complete spawn recipe to mirror — no `portable-pty`.

## The shape

```
TerminalView (channel-keyed HANDLE)
   └─ terminal::Terminals            the family: channel uri → Arc<Session>
        └─ Session                   alacritty Term + Processor (grid, keys)
             └─ Box<dyn TerminalBackend>
                  └─ AhpBackend { seat, channel }   (frontend-host)
                       write  → seat.terminal_input   (dispatch terminal/input)
                       resize → seat.terminal_resize  (dispatch terminal/resized)
                       hangup → seat.terminal_dispose (disposeTerminal + unsubscribe)
             ← events: Data(text)  → Session::output + coalesced repaint wake
                       Exited(code) → Session::exited
```

The FAMILY owns the PTY session (docs/ui/app-state.md §2): `Terminals`
maps channel uri → the one canonical `Arc<Session>`; `TerminalView` is
a handle over the channel. Displacement drops the handle and the
session survives — the peeker relists it from the family and mints a
fresh handle on pick; `dismantle` (cmd-w) hangs up AND removes the
row, and `Session`'s own `Drop` hangs up as the eviction backstop.

## Opening one

`OpenTerminal::perform` ("New Terminal") resolves the seat on the main
thread, store in hand — the standing rule. It prefers the window's
live session: the session's seat and the summary's first working
directory as cwd. Without a live session it falls back to the LOCAL
seat (`higent::LocalHost` — the himark agent host) with the permanent
local session `hihost-fs:/local` and the workspace's first folder as
cwd. Either way the same effect goes out:

```rust
NewTerminalEffect { seat, session, cwd, window }
    // → Option<Arc<terminal::Session>>
```

The handler (frontend-host) mints `ahp-terminal:/<uuid>` FIRST, builds
the `Arc<Session>` around `AhpBackend { seat, channel }`, then calls
`seat.terminal_open` at 80×24 with an events closure: `Data` →
`Session::output`, `Exited` → `Session::exited`, both followed by a
**coalesced repaint** — a per-terminal `Arc<AtomicBool>`; only the
transition to pending pushes a `RefreshTerminal` command into the
engine inbox and fires the wake. `RefreshTerminal::perform` merely
clears the flag — a non-empty batch already answers "needs redraw",
which IS the repaint; no engine-core involvement.

The landing (`ShowTerminal`) puts the session into the `Terminals`
family and opens a `TerminalView` panel over the channel. If no pane
can take the panel, the session is hung up and removed — dispose
routes to the host, nothing orphans.

## The seat half

The seat trait (`higent::AhpServer`) carries a session-scoped,
UI-blind terminal surface:

```rust
pub enum TerminalEvent { Data(String), Exited(Option<i32>) }
pub struct TerminalHandle { pub channel: Uri }

// trait AhpServer:
fn terminal_open(&self, session: Uri, channel: Uri, cwd: Option<Uri>,
    cols: u16, rows: u16, events: Arc<dyn Fn(TerminalEvent) + Send + Sync>)
    -> SeatFuture<Option<TerminalHandle>>;
fn terminal_input(&self, channel: &Uri, data: String);
fn terminal_resize(&self, channel: &Uri, cols: u16, rows: u16);
fn terminal_dispose(&self, channel: &Uri);
```

Input/resize/dispose are **sync fire-and-forget `fn`s**, not futures —
they are called from `TerminalBackend` on the main thread where
nothing can await; the wire spawns them eagerly on its own runtime.
`terminal_open` on the wire is `createTerminal` (claim `client`, id
`himark`) + `subscribe`: the snapshot's retained content parts feed
`events` FIRST (a retained exit code replays as `Exited`), then a pump
task matches `TerminalData`/`TerminalExited` off the live stream — one
code path for attach and stream. Dispose is `disposeTerminal` +
unsubscribe, which ends the pump and drops the events closure.

Titles arrive as OSC sequences THROUGH the data stream into the
existing terminal parser — the wire's `terminal/titleChanged` action
is deliberately unused.

## The host half (`backend/agent-host`)

**`pty.rs`** (deps `rustix-openpty`, `libc`):

- `spawn(PtyConfig { shell, cwd, cols, rows })` follows alacritty's
  unix recipe: `openpty(None, Some(&winsize))`, IUTF8 on the
  controller, `Command` with user-fd stdio + `TERM=xterm-256color` +
  `COLORTERM=truecolor`; `pre_exec`: `setsid` + `TIOCSCTTY` + close
  fds + reset signal handlers. `-l` (login shell) only for
  `zsh`/`bash`/`sh` basenames. `resize` via `tcsetwinsize` (floors at
  2×2).
- `take_valid_utf8(tail, chunk) -> String` — the wire carries String,
  pty chunks split multibyte sequences: incomplete trailing bytes
  carry over to the next read; mid-stream garbage degrades to U+FFFD.
- The shell comes from `HIMARK_SHELL`, else `$SHELL`, else `/bin/sh`
  (the env override is the hermetic test door).

**`server.rs`:**

- `State.terminals` is an `rpds::HashTrieMapSync<Uri, TerminalEntry
  { state: TerminalState, pty: Option<PtyHalves> }>` — the host's one
  persistent-value state discipline; `pty` is `None` once reaped.
- `createTerminal`: rejects an existing channel, clamps cols/rows to
  `[2, 4096]` (defaults 80×24), resolves cwd via `local_path` when it
  is a directory, else `$HOME`; spawns the pty; seeds `TerminalState`
  (`is_pty: Some(true)`, `supports_command_detection: Some(false)`,
  title = the requested name or the shell basename); inserts the
  entry BEFORE answering `null` (the follow-up subscribe must find
  it); spawns the reader thread; rebroadcasts the root terminal list.
- **The reader thread** — one dedicated blocking thread per terminal,
  holding `Weak<Host>` so a dead host ends the pump: blocking read
  loop → `take_valid_utf8` → `apply(channel, terminal/data)`. EOF or
  error → `child.wait()` → `terminal/exited` with the code, the pty
  halves cleared, the root list rebroadcast.
- **Scrollback is capped at 256 KiB** across retained content parts,
  trimmed front-first at char boundaries — a late subscriber's
  snapshot replays at most that much.
- `terminal/input` dispatches route straight to the pty write —
  side-effect-only per spec: never applied, never echoed, suppressed
  after exit, and written via `spawn_blocking` outside the state lock
  (a full pty buffer must not wedge the serve loop). `terminal/resized`
  is applied AND echoed, then `tcsetwinsize` outside the lock.
- `disposeTerminal` removes the entry (unknown channel → error), kills
  the child, rebroadcasts the root list, answers `null`.

## Absent, deliberately

- **Reattach**: host terminals outlive the editor (connection death
  only drops subscribers — kept on purpose), and the attach-side
  snapshot replay already works; what does not exist is a listing
  surface over `root/terminalsChanged` plus a
  subscribe-without-create seat method.
- Wire `terminal/titleChanged` / `cleared` / claim changes; command
  detection (`supports_command_detection` is served `false`).
