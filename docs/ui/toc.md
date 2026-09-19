# The table of contents

cmd-T shows the FOCUSED PANEL's structure in the side drawer: an
editor pane's document outline, a results panel's files-with-results
forest.

The outline derives from the PARSE, held as interval ids — never by
scraping presentation markup into absolute bytes. (An earlier cut did
exactly that — header styles, foldables, a `'{'` trim, snapshotted at
drawer open — and it was wrong on every axis: structure comes from
the parse, not from what happens to be styled; absolute bytes die
under typing; and a linear walk at ask time is work the parse already
did.)

## 1. Parse products split into channels

Parsing brings THREE products: token colors, block styles and hidden
ranges; foldables; and outline items. They live in SEPARATE interval
trees on the `Syntax`:

```rust
pub struct Syntax {
    pub language: String,
    pub tree: Option<Box<dyn SyntaxTree>>,
    /// DISPLAY markup: styled tokens, block styles, hidden ranges,
    /// inlays, child syntax markers — what shaping consumes.
    pub markup: Markup,
    /// The FOLDING intervals (docs/editor/folding.md): foldable body
    /// interiors. Plain keyed `Intervals` — no wrapper type.
    pub folds: Intervals<IntervalId, ()>,
    /// The OUTLINE intervals: structural items + presentation.
    pub outline: Intervals<IntervalId, OutlineItem>,
}

/// One outline item, as the LANGUAGE derived it at parse time — the
/// interval is the structural node's full extent (nesting is
/// containment), the payload its presentation.
pub struct OutlineItem {
    /// What the row shows — the heading text, "pub fn frobnicate".
    /// Derived from the tree at parse time, where the node is in
    /// hand; glyph-capped at derivation.
    pub title: String,
}
```

Why the split, beyond the outline needing a home:

- **Consumers stop wading.** The gutter's foldable query and the
  viewport build walk the folds tree — a handful of intervals —
  instead of filtering them out of 50k tokens; the outline effect
  walks outline items only. Channel queries are linear in THEIR
  channel.
- **The splice stays untangled.** Each channel splices independently
  against the invalidated ranges; a foldable-vs-token invalidation
  coupling (one shared tree once wiped token colors and forced an
  exact-range foldable splice special case) cannot exist — removing a
  channel's entries can never take another channel's with them.
- **Edits shift all three** at the one choke: `Markup::edit`'s
  recursion step forwards the operation into the syntax's markup,
  folds, outline and tree together. Nothing new to forget.
- **Channels are NOT display products**: their splices never join the
  invalidated ranges, never damage `DocumentLayout`, never trigger a
  repair — the gutter's chevrons and the outline's rows read fresh
  state per paint / per landing.

The derivation is ONE language job: `markup_for_changes` keeps its
signature; `MarkupBuilder` is the single facade the language
pushes into (`push_styled`, `push_foldable`, `push_outline(range,
OutlineItem)`) and the pipeline routes the builder's channels into
their trees at splice.

## 2. Who emits outline items

- **himarkdown**: `section` nodes — the tree already nests them by
  heading level, so containment IS the hierarchy; the title is the
  heading's inline text.
- **hisitter**: `with_outline_kinds(&[...])` — the `with_fold_kinds`
  recipe. The title is the node's text from its start through the
  grammar's `name` field (`type` for impls); a listed kind without
  either field is skipped. Grammar-driven — no character surgery.
- **hirust** registers `function_item`, `struct_item`, `enum_item`,
  `trait_item`, `impl_item`, `mod_item`.

Items rebuild for changed regions exactly like tokens (the ancestor
containment walk foldables already do); ids re-mint there, unchanged
regions' items ride the splice untouched. The channels splice against
**`changed`** — the exact window the language re-derived them for —
never against `invalidated` (the display-damage set: changed ∪
re-derived token spans; a token overhanging the changed range would
remove entries the emission walk never re-visits).

## 3. The drawer: `PanelView::drawer_view`

```rust
// On PanelView (defaulted — panels without structure offer nothing):
fn drawer_view(&self, store: &Store, window: WindowId)
    -> Option<Box<dyn ModalView>>;
```

`toc.toggle` ("Table of Contents", cmd-T; the file tree sits on
cmd-E, its toolbar button untouched) shows the focused
panel's answer through the side-drawer machinery ([workspace.md](workspace.md));
a second cmd-T dismisses. `Panel::drawer_view`: the Editor arm
answers an `OutlineView` over the shown document; `Panel::Plugin`
defers through the erased face. Search and References answer the
files-with-results forest (`TocView::for_locations` over
`LocationList::result_locations`).

Both drawers are REAL TREES on the UNIFIED LIST ([list-tree.md](list-tree.md)): a
`ListView<TreeRow, K>` carries the nesting and the selection as
interval data; `TreeItemView` decorates rows with the chevron, inset
and expansion state; `himark::Forest` builds the slices and holds the
fold memory (consumer state — the list sees folded rows as deleted).
Keys are DOMAIN keys: the outline addresses by `(SyntaxId,
IntervalId)` directly, the trees by `ResourceLocation` — no id
minting anywhere. Keyboard: up/down walk visible rows (unkeyed rows
skipped), left/right fold/unfold (left on a leaf walks to the
parent), Enter activates (a file opens, a directory folds). The
results forest nests found files under their real directory
sub-structure, folders before files, with single-child directory
chains COMPACTED into one node ("a/b/c") so depth spends on real
branching only.

The workspace and repository trees ride the same list lazily: an
unlisted directory is a single keyed row whose toggle asks (the
pending set dedups); the landing splices header + children as ONE
slice with its nested spans; a fold splices the single row back — the
structure spans die with the rows, so re-expanding re-fetches (the
freshness story) and hifiles drops the folded subtree's watches.
Clicks arrive focus-wrapped from the list; `himark::tree_interaction`
unwraps them. Row text dims by an explicit `dim` flag, independent of
pickability (a files-tree directory is unpickable yet full-strength;
a results band or vcs note is dim).

## 4. `OutlineView`: ids in, rows landed, live while shown

The outline drawer holds **interval ids, never bytes**:

- **Derivation is a BACKGROUND effect.** While shown, the view owns
  an `OutlineEffect` lane: capture the document's outline trees
  (persistent clones — O(1)) with their syntax bases plus the
  (revision, markup_generation) stamp; the worker walks the items —
  linear in ITEMS, not in tokens — sorts by position, computes depth
  by containment, and lands rows of
  `(SyntaxId, IntervalId, depth, title)`. The landing rebuilds the
  forest from the flat pre-order (containment depths ARE the
  nesting) and swaps it wholesale — folds and selection carry over
  through stable per-address tree ids (`OutlineView::keys`), so a
  reparse re-minting an item resets its fold and nothing else (the
  degradable rule).
- **Liveness is the paint-reconcile discipline** ([UI.md](UI.md)): the
  drawer's paint pass compares the document's live stamp against the
  rows' — a mismatch answers the relaunch command, exactly as a
  stale-width editor reports its resize. Typing repaints the window,
  the next frame notices, the lane relaunches (superseding the
  in-flight run), fresh rows land. No subscription machinery; a
  closed drawer's landing drops with the side slot.
- **A pick resolves the id against the LIVE tree**:
  `Markup::resolve_outline(SyntaxId, IntervalId)` — find the
  syntax, find the key in its (small) outline tree — answering the
  interval's CURRENT range; the jump goes through the navigation
  door as an ordinary `EditorPlace` (history records; same-file =
  the in-place caret move). An id the reparse re-minted resolves to
  nothing → the pick is a quiet no-op (the chunk-action rule), and
  the very next landing carries the fresh ids anyway.

Rows render through the shared tree composition (§3): pickable rows
jump on click with the disclosure zone folding; dim unpickable bands
(directories) fold on click anywhere.

## 5. Cost invariants

- UI thread: id resolution (log-time syntax lookup + a small tree),
  the stamp compare at paint — nothing linear in the document, ever.
- Worker: the outline walk, linear in outline ITEMS (the channel
  holds nothing else).
- Parse: emission rides the existing derivation — no second pass,
  no new effect kind on the parse side.

## 6. Deliberately absent

- Id ADOPTION across reparses (same range+title keeping its id — row
  identity for selection stability): ids re-mint in changed regions
  and the live refresh covers it.
- Kind glyphs on rows: `OutlineItem` holds only a title; a kind field
  is the extension point when glyphs come.
