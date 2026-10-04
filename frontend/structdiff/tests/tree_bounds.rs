// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! A tree whose ranges extend past the source is REFUSED — the
//! `structural()` bounds gate degrades to Myers, like error-heavy
//! trees do — and must never panic: the normalize lane sits behind a
//! nounwind boundary, where a panic aborts the whole app (the
//! 2026-09 `docs/issues.md` crash: "end byte index 403 is out of
//! bounds" — markdown was then parsed over a virtual trailing
//! newline, so every fresh tree ended at len+1). The grammar now
//! treats EOF as a line ending (mdparser tests/eof_line_ending.rs
//! pins the producer); these tests keep the GATE honest against a
//! hostile tree built the old way — the answer stays exact, the
//! engine choice is unobservable by contract.

use text::Text;

/// A tree one byte longer than the text it will be converted against
/// — the old virtual-trailing-newline shape.
fn md_tree_past_the_source(src: &str) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&mdparser::markdown_language())
        .expect("the markdown grammar must load");
    parser
        .parse(format!("{src}\n").as_bytes(), None)
        .expect("parse")
}

#[test]
fn a_tree_past_the_source_end_never_panics() {
    // No trailing newline on either side — both trees end at len+1.
    let base_src = "- [ ] adding a second folder\n- Chat\n  - [ ] navigate from edit\n- [ ] panic";
    let target_src =
        "- [x] adding a second folder\n- Chat\n  - [ ] navigate from edit\n- [ ] panic";
    let base = Text::from_string_exact(base_src);
    let target = Text::from_string_exact(target_src);
    let base_tree = md_tree_past_the_source(base_src);
    let target_tree = md_tree_past_the_source(target_src);

    let operation = structdiff::diff(
        &base,
        &target,
        Some(&structdiff::SyntaxInput {
            left_tree: &base_tree,
            right_tree: &target_tree,
            language: Some("markdown"),
        }),
    );
    assert_eq!(
        structdiff::apply(&operation, base_src).as_deref(),
        Some(target_src),
        "the operation is exact whichever engine served it",
    );
}

#[test]
fn mixed_trailing_newlines_stay_exact() {
    // One side ends with a real newline, the other rides the virtual
    // one — the diff must express the newline difference exactly.
    let base_src = "plain paragraph\n\nlast line\n";
    let target_src = "plain paragraph\n\nlast line changed";
    let base = Text::from_string_exact(base_src);
    let target = Text::from_string_exact(target_src);
    let base_tree = md_tree_past_the_source(base_src);
    let target_tree = md_tree_past_the_source(target_src);

    let operation = structdiff::diff(
        &base,
        &target,
        Some(&structdiff::SyntaxInput {
            left_tree: &base_tree,
            right_tree: &target_tree,
            language: Some("markdown"),
        }),
    );
    assert_eq!(
        structdiff::apply(&operation, base_src).as_deref(),
        Some(target_src),
    );
}
