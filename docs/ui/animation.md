# Animation in imba

Animation fits imba without new machinery kinds: **an animation frame is a
state transition, and state transitions are commands.** Time enters as an
event; progression is a command a view issues to itself; a settled view
issues nothing — and silence is what stops the clock.

## The scheme

### `Event::AnimationClock`

An event, dispatched by the host once per display-link tick:

```rust
Event::AnimationClock { now: AnimationClock }

/// An instant on the animation timeline. Hosts mint it from their
/// monotonic clock; views can only carry it, compare it, and measure
/// spans — there is no way to fabricate time or leak its unit.
#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub struct AnimationClock(f64);

impl AnimationClock {
    /// Host-side constructor (the shell's display-link callback).
    pub fn from_millis(now: f64) -> Self;
    /// The span since an earlier instant, in milliseconds.
    pub fn millis_since(self, earlier: AnimationClock) -> f64;
}
```

The raw `f64` never crosses the API: a view cannot confuse seconds with
milliseconds, add two instants, or invent a timestamp — the only arithmetic
the type offers is the one animations need (`now.millis_since(last)`), and
the only mint is in the host.

It travels the widget tree like any event — as a BROADCAST, not
winner-takes-all: two blinking spinners must both answer one tick, so
containers visit *all* children for the clock (the same merged fan-out
paint and `ThemeChanged` ride) and fold every returned command through
the per-child mappers they already own into one batch
(`EventResult::merge`). A widget whose view has a *running* animation
answers with a command that progresses it; a widget whose animations are
done ignores the event, and a merely-`Handled` tick counts as silent.
The host keeps the display link running while clock dispatches produce
commands and pauses it when a tick comes back silent. That is the entire
scheduling protocol:

- nothing animating → no responders → link pauses → idle UI costs zero;
- any `perform` that starts an animation → the host unpauses the link on
  commit (it already does, for redraw) → next tick finds a responder.

No subscriptions, no timers, no needs-frame flags. The command flow *is*
the schedule.

### `Animation<T>`: the animated value itself

The animation owns the value: views ask for *the current `T`*, not for a
progress scalar to map by hand — a size animation is then one field,
not a convention every call site repeats.

```rust
pub struct Animation<T: Animatable> {
    from: T::Vector,
    to: T::Vector,
    motion: Motion,            // Ease { duration_ms, easing }
    progress: f32,
    last_clock: Option<AnimationClock>,   // None when settled
}

impl<T: Animatable> Animation<T> {
    pub fn done(value: T, motion: Motion) -> Self;   // starts settled

    /// Retargets. The motion continues from the *currently displayed*
    /// value — mid-flight retargeting redirects, never teleports.
    /// Retargeting a settled animation at its own value is a no-op.
    pub fn set(&mut self, target: T);
    pub fn jump(&mut self, value: T);                // no animation

    pub fn advance(&mut self, now: AnimationClock);  // the tick command body
    pub fn running(&self) -> bool;
    pub fn value(&self) -> T;                        // compose(current vector)
}
```

Progress advances by *elapsed time*, not tick count — dropped frames and
paused clocks cannot desynchronize it. Once settled (`progress == 1`),
`running()` is false and the view stops responding to the clock.

### The algebra: what `T` must be capable of

Exactly one thing — a round trip through a small float vector:

```rust
pub trait Animatable: Copy {
    type Vector: AnimVector;
    fn decompose(self) -> Self::Vector;
    fn compose(vector: Self::Vector) -> Self;
}

pub trait AnimVector: Copy {
    fn map(self, f: impl FnMut(f32) -> f32) -> Self;
    fn zip(self, other: Self, f: impl FnMut(f32, f32) -> f32) -> Self;
    fn fold(self, init: f32, f: impl FnMut(f32, f32) -> f32) -> f32;
}

impl<const N: usize> AnimVector for [f32; N] { … }        // stable Rust
impl<A: AnimVector, B: AnimVector> AnimVector for (A, B)  // products compose
```

Everything the animation system does is `map`/`zip` over the vector:

- **lerp** — `zip(from, to, |a, b| a + (b - a) * t)` with easing applied to
  the scalar `t`;
- **settling** — `fold` for the max displacement magnitude (the
  retarget-at-same-value check).

`T` is an affine point that can name its coordinates; the *space* those
coordinates live in is the impl's choice, and that choice is where domain
knowledge goes without `Animation` knowing anything:

| type | Vector | note |
| --- | --- | --- |
| `f32` | `[f32; 1]` | the degenerate case, replaces a raw 0→1 |
| `Point`, `Size` | `[f32; 2]` | |
| `(T, U)` | `(T::Vector, U::Vector)` | tuples compose — a "size + opacity" animation is `Animation<(Size, f32)>`, one clock, one settle |

What is deliberately *not* `Animatable`: discrete types (`bool`, enums).
There is no path between `false` and `true` — animate an `Animation<f32>`
and threshold at read, or switch content at a chosen progress. Making the
compiler reject `Animation<bool>` is the algebra doing its job.

### The loop, on the inlay example

```
user clicks inlay
  └▶ DemoInlayCommand::Toggle → perform: expanded = !expanded;
      self.height.set(target_height(expanded))       — running
host commits, unpauses the display link

each tick:
  Event::AnimationClock { now }        // now: AnimationClock, host-minted
  └▶ inlay widget: self.height.running() → EventResult::Command(Tick(now))
       └▶ perform: self.height.advance(now)          — view state only
  layout/paint in the same pass reads self.height.value() → an f32, done

progress reaches 1:
  next AnimationClock finds running() == false → Ignored
  tick comes back silent → host pauses the link
```

The same pass order the host already uses (events before layout) means the
tick's command performs before the frame paints — progression is visible
the same frame, no lag.

Animation state lives wherever the animated state lives: a dock's slide
animates in its panel view; an inlay's size participates in *document
layout*, so its animation lives in the inlay view inside the document's
markup and each tick commits a new document version — which the store
machinery already handles like any other mutation (markup generation
bumps, the repair lane relaunches, snapshots stay internally consistent).
`Animation` is plain data, updated in `perform` like every other field,
no interior mutability anywhere.

## What it does *not* do

- **No periodic sources.** The clock scheme settles because easings are
  finite by construction; a never-settling responder (a caret blink)
  would answer every tick forever — the link never pauses, a hundred
  wakeups a second to produce two visual changes. Changes at *instants*
  are deferred-work territory ([effects.md](effects.md)), not animation — the
  clock carries only motion that ends.
- **No macOS scroll momentum** — the OS synthesizes momentum events;
  animating them again would double-apply. Other shells may build a fling
  on the same `Animation` + clock scheme inside `ScrollView`.
- **No implicit "animate everything"** (Compose's `animateContentSize`
  style). Only values a view explicitly runs an `Animation` for move.
  Implicitness is where immediate-mode animation grows unpredictable.

## Testing

Everything is public machinery: tests dispatch `AnimationClock` events
with hand-stepped instants (`AnimationClock::from_millis(0.0)`, then
`16.7`, …) and assert through `perform`/layout like any other test — no
injected clocks, no special harness. Two standing assertions:

- determinism: same clock sequence → same frames (progress is a pure
  function of elapsed time);
- settling: after any interaction completes, some finite number of ticks
  must reach a silent tick — the assertion that catches every
  "animation never settles" battery-drain leak.
