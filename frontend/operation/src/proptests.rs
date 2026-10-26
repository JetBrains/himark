// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Generative laws of the operation algebra. The oracle throughout is
//! a NAIVE string model: an operation is a recipe for splicing one
//! `String` into another, so every law is stated as "the recipes
//! agree on the strings they produce" — the model needs no rope, no
//! builder and no cursor, which is exactly why it can judge them.

use proptest::prelude::*;

use crate::{op::Op, operation::Bias, operation::Operation};

/// Pieces a base text is assembled from: ASCII, two- and three-byte
/// UTF-8, an emoji (four bytes, two UTF-16 units) and newlines, so
/// every byte-versus-char-versus-unit confusion has a chance to show.
const PIECES: &[&str] = &[
    "a", "bc", "hello", " ", "\n", "é", "中", "😀", "—", "x\n", "",
];

/// Words an operation inserts. The empty word is deliberate: the
/// builder must drop it, and the model must not notice it.
const WORDS: &[&str] = &["", "q", "βγ", "😀", "\n", "tail\n", "中a"];

/// One step of an edit plan: what to do with the next `step` chars
/// of the base (`kind`: 0 and 1 retain, 2 delete, 3 insert `word`
/// then retain) — a plan is independent of its base, so one plan can
/// be replayed over any text and still cover it exactly.
type Step = (u8, usize, usize);

fn step() -> impl Strategy<Value = Step> {
    (0..4u8, 1..=3usize, 0..WORDS.len())
}

fn plan() -> impl Strategy<Value = Vec<Step>> {
    prop::collection::vec(step(), 0..12)
}

fn text() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(PIECES), 0..40).prop_map(|pieces| pieces.concat())
}

/// The operation a plan spells over `source`: it walks the char
/// boundaries, so it covers the base EXACTLY (old_len is the byte
/// length) and never splits a code point. A plan is replayed
/// cyclically; an empty plan retains everything.
fn operation_over(source: &str, plan: &[Step]) -> Operation {
    let boundaries: Vec<usize> = source
        .char_indices()
        .map(|(index, _)| index)
        .chain([source.len()])
        .collect();
    let mut ops = Vec::new();
    let mut at = 0usize;
    let mut cursor = 0usize;
    while at + 1 < boundaries.len() {
        let (kind, step, word) = match plan.is_empty() {
            true => (0, 1, 0),
            false => plan[cursor % plan.len()],
        };
        cursor += 1;
        let next = (at + step).min(boundaries.len() - 1);
        let (from, to) = (boundaries[at], boundaries[next]);
        match kind {
            0 | 1 => ops.push(Op::Retain((to - from) as u32)),
            2 => ops.push(Op::Delete(source[from..to].to_owned())),
            _ => {
                ops.push(Op::Insert(WORDS[word].to_owned()));
                ops.push(Op::Retain((to - from) as u32));
            }
        }
        at = next;
    }
    if let Some(&(3, _, word)) = plan.last() {
        ops.push(Op::Insert(WORDS[word].to_owned()));
    }
    Operation::from_ops(ops)
}

/// The model: splice `base` by the recipe, checking that the recipe
/// fits the base (deletes name the bytes they remove, retains stay on
/// char boundaries, the whole base is consumed). The EMPTY operation
/// is the crate's universal no-op — compose and transform both treat
/// it as the identity over any base — so the model does too.
fn model_apply(base: &str, operation: &Operation) -> String {
    if operation.is_empty() {
        return base.to_owned();
    }
    let mut cursor = 0usize;
    let mut out = String::new();
    for op in operation.iter() {
        match op {
            Op::Retain(len) => {
                let end = cursor + len as usize;
                out.push_str(
                    base.get(cursor..end)
                        .expect("retain must stay on UTF-8 boundaries"),
                );
                cursor = end;
            }
            Op::Insert(text) => out.push_str(&text),
            Op::Delete(text) => {
                let end = cursor + text.len();
                assert_eq!(
                    base.get(cursor..end),
                    Some(text.as_str()),
                    "delete must name the bytes it removes"
                );
                cursor = end;
            }
        }
    }
    assert_eq!(cursor, base.len(), "operation must consume the whole base");
    out
}

/// The model of `transform_offset`: walk the ops, shifting the probe
/// by inserts before it and deletes over it, with the bias deciding
/// which side of an insert AT the probe the probe lands on.
fn model_transform_offset(operation: &Operation, offset: u32, bias: Bias) -> u32 {
    let mut old = 0u32;
    let mut transformed = offset;
    for op in operation.iter() {
        match op {
            Op::Retain(len) => old += len,
            Op::Insert(text) => {
                let shifts = match bias {
                    Bias::Left => old < offset,
                    Bias::Right => old <= offset,
                };
                if shifts {
                    transformed += text.len() as u32;
                }
            }
            Op::Delete(text) => {
                let len = text.len() as u32;
                if old + len <= offset {
                    transformed -= len;
                } else if old < offset {
                    transformed -= offset - old;
                }
                old += len;
            }
        }
        let past = match bias {
            Bias::Left => old >= offset,
            Bias::Right => old > offset,
        };
        if past {
            break;
        }
    }
    transformed
}

/// The model of `next_retained_*`: the first retain run, on the given
/// axis, whose end lies past `offset`.
fn model_next_retained(operation: &Operation, offset: u32, new_axis: bool) -> Option<(u32, u32)> {
    let mut at = 0u32;
    for op in operation.iter() {
        let len = match new_axis {
            true => op.new_len(),
            false => op.old_len(),
        };
        if matches!(op, Op::Retain(_)) && at + len > offset {
            return Some((at, at + len));
        }
        at += len;
    }
    None
}

/// The canonical form of a raw op list: empties gone, neighbours of a
/// kind merged. What the builder must produce from any input.
fn model_normalize(ops: &[Op]) -> Vec<Op> {
    let mut out: Vec<Op> = Vec::new();
    for op in ops {
        if op.is_empty() {
            continue;
        }
        match (out.last_mut(), op) {
            (Some(Op::Retain(existing)), Op::Retain(len)) => *existing += len,
            (Some(Op::Insert(existing)), Op::Insert(text)) => existing.push_str(text),
            (Some(Op::Delete(existing)), Op::Delete(text)) => existing.push_str(text),
            _ => out.push(op.clone()),
        }
    }
    out
}

fn raw_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..9u32).prop_map(Op::Retain),
        prop::sample::select(WORDS).prop_map(|word| Op::Insert(word.to_owned())),
        prop::sample::select(WORDS).prop_map(|word| Op::Delete(word.to_owned())),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, .. ProptestConfig::default() })]

    /// An operation's two lengths are what the model sees: it eats
    /// exactly `old_len` bytes of the base and yields `new_len` bytes,
    /// and both stay valid UTF-8 because the plan never splits a char.
    #[test]
    fn the_lengths_an_operation_reports_are_the_lengths_the_model_splices(
        source in text(),
        plan in plan(),
    ) {
        let operation = operation_over(&source, &plan);
        let result = model_apply(&source, &operation);
        prop_assert_eq!(operation.old_len() as usize, source.len());
        prop_assert_eq!(operation.new_len() as usize, result.len());
        prop_assert_eq!(operation.is_empty(), source.is_empty() && result.is_empty());
    }

    /// Composition is sequencing: applying `a` then `b` reaches the
    /// same text as applying `compose(a, b)` once, and the composite
    /// spans from `a`'s base to `b`'s result.
    #[test]
    fn composing_two_operations_applies_them_in_sequence(
        source in text(),
        first in plan(),
        second in plan(),
    ) {
        let a = operation_over(&source, &first);
        let mid = model_apply(&source, &a);
        let b = operation_over(&mid, &second);
        let composed = a.compose(&b);
        prop_assert_eq!(composed.old_len(), a.old_len());
        prop_assert_eq!(composed.new_len(), b.new_len());
        prop_assert_eq!(model_apply(&source, &composed), model_apply(&mid, &b));
    }

    /// The splice shortcuts are optimizations of `compose`, so they
    /// must land on the same text whichever side carries the margins.
    #[test]
    fn splice_composition_agrees_with_full_composition(
        source in text(),
        first in plan(),
        second in plan(),
    ) {
        let a = operation_over(&source, &first);
        let mid = model_apply(&source, &a);
        let b = operation_over(&mid, &second);
        let expected = model_apply(&mid, &b);
        let spliced = a.splice_compose(&b);
        prop_assert_eq!(spliced.old_len(), a.old_len());
        prop_assert_eq!(model_apply(&source, &spliced), expected.clone());
        let spliced_into = a.splice_compose_into(&b);
        prop_assert_eq!(spliced_into.old_len(), a.old_len());
        prop_assert_eq!(model_apply(&source, &spliced_into), expected);
    }

    /// Transformation converges: two operations over one base, each
    /// transformed against the other, reach the same text in either
    /// order. This is the law docsync and the undo seam lean on.
    #[test]
    fn transforming_concurrent_operations_converges(
        source in text(),
        left in plan(),
        right in plan(),
    ) {
        let a = operation_over(&source, &left);
        let b = operation_over(&source, &right);
        let a_prime = a.transform(&b);
        let b_prime = b.transform(&a);
        if !a.is_empty() && !b.is_empty() {
            prop_assert_eq!(a_prime.old_len(), b.new_len());
            prop_assert_eq!(b_prime.old_len(), a.new_len());
        }
        let a_then_b = model_apply(&model_apply(&source, &a), &b_prime);
        let b_then_a = model_apply(&model_apply(&source, &b), &a_prime);
        prop_assert_eq!(a_then_b, b_then_a);
    }

    /// Offset mapping agrees with a linear walk for every offset in
    /// and just past the base, under both biases; mapping back is
    /// mapping forward through the inverse.
    #[test]
    fn offset_transformation_matches_the_linear_model(
        source in text(),
        plan in plan(),
    ) {
        let operation = operation_over(&source, &plan);
        let inverted = operation.invert();
        for offset in 0..=operation.old_len() + 2 {
            for bias in [Bias::Left, Bias::Right] {
                prop_assert_eq!(
                    operation.transform_offset(offset, bias),
                    model_transform_offset(&operation, offset, bias),
                    "offset {} bias {:?}", offset, bias
                );
            }
        }
        for offset in 0..=operation.new_len() + 2 {
            for bias in [Bias::Left, Bias::Right] {
                prop_assert_eq!(
                    operation.transform_offset_back(offset, bias),
                    inverted.transform_offset(offset, bias),
                    "back offset {} bias {:?}", offset, bias
                );
            }
        }
    }

    /// Inversion undoes: applying `a` then `invert(a)` restores the
    /// base, and inverting twice is the identity on the recipe.
    #[test]
    fn inverting_an_operation_restores_the_base(
        source in text(),
        plan in plan(),
    ) {
        let operation = operation_over(&source, &plan);
        let inverted = operation.invert();
        prop_assert_eq!(inverted.old_len(), operation.new_len());
        prop_assert_eq!(inverted.new_len(), operation.old_len());
        let result = model_apply(&source, &operation);
        prop_assert_eq!(model_apply(&result, &inverted), source);
        prop_assert_eq!(inverted.invert(), operation);
    }

    /// The builder normalizes any raw op list to its canonical form:
    /// no empty ops, no two neighbours of one kind, lengths that are
    /// the sums of the parts — and iterating gives that form back.
    #[test]
    fn building_from_raw_ops_yields_the_canonical_form(
        ops in prop::collection::vec(raw_op(), 0..40),
    ) {
        let operation = Operation::from_ops(ops.clone());
        let expected = model_normalize(&ops);
        let actual: Vec<Op> = operation.iter().collect();
        prop_assert_eq!(&actual, &expected);
        prop_assert_eq!(operation.size(), expected.len());
        prop_assert_eq!(
            operation.old_len(),
            ops.iter().map(Op::old_len).sum::<u32>()
        );
        prop_assert_eq!(
            operation.new_len(),
            ops.iter().map(Op::new_len).sum::<u32>()
        );
        prop_assert_eq!(Operation::from_ops(actual), operation);
    }

    /// Seeking the next retained run agrees with a linear walk on
    /// both axes, for every offset in and past the operation.
    #[test]
    fn the_next_retained_run_matches_the_linear_model(
        source in text(),
        plan in plan(),
    ) {
        let operation = operation_over(&source, &plan);
        for offset in 0..=operation.old_len() + 2 {
            prop_assert_eq!(
                operation.next_retained_old(offset).map(|range| (range.start, range.end)),
                model_next_retained(&operation, offset, false),
                "old offset {}", offset
            );
        }
        for offset in 0..=operation.new_len() + 2 {
            prop_assert_eq!(
                operation.next_retained_new(offset).map(|range| (range.start, range.end)),
                model_next_retained(&operation, offset, true),
                "new offset {}", offset
            );
        }
    }

    /// Resuming iteration at an old offset yields a suffix of the
    /// full walk, seeded at an op start at or before the offset.
    #[test]
    fn resuming_iteration_yields_a_suffix_of_the_full_walk(
        source in text(),
        plan in plan(),
    ) {
        let operation = operation_over(&source, &plan);
        let full: Vec<(u32, u32, Op)> = {
            let (mut old, mut new) = (0u32, 0u32);
            operation
                .iter()
                .map(|op| {
                    let at = (old, new, op.clone());
                    old += op.old_len();
                    new += op.new_len();
                    at
                })
                .collect()
        };
        for offset in 0..=operation.old_len() + 2 {
            let from = operation.ops_from_old(offset);
            let seeded: Vec<Op> = from.ops.collect();
            if offset >= operation.old_len() {
                prop_assert!(seeded.is_empty(), "past the end yields nothing");
                continue;
            }
            let at = full
                .iter()
                .position(|(old, new, _)| (*old, *new) == (from.old_start, from.new_start));
            prop_assert!(at.is_some(), "seed is an op start");
            let suffix: Vec<Op> = full[at.unwrap()..].iter().map(|(_, _, op)| op.clone()).collect();
            prop_assert_eq!(seeded, suffix);
            prop_assert!(from.old_start <= offset);
        }
    }
}
