# The editor gutter, and the viewport as data

The stripe left of a pane editor's text: line numbers, fold controls,
and diff stripes, with a click road into the before-card. This
document covers the shape that makes it possible — rendering split
over a shared per-frame data structure — and what each column draws.

## 1. `EditorViewport`: build once, read everywhere

The gutter is a SEPARATE component beside the text (the text pans
horizontally, which must never move the gutter), yet everything it
draws — where each line sits, which rows start hard lines, the text
baseline — is exactly what the text pass computes. Computing it twice
would double the one walk that matters; computing it inside one
widget's paint would trap it there. So the computation is a value:

```rust
// crates/editor/src/viewport.rs
pub(crate) struct EditorViewport {
    lines: Vec<ViewportLine>,   // geometry + marks + shaped paragraph per row
    inline: Vec<TextDecorationInterval>,  // shared arrays the rows slice
    hidden: Vec<Range<u32>>,
    selections: Vec<Range<u32>>,
    layout_width: f32,
    tail_spacer: Option<(f32, f32)>,
}
```

`ViewportLine` carries the element's byte range, top/height/spacer,
`text_top`/`x`, block marks and box extents, inlay metrics, the inline
and hidden slices, the shaped skia paragraph (`None` for rules and
`Instead`-covered rows), the hard-line flag, the fold state, the first
row's exact baseline y, and the row's diff classification
(`diff: Option<DiffLineKind>`).

The cost discipline: the build is ONE walk over the visible slice of
the layout rope — View × log(Doc); line numbers are newline-METRIC
seeks on the text rope plus a one-byte "is the previous byte a
newline" read per visible row. Nothing is O(document). Within the
walk, the line-marks sweep runs per contiguous VISIBLE segment: it is
dropped whenever a collapsed run appears — an element with
`height <= 0.0` and a positive byte size: a fold, an
`Instead(FullLine)` row, a windowed fragment's hidden prefix — and
lazily re-seeded (`get_or_insert_with`) at the next visible line, so
collapsed gaps are never traversed. The `StripeWalk` is NOT re-seeded
the same way: it is created once before the loop and walks straight
through the gaps, advancing its interval cursor in lockstep with the
rows.

**Built once, per frame.** `EditorView`'s lazy body mints one plain
`Rc<SharedViewport>` inside the `lazy()` closure and clones it into
both widgets — `EditorGutterView` and `EditorCoreView`; it dies with
the frame's widget tree. The build is FORCED at display/realize time:
visible-inlay placement and projected overlays consume it during
layout, so by the time either paint runs, both hit the memoized
`RefCell`. There is no "whichever paints first builds" race — the
build precedes both paints structurally.

Focus is baked in at build time: the `focused` bit derives from the
frame store's `FrameFocus` seat — `document.seat_key(editor) ==
Some(seat)`, falling back to the editor's own focus being `Text`. There
is no paint-time focused upgrade: `Event::Paint`'s own focused flag
affects only caret painting, and the gutter ignores it. An unfocused
editor bakes an EMPTY selections vector, which also changes the
shape-cache key — selection tinting drops out of the shaped rows, not
out of a paint-time branch.

The `Rc` is plain owned data dropped with the widgets at the end of
the event — never a bare `Arena::alloc`, because that door skips
`Drop` and would leak every retained skia `Paragraph` (`Arena::boxed`
— the drop-running arena box the widget tree itself rides — is the
door for arena-placing Drop-carrying frame data; `DisplayText`/
`ShapedLine` are owned `String`/`Vec` for the same reason). Non-paint
events never touch the slot, so keystroke dispatches pay nothing.

## 2. The split view

```
EditorView { document, editor, reports_geometry, location, gutter_width, base }
 └─ lazy
     ├─ EditorGutterView        (x: 0,      width: gutter_width)
     └─ container               (x: gutter_width)  ← the horizontal-pan seam
         ├─ EditorCoreView      (text, events, geometry reports)
         └─ …visible inlays…
```

- `gutter_width: 0.0` means no gutter — value editors, inputs, diff
  halves, search rows are untouched. The CREATOR resolves the width:
  `EditorIdView::with_gutter()` (pane nodes only) reads
  `theme.ui().editor_gutter.width` at gather, and only a gutter-bearing
  editor joins the stripes diff (`EditorView.base`) — stripes need
  BOTH the diff join and a gutter.
- The text column narrows by the gutter: the core's geometry report
  targets `constraints.max.width - gutter_width`, and the pane mount
  fallback (`fallback_pane_editor_width`) subtracts it so a fresh pane
  does not pay a first-paint resize.
- Horizontal pan lives inside the core container: a no-softwrap editor
  clips its text to a window of the target width and places the core
  at `-scroll_x` inside it, while the whole window sits at
  x = gutter and the gutter column stays put at x 0 — the seam is the
  reason the split exists.
- Popups and projected overlays anchor from the same origin the text
  does: `(gutter - scroll_x, 0)`.
- Events translate through the container placement like any nesting;
  the gutter answers its own clicks (§3).

## 3. What the gutter draws

Drawn by `EditorGutterView` from the shared viewport:

- **Line numbers** on the hard line's FIRST soft row, right-aligned
  against the fold column (at `fold_right - fold_size`), ON THE TEXT'S
  BASELINE — `ViewportLine.baseline` is the shaped paragraph's
  first-row baseline, so alignment is exact by construction, headers'
  larger faces included. Text-less rows (horizontal rules) center by
  the number font's own metrics; `Instead`-covered rows (folds,
  tables) show no number.
- **Fold controls**: a chevron per foldable row, rotating with the
  fold's spin animation; a click outside the stripe band answers
  `ToggleFold`.
- **Diff stripes**, when a stripes diff is attached (the tracked base
  diff, docs/editor/diff-stripes.md): per-row classification comes from a
  `StripeWalk` — an interval walk over the diff markup's hunk
  intervals (`StyleId::DiffAdded`/`DiffModified` spans, zero-length
  `DiffDeleted` deletion markers), built only when the gutter is on
  and advanced in lockstep with the viewport rows. The rules: a
  full-row `DiffAdded` span classifies the row `Added` — `Modified`
  instead if a deletion marker sits at the row's start; a partial add,
  any `DiffModified` span, or an interior deletion marker classifies
  it `Modified`; a bare marker at the row's start classifies it
  `DeletedAbove`. `Added`/`Modified` paint rects at
  `x = stripe_inset`, `stripe_width` wide, the full line height;
  `DeletedAbove` paints a right-pointing triangle of size
  `stripe_width * 2` centered on the line's top edge. A click within
  `stripe_inset + stripe_width * 2` (stripes attached) answers
  `ToggleBeforeInlay { at }` with the row's first byte — the
  before-card door (docs/editor/diff-stripes.md §6).

Chrome is `ui.editor_gutter` (`width`, `number_size`, `number_color`,
`pad`, `fold_size`, `stripe_width`, `stripe_inset`, and the three
`stripe_*` colors), 2x-tuned in both theme files; the number face is a
process-wide cached typeface (the fonts rule).

## 4. Zero-byte elements at a deletion boundary

A byte-accounting rule any pane narrowing exercises:
`DocumentLayout::delete_bytes` must not stop when its seek lands on a
ZERO-BYTE element (a paragraph's ghost/badge row) sitting exactly at
the deletion offset — stopping there silently drops the deletion's
tail, the layout keeps bytes the text lost, and every element boundary
after sits mid-character (the release-build `from_utf8` abort class).
The loop steps past zero-byte elements to the element that carries the
bytes, so byte accounting keeps them intact. Regression tests live in
`document_layout.rs`; the narrow-pane fuzz
(`typing_and_scrolling_survive_the_reparse_pipeline`) is the
end-to-end oracle.
