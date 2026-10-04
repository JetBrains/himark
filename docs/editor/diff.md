# Diff

Side-by-side (split) and inline diff for himark. The design is
modeled on Fleet's diff — the closest architectural relative: an
editor engine where diff is **not a bespoke widget** but two real
editors whose diff-ness is ordinary editor machinery — and goes one
step further in two places: the diff stays **continuously valid by
composition** (Fleet defines a compose but recomputes instead), and
alignment is a **repair concern** — the same viewport-bounded/
background split every layout change already follows — rather than
view-model bookkeeping.

**Placement**: the diff model *and* the split-view machinery live in
the **editor crate** — the diff is document machinery, like layout
and repair, not a plugin concern. The live state (the operation, the
normalization lane, the hunk markup) is document + registry state —
`Document.diffs` and the `OpenDocuments` diff table
(docs/editor/diff-stripes.md) — so every face shares it: the split pane, the
inline view, the gutter stripes, the scroll track. The `hidiff`
plugin is only the workbench face: it *shows* a diff (panel, store
identities, commands), it does not own one.

The four load-bearing decisions, up front:

1. **A diff IS an `operation::Operation`** — the edit that turns the
   left text into the right text.
2. **Sync changes adjust the diff by composition**, on the UI thread,
   O(log n) per keystroke. The diff never goes stale, only
   non-minimal.
3. **Diff computation is an optimization effect**: it takes the two
   texts, computes the minimal diff off-thread, and its landing
   rebases over whatever composed in while it ran — exactly like
   typing. A landing is never discarded.
4. **Alignment = paired repair over hardlines.** After each repair of
   either side's layout rope, spacers for the repaired region are
   inserted/updated so corresponding hardlines share a y. The
   operation, being a rope, maps offsets between the two documents in
   log time.

Fleet's blueprint, kept and departed from: its persisted model is
also a single retain/replace operation with fragments derived lazily;
its spacer heights also come from the *other* editor's laid-out pixel
geometry, never line counts — that insight is kept. What is replaced:
Fleet additionally inflates per-line heights in a half-incremental
paired cache, which paired repair supersedes; and Fleet recomputes
the whole diff reactively per edit where himark composes.

---

## 1. The model: a diff is an `Operation`

`operation::Operation` is everything the model needs — it is
**rope-backed** (`Rope<Op, OperationMeasure>`, the operation crate)
with the two cumulative axes `OLD_LEN` / `NEW_LEN`, which is
precisely the log-time offset mapping between the two documents:

- `transform_offset(offset, bias)` — left-document offset → right;
  `transform_offset_back` is the same seek on the other axis.
- `compose(&self, subsequent)`, `transform`, and `invert()` (trivial,
  since `Delete`/`Insert` carry their text — swap them, keep
  retains).

`editor::diff` supplies the computation CONTRACT — a policy the edge
installs (docs/editor/structural-diff.md):

```rust
/// The exact edit turning `base` into `target`; policies differ only
/// in alignment quality and cost. Installed as `env::Differ`.
pub trait DiffPolicy: Send + Sync {
    fn diff(&self, base: &Text, target: &Text, syntax: Option<&DiffSyntax>) -> Operation;
}
```

The baseline policy is the **`myersdiff` crate** wrapping `similar`
(MIT/Apache, pure Rust, wasm-clean — Myers with
patience/unique-anchoring built in); it owns what Fleet layers on
top: **by-word refinement** inside each modified block
(identifier-ish tokens / CJK codepoints / newlines, delete+insert
pairs within a small gap merged into modified word ranges), degrading
to the plain line block if it blows the budget. `frontend-host`
overrides with `structdiff::Structural` — difftastic's syntax-tree
alignment where trees exist, Myers everywhere else; the web edge
stays on Myers. Core crates depend on neither engine.
Whitespace policies are deliberately out of scope — the diff is
exact. (An ignore-whitespace diff is not a literal text
transformation, so compose/rebase must keep running against the exact
diff; a whitespace policy can only ever apply at the presentation
stage — the `hunk_markup` derivation is the seam built for it.)

### Fragments — lazy views

```rust
pub enum FragmentKind { Added, Deleted, Modified, Unchanged }

pub struct Fragment {
    pub kind: FragmentKind,
    pub left: Range<u32>,
    pub right: Range<u32>,
    pub words: Vec<(Range<u32>, Range<u32>)>,  // Modified refinement
}
```

Fragments are derived lazily from the operation and the texts —
nothing ever materializes the full list. `fragments_from(op, left,
from)` iterates from the origin; `fragments_at(op, left, from)` is
the seeded walk: it seeks the operation rope to a safepoint (a
newline-carrying retain) at or before `from` in log time and iterates
from there — what change navigation, mark derivation and the
before-card's block walk all ride. Adjacent replaces on touching
lines group into one fragment; kind falls out of which sides have
lines.

---

## 2. Live maintenance: compose at the door, normalize on a lane

**Invariant: the diff is always a valid transformation of the current
texts.** The machinery lives on the document and in the registry
(docs/editor/diff-stripes.md §§1–2); the shape:

### Sync: composition per edit (UI thread, cheap)

Every edit composes into the standing operation in the same
transaction that applied it:

- an edit `b` to the **target** document composes at the document's
  own edit door: `diff = diff.compose(b)`;
- an edit `a` to the **base** document composes through the
  registry's eager base-side update: `diff = a.invert().compose(diff)`
  (the compose-cursor idiom).

Point edits are a splice at one rope position — O(log n). The result
is exact as a transformation (offset mapping, alignment, apply/revert
all stay correct); what degrades is *minimality* — retype deleted
text and the diff shows a delete+insert where a fresh diff would show
nothing. That is a presentation blemish, not a correctness issue, and
it is what normalization heals.

### Async: the normalization lane

Normalization is an optimization effect — not deferred work whose
absence is a hole: the faces are fully functional on the composed
diff; the effect lands a minimal one. One supersession lane per diff
(`DiffNormalizeEffect`, relaunched through the stored token, so a
typing burst costs one computation at the end); the worker computes
`diff(base, target)` off-thread and derives the fresh hunk markup
beside it. **Landing rebases instead of discarding** — exactly the
typing rule, applied to the edits that arrived while it ran:

```
a = base.log.compose_since(base_revision)     // EditLog machinery the
b = target.log.compose_since(target_revision) // reparse landing also uses
diff = a.invert().compose(minimal).compose(b)
```

So a landing is never wasted, no matter how much typing overtook it.
Guards, in order: **document identity** (revisions are per-document
counters and cannot tell documents apart — the cross-document reparse
lesson), then rebase through the logs. A log that no longer reaches
the captured revision falls back to scheduling a fresh capture.

---

## 3. The split view — editor-crate machinery, `hidiff` face

The split view is **editor-crate machinery**, the way `EditorView`
is: a value-level `SplitDiffView` owning the pairing — the two
documents' bound editors, the settled operation clone
(`DiffState`), the mark windows and the spacer discipline —
implementing `View` like everything else. The `hidiff` plugin is the
thin workbench face over it: a `PanelView` that gathers the two
documents from the registry (the `EditorIdView` gather/scatter
scheme, docs/editor/documents.md), tracks/untracks the diff, routes landings
home by identity, registers `diff.open`, and dismantles. The plugin
knows *how a diff is shown in a workbench*; the editor crate knows
*what a diff is*.

- **Equal widths, one scroll.** Both editors lay at
  `(panel_width - center) / 2`. Unchanged runs are identical source at
  identical width → identical wrapping, identical heights (inlays
  included — an unchanged table renders the same twice). Height
  divergence is confined to changed fragments and healed by spacers —
  so both sides have equal total height and a single `ScrollView`
  scrolls the pair. No sync protocol; himark soft-wraps, so there is
  no horizontal scroll at all.
- **Both sides are ordinary mutable editors.** Edits write through the
  ordinary editor path and compose into the diff (§2). There is no
  read-only editor kind; a vcs old side is simply a revision-pinned
  snapshot document (docs/editor/diff-stripes.md §7).
- Opened by a registered command (`diff.open`, a `DynamicCommand`) or
  by the vcs roads (§6); `dismantle` untracks — the pane's markups die
  with its editors, the entry with its last ref.

---

## 4. Alignment: paired repair over hardlines

The alignment invariant: **corresponding hardlines sit at the same
y.** A *hardline* is a newline-delimited line (not a soft-wrap row).
Correspondence is defined by the diff: inside every `Retain` run, left
and right hardlines pair 1:1; inside a changed fragment there is no
correspondence, and the fragment's two sides just need equal *total*
height at the next aligned boundary. The operation rope answers
"which right offset corresponds to this left hardline start" in
O(log n) — that lookup is what makes alignment a local, repair-shaped
computation instead of a global pass.

### The mechanism

Spacers are **per-editor layout state, not markup**: a
`LayoutElement` carries an interline `spacer_above: f32` folded into
its height metric. Two reasons this beats markup inlays: markup is
per-document (the same document open in a normal pane must not show
diff spacers — layouts are already per-editor), and the repair
pipeline already owns splicing this rope.

**Paired repair**: a diff pane's two editors' layouts are paired.
Every time a repair region lands in either side's layout rope — the
sync viewport repair on edit, a background repair landing, a resize
re-lay — a *spacer-sync pass* runs over exactly the repaired region:

1. Walk the repaired region's hardline boundaries (NEWLINES metric —
   log-time seeks, linear in the region's lines).
2. Map each boundary through the diff to the other side. Boundaries
   inside `Retain` runs are aligned pairs; a changed fragment
   contributes one aligned pair at its end boundary.
3. At each aligned pair, compare cumulative heights:
   `height_before(left_boundary)` vs `height_before(right_boundary)`
   (existing log-time layout metric, both sides measured *without*
   their own spacer at that boundary).
4. Splice `spacer_above` on the shorter side's next element to the
   difference; zero it where the sides agree. Each splice is the
   O(log n) height-splice repairs already do.

The pass (`align::sync_spacers`) is idempotent and local: it reads
real laid-out geometry (so wrapped lines, headers' font sizes,
settled table inlays are all accounted for — Fleet's key insight
kept), and it costs O(region hardlines · log n) — the same order as
the repair that triggered it.

**Correct under any inlays — by requirement.** Because alignment
measures geometry rather than counting lines, arbitrary inlays on
either side (tables, host badges, the fold strips) are automatically
absorbed into the heights being compared. Two consequences the
implementation honors: a hardline boundary hidden inside an `Instead`
inlay's range does not exist visually — the spacer-sync skips it and
aligns at the next visible boundary; and an inlay whose height
settles later (a table's async re-lay) is just another height change
landing through repair, which re-triggers the sync for its region.
Collapsing unchanged regions is therefore **orthogonal by
construction**: the folds (§5b) are one more inlay kind, and the
alignment neither knows nor cares — it only ever promises that
visible corresponding hardlines share a y.

### Where it runs (the perf guarantees, point by point)

- **Viewport-bounded repair** (the sync, budgeted repair every edit
  and scroll-reveal performs) triggers spacer sync **for that region
  only** (`sync_region`). The user's viewport is always aligned by
  the time the frame paints.
- **Background repairs** trigger the same pass over their regions as
  they land — off-viewport spacers heal progressively, exactly like
  layout itself. No separate "alignment effect" exists: alignment
  piggybacks on the repair traffic that already flows, through the
  effect-mapping discipline the tree lives by — **`SplitDiffView`
  transforms the effects coming out of each editor half so their
  landings take care of BOTH halves** (`RepairDiffEffect`, the paired
  lane). The repair machinery is reused untouched; the diff behavior
  lives entirely in the wrapper.
- **Owed spans split visible/deferred** (`sync_visible_owe_rest`):
  the viewport slice aligns in-frame, the document-scale remainder
  rides the paired worker effect. The INLINE share is HARD-CAPPED —
  the viewport window degenerates to the whole document when a half's
  layout is mostly unlaid (`byte_at_y` clamps past the laid extent);
  the monster probe caught multi-megabyte inline walks through
  exactly that hole, and the cap only defers alignment since the full
  span rides the worker regardless.
- **The UI thread never diffs.** Its per-keystroke diff work is the
  compose (O(log n)) and the viewport spacer pass
  (O(visible hardlines · log n)). The diff policy runs only on the
  executor.
- A normalization landing can move fragment boundaries without any
  layout change; the pane observes the generation move, computes the
  disagreement regions, and the ordinary repair traffic — viewport
  first, background tail — re-syncs spacers.

Editors outside a diff pane never see any of it: their effects are
simply not wrapped, so the non-diff path costs zero by construction
rather than by a `None` check.

---

## 5. Highlighting

Two layers, deliberately split by cost and scope:

- **Line washes ride THE diff markup** (`Diff.markup`,
  docs/editor/diff-stripes.md §3): whole-document hunk intervals —
  `DiffAdded`/`DiffModified` spans plus zero-length `DiffDeleted`
  markers — derived from the operation at birth and refreshed per
  normalization landing. The pane's halves `show_markup` it; washed
  from birth, no window to chase. A modified line washes with the
  modified tint.
- **Word tints are pane presentation**, derived viewport-scoped on
  the pane's paired-repair rider (the marks job): the window is the
  displaying viewports' union quantized to a 4KB grid — word-level
  derivation over whole monsters is the cost class the window exists
  for. The right side's tints land in the pane-owned right-extras
  markup, the base side's washes and tints on the record's
  `base_markup`; both are `replace_markup` swaps with worker-computed
  change sets (the producer-brings-the-change-set contract,
  docs/editor/markup.md). Fragment walks are capped per derivation; marks
  shift as intervals under edits for free between derivations.

Washes resolve through the theme (`ui.diff` chrome, the
`diff-added/modified/deleted` styles in both theme files) to
full-line background washes — left deleted-tinted, right added-tinted
— with word marks as the stronger inline variants inside Modified
fragments. Ordinary editors never show any of it: the diff markup is
View-scoped presentation a face opts into.

---

## 5b. Folds: collapsed unchanged regions

Unchanged runs collapse behind paired `Instead` strips
(`split_diff/fold.rs`), Zed-style: a horizontal widget labeled
"⋯ N unchanged lines" with reveal/hide-context buttons at each edge
and a remove button.

**The strips are not pane state.** Each strip is an inlay interval in
the pane's own marks markup, one per half, under **one shared
`IntervalId`** — the same key names the strip in both markups, so the
key alone pairs the halves (`adjust_fold` resolves both sides from
it). Strip keys mint downward from `u32::MAX`; wash builders mint
upward from zero, so they never collide. The only fold field on
`DiffState` is a one-shot `FoldPhase` latch.

**Derivation is a worker product, like the marks.** The first
normalized generation flips the latch to `Owed`; the next marks job
derives `FoldSpec`s (`derive_folds`: retained runs, whole hard lines,
`FOLD_CONTEXT` visible lines at any edge that abuts a change on
EITHER side, `FOLD_MIN_LINES` minimum — and an identity diff folds
*nothing*) and mints the strips straight into the replacement markups
(`mint_fold_strips`). The landing is the ordinary atomic
`replace_markup` swap. Later marks jobs carry the standing strips
into each replacement by key; in the set-diff a strip fingerprints by
its KEY, so a carried strip is "unchanged" and the change set stays
wash-only (fingerprinting inlays as always-changed would re-damage
every strip per landing — an infinite swap loop).

**Collapse makes repair cheap, not expensive.** A paragraph fully
covered by an `Instead` inlay never shapes (`visual_lines` emits a
zero-height element without building its display text), so installs
and repairs over folded spans cost element emission only.
Marks-landing reshape heals route through `sync_visible_owe_rest`:
the viewport slice aligns in-frame, the document-scale remainder
rides the paired worker effect.

**Adjust runs at the pair level.** The halves'
`EditorCommand::Inlay` arms intercept commands that downcast to
`FoldCommand`; reveal/hide hop `FOLD_STEP` hard lines at the pressed
edge, clamped to the fold's maximal extent — recomputed from the live
diff, never stored — and the same key swaps in place on both halves.
Remove deletes both strips; since derivation is one-shot, a removed
fold never returns. Inlay repairs damage spans (`inlay_repair_span`):
`Instead` re-lays its whole interval, and `Document::replace_inlay`
damages old ∪ new so a shrinking strip re-lays the lines it releases.

---

## 6. The atomic open

A pane opened with an off-thread PREP opens FULLY DRESSED — washes,
fold strips and the normalized operation are all standing before the
first frame paints. **No diff is ever computed on the UI thread to
dress a pane** (docs/no-diff-on-ui-thread): the dressing rides a diff
the WORKER already produced, or it is deferred to the normalize lane.

The dressing is pure compute over `(Operation, left Text)`:
`prepare_marks` (`split_diff.rs`) derives whole-document marks
(fragment-capped like every derivation) plus the strips, sharing the
minting code with the pipeline. The prep travels stamped with the
revisions it was diffed against (`DiffPrep`/`PreparedDiff`), so
`track_diff` trusts it only if the LIVE pair still stands where the
worker snapshotted it (§below). Who produces it:

- **The vcs open** (`OpenDiffByLocationsHandler`, hiahp): on the
  WORKER, right after both documents build — the prep rides the
  `OpenDiffPair` landing, stamped with the built sides' revisions.
  The landing forwards it only when BOTH sides register fresh; a
  deduped side opens on the seed and dresses from the lane.
- **Row-owned diffs** (the chat diff cell, the diff canvas —
  docs/editor/diff-canvas.md): on the build worker, landing with the row,
  stamped with the built sides' revisions.
- **Local opens** (`diff.open`, back-navigation): carry NO prep — a
  synchronous diff here is banned. The pane opens on the whole-replace
  SEED (an exact delete-all/insert-all, zero diffing) and the
  batch-tail sweep owes it one normalization, which lands the minimal
  diff and its dressing from a worker one round-trip later.

`track_diff` is the gate. Given a stamped prep it checks the live
pair: unmoved (same revisions, matching lengths) → install the
operation normalized at birth (generation 1, `record.normalized`
stamped — the sweep owes nothing); moved (a docsync host edit landed
between the worker's snapshot and this landing — the 2026-09-24
crash) or absent → install the whole-replace SEED (generation 0) and
leave the minimal diff owed to the normalize lane. It never rebases a
prep across the live log: the prep was diffed against a fetched
snapshot, not a point in the edit log, so composing the live tail onto
it is unsound — the sound rebase lives in the normalize landing, whose
capture IS a live ancestor.

With a valid prep `diff_panel` installs everything before the pane
exists: the entry normalized at birth; the marks markups seed with
empty change sets (a plain map insert — no editors yet); the half
editors mount with the markups in `shown`, so the FIRST layout shapes
washes and collapses folds (`Instead` spans skip shaping — the dressed
first build is cheaper than a naked one); and `DiffState::attach` runs
eagerly at construction with the seeded window — fold phase Done, marks
clean, the standing `marks_window` also serving as the pre-viewport
fallback in `marks_window_now` so no shrinking marks job fires before
the first viewport report. A pane opening on the seed (no/rejected
prep) attaches undressed — marks dirty, no seeded window — and dresses
on the first normalize landing. A pane joining an ALREADY-tracked pair
skips the prep entirely — the standing entry and its markups are the
truth.

---

## 7. The inline view — `UnifiedDiffView`

One editor — the right document — with deleted/modified fragments
shown as before-cards: `Above` inlays hosting **bounded editors over
the base document** (the before-card machinery, docs/editor/diff-stripes.md
§6). Added lines are washes on the host editor. Inline needs no
paired repair at all — there is one layout.

`UnifiedDiffView` wraps the `SplitDiffView` — the ONE diff state —
and switches FACES on `diff.toggle-layout` (offered on the pane's
focus chain, `UnifiedDiffCommand::SetLayout`). The inline face is a
FRESH editor on the right document (no pair spacers), minted once on
the first switch: it shows the diff markup and the pane's
right-extras from the first build — washes, word tints and fold
strips all standing, so unchanged runs collapse exactly like the
split's and fold buttons stay pair-lockstep (one shared key; the fold
commands route through the split's `adjust_fold` from either face) —
and every changed block's before-card is expanded SETTLED at mint
(`Document::expand_before_inlays` — the stripe click's block walk
over the whole document, `animate: false`). The face is a
gutter-bearing editor with the base joined, so it carries the gutter
stripes and the stripe-click road like any base-joined editor. Inline
edits write through the shared right document, and the pair settles
after each (`settle_after` + `pair_lane`), so the split face is
honest the moment the user switches back. The face persists on the
`DiffState` row (`unified_layout`/`inline_editor`); the pane's
dismantle removes the inline editor with the halves'.

---

## 8. Perf summary (the guarantees)

| moment | UI thread | executor |
|---|---|---|
| keystroke in a diffed doc | compose O(log n) + viewport spacer sync O(visible hardlines · log n) | one in-flight normalization (the diff policy, lane-relaunched) |
| normalization landing | rebase (two composes) + markup shift-and-swap | — |
| scroll reveals stale region | that region's repair + its spacer sync | background repair tail continues |
| background repair lands | splice + region spacer sync (both O(region · log n)) | — |

Nothing diff-shaped is ever O(document) on the UI thread; alignment
work is always bounded by the repair region that carried it.

---

## 9. Design decisions, with their reasons

1. **The algorithm is a policy, injected at the edge** —
   `editor::diff::DiffPolicy`, installed as `env::Differ`. The Myers
   baseline (`myersdiff`, wrapping the `similar` crate — pure Rust,
   wasm-clean, patience/unique-anchoring built in) serves texts
   without trees; `structdiff::Structural` upgrades tree-backed files
   to difftastic's structural alignment (docs/editor/structural-diff.md).
   Every policy's operation is EXACT; they differ only in alignment
   quality and cost.
2. **No read-only editor kind** — both sides are ordinary mutable
   editors; a never-editable baseline is a revision-pinned snapshot
   document, not a new editor mode.
3. **No whitespace policy** — the diff is exact; a policy can only
   apply at the markup-derivation stage, never to the operation
   compose/rebase runs against.
4. **Collapsing unchanged regions is orthogonal** — one more inlay
   kind; the diff's only obligation is correctness in the presence of
   arbitrary inlays (§4), which the geometry-measuring alignment
   gives by construction.
