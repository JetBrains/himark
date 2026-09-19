# Effects

Anything `perform` cannot afford — layout repair over megabytes, a
reparse, a file read, a question only the host can answer — becomes an
**effect**: a declared piece of data describing deferred work. This
document is the effect system's design story: what an effect is, how it
is launched and cancelled, how its result comes home, and how the
runner executes it. The host's half — external handlers, capability
tables, fulfill entry points — is `docs/ahp/host.md`; the View/perform side
it plugs into is `docs/ui/UI.md`.

## An effect is data

An effect captures what the work is about against persistent snapshots
(a `Text`, a `Document`, a location — clones of persistent values, so
they stay internally consistent forever). It has no behavior. It is
dispatched, by type, to the ONE handler registered for that type:

```rust
pub trait Effect: Send + Sync + 'static {
    type Result;
}

pub trait EffectHandler<E: Effect>: Send + Sync + 'static {
    async fn handle(&self, effect: E) -> E::Result;
}
```

Handlers are registered at construction (`Application::register_handler`),
each holding its own state explicitly. The in-process ones — repairs,
reparses, diff pairing, document opens — share a `Workshop`: the lazily
materialized font collection and the live theme. Their `handle` bodies
are plain compute; the `async` is not decoration, it is the seam the
external world plugs into (see `docs/ahp/host.md`).

An effect travels as `AnyEffect<R>`, the erased transport: typed rims,
erased middle. The payload stays the concrete effect value end to end
(the one downcast happens under the dispatch registry's `TypeId`
guarantee), inspectable anywhere by type (`effect.is::<RepairEffect>()`);
the result side carries a composable lift that turns the typed `Result`
into a command in the root's own type.

## Launching: the builder, the batch, the token

`perform` does not return effects — it receives an **effects builder**
(`imba::effect::Effects<'_, Command>`) and pushes into it:

```rust
fn perform(&mut self, store, ui, command, fx: &mut Effects<'_, Self::Command>) {
    let token = fx.launch(RepairEffect { ... });   // -> CancellationToken
    fx.cancel(older_token);                        // supersede in-flight work
}
```

The builder is a thin writer over a **`Batch`** — the transaction record
the app root owns. One `perform_batch` builds ONE batch: every command
performed that frame pushes launches and cancels into it, in program
order; the store commits; THEN the batch's messages go to the runner. A
discarded batch is a discarded transaction — none of its launches or
cancels ever leave the UI thread.

One message never reaches a handler: `Effects::settle` pushes the
engine-unwrapped `Settle` request, and `Batch::take_settle` strips it at
commit — the engine answers it with a synchronous `Event::Settle` pulse
at the end of the batch, before the next frame paints
([viewport-preservation.md](../editor/viewport-preservation.md)). It rides the batch because a settle
request is transactional like everything else a perform declares; it is
not deferred work, so it never queues.

Containers never touch effect routing by hand: a parent maps a child's
commands with a **scope**, exactly as widgets wrap child commands —

```rust
fx.scope(SplitCommand::First, |fx| self.first.perform(store, ui, command, fx))
```

— and every launch inside lifts to the root command type with its token
untouched. `scope_filtered` additionally drops launches the parent
refuses (the diff pane uses it to intercept its halves' repair lane).
`DynView` erases a view's command type for heterogeneous containers; its
blanket impl is one boxing scope.

Every launch mints a **`CancellationToken`** — an inert `Copy` id, safe
to store in views and entities that the store clones freely. The token
does nothing by itself; the ONLY way to act on it is `fx.cancel(token)`
through the builder, which keeps cancellation inside the same
transactional commit as everything else.

## Every launch lands; lanes supersede

The builder's guarantee: a launched effect runs and its result lands,
in order — nothing is dropped, merged or reordered behind the
launcher's back, even when two effects race over the same ground.
Ask-answer effects (a fetch, a listing, an open, a picked file) are
fire-and-forget on that guarantee: every asker is owed its answer,
including one already outrun by a newer ask.

When newer work genuinely supersedes older work — a keystroke's reparse
racing the previous keystroke's — the OWNER of that lane says so
explicitly: it stores the in-flight token and relaunches through the one
idiom,

```rust
fx.relaunch(&mut self.reparse_token, ReparseEffect { ... });
```

which cancels the stored token (if any), launches the new effect, and
stores its token. The supersession travels as ONE message
(`Message::Relaunch`), and that atomicity buys fairness: when the
superseded effect is still queued, the fresh one takes its PLACE IN
LINE instead of the back of the queue. A cancel+launch pair would reset
the lane's queue position on every supersession — a lane relaunching
under churn (a diff pane's paired repair fires on nearly every landing)
would starve behind work that arrived after it whenever the queue is
busy — visible as diff folds materializing only seconds after open. The
lanes in the tree: repair, reparse and enrichment (per document),
pair-repair and diff-optimize (per diff), save (per document entity),
search scan/find, the peeker's find, speed-search, completion, hover,
the outline, scroll stripes. (`fx.relaunch_erased` is the same idiom
for a pre-erased/mapped `AnyEffect` — `push`'s twin.)

Cancellation is **best-effort by contract**: it prevents a queued effect
from starting and a parked one from resuming (its future drops, its host
slot cleans up), but it never claws back a result already dispatched. A
cancel racing a completion means the result lands anyway — which is why
the landing-side staleness guards (revision checks, `DocumentToken`,
serials, `pair_seq`) remain the correctness layer. Cancellation is
throughput and delivery hygiene; guards are truth.

## The runner

The queue drains on ONE worker thread the host supplies (himark spawns
nothing). The channel from the UI thread is single-producer FIFO of
launches and cancels in committed program order, so a cancel always
trails its launch: the runner deletes the queued entry (or drops the
parked future) the cancel names, and a cancel matching nothing means the
work already completed — a no-op. A relaunch is the one exception to
strict FIFO: if its predecessor is still queued, the fresh payload
replaces it in place (keeping the slot); a predecessor already parked
or completed degrades it to plain cancel + tail launch.

The runner drives handle-futures with a poll set:

- a **compute handler**'s future completes on its first poll — the work
  runs to completion right there on the worker, exactly the synchronous
  work it always was;
- an **external handler**'s future parks: it has sent a request out and
  awaits a completion some other thread will deliver. A parked handle
  never blocks the queue — the next effect dispatches past it, repairs
  keep flowing while a file picker sits open. Cancelling a parked task
  drops its future, and the drop cleans up its host request slot.

Either way the result comes home the same way: the lift chain turns the
typed result into an app command, the dispatcher posts it, the wake
fires, and the main thread applies it like any event — perform →
publish → launch again. **There is one delivery mechanism.** A repair
landing and a picked file landing are indistinguishable to everything
downstream.

On wasm without threads the same dispatch runs inline between the commit
and the next frame, cancel-wins within each commit — the deterministic
equivalent of the native slow-worker outcome.

## Effects calling effects

A handler can also call a declared effect ITSELF and await the typed
result inline — the way one asks the host for something mid-effect. The
handle is `imba::effect::EffectCaller`, construction-time state like any
handler dependency (`Application::effect_caller()` mints one; the handler
that needs it takes it at registration, exactly as it takes a Workshop):

```rust
match self.caller.call(FilePickerEffect { window }).await {
    Some(files) => ...,   // the registered handler's answer
    None => ...,          // nobody registered — the host lacks it
}
```

The contract differs from launching on purpose:

- the result returns to the **awaiter** — it never lands as a command;
- the call bypasses the launch queue — you asked, you await; no stored
  token, nothing another perform could cancel;
- `None` is the capability answer, not an error: hosts register external
  handlers only for what they bring, so an absent picker answers `None`
  here exactly as its palette command never exists.

No new machinery runs underneath: the child future is awaited inside
the parent's `handle`, so it sees the parent task's waker — an external
fulfill wakes the parent through the ordinary wake, and the queue keeps
draining past the parked parent meanwhile. (On a host that executes
effects inline — the no-atomics web build — a parked child would block
the calling thread; that build registers no external capabilities, so
such calls answer `None` before parking.)

## What this buys

Every deferred thing — a repair, a reparse, a picked file, a spawned
shell, tomorrow a fetch or a language server — is one shape: an effect
dispatched to its handler, launched under a cancellation token, landing
as a command. Tests register fakes and drive the same machinery; the
wasm build runs the same dispatch inline; hosts differ only in which
capabilities they register (`docs/ahp/host.md`).
