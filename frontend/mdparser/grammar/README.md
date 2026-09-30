The markdown BLOCK grammar, vendored from the `tree-sitter-md` crate,
version 0.5.3 (MIT licensed, per that crate's manifest —
https://github.com/tree-sitter-grammars/tree-sitter-markdown).

- `grammar.json` — upstream, PATCHED: `_atx_heading1..6` and the
  fenced-code-block CLOSING sequence accept `$._eof` beside
  `$._newline`, the same alternative upstream already gives
  paragraphs, setext headings, thematic breaks and tables. Without it
  a file lacking a trailing newline parsed its last heading as ERROR
  and left its last fence unclosed — which forced consumers to parse
  over a virtual trailing newline, producing trees one byte longer
  than their source (the 2026-09 structdiff out-of-bounds abort).
  `build.rs` generates `parser.c` from it at build time
  (`tree-sitter-generate`); the generated file is never committed.
- `scanner.c` — upstream's hand-written external scanner, PATCHED
  (sites marked `PATCHED (himark)`):
  - `parse_pipe_table` accepted delimiter rows with EMPTY cells, so a
    table body row of empty cells (exactly what inserting a fresh row
    produces) followed by more content re-registered as a new table's
    header+delimiter and derailed the block parse of everything
    below; GFM requires at least one dash per delimiter cell.
  - `parse_fenced_code_block` only recognized a closing delimiter
    followed by a newline; EOF now counts as the line ending, pairing
    with the grammar.json `_eof` patch above.
- `tree_sitter/*.h` — upstream support headers, untouched.
