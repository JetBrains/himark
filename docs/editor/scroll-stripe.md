# Scroll-bar stripes

The error-stripe track: a thin column beside the vertical scrollbar
mapping the WHOLE document onto the pane's height — the session's
CHANGES first of all, then find-bar hits, tomorrow diagnostics — so a
changed place five thousand lines away is one glance and one click
away. The document in miniature, one mark per interesting range, and
the primary reason it exists: navigate straight to what changed.

The substance already exists: every candidate mark is an interval in
some feature markup (docs/editor/markup.md), and the vertical geometry is the
editor's `DocumentLayout` rope, whose elements carry heights. What the
track adds is the PROJECTION — markup bytes → track pixels — and a
home for its result: a background pass in the house shape — snapshot
in, effect on a lane, command out, strict last-wins. The machinery
lives in `frontend/editor/src/scroll_stripe.rs`; the sweep in
`frontend/documents/src/scroll_stripes.rs`.

The load-bearing decisions, up front:

1. **Whether an entry contributes is per-EDITOR registration, not a
   markup property.** Each editor's stripe slot holds the set of
   `MarkupId`s its track projects — `Document::mark_scroll_stripes`
   seats an entry, `unmark_scroll_stripes` unseats it. `Markup` itself
   carries NO flag: an entry does not care how it is consumed, and
   registration follows the display rule's per-editor grain — a find
   bar's tints stripe only the pane that shows them. Most markups
   never register, and the big ones must not: the SYNTAX markup is
   50k tokens of coloring that mean nothing at track scale. The
   registered set is the pruning gate that keeps the derivation
   linear in the INTERESTING intervals, not in the document's markup.
   WITHIN a registered entry, the theme decides per style: a style
   definition carries an optional `stripe` color (the per-style
   policy recipe — theme JSONs, 2x-tuned, no constant in code); a
   styled interval whose style has no stripe color contributes
   nothing. And the converse is legal on purpose: a style whose ONLY
   policy is its stripe color paints nothing in the text — which is
   what lets the diff markup ride every pane without tinting
   anyone's text.
2. **The diff contributes AS A MARKUP — the one diff markup.** No
   operation-derived side channel, no second segment source: there is
   ONE DIFF MARKUP — `Diff.markup` (docs/editor/diff-stripes.md) — holding
   whole-document hunk intervals, and the gutter, the pane's line
   washes and the track all read it. The stripe pass projects it
   exactly like any other registered entry (§6).
3. **The derivation is one LINEAR background pass over (registered
   markups × the editor's `DocumentLayout` rope), and its output is a
   bounded SEGMENTS LIST** — never a markup. The intervals' boundary
   events sort worker-side and resolve to y in one rope co-walk; the
   result quantizes onto a fixed grid (adjacent same-style runs
   merge), so the landed value and the per-frame paint are bounded by
   the GRID, not by the match count — 20k find hits land as at most a
   grid-full of segments. Linear work runs on the worker; the UI
   thread only ever swaps the finished value (the standing rule).
4. **It runs as an effect on a per-editor lane and lands as a
   command.** `ScrollStripeEffect` captures O(1) persistent clones
   (the registered entries, the layout rope, a serial), the handler
   derives off-thread, the result (`StripeOutcome`) comes home as
   `EditorCommand::ApplyScrollStripes` through the entity road
   (`AppCommand::Entity(DocumentId, …)` — the enrichment landing's
   address). Guards: document token, editor still present, stale
   serial (strict last-wins, the enrichment rule). Supersession is
   `fx.relaunch` on the stored token; cancellation is hygiene, guards
   are truth (docs/ui/effects.md).
5. **The result is a plain immutable VALUE on the `Editor`.** The
   stripe slot's `landed: Option<Arc<ScrollStripes>>` — an `Arc`'d
   segment list plus the content height it was derived against,
   swapped wholesale inside the landing's perform. No mutable shared
   state, no `Mutex`, no interior mutability anywhere: the value
   rides document clones like every editor field, the panel widget is
   a per-frame artifact reading it, and staleness is honest — between
   landings the track is at most one in-flight window behind, the
   colors-one-parse-behind contract at a scale where a line-height of
   drift is sub-pixel.
6. **This is NOT an enrichment pass.** It shares enrichment's idioms —
   the lane, the serial guard, the snapshot capture, the entity
   landing — and none of its contract (§4, "why standalone").
7. **Rendering rides the overlay system; `ScrollView` is untouched.**
   The stripe panel travels the sticky-lines road verbatim
   (`editor::sticky`, the proven recipe): the pane node declares a
   dedicated host wrapping the vertical scroll
   (`.overlay_host(editor::scroll_stripe::HOST)` beside
   `editor::sticky::HOST` in `workbench_node`), and the pane's
   gathered realize mints the request. Bubbling does the rest: the
   scroll's realize offsets the anchor on the way up, so the placed
   panel sits in pane-viewport coordinates with the CURRENT scroll
   applied at realize time — pinned to the track, no per-frame lag,
   and not one line of `ScrollView` changes.

## 1. Registration and the trigger

```rust
// Per editor, inside the Editor's stripe slot:
enabled: bool,                    // pane editors opt in (§3)
markups: Vec<MarkupId>,           // the entries THIS track projects
```

- `Document::mark_scroll_stripes(editor, id)` seats an entry on one
  editor's track; `mark_scroll_stripes_on_enabled(id)` seats it on
  every enabled track (the diff registration door);
  `unmark_scroll_stripes(id)` clears it everywhere.
- `Document` maintains `scroll_stripe_generation: u64`, bumped by
  every registered-entry lifecycle event: a seat, a registered
  entry's `replace_markup`/`remove_markup`, a pick joining or
  leaving. Change detection is a generation, never a compare
  (docs/code-style.md); this is the trigger's whole input from the
  markup side.
- **The document's `diffs` map is deliberately NOT enumerated** —
  only registered entries project. A split-diff panel's own tracked
  diff over the same document must never leak onto an ordinary
  pane's track; the stripes-role diff reaches the track only through
  the documents layer's two doors — `track_diff(stripes: true)`
  seats its markup on every enabled track, and the pane-mount
  `documents::scroll_stripes::enable_scroll_stripes` seats the
  standing one on a fresh editor. `untrack_stripes` unseats it even
  while a panel's ref keeps the diff itself alive
  (`scroll_stripes_follow_the_diff_through_the_app` pins the road at
  app level).

## 2. The value

```rust
pub struct ScrollStripes {
    /// Ascending by y, quantized and merged (decision 3). Bounded by
    /// the grid — never by the match count.
    pub segments: Vec<StripeSegment>,
    /// The layout height the segment ys address — the paint pass's
    /// denominator. Live height may have drifted (repairs landed);
    /// the drift is staleness the next landing heals, and at track
    /// scale it is invisible meanwhile.
    pub content_height: f32,
}

pub struct StripeSegment {
    pub y: Range<f32>,        // layout px at derivation
    pub style: StyleId,       // resolved to color AT PAINT, live theme
    pub byte: u32,            // the run's first byte — the click target
}
```

Segments carry the `StyleId`, not a color: paint resolves through the
LIVE theme, so a theme switch recolors the track next frame without a
re-derivation (the switch still relaunches — a theme may change WHICH
styles carry stripe colors — but the track never shows the old
palette while that lands). The value is consume-once-shaped but must
ride document clones, so it is `Arc<ScrollStripes>` — O(1) clone,
never a deep copy (the persistent-structures rule).

## 3. The algorithm

Input, all O(1) captures at launch (persistent values, internally
consistent forever): the editor's registered `Markup` clones (a
handful of entries — feature markups are small by nature), the
editor's `DocumentLayout` clone, the theme (to FILTER which styles
carry a stripe color; the color itself resolves at paint), and
`(DocumentToken, EditorId, serial)` for the landing guards.

The pass, linear as stated:

1. Collect the registered entries' contributing intervals and sort
   their boundary events worker-side (each `query(0.., Ascending)` is
   already ascending; feature markups hold no child syntaxes, so the
   recursive query degenerates to a plain interval walk — nothing
   recurses).
2. Resolve boundaries to y in ONE co-walk over the layout rope,
   accumulating `(byte, y)` element by element against the sorted
   events — O(elements + intervals), one pass, no seeks. A boundary
   past EOF (a trailing deletion's zero-length marker) resolves to
   the track's BOTTOM.
3. Quantize: bucket ys onto a fixed grid (finer than any physical
   track), merge touching same-style runs (a merged run keeps its
   FIRST contributor's byte — the click target), emit ascending.
   Unrepaired tail elements carry height 0 and contribute degenerate
   ys — honest: the track compresses what layout has not reached, and
   the repair landing's relaunch (§4) stretches it out as heights
   materialize.

Unlaid, empty, or registered-but-empty inputs land an empty list —
the panel simply mints nothing next frame (an empty landing must not
starve the lane; it swaps silently, the `replace_markup` precedent).

## 4. Machinery: lane, effect, landing, sweep

- **One lane per (document, editor)** — the token in the stripe slot
  (tokens are inert `Copy` ids, safe in store values), relaunched
  through `fx.relaunch`: a typing storm holds one derivation in
  flight and the last capture wins the lane.
- **The landing** rides the entity road: `AppCommand::Entity`
  delivers, and the `ApplyScrollStripes` arm guards — token mismatch
  drops (the cross-document lesson), a retracted editor drops, a
  stale serial drops (strict last-wins). Apply is one field swap; no
  damage, no repair, no text invalidation — the store change marks
  the redraw and the panel repaints from the new value.
- **The trigger is a batch-tail sweep** —
  `documents::scroll_stripes::sync_scroll_stripe_lanes` beside
  `sync_diff_lanes` at `perform_batch`'s tail: ONE choke covering
  every path that can move the projection (typing, paste, undo,
  markup swaps, repair landings, reparse landings, theme switches),
  with no per-site launch to forget. The early-out is
  `wants_scroll_stripes` — something to project (or something STALE
  still painted: the last contributor left and the track owes one
  clearing relaunch) AND an enabled editor showing a track; O(1) when
  nothing registers anywhere. Per enabled editor, a fingerprint
  compares against the last-launched stamp:

  ```rust
  struct StripeStamp {
      revision: u64,               // edits shift bytes AND heights
      generation: u64,             // registered-entry lifecycle (§1)
      height_bits: u32,            // repairs move the y axis
      theme: Arc<str>,             // stripe colors are the theme's
  }
  ```

  Moved → relaunch. The sweep is O(enabled editors) compares per
  batch; nothing bypasses the batch tail, and were a path ever
  missed, the stamp makes it staleness the next batch heals, never
  corruption (the diff-registry argument, verbatim).
- **Which editors participate**: pane editors only — the pane mount
  opts in through `documents::scroll_stripes::enable_scroll_stripes`
  (the `with_gutter` road). Value inputs, chat cells, search rows,
  bounded fragments, diff halves never derive and never mint.

### Why standalone, not an enricher

The pass LOOKS like an enricher — background, snapshot-in/value-out,
a per-(document, editor) lane, a serial-guarded landing — and the
mismatches are structural, not cosmetic:

1. **The output contract is wrong.** An enricher's whole meaning is
   "one feature markup entry, landed through `replace_markup`" —
   byte intervals that shift at the edit door, damage ranges, visible
   repairs. Stripes produce Y-SPACE GEOMETRY: nothing to shift (an
   edit invalidates the projection wholesale — the y axis moved),
   nothing to damage (no text pixel changes). Landing through
   `replace_markup` would mean a fake markup nobody displays plus a
   SECOND projection walk to turn it into track pixels — the
   projection is the entire job, so the enrichment door does none of
   the work and all of the ceremony.
2. **The input contract is wrong.** `EnrichInput` is (text, syntax,
   changed, previous), and the rule beneath it is "passes are
   independent" (docs/editor/editor-enrichment.md). The stripe pass reads
   EVERY registered feature markup — other passes' outputs — plus the
   `DocumentLayout`, and never reads syntax at all. It is not a peer
   pass; it is the one AGGREGATOR downstream of all of them.
3. **The triggers are wrong.** `Interest` is syntax|carets. Stripes
   wake on registered `replace_markup` (including enrichment landings
   themselves), on every edit, on REPAIR landings (heights are half
   the projection), on theme switches. The batch-tail fingerprint
   sweep covers all of it in one place. (There is also a loop hazard
   held off by construction: a pass that lands a NON-markup value
   cannot re-trigger itself, structurally.)
4. **The incrementality is wrong.** Enrichment splices fresh work
   into a persistent previous; stripes are a whole-document
   projection whose output is bounded by a grid — one linear worker
   pass, and the splice machinery buys nothing.
5. **The cardinality is wrong.** Enrichers are producers, one lane
   per feature; stripes are one CONSUMER of everything.

What standing alone costs: one lane's bookkeeping on the editor, one
sweep at the batch tail, one effect type. What it buys: both
contracts stay honest. The diff registry made the same call for the
same reason (its normalize lane shares enrichment's idioms, not its
trait).

## 5. Rendering: the overlay road

The sticky-lines recipe, applied to the right edge:

- **The host**: `editor::scroll_stripe::HOST`
  (`OverlayHost("editor.scroll-stripe")`), declared by the pane node
  on the thunk that WRAPS the vertical scroll, beside sticky's. Host
  locality (docs/ui/UI.md): the request resolves one level up, never
  rides the deep map chain to the window.
- **The request**: minted in the pane's gathered realize
  (`GatheredPane`, documents' entity_view, via
  `EditorView::scroll_stripe_overlays`) — the seam between the
  horizontal pan the editor owns and the vertical scroll above it.
  Gated on the stripe-enabled editor (§4) and a non-empty landed
  value; anchored at the realize viewport. The `ScrollView`'s realize
  offsets the anchor by the scroll on the way up, so the host
  receives it in PANE-VIEWPORT coordinates at this frame's true
  scroll.
- **The panel**: a per-frame widget over frame-borrowed values — the
  `Arc<ScrollStripes>`, the live theme, the track rect. Layout: full
  host height, `width` wide, in its own lane `inset` LEFT of the
  scrollbar's own margin lane; the marks map onto the knob's
  `track_inset` band (the knob and the marks never contend, which is
  what keeps the z-order question, and the `ScrollView`, untouched).
  Paint: for each segment,
  `y' = y × track_height / content_height`, clamped to `min_height`,
  filled with the style's live stripe color. Bounded by the grid,
  every frame.
- **A click NAVIGATES — this is the feature's point.** The panel
  resolves the click's track-y to the nearest segment (the list is
  ascending — a binary search) and answers the standing
  `EditorCommand::RevealAt { byte }` — the sticky recipe: the perform
  places the caret and arms the reveal, and the scroll-to-caret road
  (docs/ui/scroll.md — the golden-section placement) does the rest. The
  overlay's command mapping routes it home for free. A click on
  empty track does nothing; a segment whose byte an edit has since
  moved degrades to a nearest-position caret drop (the chunk-action
  rule — resolve at click time, never stored geometry).
- **No shared mutable state anywhere on this road**: the value is an
  `Arc` read from the gathered document, the widget is a frame
  artifact, the request is data that moves by value. Nothing
  retained, nothing locked.

## 6. Producers

### The diff markup — the reason the track exists

The user's primary gesture is "take me to the changed place", so the
diff contributes first of all — and it contributes as THE diff markup
(decision 2), through the one projection everything else uses.
`Diff.markup` is whole-document by construction (docs/editor/diff-stripes.md
§3): hunk-granularity `DiffAdded`/`DiffModified` spans plus
zero-length `DiffDeleted` markers, derived from the operation at
`add_diff` and refreshed at every normalize landing — so a change far
OUTSIDE any viewport stripes, which is exactly the scale the track
exists for. (The pane's word tints are viewport-windowed and stay
what they are — text presentation; they never enter the track.)
Between normalize landings the markup is honestly approximate: its
intervals shift at the edit door like every interval, so standing
hunks track typing the same frame; a fresh edit's OWN hunk appears
when its normalization lands — the one-landing-behind colors
contract, invisible at track scale.

The `diff-added`/`diff-modified`/`diff-deleted` styles carry `stripe`
colors in both theme JSONs; on the track they paint the gutter's
palette, and in a non-pane editor's text they paint nothing an editor
did not opt into — ordinary editors never `show_markup` the diff
entry (View scope), so the track shows the change and the text stays
untinted.

### The find bar

The find bar registers its per-pane tint markup beside its
`show_markup` pick — cmd-F instantly shows the match distribution on
the track, in that pane only; `match` carries a `stripe` color in
both themes. Two panes over one document each project their own
picks; closing the bar unseats the entry and the diff stripes stand
alone again.

### Deliberately not producers

- **Search-panel installs** (`LocationList`): the install markup is
  picked by the ROW editors only, so registering it would stripe no
  pane — it stays off the track until the pick story says otherwise.
- **Occurrences / brace match**: occurrences want a registration at
  the slot's mint, which the enricher road does not yet offer; brace
  match stays off on purpose — two marks at the caret say nothing at
  track scale. Per-feature judgment is the registration model's
  point.
- **Diagnostics**, when their rendering surface lands (docs/ahp/lsp.md's
  deferred arc): the registration and colors are ready — severity
  styles gain stripe colors and the classic error stripe falls out.

Also deferred, deliberately: a caret/selection you-are-here marker;
per-style paint priority when segments overlap (paint order is entry
order then interval order; a theme-side rank if it ever reads wrong);
next/previous-change commands hopping the diff markup's hunks (the
data is there; the commands are a small later rider).

## 7. Chrome

`ui.scroll_stripe` in `UiTheme` (`#[serde(default)]`, physical px,
both theme files): `width`, `inset` (the lane's gap left of the
scrollbar's own lane), `min_height`. Style-level `stripe` colors per
§1. No constant in code.

## 8. Pinned invariants

The derive/guard/lane battery (`scroll_stripe/tests.rs`): exact
projection positions from known markups and layouts; unregistered
entries invisible; a registered entry with a stripe-less style
invisible; the grid merge; the fingerprint relaunch table (typing,
registered swaps, repair-grown heights, theme switches — and an
unregistered swap does NOT relaunch); strict last-wins under churn;
hunk classification; the diff-birth + normalize-refresh road; the
EOF deletion marker. The sweep→launch→land round trip is pinned in
`documents/src/scroll_stripes.rs`; the diff-role isolation
(`scroll_stripes_follow_the_diff_through_the_app`) at app level. The
perf gates: the derivation never runs on the UI thread, the landing's
perform is O(1), paint is bounded by the grid on a monster document
with 20k matches.
