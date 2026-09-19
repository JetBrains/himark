# Diffs as document state, and gutter stripes

Gutter stripes: the working document's changes against a BASE (a VCS
HEAD, another document) shown as colored bars in an ORDINARY editor's
gutter — green added, blue modified, a red marker where lines were
deleted — with a click expanding the before-text inline. The same diff
substance the split pane shows, in a second face.

A second face is why the diff does not live in any view: **a diff is
document + registry state, maintained live whether or not any pane
shows it**, and every face — the split pane, the gutter stripes, the
scroll track (docs/editor/scroll-stripe.md), the inline view — reads the one
maintained value.

The load-bearing decisions, up front:

1. **A diff lives ON the target document**: `Document.diffs`, a
   persistent map `DiffId → Diff { operation, base_revision, markup,
   generation }`, where the operation turns the base text into THIS
   document's CURRENT text. The one edit door composes every applied
   operation into every entry — own-side validity is structural, like
   the edit log's.
2. **Base edits are applied EAGERLY, by the registry.** The entry
   stores the base revision its old side matches — a compose CURSOR;
   the batch-tail sweep (§2) composes the missed ops into the target
   entries at the post-scatter choke. After every dispatch both sides
   of every entry are current; readers never roll.
3. **`OpenDocuments` owns the pairing and the maintenance.** A
   registry table says which diffs exist, between which documents, and
   holds the normalization lane (the installed diff policy —
   docs/editor/structural-diff.md) per diff. Views hold a
   `DiffId`, never a diff.
4. **There is ONE DIFF MARKUP — `Diff.markup` — and every face reads
   it**: whole-document hunk intervals derived FROM the operation,
   feeding the pane's line washes, the gutter's `StripeWalk`, and the
   scroll track alike. The pane keeps only what is genuinely its own —
   the windowed word tints and the fold strips — in a pane-owned
   right-extras entry; the base side's washes live on the record's
   `base_markup`.
5. **The split pane keeps only presentation**: spacer alignment (the
   paired repair), the fold strips, the word tints. Operation
   maintenance, normalization and the hunk markup are the document's
   and the registry's.

---

## 1. The `Diff` on the document

```rust
// crates/editor — beside MarkupId, minted the same way:
pub struct DiffId(u64);

// Document gains:
diffs: HashTrieMapSync<DiffId, Diff>,

pub struct Diff {
    /// base@base_revision → THIS document@its current revision.
    /// The new side is ALWAYS current — maintained at the edit door.
    operation: Operation,
    /// The base document's revision the old side matches — the
    /// compose CURSOR the registry's eager update reads and bumps
    /// (§2); normalization landings bump it too.
    base_revision: u64,
    /// THE diff markup: whole-document hunk intervals
    /// (`DiffAdded`/`DiffModified` spans, zero-length `DiffDeleted`
    /// deletion markers), derived from the operation (§3). Ordinary
    /// editors never SHOW it (View scope) — its washes tint only the
    /// faces that opt in.
    markup: MarkupId,
    /// Bumped per normalization landing. Faces that cache anything
    /// derived from the operation (the pane's pairing) compare this
    /// instead of deep-comparing operations.
    generation: u64,
}
```

**The edit-door invariant.** The one place every text mutation already
passes (the edit log, undo recording) composes the applied operation
into every entry: `diff.operation = diff.operation.compose(&edit)`,
O(diffs · log n). Typing, IME commits, multi-caret bulk edits, undo,
paste, chunk applies, the watcher's refetch — no edit path can leave a
diff stale on its own side, by construction rather than by coverage.

**The base side.** The base document's edit door cannot see entries
stored on the target (and `Document` cannot name other documents —
`DocumentId` is registry vocabulary, a layer up). Cross-document
maintenance is therefore the REGISTRY's job, and it is EAGER: when a
base document's revision moves, the sweep updates every affected
target entry (§2). The update is one idiom
(`Diff::apply_base_edits`, surfaced as
`Document::apply_diff_base_edits`):

```rust
let a = base.log().compose_since(diff.base_revision)?;   // base@cursor → base@now
diff.operation = a.invert().compose(&diff.operation);    // base@now → target@now
diff.base_revision = base.revision();
```

`base_revision` is the compose cursor, not a laziness flag: it says
which ops the update owes, makes running it twice compose nothing
(idempotent), and turns a MISSED choke point into staleness the next
sweep heals instead of corruption nobody can detect. `None` (history
no longer reaches the cursor) falls back to a fresh normalization
capture.

**Why eager is safe** — the gather/scatter clobber (docs/ui/UI.md) does
not bite here: the registry's write runs at the POST-SCATTER choke,
when no stale gathered clones of the target are outstanding — the
sweep is the batch's last writer to the entries. The one consumer
that needs currency EARLIER is the split pane (spacer sync runs
mid-perform, before its scatter); it pre-empts with the same idiom
over the documents in its hands (`DiffState::attach` runs
`apply_base_edits` on its own clone), and the sweep then finds the
cursor current and does nothing.

In the stripes case the base is a revision-pinned document that never
changes, and the whole story is a no-op forever.

**Lifecycle on the document**: `add_diff(operation, base_revision) →
DiffId` allocates the entry AND derives its markup at birth (§3);
`remove_diff(id)` removes both. `install_normalized_diff(id,
operation, base_revision)` is the normalization landing's door —
operation, cursor and `generation`+1 in one write.
`install_diff_markup(id, markup, changed, derived_at, …)` lands the
worker-derived markup: it only shifts the markup and the change set
through `compose_since` and swaps — nothing at a landing walks a
markup or an operation. Entries ride document clones like everything
else — persistent map, O(1).

## 2. `OpenDocuments`: the diff registry

The pairing — which document is whose base — is registry state,
beside the documents it connects: a **`Diffs`** struct inside
`OpenDocuments` — the records, a BIDIRECTIONAL index (`by_target` /
`by_base`) so lookups never scan, and the pane-view table
(`diff_views`):

```rust
struct Diffs {
    records: HashTrieMapSync<DiffId, DiffRecord>,
    diff_views: HashTrieMapSync<u64, DiffView>,   // the panes' gathered state
    by_target: HashTrieMapSync<DocumentId, Vec<DiffId>>,
    by_base: HashTrieMapSync<DocumentId, Vec<DiffId>>,
}
// Queries: of_target(id), of_base(id), by_pair(base, target) — the
// track dedup — and stripe_diff(target). Every write goes through
// Diffs' own methods, which keep the indexes in step with the
// records — the invariant lives in one place.

struct DiffRecord {
    base: DocumentId,
    target: DocumentId,
    /// The base-side wash markup (Deleted washes + word tints),
    /// allocated on the BASE document. The base holds no Diff entry —
    /// its washes are just a feature markup the derivation rewrites.
    base_markup: MarkupId,
    /// Faces holding this diff: a pane, the stripes join. The last
    /// untrack releases.
    refs: u32,
    /// True for the document's canonical base diff — what the stripes
    /// gather joins. One per target.
    stripes: bool,
    /// THE normalization lane (docs/ui/effects.md): a newer capture
    /// cancels and takes the slot.
    normalize_token: Option<CancellationToken>,
    /// The (base, target) revision pair the last launch covered —
    /// the sweep's don't-relaunch fingerprint.
    normalized: Option<(u64, u64)>,
}
```

- `track_diff(store, base, target, stripes, prepared) → DiffId` —
  dedups by (base, target): a pane and a stripes join over the same
  pair share one diff (refs). A fresh track computes the initial diff
  SYNCHRONOUSLY (callers open diffs over reasonable documents; a lazy
  first normalization is the listed escape if stripes-on-open over
  monsters ever bites), installs the entry on the target — markup
  derived at birth — allocates `base_markup` on the base, and, with
  `stripes: true`, seats the markup on every enabled scroll track
  (docs/editor/scroll-stripe.md). With a `prepared` operation (the atomic
  pane open, docs/editor/diff.md) the entry installs NORMALIZED AT BIRTH —
  generation 1, `record.normalized` stamped, no first normalization
  owed; ignored on a dedup.
- `untrack_diff(store, id, fx)` — refs down; at zero: cancel the
  lane, `remove_diff` on the target, `remove_markup` on the base.
- **Keep-alive**: `remove_if_editorless` (docs/editor/documents.md) spares a
  document that is a tracked diff's base or target. The stripes base
  is editorless by NATURE — a hidden HEAD snapshot no pane shows — and
  must not be reaped while its diff lives. Untracking releases it
  through the ordinary rule.

**The sweep**: `sync_diff_lanes` runs at the tail of EVERY perform
batch — ONE choke covering every dispatch path uniformly, with no
per-view call to forget. O(1) when nothing is tracked; O(tracked
diffs) compares otherwise. Per record, in order:

1. **applies base edits** — when the entry's cursor trails the base's
   revision: gather the target, run the §1 idiom, put back. Safe here
   because the scatter already ran (§1, "why eager is safe"); a
   cursor the log no longer reaches forces a fresh normalization
   instead;
2. **relaunches normalization** when the (base, target) revision pair
   moved past `record.normalized` — `fx.relaunch` on the stored
   token, so a typing storm holds one in flight.

Nothing bypasses the batch tail — even programmatic edits ride some
dispatch — so coverage is structural; and were a path missed, the
cursor makes it staleness the next batch heals, never corruption.

**Normalization** (`DiffNormalizeEffect`) captures both texts +
revisions, an O(1) clone of the standing markup, the language + tree
snapshots when they exactly match the texts, AND the edge-installed
diff policy (`env::Differ`); the worker runs the policy — structural
alignment where trees came along, Myers otherwise
(docs/editor/structural-diff.md) — derives the fresh markup
(`editor::diff::hunk_markup`), and set-diffs it against the captured
one (`editor::set_diff` — the producer brings the change set,
docs/editor/markup.md). **Landing rebases instead of discarding**: the
registry gathers the target, rebases the minimal diff through BOTH
logs — `a.invert().compose(minimal).compose(b)` over the edits that
arrived while it ran — writes operation + cursor + generation
(`install_normalized_diff`), lands the markup through the entity's
own effect scope (`install_diff_markup` — shift and swap only), and
broadcasts `DiffChanged { diff }` through every window's tree after
the commit (the `FileChanged` pattern) — a displaying pane answers
`Resync` and adopts the new pairing in its settle even when the user
is idle.

## 3. The markup: one entry, two build stages

The diff markup is built in TWO STAGES, deliberately separate:
the policy builds the Operation — the diff's truth — and
`editor::diff::hunk_markup(&operation, target_text)` derives the
markup FROM it — the presentation stage, and the seam where
presentation-level options (whitespace, word preferences) will
parameterize without touching the diff itself. Hunks group like the
fragment walk — interior retains without a newline stay inside their
hunk, probed on the target text — and are styled with the wash
styles: `DiffAdded`/`DiffModified` spans plus zero-length
`DiffDeleted` markers. `add_diff` derives it at birth; the normalize
worker runs both stages beside every policy run.

The consumers:

- **the split pane's halves show it** — whole-document line washes,
  no window to chase, washed from birth. A modified line washes with
  the MODIFIED tint;
- **the gutter classifies rows from it** — `viewport::StripeWalk` is
  an interval walk over the entry (§5); standing stripes shift at the
  edit door the same frame, and a fresh edit's OWN stripe is one
  normalize landing behind (the colors contract) — pinned by
  `standing_stripes_shift_with_typing_and_fresh_hunks_land_with_the_normalize`;
- **the scroll track projects it** — through the per-editor
  registration doors, never by enumerating the document's `diffs`
  map: a split-diff panel's own tracked diff over the same document
  must not leak onto an ordinary pane's track
  (docs/editor/scroll-stripe.md).

What stays OUT of the entry: the pane's windowed WORD tints and the
fold strips live in a pane-owned right-extras markup
(`Document::add_owned_markup` — dying with the right half's editor;
`DiffView.right_extras`, a `DiffState::attach` parameter), and the
base side's washes live on `base_markup`. Both are derived
viewport-windowed on the pane's paired-repair rider (the marks job,
docs/editor/diff.md) — word-level derivation is the expensive presentation
the 4KB-grid window exists for, and it stays with the one face that
shows it. The `stripe` colors ride the `diff-added/modified/deleted`
styles in both theme JSONs; there are no separate stripe styles.

## 4. The split pane, slimmed

`SplitDiffView` takes the pair plus `DiffId` and keeps exactly the
presentation the documents cannot hold:

- **stays**: the paired repair-and-align lane (`pair_repair_token`,
  `pair_seq`, `align_pending`, the visible-sync/owe-the-rest split
  and the worker's unlimited hunt), the fold strips and their latch,
  the marks job (word tints + base washes, windowed), the inline
  face's state. The inline share of an owed span is HARD-CAPPED: the
  viewport window degenerates to the whole document when a half's
  layout is mostly unlaid (`byte_at_y` clamps past the laid extent) —
  the monster probe caught multi-megabyte inline walks through
  exactly that hole; the full span rides the worker regardless, so
  the cap only defers alignment.
- **leaves** (to the document and the registry): the operation, the
  revisions of record, the optimizer lane, the hunk markup.

The settle: bring the entry's base side current with the §1 idiom
(the pane holds both documents and needs the live pairing BEFORE its
spacer sync — it pre-empts the sweep, which then finds the cursor
current and no-ops), compute touched regions from its own
settled-revision pair + drained heals, spacer-sync, done. No
optimizer launch — the batch-tail sweep covers it. A normalization
landing is observed, not received: the pane keeps
`(seen_generation, seen operation)` — a persistent clone, cheap —
and when the entry's generation moved, computes the disagreement
regions, syncs visible / owes the rest, and re-clones. Same alignment
math, one dispatch later than an in-band landing, which alignment
tolerates by design (spacers are derived data).

`DiffPair` (hidiff) tracks on open (`track_diff`, refs) and untracks
at dismantle; its gather passes the id through. The
`RepairDiffEffect` keeps repairing both halves and aligning spacers
on the worker.

## 5. Stripes in the gutter

The second face, and the reason for all of the above.

**The join.** `EditorView` carries an optional base:

```rust
pub struct EditorView {
    // …document, editor, reports_geometry, location, gutter_width…
    /// The stripes diff: the base document (for the before-card's
    /// fragment walks) and the entry id on THIS document.
    pub base: Option<(Document, DiffId)>,
}
```

The GATHER joins it, like the location join (docs/editor/documents.md): a
pane node built `with_gutter` asks
`OpenDocuments::stripe_diff(document_id)` — the record flagged
`stripes` — and clones the base document beside the target. Only a
gutter-bearing editor joins — stripes need BOTH the diff join and a
gutter; value editors, search rows, diff halves gather no base and
render no stripes.

**Classification rides the viewport build.** `EditorViewport::build`
(docs/editor/gutter.md) already walks the visible lines once; each row gets
a `diff: Option<DiffLineKind>`:

```rust
enum DiffLineKind { Added, Modified, DeletedAbove }
```

The `StripeWalk` — created once before the loop, only when the gutter
is on — walks the diff markup's hunk intervals in lockstep with the
rows: a full-row `DiffAdded` span → `Added` (`Modified` if a deletion
marker sits at the row's start); a partial add, a `DiffModified`
span, or an interior deletion marker → `Modified`; a bare marker at
the row's start → `DeletedAbove`.

**Rendering.** A stripe column at the gutter's left edge (before the
numbers), painted by `EditorGutterView` from the shared viewport:
per-row bars in the `ui.editor_gutter.stripe_*` chrome (width, inset,
the three colors — themed beside `ui.diff`, 2x-tuned in both theme
files, never hardcoded), `DeletedAbove` as a right-pointing triangle
centered on the row's top edge (docs/editor/gutter.md §3 has the exact
geometry and click bands).

**Cost.** The walk is View × log(Doc): one interval walk beside the
viewport's rows; the base document clone is O(1); no window churn,
no landings in the loop — standing stripes are ALWAYS current because
the markup's intervals shift at the edit door, and a fresh edit's own
hunk is one normalize landing behind.

## 6. The before-card (click a stripe)

A stripe click answers `EditorCommand::ToggleBeforeInlay { at }` (the
row's first byte, resolved by the gutter's log-time seeks). The
base-joined view is the door — only it can map the range; value
editors and rows ignore the command. `Document::toggle_before_inlay`
takes an explicit `animate: bool` and resolves at click time against
the live operation (a stale click degrades to a no-op, the
chunk-action rule):

- **the changed BLOCK, not one fragment**: fragments split at
  retained newlines by design, so consecutive changed rows arrive as
  several — the resolution merges fragments whose target hard lines
  touch or continue each other (a deletion's marker row included)
  into the visual block. CAPPED at 64KB, and the walk is seeded one
  cap early (the fragment filter drops mates ending before the
  seed): a click on a wholly-rewritten monster walks ~two caps of
  ops, once.
- **"range here — range there"**: the block's target lines anchor the
  card; the merged fragments' left union, widened to whole base hard
  lines, is what it shows. Toggle: a standing card intersecting the
  anchor leaves instead.
- the card is an `Above` inlay in an EDITOR-OWNED markup (the fold
  recipe — `Editor.before`, lazily allocated, dying with the editor;
  two panes card independently), pushed as `InlayMode::Above` and
  projected `over_aligned(INLAY_HOST)`. It hosts
  `BeforeInlay { view, base_range, grow }`: a fragment-bounded,
  COMPLETE-laid child editor over a SELF-CONTAINED CLONE of the base
  document (`add_fragment_set`/`add_fragment`/`add_editor(Complete,
  [wash])` — the anonymous-value rule, docs/editor/documents.md), washed
  `DiffDeleted`. The anchor is an ordinary interval: edits shift the
  card with its lines.
- **the card sits FLUSH** — no frame, no pads, no card chrome
  (`ui.diff` carries only the spacer fill and the fold-strip chrome).
  What tells the base text apart is its own gutter: the card renders
  one at the host's `editor_gutter.width` carrying the BASE
  document's line numbers, so its columns line up with the host
  editor's exactly.
- **appearing is a height clip, with live content.** A user's gutter
  click passes `animate: true` — the card is born `appearing`, an
  `Animation<f32>` 0→1 (Ease, 160ms, EaseOut) driving the card's
  height, and the growing height CLIPS the reveal: content is laid
  and live during the growth, never an empty surface. A programmatic
  mount (the inline face expanding every block at mint,
  `expand_before_inlays`) passes `animate: false` and the card is
  born `settled` at full height — animation announces a change the
  user caused, not one the pane was born with.
- **FOCUSABLE, with passive plumbing.** Commands route as
  `BeforeCommand::{Editor, Rewrap(f32), Tick}`; `passive()` — Rewrap,
  Tick, and the plumbing editor arms (ApplyRepair, ApplyReparse,
  ApplyEnrichment, Retheme, Viewport, ViewportTop, Drag, DragEnd) —
  never moves focus, in the host's inlay router and in the card's
  own perform alike: focus rides clicks, and a drag continuation or
  release reaching a card must never move it. A clicked card takes
  the inlay focus; its caret/selection are live, and its launches
  (resize repairs) ride the ordinary lanes and land back in the
  card.
- **resize speaks through ONE door**: the paint pass reports a stale
  width once per change (never per scrolled frame) as
  `BeforeCommand::Rewrap(want)` with
  `want = (size.width - gutter_width).max(120.0)`.
- deferred: a header strip with dismiss/revert, word tints inside the
  card, live (registry-backed) base content, a departure animation on
  toggle-off.

## 7. Who tracks: the VCS base

One ask, the FindEffect shape (core declares, higit answers,
himark-api wires under the vcs gate):

- **`FetchBaseEffect { location } → Option<ResourceLocation>`** — a
  working document's own location in, its base's location out.
  higit's `base_location` probes the repository, pins the CONCRETE
  sha HEAD resolves to (the base's content then matches its identity
  forever; a commit mints a different base for later opens), and
  answers `None` for untracked files, non-repositories, and its own
  minted locations — a base has no base.
- **The base sweep** (`sync_stripe_bases`, run beside the watch sweep
  at every registration point): every located, non-synthetic document
  not yet asked launches ONE ask — the entity's `base_requested` flag
  is the don't-ask-twice latch, dying with the entry so a re-open
  re-asks (`rearm_base_asks` clears it when a repository's answer
  should be retried). Gated on the `StripeBases` marker
  (`AppState::observe_stripe_bases`, installed with the handler).
- **The landing chain**, all existing machinery: `BaseLocated` (an
  open base tracks on the spot; a cold one launches the fetch) →
  `BaseFetched` → `BuildDocumentEffect` → `BaseBuilt` (register born
  clean, deduped by location — the vcs old-side discipline — and
  `track_diff(base, target, stripes: true)`).
- **The release**: a stripes track is tied to the document's OPEN.
  `release_editorless` untracks it (`untrack_stripes` — clears the
  stripes flag FIRST so a pane-shared ref can never double-drop, then
  drops the sweep's ref, and unseats the markup from the scroll
  tracks even while a panel's ref keeps the diff alive), and the
  untrack's own retraction releases the base and the document. The
  sweep re-tracks on a future open.

Deferred: HEAD moves (branch switch, commit) re-minting the base —
the freshness story rides watching; diff-context gutter buttons;
per-hunk staging; stripes against anything but the tracked base.
