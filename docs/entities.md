# Entities: collections addressable by id

Dispatch and state are fused to windows and sessions because
addressing is hand-rolled per command — `Entity(DocumentId, …)`,
`DiffViewCommand { session, … }`, `CanvasViewCommand { session, … }`,
`InSession(…)`, the document-addressed landing family — each with its
own perform arm, scope discipline, and reverse lookup
(`session_of_document` / `session_of_diff` / `session_of_watch`), and
because reads are TYPE-keyed singletons (`X::of(store)`), which is
what forces every collection into one crate that knows `Hosts`.

**The mission: replace type-based store lookup with id-based, and
retire the host/session pair as the address of anything but the
catalog itself.** Collections that are reached by id and wired to
siblings by id know nothing about `Hosts` — and can decouple into
their own crates. Lifecycle stays MANUAL: today it is one mint and
one dispose, and automation would buy nothing yet (the reachability
direction is recorded at the tail, not legislated).

## The law

1. **The entities are the COLLECTIONS, not their records.**
   `OpenDocuments`, `Diffs`, `Chats`, `Changes`, `History`,
   `Comments`, `Terminals`, `SessionTree`, `RecentLocations`,
   `LocationLists` — each a row in ONE flat table, addressed by a
   typed `Id<T>`. `DocumentId`, `EditorId`, `DiffViewId` are the
   collection's PRIVATE schema; record lifecycle inside stays the
   collection's manual, fallible business — as today.
2. **Lookup is id-based and OPTIONAL.** The table is per-kind
   persistent hash maps (rpds) keyed by a monotonic, never-reused,
   `Copy` id — no slotmap, no generations. `store.get(id) ->
   Option<&T>`; a dangling id resolves `None` and the caller handles
   or discards — the gone-document convention. `X::of(store)`
   retires as collections convert.
3. **Ids are THREADED to where lookup is used.** No ambient reads:
   the id in hand comes from your own record (wired at birth), your
   command's address, or your caller. A read with no principled id
   source is a COUPLING the old globals were hiding — surface it,
   don't smuggle a singleton back in.
4. **Siblings are wired at birth.** `Diffs` is handed
   `Id<OpenDocuments>` at the mint and never learns what a session
   is. Cross-collection links are `(Id<X>, private key)`.
5. **One command road: `At(Id<T>, T::Command)` — carried by the
   `Entity` trait.** LANDED as `AppCommand::At(Addressed)` — ONE
   type-erased variant for every collection (`Addressed` boxes the
   typed `AtCommand<T> { id, command }` behind the one-impl
   `AddressedCommand` trait), built by `AppCommand::at(id, command)`.
   The himark-side `AppEntity` trait declares what the road needs per
   collection: `label` (the reconcile trace name — the type is erased
   by the time the trace reads one) and `after_route` (the landing's
   application tail — the changes rearms, the comments card work).
   An addressed command answers NO session scope: gather ignores
   sessions (the store is single and global), and the batch tail runs
   the sync lanes over EVERY family — each lane drains its own
   pending queue, so a clean family costs map reads. The typed
   `session_of_*_id` scans retired with nothing replacing them; what
   remains of scope is the window half (its projection) and the
   session a window names, which feeds the empty-family housekeeping
   on scatter. `DiffViewCommand` / `CanvasViewCommand` stay separate
   variants for now: their performs reach their own collection
   through the store by id (e.g. `Canvas::perform` → `Changes::of`),
   which under a lease is reentrancy — folding them in is step-4
   row-threading per collection, not dispatch work. An entity
   declares its command type the way views do:

   ```rust
   trait Entity: Clone + Send + Sync + 'static {
       type Command;
       fn perform(&mut self, command: Self::Command,
                  store: &mut Store, fx: &mut Fx);
       fn destroy(&mut self, store: &mut Store);  // retract owned ids
   }
   ```

   Routing is a LEASE: take the row out, `perform` (siblings reached
   through the store it is handed), put it back — the take/put door
   the store already uses, made the one road. The lease leaves a
   MARKER in the slot: `get` on a currently-leased entity is a
   REENTRANCY BUG, distinct from gone — it PANICS in debug; in
   release it returns `None` and logs loudly, never silently.
   Closed-world enums per kind. Effects are stamped with the entity
   id they were launched at; a landing whose target is gone discards.
6. **Lifecycle is MANUAL and symmetric.** The session mint creates
   its collections and wires siblings; dispose is
   `Store::retract(id)` — remove the row, run its `destroy`, which
   RETRACTS the entities it owns. Teardown cascades by ownership
   instead of a hand-kept list; both ends live in ONE place.
7. **The table is a persistent value.** Clone is a snapshot; scatter
   a plain write-back; a background job's snapshot resolves its own
   ids safely, being a value.

(From gpui we keep the one identity mint and typed ids; we reject
record-granularity slots, refcounted lifetimes, and the notify
graph — the push road already covers it.)

## The plan

### Step 1 — The table, behavior-neutral

`Id<T>` mint + per-kind maps + `get(id)` / `retract(id)` in the
store;
`SessionState`'s fields become ids resolving into the table. The
flat type-keyed installs STAY as scaffolding for unconverted
readers; each later step shrinks them.

### Step 2 — The mint ceremony

Session creation mints the collections and hands out sibling ids;
session dispose `retract`s them, and `destroy` cascades to owned
entities. One boringly explicit function pair — the only place that
knows the whole wiring.

### Step 3 — Thread ids: documents first

Convert `OpenDocuments` readers to a threaded `Id<OpenDocuments>`;
collapse `Entity(DocumentId, …)` and the document-addressed landings
(`BaseLocated/Fetched/Built`, `Watched`, `Refetched`,
`RefetchDiffed`, `DocumentStored`) into `At`; `entity_scope` becomes
address stamping; `session_of_document` retires; drop documents'
flat install.

### Step 4 — Thread ids: the rest

`Diffs`, `Chats`, `Changes`/`History`, `Comments`, trees, terminals,
lists. `DiffViewCommand` / `CanvasViewCommand` lose their
hand-carried `session` fields; `InSession` and the remaining forest
scans retire with the last conversion.

### Step 5 — Crates

Converted collections move into their own crates — they compile
without `Hosts`, which shrinks to the catalog and the mint ceremony:
the one owner that still speaks host/session.

### Step 6 — The frame; dispatch decomposes

The batch loop's native entry is `perform_batch(addressed commands)`;
window dispatch is one producer (the frame turns events into `At`s).
Proof: the headless boot test — zero windows; designate the host,
enter a session, open documents, take feed landings, run the diff
and sync lanes, drive a chat turn, assert against the store.

### Step 7 — Shells hold ids

The workbench becomes one client of the table; a native shell is
another ([ui/ffi.md](ui/ffi.md), rewritten: bridges mint against
`(core, frame)`, holding ids + private keys — the same currency as
panes). The payoff, not the work.

## Later, maybe: reachability

When lifecycles stop being simple — shared and detached entities,
native shells holding references, cross-session moves — the recorded
direction is GC: `trace() -> Refs` at collection granularity (tiny,
hand-written), roots = `Hosts` + frames, a batch-tail sweep,
disposal-as-unlinking, the `At` router as the one fallible door with
in-perform reads infallible by position. Deferred, not rejected:
with one mint and one dispose per session, reachability automates
nothing we do.

## Non-goals and risks

- **Gather keeps the session bundle** while scaffolding lasts;
  projection minimization is nobody's goal.
- **Flattening stops at the collection.** No global schema learns
  that editors live in documents.
- Risk: scaffolding that stops shrinking — each converted collection
  MUST drop its flat install, or two read roads coexist forever.
- Risk: `Option` sloppiness — a `None` silently swallowed where
  liveness was certain hides a real bug; discard is for RACES,
  `debug_assert` where you believe the id must be live.
