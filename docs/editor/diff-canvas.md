# The diff canvas

Selecting a **revision** (history view) or the **session folder root**
(changes view) opens the **diff canvas**: one workbench panel holding
one huge scrollable list — a **banner** first (the commit's message
and author on a commit canvas; the **commit composer** — message box,
COMMIT button, ⌘⏎ — on the working-copy canvas), then per changed
file a large right-aligned header styled exactly like markdown
Header 1, and under it that file's diff. Diffs are **lazy**: a row entering the viewport arms on its
first paint, fetches its sides, builds the diff off-thread, and lands
**atomically** into the list. Selecting a **particular file** in
either view does not open a separate pane — it **reveals** that file's
row in the canvas.

This is the feature viewport preservation
(docs/editor/viewport-preservation.md) exists for: landings above the
viewport re-aim the scroll through the settle pulse, so a canvas that
is loading all around the user never moves under them. No frame paints
an un-shifted viewport, and nothing canvas-shaped is ever O(files) or
O(bytes) on the UI thread.

The load-bearing decisions, up front:

1. **A tree, two rows per file** — `ListView<CanvasRow, CanvasKey>`:
   the header is a PARENT row (`CanvasKey::File`, the file's `new`
   side — the reveal key) whose cover span encloses the diff child
   row (`CanvasKey::Diff`; body: placeholder | built diff | failure
   note). Parenthood is the list's own `Intervals` structure, so the
   list's generic sticky option plants the current file's header —
   buttons live — while its diff scrolls, and collapse is a splice
   that parks the built child. The banner row (`CanvasKey::Banner`)
   covers nothing and never sticks. Build routing is BY KEY
   (`CanvasCommand::ToRow`): splices shift indices under in-flight
   landings.
2. **A canvas row is a REGISTERED pair.** Each side is the
   `OpenDocuments` document for its location (reused if already open,
   registered from the fetched build otherwise), the diff is tracked
   by the Diffs subsystem (`build_diff_view` → `track_diff`), and the
   row holds only a `PairPane` id over the store-held `DiffView` —
   the standalone diff pane's exact shape. Editing a row edits the
   real document; the diff composes at the edit door and the
   normalize lane keeps it minimal (§7).
   The canvas changes *when* the build runs (viewport-armed, not
   eager) and *how tall the unbuilt row is* (stat-reserved).
3. **Placeholders reserve real height** — estimated from the entry's
   `added`/`removed` counts, a flat default when stats are absent.
   Heights are pure arithmetic carried in the list rope's elements
   (`push_keyed_sized`), so populating the canvas never lays a row to
   learn its size.
4. **Every height change is a door.** A build landing swaps body and
   height in ONE one-row splice and raises `fx.settle()`; the settle
   pulse re-aims the scroll before paint. Zero wrong frames, by
   construction.
5. **Reveal is `TopLeftAt` at the row's header**, keyed and
   selection-free (`ListView::reveal_row`). The file's header line
   lands exactly at the viewport top and *stays* there while neighbors
   build — preservation's promise, and the feature's acceptance test.

---

## 1. The shape

The panel lives in `plugins/hidiff` (the workbench face for diffs —
docs/editor/diff.md's placement rule), `frontend/plugins/hidiff/src/canvas.rs`:

```rust
pub struct Canvas {            // STORE-HELD state, in `Canvases`
    source: CanvasSource,       // what changeset this shows
    rows: ScrollView<ListView<CanvasRow, CanvasKey>>,
    files: …,                   // key → the pair of side locations
    note: …,                    // the whole-canvas pending/error note
    seen: Option<u64>,          // last adopted feed generation
    populated: bool,
    request: Option<PanelRequest>,
    phases: …,                  // key → RowPhase, the test oracle
    reveal: Option<ResourceLocation>,  // armed by navigation
    refs: u32,                  // views standing on this canvas
    stash: …,                   // collapsed files' parked diff rows
}

pub struct DiffCanvasView {    // the PANEL — a reference view
    id: CanvasId,
    source: CanvasSource,
    request: Option<PanelRequest>,
}

pub enum CanvasSource {                                     // himark::diff_canvas
    WorkingCopy { folder: ResourceLocation },               // changes root
    Commit { folder: ResourceLocation, id: String },        // history revision
}
```

- The content under the `ScrollView` **is** the `ListView` — no
  wrapper in between, so settle-time viewport observation and
  `settle_to` work unmodified.
- **The feed** is `himark::diff_canvas` over the stores that already
  own the data: `canvas_generation` is an O(1) probe
  (`Changes::generation` / `History::generation`);
  `canvas_files(source)` returns `(generation, CanvasListing)` — for a
  working copy each file pairs (before-side ref, working location),
  for a commit (before side, after side or the empty side when the
  after side is absent). A listing that is still computing, errored,
  or empty comes back `Pending` and renders as a whole-canvas note.
  The panel populates its rows from the first ready listing; after
  that, every generation bump (a host push, the REFRESH chip — both
  funnel through `Changes::generation`, the unified gate) drives a
  **keyed reconcile** instead of a repopulate: removed pairs retire,
  added pairs splice in as lazy placeholders at their listing
  position, and a pair whose `updated` stamp (`ChangeEntry::updated`,
  the generation at which the host last touched the entry) moved past
  the row's build relaunches the build at the row's standing width —
  the old view keeps showing until the fresh landing swaps it, so a
  refresh never flashes placeholders. Surviving rows never move: the
  feed reorders on every touch (`ChangesetFileSet` re-appends), and
  rows must not jump. Untouched rows are not rebuilt — the stamp, not
  value comparison, decides staleness, because a same-stats content
  edit produces a value-equal entry; only the host knows it resent
  the file. The reconcile is DRIVEN from the app's sync tick —
  `Canvases::sync`, registered as a `himark::SyncObserver` beside the
  diff/stripe lanes — so a store-held canvas stays current even while
  no panel paints it; the in-panel paint probe is just the same
  refresh arriving through the view. That is why clicking a file in
  the changes view always reveals: the row is already there.
- Canvases are **store-held** (`hidiff::Canvases`, keyed by
  `CanvasId`, at most one per source — the `OpenDocuments`
  discipline). The panel is a REFERENCE view over an id; opening a
  source that already has a canvas reuses it, built diffs and all.
  `CanvasPlace { source, reveal }` is the navigation location
  (equality is the source alone) and `hidiff::CanvasNavigator`
  answers it: find-or-create, hand back a view. Canvases live while
  views retain them; the working-copy canvas stays once opened.
- `phases` (`RowPhase::Placeholder | Built | Failed`) mirrors each
  row's lifecycle for the engine-driving tests — `probe_rows` is the
  oracle.

```rust
pub(crate) struct DiffRow {
    file: CanvasFile,          // name, +N −M stats, the pair
    body: RowBody,
    rewrap_ask: Option<f32>,   // the resize-storm latch (§4)
    built_width: Option<f32>,  // a stale row relaunches at this width
}

enum RowBody {
    Placeholder { armed: bool },    // height lives in the list rope
    Built { pane: PairPane },       // a DiffViewId — state in the store
    Failed(String),
}
```

Built rows live until the change set says otherwise: a retired pair
tears down (untrack + editors removed — the documents stay if open
elsewhere), a restamped pair rebuilds in place. Their bounded editors join the focus chain, and commands
and effects route through `ListCommand::Child` exactly as chat cells
route theirs — caret, selection, clipboard, IME, navigation all work,
because focus wiring is load-bearing for the whole panel: without it
every editor-shaped interaction in the canvas breaks, not just
typing. The focus walk is also how `diff.toggle-layout` reaches a
row: the command is per-`UnifiedDiffView` and lands on the FOCUSED
row only — there is no canvas-wide face toggle.

## 2. The header

A hand-painted chrome leaf (not an editor) resolving
`StyleId::Header(1)` through the live theme — the markdown H1 face
(48.0 by default), emboldened. The file NAME renders
**right-aligned** in the row's width; the `+N −M` trail sits at the
LEFT edge in the theme's added/removed colors (zero counts filtered).
The band's height is derived from the H1 face and identical for every
row, so it contributes a constant to placeholder math.

The header is also the row's interaction surface: the chevron folds
the file, the three right-edge buttons toggle the diff face, open the
live file, and open the standalone side-by-side pane
(`hichanges::OpenDiffForPair`), and a press on the band's body opens
the file too (`workbench.open-in-full` — ⌘⏎ — rides the focused
row). The canvas is the default face; the standalone pane stays one
click away.

## 3. Placeholders — reserved height, no layout

A placeholder holds the scroll geometry honest before its diff
exists: `reserved = header_band + gap + est_lines · line_rhythm`,
where the line rhythm is `title_size × 1.5` and
`est_lines = (added + removed + 4).clamp(4, 60)` when the entry
carries stats — a 2000-line rewrite must not reserve 2000 lines; the
correction on landing is what the settle pulse is for — and a flat 12
otherwise. All arithmetic: the height is pushed with the key
(`push_keyed_sized`) and lives in the list rope's element, recomputed
per layout, never stored in the row. Populating a canvas of N files
is N pushes and zero layouts.

## 4. The lazy build pipeline

**Arming.** A visible placeholder's widget handles its first
`Event::Paint` by answering `RowCommand::Arm(width)` — one-shot.
`ListView` culling walks only visible rows, with no overscan, so
laziness falls out of the existing walk: a row arms exactly when it
first appears on screen, once. The canvas's `perform` peeks the `Arm`
and launches `himark::BuildFileDiffEffect { old, new, width }` before
forwarding.

**The effect** is handled off-thread in `frontend/hiahp/src/open.rs`,
beside the standalone pane's open handler: fetch both sides (a
missing side becomes the empty side — the build fails only when BOTH
sides are absent), build both documents language-aware (markdown as
the fallback), run the installed diff policy (`env::Differ` —
structural where trees exist, Myers otherwise;
docs/editor/structural-diff.md), and `prepare_marks` — washes and
fold strips derived on the worker, the atomic-open discipline
(docs/editor/diff.md). The armed width rides through for the landing's
target; the effect does NOT prebuild a layout at that width — editors
are added at landing, and the height is measured by laying the
finished view once into a throwaway `Arena`.

**The landing** (`CanvasCommand::Landed → land()`) resolves the key
to its row index and runs `mounted()` under the row's effect scope,
yielding `(pane, height)`:

1. REGISTER both sides (`register_side`): a document keyed to a
   location IS the registered `OpenDocuments` document for it — reuse
   the open one, else register the fetched build (§7);
2. `build_diff_view` hands the pair to the Diffs subsystem:
   `track_diff` (the birth diff + THE hunk markup + the normalize
   lane), a bounded editor per half at
   `editor_width = (width - gutter).max(120.0)` with the prepared
   marks and paired repairs, `DiffState::attach` — and mints the
   store-held `DiffView`. The row keeps only the `PairPane` id;
3. `SetLayout(Inline)` performed at mount — the canvas face is the
   inline one, set once per row; the inline-face build expands every
   changed block's before-card programmatically and SETTLED
   (`expand_before_inlays`) — a canvas full of cards growing one by
   one would be noise; fold strips come minted from the prepared
   marks (`mint_fold_strips`);
4. `height = header_band + body + gap`, the body measured by laying
   the pane once into a throwaway `Arena` — bounded by this one row;
5. swap: ONE one-row `splice_slice` with `push_keyed_sized` replaces
   body and height in the same mutation — a door — then
   `fx.settle()`. A relaunch (a stale row rebuilding) tears the
   standing pair down first (`teardown_row`) so no tracked diff or
   editor leaks.

The swap is atomic by construction: the anchor is captured through
the splice door (`door_anchor`/`door_resolve` → `settle_to`), the
settle pulse re-aims the scroll before paint, and the frame that
shows the diff is the first frame that knows about it. Below the
viewport nothing moves; a placeholder the user is looking at swaps in
place at a near-identical top edge. Rows whose live height later
drifts past half a pixel self-report through `ListCommand::SetHeight`,
which rides the same door.

**Rewrap has a two-frame debounce.** The row frame notices a laid
width more than a pixel off its target and reports it once per paint
(the `RewrapOnPaint` recipe); the row's `perform` requires the SAME
width twice in a row (the `rewrap_ask` latch — the resize-storm
guard) before resizing both editors and the inline face and
resyncing — so a resize storm settles before any relayout runs, and
the row pays one rewrap per stable width, not one per frame.

## 5. Reveal, and why preservation is the point

The reveal is a field ON the store-held canvas, armed by whoever
navigates (`CanvasPlace.reveal` — `navigate_to` in place, or the
navigator at find-or-create). It is applied at populate, or — on an
already-populated canvas — the display's paint probe sees it armed
and answers PickupReveal; `perform` takes it and
calls `rows.reveal_row(key, Placement::TopLeftAt)` — keyed,
selection-free, resolved from the row's rope range and emitted as an
exact `TopLeftAt` reveal. It works before the row's content lands,
survives the row's own swap, and clamps at the tail on both the list
and the scroll side, so a bottom-of-canvas file does not re-emit
forever.

The sequence "reveal `foo.rs`, then its neighbors finish building" is
the flagship scenario: the reveal jumps the header to the top; every
later landing above it goes through a door, notes `settle_to`, and
the pulse re-aims — the header does not move by a pixel. The revealed
file's own landing swaps in place under a stable top edge.

## 6. Entry points

- **Changes view, session folder root**: activating the root row
  opens the canvas for `WorkingCopy { folder }` (inner directory rows
  just toggle).
- **Changes view, file row**: opens/focuses the same canvas and posts
  the reveal for the file's `new`-side key — the row keys in the
  canvas and the tree are the same locations, no translation.
- **History view, revision row**: opens the canvas for
  `Commit { folder, id }` and toggles the tree; the one commit-files
  fetch feeds the tree and the canvas alike. Until the changeset
  adopts, the canvas shows the whole-canvas computing note, then
  populates.
- **History view, file under a revision**: canvas + reveal, same as
  changes.
- The standalone pane road (`diff.open`, `OpenDiffForPair`) is
  untouched and reachable from every canvas header.

## 7. Ownership: rows are registered pairs

A document that answers to a `ResourceLocation` must BE the
registered `OpenDocuments` document for that location — the invariant
an earlier "row-owned snapshot" design broke, and the reason edits in
a canvas row once updated nothing.
Rows now register both sides at landing (`register_side`: reuse the
open document, else register the fetched build) and track the diff
through the Diffs subsystem (`build_diff_view` → `track_diff`), so
one identity serves every view of the file: typing in a row edits the
real document, the operation composes at the edit door, and the
normalize lane re-minimizes it — covered by
`typing_in_a_canvas_row_updates_its_diff`. Registration is still
LAZY (a row registers when it builds, not when the canvas opens), so
a hundred-file canvas does not adopt a hundred documents up front.
Teardown is explicit now that nothing dies with the canvas: retiring
or rebuilding a row untracks its diff and removes the pair's editors
(`teardown_row` → `teardown_diff_view`) without closing documents
that are open elsewhere. Freshness comes from the reconcile (§1):
when the host stamps an entry, the row's build reruns and the landing
swaps it in place.

## 8. Cost invariants

| moment | UI thread | executor |
|---|---|---|
| open canvas (N files) | N keyed pushes, arithmetic heights; paint visible rows | listing fetch (stores' own roads) |
| scroll | rope seeks O(log n) + visible-row paint; entering placeholders arm | armed builds: fetch ×2, document builds, the diff policy, `prepare_marks` |
| build landing | mount + one-row lay + one splice + settle pulse (bounded ×3) | — |
| resize | two-frame debounced rewrap per visible row | repair tails |

Nothing is O(files) or O(bytes) on the UI thread; the per-file heavy
lifting (fetch, parse, the diff policy, mark derivation) runs in the
build effect, and a landing's UI cost is bounded by its one row.

## 9. Pinned invariants

The engine-driving harness covers the canvas deterministically;
`probe_rows` (Placeholder / Built / Failed) is the oracle. The pinned
invariants: stat-reserved placeholder heights and the flat no-stats
default; arming exactly on first visibility; the atomic swap — a
landing above the viewport leaves the visible row's screen-y
bit-identical across the frame; reveal lands the header at the top,
survives its own row's swap, and later landings above leave it exact;
a failed build lands the failure note, not a hole.
