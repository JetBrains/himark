# Editor enrichment: background passes over syntax

The parse pipeline delivers STRUCTURE only: text → tree → token
markup, on the one hierarchical parse effect (docs/editor/markup.md), with
typing latency to fresh colors as its budget. Everything derived ON
TOP of that structure — a rendered diagram, a table widget, an
embedded editor, a fetched image, a brace-match tint — is an
**enrichment pass**: a background job on its own lane that reconciles
derived widgets and decorations with the syntax information. Plural
by design: a slow diagram render must not delay a brace-match tint.
Nothing linear ever runs on the UI thread (the Design.md three-loops
rule; the `replace_markup` contract states it outright).

The load-bearing decisions:

1. **Parse delivers structure only.** `SyntaxLanguage` has two honest
   jobs — tree and token markup. Widget construction, rendering,
   measurement, and anything that could ever await live in
   enrichment. The parse effect's cost is the grammar's own cost,
   full stop.
2. **An enricher owns a feature markup.** Each pass's output is one
   markup entry (docs/editor/markup.md, feature markups — "one feature
   instance, one markup"), written through `replace_markup` with the
   producer-supplied changed set. The entry is `Document`-scoped for
   document-truth enrichers (diagrams, tables, embeds, images) and
   `View`-scoped for editor-truth ones (brace match, occurrences).
3. **A pass is an effect on a lane.** One lane per
   (document, enricher) — per (document, editor, enricher) for
   caret-keyed passes — driven by the standing `fx.relaunch` idiom:
   a new trigger supersedes the in-flight run, and the landing-side
   staleness guards remain the correctness layer (docs/ui/effects.md —
   cancellation is hygiene, guards are truth).
4. **Reconcile is the PASS'S policy, never the core's — and the pass
   never mediates.** The core provides mechanics only: intervals
   shift at the edit door, landings are guarded, output applies
   through the one `replace_markup` door, and the enricher gets a
   synchronous landing-time hook to marry its background output with
   the CURRENT live entry. What to preserve, adopt, rebuild or drop
   is each enricher's own decision. And the pass is never in the
   interactive path: widget commands, write-through edits
   (`InlayOutcome::edit`), local applies and focus routing run
   synchronously through the standing inlay roads — a keystroke never
   waits on enrichment.
5. **Async rides `EffectCaller`, and the base rendering is the honest
   fallback.** A pass that needs the outside world — a document open
   through the seat, a resource read — calls declared effects and
   awaits inline: the future parks, the worker drains past it,
   nothing anywhere blocks. Until the landing arrives there are NO
   placeholders to invent: an unenriched fence is an ordinary colored
   code block, an unenriched table is markdown source — the
   document's own rendering is the loading state, the same honesty
   class as "newly typed text is uncolored until its landing"
   (docs/editor/highlight.md).
6. **Caret-keyed passes are the same machinery at editor scope**,
   with a `View`-scoped markup the owning editor picks
   (`show_markup`) and a lane keyed by the editor. A frame or two
   behind the caret is the accepted contract, same as colors.

## The shape

The machinery lives in `frontend/editor/src/enrich.rs`; registration
is the `SyntaxLanguages` recipe (`AppCommand::RegisterEnrichers`,
built by `frontend-host::enrichment_passes`, `env::Enrichers` beside
`env::Parsers`).

```rust
pub struct EnricherId(pub &'static str);
pub struct Interest { pub syntax: bool, pub carets: bool } // default: syntax only

pub trait Enricher: Send + Sync {
    /// Stable identity: keys the markup entry and the lane.
    fn id(&self) -> EnricherId;
    fn interest(&self) -> Interest { Interest::default() }
    /// The background job. Snapshots in — persistent values, O(1) to
    /// take, internally consistent forever. May await declared
    /// effects through `cx.caller`; parked, never blocking.
    fn derive<'a>(&'a self, input: &'a EnrichInput, cx: &'a EnrichCx<'a>)
        -> EnrichFuture<'a>;         // Pin<Box<dyn Future<Output = Enrichment>>>
    /// The landing-time marry: the background output meets the
    /// CURRENT entry. Synchronous, bounded to the changed ranges.
    /// Default: carry_live_views_in — live widgets inside changed
    /// ranges ride over into the replacement.
    fn reconcile(&self, replacement: &mut Markup, changed: &[Range<u32>],
        live: &Markup, fonts: &FontCollection, theme: &Theme) { … }
    /// Main-thread hook with the store in hand — where a pass mounts
    /// editors or other store-registered state. Default no-op.
    fn install(&self, store: &mut Store, replacement: &mut Markup,
        changed: &[Range<u32>], fonts: &FontCollection, theme: &Theme) {}
}

pub struct EnrichInput {
    pub text: Text,
    pub syntax: Syntax,               // the root hierarchy, cloned (O(1))
    pub revision: u64,                // the landing guard's anchor
    pub changed: Vec<Range<u32>>,     // what this run must re-derive
    pub previous: Markup,             // the pass's own current entry
    pub base: Option<ResourceLocation>, // for reference-resolving passes
    pub caret: Option<CaretContext>,  // { selection, offset } — caret passes only
}

pub struct EnrichCx<'a> {
    pub fonts: &'a FontCollection,
    pub theme: &'a Theme,
    pub caller: EffectCaller,
    pub languages: Option<Arc<SyntaxLanguages>>,
}

pub struct Enrichment {
    pub replacement: MarkupBuilder,   // fresh output for the changed ranges
    pub changed: Vec<Range<u32>>,
}
```

`Enrichers` is a Vec, not a map — registration order is iteration
order — with a duplicate-id `debug_assert` (ids key entries and
lanes). A pass FINDS its subjects itself: a tree walk over the
changed ranges (`pipe_table` nodes, path-bearing fence info strings,
`inline` nodes with images), bounded by exactly what moved. The
registry knows nothing of fences or tables — an enricher is its own
query.

## Lanes and slots

`Document` keys its enrichment state by
`EnrichKey { enricher: EnricherId, editor: Option<EditorId> }` →
`EnrichSlot { markup, token, launched }`. Slots mint on demand: a
syntax pass's slot (`editor: None`) carries a `Document`-scoped
markup entry; a caret pass's slot is per showing editor —
`View`-scoped, picked into the editor via `show_markup` at mint.
`remove_editor` drops that editor's entries, slots and lane tokens.

## Triggering and scheduling

- **The parse landing is the syntax trigger.** `Document::land_reparse`
  launches every `interest().syntax` pass: full-range on a slot's
  first run, else the landing's invalidated ranges. The document also
  remembers the base its last launch ran with (`enrich_base` —
  bookkeeping, never identity: the registry owns location); a base
  the passes have not seen re-derives the whole document once, so a
  pass resolving references against the document's own location gets
  a run that HAS a base even when the parse was already fresh at
  registration.
- **Caret moves are the caret trigger**: `Document::perform` compares
  the acting editor's carets before and after every command and calls
  `launch_caret_enrichment` on change (landings move no carets, so no
  landing re-triggers itself). Every parse landing ALSO relaunches
  caret passes for all attached editors — the tree changed under
  unmoved carets. Caret runs carry EMPTY `changed`; the pass reads
  its old footprint from `previous` (`Markup::styled_ranges_in`) and
  answers changed = old ∪ new.
- **Supersession per lane**: `fx.relaunch` on the slot's token; the
  landing guard is `serial == launched` — strict last-wins, so a
  caret drag never queues a hundred runs.
- **Passes are independent.** No ordering, no cross-pass reads: each
  derives from (text, syntax, own inputs) alone.

`enrich_now` / `enrich_sync` are the synchronous variant for builders
(cold construction, tests): syntax passes run inline; caret passes
are skipped — there is no acting editor at construction.

## The worker, and the landing

The worker (`enrich::run_work`) calls `derive`, then splices: the
final replacement is the pass's PREVIOUS entry with the fresh
builder's output substituted over the changed ranges — the pass only
ever computes the damage.

The landing (`EditorCommand::ApplyEnrichment` →
`Document::apply_enrichment`) is mechanics, deliberately dumb:

1. **Guards**: document token, slot existence, markup identity,
   `serial == launched`, and a revision from the future drops (its
   superseding run is already in flight).
2. **Rebase**: the output addresses its snapshot's revision;
   `log.compose_since` transforms the changed ranges and edit-steps
   the replacement markup up to the present.
3. **Release**: `destroy_inlays_in(changed)` on the old entry — a
   widget spliced out gets its `InlayView::destroy_view` retraction
   (an embedded editor releases its target document through the
   standing editorless rule; no walk, no carried set).
4. **Reconcile**: the enricher's marry hook against the CURRENT live
   entry — bounded to the changed ranges; anything linear in the
   document here is the pass's bug.
5. **Install**: the enricher's main-thread hook, store in hand.
6. **The one door**: `replace_markup(slot.markup, replacement,
   changed, …)` — damages exactly the changed ranges on displaying
   editors, repairs visible parts synchronously, launches tail
   repairs; an empty changed set swaps silently and starves nothing.

When the host document itself leaves, `release_enrichment` (called
from the registry's removal path) destroys every entry's inlays
whole-range — embedded targets fall out through the editorless rule,
no leak.

## Reconcile policies genuinely differ

- **Tables trust the DOCUMENT.** The source is the single truth
  (docs/editor/table.md's founding rule); widget preservation is best-effort:
  the belief comparison keeps a live view that is a faithful
  continuation of the source (the just-typed cell), and a document
  that changed otherwise rebuilds the widget — losing whatever state
  the pass did not care enough to transfer. Accepted openly: that is
  the degradable rule (docs/editor/markup.md, Non-derived state), and it is
  what keeps the table honest against foreign edits. A write-through
  cell edit's freshly parsed lines MATCH the live view's belief, so
  adoption keeps it — living in the enricher's own markup, the table
  can never be clobbered by a parse landing.
- **Addressable fences trust MEMORY.** A loaded embed — a live editor
  over a registered document, with its scroll, focus and warm fetch —
  is worth more than a re-derivation; old widgets survive as-is when
  only their surroundings moved, and the spec is re-handed only when
  the fence's own text changed.
- Caret tints carry no state — rebuild is free and the default
  reconcile is the whole policy.

## The enrichers

The complete set:

- **`markdown-tables`** (himarkdown `TableEnricher`, syntax): an
  `Instead(FullLine)` `TableEditor` inlay per `pipe_table` —
  intrinsics, the two layout passes, cell edits writing through
  `InlayOutcome::edit` in the same frame (docs/editor/table.md).
- **`markdown-fence-embed`** (himarkdown `FenceEmbedEnricher`,
  syntax, async): a fenced block whose info string carries a path
  after the language (`` ```rust src/main.rs ``) resolves against
  `input.base` and materializes an EDITING `Instead(FullLine)` editor
  over the target's REGISTERED document — `derive` awaits the content
  through `FetchDocumentEffect` on the caller; `install` (store in
  hand) registers-or-dedups by location and mounts the editor, so the
  embed IS the document a pane holds, not a copy. A `#L…` fragment
  mounts a line-window bounded by a fragment set on the TARGET
  document — plain shifting anchors, so the window follows the
  target's edits with no re-derivation. A path that resolves nowhere
  lands nothing — the fence stays the plain code block it already is.
- **`markdown-image`** (himarkdown `ImageEnricher`, syntax, async):
  `![alt](target)` shows its picture as an `Under` inlay below the
  line that names it; the source text is untouched. `derive` scans
  the `inline` nodes the changed ranges touch, asks the host for each
  target through `FetchResourceBytesEffect` (needs `input.base` for
  relative references), decodes and scales down, never up. Identity
  is the SPELLING: a run whose reference is unchanged carries the
  loaded view over from `previous` instead of fetching again, so
  typing beside an image costs nothing. Nothing marks a failure — an
  unloaded image is the markdown source it already is.
- **`mermaid`** (himermaid `MermaidEnricher`, syntax): renders the
  diagram on its own lane and lands one `Under` inlay per mermaid
  fence; only touched fences re-render (`changed` makes the
  incrementality structural). The language registration survives to
  route `.mmd` files — its parse value is a unit marker.
- **`ts-brace-match`** (hisitter `BraceMatchPass`, carets): pairs the
  delimiter TOKEN beside the caret with its balance-counted partner
  among the parent node's direct children (a `")"` inside a string
  never pairs) — two `StyleId::BraceMatch` styled ranges. Tree-driven,
  never a text scan; descends to the innermost nested syntax first,
  so a caret inside a rust fence asks the fence's tree.
- **`ts-occurrences`** (hisitter `OccurrencePass`, carets): collects
  identifier-kind LEAVES equal to the leaf under the caret, capped at
  512 — `StyleId::Occurrence` styled ranges.

There is NO link-detection and NO emphasis enricher: emphasis and
link styling come from the markdown parse itself.

## Deferred, deliberately

- Cross-enricher dependencies and any ordering between passes.
- Viewport-priority scheduling inside a pass (derive visible fences
  first) — the lane shape permits it; monster documents ask for it.
- External-generation interests (diagnostics, changeset ticks) — the
  `Interest` seam is where they plug in.
- Persistence of enrichment across sessions (a rendered diagram
  cache) — recompute is honest until it hurts.
- Remote `http(s)` image targets (the host has no client; a
  capability and a privacy decision, not a frontend one).
