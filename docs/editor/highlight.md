# Highlighting: the grammar side

The substrate — recursive markup, syntaxes, the one hierarchical parse
effect — is `docs/editor/markup.md`; this doc keeps what is specific to *coloring
code*.

## Grammars are `SyntaxLanguage`s

The editor itself carries no tree-sitter dependency: a parse crosses
its boundary as an opaque `SyntaxTree` value — cloneable, edit-stepped
through operations, able to name what changed since a previous parse,
nothing more. `frontend/hisitter` is where those values become real
trees: it owns the tree-sitter dependency and ships
`TreeSitterLanguage` — a grammar plus its `highlights.scm` query,
compiled at registration (a malformed query panics at startup, not
silently at use), implementing the two [`SyntaxLanguage`] jobs:

- **parse** — chunked (8 KiB pages pulled through a `TextView`'s leaf
  cache in `parse_with_options`) and incremental (the old edit-stepped
  tree is reused). Nothing materializes.
- **markup_for_changes** — the query cursor runs `set_byte_range` over
  each changed range only, reading node text through a rope-backed
  `TextProvider` (`RopePages` — the text is never materialized);
  unchanged tokens survive via the generic splice.

Grammar crates stay out of hisitter too: a language plugin
(`frontend/plugins/lang/hirust` and its ~50 siblings) brings its
`tree_sitter_*` dependency and registers a `TreeSitterLanguage`
(`hisitter::register_grammar!` — grammar, query, fold/outline kind
lists, ~30 lines); `frontend-host`'s `syntax_languages()` builds the
distribution's `SyntaxLanguages` registry, with `"markdown"` in the
same registry. A new language is one `register()` call. A language
need not be a grammar at all — himermaid's parse value is a unit
marker; its diagram is an enrichment pass's output
(docs/editor/editor-enrichment.md), not markup.

The distribution ships a plugin per popular language, each the hirust
shape: python, javascript (JSX included), typescript + tsx, go, java,
c, c++, c#, ruby, php, bash, lua, swift, kotlin
(`tree-sitter-kotlin-sg`), scala, haskell, elixir, ocaml, zig, sql
(`tree-sitter-sequel`), json, css, html, yaml, toml, cmake, d, dart,
elm, erlang, fortran, f#, gleam, glsl, graphql, groovy, hcl, julia,
make, nix, objective-c, odin, perl, powershell, r, solidity, xml.
Where a compatible crate ships no usable query, the plugin embeds its
own `highlights.scm` (d, graphql, groovy, hcl, julia, perl). Additive
queries concatenate their base grammar's first (typescript over
javascript, c++ and objective-c over c — later patterns win). The
registry is built ONCE per process (`OnceLock` — opening a document
consults it, and compiling ~50 queries per open would be waste).

The breadth rides a handful of hisitter generalizations, all
field-driven rather than per-language code:

- **fold interiors for indentation bodies** (`fold_interior`) — a
  body whose first child is no delimiter token (python's `block`,
  ruby's `body_statement`) folds from the end of the token before it
  (the `:`) through the body's end, so the header line joins the chip
  and a trailing `end` keeps its own line; delimited bodies keep the
  between-the-braces rule. Bodies resolve through the `body` field,
  else the last `*_body`-kind child (the fieldless kotlin grammar).
- **outline titles across conventions** (`title_node`) — `name`, else
  `declarator` (C's functions and typedefs — before `type`, which
  there is the RETURN type), else `type` (rust's impls), else the
  first identifier-kind child (kotlin again).
- **outline kinds gated on a body** (`with_outline_kinds_with_body`) —
  C's `struct_specifier` names definitions and mere references
  (`struct foo x;`) with one kind; only the bodied ones outline.
- **duplicate styles resolve by PATTERN INDEX**, not cursor order —
  javascript's query opens with an `(identifier) @variable` catch-all
  its later function patterns override, and the cursor yields matches
  in tree order, so "later in the query file wins" needs the index:
  an exact-range duplicate keeps the higher pattern index.
- **predicate-gated patterns we cannot evaluate never fire** —
  `#lua-match?` and kin arrive as GENERAL predicates (text predicates
  like `#eq?`/`#match?`/`#any-of?` are checked natively against the
  rope-backed provider); a pattern carrying any general predicate is
  skipped whole, because treating an unevaluable gate as passed
  re-captures wrongly (a shebang pattern's keyword swallowing every
  comment).

## Captures → styles → theme

Query capture names map by their first dotted segment onto the closed
`StyleId` enum (`frontend/editor/src/theme.rs`):
`keyword.control.import` → `StyleId::Keyword`; the code-facing slots
are `Keyword`, `String`, `Comment`, `Number`, `Type`, `Function`,
`Variable`, `Constant`, `Operator`, `Punctuation`, `Attribute`,
`Embedded`. Unmapped captures drop. (tree-sitter-rust names integer
literals `constant.*`.)

Styled spans land in the markup's STYLES lane as
`Decoration::Styled(StyleId)` — the bulk lane; structure (hidden
ranges, unhide, shaping decorations) rides the shape lane
(docs/editor/markup.md). Resolution is one step and app-side: the style ids
covering a range accumulate into a `BlockStyle`, and
`BlockStyle::resolved(theme)` merges the theme's per-slot
`TextAttributes` over the base — a fat bundle (bold / italic /
strikethrough / underline / color / background / font_families /
font_size / line_height / gutter / rule / block_gap / inset /
block_height / alignment / scroll-stripe color). Extend by FIELD, not
by `Decoration` variant: a new visual policy is a new
`TextAttributes` field every style may set, never a new decoration
kind.

## Honest limits

- Newly typed text is uncolored until its landing (one parse behind);
  surviving tokens shift correctly meanwhile.
- A block's markup caps at 50k tokens (`MAX_TOKENS_PER_BLOCK`),
  truncating from the end.
- Trees are retained per syntax for the incremental path. Syntaxes
  are reproducible by construction, so shedding trees for cold
  documents is always a safe trade if memory ever bites.
