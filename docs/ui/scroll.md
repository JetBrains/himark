# Scroll-to-point (reveal)

A view deep in the tree — an editor whose caret moved, a list or tree
([list-view.md](list-view.md)) whose selection moved — wants a
**rectangle** of itself visible on screen, through every scroll view
above it. Modeled as an **animation** ([animation.md](animation.md)):
the wanting view arms a reveal, the animation clock drives it, scrolls
answer by moving (instantly or eased), and visibility is what settles
it. (Keeping the viewport still while content moves is the DUAL
problem — [viewport-preservation.md](../editor/viewport-preservation.md).)

## 1. The `Reveal` result

An `EventResult` variant — a *geometric* answer, beside commands:

```rust
EventResult::Reveal { rect: Rect }   // in the answering widget's coordinates
```

It rides UPWARD, the mirror of `Event::translated` going down:

- a **placed child** wrapper shifts the rect by its placement offset —
  each hop re-expresses it in the parent's space;
- every command-`map` boundary passes it through untouched (it is not a
  command, so it crosses heterogeneous command types for free — no enum
  in the app grows a variant);
- a **scroll widget** intercepts it (§3);
- anything else lets it bubble; at the root it dissolves harmlessly.

## 2. Arming: a reveal is animation state

The view that moved its caret/selection sets plain state in `perform`,
where every animation lives:

```rust
reveal: Option<RevealRequest>   // { since: Option<AnimationClock> }
```

Arming unpauses the display link like any animation start. On each
`Event::AnimationClock` tick the view's widget — which, unlike
`perform`, holds real geometry — resolves the rect (caret rect, selected
row rect) and checks it against its own `viewport`:

- **not visible** → answer `Reveal { rect }`; the request stays armed;
- **visible** → answer the view's own *done* command, which clears the
  request; the next silent tick pauses the clock.

No clock or deadline lives in the request — it is a plain armed flag. A
rect no ancestor can serve simply **parks**: the root counts a bare
`Reveal` as silence, the clock pauses, and the next state change that
re-ticks (a repair landing, a keystroke) re-evaluates it. That re-tick
IS the convergence driver: background layout repairs move the target
between frames and the loop converges on whatever the geometry settles
to. A manual scroll cancels an armed reveal outright — the reveal never
fights the wheel.

## 3. What a scroll does with it

The scroll widget receives the rect in content coordinates and computes
the scroll offset that shows it, **top-left corner first**: align the
top edge when the rect is taller than the viewport, the bottom edge when
it sticks out below, nothing when already inside. (Scrolls are vertical
today; the protocol carries a full rect so horizontal composes later
without change.)

Then it moves — this is where "with animation" is an *option*:

- **short hops** (within ~half a viewport) **jump instantly**: the caret
  walking off the viewport edge tracks 1:1, and a new nearby target
  finishes any glide in flight on the spot;
- **long jumps ease**, time-scaled (`1 - exp(-dt/τ)` per tick, settled
  in ~75 ms of wall-clock at any frame rate), and never from afar — the
  first step lands within one viewport of the target so only the last
  stretch animates.

Either way the scroll **consumes** the reveal and **re-emits** it: the
rect mapped into its own frame and clamped to its viewport bubbles on,
so an outer scroll reveals the inner scroll's window in turn. Nesting
composes with no coordination — each level solves one hop.

## 4. Adopters

- **Editor caret**: caret-moving commands (movement, typing,
  select-next-occurrence) arm a reveal of the primary caret's rect —
  the editor pane's scroll, and any scroll above it, follow.
- **`RowList`** (peeker, palette): selection arms a reveal of the
  selected row — no private `ensure_visible`/`set_scroll_y` scheme,
  the shared protocol carries it.
- **Trees** (hifiles) and other panels: anything that can point at a
  rect of itself joins for free.

## 5. Non-goals

No scroll-position ownership moves: scrolls keep their state; the
protocol only *asks*. No horizontal scrolling is added. No "scroll into
view" API call exists — there is nothing to call; state + the clock is
the whole interface.

## 6. Scroll capture

The wheel/trackpad rule, exactly one sentence long: **the scroll
surface under the cursor at gesture start owns the whole gesture.**
Everything else is a corollary — the rule exists because the
alternatives were observed failing: an exhausted pane handing its
deltas to whichever surface happened to be next, and nested surfaces
stealing gestures mid-flight:

- **A gesture is a stream, not an event.** Hosts deliver no phase, so
  the dispatch root (`HimarkEngine::scroll_at_time`) infers it: scroll
  events closer together than 250ms continue one gesture; a pause
  begins a fresh one. The gesture rides every `Event::Scroll` as a
  capture cell (`imba::event::ScrollGesture`).
- **Innermost claims first.** A surface DESCENDS into its content
  before considering the gesture its own — a nested scroll, a pannable
  editor under the cursor outranks every ancestor. Claiming is gated
  by SERVICE: a y-surface only claims a y-dominant gesture, fitted
  content never claims, an x-panning editor only claims an x-dominant
  one — so a diagonal chat scroll over a code cell belongs to the
  chat.
- **The owner eats.** Once claimed, every event of the gesture is the
  owner's, whatever its axis mix and wherever the cursor drifted: at
  the extent's edge the delta dies (`Handled`, never `Ignored`) —
  no surface EVER inherits a gesture mid-flight. A new gesture
  re-resolves from scratch.

Surfaces in the protocol: `imba::ScrollView` (every list and pane
rides it), the terminal's scrollback, the editor's horizontal pan
(`ScrollSurfaceId::keyed` off the editor id — the retained state IS
the surface; widgets are per-frame). Pinned by imba's
`an_exhausted_owner_eats_the_gesture` /
`a_claiming_content_owns_the_whole_gesture` / `a_fitted_view_never_claims`.
One measured consequence: "scroll until dispatch says unhandled"
cannot detect the bottom (the owner eats) — the demo benchmark
derives its step count from content height instead, which also makes
every pass render the identical frame sequence.

## 7. The golden section

A reveal that must move never stops at barely visible: the target
places the revealed rect's top at the GOLDEN SECTION of the scroll
viewport — ≈38.2% down, the eye line — clamped to the scrollable
range (`reveal_scroll_target`). A rect already inside moves nothing,
so visible carets never jitter; the placement applies exactly when a
scroll happens at all. One consequence to know: a caret walking off
the bottom edge recenters to the eye line (a ~0.38-viewport hop,
still inside the instant-jump threshold) instead of hugging the edge
line by line — less total scrolling, periodic rather than continuous
movement.
