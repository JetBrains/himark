# Code style

House rules for this codebase. A living list — entries get added as
they come up.

## Early returns are discouraged

A function should read as one shape: the conditions visible in the
structure, one place where control comes back together. A body riddled
with `return`s (or `match` arms that each bail) makes the reader
simulate every exit to know what actually runs — the flow lives in
their head instead of on the page. Prefer:

- `if`/`else` over `condition → return` pairs — the else branch is the
  story, tell it as one;
- expressing a classification as a value (`let mapped = match … {…}`)
  and branching on it once, over returning out of the middle of a
  `match`;
- splitting a long body into named private methods over guarding your
  way down a single long one.

A single guard at the very top of a function (`let Some(x) = … else {
return }`) is tolerable; anything past that wants restructuring.

## Persistent data structures over Vec and HashMap

State that rides the store — anything cloned per frame, per command,
or per snapshot — uses persistent structures (`rpds`:
`HashTrieMapSync`, `VectorSync`) so a clone is pointer bumps, never a
copy. `Vec` and `HashMap` on such values turn every store commit into
O(n) copying. Plain collections are fine for frame-local scratch and
value types that never outlive one call. A consume-once payload (a
worker→landing result moved by value) keeps its plain `Vec` — but
must NOT derive `Clone`: if it never needs cloning, make the type
unable to.

The discipline is BOTH sides of the wire. A server's state is the
same persistent VALUE the client's store is: readers take an O(1)
`Arc` snapshot they may hold across awaits, writers derive the next
value copy-on-write and swap it in; a `Mutex` shrinks to a swap
latch, never a region logic runs inside. Live ends (processes, ptys,
watchers, senders) ride `Arc`s inside the value; truly live OBJECTS
with their own plumbing stay beside the state, never in it.

## Long-running work is cancellable, requests are independent

- A request handler must never be awaited inline in a connection's
  read loop — one parked handler wedges every request behind it.
  Requests answer on their own tasks; ordering guarantees are
  per-request (a handler's publishes and its answer leave the same
  task, in order), and cross-request order is the client's await.
- Work that can outlive anyone's interest (a filesystem walk, a
  scan) carries a LEASH — a stored cancellation flag checked in the
  loop, raised by supersession or a dead client. A cut answer says so
  (`truncated`), never poses as complete.

## Buffers are `Text`, identifiers are typed, containment is the link

- **Document/buffer content is `text::Text`, never `String`** — on the
  backend as much as the frontend. A `String` buffer means O(n)
  rebuilds per operation and line scans over gigabyte texts; the rope
  navigates by metric seeks. `String` materialization is legal only at
  the WIRE boundary (a protocol field that is a string) and in test
  oracles. Never build whole-file index Vecs (`lines()`-style) to
  resolve positions — resolve through the rope.
- **A feature MIRRORING editable text mirrors the rope** — any record
  shadowing something a user types into holds `Text` (clones are O(1)
  rope snapshots), and change detection rides the DOCUMENT REVISION —
  never a text compare, never a per-keystroke `String`
  materialization.
- **Identifiers are newtypes, never bare `String`s** — parsed once at
  the wire boundary; in-memory code compares values, not strings.
- **No unbounded accumulation for a gate a single value can answer** —
  prefer `base == current version` over an ever-growing applied-id
  set.
- **A relation is a MAP KEY, not a field to scan for** — "which
  entity owns X" is an index, never `iter().find(…)` over a field
  stored in the value.
- **Containment IS the link** — a value owned by a session does not
  store the session's id; it lives in the session's map.

## Nothing linear on the UI thread

Per-frame and per-landing work must not scale with the data. Imagine
the value at 1M entries before writing the line:

- **Membership and dedup** are a persistent set
  (`rpds::HashTrieSetSync`) — `Vec::contains` per insert is O(n²)
  across a landing.
- **Change detection is a GENERATION** (`u64` bumped on real change),
  never a deep compare of the collection — and never a per-frame
  recompute.
- **Derived UI is BOUNDED**: a tree or list built from an unbounded
  set caps its rows and the header names the TRUE total; silent caps
  are lies, unbounded builds are stalls.
- Scans and builds go OFF-THREAD (the effect lanes); the UI thread
  installs bounded results.

## No mock hosts

There is deliberately no in-process fake of the agent host. Tests run
the REAL host (in-process over a socketpair or unix socket) and mock
the AGENT behind it (the scripted CLI — `agent_host::testing`); seat
handles that a test never calls use an inert stub local to the test.
A mock host re-implements the protocol contract badly and lets the
two drift; the real host is cheap to spin.
