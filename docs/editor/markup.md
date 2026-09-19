# Recursive markup

One markup model for everything: a `Markup` is **interval lanes and a
HAMT**, and it recurses. Some markers in the intervals denote *syntax
over this range*; for every such marker the HAMT holds a `Syntax` — a
`Tree` and another `Markup` — and so it recurs. Markdown is not
special: it is the root syntax. Injections are not special: they are
syntaxes. There is no user/derived split, no content hashing, no
separate injection pipeline, and no `Document::parse` — the document
holds text, a syntax, and the language registry.

```rust
pub struct Markup {
    /// The BULK lane: Styled spans only — a syntax highlighter's
    /// volume lives here.
    styles: Intervals<IntervalId, Decoration>,
    /// The STRUCTURE lane: everything that changes shape — Inlay,
    /// Hidden, Unhide, Alignment, and the Syntax mount markers.
    shape: Intervals<IntervalId, Decoration>,
    syntaxes: rpds::HashTrieMapSync<SyntaxId, Syntax>, // persistent: O(1) clones
    scope: MarkupScope,     // feature markups: View (picked) | Document (all)
}

pub struct Syntax {
    pub language: String,   // registry name; the root's is "markdown"
    pub tree: Option<Box<dyn SyntaxTree>>, // edit-stepped through forwarding
    pub markup: Markup,     // derived decorations, marker-relative — recursive
    pub folds: Intervals<IntervalId, ()>,            // parse channel (docs/ui/toc.md)
    pub outline: Intervals<IntervalId, OutlineItem>, // parse channel
}

// The payload, in Decoration:
Styled(StyleId) | Hidden | Unhide | Inlay(Inlay)
| Alignment(TextAlignment) | Syntax(SyntaxId)
```

**Two lanes, one write door.** Every insert routes by variant
(`insert_split`): `Styled` into `styles`, everything else into
`shape`. The reason is scale asymmetry: a highlighter emits thousands
of plain spans, while structure — inlays, folds, hidden ranges,
mounts — is sparse; and the STRUCTURAL walks are the hot ones. The
inlay collection, the fold and outline channels, and the child-syntax
lookups query the shape lane only, so a viewport sweep over a densely
highlighted document never wades through the styling bulk.
Whole-markup reads (the reparse splice, counting) use `merged_query`,
which merges both lanes back into one ordered stream. Both lanes
edit together; the split is a storage decision, invisible to
consumers.

**There is no "THE markup."** The document holds
`syntax: Option<Syntax>` — the parse hierarchy directly, `None` for
plain text. The root syntax holds the markdown tree and markup, whose
shape lane contains the code-block markers, whose syntaxes hold rust
trees and token markup — any depth. `Document::markup()` returns the
root syntax's markup (empty for parse-less documents): **derived data
only**. Everything else has a real home:

- feature decorations → the `markups` map (below);
- fragments (bounded-editor windows, row anchors) →
  `fragments: HAMT<FragmentSetId, Intervals>` — plain shifting anchors,
  one **set per owner** (`add_fragment_set`), removed whole with the
  owner (`remove_fragment_set`). Never displayed, never touched by
  reparses; shifted at the edit door. `FragmentKey = (set, key)`;
  bounded layouts store their window RESOLVED and the edit door
  refreshes it from the set.
- construction-time host decorations (an input's style covering, a
  source file's mono face) → `Document::new` turns a non-empty markup
  argument into an anonymous **document-scoped** feature markup.

The root's own channels (folds, outline) are reachable without a marker
walk: `Document::{foldables_in, has_outline, outline_items,
resolve_outline}` merge the root's channels with the nested syntaxes'.
Outline addresses use `SyntaxId::DOCUMENT` (reserved) for the root's own
items — the root is a field, not a marker, so it has no minted id.

## The interval store

`Intervals<K, V>` (`frontend/intervals`) is a persistent interval
B-tree: fanout 32, parent-relative coordinates (a subtree shifts by
editing one offset), an augmented `max_end` for stabbing queries, and
copy-on-write structure sharing — a clone is a pointer bump, like
every other document part. Greediness (does an insert at my edge
belong to me?) is encoded per endpoint; internally greedy-left
intervals live in a separate root so both greedinesses stay
order-consistent. Rebase is `edit(steps)` — expand/collapse over the
operation's retain/insert/delete shape; a delete that swallows an
interval whole drops it and reports the dropped keys.
`MergedQuery` zips two ordered queries — the same helper serves the
two lanes of one markup and any other pairwise merge.

## Coordinates

Syntax markups are **marker-relative**; the marker's live start is the
only translation, applied at query time, and recursion accumulates bases.
(The root's base is 0, so its coordinates coincide with absolute ones.)

## Queries recurse

`impl IntervalQuery for Markup` IS the recursion: the query iterator
merges a small stack of levels by translated start (a marker precedes its
syntax's decorations; yielding a marker pushes its level), and each
level is itself the two-lane merge. Consumers — marked ranges, block
marks, inline/hidden collection — are written against the query trait
and know nothing of nesting or lanes. Inlays keep one bespoke
recursion (`all_inlays_in`) because they carry two-level addressing
(`MarkupLayer`: the host's vs a syntax's). Still per-visible-line,
still View × log(Doc).

`LineMarksSweep` is the amortized form of the per-line read: one
forward recursive query plus a carry list of still-active hits,
advanced line by line — so a contiguous run of visible lines costs
one ordered walk, not one query per line. The viewport build seeds
one sweep per contiguous visible segment and drops it at every
collapsed run (a fold, a windowed-fragment gap), re-seeding past the
gap — folded content's markup is never traversed
(docs/editor/viewport-preservation.md, docs/editor/scroll-stripe.md).

## Edits: one choke point

The edit door forwards the operation into the root syntax whole — its
markup, folds, outline and tree step together. From there `Markup::edit`
shifts both lanes and forwards the operation, rebased
marker-relative (greedy flags decide edge ownership), into every
intersecting child syntax — markup, channels and tree — which recurses.
An edit crossing a marker's boundary drops the syntax: the next landing
mints a fresh id and re-parses. Feature markups and fragment sets shift
at the same door.

## One effect parses the whole hierarchy

Parsing and markup derivation are **two different jobs**, and languages
implement exactly those two; the pipeline owns everything else:

```rust
pub trait SyntaxLanguage {
    /// Job 1: text → tree. `old` present = incremental, always. Chunked:
    /// tree-sitter pulls bounded pages (`Text::page_at`) from the rope —
    /// the text is NEVER materialized, at any size.
    fn parse(&self, text: &Text, range: Range<u32>, old: Option<&dyn SyntaxTree>) -> Option<Box<dyn SyntaxTree>>;
    /// Job 2: tree → markup, rebuilt for the changed ranges only.
    fn markup_for_changes(&self, text, range, tree, changed, replacement, invalidated);
    /// Child sites (markdown's fenced blocks); default none.
    fn sites(&self, text, range, tree) -> Vec<SyntaxSite> { Vec::new() }
    /// Language-specific typing assists; default none (docs/editor/type-assist.md).
    fn assist(&self, request: &AssistRequest) -> Option<Assist> { None }
}

pub struct SyntaxLanguages { /* name → language; grammars may register
                                lazily and load on first ensure
                                (docs/editor/lazy-languages.md) */ }
```

`SyntaxLanguages::parse_syntax` is the generic per-syntax routine: parse
(tree reuse when old), `changed_ranges ∪ edited`, rebuild markup over
that, splice into the old markup — the cold parse is the same code with
everything changed. The union exists because `changed_ranges` reports
*structure* only and markdown derives inline styling from raw text; pure
tree-function languages ignore `edited`.

`ReparseWork::run` — **one effect per document, one in-flight per
document's reparse lane** —
calls `parse_syntax` for the root, reconciles child markers (exact
shifted-range + language match → id and syntax ride over; fresh sites
mint empty), then **descends**: every fresh or edit-touched child parses
in the same run, top down, trees in hand. There are no cross-effect races
because there are no cross effects: syntaxes are hierarchical, so the
hierarchy parses as one unit. himarkdown registers `"markdown"` in the
registry like any grammar; the root's only distinction is that string.

## Landing

One swap: replay the since-capture edits over the outcome's root
(markup, channels and tree), carry live inlay views in the invalidated
ranges, assign `Document::syntax`, re-lay editors over the invalidated
ranges. Guards:
a landing overtaken by an edit drops itself — that edit already captured
fresh work (the repairs convergence argument); ids nothing re-claimed die
with the outgoing markup (no leak).

## Non-derived state

Because ids are stable across ordinary edits, a syntax may carry
authoritative state — a fold flag. The degradable rule stands:
structural changes (block split/merge, boundary ambiguity) mint fresh
ids and reset such state; what cannot tolerate a reset belongs in the
document, *referenced* by a syntax. Derived state's reset cost is a
re-parse; that asymmetry is the whole safety argument. (Rich widgets
— tables, diagrams — deliberately do NOT ride syntax state: they are
enrichment-pass products in their own feature markups,
docs/editor/editor-enrichment.md; the parse stays structure-only for them.
Checkboxes ride the syntax markup — deliberately parse-emitted,
cheap and sync.)

## Feature markups

The recursive markup above is the document's OWN — text-derived
structure. Everything a *feature* paints lives beside it, in
`Document::markups: HashTrieMapSync<MarkupId, Markup>` (persistent, like
everything a `Document` clone rides). **One feature instance, one
markup**: a search panel's tints are one entry, a diff pane's washes are
one entry per document. A user rarely adds a single marker — they add a
feature, and a feature controls an arbitrary set of intervals; rewriting
a small dedicated markup wholesale beats cherry-picking keyed intervals
out of one shared set, and it makes coexistence structural: two search
panels over one document, or one document in two diff panes, never touch
each other's entries.

Display is scope-driven; **there is no document-side show list**. Every
markup carries a `MarkupScope`: `View` entries show only on editors that
picked them (`show_markup`; search tints, diff washes, an editor's own
overlay), `Document` entries show on every editor of the document,
current and future — the merge filters the map by the flag (comment
cards, demo badges, the mono covering). Search-result row tints are
View-scoped picks on the panel's own row editors — a panel's markers
never leak into the document's other editors.

Each **editor picks** what its view merges: its unhide `overlay` (itself
just a feature markup the editor owns) plus `Editor::markups`, an
ordered id list — order is merge precedence — then the document-scoped
entries. `OverlaidMarkup` seeds the recursive query with the base
(root-syntax) markup and the extras; consumers still see one query.

Lifecycle, all on `Document`:

- `add_markup() -> MarkupId` — allocate an empty View-scoped entry.
  (`add_owned_markup(editor)` ties the entry's life to an editor.)
- `ensure_document_markup(id)` — install a feature's own static id,
  Document-scoped (idempotent).
- `show_markup(editor, id)` — join a View-scoped display pick.
- `replace_markup(id, replacement, changed, ...)` — the wholesale
  swap, and the ONLY door for feature-derived content (scope survives
  — it rides the entry, not the replacement value). **The producer
  brings the change set** — whoever derived the replacement (a
  background search scan, a diff pane's marks-window-bounded
  derivation) also knows what moved; computing a set diff inside
  `Document` would be linear on the UI thread, which is prohibited.
  `changed` damages exactly those ranges on the displaying editors
  (decorations bake into shaped runs), bumps `markup_generation`
  (discarding in-flight repair captures), repairs the visible parts
  synchronously and returns effects for the tails. An empty `changed`
  swaps silently — a no-op derivation must not starve repairs.
- `remove_markup(id, changed, ...)` — same contract; the id leaves every
  display pick. A pane whose own editors leave with it passes nothing.
- Closing an editor removes its owned entries; features clean up their
  ids when they close.
- Beside the wholesale door, the document owns TARGETED doors for the
  entry lifecycles it manages itself: the inlay lifecycle
  (`push_inlay`/`remove_inlay`/`swap_inlay`/`replace_inlay`), the
  unhide refresh, and syntax installation — same damage discipline,
  point writes instead of derivation swaps.
