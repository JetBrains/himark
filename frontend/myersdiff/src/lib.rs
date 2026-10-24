// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The text-diff policy: a LINE pass that matches the important
//! lines first and slides hunk edges to empty lines, refined inside
//! each replaced block by a WORD pass — words match whole, the
//! punctuation between matched words is matched on its own, and
//! equal whitespace on a change's edges is given back. The passes
//! are IntelliJ's comparison algorithms ported (`ByLineRt`,
//! `ByWordRt`, `ChunkOptimizer`; Apache 2.0) over `similar`'s Myers
//! engine. Core crates carry no diff-engine dependency — they call
//! whatever `editor::diff::DiffPolicy` the edge installed
//! (docs/editor/structural-diff.md); this crate is that baseline
//! policy, and `structdiff` layers the difftastic engine on top and
//! falls back here.

mod lines;
mod ranges;
mod words;

#[cfg(test)]
mod tests;

use operation::{op::Op, operation::Operation};
use text::text::Text;

/// `editor::diff::DiffPolicy` face of [`diff`]. Ignores the syntax
/// context — the text passes are the policy for texts without trees.
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

/// A replaced block past this many bytes on either side lands as
/// plain delete+insert — no word pass.
const REFINE_BUDGET: usize = 4 * 1024;

/// Ceiling on the line pass: past it `similar` degrades to cruder
/// hunks instead of stalling the worker — the diff stays exact either
/// way, only hunk quality drops. Myers is O((N+M)·D), unbounded on a
/// large rewrite without this.
const LINE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

/// Total word-pass budget per diff: a replaced block past it lands
/// as plain delete+insert (no word tints) rather than stall. Each
/// block is already capped at `REFINE_BUDGET`, but a diff can hold
/// hundreds of blocks.
const REFINE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(200);

pub fn diff(left: &Text, right: &Text) -> Operation {
    let left = materialize(left);
    let right = materialize(right);
    Operation::from_ops(diff_str(&left, &right))
}

/// The ops turning `left` into `right`: the line pass, each replaced
/// block refined by the word pass while the budget lasts.
pub(crate) fn diff_str(left: &str, right: &str) -> Vec<Op> {
    let mut ops: Vec<Op> = Vec::new();
    let line_deadline = std::time::Instant::now() + LINE_DEADLINE;
    let blocks = lines::blocks(left, right, Some(line_deadline));
    let refine_until = std::time::Instant::now() + REFINE_DEADLINE;
    for block in blocks {
        let old = &left[block.old];
        let new = &right[block.new];
        if block.equal {
            push_retain(&mut ops, old.len());
        } else if old.is_empty() {
            push_insert(&mut ops, new);
        } else if new.is_empty() {
            push_delete(&mut ops, old);
        } else if std::time::Instant::now() < refine_until {
            refine(&mut ops, old, new);
        } else {
            push_delete(&mut ops, old);
            push_insert(&mut ops, new);
        }
    }
    ops
}

/// Word-level refinement of one replaced region: appends the ops that
/// turn `old` into `new`, budget-capped. Public because the structural
/// diff (structdiff crate) refines its gap regions through the same
/// pass so word tints behave identically on both paths.
pub fn refine(ops: &mut Vec<Op>, old: &str, new: &str) {
    if old.len() > REFINE_BUDGET || new.len() > REFINE_BUDGET {
        push_delete(ops, old);
        push_insert(ops, new);
        return;
    }
    let (mut at1, mut at2) = (0usize, 0usize);
    for change in words::changes(old, new, None) {
        debug_assert_eq!(change.start1 - at1, change.start2 - at2, "the stretch between changes is equal");
        push_retain(ops, change.start1 - at1);
        push_delete(ops, &old[change.start1..change.end1]);
        push_insert(ops, &new[change.start2..change.end2]);
        at1 = change.end1;
        at2 = change.end2;
    }
    push_retain(ops, old.len() - at1);
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
    // A delete sits BEFORE the insert it pairs with.
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
