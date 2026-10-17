# The render loop: two threads, one channel

Agreed 2026-10-15 (Andrey × Manuel × fable). The end state for the Apple
shells' frame pipeline, and the staged road to it. Everything here is
grounded in code as of `ea724266`; the stages land strictly in order,
each independently shippable.

## Where today's loop stands

One main thread does everything: AppKit event dispatch, engine commands
(`perform`), layout + realize (the stage road), Graphite recording,
`insertRecording` + `submit`, and the transaction-coupled present —
including a `waitUntilScheduled` stall. The display link is per
`HimarkBridgeView` (`caTick`), and the animation clock is
`CACurrentMediaTime()` at tick entry.

## The end state

**Two threads.** Not three: AppKit demands synchronous answers — key
events must bubble to the next responder when unconsumed, `bridgeEvent`
returns pointer shapes and handled flags, and `NSTextInputClient` is a
synchronous interrogation protocol (IME asks for selected range and
caret rects mid-event). A thread hop anywhere in that road turns event
handling into a black hole (the Java EDT lesson). So:

- **UI thread** (the AppKit main thread): events, hit-testing,
  commands, the store. The transactor stays merged into the event
  handlers; every synchronous AppKit answer keeps working untouched.
- **Render thread**: owns the display link, takes the latest store
  snapshot, runs layout → realize → record, plays the recording through
  the Graphite context, presents. Owns the per-thread caches (`UiCtx`,
  shape and viewport `thread_local!`s) warm and coherent, because all
  layout happens here and only here.

**The frame crosses back, not the events.** Hit-testing is an inherent
race against the user — they click on what they *saw*, which is the
last presented frame. Double-buffering makes that honest: two arenas
per slot; the render thread builds frame N in arena A, publishes it,
and takes back arena B from retired frame N−1. The UI thread hit-tests
and key-dispatches against the published frame — the one on glass —
extracts commands, and performs them right there. Commands are born and
die on the UI thread; `DynCommand` never needs `Send`; the loop closes
through the store and nowhere else. (This is `mounted_frame`'s existing
frame + `spare_arena` ping-pong with the two ends on different threads
and a channel in the middle.)

**The clock is `targetTimestamp`.** The display link hands the driver's
own estimate of when the frame being produced will reach glass; feeding
that to `engine.tick` makes animations land on the presented frame, not
the one that just left.

## What the types already guarantee

- The store is `Sync` by construction (`HashTrieMapSync` rows,
  `Arc<dyn Any + Send + Sync>` components). Snapshots cross threads.
- The view fleet is `Send + Sync` by law: `ModalView: DynView + Send +
  Sync` always demanded it, and `BridgeView: Send + Sync` (ea724266)
  closes the wrapper tier. Slots hold `Arc<dyn BridgeView>`;
  `Arc::get_mut` keeps the perform road's exclusivity law unchanged.
- Views are persistent in practice (the no-mutable-state convention);
  the bounds turn the practice into a compile error for the next
  accidental `Cell`.

What is NOT yet guaranteed: the `Frame` package carries `Rc<UiCtx>`,
and frames crossing threads must not split `Rc` clone families across
threads. The frame handoff stage replaces that edge with `Arc` or drops
the reference.

## The stages

1. **targetTimestamp clock.** Feed the display link's target timestamp
   to `engine.tick` instead of `CACurrentMediaTime()`. Tiny, strictly
   more correct, no dependencies.
2. **Graphite playback + present off main.** Recording (`snap()`)
   stays on the UI thread where the store lives; a submit thread owns
   the `Context`: `insertRecording`, `submit`, command buffer,
   `waitUntilScheduled`, present. Graphite was built for this split —
   Recorders are thread-affine, the Context is a separate affinity.
   CAVEAT (the flicker law): `presentsWithTransaction` exists because
   band layers MOVE — geometry and pixels must commit in one CA
   transaction. The off-main present is conditional: frames where the
   band layer's frame changed keep the transaction-coupled main-thread
   present; still-geometry frames (most) present async from the submit
   thread.
3. **The frame handoff.** Layout + realize move to the render thread;
   double-buffered frames cross to the UI thread for hit-testing as
   above. Gate: `Frame`'s `Rc<UiCtx>` edge, and the retained-view
   aliasing discipline (`Arc` head on the UI thread, copy-on-write via
   `Arc::make_mut` exactly when the render thread holds a snapshot).
   The entities plan (view state into the store) remains the cleanup
   that shrinks what the snapshot must carry — no longer the gate.
4. **Pacing heuristics** (optional, last): Chrome-style
   `frameStartTime = targetTimestamp − estimatedFrameTime`, delaying
   the snapshot cut to the latest reliable moment. Only worth tuning
   once stage 3 exists to pace; bad estimates cost a frame (8ms at
   120Hz), so it ships behind a flag with the estimator's error
   tracked.

## What threading does not buy

An overloaded layout thread still freezes imba's own content — threads
buy chrome responsiveness and halve the critical path; they do not make
slow frames fast. The per-frame diet (realize cost, blob paint, atlas
hygiene) stays the main performance work regardless. No silver bullet.
