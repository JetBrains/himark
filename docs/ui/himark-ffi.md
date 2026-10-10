# The himark FFI: the session surface

[imba-ffi.md](imba-ffi.md) is the lower half of the library story:
any imba view mounted as a `ViewBridge`, driven by a host that owns
the frame. This document is the upper half — **the app crossing the
same boundary: the agent sessions the shell sees, creates, and
disposes natively, and the addressing that turns a session into
bridges worth mounting.** Together the two halves make himark a
library. The native shell builds its window out of native views and
bridged ones — a native sidebar listing sessions, a native "new
session" sheet, a bridged chat in a native drawer, a bridged diff in
a native tab — and the workbench never paints.

The whole-window engine API ([embedding-api.md](
../editor/embedding-api.md)) stays untouched; its only session door
today is `himark_perform_command(engine, window, "session.new")` —
fire and look away, nothing comes back. This surface is what gives
the host its hands.

## The engine serves sessions — there is no second client

A shell that wants sessions without the imba UI has, on paper, a
shortcut: link `ahp-wire` and speak to the agent host itself. The
crate is built as exactly that boundary — "no catalog, no
collections, no window anywhere" (`frontend/ahp-wire/src/lib.rs`),
with `SessionClient`/`ChatClient` traits over the daemon's JSON-RPC.

The FFI deliberately does NOT take that road. **The engine already
runs the whole session machine, and the catalog the shell wants is
the catalog the bridged views are built from — one subscriber, one
fold, one truth.** Concretely:

- The sessions lane (`ahp-session`'s `sync_sessions_lane`) connects
  every registered host, lists its sessions, and keeps draining
  `poll_root`; feed landings fold into the store's catalog
  (`Agents`/`Hosts`) through `apply_channel_actions`. One wire feed
  per channel, each drain handling the whole batch — a shell-side
  `WireHost` would be a second subscriber re-implementing that fold
  against a second copy of the state.
- The views the host will bridge — chat, changes, trees — live in
  the same store the catalog lives in. A session the shell created
  through its own wire client would still have to arrive in the
  engine's catalog before a single view of it could be minted; the
  direct road saves nothing and splits ownership.

So the session surface is a PROJECTION of the engine's catalog over
the C ABI: the host reads it, asks for changes to it, and the
engine's existing machinery does the work. The Rust home of that
machinery — and of this API, which the C surface merely wraps — is
the `himark-core` crate ([himark-core.md](../himark-core.md)).

## One catalog, no window

The catalog is engine-level, not window-level. The store is one per
engine — `window_store` takes a window and ignores it
(`frontend/himark/src/app.rs`); windows are focus scopes over the
one store, and `Agents`/`Hosts` ride it globally. Every call in
this section therefore takes the engine alone. (Bridge minting
still takes a window — the next section — because a view is built
with a window's focused seat and theme, not because its session
belongs to one.)

A session is addressed by the pair that names it internally
(`ahp_wire`'s `SessionId`): a **host** (the integer id of a
registered agent server) and a **session uri** (the `resource`
string of its summary). Both cross the ABI as-is.

## Reading the catalog: epoch + snapshot

```
himark_sessions_epoch(engine)          -> u64
himark_sessions_copy(engine)           -> snapshot (handle)
himark_sessions_release(engine, snapshot)
```

(Like every object in this surface, the snapshot is a HANDLE, not
a pointer — see Rules of the road.)

The epoch bumps whenever the fold changes the catalog — a session
added, removed, retitled, its status or activity moved, a host
connected or failed. The discipline rides the loop the host already
runs: wake fires, the host drains on main, and **after a drain it
reads the epoch; moved means copy a fresh snapshot and re-render
the native list.** Equal epochs guarantee an identical catalog —
no snapshot taken, no diffing done.

The snapshot is an owned copy, immutable, freed by its destructor —
cheap to take, because the catalog is persistent structures
underneath (structural sharing, not deep copies). Accessors walk
it:

- **hosts** — count, then per row: id, display name, status
  (idle / connecting / connected / failed + message), and the
  provider list (`AgentInfo`: the agents this host can run) the
  native "new session" sheet is built from.
- **sessions** — count, then per row: one accessor per column of
  `SessionSummary` — host, resource uri, provider, title, status,
  activity, working directories, created/modified timestamps,
  change counts. Flat getters, no structs with embedded pointers;
  strings are borrowed from the snapshot and live until its free.

Deltas are deliberately NOT part of the ABI (see Decisions).

## Lifecycle: create by ticket, dispose by name

```
himark_session_create(engine, host,
                      dirs, ndirs,            // UTF-8 paths
                      provider,               // from the host's agents
                      model_or_null,
                      config_json_or_null)    -> ticket (u64)
himark_session_ticket(engine, ticket)         -> PENDING
                                               | READY(host, uri)
                                               | FAILED(message)
himark_session_dispose(engine, host, uri)

himark_host_add(engine, url)                  -> host
```

Creation is asynchronous end to end — an effect, the wire's
`createSession`, a landing — so the call returns a **ticket**, not
a session. The host polls the ticket after drains (the same moments
the epoch is read); `READY` hands back the minted `(host, uri)` and
implies the session is already in the catalog — the ticket is the
correlation ("the one I asked for"), the snapshot is the state. The
arguments mirror the internal create (`SessionClient::
create_session(working_dirs, SessionOptions { provider, config,
model })`); `config_json` is an uninterpreted passthrough to the
provider's config.

`dispose` needs no ticket: the host watches the session leave the
catalog like any other change.

`himark_host_add` is the existing add-host flow
(`AgentFlows::install_add_host`: url → `WireHost::at` → minted into
`Servers`, seeded into the catalog) given an FFI door, so a native
settings screen can register a remote agent host. The two boot
hosts (VS Code discovery, the himark daemon) keep registering at
engine create, as today.

## Turns: the composer is native

In the native shells the composer — the text field, the send
button, the model picker, the stop button — is HOST chrome; the
bridged transcript only displays. So driving a turn crosses the
boundary:

```
himark_turn_start(engine, host, uri,
                  text, model_or_null)
himark_turn_cancel(engine, host, uri)
```

`turn_start` speaks to the session's default chat (the channel
already names one; the wire road is `ChatClient::start_turn`) —
the host never holds a chat id. Consequences arrive where they
already land: the transcript bridge repaints off the feed, and the
running/elapsed state a stop button needs rides the catalog
snapshot's status and activity columns, behind the same epoch.

## Callbacks: intents surface to the host

The one layer where himark calls OUT (beyond the platform services
`himark_set_host` already carries): **a mounted view asks to
navigate — a file tapped in the changes tree, a reference followed
in a document — and in a shell that owns the frame, only the host
can answer**, because answering means pushing a screen and minting
a bridge for it. The workbench absorbs these same requests
internally when it is the shell.

```
himark_set_frontend(engine, callbacks, ctx)
  on_navigate(ctx, origin, place) -> bool
      // origin: the bridge the ask came from; place: a PLACE
      // handle — the same currency minting takes, introspectable
      // (kind, session, location) so the host can route a screen.
      // false DECLINES: the call falls through to the workbench
      // frontend, so a native shell takes over one place kind at
      // a time instead of all navigation at once.
```

What an intent carries is a **place** — the minting currency of the
next section — so the host's whole answer is: pick a screen, hand
the place back through the minting door. Intents fire on the main
thread during a dispatch or drain; the host records and returns,
answering on its own run loop — the same re-entrancy discipline as
every wake. This is where [imba-ffi.md](imba-ffi.md)'s
`PanelRequest` routing flag lands in a native-frame shell: an
UNROUTED bridge's requests arrive here as places instead of
surfacing raw on `pump`. The Rust home of this table is the
`FrontendHost` trait ([himark-core.md](../himark-core.md)) — the
app logic behind every navigation stays in Rust; only the
placement choice crosses.

## From a place to a bridge: one minting door

Minting is UNIFIED behind the navigation vocabulary that already
exists: `hikit::navigation::Navigator` is precisely "given a place,
re-mint the pane off its collection — windowless"
(`navigate(store, ui, place, fx) -> Option<DynPanelView>`), and
`NavigationLocation` is the type-erased place built to cross
boundaries. **Given a place, Himark gives you a view:**

```
himark_bridge_view(engine, window, place)  -> bridge   // 0: no
                                                       // navigator
                                                       // answers
himark_place_release(engine, place)

// well-known places, session-addressed — every session already
// owns the row of collections they name (`SessionState`):
himark_place_chat(engine, host, uri)       -> place
himark_place_filetree / _changes / _history / _comments
  / _terminal / _locations(engine, host, uri)
himark_place_document(engine, host, uri, location) -> place
```

- **A place names content, never a panel.** The workbench's panel
  namespace stays internal; what crosses the ABI is the
  session-scoped address — the currency the entities plan
  standardizes (collections reached by their own ids under any
  scope — [entities.md](../entities.md)). Roles are not an enum:
  chat, trees, changes are just well-known places.
- **Intents and minting are one vocabulary.** A place the engine
  hands out through `on_navigate` is exactly what the host hands
  back to mint — and a native navigation stack is a stack of
  places, which is also what panes already record as where they
  stand (the walk stack).
- **Documents take a location** on top of the session, because a
  document view is "this file in this session's world" — the
  session supplies authority and diff context (`SessionId::
  of_location`: a plain file belongs to the local session, never to
  nothing). Opening the document in the session's `OpenDocuments`
  and bridging its view are one act.
- **Claiming is a move**, exactly as [imba-ffi.md](imba-ffi.md)
  states it: a bridged view leaves the workbench's composition, and
  `view_bridge_destroy` runs its teardown. One view, one owner, one
  painter.
- The window argument scopes focus and theme: the view is built
  against `frame_store(window)` — the store plus that window's
  focused seat — so native focus arbitration
  (`view_bridge_set_focused`) and keyboard routing work per OS
  window.

## Windows in library mode

A library host still calls `himark_add_window` — once per OS window
— but the window is a SCOPE, not a picture: the focus domain its
bridges share, the theme seat, the viewport record. The host never
calls `himark_draw` for it, and the workbench never composes the
views the host has claimed. One wrinkle to remove on the way:
today's production engine auto-creates a session for every new
window (`compose_new_windows` fires `OpenNewSession` inside
`add_window`) — library hosts need the bare window the test engines
already get, so window minting grows a flag (or the auto-session
moves behind the whole-window surface, where it belongs).

## Rules of the road

- **One thread.** Every call above is main-thread, like the whole
  existing surface; `run_pending` stays the only worker-side call.
- **No new machinery across the boundary.** Wake, effect wake,
  `run_pending`, `drain` are untouched; the session surface adds no
  threads and no feeds — the engine remains the only subscriber of
  agent-host channels, and the host reads consequences, never
  streams.
- **Handles, not pointers.** No engine object crosses the ABI as a
  pointer: snapshots, places, bridges, events are dense ids into
  engine-owned tables, each with its dedicated release call. The
  engine validates every handle at the door — stale, foreign, or
  double-released is a checked refusal, never UB. Raw pointers
  appear only as borrowed payload within a single call: a canvas
  handed to draw, UTF-8 in, snapshot strings out (valid until that
  snapshot's release).
- **Ownership is explicit.** Snapshots and tickets are engine-made,
  host-released (tickets by being resolved or abandoned — the
  engine retires a resolved ticket on the read that saw it);
  strings are borrowed from the snapshot that returned them.

## Decisions

- **Engine-mediated, not a shell-side `ahp-wire` client.** One
  subscriber, one fold, one catalog; the views to bridge live in the
  same store. Revisit only if a consumer wants sessions with NO UI
  at all — a daemon-shaped host with no views to mint — and then
  the answer is splitting `ahp-session`'s fold out of the UI crates,
  not a second truth beside the first.

- **Epoch + snapshot, not an event stream.** Two functions and a
  counter are the whole change protocol; native list views already
  diff and animate on their own (`NSDiffableDataSource` and kin eat
  snapshots natively). A typed event queue would freeze an enum of
  change kinds into the ABI to save the host work it does anyway.

- **A ticket only where correlation is real.** Creation is the one
  call whose result the host must match to its request; everything
  else — dispose, status, host health — is catalog-watching. No
  general request/response plumbing crosses the boundary.

- **Config crosses as JSON.** The engine passes it through to the
  wire (`SessionOptions.config`) uninterpreted; provider config
  schemas evolve server-side and the ABI should not chase them.
  `resolve_session_config` can back a native settings sheet later
  through the same passthrough shape.

- **Turns cross the boundary because input is native.** An earlier
  draft deferred programmatic turns on the theory that conversation
  rides the bridged chat whole. The iOS design overruled it: the
  composer is native chrome, so send/cancel/model are host acts and
  the transcript is display-only. The surface stays thin — default
  chat, text, model — and anything richer (attachments, explicit
  chat addressing) must earn its columns later.

- **Intents and minting share one currency: the place.** The
  navigate callback names what the user meant (a document, a diff,
  a location in a session) as a place, never how to show it —
  screens, stacks, and split layouts are host vocabulary, and the
  host answers by handing the place back to the minting door. One
  callback table, main-thread, record-and-return.

- **Handles, never pointers — and handles are NEWTYPES.** Each
  handle kind crosses C as a single-field struct (`HimarkBridge`,
  `HimarkEvent`, `HimarkLayout`, `HimarkFrame`, `HimarkWindow`),
  never a bare integer: distinct types in C and Swift, so an event
  where a layout belongs is a compile error — and the one-u64
  struct's ABI is the u64 itself, so the law costs nothing. A
  foreign host WILL misuse what it holds — too early, twice, after a session died. Ids into
  engine-owned tables make every such misuse a checked refusal at
  the door instead of UB in a table the host can't see. The cost
  (one lookup per call) is noise against what any call does. The
  ONE exception, by decision: the engine itself stays a pointer —
  it is the object that OWNS the tables, its create/destroy
  ownership is unambiguous, and there is no engine-owned place a
  guard for it could live.
