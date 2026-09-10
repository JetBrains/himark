The markdown BLOCK grammar, vendored from the `tree-sitter-md` crate,
version 0.5.3 (MIT licensed, per that crate's manifest —
https://github.com/tree-sitter-grammars/tree-sitter-markdown).

- `grammar.json` — upstream, unmodified. `build.rs` generates `parser.c`
  from it at build time (`tree-sitter-generate`); the generated file is
  never committed.
- `scanner.c` — upstream's hand-written external scanner, PATCHED: its
  `parse_pipe_table` accepted delimiter rows with EMPTY cells, so a table
  body row of empty cells (exactly what inserting a fresh row produces)
  followed by more content re-registered as a new table's
  header+delimiter and derailed the block parse of everything below. The
  patched sites are marked `PATCHED (himark)`; GFM requires at least one
  dash per delimiter cell.
- `tree_sitter/*.h` — upstream support headers, untouched.
