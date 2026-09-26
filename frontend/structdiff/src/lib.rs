// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Structural diff over the vendored difftastic engine
//! (docs/editor/structural-diff.md). `diff` upgrades to syntax-node alignment
//! when the caller supplies trees, and degrades to the line/char Myers
//! pass (`myersdiff::diff`) on any structural failure: no trees,
//! size cap, error-heavy trees, graph-limit blowup, or a failed
//! exactness guard. Same `Operation` contract either way — callers
//! cannot observe which path ran, except through alignment quality.
//!
//! Runs off-thread (the normalize lane); nothing here touches shared
//! state — both texts and both trees are snapshots.

mod convert;
mod derive;

use difftastic_core::diff::changes::ChangeMap;
use difftastic_core::diff::shortest_path::mark_syntax;
use difftastic_core::diff::sliders::fix_all_sliders;
use difftastic_core::diff::unchanged::mark_unchanged;
use difftastic_core::options::DEFAULT_GRAPH_LIMIT;
use difftastic_core::parse::guess_language::Language;
use difftastic_core::parse::syntax::{init_all_info, init_next_prev};
use operation::Operation;
use text::Text;
use typed_arena::Arena;

/// Trees for the structural path. Both are snapshots parsed from
/// exactly the byte content of the corresponding `Text`.
pub struct SyntaxInput<'t> {
    pub left_tree: &'t tree_sitter::Tree,
    pub right_tree: &'t tree_sitter::Tree,
    /// himark language name; only used for difftastic's slider
    /// preference (Lisp/JSON-family languages prefer outer delimiters).
    pub language: Option<&'t str>,
}

/// Dijkstra cost grows much faster than Myers; past this size the
/// structural path is not worth attempting (docs/editor/structural-diff.md,
/// "Fallback to similar").
const MAX_STRUCTURAL_BYTES: usize = 1024 * 1024;

/// Half-parsed trees align garbage; above this share of
/// `ERROR`/`MISSING` nodes the tree is not trusted.
const MAX_ERROR_RATIO: f64 = 0.05;

/// Structural alignment buys READABILITY on a modest change — a
/// re-indent, a moved block, a rewrite-in-place. Past this change
/// mass the diff renders as "everything changed" under either engine,
/// while difftastic's cost explodes (a Dijkstra per changed region:
/// measured 15.7s on a 461KB global rename whose answer matched Myers
/// piece for piece). Myers runs FIRST — it is the fallback anyway and
/// 100-1000x cheaper — and its result is the gate.
const MAX_STRUCTURAL_PIECES: usize = 256;
const MAX_STRUCTURAL_CHANGED_BYTES: usize = 64 * 1024;

fn worth_structural(myers: &Operation) -> bool {
    let mut pieces = 0usize;
    let mut changed = 0usize;
    for op in myers.iter() {
        match op {
            operation::Op::Retain(_) => {}
            operation::Op::Delete(text) | operation::Op::Insert(text) => {
                pieces += 1;
                changed += text.len();
            }
        }
        if pieces > MAX_STRUCTURAL_PIECES || changed > MAX_STRUCTURAL_CHANGED_BYTES {
            return false;
        }
    }
    true
}

pub fn diff(left: &Text, right: &Text, syntax: Option<&SyntaxInput>) -> Operation {
    let myers = myersdiff::diff(left, right);
    if let Some(input) = syntax {
        if worth_structural(&myers) {
            if let Some(operation) = structural(left, right, input) {
                return operation;
            }
        }
    }
    myers
}

/// The edge-installable `editor::diff::DiffPolicy`: structural where
/// trees exist (provided, or parsed here from the language name),
/// Myers everywhere else. Owns the language registry so it can parse
/// a side the caller has no tree for (typically the baseline).
pub struct Structural {
    languages: std::sync::Arc<editor::SyntaxLanguages>,
}

impl Structural {
    pub fn new(languages: std::sync::Arc<editor::SyntaxLanguages>) -> Self {
        Self { languages }
    }

    /// Parse a side — INCREMENTALLY when the caller has an
    /// edit-adjusted (stale) tree: tree-sitter reuses everything the
    /// edits did not touch, so catching a keystroke up costs ~nothing,
    /// where a cold parse re-reads the whole file.
    fn parse(
        &self,
        language: &str,
        text: &Text,
        old: Option<&dyn editor::SyntaxTree>,
    ) -> Option<Box<dyn editor::SyntaxTree>> {
        let language = self.languages.ensure(language)?;
        let len = text.view().byte_count() as u32;
        language.parse(text, 0..len, old)
    }

    fn try_structural(
        &self,
        base: &Text,
        target: &Text,
        syntax: &editor::diff::DiffSyntax<'_>,
    ) -> Option<Operation> {
        let parsed_base;
        let base_tree = match &syntax.base {
            Some(side) if side.fresh => hisitter::TsTree::of(side.tree)?,
            side => {
                parsed_base =
                    self.parse(syntax.language, base, side.as_ref().map(|side| side.tree))?;
                hisitter::TsTree::of(parsed_base.as_ref())?
            }
        };
        let parsed_target;
        let target_tree = match &syntax.target {
            Some(side) if side.fresh => hisitter::TsTree::of(side.tree)?,
            side => {
                parsed_target =
                    self.parse(syntax.language, target, side.as_ref().map(|side| side.tree))?;
                hisitter::TsTree::of(parsed_target.as_ref())?
            }
        };
        structural(
            base,
            target,
            &SyntaxInput {
                left_tree: base_tree,
                right_tree: target_tree,
                language: Some(syntax.language),
            },
        )
    }
}

impl editor::diff::DiffPolicy for Structural {
    fn diff(
        &self,
        base: &Text,
        target: &Text,
        syntax: Option<&editor::diff::DiffSyntax<'_>>,
    ) -> Operation {
        let myers = myersdiff::diff(base, target);
        if let Some(syntax) = syntax {
            if worth_structural(&myers) {
                if let Some(operation) = self.try_structural(base, target, syntax) {
                    return operation;
                }
            }
        }
        myers
    }
}

fn structural(left: &Text, right: &Text, input: &SyntaxInput) -> Option<Operation> {
    let left_src = materialize(left);
    let right_src = materialize(right);
    if left_src.len().max(right_src.len()) > MAX_STRUCTURAL_BYTES {
        return None;
    }

    let lhs_arena = Arena::new();
    let rhs_arena = Arena::new();
    let lhs = convert::convert(&lhs_arena, input.left_tree, &left_src);
    let rhs = convert::convert(&rhs_arena, input.right_tree, &right_src);
    if error_heavy(&lhs) || error_heavy(&rhs) {
        return None;
    }

    // The engine, in upstream's call order (difftastic src/main.rs).
    init_all_info(&lhs.roots, &rhs.roots);
    let mut change_map = ChangeMap::default();
    let regions = mark_unchanged(&lhs.roots, &rhs.roots, &mut change_map);
    for (lhs_region, rhs_region) in regions {
        init_next_prev(&lhs_region);
        init_next_prev(&rhs_region);
        mark_syntax(
            lhs_region.first().copied(),
            rhs_region.first().copied(),
            &mut change_map,
            DEFAULT_GRAPH_LIMIT,
        )
        .ok()?;
    }
    let language = slider_language(input.language);
    fix_all_sliders(language, &lhs.roots, &mut change_map);
    fix_all_sliders(language, &rhs.roots, &mut change_map);

    let operation = derive::derive(
        &left_src,
        &right_src,
        &lhs.roots,
        &change_map,
        &line_starts(&left_src),
        &line_starts(&right_src),
    )?;
    debug_assert_eq!(
        apply(&operation, &left_src).as_deref(),
        Some(right_src.as_str()),
        "structural operation must be exact",
    );
    Some(operation)
}

fn error_heavy(converted: &convert::Converted) -> bool {
    converted.total_nodes > 0
        && converted.error_nodes as f64 / converted.total_nodes as f64 > MAX_ERROR_RATIO
}

fn materialize(text: &Text) -> String {
    let mut view = text.view();
    let count = view.byte_count();
    view.byte_string(0, count)
}

fn line_starts(src: &str) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (at, byte) in src.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(at as u32 + 1);
        }
    }
    starts
}

fn slider_language(name: Option<&str>) -> Language {
    // Only the prefer-outer-delimiter predicate reads this
    // (difftastic-core sliders.rs); everything else is
    // language-independent. Rust stands in for "prefers inner".
    match name.map(str::to_ascii_lowercase).as_deref() {
        Some("json" | "jsonc") => Language::Json,
        Some("toml") => Language::Toml,
        Some("hcl" | "terraform") => Language::Hcl,
        Some("sql") => Language::Sql,
        Some("clojure") => Language::Clojure,
        Some("scheme") => Language::Scheme,
        Some("racket") => Language::Racket,
        Some("commonlisp" | "common-lisp" | "lisp") => Language::CommonLisp,
        Some("elisp" | "emacs-lisp") => Language::EmacsLisp,
        Some("janet") => Language::Janet,
        _ => Language::Rust,
    }
}

/// Applies `operation` to `left`; `None` if it doesn't fit. Test/debug
/// support for the exactness contract.
pub fn apply(operation: &Operation, left: &str) -> Option<String> {
    use operation::Op;
    let mut out = String::with_capacity(operation.new_len() as usize);
    let mut at = 0usize;
    for op in operation.iter() {
        match op {
            Op::Retain(len) => {
                out.push_str(left.get(at..at + len as usize)?);
                at += len as usize;
            }
            Op::Delete(text) => {
                if left.get(at..at + text.len())? != text {
                    return None;
                }
                at += text.len();
            }
            Op::Insert(text) => out.push_str(&text),
        }
    }
    (at == left.len()).then_some(out)
}

#[cfg(test)]
mod gate {
    use super::*;
    use operation::Op;

    #[test]
    fn a_modest_change_is_worth_structural() {
        let op = Operation::from_ops([
            Op::Retain(1000),
            Op::Delete("old body".to_owned()),
            Op::Insert("new body".to_owned()),
            Op::Retain(1000),
        ]);
        assert!(worth_structural(&op));
    }

    #[test]
    fn a_scattered_rewrite_gates_out() {
        let mut ops = Vec::new();
        for _ in 0..300 {
            ops.push(Op::Retain(10));
            ops.push(Op::Delete("x".to_owned()));
            ops.push(Op::Insert("y".to_owned()));
        }
        assert!(!worth_structural(&Operation::from_ops(ops)));
    }

    #[test]
    fn a_bulk_replacement_gates_out() {
        let big = "x".repeat(65 * 1024);
        assert!(!worth_structural(&Operation::from_ops([
            Op::Delete(big.clone()),
            Op::Insert(big),
        ])));
    }
}
