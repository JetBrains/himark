// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! `ChangeMap` → `operation::Operation` (docs/editor/structural-diff.md,
//! "Deriving the Operation").
//!
//! Difftastic's matching is monotonic (no move detection), so matched
//! atoms appear in the same order on both sides and an edit script
//! exists: retains over matched atoms, delete+insert over the gaps,
//! with the gaps' byte-identical prefixes/suffixes coalesced back into
//! retains (interstitial whitespace is not covered by tree-sitter
//! nodes) and the rest refined by the shared char-level pass.
//!
//! Every retain is emitted only after comparing the actual bytes, and
//! any monotonicity violation aborts to `None` — the structural path
//! may produce a non-minimal operation, never a wrong one.

use std::ops::Range;

use difftastic_core::diff::changes::{ChangeKind, ChangeMap};
use difftastic_core::parse::syntax::Syntax;
use operation::{Op, Operation};

pub(crate) fn derive<'a>(
    left: &str,
    right: &str,
    lhs_roots: &[&'a Syntax<'a>],
    change_map: &ChangeMap<'a>,
    lhs_lines: &[u32],
    rhs_lines: &[u32],
) -> Option<Operation> {
    let mut pairs: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    for root in lhs_roots {
        collect(root, change_map, lhs_lines, rhs_lines, &mut pairs);
    }

    let mut ops: Vec<Op> = Vec::new();
    let mut left_at = 0usize;
    let mut right_at = 0usize;
    for (l, r) in pairs {
        // Monotonicity guard; sliders shuffle matches between
        // equal-content neighbours and must not produce crossings.
        if l.start < left_at || r.start < right_at {
            return None;
        }
        let matched_left = left.get(l.clone())?;
        let matched_right = right.get(r.clone())?;
        if matched_left != matched_right {
            return None;
        }
        gap(
            &mut ops,
            left.get(left_at..l.start)?,
            right.get(right_at..r.start)?,
        );
        push_retain(&mut ops, l.len());
        left_at = l.end;
        right_at = r.end;
    }
    gap(&mut ops, left.get(left_at..)?, right.get(right_at..)?);

    Some(Operation::from_ops(ops))
}

/// Preorder walk of the lhs tree collecting matched atom byte ranges.
/// Lists are always descended into — a novel list can still contain
/// unchanged children. `ReplacedComment`/`ReplacedString` atoms differ
/// in content and stay in the gap, where `refine` handles them.
fn collect<'a>(
    node: &'a Syntax<'a>,
    change_map: &ChangeMap<'a>,
    lhs_lines: &[u32],
    rhs_lines: &[u32],
    pairs: &mut Vec<(Range<usize>, Range<usize>)>,
) {
    match node {
        Syntax::Atom {
            position, content, ..
        } => {
            let Some(ChangeKind::Unchanged(opposite)) = change_map.get(node) else {
                return;
            };
            let Syntax::Atom {
                position: opposite_position,
                content: opposite_content,
                ..
            } = opposite
            else {
                return;
            };
            if content.len() != opposite_content.len() {
                return;
            }
            let (Some(l), Some(r)) = (
                byte_range(position, content.len(), lhs_lines),
                byte_range(opposite_position, opposite_content.len(), rhs_lines),
            ) else {
                return;
            };
            pairs.push((l, r));
        }
        Syntax::List { children, .. } => {
            for child in children {
                collect(child, change_map, lhs_lines, rhs_lines, pairs);
            }
        }
    }
}

/// Byte range of an atom from its stored line spans. `new_atom` trims a
/// trailing newline/CR from the content without adjusting the spans, so
/// the end is derived from the content length, not the last span.
fn byte_range(
    spans: &[line_numbers::SingleLineSpan],
    content_len: usize,
    line_starts: &[u32],
) -> Option<Range<usize>> {
    let first = spans.first()?;
    let line_start = *line_starts.get(first.line.as_usize())? as usize;
    let start = line_start + first.start_col as usize;
    Some(start..start + content_len)
}

/// Emits one gap: coalesces the byte-identical prefix and suffix into
/// retains, refines the differing core through the shared char pass.
fn gap(ops: &mut Vec<Op>, old: &str, new: &str) {
    if old.is_empty() && new.is_empty() {
        return;
    }
    let prefix = common_prefix(old, new);
    push_retain(ops, prefix);
    let (old, new) = (&old[prefix..], &new[prefix..]);
    let suffix = common_suffix(old, new);
    myersdiff::refine(ops, &old[..old.len() - suffix], &new[..new.len() - suffix]);
    push_retain(ops, suffix);
}

fn common_prefix(old: &str, new: &str) -> usize {
    let mut len = old
        .as_bytes()
        .iter()
        .zip(new.as_bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(len) {
        len -= 1;
    }
    len
}

fn common_suffix(old: &str, new: &str) -> usize {
    let mut len = old
        .as_bytes()
        .iter()
        .rev()
        .zip(new.as_bytes().iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - len) {
        len -= 1;
    }
    len
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
