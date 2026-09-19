# The host

himark is a headless engine. It is handed a canvas and events, computes
the next state and the next frame, and asks for things it cannot do
itself. Everything on the other side of that line is **the host**: the
program that owns the process, the windows, the GPU surface, the event
loop, every thread, and every capability that touches the outside world.
The Swift shell, the winit shell and the web shell are hosts; a test
harness is a host; the engine never knows which one it is talking to.

The line is strict in both directions:

- **himark owns no resources.** It spawns no threads, opens no files,
  forks no processes, creates no windows, and never blocks. Its whole
  contract with the operating system goes through the host.
- **The host owns no semantics.** It never inspects documents, never
  routes commands, never decides what a click means. It supplies
  mechanism — a thread to run on, a native file picker, the clipboard —
  and delivers results back through the one front door.

The platform host's job has narrowed on purpose: everything
workspace-shaped — files, watches, search, vcs, language servers,
terminals, documents, agents — is served by the **himark agent host**
over AHP (docs/ahp/agent-host.md) and reached through seats, not through
platform callbacks. What remains at the FFI seam is the shell's own
territory: windows, input, IME, fonts, rendering, pickers, clipboard.

`docs/editor/embedding-api.md` specifies the concrete FFI surface (functions,
threading rules, the skia handoff); `docs/ui/effects.md` is the effect
system itself. This document is the design story of the boundary: what
the host *is*, and how the engine's deferred work reaches capabilities
only the host has.

## The three signals

The host wires two callbacks at startup and gets one lever back:

- **wake** — "results are waiting": himark asks the host to call
  `drain()` on the main thread soon. Landings apply there, exactly like
  events.
- **effect wake** — "there is background work": himark asks the host to
  run `run_pending()` on a worker thread the host owns.
- **the display link** — frames are the host's schedule entirely. himark
  reports whether anything still moves (`draw`'s reconcile flag, the
  animation tick's answer); silence lets the host pause the link.

That is the whole threading model: one main thread where state mutates
and paints, one serial worker where deferred work runs, and the host
pumping both only when signalled. A fully settled himark costs nothing.

## Effects, briefly

The effect system itself — effects as pure data, the builder/batch/token
launch contract, cancellation lanes, the runner's poll set, handlers
calling effects — is `docs/ui/effects.md`. What matters here is the shape
of the seam:

```rust
pub trait EffectHandler<E: Effect>: Send + Sync + 'static {
    async fn handle(&self, effect: E) -> E::Result;
}
```

One handler per effect type, registered at construction. In-process
handlers are plain compute that completes on its first poll; an
external capability is just a handler whose `handle` body is a round
trip to the outside — the platform's callback table for one family,
an AHP seat for the other. Either way the typed result lifts into an
app command and lands on the main thread like any event — one delivery
mechanism.

## The engine's boot

The shell creates a `HimarkEngine` over FFI (`himark_create`), wires
the wakes, and feeds it events. Construction registers everything that
does not depend on the shell: the compute handlers (search, document
builds, repair — engine-internal, docs/ui/effects.md), the agent-effect
handlers (`hiahp::registry::register_all` — one stateless handler per
higent effect, each a call on the seat the effect carries,
docs/ahp/agents.md), the fs routing handlers (below), the terminal handler
and command (docs/ahp/terminal.md), and the document-channel sync hook
(docs/ahp/ahp-documents.md).

Boot also starts the workspace-services process itself: the engine
calls `host_discovery::autostart()` — the lockfile-singleton dance
that adopts a live himark agent host or spawns a fresh daemon
(docs/ahp/agent-host.md §2). The platform host never manages that process;
it is the engine's own backend.

### The built-in seats

Two seats register at construction, in the seat registry the agents
arc owns (`higent::Servers`, docs/ahp/agents.md):

- **"himark Agent Host"** — `WireHost::himark_host`: our daemon,
  discovered through its lockfile, dialed as ndjson over a unix
  socket. This seat is designated LOCAL (`SeatDirectory::set_local` +
  `Application::designate_local_host`): the authority `local` — every
  plain workspace file — routes here, into the permanent session
  `hihost-fs:/local`.
- **"VS Code Agent Host"** — `WireHost::new`: the secondary
  compatibility seat; a WebSocket client discovered via
  `HIMARK_AHP_URL` or that host's own lockfile (`?tkn=` token).

A third kind arrives at runtime: **add host by URL**. The agents
drawer's add-host row calls the installed flow
(`Agents::install_add_host`), which mints a `WireHost::at(url)` seat
and registers it like the built-ins — a remote host is the same seat
with a different dial string (docs/ahp/remote.md).

### The seat road: workspace effects

The fs-flavored effects the engine has always spoken — fetch, store,
list, subscribe — keep their vocabulary, but their handlers route over
seats (`hiahp::fsroute`): the location's authority resolves to a seat
and session (`s<host>-<session>` scoping, or `local` → the local
seat), and the handler becomes `resourceRead`/`resourceWrite`/
`resourceList`/`createResourceWatch` on the wire. Alongside them:

- **`FindEffect`** — `NativeFindHandler` routes through the seat to
  the host's `search` method (the host-side ripgrep engine,
  docs/ahp/ahp-search.md); results map back under the asked folder.
- **Revision-scoped locations** (docs/ahp/vcs.md) — higit runs HOST-side;
  a diff's before-text is a `hihost-git:/` ref answered over
  `resourceRead`, and `FetchBaseEffect` resolves the gutter stripes'
  base against the change refs the changeset channels published.
- **LSP routes** — definition, references, completion, hover ride the
  seat's `lsp` envelope (docs/ahp/ahp-lsp.md).
- **Documents** — an opened document's channel syncs through the seat
  (docsync, docs/ahp/ahp-documents.md); a save flows through the channel
  when one stands, through `resourceWrite` when none does.
- **Terminals** — `NewTerminalEffect` opens a PTY on the host over the
  seat, end to end (docs/ahp/terminal.md).

Watch delivery closes the loop without the platform: the seat
directory hands each subscription a deliver closure that pushes
`AppCommand::FileChanged` into the engine inbox and fires the wake.

## The platform callback table

`himark_set_host` installs `HimarkHostCallbacks` — the per-capability
table; a null entry means the host does not bring that capability, and
the feature it would carry is simply never registered. Nothing can
launch an effect nobody answers; capabilities are discovered, not
assumed.

Two entries carry real traffic, through the request/fulfill
discipline:

| Effect | Callback | Answered by | Notes |
| --- | --- | --- | --- |
| `FilePickerEffect`, `PickFoldersEffect` | `pick_files` | `himark_host_picked` | Zero files = cancelled. Present → `file.open` registers; picked folders session into the local host (docs/ahp/agents.md). |
| `PickSaveEffect` | `pick_save` | `himark_host_picked_folder` | Null = cancelled; `suggested` seeds the panel's file name. Drives the scratch save-as re-point (docs/ahp/files.md). |

`set_clipboard` is a plain synchronous service — it also gates the
"Share Host over HTTP" command, whose effect asks the local seat for
`httpServe`'s URL and copies it (docs/ahp/remote.md).

The `fetch_document` / `store_document` / `list_directory` entries
survive as **capability flags**: their presence gates which feature
families register (open/reload/diff/lsp commands on fetch; save,
save-all and script running on store; workspace find on list; the
session tree on fetch+list). The traffic itself rides the seat road
above — a shell that brings the flags brings the features, and the
distribution engine installs all of them at construction on desktop
platforms; the winit shell passes the preset
(`HimarkHostCallbacks::agent_host_filesystem()`) whose fs entries are
inert stubs.

### The round trip

An external platform handler is still an ordinary registered handler —
same trait, same registration, same queue, same landing path — whose
`handle` body is:

1. **mint a request id** and open a slot for it in the request
   registry (`HostRequests`);
2. **call out** through the callback table — a C function pointer the
   host installed, invoked on the worker thread with the request id;
3. **await the slot.** The future parks; the worker moves on.

The host does whatever the capability means — hops to its main thread,
shows an NSOpenPanel — on its own time, in its own idiom. When it has
an answer it calls the matching **fulfill entry point**
(`himark_host_picked`, …) on the main thread. Fulfilling fills the
slot, wakes the parked handle (which asks the host for one more worker
pass), and the round trip closes through the ordinary landing path.
A dropped ask takes its slot with it — a late fulfill finds nobody and
answers false; one request, one answer.

### The story of one file open

```
palette: "Open File…"                            (registered iff the host
    │                                             brought a picker)
    ▼
DynamicCommand file.open ──▶ FilePickerEffect { window }     (pure data)
    │ launch (fire-and-forget: no lane owns a picker)
    ▼
worker: FilePickerHandler::handle
    │ request id 7 → slot; callbacks.pick_files(ctx, 7, window)
    │ future parks — the queue keeps draining
    ▼
host: hop to main, NSOpenPanel runs, user picks,
      host answers locations (himark never touches a path)
    │ himark_host_picked(engine, 7, locations)      (main thread)
    ▼
slot 7 fulfills → parked handle wakes → returns the locations
    │ lift → AppCommand (OpenPicked carrying the batch)
    ▼
main: drain → perform: documents open through the ordinary
      open road (fetch over the seat, build off-thread, land);
      picked FOLDERS become session directories on the local host
```

Cancel is not special: the picker answers with zero files, the effect
lands, the command opens nothing. Every path through the host
terminates in a landing.

## Thread contracts, in one place

- **Capability callbacks fire on the worker thread.** A host showing UI
  hops to its main thread itself; the callback must only be cheap and
  non-blocking.
- **Fulfill entry points are main-thread calls**, like every other
  engine call.
- **The worker is serial.** `run_pending` must never run concurrently
  with itself; the poll set and the effect queue rely on it. One
  serial dispatch queue on the host side satisfies everything. Seat
  handlers respect it too: wire futures park on bridge futures from
  the seat's own runtime and never block the worker.
- **The effect channel is single-producer FIFO.** Launches and cancels
  arrive in the program order the UI thread committed them, so a cancel
  always trails its launch; a cancel matching nothing simply means the
  work already completed.
- **Wakes are thread-free signals.** Fulfilling from the main thread,
  landing from the worker — both just ask the host to schedule the other
  side; nothing ever blocks waiting for a thread.

## What this buys

The engine's semantics never fork on "who is asking": every deferred
thing — a repair, a reparse, a picked file, a search, a terminal byte —
is one shape, an effect dispatched to its handler, launched under a
cancellation token, landing as a command. Platform hosts differ only in
which callbacks they install and how they implement them; tests
register fakes and drive the same machinery; the wasm build runs the
same dispatch inline. And because the workspace half of the seam is a
seat rather than a callback table, the same effects reach a host across
the network unchanged — the platform host is not a backend with its own
protocol, it is the small set of handlers that happen to live in the
shell.
