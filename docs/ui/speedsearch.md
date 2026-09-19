# Speed-search over the unified list

The feature the list-tree unification ([list-tree.md](list-tree.md)) was ground work
for: type into any list or tree surface — no dedicated input field —
and the cursor walks the rows matching the query. One implementation
(frontend/himark/src/speedsearch.rs), over the one addressing model
(keys), for the drawers, the trees, and anything else on `ListView`.

## 1. The shape

```rust
pub struct SpeedSearchView<T, S>
where
    T: View + SearchableList<S::Key>,
    S: Searcher<View = T>,
{
    inner: T,
    searcher: S,
    /// The SHADOW input: a real one-line input editor (value host, own
    /// document) — real because typing must be REAL typing: IME
    /// composition, dead keys, backspace, clipboard all come free.
    /// Renders as a compact query pill only while non-empty.
    input: EditorView,
    /// THE filter lane: one in-flight run, a newer query supersedes
    /// (fx.relaunch — ask-answer never conflates).
    lane: Option<CancellationToken>,
    /// The launch guard stamp: (query, items generation).
    launched: Option<(String, u64)>,
}
```

A container, not a surface: consumers wrap whatever sits between their
keymap and their list — usually `ScrollView<ListView<…>>` — and route
its commands like any child's.

## 2. The two borrow problems, and their one resolution

The user-visible statement of the problem: the `Searcher` must READ
the items out of the view and later WRITE match state back into a
`ListView` buried a level (or three) below the wrapped `T` — while
`SpeedSearchView` owns both the searcher and the view. Two distinct
knots:

**(a) The searcher cannot borrow the view.** `S` holding `&T` (or the
reverse) is a self-referential struct — unbuildable. Resolution: the
searcher is PURE POLICY and holds nothing; every method takes the view
as a parameter. The parent owns both fields and lends `&`/`&mut inner`
at distinct times — no aliasing exists to fight:

```rust
pub trait Searcher: Clone + Send + Sync + 'static {
    type View: View;
    type Key: Clone + Eq + Hash + Send + Sync + 'static;

    /// O(1) capture on the UI thread: CLONE the persistent domain
    /// state (a Forest, an items map — everything store-shaped is
    /// already O(1) to clone) into a Send closure; the WORKER calls
    /// it to enumerate `(searchable string, key)` pairs. The view
    /// itself never crosses the thread.
    fn capture(&self, view: &Self::View) -> ItemSource<Self::Key>;

    /// The generation `capture` would enumerate — bumped by whatever
    /// changes the items (a lazy listing landing, an outline
    /// relanding). The lane relaunches when it moves.
    fn generation(&self, view: &Self::View) -> u64;
}

pub type ItemSource<K> = Box<dyn FnOnce() -> Vec<(String, K)> + Send + Sync>;
```

**(b) The effect cannot borrow anything.** The filter runs on a
worker; the view lives on the UI thread. Resolution: the same
discipline every background feature here rides — capture PERSISTENT
VALUES (the outline's `Forest`, the changes tree's items map, a rope
of labels), not views. `capture` is O(1) because those clones are; the
worker pays the linear enumeration and the match itself.

**(c) The list is a level deeper than `T`.** `T` is deliberately NOT
`ListView` — there is a scroll (at least) in between, and
`SpeedSearchView` must not know the nesting. Resolution: the
reach-through is a trait of OPERATIONS, not structure — the few ops
speed-search needs, forwarded one line per wrapper:

```rust
pub trait SearchableList<K> {
    fn set_matches(&mut self, keys: &[K]);
    fn clear_matches(&mut self);
    /// Cursor to the next/previous MATCHED row (delta 0 = the first
    /// match at/after the cursor), reveal armed. Wraps at the ends —
    /// cycling is what makes speed-search fast.
    fn step_matched(&mut self, delta: isize);
    fn match_count(&self) -> usize;
}

impl<T: Clone, K: …> SearchableList<K> for ListView<T, K> { … }    // the real ops
impl<V: SearchableList<K>, K> SearchableList<K> for ScrollView<V> {
    // one line each: delegate to content_mut()
}
```

Any bespoke wrapper between a consumer's scroll and its list forwards
the same way, once. Note what this buys: the ROW TYPE never appears —
`SpeedSearchView<T, S>` bounds `T: SearchableList<S::Key>` and stays
ignorant of `TreeRow` vs `LabelRow` vs editor rows. The searcher
stays ignorant of presentation entirely.

## 3. Matches are the third interval set

`ListView` carries `matches: Intervals<K, ()>` beside structure and
selection — the same index space, the same one-Operation shift at
every splice, entries dying with their rows. `set_matches` resolves
each key's own row (log n each) and swaps the set wholesale;
`step_matched` queries the set ascending from the cursor's row —
order comes free from the intervals, no sorting, no index vectors.

Painting: matched rows get a calm tint (the selection style's fill at
reduced alpha — one more probe in the visible-row walk, O(log n) per
row like selection). The cursor keeps the full selection paint.

Folded and unfetched rows: only MATERIALIZED rows match — a key whose
row is inside a collapsed subtree resolves to nothing and drops from
the set (consistent with "the list sees folded rows as deleted").

## 4. The flow

1. **Typing** routes to the shadow input via the semantic focus walk:
   `SpeedSearchView::focus_data` seats the input ALWAYS — typing is
   what STARTS a search; the rows never claim text focus. Every input
   change relaunches the lane: `SpeedSearchEffect` carrying the
   captured `ItemSource<K>` and the `(query, generation)` stamp.
   A `generation` move (or an edited query) is noticed by the
   paint-reconcile pass, which answers `Refresh` — the lane relaunches
   against the fresh items.
2. **The worker** enumerates the source once and filters:
   case-insensitive subsequence (every query char appears in order),
   answering matched keys in item order.
3. **The landing** (stamp-guarded): `inner.set_matches(&matched)`,
   then `inner.step_matched(0)` — the cursor jumps to the first match
   at/after where it stands, revealed. No searcher involvement: the
   ops trait carries it.
4. **Arrows** while a query stands: Up/Down → `step_matched(±1)`,
   consumed. **Escape** clears the query, the matches and the pill —
   consumed; a second Escape (empty query) is Ignored and falls to
   the surface's own dismiss. **Enter** is the surface's own pick,
   with the cursor already on the match; the surfaces pair it with a
   `Clear` in the same stroke, so the search never outlives the pick.
5. **Unmount** clears matches (the container's destroy).

## 5. Commands and focus

```rust
pub enum SpeedSearchCommand<C> {
    Inner(C),
    Input(EditorCommand),                // the shadow editor's
    Landed(SpeedSearchMatches),          // { matched, stamp }
    Step(isize),
    Clear,
    Refresh,                             // stale stamp noticed at paint
}
```

`focus_data`: the stepping keys (Up/Down/Escape) answer only while a
query is live; the input's focus data merges under them — always
seated for text, but its COMMANDS are withheld while the search is
closed, so a dormant speed-search contributes nothing to the palette;
the wrapped view's own focus data merges under both, untouched. The
pill paints over the inner's top-right corner; the input editor
itself lays out into the pill (it IS the pill's text).

## 6. Consumers

Every tree surface mounts it the same way — wrap the scroll, forward
the ops trait through any wrapper, provide a `Searcher`:

- **Outline / results ToC / changes**: `ForestList` (the forest and
  its scrolled list as ONE view — the searchable unit the capture
  reaches through `&View`) with the shared `ForestSearcher`
  (frontend/himark/src/forest.rs — the FOREST knows nothing of
  search; it exposes one generic pre-order flatten, and the searcher
  projects the pickable labels out of it on the worker).
- **Workspace tree** (hifiles): its lazy `TreeList` with a
  `LocationSearcher` that snapshots the LIST's structure
  (`structure_keys`, persistent) — the location keys ARE the labels;
  nothing else is stored.
- Freshness is uniform: `ListView::generation()`, bumped by every
  splice — no per-consumer counters.
- **Peeker/palette**: have a real input — they keep it; their
  filtering is their own.

## 7. Cost invariants

- Capture: O(1) (persistent clones) — the UI thread never enumerates
  items.
- Filter: worker, linear in items × query.
- Landing: k matches × O(log n) row resolution; one wholesale swap.
- Step: O(log n). Paint: one probe per visible row.
- No per-frame work while idle; the lane holds at most one run.
