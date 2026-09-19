# Viewport preservation

When content changes ABOVE what the user is looking at, the viewport
must not move — visually. A bare scroll offset cannot promise that:
it is a dumb pixel count, so anything that changes the height of
content above it (a repair landing, a concurrent edit, a splice
growing a tree, a row re-measuring) silently re-aims the viewport at
different content.

Preservation is the fix: viewport tops observed continuously by the
widgets that already know their viewports, corrections re-aimed
through the EXISTING reveal machinery — extended with an exact
top-left placement and an engine "settle pulse" that runs the reveal
loop synchronously between the perform batch and the paint. No frame
ever paints the un-shifted viewport.

## 1. The problem, concretely

- **Editor**: `EditorPane = ScrollView<EditorIdView>`
  (workbench_node.rs). The vertical offset is the ScrollView's
  `scroll_y`; the document's layout is a rope measured in
  `VERTICAL_PX` (document_layout.rs). A repair (`repair_layout`), a
  concurrent edit from another pane, a fold swap, or an inlay/spacer
  change above `scroll_y` changes the pixel prefix — the same
  `scroll_y` now points at different bytes.
- **ListView**: rows live in a rope measured in `ROW_PX` (list.rs). A
  `splice_slice` above the viewport, or a `SetHeight` self-report on
  an off-screen row, shifts every offset below it.
- **Composition** — the real challenge. The chat transcript is
  `ScrollView<ListView<ChatRow>>` where rows hold markdown editors and
  diff views; editors host inline panes (unified_diff.rs
  `InlinePane`) and before-cards; the diff canvas is
  `ScrollView<ListView<CanvasRow>>` whose rows land whole diffs at
  once. One shared `scroll_y`, arbitrary nesting. When a diff cell
  three rows above the viewport streams in another hunk, *and* the row
  under the viewport's top edge is itself an editor whose own upper
  half just got a repair — who preserves what?

## 2. The raw material

Almost everything preservation rides exists for other reasons:

- `EventResult::Reveal` (event.rs) **traverses the hierarchy upward
  with coordinate translation at every level**: widgets hoist it via
  `reveal_translated(dx, dy)`, and `ScrollView::reveal_intercepted`
  (scroll.rs) either satisfies it (`JumpTo`/`GlideTo`) or re-emits the
  rect *lifted into its own parent's coordinates* so an outer scroll
  can finish the job. Nested reveal composition is a solved problem —
  for the upward direction.
- The default placement policy: `reveal_scroll_target` puts the rect
  at the golden section if it isn't visible, and does nothing if it
  is. Preservation needs a second vocabulary — "put the top-left
  EXACTLY here" — which is `Placement::TopLeftAt` (§3.4).
- Both content ropes answer the two queries an anchor needs in
  O(log n):
  - list: `cursor_at_y(y)`, `row_range(key)`, `height_at(index)`
    (list.rs);
  - editor: `byte_at_y(y)` and `height_before(byte)`
    (document_layout.rs).
- Effects are async-only (`Message::{Launch,Cancel,Relaunch}`) — but
  the engine owns the batch before launching it, so a designated
  payload can be STRIPPED there (`Batch::take_settle`) instead of
  dispatched to a handler: an effect as a same-frame boolean to the
  engine.
- `dispatch_event` lays out and realizes a **fresh widget tree for
  every event** (app.rs). A pulse dispatched right after a perform
  batch therefore naturally sees the post-mutation content under the
  pre-correction `scroll_y` — exactly the state an anchor comparison
  needs.

## 3. Design: preservation rides the reveal road

The viewport-preservation loop is the SAME loop reveals already run —
retain state, emit on a pulse, bubble with translation, let the
nearest ScrollView convert to a scroll command. It adds exactly two
things:

1. a `Reveal` that says "put the viewport's top-left EXACTLY here"
   (the older vocabulary was only "make this rect visible, golden
   section");
2. a way to tell the engine "run the pulse NOW, before this frame
   paints" instead of waiting for the next `AnimationClock`.

Everything else — nested translation, interception, the lift into
outer scrolls, the command routing back into `perform` — is reused
verbatim.

### 3.1 The retained top: observed, never pushed

Every traversal already delivers each widget its honest clipped
viewport; a widget whose view anchors compares `viewport.top` against
the retained copy and answers with an ordinary command when they
drift — `ListCommand::ViewportTop(top)`,
`EditorCommand::ViewportTop(top)` — whose perform refreshes the copy
and drops any pending correction (the observed move supersedes the
door). No back channel hands the scroll's state to the view; the
command road carries it, so content a `ScrollView` never touches
directly (editors nested in list rows) hears about its viewport the
same way everything else does.

Exactness comes from WHEN the observation runs, not how fast the
report lands: a scroll-mutating perform arm raises the settle bit
(`fx.settle()`), and the pulse runs at the END of that batch — so the
re-observation postdates the mutation, and a door in any LATER batch
reads a top no older than the last move. Paint reports the same drift
as a belt for programmatic `set_scroll_y` placements (which raise no
bit; they are followed by frames, not doors). The one residual,
accepted and documented: a scroll command and a door chunked into the
SAME batch vec still read the pre-batch top — the pulse cannot run
mid-batch.

The anchor itself never persists. A height-mutating door — running in
`perform`, with `&mut self` and the retained top — captures it
transiently against the PRE-mutation rope ("row keyed K, dy into it" /
"byte B, dy into its line"), resolves it against the POST-mutation
rope, and stores ONE plain field when they differ:

```rust
settle_to: Option<f32>   // where the viewport's corner must re-aim
```

`settle_to` is absolute, not a delta — so repeated pulses converge at
the scroll (`target == effective` → done) instead of compounding. It
is set by doors in perform, READ (never written) by the settle pulse,
and cleared by the next `ViewportTop` — the landed correction itself,
or a user scroll, which supersedes it. Mutation stays in perform, full
stop.

### 3.2 The settle pulse: Effects carry the "now" bit

A door that noted a correction does one extra thing: pushes a
**settle effect** through its ordinary `fx` — a request the engine
UNWRAPS instead of dispatching. `Batch::take_settle` strips it before
launch and sets a bit on the app (`Application.settle_requested`);
scoping is irrelevant because the effect's command type never
materializes, so any view at any depth can raise it (`fx.settle()`).

At the END of the bit-raising perform batch — not deferred to paint;
end-of-batch is what makes scroll re-observation land before any
later batch's door — the engine runs the pulse synchronously:

```
loop (bounded, 3 rounds):
    layout + realize a fresh tree        // post-mutation state,
                                         // pre-correction scroll_y
    dispatch Event::Settle
    collect the resulting commands       // the JumpTo(exact) from
                                         // reveal interception
    perform them                         // scroll_y mutates HERE
    if the bit was not re-raised: break
paint                                    // first frame out is correct
```

This is cheap and structurally free: `dispatch_event` ALREADY lays
out and realizes a fresh widget tree for every event it dispatches —
the pulse is one more `dispatch_event` call with a dedicated event,
not a new machine. A dedicated `Event::Settle` (rather than a
synthetic `AnimationClock`) keeps glides and animations from
double-stepping. A guard keeps the pulse's own performs from
recursing into another pulse.

On the pulse, a widget first re-checks its viewport: a drifted
retained top means the scroll moved since the last observation, and
the widget answers `ViewportTop` INSTEAD of revealing — no correction
rides a stale top; the next round, if any, speaks from fresh state.
Otherwise, a widget whose view carries a pending `settle_to` emits

```rust
EventResult::Reveal(Reveal {
    rect:      Rect at settle_to,
    placement: TopLeftAt,   // the corner goes EXACTLY here
    motion:    Jump,        // no glide, no golden section
})
```

and the existing road does the rest: ancestors translate the rect
upward through their NEW offsets — which is what makes composition
free: if the chat list also spliced rows above, the editor-in-a-row's
rect gets translated by the row's new offset, so both shifts compose
into one rect by construction. The nearest `ScrollView` intercepts,
emits the exact `JumpTo`, the settle loop performs it, done. Zero
wrong frames; the "atomicity" is just the loop sitting between
perform and paint.

Cross-pane mutations fall out for free. Pane A edits a shared
`Document`; the edit door runs for EVERY editor of that document,
noting each one's own `settle_to` from each one's own retained
viewport; the pulse is dispatched to the whole window tree and each
pane's widget answers into its own scroll. Nobody had to know who is
watching the document.

### 3.3 Composition: who speaks

One standing rule: **the deepest widget that contained the viewport's
top-left speaks; ancestors defer.** Mechanically this is the merge
order that already exists — the list routes the pulse to its visible
rows first and only appends its own reveal when the merged child
result carries none (the exact shape of its selection-reveal merge).
So in `ScrollView<ListView<ChatRow(editor)>>`:

- top-left inside an editor cell → the editor emits (byte-precise,
  survives intra-row shifts), the list stays silent, translation
  handles the list-level shifts;
- top-left on a plain row → the list emits (key-precise);
- neither has a pending note → nothing is emitted, the viewport
  keeps plain pixels — plain-pixel behavior per level, which keeps
  adoption incremental.

`merge` keeps the first reveal, so a stray second anchor deeper in
the tree cannot hijack the frame.

Worked example — rows E0..E3 hold editors; E1 straddles the scroll
top (`dy` into it, byte B under the corner); E0 is above the viewport
and grows by Δ:

1. traversals keep the list's retained top current; E1's editor
   retains its own viewport report (only E1's child viewport has
   local top > 0 — the boundary is inside it; E2/E3 sit at 0; E0
   isn't even realized).
2. the mutating door captures `{K1, dy}` against the old rope,
   resolves against the new one, stores `settle_to = offset′(K1)+dy`,
   raises the settle bit. E1's editor's own prefix never moved — its
   door notes nothing.
3. pulse, fresh tree, old `scroll_y`: E1's editor has no pending
   note → silent. The list emits `TopLeftAt(settle_to)`.
4. interception: `JumpTo(scroll_y + Δ)`, performed in the loop,
   painted correct.

If a repair ALSO lands inside E1 above byte B (internal shift δ),
E1's editor's door notes its own `settle_to` at local `dy + δ`; on
the pulse the editor emits, the list defers, and the rect is
translated by the row's NEW offset while bubbling — Δ and δ compose
into one rect through ordinary translation. The mutation site never
chooses the anchor; the retained viewport geometry does.

### 3.4 Parameterized Reveal

`Reveal` carries options; the rect-only form keeps the golden-section
semantics:

```rust
pub struct Reveal {
    pub rect: Rect,
    pub placement: Placement,   // EnsureVisible (default, golden section)
                                // | TopLeftAt   (rect.origin becomes the viewport's top-left, exactly)
    pub motion: Motion,         // Auto (jump near / glide far)
                                // | Jump (unconditional, for preservation)
}
pub enum EventResult<C> { ..., Reveal(Reveal) }
```

`reveal_intercepted` with `TopLeftAt` becomes `JumpTo(rect.top)`
(clamped to `[0, max_scroll]`), skipping the visibility check; a
satisfied `TopLeftAt` answers `Handled` rather than lifting — an
exact placement is per-scroll business and must not leak into an
outer scroll's anchor. `TopLeftAt` is independently useful outside
preservation: the diff canvas's keyed reveal
(`ListView::reveal_row(key, Placement::TopLeftAt)` — selection-free,
with a tail clamp on both the list and scroll sides so a bottom row's
reveal does not re-emit forever), restoring a session's viewport, "go
back" navigation. The nested-scroll lift keeps working unchanged for
`EnsureVisible`, since a lifted rect is still just a rect in the
parent's coordinates.

## 4. Policy: what wins

Preservation is the DEFAULT, not the only actor. Precedence, top down:

1. **Explicit reveals** (caret typing, `RevealAt`, search navigation):
   a pending reveal expresses intent to MOVE; it beats anchoring.
2. **Tail-follow** (chat's `near_tail`): a viewport pinned to the tail
   stays pinned; appends above are the tail moving, not a disturbance.
   The pin is the anchor ("the end"), not a pixel point.
3. **The anchor** — everything else.
4. **Fallbacks** when the anchored thing died: resolve to the nearest
   surviving neighbor (list: the row that took the dead row's index;
   editor: the mapped byte clamps into the edit's replacement), else
   keep plain pixels, always clamped to `[0, max_scroll]`.

A user scroll supersedes a pending correction outright: the
`ViewportTop` its move produces clears `settle_to` before any pulse
can act on it.

## 5. The doors (where heights change)

Every mutation that can change content height raises the settle bit.
The inventory:

**ListView** — all funnel through `splice_impl` / `set_heights`
(list.rs): `splice`, `splice_slice[_animated]`,
`ListCommand::SetHeight`. Two spots, complete coverage. Splice
animations track the *target* heights, with the animation running
relative to the preserved anchor — an animated splice above the
viewport must not wiggle the view at all (targets, not frames).

**Editor / Document** — more doors, each already a discrete method:
`edit` (typing, concurrent ops), landed async repairs
(`apply_repair_anchored` — the prefix above the sync window arrives
here), `resize` (width changes: the sync rewrap around the anchor),
`retheme_editor` (font-size/theme rewraps), fold swap
(`take_swap_fold` consumers), inlay/spacer installation (before-cards
expanding, markup with `spacer_above`).

A silent door is the failure mode — it reintroduces the jump the
mechanism exists to kill — so the inventory doubles as the test
checklist.

## 6. Pinned invariants

Deterministic, no screenshots needed — anchors are numbers:

- list: splice N rows above the viewport → `scroll_y` grows by exactly
  their measured height; splice below → unchanged; splice AT the
  anchor row → nearest-neighbor fallback.
- list-in-scroll with editor rows (the chat harness): stream a diff
  hunk into a cell above the viewport → the visible cell's screen-y is
  bit-identical before/after the frame.
- editor: repair above the top byte → top byte unchanged, `scroll_y`
  shifted by the repair's height delta; concurrent edit spanning the
  anchor byte → clamps to the replacement's start.
- reveal: `TopLeftAt` lands exact including clamps at both ends;
  nested scrolls do not inherit a satisfied exact placement.
- policy: pending caret reveal beats anchor; `near_tail` beats anchor.
