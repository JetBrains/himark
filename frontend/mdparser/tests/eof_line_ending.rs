// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! EOF is a line ending. Upstream tree-sitter-md accepts `$._eof` in
//! place of a newline for paragraphs, setext headings, thematic
//! breaks and tables — but forgot ATX headings and the closing fence
//! delimiter, so a file without a trailing newline parsed its last
//! heading as ERROR and left its last fence unclosed. Consumers used
//! to paper over this by parsing over a virtual trailing newline,
//! which produced trees one byte LONGER than their source — the
//! 2026-09 structdiff out-of-bounds abort. The grammar is patched
//! instead (grammar.json `_atx_heading*`/`fenced_code_block`,
//! scanner.c `parse_fenced_code_block`); these tests pin it.

/// Every construct parses the same with and without the trailing
/// newline, and no node ever extends past the source.
#[test]
fn a_missing_trailing_newline_changes_nothing() {
    let cases = [
        "hello",
        "# Title",
        "## deeper\n\nbody",
        "###### six",
        "- [ ] one\n- [x] two",
        "- one\n- ", // the Enter-continuation flow's intermediate state
        "1. a\n2. ",
        "```rust\nfn x() {}\n```",
        "~~~\ntext\n~~~",
        "```rust\nfn x() {}", // unclosed fence stays unclosed
        "> quote",
        "> # quoted heading",
        "- item\n  ```\n  fenced in a list\n  ```",
        "| a |\n|---|\n| 1 |",
        "para one\n\npara two",
        "setext\n===",
        "---",
    ];
    for src in cases {
        let bare = mdparser::block_tree(src);
        let with_newline = mdparser::block_sexp(&format!("{src}\n"));
        assert_eq!(
            bare.root_node().to_sexp(),
            with_newline,
            "EOF must read as a line ending for {src:?}",
        );
        assert!(
            bare.root_node().end_byte() <= src.len(),
            "no node may extend past the source for {src:?}",
        );
    }
}

/// The closing delimiter on the last line is a real close: the block
/// ends, and content after a subsequent edit lands OUTSIDE it.
#[test]
fn a_fence_closed_at_eof_is_closed() {
    let parsed = mdparser::block_sexp("```rust\nfn x() {}\n```");
    assert_eq!(
        parsed.matches("fenced_code_block_delimiter").count(),
        2,
        "{parsed}"
    );
    assert!(!parsed.contains("ERROR"), "{parsed}");
}
