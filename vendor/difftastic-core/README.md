# difftastic-core (vendored)

The language-agnostic structural-diff engine of
[difftastic](https://github.com/Wilfred/difftastic), vendored from
upstream tag **0.71.0**, commit
`b7d119e90ac9f972f03f69508da765dacd302c0c`. MIT licensed — see
`LICENSE` (copied verbatim from upstream). Consumed by
`frontend/structdiff`; see docs/structural-diff.md for the design.

Contains only the diff core: the `Syntax` tree representation
(`src/parse/syntax.rs`), the diff algorithm (`src/diff/*`: unchanged
pre-pass, Dijkstra shortest path, sliders), language detection tables
(`src/parse/guess_language.rs`, used only for slider preferences
here), and small support files (`hash.rs`, `lines.rs`, `words.rs`).
None of difftastic's parsers, vendored grammars, display code or CLI.

## Local modifications

All mechanical; everything else is verbatim upstream.

- `src/lib.rs`, `src/parse/mod.rs`, `src/diff/mod.rs`: new (upstream
  is a binary crate; `lib.rs` also carries `DEFAULT_GRAPH_LIMIT`,
  copied from upstream `src/options.rs`).
- `pub(crate)` widened to `pub` throughout (upstream exposes no API).
- `src/diff/sliders.rs`, `src/diff/unchanged.rs`: trailing
  `#[cfg(test)]` modules removed — they imported upstream's
  tree-sitter parsers, which are not vendored.
- `src/diff/shortest_path.rs`: added `use log::{debug, info};`
  (upstream imports the macros crate-globally).
- `src/diff/unchanged.rs`, `src/diff/sliders.rs`: one doc-comment
  code fence each marked `text` (they are Lisp examples, not Rust).

## Upgrading

Re-copy the files above from the new upstream tag, re-apply the
modifications (each is listed so this stays a diff-and-redo job),
update the tag + commit here and in `Cargo.toml`/`src/lib.rs`, and run
`cargo test -p difftastic-core -p structdiff`.
