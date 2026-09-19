# One list: `ListView<V, K>` with tree structure as data

One component (frontend/imba/src/list.rs) unifies the flat list and
the tree, so that selection (single and multi), speed-search and the
cursor are implemented ONCE, over one addressing model — not per
component.

## 1. Motivation: the combinatorics

A separate tree component means a second addressing model beside the
list's row indices — private node ids, and per-consumer minting layers
translating `key → id → key` around every surface. Selection then
exists once per component, and every further row feature —
multi-selection, speed-search, drag ranges — multiplies against the
list/tree split again.

The observation: a tree IS a flat list whose nesting is data. Make the
nesting an interval set on the list, give rows caller-meaningful keys,
and every row feature lands once, tree-blind.

## 2. The shape

```rust
pub struct ListView<V: Clone, K: Clone + Eq + Hash = ()> {
    /// The rows: heights cached in the rope, O(log n) positional
    /// queries, the substrate splices land in.
    items: Rope<ListElement<V>, ListMeasure>,
    /// The NESTING, as data: one interval per node, covering exactly
    /// the flat rows its subtree occupies, keyed by the caller's OWN
    /// key. Empty for flat lists (a flat list is a tree of leaves;
    /// nothing branches).
    structure: Intervals<K, ()>,
    /// The selection, when configured (`with_selection(style)`):
    /// intervals over the SAME row-index space, keyed by the selected
    /// node, plus the cursor and its armed reveal. Splices shift it
    /// like they shift the structure — selection follows insertions
    /// for free, and rows that leave take their selection with them.
    selection: Option<SelectionState<K>>,
    /// Speed-search matches: the third interval reader of the same
    /// index space (docs/ui/speedsearch.md).
    matches: Intervals<K, ()>,
    // …focused row (event routing), splice animations, generation.
}
```

One index space — flat row indices — three interval readers of it: the
rope (heights/geometry), the structure (nesting), the
selection/matches. Every splice is ONE `Operation` applied to all of
them.

### Keys are the caller's, and they mean something

`K` is a real domain key: the file tree's key is the location (the
whole path IS the key), the changes tree's the row's
`ResourceLocation`, the outline's its `(SyntaxId, IntervalId)`
address, search's group key the `DocumentId`. Requirements are what
`Intervals` already asks: `Clone + Eq + Hash`.

This removes the id-minting layer wholesale — no private `NodeId`
namespace, no reverse maps in the consumers. The list speaks the
caller's keys:

```rust
fn row_range(&self, key: &K) -> Option<Range<usize>>;  // node → rows
fn key_at(&self, index: usize) -> Option<&K>;          // row → innermost node
fn depth_at(&self, index: usize) -> usize;             // nesting count
```

## 3. Splices take a SLICE, not a Vec

```rust
/// A pre-built block of rows with its own nested structure, in LOCAL
/// coordinates (rows 0.., intervals over them). Built anywhere —
/// including on a WORKER: a 100k-entry listing shapes its rope and
/// spans off-thread, and the UI-thread splice is logarithmic.
pub struct ListSlice<V, K> {
    rows: Rope<ListElement<V>, ListMeasure>,
    spans: Vec<Interval<K, ()>>,
}

impl ListView<V, K> {
    /// THE edit: replace `range`'s rows with the slice. Rope surgery
    /// is O(log n) (concat, not per-row insert); covered structure,
    /// selection and match intervals die with their rows; survivors
    /// shift by the one Operation; the slice's spans graft in REBASED
    /// by the insertion offset, one bulk insert.
    pub fn splice_slice(&mut self, range: Range<usize>, slice: ListSlice<V, K>);
    pub fn splice_slice_animated(&mut self, range: Range<usize>, slice: ListSlice<V, K>);
    // splice / splice_animated: the (row, height)-iterator conveniences.
}
```

What must be true at every call site: the UI thread never walks the
slice's ROWS — rows accumulate in the slice O(1) and SEAL into the
rope once at the build site (`ListSlice::sealed`, a worker for monster
slices); the splice is the rope concat plus the interval graft.

Because a `ListSlice` carries its own nested spans, one splice lands a
whole subtree — correctly, atomically. The alternative (every inserted
row minted as a single-row span) forces expanded subtrees to be built
level by level, or inner branches orphan their children on the first
fold; carrying the spans in the slice deletes that class of bug.

### The animation adapts to the block

A splice whose new block is no taller than what it replaces animates
by SCALING: the inserted rows land pre-scaled and ease to their
natural heights. A GROWING splice UNROLLS instead: one animated edge
sweeps down the inserted block, rows appear at their natural height as
the edge passes them — no squish, no intersections — and everything
below the edge simply is not there yet. Per frame that is ONE extent
update (O(log n)) plus the ordinary visible-slice paint; the unrolled
prefix is capped (rows beyond the cap land at full height at once), so
the animation never lays out more per frame than the cap admits,
whatever the slice's size. Folds mirror in reverse.

## 4. Tree presentation: `TreeItemView<V>`

The list knows structure, not chevrons. Rows that ARE tree nodes wrap
in a decorator (frontend/himark/src/tree_item.rs):

```rust
pub struct TreeItemView<V: Clone> {
    inner: V,
    depth: u16,            // baked at build; pixels = depth × chrome indent
    expanded: Option<bool>, // Some = disclosure; None = leaf row
    toggle_on_body: bool,   // an unpickable row's body toggles too
}

pub enum TreeItemCommand<C> { Toggle, Inner(C) }
```

`TreeItemView` draws the chevron and inset and HOLDS the expansion
state — the row itself knows how it was built (`leaf`, `branch`,
`toggling_on_body`). A toggle is just a command to the consumer; the
consumer answers with the eventual state change and the splice: an
eager tree splices its known children in the same perform, a lazy one
launches its fetch and splices at the landing — expand asks, collapse
forgets. Fold memory — which subtrees stay folded across rebuilds — is
the consumer's state (`himark::Forest` holds it for the eager trees);
the list only sees splices. Flat lists never see any of this: no
decorator, no structure intervals, zero cost.

## 5. Selection, once

Configured per list (`with_selection(style)`), OFF by default — value
hosts and layout-only lists pay nothing.

- **Model**: `Intervals<K, ()>` over row indices. A selected node's
  interval covers its own row (not its subtree — selecting a folder is
  selecting its row). Multi-selection is simply several intervals
  (`toggle_selected`, the cmd-click gesture).
- **It moves by construction**: a splice above shifts it; a fold whose
  rows leave deletes the covered entries — and the component applies
  THE policy: the selection RE-POINTS to the folded node. One rule,
  nowhere hand-rolled.
- **Cursor**: the single "focused" selection entry the arrows move
  (`cursor_step(±1)` over visible rows), Enter activates, the reveal
  protocol keeps it on screen (`reveal_row` is the keyed reveal,
  independent of selection) — one implementation, shared by flat and
  tree consumers.
- **Ops**: `select_only(key)`, `toggle_selected(key)`,
  `selected() -> keys`, `is_selected(&key)`, `clear_selection()` — all
  O(log n).

`ListView.focused` (event routing — the list's `focus_data` hands the
keyboard to the focused row by state) is NOT selection and stays
separate: routing is focus, selection is model.

ONE paint everywhere: the shared row highlight
(`himark::rows::selection_style`) on trees, peeker, palette and combo
alike; surfaces differ only in row metrics. Matched rows (speed-search)
tint with the same fill at reduced alpha; the cursor keeps the full
selection paint.

## 6. Speed-search

What speed-search needs, the unified list provides uniformly: rows
enumerable with their keys, the cursor movable to an arbitrary KEY
(`select_only` + reveal), matches as one more interval set over the
same index space. No tree/flat split anywhere in it — the full design
is [speedsearch.md](speedsearch.md).

## 7. What this replaces

Structure, selection, cursor and reveal live in the ListView; the
chevron lives in `TreeItemView`. What remains around them:

- `RowList` (frontend/himark/src/rows.rs) is a thin assembly over
  `ScrollView<ListView<LabelRow, usize>>` — the shared
  peeker/palette/completion convenience — with no selection or reveal
  machinery of its own. Its note row is unkeyed, therefore never
  addressable.
- Tree consumers build slices, not components: `himark::Forest` turns
  a forest into a `ListSlice` (relandings pass their collapsed set
  into it) and keeps the fold memory; lazy trees own their pending
  asks and watches — a `Toggle` command reaching the consumer IS the
  protocol. From the ListView's point of view, folded rows are simply
  deleted.

Consumers (toc, hifiles, hichanges, peeker, LocationList) keep their
surfaces; their trees address by domain keys directly.

## 8. Cost invariants

- UI-thread splice: O(log rows + inserted nodes). Slice row building
  is linear but runs wherever the caller wants — including a worker.
- Selection ops, key/row resolution, depth: O(log n).
- Flat lists: structure empty, selection None — no cost against a
  plain list; the generics compile away.
- Nothing iterates all rows per frame; paint stays the visible-slice
  walk — the splice animations included (one edge or scale value per
  frame, never a per-row height pass).
