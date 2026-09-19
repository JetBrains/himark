# Syntax folding

Collapse a function's body behind a tiny "…" chip; toggle it from the
gutter. The parse says what CAN fold, one inlay says what IS folded,
and nothing else stores any state.

## 1. Foldables — a parse channel

Foldables ride the parse as a CHANNEL beside tokens: languages emit
ranges through `MarkupBuilder::push_foldable`, and the landing files
them into the syntax markup's `folds` interval set
(`Markup { folds: Intervals<IntervalId, ()> }`), where they shift
through edits like every interval and rebuild with every reparse
splice. The range is the construct's **body interior** — the
signature and the delimiters are code and never leave the display.

hisitter derives interiors from each plugin's kind list
(`TreeSitterLanguage::with_fold_kinds` — hirust registers
`["function_item"]`). `fold_interior` resolves the body through the
grammar's `body` field, else the last `*_body`-kind named child, and
then picks the rule by shape:

- **delimited bodies** (the body's first child is a delimiter token)
  fold between the delimiter tokens, exclusive;
- **indentation bodies** (python's `block`, ruby's
  `body_statement`) fold from the end of the token before the body
  (the `:`) through the body's end — the header line joins the chip
  and a trailing `end` keeps its own line.

Single-line bodies emit nothing; markdown emits nothing.
`Document::foldables_in` / `foldables_into` merge the root syntax's
channel with every nested syntax's (offsets rebased), so a rust
fence's functions fold inside a markdown document.

## 2. Folded state IS the inlay — never denormalized

Foldables are SYNTAX info — document-wide. Folded-ness is a VIEW
concern: each editor owns a fold markup of its own
(`Editor::folds: Option<MarkupId>`), minted lazily on the first
toggle (`fold_markup_of`) as an owned markup — merged into that
editor's display only, dying with the editor — so a split's halves
fold independently. A fold is an `Instead(Inline)` chip inlay in that
markup **with the same range as its foldable**; there is no fold
table, no flag, no denormalized set anywhere.

- **Folded?** — derived: `fold_matching` finds a fold-markup inlay
  whose range equals the foldable's live range. Both intervals are
  non-greedy over identical ranges, so every edit and reparse shifts
  them in step and the match survives by construction.
- **Toggle** (gutter chevron) → `EditorCommand::ToggleFold { range }`
  → `Document::toggle_fold`: a matching inlay flips its departure;
  none standing and the syntax still offers the range →
  `push_inlay`; a stale range the syntax no longer offers is a quiet
  no-op.
- **Unfold** (chip click) → the chip answers `FoldCommand::Unfold`;
  the `Inlay` command arm intercepts it and starts the chip's
  departure by its key.
- The chip ANIMATES: it appears at the collapsed run's full height
  and eases down to the chrome height (160ms, ease-out); departure
  reverses the motion and `remove_inlay` fires when it comes to
  rest. The gutter chevron's rotation (`LineFoldable.spin`) reads the
  same animation.
- An orphan (reparse moved the function under a standing fold) keeps
  its chip — visible, click-removable; the gutter just shows no
  chevron for it.

## 3. `Instead` has two kinds

`Instead` means what it says — replace a RANGE of text with a widget
— but most consumers cover whole blocks, so line replacement is a
mode of its own rather than the only meaning:

```rust
InlayMode::Instead(InsteadKind)
enum InsteadKind { FullLine, Inline }
```

- **`FullLine`** — replaces the full hard lines from the one holding
  the interval's start through the one holding its end. The widget's
  MINIMUM width constraint is the editor width. Tables, diagram
  blocks.
- **`Inline`** — replaces exactly the covered range with a widget in
  the text flow (min-width 0, max the remaining editor width). The
  covered newlines are gone from the display, so the text after the
  interval JOINS the same display line: a folded `fn demo() {` +
  chip + `}` render as ONE soft line, wrapping at width like any
  other.

Neither kind ever shapes — or even READS — the text it covers.

## 4. The join, mechanically

Inline replacement happens where layout produces its units, not in
per-line shaping. When the item stream reaches a hard line containing
the start of an `Inline` replacement that crosses the line's end, the
unit EXTENDS: display = `[line start .. interval.start]` + inline
placeholder (the chip) + `[interval.end .. that line's end]`, chaining
if the tail holds another replacement. The unit is assembled from the
VISIBLE SEGMENTS only (`DisplayText::build_segments` — the raw↔display
maps jump across the gap) and soft-wraps normally. The covered
interior lines emit ZERO-HEIGHT layout elements that carry their
bytes — positions stay addressable, nothing shapes. Costs stay
honest:

- the interior is never materialized — a multi-megabyte body folds in
  element-emission time (the `fold-toggle` perf test pins it);
- tail reads are newline-metric seeks (log) plus one line's bytes,
  grid-capped like any monster slice, so repairs cut deterministically;
- the render/caret paths read the same segments (`ShapedLine` skips
  the gaps), so hit-testing, selections and IME rects map through the
  jumping raw maps for free.

When a background-repaired layout swaps in over a live one, the range
a fold covered is noted (`swap_fold`) — the split diff drains it
(`take_swap_fold`) and widens its pending row alignment so the two
halves re-align across the collapsed run.

## 5. Rendering — the viewport carries it

`EditorViewport::build` sweeps the visible band in CONTIGUOUS VISIBLE
SEGMENTS: a zero-height byte-carrying element (a fold's interior, a
windowed fragment's prefix) ends the current segment and drops the
line-marks sweep, and the next visible line re-seeds it past the gap
— the gap's markup is never pulled, so one fold squashing a large
file costs nothing per frame.

Per line the viewport notes the foldable whose range starts within it
and whether the fold inlay stands
(`ViewportLine.foldable: Option<LineFoldable { range, folded, spin }>`).
Foldables are pulled through `Document::foldables_into` in bounded
chunks (4K of bytes at a time, reused across the lines the chunk
covers), and only when a gutter is showing — a gutterless editor
never pays for the query.

- **Gutter**: a chevron in the fold column right of the numbers —
  down when unfolded, rotating to point right as the fold animates
  closed; click hit-tests the viewport's lines (`foldable_at`) and
  emits the toggle.
- **Text**: `fold::FoldChip` — just dots, tiny, clickable; chrome is
  `ui.fold_chip`.

## 6. Deliberately absent

Markdown section folding; fold-all / unfold-all; caret behavior
entering a folded range (reveal unfolds nothing); persistence across
sessions.
