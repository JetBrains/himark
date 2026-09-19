# Structural diff (difftastic)

Syntax-aware diff for files we already parse. Before this work,
every diff in himark was `editor::diff::diff(left, right)` — a
line-level Myers pass (`similar`) refined per-`Replace` block by a
char-level pass
(frontend/editor/src/diff.rs:75, `refine` at diff.rs:89). That is
exact and fast, but blind to structure: reformatting, re-wrapping and
re-indenting produce hunk noise, and a moved-in paragraph misaligns
everything after it. Difftastic's engine — Dijkstra over a graph of
(lhs position, rhs position) vertices — aligns *syntax nodes* instead
of lines, and we already have syntax trees for markdown and ~60
fenced languages (docs/editor/highlight.md, frontend/editor/src/reparse.rs).

Difftastic upstream is a CLI, not a library (everything `pub(crate)`,
changelog: "No APIs are considered stable for external usage"), and
its output is a per-node change classification, not a patch. Both
problems are solvable and this doc is the exact plan: vendor the
engine, feed it our trees, and *derive an `operation::Operation` from
its output* — so the rest of the diff machinery (docs/editor/diff.md) does
not change at all.

The load-bearing decisions, up front:

1. **An `Operation` stays THE diff model.** Difftastic is an
   alternative *producer* of the same `Operation`; hunk markup,
   fragments, split alignment, stripes, merge — everything downstream
   of `diff()` is untouched. The structural path must produce an
   operation that is exact by construction (apply it to left, get
   right), same contract as the Myers path.
2. **We vendor the engine and build its trees ourselves.**
   `vendor/difftastic-core` (MIT, upstream tag 0.71.0, commit
   b7d119e90ac9f972f03f69508da765dacd302c0c) contains only the
   language-agnostic core: `parse/syntax.rs` (the `Syntax` tree +
   `MatchedPos`), `diff/*` (unchanged-region pre-pass, Dijkstra,
   sliders), and small support files. None of difftastic's parsers,
   display code or CLI. Trees come from *our* tree-sitter trees via a
   new `frontend/structdiff` crate.
3. **The engine is a policy, injected at the edge; core depends on
   neither `similar` nor difftastic.** `editor::diff::DiffPolicy`
   (with `DiffSyntax`: language + optional `dyn SyntaxTree` sides) is
   the only diff surface core crates see. The `similar` pass moved
   verbatim into `frontend/myersdiff` (`Myers` policy + the shared
   `refine`); `structdiff::Structural` layers difftastic on top and
   falls back to Myers. The edge decides: `himark::Application`
   installs `Myers` as the baseline at construction
   (`editor::env::Differ` store slot), `frontend-host` overrides with
   `Structural` via `register_diff_policy`, the web edge stays on
   Myers. Handlers stay unit structs — effects capture the policy
   `Arc` from the store at launch. Call sites that use a diff as a
   *patch* — agent cell rewrite (higent/cell.rs), script writes
   (hiscript), snapshot adoption (hiahp docsync) — call
   `myersdiff::diff` directly (they are edge crates and want the
   minimal exact edit); the three-way watch merge, being core, goes
   through the policy but passes no syntax, which degrades any policy
   to its text pass.
4. **Every structural failure degrades to `similar`, silently.** No
   tree, unparseable baseline, error-heavy tree, graph-limit
   exceeded, or a failed exactness check — each falls back to the
   current pipeline. The structural path is an upgrade, never a
   requirement.

## The vendored crate

`vendor/difftastic-core` — wired like the other vendor crates
(workspace member; `vendor/tree-sitter-0.26.8` precedent). Local
modifications, recorded in its README: `pub(crate)` widened to `pub`,
a new `lib.rs`, two `#[cfg(test)]` modules removed (they imported
difftastic's tree-sitter parsers, which we don't vendor), one `use
log::{debug, info}` import. Everything else is verbatim upstream.
Deps are small pure-Rust crates (typed-arena, radix-heap, bumpalo,
hashbrown, imara-diff, line-numbers, strsim, …).

The pipeline it exposes, in upstream's own call order (difftastic
src/main.rs:697–735):

```rust
init_all_info(&lhs_roots, &rhs_roots);            // ids, parents, content hashes
let regions = mark_unchanged(&lhs_roots, &rhs_roots, &mut change_map);
for (lhs_region, rhs_region) in regions {         // cheap pre-pass peeled off
    mark_syntax(lhs, rhs, &mut change_map, graph_limit)?;  // Dijkstra
}
fix_all_sliders(language, &lhs_roots, &mut change_map);    // readability fixups
```

`mark_syntax` returns `Err(ExceededGraphLimit)` past `graph_limit`
(upstream default 3,000,000 vertices) — that is fallback trigger, not
error. `fix_all_sliders` takes a `guess_language::Language` but only
branches on a prefer-outer-delimiter predicate (Lisp family, JSON,
TOML, HCL, SQL → outer); we map our language name to it where it
matters and default to inner otherwise.

The output is a `ChangeMap`: `SyntaxId → ChangeKind`, where

```rust
enum ChangeKind<'a> {
    Unchanged(&'a Syntax<'a>),   // carries the OPPOSITE-side node
    IgnoredPunctuation,
    ReplacedComment(&'a Syntax<'a>, &'a Syntax<'a>),
    ReplacedString(&'a Syntax<'a>, &'a Syntax<'a>),
    Novel,
}
```

`Unchanged` carrying the counterpart node is what makes the
`Operation` derivation possible.

## Feeding it our trees: `frontend/structdiff`

New crate, deps: `difftastic-core`, `tree-sitter`, `operation`,
`text`, `hisitter` (the `TsTree` downcast), `myersdiff` (the fallback
+ the shared `refine`), and `editor` (the `DiffPolicy` trait +
`SyntaxLanguages`). Only `frontend-host` depends on it — the edge
that installs `Structural`; `editor` stays parser-agnostic (it only
knows `dyn SyntaxTree`, frontend/editor/src/reparse.rs:21).

**Conversion** `tree_sitter::Node → difftastic Syntax`, one arena per
diff:

- A leaf node (`child_count() == 0`) becomes
  `Syntax::new_atom(arena, positions, content, kind)`. `kind` is
  mapped from the node kind name: contains `comment` →
  `AtomKind::Comment` (unlocks difftastic's fuzzy comment matching →
  `ReplacedComment`), `string`/`code_fence_content` →
  `AtomKind::String`, else `Normal`. Content is read from the rope
  by byte range (the same paging the highlighter uses,
  frontend/hisitter/src/lib.rs:413 `RopePages`).
- An interior node becomes `Syntax::new_list(...)`. v1 uses **empty
  open/close delimiter content**: language-agnostic, no per-language
  delimiter tables (upstream needs them because it renders; we only
  need alignment). Cost: the delimiter-based slider fixups get less
  to work with. v2 can populate delimiters generically — first/last
  *anonymous* leaf child, which in tree-sitter grammars is exactly
  the `(`/`)`-class token.
- **Positions.** `Syntax` stores `Vec<SingleLineSpan>` (line +
  byte columns), and a multi-line atom must be split into one span
  per line — an upstream invariant; `LinePositions::from_region`
  does the split. The derivation recovers each atom's byte range
  from its FIRST stored span plus a per-side line-starts table
  (`derive.rs byte_range`), with the end taken from the content
  LENGTH — not the last span — because `new_atom` trims a trailing
  newline/CR from the content without adjusting the spans.
- **Text not covered by any child becomes a synthetic atom.**
  tree-sitter children don't necessarily tile their parent's text —
  mdparser's `inline` nodes leave paragraph prose outside any child
  (only tokens like trailing punctuation are children), so without
  this every paragraph would be invisible to the engine and diff as
  novel. Each uncovered run inside an interior node is emitted as one
  whitespace-trimmed `Normal` atom at its real byte position.
- **Whitespace between tokens is not represented** — pure-whitespace
  gaps get no atom (matching whitespace everywhere would produce odd
  alignments), and difftastic doesn't model it either. It is
  recovered as retains by the gap-coalescing pass below.

**Where the two trees come from.** The right side is the document's
live tree (`markup.rs:275 Syntax.tree`, downcast via `TsTree::of`,
frontend/hisitter/src/lib.rs:22). The left side (baseline: VCS
version, agent "before", file-on-disk) has no document and no tree —
the structural path **parses it itself** with the same
`SyntaxLanguage` (reparse.rs:58), inside the already-off-thread
normalize worker — unless the base IS a registered document with a
fresh tree, in which case the lane snapshots it like the target's
(`syntax_snapshot`, frontend/documents/src/diffs.rs). Parsed
baselines are not yet cached across normalize runs — a measured
optimization, not a correctness need. Both trees are snapshots taken
only when `edited_since_parse()` is empty (a stale tree never meets
a newer text); the worker's landing already rebases over concurrent
edits (docs/editor/diff.md decision 3) — same contract as the Myers worker.

**Markdown.** mdparser is a block-level grammar; a paragraph's prose
is one synthetic atom. v1 therefore aligns markdown at *block*
granularity (heading / paragraph / list item / fence), which is
exactly the moved-block win,
and leaves intra-paragraph changes to the char refinement pass —
same quality as today inside a paragraph, better across paragraphs.
v2 options, in order of value: split paragraph content into word
atoms in the converter (word-level structural matching for prose);
splice the injected fence trees (`SyntaxSite`, reparse.rs:15) in
place of the fence-content atom, so code fences diff with their real
grammar.

## Deriving the `Operation`

This is the crux: difftastic classifies nodes, himark needs the edit
that turns left into right. The bridge is that difftastic's matching
is **monotonic** — the Dijkstra vertex is a pair of positions that
only ever advances on both sides; there is no move detection — so
matched nodes appear in the same order on both sides, and an edit
script exists.

Given the `ChangeMap` and a line-starts table per side:

1. **Collect matched pairs.** Walk the lhs tree in document order.
   For each node with `ChangeKind::Unchanged(rhs_node)`:
   - an atom contributes the pair
     `(lhs_bytes(node), rhs_bytes(rhs_node))` — atom matches are
     content-equal by the engine's construction;
   - a list contributes its open-delimiter pair and close-delimiter
     pair (nonempty delimiters only; with v1's empty delimiters,
     lists contribute nothing and all matching comes from atoms —
     `Unchanged` on a list is "shallow", children carry their own
     kinds).
   `ReplacedComment`/`ReplacedString` pairs are *not* matches (their
   content differs) — they stay in the gap, but we remember them as
   refinement anchors. `Novel` and `IgnoredPunctuation` stay in the
   gap.
2. **Monotonicity guard.** Assert rhs ranges are strictly
   increasing. This holds by construction; the guard is a
   `debug_assert!` plus a release-mode check that triggers fallback
   rather than producing a wrong operation.
3. **Emit.** Two byte cursors. For each pair `(l, r)`:
   `delete(left[cursor_l..l.start])`, `insert(right[cursor_r..r.start])`,
   `retain(l.len())` (with `l.len() == r.len()`, contents equal).
   Tail gap after the last pair, then `Operation::from_ops` — the
   same assembly `diff()` uses today (diff.rs:75–86).
4. **Gap coalescing.** Matched atoms exclude interstitial
   whitespace, so after step 3 the newline between two matched
   tokens is a delete+insert of identical bytes — noise, and worse,
   it starves `hunk_markup` and the fragment safepoints of
   newline-carrying retains (fragments_at seeks a retain containing
   `\n`, diff.rs:225). The pass: for every delete+insert gap, grow
   the neighbouring retains over the gap's common prefix and common
   suffix (plain byte comparison), then merge adjacent retains.
   After this, everything byte-identical between two matched tokens
   is retained — restoring the "identical text is retained" property
   the Myers path has.
5. **Refine.** Each remaining gap where both sides are nonempty goes
   through the existing char-level `refine` (diff.rs:89):
   `TextDiff::from_chars` under `REFINE_BUDGET` (4 KiB), then the
   `RETAIN_NOISE` word-run suppression (diff.rs:112). Reused, not
   reimplemented — this keeps word tints
   (`fragment.words` → `DiffAddedWord`/`DiffDeletedWord`,
   frontend/editor/src/split_diff.rs:727) working identically.
6. **Exactness check.** By construction the operation is exact; in
   debug builds we additionally `debug_assert_eq!(apply(op, left),
   right)`. In release, step 3 already compares each retained span
   (`left[l] == right[r]`, cheap, it's the price of one memcmp per
   match); any mismatch → fallback to Myers. A structural diff can
   be *non-minimal* or *ugly*; it must never be *wrong*.

The result flows into `Operation::from_ops` and from there the
normalize landing (`DiffNormalizeHandler`, diffs.rs:712) — rebase
over composed edits, `set_diff` damage, `hunk_markup` — completely
unchanged.

## Fallback to `similar`

One entry point, in `structdiff`:

```rust
pub fn diff(left: &Text, right: &Text, syntax: Option<SyntaxInput>) -> Operation
```

where `SyntaxInput` names the language and hands over the right-side
tree (if the caller has one) and the cached baseline tree (if any).
`structdiff::Structural` wears the same logic as the installed
`DiffPolicy`; both fall back to `myersdiff::diff` whenever any of the
following holds:

- `syntax` is `None` — unparsed file, no registered grammar, lazy
  grammar not yet loaded (docs/editor/lazy-languages.md);
- either input exceeds a size cap (initially 1 MiB — Dijkstra cost
  grows much faster than Myers; cap tuned by measurement);
- either tree's `ERROR`/`MISSING` node ratio exceeds a threshold
  (initially 5% of nodes) — half-parsed trees align garbage;
- baseline parse fails or returns `None`;
- `mark_syntax` returns `ExceededGraphLimit`;
- the monotonicity guard or a retained-span comparison fails
  (step 2/6 above — defensive, should never fire).

Fallback is invisible to callers: same `Operation` contract.

Wiring, as built: `structdiff::Structural` implements the policy
trait — it resolves `DiffSyntax` trees by downcast
(`hisitter::TsTree::of`) and parses a missing side itself through the
`SyntaxLanguages` it owns (`SyntaxLanguage::parse`, no fonts/theme
needed). Syntax reaches it from two producers: the normalize lane
(diffs.rs `sync_diff_lanes` snapshots language + `clone_tree()` per
side at capture — only when `edited_since_parse()` is empty, so a
stale tree never meets a newer text) and hiahp's off-thread file-diff
builders (both documents freshly parsed, trees handed over directly).
Every synchronous UI-thread site — diff birth in `track_diff`, the
`DiffState::attach`/`settle` repair fallbacks, panel seeds — calls
the policy with `None` syntax: exact, cheap, and the normalize
landing upgrades the alignment moments later. If no policy was
installed (bare-store unit tests), `editor::diff::ReplaceAll` stands
in: still exact, deliberately crude.

## Phases

- **0 — vendored crate compiles** (done): crate builds standalone,
  upstream unit tests that survived the pruning pass.
- **1 — converter + derivation, tests-first** (done —
  frontend/structdiff, tests/structural.rs): `Node → Syntax`
  conversion with synthetic gap atoms, derivation steps 1–6 with
  byte ranges recovered from the stored line spans. Tests: operation
  shape over mdparser trees (identical → one retain; insertion →
  zero deleted bytes; deletion → deleted bytes account exactly for
  the length difference), apply-exactness over a deterministic
  battery of block-level edit combinations, unicode content, empty
  sides, and the no-tree fallback.
- **2 — policy injection** (done): the `DiffPolicy` trait +
  `env::Differ` slot, `similar` extracted to `frontend/myersdiff`
  (editor no longer depends on it), `Structural` installed by
  frontend-host, Myers baseline installed by `himark::Application`
  and the web edge. Still open from this phase: perf comparison in
  the perf harness (structural vs Myers wall time and fallback rate)
  before calling structural the durable default.
- **3 — quality**: markdown word atomization, fence tree splicing at
  `SyntaxSite`s, generic list delimiters from anonymous leaf tokens,
  language-name → `guess_language::Language` map for slider
  preference.

## Risks

- **Cost.** Dijkstra is not Myers; the graph limit and size cap
  bound the worst case, and the normalize lane is already
  off-thread and rebase-tolerant, so a slow structural diff delays
  *minimality*, never correctness. Measured before default-on.
- **Alignment quality with empty delimiters.** The nested-slider
  fixups have less signal in v1; if visible, phase 3's generic
  delimiters address it.
- **Fork maintenance.** The vendor README pins tag + commit and
  lists every local edit; upgrading is re-copy + re-apply (all edits
  are mechanical). Upstream moves slowly in the core files.
