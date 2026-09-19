# Background search

Search finds **text or regex** across the session and shows the
occurrences grouped by document, each occurrence rendered as a real
`EditorView` bound to a live fragment of its document. Two scans run
under one query serial, both off the UI thread:

1. **the open-document scan** — a `SearchEffect` over snapshots of
   the session's open documents (documents are per-session state,
   docs/ui/app-state.md);
2. **workspace Find** — a `FindEffect { folders, term,
   FindTarget::Text }` over the session's folders (the ripgrep-backed
   native handler; the folder scoping and the temp-document install
   story are docs/ui/workspace.md).

Results land through commands, exactly like a background reparse or a
background open. Nothing here invents a new threading model or a new
store: it is the effect pipeline plus the existing per-editor
document model.

---

## 1. Building blocks — see [`docs/ui/list-view.md`](../ui/list-view.md)

Two pieces are general, not search-specific, and are specified in
[`docs/ui/list-view.md`](../ui/list-view.md) and [`docs/ui/list-tree.md`](../ui/list-tree.md);
search was their first consumer:

- **The list** — the unified `ListView` the results panel rides
  (through `himark::LocationList`): documents as groups, occurrences
  as rows.
- **Bounded editors** — an `EditorView` scoped to a fragment of a
  document, the interval shifting with edits. Each occurrence is a
  bounded editor over its match range, so an occurrence row is
  editable in place and can never desync from the document;
  whole-document rows leave the bound unset.

This document covers what is search-specific: the match tints, the
background scan, the results panel, and the in-document find bar.

---

## 2. Match highlighting

Match ranges are a **feature markup** on the target document:
`StyleId::Match` ranges pushed through `push_styled` — a tinted wash
over each occurrence, visible in the main editors as well as the
result rows, shifting with edits like all markup. The producer brings
the change set (old ∪ new occurrence ranges) to every
`replace_markup` swap, so stale washes leave with the query that made
them. Tints clear when the query changes and when the panel goes.

There is deliberately no `Decoration::Match` variant and no side
interval set: feature markup already owns "a producer's ranges over a
document," gives main-editor highlighting for free, and leaves
"inlay" meaning a widget.

---

## 3. The search effect

A background scan: capture what it needs, do the work off-thread,
land a value command.

```rust
struct SearchEffect {
    serial: u64,          // the query's stamp — stale landings discard
    query: Query,         // case-insensitive regex when it parses,
                          // its escaped literal otherwise
    targets: Vec<(DocumentId, Document)>,   // snapshots — Send, O(1) clones
    width: f32,           // the install width for prebuilt rows
}
```

The scan windows each text over line-aligned 64KB chunks
(`SCAN_WINDOW`) so it never materializes a giant `String`, rebasing
local match offsets to absolute bytes; byte offsets feed straight
into the markup and fragment machinery (both `u32`-byte-keyed).

**Supersession.** The panel holds an effect token per lane; a new
query `relaunch`es the token — the superseded scan's landing is never
delivered — and every landing carries the `serial`, so anything that
outruns the cancel discards itself on arrival. A superseded query's
results are pure waste, which is why this is a lane and not a
fire-and-forget ask (the delivery rule, docs/ahp/files.md: ask-answer
effects are owed their answers; a search is not an ask, it is the
newest interest winning). The workspace `FindEffect` launches beside
the scan under the same serial; found locations (already-open ones
dropped, capped) fetch and install per docs/ui/workspace.md.

---

## 4. Landing results, and the panel

**Routing is by identity, not tree path.** Results land into the
panel's store row — `himark::LocationLists`, keyed by the panel's
list id — the way `ApplyRepair` lands in an editor. The view tree may
have changed since the query launched (a pane split or closed
mid-search must not drop results); the query itself rides the tree
the other way, like a keystroke: typing in the panel's input reaches
it as a content command, and the panel's `perform` relaunches the
effects. Only the *result* needs identity routing, because it returns
from a worker after the tree may have moved.

Each landed group installs tint markup, fragments, and bounded row
editors — an open document's group and a fetched temp document's
group are indistinguishable (the header shows the location's path).
The install records own their bookkeeping and unwind on the next
query's drain and on dismantle; the panel's clone CARRIES the
bookkeeping (the install-ownership lesson, docs/ui/workspace.md).

**Search is a panel, not an overlay.** The peeker is a transient
quick-switcher that dismisses on pick; search is a workbench panel,
peer to an editor — split it in beside your text, leave it open,
click a result to open the target in a sibling pane. It is a plugin
panel (`Panel::Plugin`, docs/ui/list-view.md §4). In modal mode it opens
under the toolbar and the toolbar's well feeds it through
`set_query` (`%`-prefix, docs/ui/toolbar.md); pinned, it keeps its own
input and lives in the pane. A displaced search survives with its
results on the unmounted list (docs/ui/peeker.md); it has no navigation
place — restoring a search means relaunching its scan, which a
navigator cannot do (docs/ui/navigation.md).

Occurrence rows are blurred previews by default; focusing one edits
in place — the bounded editor makes both trivial, and edits write
through to the real document.

---

## 5. The in-document find bar

cmd-F drops a bar across the top of the focused editor pane — a
query input plus an occurrence count; shift-cmd-F stays the global
search. Enter / cmd-G walk forward,
shift-Enter / cmd-shift-G back — each step SELECTS the occurrence and
reveals it through the ordinary scroll machinery
(`Document::reveal_selecting`). Escape dismisses.

The bar lives on the PANE SLOT (`PaneSlot.find`), like navigation
history: it survives panel displacement — switching files keeps the
bar and re-scans the new document — and never touches `Panel`. Its
tints are one feature markup on the target document, `StyleId::Match`
ranges exactly like the global search's washes, shown on the pane's
editor only (two panes over one document each carry their own bar);
the producer brings the change set (old ∪ new occurrence ranges) to
every `replace_markup` swap. Scanning is the global search's
semantics — case-insensitive, the query as a regex when it parses and
its escaped literal otherwise — over line-aligned 64KB windows, capped
at 20k occurrences.

Focus follows the shield pattern: the bar wraps its input in
`focus_scope(focused)` and its keymap leaf is placed last (tried
first); a click into the pane text hands the keyboard back without
closing the bar, and cmd-F while open refocuses with the query
selected. Every slot command funnels through one sync, so query edits,
pane edits, and displacement all converge on fresh tints; the walk
restarts from the current occurrence after each re-scan.
