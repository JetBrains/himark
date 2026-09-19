# Table editing

Markdown tables render as an **`Instead` inlay** over the table's source
range: a real table editor whose every cell is an `EditorView`. The
markdown source stays the single truth — the table editor is *routing plus
layout*, never a second document model. Cell `<br>`s are newlines inside
the cell view; serialization restores them.

The table widget is an ENRICHMENT-PASS product
(docs/editor/editor-enrichment.md): himarkdown's parse recognizes the
block, `TableEnricher` builds the inlay in its own feature markup on
its own lane, and adoption (below) runs at the pass's splice and
landing — not the syntax-markup splice.

## The projection model

```
| Name | Notes          |          ┌──────┬────────────────┐
|:-----|----------------|    ◀──▶  │ Name │ Notes          │
| a    | first<br>second |         ├──────┼────────────────┤
                                   │ a    │ first          │
                                   │      │ second         │
                                   └──────┴────────────────┘
```

- The parse recognizes a table block; `TableEnricher` emits one
  `Inlay` with `InlayMode::Instead` covering the block's byte range,
  carrying a `TableEditor` view.
- `TableEditor` holds the *projection*: `Vec<Vec<Cell>>` where each
  `Cell { view: EditorView, span: Range<u32> }` — the view is a value
  editor over the unescaped cell text (`<br>` → `\n`, `\|` → `|`), the
  span is the cell's byte range **within the table block** (cell-local
  coordinates; the inlay's own interval supplies the absolute base and
  shifts with edits like every interval).
- Column alignments come from the delimiter row and affect text alignment
  inside cells, not widths.

### Cell edits write through to the source

A keystroke in a cell must edit the *document*, not a copy. The inlay
contract has one channel for this: an inlay's `perform` may return a
text operation alongside its replacement view —

```rust
pub struct InlayOutcome {
    // the usual outcome: the replaced view, the repaint range/mode, the effects
    pub edit: Option<Operation>,   // in document coordinates
}
```

`Editor::perform_inlay` applies the returned operation through the
ordinary `apply_edit` path: the edit log records it, markup and layouts
shift, the reparse effect launches — undo, rebasing and repair all work on
cell edits with **zero table-specific cases**, because a cell edit *is* a
document edit.

The table editor also applies the same edit *locally* (to the cell's
`EditorView` and its spans) in the same `perform`, so the projection is
consistent with the source in the very frame the key lands — it never
waits for the round-trip.

Serialization details, both directions:

| source | cell view |
| --- | --- |
| `<br>` | `\n` |
| `\|` | `\|` (escaped pipe → literal pipe) |
| padding spaces around cell content | trimmed on parse, single-space re-added on write |

The write-back operation is computed against the *source* form (cell-local
offset mapped through the escape table to a document offset), so a typed
`|` becomes an inserted `\|` and a typed newline becomes `<br>` — Enter
(hard and soft) maps to a plain `"\n"` insertion before the write-through,
never through type assist: a cell has no structure to continue.

**The divergence rule**: a mutation the write-through cannot express yet
(forward deletes, undo inside a cell) must not apply locally either — a
local-only edit diverges the projection from the source, and the next
cell rebuild (a relayout, a reparse) silently reverts it. Such commands
are swallowed until they get a write-through mapping.

### Reparse vs. live cell state: adoption

Cell edits trigger reparses, and a reparse *replaces* the syntax markup for
the invalidated block — naively that would rebuild the table inlay from
scratch every keystroke, losing the focused cell and its caret.

The fix is **adoption**: when a markup splice inserts a replacement inlay
whose block coincides with an existing inlay of the same kind, the
existing inlay's *view* (and key — so focus routing survives) is adopted
and only its range updates. The table's live view already reflects the new
source (it applied the edit locally), so adopting it is not stale — it is
*more* current than a rebuild, which would only re-derive what the view
already knows.

Adoption is a small, generic contract on `Inlay` (match by kind + block
identity), not a table special case — nested editors and any future
stateful inlay want the same survival.

## The layout algorithm

The key property: **heights are a function of widths, and widths are a
function of content — there is no cycle** (no CSS-style percentage
heights). Layout is a deterministic two-pass; no iteration, no fixpoint.

### Pass 0 — cell intrinsics (cached)

Shape each cell once at unbounded width. Skia's `Paragraph` returns
exactly the two numbers table layout needs:

- `min_intrinsic_width` — the widest unbreakable token: the narrowest the
  cell can go without overflow;
- `max_intrinsic_width` — the natural width: the widest *line* (which is
  why `<br>` handling falls out for free).

Add cell padding; cache per cell, invalidated only when that cell's
content changes. Aggregate per column:

```
MIN_j = max over rows of min(cell)   clamped to [floor, cap]
MAX_j = max over rows of max(cell)   clamped likewise
```

`floor` is a few `ch` (empty cells still fit a caret); `cap` is a fraction
of the available width (~60%) so one pasted URL cannot starve every other
column — a capped column falls back to break-anywhere wrapping.

### Pass 1 — column widths (pure arithmetic)

With `W` = pane content width minus gutters and rules:

- `ΣMAX ≤ W` → every column gets `MAX_j` plus the slack distributed
  proportionally to `MAX_j` — the table fills the pane width.
- `ΣMIN ≥ W` → every column gets `MIN_j` (a capped column falls back
  to break-anywhere wrapping; there is no inner horizontal scroll).
- otherwise, distribute the squeeze by compressibility (the CSS auto
  algorithm):

  ```
  w_j = MIN_j + (W − ΣMIN) · (MAX_j − MIN_j) / Σ(MAX − MIN)
  ```

Quantize the results to the chrome's quantum grid — see *stability*.

### Pass 2 — row heights

Lay each cell's `EditorView` out at `w_j − padding`; a row's height is its
tallest cell plus padding; the table's height is rows plus rules. This is
the only pass that wraps text, and it only re-wraps cells whose column
width changed.

### Incrementality

On a cell edit:

1. recompute that cell's intrinsics (one paragraph shape);
2. recompute `MIN/MAX` for that column (O(rows) max-scan; tables are
   small — no clever structure);
3. column widths unchanged (the common case: aggregates are maxes, most
   keystrokes don't move the max) → re-wrap the *edited cell only*; if its
   height changed, the row height changes, the inlay's size changes, and
   that propagates through the **existing** inlay-resize path —
   `perform_inlay` → damage at the inlay → bounded repair — exactly like
   the animated demo inlay;
4. a column width crossed a quantum → re-wrap that column and propagate
   the size change the same way.

### Resize: the off-thread relay

A pane resize changes the width the table renders at, but cells keep
the wrap of the width they were LAID at — re-shaping them is real work
that never belongs on the UI thread. The paint pass is the detector:
`layout` compares the widths a relay would choose against the laid ones
(column arithmetic only), and while they differ every painted frame
answers with `TableCommand::Relayout { width }`. Its perform launches
`TableRelayoutEffect` — a clone of the view re-laid on the worker — and
the result lands as `TableCommand::Relaid`, which swaps the re-wrapped
cells in and repairs the covered lines. Both commands are **passive**
(`InlayEditing::passive`): machinery landing back at the view must
never steal the focus.

Guards: one relay per wanted width is in flight at a time (the lane
relaunches only when the wanted width moves), and every content
mutation re-mints the view's `version` — a landing whose version does
not match the live view's discards itself, and the next paint
re-detects whatever width lag remains. Freshly parsed tables lay at
their natural widths, so each table relays once when first displayed.

### Foreign edits: the parser is the truth

The live view mirrors the source only because every widget edit writes
through AND applies locally. An edit the widget did not make — undo,
find/replace, another pane, an agent — breaks the mirror, and no local
bookkeeping can restore it: the reparse can. Each rebuild's
`adopt_from` compares its freshly parsed `lines` against what the
previous incarnation believes it wrote (`previous.lines`); a match
means the live view is a faithful continuation (the landing's carry
keeps it, local novelty included), a mismatch means the rebuild is the
truth and the landing must NOT carry the live view over it
(`InlayView::carry_live`). Checkboxes answer the same question with
their `checked` belief.

### Stability while typing

Honest auto-layout jitters: a keystroke in the widest cell moves a column
boundary, and per-keystroke pixel shifts across the whole table read as
chaos. The mitigation is **quantized widths** (the chrome's quantum
grid): sub-quantum content changes move nothing. There is no
crossing animation — a width that crosses a quantum snaps; gliding
the boundary over an animation clock is a possible refinement, not a
mechanism the table has.

## Commands, focus, navigation

```rust
enum TableCommand {
    Cell { row: usize, col: usize, command: EditorCommand },
    InsertRow(usize), RemoveRow(usize),        // structure edits
    InsertColumn(usize), RemoveColumn(usize),
    Relayout { width: f32 },                   // the off-thread relay
    Relaid(Box<TableEditor>),
}
```

- The focused cell renders its caret; the table remembers `(row, col)` and
  the document's `EditorFocus::Inlay(key)` routes position-less events to
  the table.
- **Tab / Shift-Tab** hop cells (wrapping row to row); **arrows** move
  within a cell and hop at its edges (up at the first line → cell above);
  **Enter** inserts `<br>` (a newline in the view) — the markdown
  convention, since a source newline would end the table row.
- Click routing uses the computed grid: point → column by prefix sums of
  `w_j`, row by prefix sums of heights, then into the cell's view.

Structure editing — inserting and removing rows and columns
(`InsertRow`/`RemoveRow`/`InsertColumn`/`RemoveColumn`, the hover
affordances and the `table.insert-row-above`/`-below` commands) — is
each a plain text edit on the source (`| … |` line insertion, adding
a pipe per row) expressed through the write-through channel. There is
no alignment-editing affordance; alignment comes from the delimiter
row as typed.

## What this deliberately does not do

- **No second document model.** Cells project spans of the source;
  every mutation is an `Operation` on the document. Undo, the edit log,
  reparse rebasing and repairs treat table edits as what they are — text.
- **No manual column widths** — they would have to serialize into the
  source somehow; likely never.
- **No spanning cells** — markdown tables have none.
- **No nested tables** — markdown forbids them; the cell views get plain
  markup (bold/italic/code render via the cell's own markup, but a cell
  never hosts another Instead inlay).

## The perf invariant

Typing inside a cell stays within the keystroke budget: it is a
document edit plus one cell re-shape — nothing about a table makes it
linear. The oracle tests cover column widths for known content at
known widths, the three Pass-1 regimes, `<br>` height effects, the
write-through escape cases (`|`, `<br>`), undo through the edit log,
and the load-bearing one: a caret surviving the reparse round trip
through adoption.
