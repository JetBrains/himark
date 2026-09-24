# The list keyboard: one controller, one selection signal

`ListKeyboardController` (frontend/himark/src/list_keyboard.rs) is
THE keyboard layer for every list and tree surface: the
arrow/Enter/Escape/fold table declared once, speed-search folded in as
an OPTION (`Searcher` supplied) instead of the reason the wrapper
exists. Its companion is one change in the list itself: selection
edits become a `ListCommand` variant carrying the new selected index,
so a surface reacts to selection by INSPECTING the command stream it
already routes — never by diffing, peeling or bookkeeping.

## 1. Motivation: the loop was re-typed per surface

Before this design landed, `ListView` implemented selection once
(docs/ui/list-tree.md §5) — but only as silent primitives
(`cursor_step`, `select_only`). Everything between a key press and
those primitives was re-typed per surface, in four dialects at ~17
call sites:

- Nine tree panels (toc ×2, hifiles, hihistory, hichanges, hicomments,
  hisearch, the agents drawer, combo) each declare the same six-arm
  match — Esc/Up/Down/Left/Right/Enter → Dismiss/`Select(±1)`/
  `Fold(bool)`/Pick — TWICE: once in `focus_data`, once re-expressed
  as `Event::KeyDown` arms in a keymap overlay leaf, kept in sync by
  hand. Every arm re-threads its own `if searching` gate, subtly
  inconsistently (some gate Enter on non-empty rows, some don't;
  hisearch folds area focus into the same bool).
- The `RowList` trio (palette, peeker, completion) track their own
  `selected: usize` beside the list's cursor and push it in — palette
  forgets the push on click and desyncs by design.
- No selection-change signal exists, so the three surfaces that react
  to the cursor moving invented three mechanisms: hisearch diffs
  `cursor()` before/after every child perform, peeker/hipeek call
  `ensure_preview` at each mutation site, completion calls `swap_view`.
- Clicks never select: the widget's `MouseDown` answers
  `ListCommand::Focus(index, …)` — a ROUTING index — and every surface
  re-derives click→selection through one of four hand-written peelers
  of the same `Focus(index, Some(Child(…)))` shape (`tree_interaction`,
  `tree_action`, `RowList::picked`, `peeled_activation`).

## 2. The selection command

```rust
pub enum ListCommand<C> {
    Child(usize, C),
    Focus(usize, Option<Box<ListCommand<C>>>),
    /// THE selection edit and THE selection signal — one variant.
    /// Absolute row index, resolved by the initiator at the edge
    /// (controller key table, mouse arm, speed-search landing).
    Select(usize),
    /// THE activation signal. The list itself does nothing with it
    /// (its perform arm is empty) — it exists so the surface RECEIVES
    /// activation uniformly, parameterized by what triggered it.
    Activate(usize, ActivateTrigger),
    ViewportTop(f32, f32),   // top, band height — page_index's extent
    SetHeight(usize, f32),
    Animate(AnimationClock),
    Revealed,
}

/// EXHAUSTIVE on purpose: a surface must match every trigger and
/// decide what each means for it (hisearch: Click browses, Enter
/// jumps and focuses; combo: both pick and close). There is no
/// default to hide behind, and a future trigger (DoubleClick) breaks
/// every match — forcing exactly the per-surface decision it should.
pub enum ActivateTrigger { Enter, Click }
```

- **`ListView::perform`'s `Select` arm** is the one mutation door:
  `key_at(index) → select_only(key)`, reveal armed. `cursor_step` and
  `step_matched` stop being public mutators; their walks survive as
  READ queries (§3) that the initiators resolve indices with.
- **Mouse**: on a selection-configured list, a click that lands on the
  ROW BODY (no row affordance consumed it) answers
  `Commands([Focus(index, Some(Select(index))), Activate(index, Click)])`
  — routing, selection and activation, each its own inspectable
  command. A click a row affordance DID consume (a chevron's `Toggle`,
  a button) stays `Focus(index, Some(Child(index, …)))` exactly as
  today — affordances are not activation. Lists without selection
  keep the bare `Focus` (transcript, tool groups: nothing changes).
- **Why an index, not a key**: `ListCommand<C>` is deliberately
  K-blind, and command dispatch is synchronous within the frame — the
  index the initiator resolved is still the index when the `Select`
  arm runs. Key resolution happens once, inside the arm. A consumer
  that wants the domain key asks `key_at(index)` at inspection time —
  same frame, same answer.

**The reaction idiom**: commands descend root → surface → controller →
scroll → list; the surface's `perform` already routes every one of
them. Reacting to selection — preview, navigate, detail refresh — is
matching `Select(index)` on the way through; acting on activation is
matching `Activate(index, trigger)` and deciding PER TRIGGER. Keyboard,
click and speed-search all produce the same descending shapes; the
before/after diff, the `ensure_preview` call sites and the peelers all
collapse into those two matches.

## 3. `ListKeyboardController`

```rust
pub struct ListKeyboardController<T, S = NoSearcher<T>>
where
    T: ListOps,
    S: Searcher<View = T, Key = T::Key>,
{
    inner: T,
    /// Left/Right emit `Fold` only for surfaces that opted in
    /// (`.with_folds()`) — on a flat list the keys stay unconsumed.
    folds: bool,
    /// Present only when the surface opted into speed-search
    /// (`::searchable(…)` instead of `::new(…)`): the shadow input
    /// pill, the filter lane, the launch stamp.
    search: Option<SearchLane<S>>,
}

pub enum ListKeyCommand<C> {
    Inner(C),
    /// Left/Right on the cursor row. Fold stays a SURFACE-handled
    /// command, uniformly — the controller carries no fold policy.
    /// The eager trees answer it with the shared `Forest::fold_cursor`
    /// (Right expands, Left collapses or walks to the parent) — one
    /// line each; hifiles, hihistory and the drawer answer with their
    /// own lazy-fold states, as they must. Decided: no policy hook in
    /// the controller — uniform commands beat two configuration paths.
    Fold { index: usize, expand: bool },
    // — the search lane, only reachable with a Searcher: —
    Input(EditorCommand),
    Landed(SpeedSearchMatches),
    Clear,
    Refresh,
}
```

Activation is NOT here — it is `ListCommand::Activate(index, trigger)`
(§2), constructed through the ops trait like `Select`, so keyboard and
mouse activation descend the same chain and the surface matches ONE
shape.

`Searcher`, the capture/generation split, the worker filter, the
match intervals and the pill are unchanged from the speed-search
design (§5–§6). What changes is who owns the keys.

### The one key table

Declared once in the controller and served to BOTH consumers of it:
`focus_data`'s `on_key` (the semantic focus walk) and the keymap
overlay leaf the controller's `display` plants itself — the
per-surface duplicate overlays are deleted, and the two copies cannot
drift because there is only one.

| Key | No query standing | Query standing |
|---|---|---|
| Up/Down | `Inner(select_command(step_index(±1)))` | `Inner(select_command(matched_step_index(±1)))` |
| Home/End | `Inner(select_command(edge_index(first/last)))` | same, over the MATCHED rows |
| PageUp/PageDown | `Inner(select_command(page_index(∓1)))` — one viewport extent, resolved on the rope's y-metrics | same |
| Enter | `Inner(activate_command(cursor_index, Enter))` | same — surface pairs its pick with `Clear` |
| Escape | Ignored — falls through to the surface's dismiss | `Clear` |
| Left/Right | `Fold { index: cursor_index, expand }` (`.with_folds()` surfaces; elsewhere the keys stay unconsumed) | same |
| typing | starts the search (Searcher present) | edits the query |

Home/End and the page keys are in the first cut — nobody implements
them today precisely because every surface would have had to; the one
table is where they become free. `page_index` needs the viewport
EXTENT beside its top: the scroll's traversal report grows from
`ViewportTop(f32)` to carry the band height (same report, one more
field — docs/editor/viewport-preservation.md §3.1).

The `searching` gate lives HERE, once. Surface modes that override
keys (the drawer's add-host input, hisearch's input/results area
focus) stay surface-level: their `focus_data` merges OVER the
controller's, exactly the merge discipline focus already has.

### `ListOps<K>`: the reach-through grows up

`SearchableList<K>` was the ops trait that let speed-search reach a
list buried under a scroll (one line of forwarding per wrapper). The
controller needs the same reach for keys, so the trait grows into
`ListOps<K>` — still operations, never structure:

```rust
pub trait ListOps<K>: SearchableList-ops… {
    // match state (as today): set_matches / clear_matches / match_count
    // read queries — the mutating walks of cursor_step/step_matched,
    // minus the mutation, O(log n):
    fn cursor_index(&self) -> Option<usize>;
    fn step_index(&self, delta: isize) -> Option<usize>;          // clamps
    fn matched_step_index(&self, delta: isize) -> Option<usize>;  // wraps
    fn edge_index(&self, end: Edge) -> Option<usize>;             // Home/End
    fn page_index(&self, direction: isize) -> Option<usize>;      // viewport extent
    /// The COMPOSITIONAL addressing: each wrapper wraps its inner's
    /// answer (`ScrollCommand::Content(inner.select_command(i))`) —
    /// the controller emits list commands without knowing the nesting.
    fn select_command(&self, index: usize) -> Self::Command;
    fn activate_command(&self, index: usize, trigger: ActivateTrigger) -> Self::Command;
    /// The inspection inverses, for surfaces reacting to the stream:
    /// peel their own layer and ask inward. `Some` iff the command
    /// is (or carries) the matching `ListCommand` variant.
    fn selected_index(command: &Self::Command) -> Option<usize>;
    fn activated(command: &Self::Command) -> Option<(usize, ActivateTrigger)>;
}
```

`ListView` answers the real ops; `ScrollView`, `TooltipView`,
`ForestList` and the controller itself forward one line each — a
surface asks the TYPE it routes commands through.

## 4. What a surface says after this

```rust
// mount: keyboard always; search only if a Searcher is given
ListKeyboardController::new(tree, store, ui)              // keys only
ListKeyboardController::searchable(tree, searcher, …)     // keys + pill

// perform: the whole keyboard story of a tree panel
TocCommand::Rows(command) => {
    if let Some(index) = ForestList::selected_index(&command) {
        // selection moved — keyboard, click or search step alike
        self.preview(index, store, fx);
    }
    if let Some((index, trigger)) = ForestList::activated(&command) {
        // the surface DECIDES per trigger — the match is exhaustive
        match trigger {
            ActivateTrigger::Enter => self.pick_row(index, /*focus*/ true, …),
            ActivateTrigger::Click => self.pick_row(index, /*focus*/ false, …),
        }
    }
    fx.scope(TocCommand::Rows, |fx| self.rows.perform(store, ui, command, fx));
}
TocCommand::Rows(ListKeyCommand::Fold { index, expand }) => self.fold(index, expand, …),
```

Gone per surface: the six-arm key match (×2), the `searching`
plumbing, the relative `Select(±1)`/`Pick`/`Fold` command variants,
the click peeler, and — for the RowList trio — the shadow
`selected: usize`.

## 5. Speed-search (unchanged mechanics)

The search half keeps the prior speed-search design wholesale:

- **The `Searcher` is pure policy** and holds nothing; every method
  takes the view as a parameter, which is what dissolves the
  self-referential borrow (the parent owns both fields and lends
  `&`/`&mut inner` at distinct times):

  ```rust
  pub trait Searcher: Clone + Send + Sync + 'static {
      type View: View;
      type Key: Clone + Eq + Hash + Send + Sync + 'static;
      /// O(1) capture on the UI thread: CLONE persistent domain state
      /// (a Forest, a structure snapshot) into a Send closure; the
      /// WORKER enumerates `(label, key)` pairs. Views never cross.
      fn capture(&self, view: &Self::View) -> ItemSource<Self::Key>;
      /// Bumped by whatever changes the items; the lane relaunches
      /// when it moves.
      fn generation(&self, view: &Self::View) -> u64;
  }
  ```

- **The filter runs on a worker** over captured values (linear in
  items × query, case-insensitive subsequence), stamped
  `(query, generation)`; a stale stamp noticed at paint answers
  `Refresh`, a newer query supersedes the lane (`fx.relaunch`).
- **Matches are the third interval set** over the row-index space
  (beside structure and selection), shifted by the same one Operation
  per splice; matched rows tint with the selection fill at reduced
  alpha; only materialized rows match.
- **The shadow input** is a real one-line editor (IME, dead keys,
  clipboard for free), rendered as the query pill only while a query
  stands, always seated for text in the focus merge — typing is what
  STARTS a search.

Two deltas against the prior design:

- The LANDING no longer mutates selection silently: it resolves
  `matched_step_index(0)` and sends the `Select` around the
  `AnnounceSelect` round trip — an always-ready effect whose result
  is the select command, dispatched from the root — so the surface
  sees the same descending `Select` it sees for every other source.
- `step_matched` mutators leave the ops trait (§3).

## 6. Focus and escape ordering

`focus_data` merge order, top to bottom: surface modes (if any) →
controller key table → shadow input (commands withheld while the
search is closed, as today) → wrapped view. Escape resolves by that
order: a standing query eats it (`Clear`); otherwise it falls through
to the surface's dismiss; modal surfaces above the controller keep
their own Escape (docs/ui/keymap.md's deliberate non-rebindables).

## 7. What landed where

- **imba**: `ListCommand::Select`/`Activate` and their perform arms;
  the mouse arm's body-click answer (`Focus(i, Select(i))` then
  `Activate(i, Click)`) on selection-configured lists; the read
  queries; `ViewportTop` carries the band height. Row bodies
  (`TreeLabel`, `LabelRow`) stopped consuming clicks — affordances
  (chevron `Toggle`, `Action` chips) still claim theirs.
- **himark**: `list_keyboard.rs` (né speedsearch.rs) holds the
  controller; `SearchableList` became `ListOps`.
- **Ported onto the controller**: toc ×2, hifiles, hihistory,
  hichanges, hicomments, hisearch (the before/after cursor diff is
  gone — it matches `selected_index`), the agents drawer, combo
  (the raw `Focus`-as-pick match is gone — `picks()` classifies by
  `Activate`), hipeek (`new`, keys only), palette and peeker
  (keys-only controller over `ScrollView<ListView<LabelRow, _>>`,
  their shadow `selected: usize` deleted).
- **Completion** kept its own key arms deliberately: its popup view
  and its state machine live on opposite sides of the editor's inlay
  routing, and Tab-picks are its own semantics — but it dropped
  `RowList`, reads the list's cursor as THE selection, and decodes
  clicks with the `activated` peel like everyone else.
- **Deleted**: `RowList` (and its `RowCommand::Picked`),
  `tree_interaction` (replaced by the narrower `tree_toggle` —
  chevrons and toggling bodies are a fold protocol, not activation),
  the drawer/panel key-match duplicates. `tree_action` (the chip
  press) survives — chips are affordances.
- Tests drive the same commands the table emits (`step_index` →
  `select_command`, `activate_command`) instead of the retired
  relative variants; landings drain the `AnnounceSelect` round trip.

## 8. Cost invariants

- Key → command: the read queries are the same O(log n) interval
  walks the mutators do today; resolving at the edge adds nothing.
- `Select` arm: `key_at` + `select_only`, O(log n).
- The controller without a Searcher carries no input, no lane, no
  stamp — a keys-only surface pays two `Option` checks.
- Inspection (`selected_index`, `activated`) is a constant-depth enum
  peel.

## 9. Decisions log

- Activation is `ListCommand::Activate(index, trigger)` with an
  exhaustive `ActivateTrigger` — the surface decides per trigger, no
  default (§2).
- Fold stays a surface-handled command uniformly, no controller
  policy hook (§3).
- Home/End/PageUp/PageDown are in the first cut, in the one table
  (§3).
- **`RowList` dies.** It was an index-keyed shadow of what the list
  already does (its `selected: usize` mirror is the §1 desync bug by
  construction). Palette, peeker and completion mount
  `ListKeyboardController` over `ScrollView<ListView<LabelRow, _>>`
  directly; `LabelRow` (the row VIEW) stays.
