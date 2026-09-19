// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Exactness and shape of the structural diff over real markdown trees
//! (docs/structural-diff.md, phase 1).

use operation::{Op, Operation};
use structdiff::{apply, diff, SyntaxInput};
use text::Text;

fn structural(left: &str, right: &str) -> Operation {
    let left_tree = mdparser::block_tree(left);
    let right_tree = mdparser::block_tree(right);
    diff(
        &Text::from_string(left),
        &Text::from_string(right),
        Some(&SyntaxInput {
            left_tree: &left_tree,
            right_tree: &right_tree,
            language: Some("markdown"),
        }),
    )
}

#[track_caller]
fn check(left: &str, right: &str) -> Operation {
    let operation = structural(left, right);
    assert_eq!(
        apply(&operation, left).as_deref(),
        Some(right),
        "operation must turn left into right\nleft: {left:?}\nright: {right:?}",
    );
    operation
}

fn total_deleted(operation: &Operation) -> usize {
    operation
        .iter()
        .map(|op| match op {
            Op::Delete(text) => text.len(),
            _ => 0,
        })
        .sum()
}

#[test]
fn identical_is_one_retain() {
    let src = "# Title\n\nA paragraph here.\n\n- one\n- two\n";
    let operation = check(src, src);
    let ops: Vec<Op> = operation.iter().collect();
    assert_eq!(ops, vec![Op::Retain(src.len() as u32)]);
}

#[test]
fn inserted_paragraph_deletes_nothing() {
    let left = "# Title\n\nFirst paragraph.\n\nLast paragraph.\n";
    let right = "# Title\n\nFirst paragraph.\n\nBrand new middle.\n\nLast paragraph.\n";
    let operation = check(left, right);
    assert_eq!(total_deleted(&operation), 0, "{operation:?}");
}

#[test]
fn deleted_paragraph_keeps_the_rest_retained() {
    let left = "# Title\n\nFirst paragraph.\n\nDoomed middle.\n\nLast paragraph.\n";
    let right = "# Title\n\nFirst paragraph.\n\nLast paragraph.\n";
    let operation = check(left, right);
    // Everything surviving is retained: deleted bytes account exactly
    // for the length difference.
    assert_eq!(total_deleted(&operation), left.len() - right.len());
}

#[test]
fn edited_word_inside_paragraph() {
    check(
        "Alpha beta gamma delta.\n\nSecond paragraph stays.\n",
        "Alpha BETA gamma delta.\n\nSecond paragraph stays.\n",
    );
}

#[test]
fn moved_paragraph_is_exact() {
    check(
        "# Doc\n\nMover paragraph.\n\nAnchor one.\n\nAnchor two.\n",
        "# Doc\n\nAnchor one.\n\nAnchor two.\n\nMover paragraph.\n",
    );
}

#[test]
fn heading_and_list_edits() {
    check(
        "# Old title\n\n- apple\n- banana\n- cherry\n\n```rust\nfn main() {}\n```\n",
        "# New title\n\n- apple\n- BANANA!\n- cherry\n- date\n\n```rust\nfn main() { println!(); }\n```\n",
    );
}

#[test]
fn empty_sides() {
    check("", "# Fresh\n\nContent.\n");
    check("# Stale\n\nContent.\n", "");
    check("", "");
}

#[test]
fn multibyte_content() {
    check(
        "# Название\n\nПервый абзац с юникодом → ещё.\n",
        "# Название!\n\nПервый абзац с юникодом → ещё. Плюс хвост.\n",
    );
}

#[test]
fn whitespace_between_blocks_is_retained() {
    // The gap between matched blocks (blank lines) is not covered by
    // tree nodes; coalescing must recover it as retains, so an
    // unchanged tail after an edit produces no delete/insert there.
    let left = "First paragraph.\n\n\nSpaced out.\n";
    let right = "First paragraph EDITED.\n\n\nSpaced out.\n";
    let operation = check(left, right);
    let retained: u32 = operation
        .iter()
        .map(|op| match op {
            Op::Retain(len) => len,
            _ => 0,
        })
        .sum();
    // "First paragraph" prefix + ".\n\n\nSpaced out.\n" suffix survive.
    assert!(
        retained >= "First paragraph".len() as u32 + ".\n\n\nSpaced out.\n".len() as u32,
        "expected the unchanged prefix/suffix retained, got {operation:?}",
    );
}

#[test]
fn fallback_without_trees_is_exact() {
    let left = "no trees here\nline two\n";
    let right = "no trees HERE\nline two\nline three\n";
    let operation = diff(&Text::from_string(left), &Text::from_string(right), None);
    assert_eq!(apply(&operation, left).as_deref(), Some(right));
}

#[test]
fn many_random_edits_are_exact() {
    // A deterministic battery of block-level edit combinations.
    let blocks = [
        "# Heading\n",
        "Plain paragraph with some words.\n",
        "- item one\n- item two\n",
        "```js\nconsole.log(1);\n```\n",
        "> a quote block\n",
        "Final thoughts.\n",
    ];
    let mut seed = 0x9e3779b9u32;
    let mut rand = move || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        seed
    };
    for _ in 0..50 {
        let pick = |bits: u32| (bits as usize) % blocks.len();
        let mut left = String::new();
        let mut right = String::new();
        for _ in 0..(rand() % 5 + 1) {
            left.push_str(blocks[pick(rand())]);
            left.push('\n');
        }
        for _ in 0..(rand() % 5 + 1) {
            right.push_str(blocks[pick(rand())]);
            right.push('\n');
        }
        check(&left, &right);
    }
}
