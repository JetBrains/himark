# himark-core: the engine nucleus

himark is becoming a library ([ui/himark-ffi.md](ui/himark-ffi.md),
[ui/imba-ffi.md](ui/imba-ffi.md)), and the library needs a heart
that is not `Application`. Today `frontend/himark/src/app.rs` is the
shell's god object: the batch loop, the effects plumbing, window
dispatch, frame caching, keymaps, focus, boot wiring — one struct,
and every public API on it grown by accretion. **This document
extracts the clean core: a new crate, `himark-core`, that owns the
store, the command batch loop, the effects runner, and the session
catalog — and exposes ONE minimal, cohesive public API.** Nothing
moves out of himark into it; the core is extracted new and the shell
becomes its client, so the debris stays behind and dies in place.

## The unification: every shell is core + bridges

The two FFI documents describe bridges as an alternative to the
whole-window engine. The truth is stronger, and it is the design:

**There is no whole-window mode. A shell is himark-core plus some
number of `ViewBridge`s, and the workbench is just the biggest view
there is.**

- **The macOS app today** is the degenerate case: ONE bridge whose
  view is the workbench root, filling the window. Everything the
  engine API does — draw, events, IME — is that single bridge's
  contract, worn as a special case.
- **The new iOS app** mints no workbench at all. It bridges the
  views it actually needs — a chat, a changes list, a file tree, a
  diff — one per native screen, inside SwiftUI chrome.
- **macOS, in the future,** ports to the same native frame: AppKit
  windows and splitters around bridged views.
- **The workbench survives** as the composition layer for shells
  that want "the app in a rectangle" — linux, windows, web — a
  VIEW those shells mount as their one bridge, not a framework
  anything else sits on.

So the layering, bottom to top:

```
imba          store · entities · effects vocabulary · views · Verb
himark-core   perform · pump · view registry · lanes · effects
              runner · wake contract · session catalog + lifecycle
features      editor, documents, chat, changesview, filetree, …
              (entities + lanes + effect handlers, registered at boot)
bridges       ViewBridge: per-bridge frame, draw/event/perform/pump
workbench     one big composite view (the linux/windows/web shell's
              single bridge; macOS's, until its native-frame port)
shells        SwiftUI iOS · AppKit macOS · winit · web
```

## Commands: one vocabulary, two address spaces

Commands are THE way to interact with the engine, and they are
host-visible on purpose: a native palette row, a menu item, a key
binding, a bridged view's answer to a tap — all speak the same
vocabulary through the same `perform` door, and the HOST decides
when that door opens, because command handling is part of the
native event loop. Handing the library's client control over
mutation is the point, not a leak. The vocabulary has two arms:

- **`At(Addressed)`** — THE addressed road (entities.md law 5),
  and commands are DATA: `Verb` and `Addressed` clone (the typed
  command underneath always could), an address opens back up
  (`Addressed::downcast` → the typed `(id, command)` pair), and
  the genuinely move-once payloads ride `OneShot` — clone shares
  the in-flight slot, the value moves exactly once, a spent twin
  drops loudly.
- **`Verb`** — the window-free vocabulary that already exists
  (`imba::command::Verb`: `At`, `Dynamic`/`Once` for registered
  commands; `frontend/imba/src/command.rs`).

This section is LANDED (2026-10-10): the whole window/workbench
vocabulary rides `At` at the `WorkbenchShell` entity
(`frontend/himark/src/shell.rs`) — a real entity at one well-known
id, whose command is the old window arm set and whose perform is
the old ceremony, running with a genuine engine-vocabulary fx
through its `App` wrap arm; self-addressed recursions unwrap via
`Addressed::downcast` instead of re-entering their own lease. The
engine's `AppCommand` is `Verb` + `At` + `FileChanged` + the three
editor-environment registrations — nothing in it names a window.
Still to dissolve: `FileChanged` into an `At` on the owning
documents collection, `Register*` into boot-time store writes.

## The public API

The API provides exactly three functionalities, plus one outbound
layer:

1. **Query session state** — snapshots the native shell renders in
   its own views: the session picker, nav titles, badges, the
   running-turn indicator behind a stop button.
2. **Control the session** — create and dispose, and drive turns:
   the composer is NATIVE chrome in the new shells, so send,
   cancel, and model choice cross the API; the bridged transcript
   only displays.
3. **Mint imba views** — bridges for editors, canvases, the
   change/file/history trees, chat transcripts ([ui/imba-ffi.md](
   ui/imba-ffi.md)). The minting door is UNIFIED: given a
   navigation place, Himark gives you a view — the Navigator road
   that already exists (`hikit::navigation::Navigator`: place →
   `DynPanelView`, windowless by design; `NavigationLocation` is
   the erased place that crosses boundaries).
4. **Callbacks, himark → host** — the intents layer: a view the
   host mounted raises "navigate to this" (a file tapped in the
   changes tree, a reference followed), and what it raises IS a
   place. The HOST decides the screen — pushes it, and hands the
   same place back through the minting door. In workbench shells
   the workbench absorbs the same places internally.

The whole surface, and the rule is that it stays this size — every
helper that wants to be `pub` on `Core` must displace something:

```rust
pub struct Core { /* store, ui, view registry, handlers, launcher */ }

impl Core {
    pub fn new(fonts: AppFonts) -> Core;

    // ── commands: the one mutation door, in HOST hands ──
    pub fn perform(&mut self, Vec<Command>) -> Outcome;  // the batch
    pub fn pump(&mut self) -> Vec<Command>;      // what the engine owes
                                                 // the host: landed
                                                 // effects, queued asks
    pub fn commands(&self) -> Vec<CommandInfo>;  // id + title: what a
                                                 // native palette, menu
                                                 // or keymap binds to
    pub fn worker(&self) -> Worker;              // run_pending seat
    pub fn set_wake(&mut self, …);               // "pump on main"
    pub fn set_effect_wake(&mut self, …);        // "run work off-main"

    // ── boot registration (composition root only) ──
    pub fn register_handler<E: Effect>(&mut self, impl EffectHandler<E>);
    pub fn register_lane(&mut self, Lane);       // batch-end sync
    pub fn register_ceremony(&mut self, …);      // per-session install
    pub fn store_mut(&mut self) -> StoreMut<'_>; // the one &mut door

    // ── query (himark-ffi.md's surface, Rust-side) ──
    pub fn sessions_epoch(&self) -> u64;
    pub fn sessions(&self) -> CatalogSnapshot;   // hosts, providers,
                                                 // sessions, activity

    // ── control ──
    pub fn create_session(&mut self, HostId, &[Path], SessionOptions)
        -> Ticket;
    pub fn ticket(&self, Ticket) -> TicketState;
    pub fn dispose_session(&mut self, SessionId);
    pub fn add_host(&mut self, url: &str) -> HostId;
    pub fn start_turn(&mut self, SessionId, text: &str, TurnOptions);
    pub fn cancel_turn(&mut self, SessionId);

    // ── views: ONE minting door (the Navigator road) ──
    pub fn view(&mut self, place: NavigationLocation) -> Option<ViewId>;
    pub fn drop_view(&mut self, ViewId);

    // ── callbacks, himark → host ──
    pub fn set_intents(&mut self, impl Intents);

    // ── for view layers (bridges, workbench) ──
    pub fn store(&self) -> &Store;
    pub fn ui(&self) -> &Rc<UiCtx>;
}

pub trait Intents {
    /// A mounted view asked to navigate. What arrives is a PLACE —
    /// the same currency `view` mints from: the host picks the
    /// screen and hands the place back through the minting door.
    fn navigate(&self, place: NavigationLocation);
}

pub struct Outcome {
    pub epoch: u64,          // store moved: frames revalidate
    pub needs_redraw: bool,
    pub settle: bool,        // a perform raised the settle bit
    pub theme_changed: bool,
}
```

`perform` is today's `perform_batch` (`app.rs:1043`) minus its
window tail: take the store, run the queue with follow-ups, run the
registered lanes under `fx.scope`, launch the effect batch, sweep —
and return an `Outcome` instead of walking windows. The settle
rounds, theme dispatch and file-event delivery that `perform_batch`
does per window today become each frame holder's reaction to the
`Outcome` (a bridge dispatches its own `Settle`/`ThemeChanged`, as
[ui/imba-ffi.md](ui/imba-ffi.md) already specifies).

Nothing mutates behind the host's back. Effects run on the worker
and LAND as commands in the engine's outbox; `wake` says only "you
are owed commands", and the host pumps on its own loop and performs
what it pumped — or defers it, or drops what a dead screen no
longer wants. Who schedules effect landings on the UI loop is
thereby answered: the host, always, through the same two calls
everything else uses.

`start_turn` speaks to the session's default chat (the
`SessionChannel` already names it) through the wire's existing
`ChatClient::start_turn`/`cancel_turn`; the transcript bridge shows
the consequences, and the running state the stop button needs rides
the catalog snapshot's activity. `Intents` is the one outbound
trait, and it is how the `PanelRequest` routing of
[ui/imba-ffi.md](ui/imba-ffi.md) resolves in a native-frame shell:
requests a workbench would absorb become intents the host sees.

The effects machinery moves nearly unchanged: `Handlers`,
`EffectLauncher`, `BackgroundRunner`, the dispatcher/`Lift` types
(`frontend/himark/src/effects.rs`) are already generic over the
command type apart from `register_builtins` — which stays behind,
split among the feature crates that own each handler. The inbox and
wake slots move IN from `frontend-host` (`Shared`, lib.rs): they
are the engine's edge, not the C ABI's.

Core's dependencies: `imba`, `ahp-wire`, the protocol types —
**no editor, no workbench, no feature crate.** The `Workshop` and
every handler that needs it arrive at registration, from the
composition root.

## The catalog moves into core

The session API is core's purpose, so the catalog is core's data.
`ahp-session` splits:

- **Into core:** `Agents`/`Hosts` (hosts, statuses, `AgentInfo`
  lists, `SessionSummary` rows, channels), the sessions lane
  (`sync_sessions_lane`: connect, list, drain `poll_root`), and the
  feed fold (`apply_channel_actions`) — the parts whose only
  dependencies are `imba` and the wire.
- **Stays above:** the `SessionState` row of collection ids
  (`ahp-session/src/session/state.rs` — its ids name chat, tree,
  changes, canvas types core must not know), installed by the
  session CEREMONY the feature layer registers
  (`register_ceremony`); and the drawer UI, which is workbench
  territory anyway.

This is the split the entities plan already calls for (collections
decouple from `Hosts` into crates); the crate boundary just makes
it enforceable.

## Frames go per-bridge; the host compositor composes

Today there is one realized `Frame` per window, cached in
`Application` (`app.rs:90` — arena + widget tree + store snapshot,
with the `'static` erasure documented on the type), drawn into one
skia canvas per window. **That singleton dies. The frame holder
becomes the property of whatever paints: each `ViewBridge` owns its
own frame, realizes its own viewport slice, draws into its own
host-owned layer — and the HOST COMPOSITOR composes the layers.**
One store, many frames.

- The holder generalizes: today's `Frame` is window-shaped (laid
  out by `Application::layout`, command type `AppCommand`). It
  becomes a private type of the bridge machinery, parameterized by
  the bridge's `DynView` root — same arena discipline, same
  invariants, same unsafe kept local to the holder.
- The holder retains NO host geometry. Constraints arrive at
  `measure`, layout size and viewport arrive with EVERY draw and
  EVERY event dispatch — the native scroller owns the viewport and
  it moves between paints, so nothing inside may remember it. The
  frame is a pure memo keyed on (store state, size, viewport): a
  repeated call at unchanged keys pays nothing, and that is its
  whole contract.
- Invalidation inverts: instead of every store door calling
  `invalidate_frames` on a map it owns, `drain` returns the epoch
  and each holder rebuilds lazily when it next draws or routes an
  event against a stale epoch.
- Each bridge has its own `needs_redraw`, so native views repaint
  independently — a chat streaming tokens does not repaint the file
  tree's layer. The workbench-as-one-bridge keeps today's behavior
  exactly: one frame, one layer, whole window.
- Skia stays host-owned per [editor/embedding-api.md](
  editor/embedding-api.md): the host hands each bridge draw a
  canvas backed by that bridge's layer. One shared skia build, N
  surfaces.

## First goal: the iOS app, from scratch

The proof of the API is the SwiftUI rebuild
(`apps/himark-apple/Sources/iOS` retires). The design (2026-10-10):
a session-centric app — session picker up top, three tabs.

- **Shell:** SwiftUI owns the tab bar, navigation stacks, sheets,
  the composer chrome, Liquid Glass. Nothing else is native —
  chat transcript, changes list, file tree, diff, code are ALL
  imba, one bridge per screen, inside `UIScrollView`s that own the
  physics ([ui/imba-ffi.md](ui/imba-ffi.md), touch section).
- **Session screen:** the catalog snapshot drives the picker and
  nav titles; "new session" is `create_session` + a ticket.
- **Tabs:** `himark_bridge_view(core, …, session, role)` for
  chat / changes / filetree; pushing a file opens
  `himark_bridge_document` on the navigation stack.
- The app manages sessions and mounts views; it never sees a
  workbench, a panel id, or a feed.

## The plan

### Milestone 1 — the mac app becomes the degenerate case

The nearest goal. The mac app stops driving the pile of ad-hoc
whole-window FFI and starts speaking the target architecture with
the right cardinality: **one `Core` per process, one `ViewBridge`
per OS window, and that bridge's view is the workbench.** No
session C surface yet — the workbench drives sessions internally,
exactly as today; what M1 proves is the core loop and the bridge
contract under a real foreign host.

1. **Build `ViewBridge` in place.** Inside the himark crate, not a
   new one. The retained per-window `Frame` cache (`app.rs:90`,
   the `frames` map) is REPLACED by one workbench `ViewBridge` per
   window: the holder's arena discipline and invariants move into
   the bridge whole (unsafe stays local to it), and the bridge
   owns what the window tail does today — its `needs_redraw`, its
   settle rounds, its theme/file-event reaction. `Application::
   dispatch*` and `draw_window*` become thin routers to the
   window's bridge; `perform_batch` and the effects road are NOT
   touched — they already run at the right scope, and the bridge
   returns commands into the same loop the `Frame` did. From
   outside `Application` nothing changes; every shell keeps
   working. Gate: the suite + desktop parity by hand. This is
   where the cardinality becomes real: N windows, N bridges, one
   store.
2. **Mirror imba's pipeline into user space.** (LANDED 2026-10-10.)
   The bridge is the staged pipeline, not an entry-point bundle:
   `ViewBridge.layout(store, constraints) → BridgeLayout` (sized;
   `size()` IS measure, a read), `realize(viewport) → Frame`
   (consumed per viewport), `Frame.handle(event) → commands` —
   paint is an event, exactly as inside imba. No caching in the
   pipeline: the registry entry (`Mounted`) is plain data — the
   bridge and the last derived frame with its tags — and each
   CLIENT composes the stages visibly (the window client realizes
   the full rect; a native scroller holds the layout and realizes
   slices).
3. **The normalized C ABI + the mac port, directly.** (Reordered
   2026-10-10: the mac app is the better de-risk than any harness
   — it IS the degenerate case, and it exercises IME, clipboard
   and focus no harness would.) The C surface mirrors the
   pipeline as STAGE HANDLES — `bridge_layout → layout`,
   `layout_size`, `layout_realize(viewport) → frame` (consuming),
   `frame_handle(event) → command handles` — with `draw` only
   sugar minting `Event::Paint` around the per-call borrowed
   canvas, commands host-held (`perform`/`pump`), handles never
   pointers. On the Swift side: one generic `NSView` bridge
   wrapper (metal layer, event factories, per-bridge
   `NSTextInputClient`), the mac app's window content becoming
   one workbench bridge in it. Gate: mac parity by hand.
4. **Extract `himark-core`, leave `Application` behind** — after
   parity, cut along the proven seam: effects machinery, the
   inbox/wake edge, `perform_batch` with the lanes registry and
   the `Outcome` tail. The winit and web shells rebase here too.
   Step 3's ground rules, carried from the triage of the ~49
   legacy `himark_*` exports: the core loop (create, wake,
   run_pending, drain) and the host services (`himark_set_host`
   and the picked/fetched family) keep their shape; draw/events/
   IME/clipboard/focus normalize onto the bridge stages; toolbar
   height and chrome clearance ride the workbench view, not the
   core; the demos die. Memory law: no object pointers cross the
   ABI — every engine object is a HANDLE (a dense id into an
   engine-owned table) with a dedicated release call; stale,
   foreign or double-released is a checked refusal, never UB.
   Pointers appear only as borrowed payload within one call (a
   canvas, UTF-8 in, snapshot strings out). Both surfaces coexist
   during the port; `himark.h` regenerates.
5. **Burn the legacy surface.** The apple shell was the C ABI's
   only consumer: the whole-window `himark_*` entries, the
   per-window draw path, and the engine-side IME plumbing they
   dragged along are deleted from `frontend-host`, not deprecated.

### Milestone 2 — iOS on the native wireframe

8. **The catalog split + session surface.** `Agents`/`Hosts` +
   driver + fold move into core; the query/control/turns API lands
   on `Core`; ceremonies register from the feature layer. Proof:
   the zero-window headless boot test (entities.md), written
   against `Core` alone. Then the C projection: epoch + snapshot,
   tickets, `himark_turn_*`, `himark_set_intents`
   ([ui/himark-ffi.md](ui/himark-ffi.md)).
9. **Library windows and role minting.** Bare scope windows (no
   `compose_new_windows` auto-session), `himark_bridge_view` by
   `(session, role)`, `himark_bridge_document` by location — the
   first bridges that are NOT the workbench crossing C.
10. **The SwiftUI app, from scratch** (the 2026-10-10 design:
    session picker, Chat/Changes/Files tabs, native composer,
    intents-driven navigation). `apps/himark-apple/Sources/iOS`
    retires.

## Decisions

- **Extract, don't relocate.** `himark-core` starts empty and
  takes only what it can own cleanly; `Application` keeps its
  debris until each piece's replacement makes it dead code. No
  flag-day rewrite of the shell.
- **`Verb` is the core command type — no `CoreCommand` enum.** The
  vocabulary exists, is window-free by construction, and is where
  the entities work is already driving all commands. Bespoke
  variants dissolve into `At` or boot-time store writes.
- **The workbench is a view, not a framework.** Demoted, not
  deleted: it remains the single-bridge shell composition for
  platforms that want it, and nothing beneath it may depend on it.
- **One public API, sized to the iOS app.** The SwiftUI rebuild is
  the completeness test: what it does not need, core does not
  expose. Additions after that bar must displace, not accrete.
- **Per-bridge frames over one window frame.** The host compositor
  is better at composition than a software pass over one canvas,
  and independent `needs_redraw` is the perf story (the frame-cache
  discipline carries over, one holder each). The workbench
  bridge preserves today's single-frame behavior byte for byte.
