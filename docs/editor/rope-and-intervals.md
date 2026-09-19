# Ropes and intervals: the positional substrate

Two zero-UI crates carry every positional fact in the engine:
`frontend/rope` (a generic metric-measured rope) and
`frontend/intervals` (an edit-tracking interval set). Text, edit
operations, layout, markup, folds, damage — each is one of these two
shapes with a domain-specific measure. Both are persistent: a clone
is `Arc` bumps, so the store's copy-on-write commits and every worker
snapshot ([document.md](document.md)) stay O(1).

## The rope: `Rope<T, M: Measure<T>>`

A persistent B-tree over elements of `T` (frontend/rope/src/node.rs):

```rust
enum Node<T, M> {
    Internal(Vec<Child<T, M>>),   // ≤ 32 children
    Leaf(Vec<T>),                 // ≤ 64 elements
}
struct Child<T, M> {
    metrics: MetricsWithLength<M::Metrics>,  // element count + measured metrics
    node: Arc<Node<T, M>>,                   // shared; edits path-copy
}
```

`Measure<T>` maps an element into a small vector of ADDITIVE metrics
(`Metrics`, addressed by `MetricId`). Every child edge caches its
subtree's totals, which buys the whole API:

- **`Cursor::seek(metric, value, mode)`** — position by any metric in
  O(log n): byte → line, line → byte, y → element, old-offset →
  new-offset, all the same walk down cached totals.
- **`position()` / `element_metrics()`** — the inverse: where am I,
  in every metric at once.
- **`insert` / `delete` / `split`** — local splices with path
  copying; untouched subtrees are shared with every earlier
  snapshot.

One structure, four load-bearing instantiations:

| rope | element | metrics | the seek it sells |
|---|---|---|---|
| `text::Text` | `u8` | bytes, newlines, UTF-16 units | byte ↔ line/col ↔ UTF-16 ([document.md](document.md)) |
| `operation::Operation` | `Op` | old length, new length | `transform_offset` across an edit ([diff.md](diff.md)) |
| `DocumentLayout` | `LayoutElement` | bytes, height | `byte_at_y`, `height_before` ([document.md](document.md) §layout) |
| imba `ListView` | row | index, height | row ↔ y for million-row lists ([../ui/list-view.md](../ui/list-view.md)) |

The rationale is the metric algebra: because metrics are additive and
cached per edge, "convert a position between any two coordinate
systems" is always one seek — never a scan, never a side index that
can drift ([../code-style.md](../code-style.md): resolve through the
rope). An `Operation` being itself a rope is what makes diff
composition and offset transformation O(log n) per keystroke
([diff.md](diff.md) decision 2).

## The intervals: `Intervals<K, V>`

An id-keyed set of ranges that stays correct under edits
(frontend/intervals/src/lib.rs):

```rust
struct Interval<K, V> {
    range: Range<u32>,
    greedy_left: bool,    // does an insertion AT an edge grow me?
    greedy_right: bool,
    key: K,               // stable identity, find_by_id / remove
    value: V,
}
```

- **`edit(steps)`** — the one verb everything depends on: an edit
  operation lowered to `Retain/Insert/Delete` steps shifts every
  range, with greediness deciding edge capture. Features anchor
  meaning to ranges once; the structure keeps every anchor true
  through every keystroke, with no feature knowing about any other.
- **`insert` / `remove` / `find_by_id` / `ordered_keys`** — id-keyed
  CRUD; by default empty intervals vanish (`keeping_empties` opts
  out for markers that must survive their content).
- **`graft(offset, other)`** — mount a whole child structure at an
  offset: how nested syntax mounts compose markup
  ([markup.md](markup.md), [highlight.md](highlight.md)).

The instantiations: both lanes of every `Markup` entry (styles and
structure — [markup.md](markup.md)), folds and outline entries,
fragment sets, and — pleasingly — the layout's own damage set
([document.md](document.md) §layout): dirty ranges are just greedy
intervals that edits shift like any other anchor.

## The third shape: the sum tree

Where a metric is not additive — the widest line in a range is a
`max`, and max has no subtraction — the rope's algebra is not enough.
[sumtree.md](sumtree.md) is the same persistent B-tree generalized to
any monoid summary; the layout's `WidthTree` rides it. If your
summary is additive, use the rope; if it is merely associative, the
sum tree.
