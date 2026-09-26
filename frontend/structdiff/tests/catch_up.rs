// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! A STALE side tree (edit-adjusted, structurally behind — the shape
//! the normalize lane captures mid-typing) must ride the incremental
//! catch-up parse into an EXACT structural operation, never a cold
//! parse and never a misalignment. The engine's own debug exactness
//! guard (`structural()`'s apply check) is armed in these builds, so a
//! stale tree leaking into alignment would panic here.

use std::sync::Arc;

use editor::diff::{DiffPolicy, DiffSyntax, DiffTree};
use text::Text;

fn rust_languages() -> Arc<editor::SyntaxLanguages> {
    let mut languages = editor::SyntaxLanguages::new();
    hirust::register(&mut languages);
    Arc::new(languages)
}

#[test]
fn a_stale_target_tree_catches_up_to_an_exact_operation() {
    let languages = rust_languages();
    let rust = languages.ensure("rust").expect("rust grammar");

    let base_src = "fn alpha() -> u32 {\n    1 + 2\n}\n\nfn beta() -> u32 {\n    3 + 4\n}\n";
    let target_src = "fn alpha() -> u32 {\n    1 + 2 + 10\n}\n\nfn beta() -> u32 {\n    3 + 4\n}\n";
    let base = Text::from_string_exact(base_src.to_owned());
    let target = Text::from_string_exact(target_src.to_owned());

    let base_tree = rust
        .parse(&base, 0..base.view().byte_count() as u32, None)
        .expect("base parse");

    // The stale target tree: parsed for the BASE content, then
    // edit-adjusted for the change — exactly the document's tree
    // between a keystroke and its reparse landing.
    let mut stale = rust
        .parse(&base, 0..base.view().byte_count() as u32, None)
        .expect("pre-edit parse");
    let edits = myersdiff::diff(&base, &target);
    stale.edit(&edits, &mut target.view(), 0);

    let policy = structdiff::Structural::new(languages.clone());
    let syntax = DiffSyntax {
        language: "rust",
        base: Some(DiffTree {
            tree: base_tree.as_ref(),
            fresh: true,
        }),
        target: Some(DiffTree {
            tree: stale.as_ref(),
            fresh: false,
        }),
    };
    let operation = policy.diff(&base, &target, Some(&syntax));
    assert_eq!(
        structdiff::apply(&operation, base_src).as_deref(),
        Some(target_src),
        "the caught-up structural operation is exact"
    );

    // The mirrored shape: a stale BASE (the baseline moved under a
    // reload) catches up the same way.
    let syntax = DiffSyntax {
        language: "rust",
        base: Some(DiffTree {
            tree: stale.as_ref(),
            fresh: false,
        }),
        target: Some(DiffTree {
            tree: base_tree.as_ref(),
            fresh: false,
        }),
    };
    let operation = policy.diff(&target, &base, Some(&syntax));
    assert_eq!(
        structdiff::apply(&operation, target_src).as_deref(),
        Some(base_src),
        "a stale base catches up too"
    );
}
