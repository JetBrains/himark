# The document: one value, every projection inside

`Document` (frontend/editor/src/document.rs) is the editor engine's
central value. [documents.md](documents.md) tells the STORAGE story —
where documents live, who holds one, when one dies. This is the
VALUE's story: what a document is made of, and why everything that
depends on the text — markup, diffs, layouts, carets — lives inside
the same struct instead of beside it.

## The layout

```rust
pub struct Document {
    text: Text,                       // the substance: a rope

    parsed_revision: u64,             // the revision `syntax` parsed
    syntax: Option<Syntax>,           // language + tree + derived markup

    log: EditLog,                     // every operation, in order
    undo: UndoHistory,

    markups: HashTrieMapSync<MarkupId, Markup>,
    markup_generation: u64,           // change probe for consumers
    markup_changes: QueueSync<MarkupChange>,
    scroll_stripe_generation: u64,    // the stripe sweep's input

    fragments: HashTrieMapSync<FragmentSetId, Intervals<…>>,
    diffs: HashTrieMapSync<DiffId, Diff>,

    editors: HashTrieMapSync<EditorId, Editor>,

    repair_token: Option<CancellationToken>,   // the repair lane
    reparse_token: Option<CancellationToken>,  // the reparse lane
    enrich_base: Option<ResourceLocation>,
    enrich: HashTrieMapSync<EnrichKey, EnrichSlot>,

    token: DocumentToken,             // landing guard (stale-result check)
}
```

Every collection is persistent (`rpds` — [code-style.md](../code-style.md)):
a `Document` clone is pointer bumps, so the store's copy-on-write
commit and every worker snapshot are O(1), and a frame renders from a
value that cannot be half-mutated.

The pieces, and where each is documented:

- **`text: Text`** — the rope
  ([rope-and-intervals.md](rope-and-intervals.md) is the substrate):
  byte/line/point metrics answered by seek, never by scan. The text is the only ground truth; everything else in the
  struct is derived from or positioned against it.
- **`log: EditLog`** — the append-only list of every `Operation`
  applied, with an `EditIdentity` per entry. `compose_since(revision)`
  is the universal catch-up: workers land against the revision they
  captured and rebase over what composed meanwhile
  ([rebase.md](rebase.md), [diff.md](diff.md) decision 3). The
  document's REVISION is this log's length.
- **`syntax` + `parsed_revision`** — the parse snapshot: language
  name, the `dyn SyntaxTree`, and the markup/folds/outline derived
  from it ([highlight.md](highlight.md),
  [lazy-languages.md](lazy-languages.md)). `parsed_revision` says
  which text the tree matches; `edited_since_parse()` is how
  consumers (the structural diff's tree snapshots,
  [structural-diff.md](structural-diff.md)) know the tree is exact.
- **`markups`** — every markup entry anyone shows on this text, each
  a two-lane interval structure ([markup.md](markup.md)): a BULK lane
  of styled spans and a STRUCTURE lane of inlays/hides/alignment.
  Entries are document-held so one edit shifts them all;
  `markup_generation` is the O(1) "did anything change" probe.
- **`diffs`** — every tracked diff whose TARGET is this document:
  `Diff { operation, base_revision, markup, generation }`
  ([diff.md](diff.md)). Held here so the edit door can compose them
  (below); tracked and normalized by the registry's diff machinery
  ([diff-stripes.md](diff-stripes.md)).
- **`fragments`** — keyed interval sets features anchor ranges in
  (diff fragments, bounds) that must shift with edits like markup
  does.
- **`editors`** — the projections; the next section.
- **The lane tokens** — the document OWNS its repair and reparse
  lanes: the in-flight token is stored here and `relaunch`ed on
  supersession ([../ui/effects.md](../ui/effects.md)).
- **`token: DocumentToken`** — the landing guard: an async result
  carries the token it was launched with, and a landing against a
  reopened or replaced document discards cleanly.

## One edit door

`Document::edit` is the only way text changes, and it admits an
operation only if it covers the text EXACTLY (`old_len == byte
count`). There is no padding and no clamping tolerance anywhere
behind the door — `Text::edit`'s clamps mean a mismatched operation
would not crash, it would silently corrupt and desync every reader,
so the door is the reject point: a mismatch is a debug assert in
development and a logged no-op in production (the empty operation is
the one explicit no-op). Producers own their trailing retain
(`Operation::insert_in`/`delete_in` are the exact-splice
constructors; `insert_at`/`delete_at` are partial by construction and
only for consumers that complete the coverage themselves, like the
inlay write-through forwarder). Anything computed off-thread against
a snapshot must be revalidated or rebased through the edit log before
it reaches the door — never recomputed on the UI thread, and never
trusted by length alone (the 2026-09-24 crash was a stale worker diff
installing over a live document).

Admitted, the edit mutates EVERYTHING in one pass — first the
substance, then every projection:

1. splice the rope; shift every `markups` entry and `fragments` set;
   **compose every `diffs` entry** with the operation (a tracked diff
   never goes stale, only non-minimal — [diff.md](diff.md) decision
   2); append to `log`; note `undo`.
2. for EVERY editor: capture the viewport's anchor against the
   pre-edit layout, `layout.edit(operation)` (shift the layout rope),
   run a BOUNDED repair around the edited region (the layout
   section below), and set `settle_to` so the settle pulse re-aims
   the scroll before paint
   ([viewport-preservation.md](viewport-preservation.md)).

Because both halves happen inside one `&mut Document` before the
value is put back, no observer — a paint, a worker capture, a store
snapshot — can ever see text with markup, a diff, or a layout that
has not caught up. Consistency is by construction, not by protocol.

## Why editors live inside the document

An `Editor` (frontend/editor/src/editor.rs) is a PROJECTION of this
text and nothing else:

```rust
pub struct Editor {
    layout: DocumentLayout,   // the layout rope at THIS width
    carets: MultiCaret,
    markups: Vec<MarkupId>,   // which document markups this view shows
    folds, before, bounds,    // fold state, before-cards, clip range
    seat: SeatKey,            // input identity (IME) — shared by clones
    softwrap, scroll_x, target_width,
    viewport: Option<Range<f32>>,
    marked, drag, placeholder, settle_to, …
}
```

Its `DocumentLayout` is a rope of laid elements with a width tree and
a damage window — heights positioned against byte offsets OF THE
TEXT. A layout is meaningless one revision away from the text it was
built for; a caret is a byte offset; a viewport is a y-range into
that layout. None of it has independent existence.

Storing editors IN the document makes that dependence structural:

- **Atomic projection update.** The edit door walks `self.editors`
  and moves every layout in the same mutation as the text (§ above).
  If editors lived beside the document — in views, in a separate
  repository — every edit would need a fan-out protocol, and every
  frame between fan-outs would render a torn pair. There is no
  "editor catch-up" code in this codebase because the situation
  cannot arise.
- **Views stay identity-thin.** A pane holds `(DocumentId, EditorId)`
  and gathers ([documents.md](documents.md)); two panes over one
  document are two `Editor` entries over one text — split views,
  embedded diff halves, the inline face — each with its own width,
  wrap, carets and viewport, sharing everything positional beneath.
- **Lifetime falls out.** Editors ARE the document's refcount: the
  document leaves the registry when its last editor is removed — the
  retraction rule ([documents.md](documents.md)). No separate
  liveness bookkeeping can drift.
- **Clones are the same logical editor.** The store's copy-on-write
  clones documents freely; an editor's `seat: SeatKey` rides the
  clone, so focus and IME recognize the persistent copy as the same
  input site ([../ui/ime.md](../ui/ime.md)).

The trade is honest: a document value carries its editors' layouts,
so cloning is only cheap because every field is persistent — which is
exactly the [code-style.md](../code-style.md) discipline, enforced
here where it matters most.

## `DocumentLayout`: damage in, repairs out

Each editor's `layout` is where text becomes geometry
(frontend/editor/src/document_layout.rs):

```rust
pub struct DocumentLayout {
    rope: Rope<LayoutElement, LayoutMeasure>,  // laid runs: byte_size, height,
                                               // width, spacer_above, safepoint
    widths: WidthTree,          // max line width by range (sumtree.md)
    damage: Damage,             // dirty byte ranges — an Intervals<u32, ()>
    layout_width: f32,
    window: Option<Range<u32>>, // the bounded-build clip
    …
}
```

The rope of laid elements is measured by BYTES and height
([rope-and-intervals.md](rope-and-intervals.md)), so `byte_at_y` and
`height_before` — the questions scrolling, alignment and stripes ask
per frame — are O(log n) seeks over CACHED heights, never a re-lay.

**Invalidation is a splice plus a mark.** `layout.edit(operation)`
walks the operation with a cursor: retained regions keep their laid
elements and heights untouched; each insert/delete splices the
element run it lands in and records the spot in `damage` — greedy
intervals in the same intervals structure markup uses, so damage
ranges themselves shift correctly under later edits. Nothing is
re-laid at the door; geometry questions keep answering from the last
laid heights until a repair replaces them.

**Repair runs in two rings:**

1. **The viewport, synchronously.** The edit door calls
   `repair_layout_bounded(…, anchor, height_budget)` — re-lay from
   the first damage around the viewport anchor, under a HEIGHT
   budget. The frame that shows the edit shows true heights where
   the user is looking, and `settle_to` re-aims the scroll over
   whatever height the repair moved
   ([viewport-preservation.md](viewport-preservation.md)).
2. **The rest, on the repair lane.** Leftover damage is the
   document's own background concern: `pending_repairs` builds a
   `RepairEffect` — `document.substance()` (the document value MINUS
   its editors: the persistent value is its own worker snapshot)
   plus, per editor with pending damage, its layout clone, width and
   viewport anchor — and `fx.relaunch(&mut self.repair_token, …)`
   supersedes any in-flight run ([../ui/effects.md](../ui/effects.md)).
   The worker repairs outward from each editor's anchor under a
   generous budget (`WORKER_REPAIR_BUDGET`) and lands
   `ApplyRepair`; the landing swaps healed layouts only if the
   captured revision and markup generation still hold — a stale
   repair is dropped, and the next `pending_repairs` relaunches
   against fresh state. Heights that change above the viewport ride
   the same settle discipline, so background healing never moves
   what the user is reading.

`pair_managed` editors opt out of the plain lane: a diff pair's
halves must repair TOGETHER so their spacers stay aligned, and ride
the paired repair effect instead ([diff.md](diff.md) §alignment).

## What is deliberately NOT in the document

- **Identity and location** — the registry's
  (`OpenDocuments`, [documents.md](documents.md)): a document does
  not know its own `DocumentId` or `ResourceLocation`; containment is
  the link.
- **Window and panel state** — which pane shows which editor is the
  workbench's ([../ui/workspace.md](../ui/workspace.md)).
- **The world** — files, saves, watches, language servers arrive as
  effects through the registry and the protocol
  ([../ahp/host.md](../ahp/host.md)); the document is a pure value
  that never performs I/O.
