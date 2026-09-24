# `ListView` and lists of editors

`ListView` — a Rope-backed list of child views — plus what it takes
to fill one with editors: editors bounded to a *fragment* of a
document, list resize as a background repair, and a workbench leaf
that hosts something other than an editor. The retired search panel
was the first consumer (its successor is docs/ui/location-list.md);
nothing here is search-specific. Tree structure, keys and selection live in
`docs/ui/list-tree.md` (one `ListView<V, K>` with structure and selection as
interval data); this document covers the rope backing, bounded
editors, and the repair composition.

---

## 1. `ListView`

### Backing

The template is `DocumentLayout`: a `Rope<LayoutElement, LayoutMeasure>`
whose elements carry heights and whose metrics answer positional
queries (`crates/editor/src/document_layout.rs`). `ListView` is the
same shape with a simpler element:

```rust
struct ListElement<T> {
    view: T,        // T: View (or Box<dyn CloneDynView> when erased)
    height: f32,    // the child's measured height, cached in the rope
}

// RANK = 1: one cumulative axis, VERTICAL_PX. The Nth element comes from the
// rope's implicit index (Cursor::seek_to_index) — no axis spent on counting.
pub struct ListView<T: Clone, K: Clone + Eq + Hash = ()> {
    items: Rope<ListElement<T>, ListMeasure>,
    // …keys, structure and selection: docs/ui/list-tree.md
}
```

**Construction takes the rope PREBUILT.** Building the rope is the
one linear step, and it is a free function —
`imba::list::measured(items) → ListRope<T>` — so a worker can build a
bulk rope and hand it over; `ListView::from_rope[_at]` mounts it
O(1), and `ListView::empty[_at]()` is the mount state for lists whose
rows land later. There is no constructor that takes an item iterator:
a UI-thread call site must write `from_rope(measured(..))` and
thereby state the linear cost exactly where it is paid — bounded
inputs compose the two calls inline; bulk producers belong in effect
handlers.

**Not virtualized.** Every element is laid out — eagerly on build, in the
background on resize (§3). The rope holds real heights and knows the total;
there is no placeholder tail. Visible rows whose live size disagrees with the
cache report it during paint (which doubles as the per-frame geometry
reconciliation) and the fresh height splices in; off-screen rows stay stale
until scrolled in or the list repair lands. The Rope is here for what
`DocumentLayout` uses it for:

- O(log n) positional queries: total height, index → y for placement,
  y → index for clicks and scroll anchors (`seek(VERTICAL_PX, y, After)`).
- O(log n) splice when one element's height changes.
- Being the substrate the background repair splices into (§3).

### Layout and events

The list's widget reports `(constraints.max.width, total_height)`. Per-frame
work is paint and hit-testing over the visible slice only: seek to the first
on-screen element, walk until past the viewport bottom — the same viewport
walk `DocumentLayout` paints lines with. Selecting what to *paint* is not
virtualization; nothing is left un-laid-out.

### Commands

```rust
pub enum ListCommand<C> {
    Child(usize, C),          // route to element i, effects mapped up by index
    Focus(usize, Option<Box<ListCommand<C>>>),  // keyboard ROUTING index (not selection)
    Select(usize),            // THE selection edit+signal (list-keyboard.md §2)
    Activate(usize, ActivateTrigger),  // activation, per-trigger (list-keyboard.md §2)
    ViewportTop(f32, f32),    // scroll traversal report: top + band height
    SetHeight(usize, f32),    // measurement landing, door-anchored
    Animate(AnimationClock),  // splice animations
    Revealed,                 // reveal round-trip completed
}
```

`perform` routes `Child(i, c)` to element `i` and maps its effects up by the
same index — the `SplitView` pattern, indexed. Replacing element `i` is
`seek_to_index(i)` → `delete(1)` → `insert(one)`, the `DocumentLayout::edit`
dance. Nesting needs no special cases: a `ListView` is a `View`, so
lists nest and commands nest as `Child(i, Child(j, C))`.

> **Index churn.** Indices are stable only until the list is edited;
> inserting or removing elements shifts the addresses after the edit
> point. Row identity across edits is the KEY's job ([list-tree.md](list-tree.md)) —
> domain keys, never index bookkeeping.

---

## 2. Bounded editors

There is no `FragmentView`. An `Editor` is already the per-view record inside
a document (`document.editors: HashMap<EditorId, Editor>`), and `EditorView`
is `{ document, editor: EditorId }`. A fragment is an editor with one more
field — the interval that delimits it:

```rust
struct Editor {
    layout: DocumentLayout,
    bounds: Option<FragmentKey>,   // None = whole document
    caret: u32, layout_width: f32, focus: EditorFocus, marked: Option<Range<u32>>,
}
```

Same view code, same caret, same event handling, same write-through. The
interval lives in the document's fragment sets, so it shifts with every edit
and the editor re-reads its live range on each re-lay. Because a fragment is
a real editor with its own `EditorId`, it has its own caret/focus/IME and is
editable by nature (blur it for a preview); edits write through to the one
shared document. This is not the table-cell pattern (a projected cell view
plus offset write-through, `plugins/himarkdown/src/table.rs`): the bounded
editor shares the real document and cannot desync. The capability is
general — inline rename, peek-definition, outline rows, search results.

Three places honor a bound resolved to `start..end`:

- **Layout** — `DocumentLayout::build` / `repair_region` /
  `LayoutItems::new` run from `byte_start` to `byte_end` instead of
  end-of-text.
- **Size** — `content_height` is `editor.layout.height()`; correct
  as-is once the layout covers only the fragment.
- **Coordinates** — caret and click offsets are document-absolute;
  caret motion and painting clamp to `[start, end)`.

A list of whole-document editors never sets `bounds`; fragments are
opt-in per editor.

---

## 3. Resize is a repair — and it composes

An editor lays out at its stored `layout_width`, not its layout-time
constraint, so resize is a **mutation**: `document.resize(editor, width)`
re-lays via the editor's own `DocumentLayout` repair — safepoint-driven,
budgeted, incremental, because a text edit ripples across line boundaries.

The list is the same machine one level up, minus the hard part: **no
safepoints**. Element boundaries are stable — re-laying element *i* never
disturbs element *j* — so damage is a set of indices (on resize: all), and a
budget-exhausted pass resumes at the next index. No alignment machinery.

The composition: the list repair's per-element step — "re-lay this editor at
the new width" — *is* an editor repair. A large editor lays out its budgeted
head now and background-repairs its own tail, its `content_height` growing as
tails land, the list re-splicing that row's height each time. The outer repair
stays cheap (walk indices, delegate) no matter how large the editors are; the
heavy work sits in each editor's own budgeted repair, and results settle over
as many passes as the sizes demand. Editors in a list may be **arbitrarily
large**; no level ever stalls a pass. Nested lists of editors run the same
shape three deep — lines / editors / groups — budgeted at every level.

The shared shape is *rope of height elements + damage set + a background
effect that re-lays damaged elements up to a budget and returns a command
splicing them back* — the `RepairEffect`/`ApplyRepair` pair
(`crates/editor/src/repair.rs`). `DocumentLayout` is the text-and-safepoints
instance; the list is the index-and-no-safepoints instance delegating to
`document.resize`.

Two invariants keep reflow honest: scroll re-anchors to the top element's
**index** (resize moves heights, never the sequence), and the rope is the sole
source of total extent and y↔index — a late-landing repair just re-splices its
rows, the same "stale until repair" contract as the panes.

---

## 4. Workbench panels

A `ListView` needs a pane to live in, and a `WorkbenchNode` leaf hosts
more than an editor:

```rust
pub enum WorkbenchNode {
    Leaf(PaneSlot),                     // panel + navigation history, find bar, …
    Split(Box<SplitView<WorkbenchNode, WorkbenchNode>>),
}

pub enum Panel {
    Editor(EditorPane),
    Plugin(Box<dyn DynPanelView>),      // search, terminals, diffs, chats, …
}
```

The editor arm is closed and explicit; every other panel kind is a
`PanelView` implementation boxed behind the erased `DynPanelView`
face — panel kinds are plugin-provided, so the open, erased shape won
over a closed enum for them. `Panel::perform`/`layout` match the arm
and map the child's command up — the dispatch `WorkbenchNode` and
`SplitView` already do.

The workbench methods (`focused_pane`, `split_focused`,
`close_focused`, `for_each_pane*`) speak `Panel`. Resize is §3:
`for_each_pane_sized_mut` hands each panel its size and each re-lays by its
own repair. A panel is a repairable thing that fills a pane.

The genuine friction: app actions that assume the focused pane is an editor
(open-into-focused-pane, save, new-scratch). With a non-editor panel focused
they resolve "the focused **editor**" — the nearest editor leaf, else split
one in.
