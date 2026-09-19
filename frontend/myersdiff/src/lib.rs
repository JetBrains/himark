// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Myers text-diff policy (`similar`-backed): line-level pass
//! refined per-`Replace` block by a char-level pass. Moved verbatim
//! out of `editor::diff` so core crates carry no diff-engine
//! dependency — they call whatever `editor::diff::DiffPolicy` the
//! edge installed (docs/structural-diff.md, "Fallback to similar").
//! This crate is that baseline policy; `structdiff` layers the
//! difftastic engine on top and falls back here.

use std::ops::Range;

use operation::{Op, Operation};
use similar::{DiffTag, TextDiff};
use text::Text;

/// `editor::diff::DiffPolicy` face of [`diff`]. Ignores the syntax
/// context — Myers is the policy for texts without trees.
pub struct Myers;

impl editor::diff::DiffPolicy for Myers {
    fn diff(
        &self,
        base: &Text,
        target: &Text,
        _syntax: Option<&editor::diff::DiffSyntax<'_>>,
    ) -> Operation {
        diff(base, target)
    }
}

const REFINE_BUDGET: usize = 4 * 1024;

const RETAIN_NOISE: usize = 3;

pub fn diff(left: &Text, right: &Text) -> Operation {
    let left = materialize(left);
    let right = materialize(right);
    let mut ops: Vec<Op> = Vec::new();

    let lines = TextDiff::from_lines(left.as_str(), right.as_str());
    for op in lines.ops() {
        let old = byte_span(&left, lines.old_slices(), op.old_range());
        let new = byte_span(&right, lines.new_slices(), op.new_range());
        match op.tag() {
            DiffTag::Equal => push_retain(&mut ops, old.len()),
            DiffTag::Delete => push_delete(&mut ops, &left[old]),
            DiffTag::Insert => push_insert(&mut ops, &right[new]),
            DiffTag::Replace => refine(&mut ops, &left[old], &right[new]),
        }
    }
    Operation::from_ops(ops)
}

/// Char-level refinement of one replaced region: appends the ops that
/// turn `old` into `new`, budget-capped, with the short-retain noise
/// suppression. Public because the structural diff (structdiff crate)
/// refines its gap regions through the same pass so word tints behave
/// identically on both paths.
pub fn refine(ops: &mut Vec<Op>, old: &str, new: &str) {
    if old.len() > REFINE_BUDGET || new.len() > REFINE_BUDGET {
        push_delete(ops, old);
        push_insert(ops, new);
        return;
    }
    let chars = TextDiff::from_chars(old, new);

    let mut refined: Vec<Op> = Vec::new();
    for op in chars.ops() {
        let old_span = char_span(old, chars.old_slices(), op.old_range());
        let new_span = char_span(new, chars.new_slices(), op.new_range());
        match op.tag() {
            DiffTag::Equal => push_retain(&mut refined, old_span.len()),
            DiffTag::Delete => push_delete(&mut refined, &old[old_span]),
            DiffTag::Insert => push_insert(&mut refined, &new[new_span]),
            DiffTag::Replace => {
                push_delete(&mut refined, &old[old_span]);
                push_insert(&mut refined, &new[new_span]);
            }
        }
    }

    let mut old_at = 0usize;
    for (index, op) in refined.iter().enumerate() {
        let len = op.old_len() as usize;
        if let Op::Retain(_) = op {
            let interior = index > 0 && index + 1 < refined.len();
            let retained = &old[old_at..old_at + len];
            if interior && len < RETAIN_NOISE && !retained.contains('\n') {
                push_delete(ops, retained);
                push_insert(ops, retained);
                old_at += len;
                continue;
            }
        }
        match op {
            Op::Retain(len) => push_retain(ops, *len as usize),
            Op::Delete(text) => push_delete(ops, text),
            Op::Insert(text) => push_insert(ops, text),
        }
        old_at += len;
    }
}

fn push_retain(ops: &mut Vec<Op>, len: usize) {
    if len == 0 {
        return;
    }
    if let Some(Op::Retain(last)) = ops.last_mut() {
        *last += len as u32;
        return;
    }
    ops.push(Op::Retain(len as u32));
}

fn push_delete(ops: &mut Vec<Op>, text: &str) {
    if text.is_empty() {
        return;
    }

    if let Some(Op::Insert(_)) = ops.last() {
        let insert = ops.pop().expect("just matched");
        match ops.last_mut() {
            Some(Op::Delete(last)) => last.push_str(text),
            _ => ops.push(Op::Delete(text.to_owned())),
        }
        ops.push(insert);
        return;
    }
    if let Some(Op::Delete(last)) = ops.last_mut() {
        last.push_str(text);
        return;
    }
    ops.push(Op::Delete(text.to_owned()));
}

fn push_insert(ops: &mut Vec<Op>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Op::Insert(last)) = ops.last_mut() {
        last.push_str(text);
        return;
    }
    ops.push(Op::Insert(text.to_owned()));
}

fn materialize(text: &Text) -> String {
    let mut view = text.view();
    let count = view.byte_count();
    view.byte_string(0, count)
}

fn byte_span(source: &str, slices: &[&str], elements: Range<usize>) -> Range<usize> {
    span_of(source, slices, elements)
}

fn char_span(source: &str, slices: &[&str], elements: Range<usize>) -> Range<usize> {
    span_of(source, slices, elements)
}

fn span_of(source: &str, slices: &[&str], elements: Range<usize>) -> Range<usize> {
    let start: usize = slices[..elements.start].iter().map(|s| s.len()).sum();
    let len: usize = slices[elements.clone()].iter().map(|s| s.len()).sum();
    debug_assert!(source.is_char_boundary(start) && source.is_char_boundary(start + len));
    start..start + len
}
