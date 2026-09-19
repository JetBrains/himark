# sumtree — the persistent accumulator tree

`frontend/sumtree`, an isolated zero-dependency crate: a persistent
B+ tree (branching 32 / leaves 64, `Arc` path copying) generalized
from additive metrics (the rope's algebra —
[rope-and-intervals.md](rope-and-intervals.md)) to any ASSOCIATIVE
operation. Every child edge
caches its subtree's element count and a monoid `Summary` (`empty` +
`add`) — no subtraction anywhere, which is exactly what admits max
and min. Range queries combine cached summaries along the two
boundary paths: an RMQ in `O(log n)`.

## Where it sits — and where it does not

sumtree's one consumer in the workspace is the editor's width tree
(`frontend/editor/src/width_tree.rs`): horizontal-extent maintenance.
Elements are a document's lines, the summary carries `max_width`
beside additive sizes:

- the whole tree's summary is the widest line — the horizontal scroll
  extent of a non-softwrapped layout, `O(1)` after any edit;
- `summary_in(range)` is the widest line of any range — what sizes the
  rectangular marker frame boxing a code block;
- edits arrive as BULK splices and the untouched spans ride over as
  whole shared subtrees.

Text and `DocumentLayout` do NOT ride sumtree. They ride a different
crate, `frontend/rope`: `Rope<T, M: Measure<T>>` over fixed-rank
`u32` metric arrays (`Metrics<const RANK: usize>`) with SUBTRACTIVE
counters (`add_assign`/`sub_assign`), no monoid summaries — the same
32/64 fanout, plus a `Cursor` with metric `seek`, `insert`, `delete`
and node splitting. Additive-and-invertible metrics belong in rope;
sumtree exists for the summaries subtraction cannot maintain.

## The shape

```rust
pub struct SumTree<T: Item> {
    weight: Weight<T::Summary>,     // len + summary of the whole tree
    root: Arc<Node<T>>,
}

enum Node<T: Item> {
    Internal { height: u8, children: Vec<Child<T>> },  // ≤ 32
    Leaf(Vec<T>),                                       // ≤ 64
}

struct Child<T: Item> { weight: Weight<T::Summary>, node: Arc<Node<T>> }
struct Weight<S>      { len: usize, summary: S }
```

## The vocabulary

| concept | shape | role |
|---|---|---|
| `Item` | `Clone + summary()` | an element; atomic — boundaries are element boundaries |
| `Summary` | monoid (`empty`, associative `add`) | what every edge caches; the RMQ answer |
| `Dimension` | `from_summary` + additive ordered `add` | what positions are made of (byte size, accumulated width) |
| `Bias` | `Left` / `Right` | which side of an exact boundary a seek lands on |
| `Seek<D>` | `{index, start}` | a `find` answer: the element and where it begins |
| `Splice` | `{range, insert}` | one edit of a bulk `splice` stream, ascending, non-overlapping |

Element COUNT is structural (`Weight.len`, cached beside the
summary), so index-addressed operations need no `Dimension`.
Dimension offsets are RELATIVE — sums of element sizes — so a middle
insert shifts every later position for free, and `find(offset, Bias)`
seeks an offset back to its element in `O(log n)` (`Bias::Left`/
`Right` picks the side of an exact boundary; zero-sized elements sit
AT their offset).

## The operations

| operation | cost | note |
|---|---|---|
| `from_iter` | `O(n)` | bottom-up bulk build, immediately balanced |
| `summary()` | `O(1)` | precomputed whole-tree accumulator |
| `summary_in(indices)` | `O(log n)` | the RMQ |
| `summary_between(dims)` | `O(log n)` | RMQ over a dimension range; elements straddling a boundary count whole |
| `find` / `offset_of` | `O(log n)` | offset ↔ element |
| `set(index, item)` | `O(log n)` | the point update width maintenance lives on |
| `append` | `O(log n)` | ONE concat; rebalances by height, repacking the junction so churn cannot stack slivers |
| `slice(range)` | `O(log² n)` | collects the boundary subtrees, then folds them with sequential concats |
| `splice(edits)` | `O(k log² n)` | k ascending edits; the parts (shared subtrees between splices + inserts) fold the same way |
| `iter` / `iter_in` | `O(log n)` descent | in-place walks, no slicing |

`slice` and `splice` are NOT `O(log n)`: both collect their parts and
assemble by folding pairwise concats, each concat `O(log n)` — the
log² is real, and acceptable because both are bulk operations off the
per-keystroke path.

Persistence is by construction: every operation returns a new tree
sharing structure with the old one; snapshots are `Clone` (two words)
and cross threads (`Send + Sync`).

## Performance gates

The `sumtree` lane of the perf harness (`sumtree_bench` in
`src/tests.rs`, recording to `perf.csv`) pins build, range-max
queries, bulk splices, point sets and dimension seeks over a
200k-element tree. The million-element churn probe
(`million_element_churn_probe`, `--ignored --release`) runs 500
rounds of bulk splices with model-verified queries and a height
bound throughout.
