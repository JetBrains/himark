// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Generative properties over the width tree. The oracle is a
//! `Vec<f32>` with one width per BYTE: every edit the tree takes is
//! mirrored as a splice, every query recomputed as a fold over the
//! slice. Spans are generated non-empty — a zero-byte span has no
//! byte to carry its width in the model.

use operation::{op::Op, operation::Operation};
use proptest::prelude::*;

use crate::width_tree::{WidthSpan, WidthTree};

#[derive(Debug, Clone)]
enum Hunk {
    Retain(u32),
    Insert(u32),
    Delete(u32),
}

#[derive(Debug, Clone)]
enum Edit {
    /// `replace_bytes` over `a..b` (raw, resolved modulo `len + 1`)
    /// with these spans.
    Replace(u32, u32, Vec<(u32, u32)>),
    /// A multi-hunk operation, resolved against the model's length
    /// at run time so every hunk stays in range.
    Operation(Vec<Hunk>),
    Clear,
}

fn width() -> impl Strategy<Value = u32> {
    0u32..5_000
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        4 => (
            any::<u32>(),
            any::<u32>(),
            prop::collection::vec((1u32..30, width()), 0..4),
        )
            .prop_map(|(a, b, spans)| Edit::Replace(a, b, spans)),
        5 => prop::collection::vec(
            prop_oneof![
                3 => (0u32..40).prop_map(Hunk::Retain),
                2 => (1u32..25).prop_map(Hunk::Insert),
                2 => (1u32..40).prop_map(Hunk::Delete),
            ],
            1..6,
        )
            .prop_map(Edit::Operation),
        1 => Just(Edit::Clear),
    ]
}

fn spans_of(spans: &[(u32, u32)]) -> (Vec<WidthSpan>, Vec<f32>) {
    let mut out = Vec::with_capacity(spans.len());
    let mut bytes = Vec::new();
    for (len, width) in spans {
        out.push(WidthSpan {
            bytes: *len,
            width: *width as f32,
        });
        bytes.extend(std::iter::repeat(*width as f32).take(*len as usize));
    }
    (out, bytes)
}

/// Builds the operation the hunks describe over a text of `len`
/// bytes, clamping each hunk to what remains, and applies it to the
/// model exactly as the tree walks it: offsets advance over retains
/// and inserts, deletes hold.
fn resolve_operation(model: &mut Vec<f32>, hunks: &[Hunk]) -> Option<Operation> {
    let len = model.len() as u32;
    let mut ops = Vec::new();
    let mut consumed = 0u32;
    let mut offset = 0usize;
    for hunk in hunks {
        match hunk {
            Hunk::Retain(n) => {
                let n = (*n).min(len - consumed);
                if n > 0 {
                    ops.push(Op::Retain(n));
                }
                consumed += n;
                offset += n as usize;
            }
            Hunk::Insert(n) => {
                ops.push(Op::Insert("x".repeat(*n as usize)));
                let width = match offset {
                    0 => model.first().copied().unwrap_or(0.0),
                    _ => model[offset - 1],
                };
                model.splice(offset..offset, std::iter::repeat(width).take(*n as usize));
                offset += *n as usize;
            }
            Hunk::Delete(n) => {
                let n = (*n).min(len - consumed);
                if n > 0 {
                    ops.push(Op::Delete("x".repeat(n as usize)));
                    model.drain(offset..offset + n as usize);
                }
                consumed += n;
            }
        }
    }
    if consumed < len {
        ops.push(Op::Retain(len - consumed));
    }
    (!ops.is_empty()).then(|| Operation::from_ops(ops))
}

fn apply(tree: &mut WidthTree, model: &mut Vec<f32>, edit: &Edit) {
    let len = model.len();
    match edit {
        Edit::Replace(a, b, spans) => {
            let a = *a as usize % (len + 1);
            let b = *b as usize % (len + 1);
            let (a, b) = (a.min(b), a.max(b));
            let (spans, bytes) = spans_of(spans);
            tree.replace_bytes(a as u64..b as u64, spans);
            model.splice(a..b, bytes);
        }
        Edit::Operation(hunks) => {
            if let Some(operation) = resolve_operation(model, hunks) {
                tree.edit(&operation);
            }
        }
        Edit::Clear => {
            tree.clear();
            model.clear();
        }
    }
}

fn model_max_in(model: &[f32], range: std::ops::Range<usize>) -> f32 {
    model[range.start.min(model.len())..range.end.min(model.len())]
        .iter()
        .fold(0.0f32, |max, width| max.max(*width))
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// Random replacements, multi-hunk operations and clears,
    /// mirrored byte for byte into a `Vec<f32>`: after every edit the
    /// tree covers the model's byte count, its widest width is the
    /// model's, and the widest width over any byte range is the
    /// fold over that slice.
    #[test]
    fn edited_width_trees_match_the_per_byte_model(
        initial in prop::collection::vec((1u32..30, width()), 0..12),
        edits in prop::collection::vec(edit(), 1..25),
        probes in prop::collection::vec((any::<u32>(), any::<u32>()), 8),
    ) {
        let (spans, mut model) = spans_of(&initial);
        let mut tree = WidthTree::new();
        tree.replace_bytes(0..0, spans);
        prop_assert_eq!(tree.byte_size(), model.len() as u64);
        for edit in &edits {
            apply(&mut tree, &mut model, edit);
            prop_assert_eq!(tree.byte_size(), model.len() as u64, "byte coverage after {:?}", edit);
            prop_assert_eq!(tree.max(), model_max_in(&model, 0..model.len()), "whole max after {:?}", edit);
            let len = model.len();
            for (a, b) in &probes {
                let a = *a as usize % (len + 2);
                let b = *b as usize % (len + 2);
                let range = a.min(b)..a.max(b);
                prop_assert_eq!(
                    tree.max_in(range.start as u32..range.end as u32),
                    model_max_in(&model, range.clone()),
                    "max_in {:?} over {} bytes after {:?}",
                    range,
                    len,
                    edit
                );
            }
        }
    }
}
