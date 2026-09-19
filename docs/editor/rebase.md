# The rebase loop

*The client half of `documents@1` (docs/ahp/ahp-documents.md §5): one
speculating client over a server's total order. `frontend/rebase` — no
transport, no locks, no maps, and no second model of the document.*

The shape is Fleet's (`fleet/kernel/rebase`: `RebaseLog`, `RebaseLoop`),
adapted from semantic instructions to POSITIONAL operations, which is
the one real difference: Fleet re-EXECUTES an entry against a new base;
positional operations cannot be re-executed, so an entry is TRANSFORMED
across what it owes.

## 1. The log is a value

```text
  committed ──▶ speculation… ──▶ rebasing…
  ^ the server  ^ ours, already   ^ ours, still owed the transform
    confirmed     carried onto      across what the server just said
    this          `committed`
```

One chain from the last confirmed state to what the user sees, split in
two segments. Every transition is a pure function of the log:

| event | what happens |
| --- | --- |
| **local** | one more entry on the tail. Settled, it lands in `speculation` and dispatches at once (write-ahead — the UI already shows it); mid-rebase it waits its turn. |
| **remote** | `committed` takes the operation, EVERY entry moves back to `rebasing` owing it. Two foreign operations before the rebase finishes compose into one debt. |
| **step** | one entry crosses: transformed across what it owes, applied to the tip, moved into `speculation`, dispatched afresh (a foreign operation killed every chain in flight, so pending entries always re-dispatch — §5, same ids). |
| **ack** | the head came back: it becomes `committed`, and its id becomes the version. |

An entry that crosses into nothing (a pure deletion of text the server
already deleted) leaves the log. Text somebody TYPED always survives —
the standard resolution, and the one that never loses work.

## 2. The loop is one task

```text
  local  ─┐                         ┌─▶ wire    (dispatch to the server)
          ├─▶ select ─▶ RebaseLog ──┤
  remote ─┘     ▲                   └─▶ offers  (the whole state, to the UI)
                └── a rebase step, when both channels are quiet
```

Input first, a rebase step only when there is nothing to hear (Fleet's
`whileSelect` + `onTimeout(0)`). One entry per turn, so a long queue
never holds the loop. The task owns the log outright: nothing else can
see it, so nothing has to lock it.

## 3. Offers, not reconciliation

The loop does not model what the UI shows and does not send it
incremental operations. It OFFERS the whole display state, taken when
the log is settled and the local channel is quiet — while the user
types, nothing is offered.

An offer carries `seen_local`, the number of local edits the loop has
folded in. The UI applies it only if it has sent exactly that many.
An offer that lost the race is DROPPED, never repaired: the edit that
overtook it is already on its way in, and the offer that follows it
will fit.

That is the whole reason this exists. The road it replaces kept a
hand-maintained mirror of the UI's buffer, and a mirror is a belief:
every road that moved a document without passing the seam made it a
wrong one, and a user edit resolved against a wrong mirror edits the
wrong bytes (a crash taught this: a stale mirror once resolved an
edit into a byte range that split a scalar).
There is nothing to keep in sync here, so there is nothing to drift.

## 4. Generic over the action

```rust
pub trait Action: Clone {
    type State: Clone;
    /// Apply, wherever this state has got to. Answers the action AS
    /// APPLIED — what actually happened here, which is what a peer
    /// must be told.
    fn apply(self, state: &mut Self::State) -> Self;
}
```

No `transform`, no `compose`: applying is the action's whole business.
For text the state is the TEXT AND ITS EDIT LOG, and an action carries
**the whole log it was captured against** — a persistent value, so
carrying it costs nothing.

## 5. The bridge — how an action is placed

This is Fleet's `CapturedOperation.rebase` (andel), and it is what
makes re-application work for POSITIONAL operations, which cannot be
re-executed the way a semantic instruction can:

```text
                 common base
   base log  ───────┬─────── before…      (only I did these)
                    └─────── after…       (only they did these)

   arrow = compose(before).invert() ∘ compose(after)
         = undo my divergent suffix, then do theirs
   applied = operation.transform(arrow)
```

The common base is the deepest point where the two logs hold **the
same instance**, scanned from the tails — so it costs the divergence,
never the accumulated history.

**Found by IDENTITY, never by the stable id.** Every entry carries
both: a stable id (the wire's name for the operation — how a peer
recognises its own echo, kept across a rebase) and an identity that is
fresh on every append. A stable-id scan would match a replayed entry
against its own pre-rebase self, call the logs common, and answer an
identity bridge where a real transform was needed — hiding the foreign
operation the rebase inserted beneath it. Fleet's `findCommonBase`
carries the same warning, and `rebase`'s tests carry the case:

> Type; the edit dispatches. A foreign action lands; the chain goes
> back to the queue. **Before the rebase finishes, type again** —
> against what the UI still shows, which is the superseded speculative
> state. Its stable id is still in the log, one rebase later and three
> characters to the right. The bridge finds no common instance, so it
> undoes the old speculation and redoes `[foreign, replayed]`, and the
> late edit lands where its author meant: `">> hello!?"`.

The offer's slice is the same primitive: the bridge from the log the
taker stands on to the log the offer carries — one ordinary operation
for the ordinary edit door, so every structure hanging off that
document is maintained by the edit it was always maintained by.

## 6. Wiring

The document road IS the loop (`hiahp/docsync.rs`). The module keeps
no state of its own: what a synced document needs is a `SyncSeats`
value in the store like everything else the app knows, and every
decision on the road is a pure function in `docsync/rules.rs`.

- **ensure** — the fetch road says a document is served; the main
  thread notes where the document stands (`Connecting { since }`) and
  spawns the task. Nothing else happens until the channel answers.
- **adopt** — the snapshot lands. The loop starts from the document AS
  IT STOOD when we set out (`log.as_of(since)`) plus the server's
  snapshot as one confirmed edit; every entry the reader made
  meanwhile goes down the loop's channel as its first local edits —
  each captured against the log it was made on, so they rebase over
  the snapshot and go to the server. The document is not edited here
  at all: the loop's first offer brings it to the merged state by the
  ordinary road. (What was unsaved BEFORE this open still takes the
  channel's truth — §7.) A channel that cannot be reached gives the
  seat back (`GiveUp`), so the next served read starts over.
- **the seam** — `SyncSink` (an `editor::ChangeSink`, an ADDITIVE
  installation) takes each user edit on the main thread, inside the
  perform: the operation off the document's own log
  (`compose_since`), the log it was made on (`as_of`, O(log n) — it
  drops the tail, never copies the prefix), the identity it was
  recorded under (`head`). It sends `Local::Edit`; it skips the one
  entry the seat says is ours (`applied`).
- **the stream** — the pump hands each `document/applied` to the loop
  in the WIRE's coordinates; the loop tells ours from theirs by id, and
  a foreign one resolves itself against the state it lands on
  (`SyncEdit::Theirs`, a closure the codec supplies — which is why
  `documents` never learns the protocol's vocabulary).
- **offers** — made only when the display moved for a reason the UI
  has not seen (a foreign action, a rebase), the log is whole and the
  local channel quiet. The main thread takes one iff `seen_local`
  equals what the seam has sent — derived, `revision − since − taken`,
  never counted on the way out — and answers `Local::Took`; a taken
  offer lands as `state.slice_from(document.log())` through
  `OpenDocuments::edit_shared` — a peer's edit has its own door: it is
  not a reload from disk, so the undo history is CARRIED ACROSS it
  (`UndoHistory::carry_across`, every entry transformed the way a
  caret is; the peer's edit itself is nobody's to undo here) and the
  document stays as saved or unsaved as it was — recorded under the
  loop's head identity so the two logs stay tail-aligned. An offer that lost the race to a
  keystroke is dropped, and the loop RE-MAKES it the moment that
  keystroke arrives (an outstanding, untaken offer plus a new local
  edit = owed again).
- **the codec** — `docsync/codec.rs`: the ONE place byte offsets meet
  the protocol's line/col, pure both ways.
- **the transport** — `hiahp` speaks AHP and nothing else. A seat is
  born with a `transport::Connector` the shell hands it: desktop's
  (`frontend/desktop`, unix + WebSocket) or the browser's (`apps/web`,
  the page's own socket). No socket, no FFI, no `cfg(target_os)` and
  no process-wide "the transport" in the seat crate.

An action that cannot be placed — a foreign one that does not address
the text, a replay whose arrow does not exist, one that crosses into
nothing — is NOT recorded and NOT dispatched (`Action::apply` answers
`None`): a no-op entry would sit in the log waiting for an ack.

## 7. Known limits

- **The reconnect merge.** Edits made after the open are kept (§6,
  adopt); edits UNSAVED before it still lose to the channel's truth.
  Doing better needs a three-way merge against the saved baseline.
- **Trimming.** The log grows with the session; Fleet trims and fails
  the bridge explicitly when a base is below the floor. Nothing trims
  here.
