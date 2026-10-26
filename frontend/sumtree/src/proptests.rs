// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Generative properties over the sum tree. The oracle is a
//! `Vec<Span>`: every structural edit is mirrored into it, every
//! query recomputed by a linear fold. The tree is persistent, so
//! the sequence also keeps earlier versions alive and checks they
//! never move.

use proptest::prelude::*;

use crate::summary::{Bias, Item as _, Seek, Summary as _};
use crate::tests::{
    apply_to_model, assert_invariants, assert_matches_model, naive_summary, Bytes, Span,
    SpanSummary,
};
use crate::tree::{Splice, SumTree};

#[derive(Debug, Clone)]
enum Edit {
    /// Ascending, non-overlapping cuts (raw values resolved modulo
    /// `len + 1` and sorted at run time) each with its insertion.
    Splice(Vec<(u32, u32, Vec<Span>)>),
    Set(u32, Span),
    /// Keep `a..b` only.
    Slice(u32, u32),
    /// Append a fresh tree of these spans.
    Append(Vec<Span>),
    /// Prepend a fresh tree of these spans.
    Prepend(Vec<Span>),
}

fn span() -> impl Strategy<Value = Span> {
    (0u32..10_000, prop_oneof![3 => 1u32..200, 1 => Just(0u32)])
        .prop_map(|(width, bytes)| Span { width, bytes })
}

fn spans(max: usize) -> impl Strategy<Value = Vec<Span>> {
    prop::collection::vec(span(), 0..max)
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        4 => prop::collection::vec((any::<u32>(), any::<u32>(), spans(70)), 1..4).prop_map(Edit::Splice),
        2 => (any::<u32>(), span()).prop_map(|(index, span)| Edit::Set(index, span)),
        1 => (any::<u32>(), any::<u32>()).prop_map(|(a, b)| Edit::Slice(a, b)),
        1 => spans(140).prop_map(Edit::Append),
        1 => spans(140).prop_map(Edit::Prepend),
    ]
}

fn resolve_splices(model: &[Span], cuts: &[(u32, u32, Vec<Span>)]) -> Vec<Splice<Span>> {
    let len = model.len();
    let mut cuts: Vec<(usize, usize, Vec<Span>)> = cuts
        .iter()
        .map(|(a, b, insert)| {
            let a = *a as usize % (len + 1);
            let b = *b as usize % (len + 1);
            (a.min(b), a.max(b), insert.clone())
        })
        .collect();
    cuts.sort_by_key(|(a, ..)| *a);
    let mut out: Vec<Splice<Span>> = Vec::new();
    for (a, b, insert) in cuts {
        if out.last().is_some_and(|last| a < last.range.end) {
            continue;
        }
        out.push(Splice {
            range: a..b,
            insert,
        });
    }
    out
}

fn apply(tree: &SumTree<Span>, model: &mut Vec<Span>, edit: &Edit) -> SumTree<Span> {
    let len = model.len();
    match edit {
        Edit::Splice(cuts) => {
            let splices = resolve_splices(model, cuts);
            let next = apply_to_model(model, &splices);
            *model = next;
            tree.splice(splices)
        }
        Edit::Set(index, span) => {
            if len == 0 {
                return tree.clone();
            }
            let index = *index as usize % len;
            model[index] = span.clone();
            tree.set(index, span.clone())
        }
        Edit::Slice(a, b) => {
            let a = *a as usize % (len + 1);
            let b = *b as usize % (len + 1);
            let range = a.min(b)..a.max(b);
            *model = model[range.clone()].to_vec();
            tree.slice(range)
        }
        Edit::Append(spans) => {
            model.extend(spans.iter().cloned());
            tree.append(&spans.iter().cloned().collect())
        }
        Edit::Prepend(spans) => {
            let mut next = spans.clone();
            next.extend(model.iter().cloned());
            *model = next;
            let fresh: SumTree<Span> = spans.iter().cloned().collect();
            fresh.append(tree)
        }
    }
}

fn byte_offsets(model: &[Span]) -> Vec<u64> {
    let mut out = Vec::with_capacity(model.len() + 1);
    let mut offset = 0u64;
    out.push(0);
    for span in model {
        offset += u64::from(span.bytes);
        out.push(offset);
    }
    out
}

fn model_find(model: &[Span], target: u64, bias: Bias) -> Seek<Bytes> {
    let offsets = byte_offsets(model);
    for (index, span) in model.iter().enumerate() {
        let end = offsets[index] + u64::from(span.bytes);
        let past = match bias {
            Bias::Left => end >= target,
            Bias::Right => end > target,
        };
        if past {
            return Seek {
                index,
                start: Bytes(offsets[index]),
            };
        }
    }
    Seek {
        index: model.len(),
        start: Bytes(*offsets.last().expect("an offset")),
    }
}

/// The intersection rule the existing tests state: a span counts
/// when its byte range overlaps `a..b`, or when it is empty and
/// sits strictly inside. An EMPTY range `a..a` is a point, not a
/// span of bytes, and summarises to nothing — even when a span
/// straddles the point (the existing rule, read literally, would
/// count it; the tree does not, and the width tree's `max_in(a..a)`
/// leans on that).
fn model_summary_between(model: &[Span], a: u64, b: u64) -> SpanSummary {
    let mut expected = SpanSummary::empty();
    if a >= b {
        return expected;
    }
    let mut offset = 0u64;
    for span in model {
        let end = offset + u64::from(span.bytes);
        let intersects = offset < b && end > a;
        let zero_inside = offset == end && a < offset && offset < b;
        if intersects || zero_inside {
            expected.add(&span.summary());
        }
        offset = end;
    }
    expected
}

fn check_queries(
    tree: &SumTree<Span>,
    model: &[Span],
    probes: &[(u32, u32)],
) -> Result<(), TestCaseError> {
    let len = model.len();
    let offsets = byte_offsets(model);
    let total = *offsets.last().expect("an offset");
    for (raw_a, raw_b) in probes {
        let a = *raw_a as usize % (len + 1);
        let b = *raw_b as usize % (len + 1);
        let range = a.min(b)..a.max(b);
        prop_assert_eq!(
            tree.summary_in(range.clone()),
            naive_summary(&model[range.clone()]),
            "summary_in {:?}",
            range
        );
        let walked: Vec<Span> = tree.iter_in(range.clone()).cloned().collect();
        prop_assert_eq!(&walked[..], &model[range.clone()], "iter_in {:?}", range);
        prop_assert_eq!(
            tree.offset_of::<Bytes>(a),
            Bytes(offsets[a]),
            "offset_of {}",
            a
        );
        prop_assert_eq!(tree.get(a), model.get(a), "get {}", a);

        let ta = u64::from(*raw_a) % (total + 2);
        let tb = u64::from(*raw_b) % (total + 2);
        for bias in [Bias::Left, Bias::Right] {
            prop_assert_eq!(
                tree.find(Bytes(ta), bias),
                model_find(model, ta, bias),
                "find {} {:?}",
                ta,
                bias
            );
        }
        let (lo, hi) = (ta.min(tb), ta.max(tb));
        prop_assert_eq!(
            tree.summary_between(Bytes(lo)..Bytes(hi)),
            model_summary_between(model, lo, hi),
            "summary_between {}..{}",
            lo,
            hi
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// A random sequence of splices, sets, slices, appends and
    /// prepends, mirrored into a `Vec`: after each edit the tree
    /// holds the model's items and summary, its invariants hold, and
    /// every query — range summaries, range walks, offsets, finds in
    /// both biases, dimension summaries — agrees with a linear fold.
    #[test]
    fn edited_trees_match_the_vec_model_under_every_query(
        initial in spans(300),
        edits in prop::collection::vec(edit(), 1..12),
        probes in prop::collection::vec((any::<u32>(), any::<u32>()), 6),
    ) {
        let mut model = initial.clone();
        let mut tree: SumTree<Span> = initial.into_iter().collect();
        assert_matches_model(&tree, &model);
        for edit in &edits {
            tree = apply(&tree, &mut model, edit);
            assert_matches_model(&tree, &model);
            check_queries(&tree, &model, &probes)?;
        }
    }

    /// Persistence: every version produced along the way is kept and
    /// still equals the model it was taken from after all later
    /// edits — a later version must never write through a shared
    /// node.
    #[test]
    fn earlier_versions_survive_later_edits(
        initial in spans(200),
        edits in prop::collection::vec(edit(), 1..10),
    ) {
        let mut model = initial.clone();
        let mut tree: SumTree<Span> = initial.into_iter().collect();
        let mut versions = vec![(tree.clone(), model.clone())];
        for edit in &edits {
            tree = apply(&tree, &mut model, edit);
            versions.push((tree.clone(), model.clone()));
        }
        for (version, snapshot) in &versions {
            assert_matches_model(version, snapshot);
            assert_invariants(version);
        }
    }

    /// Splicing a tree in one pass equals splicing it one cut at a
    /// time from the back (so earlier indices stay valid): the bulk
    /// path and the single path must agree.
    #[test]
    fn a_bulk_splice_equals_its_cuts_applied_singly(
        initial in spans(300),
        cuts in prop::collection::vec((any::<u32>(), any::<u32>(), spans(40)), 1..6),
    ) {
        let model = initial.clone();
        let tree: SumTree<Span> = initial.into_iter().collect();
        let splices = resolve_splices(&model, &cuts);
        let expected = apply_to_model(&model, &splices);
        let bulk = tree.splice(splices.iter().map(|splice| Splice {
            range: splice.range.clone(),
            insert: splice.insert.clone(),
        }));
        let mut singly = tree.clone();
        for splice in splices.iter().rev() {
            singly = singly.splice([Splice {
                range: splice.range.clone(),
                insert: splice.insert.clone(),
            }]);
        }
        assert_matches_model(&bulk, &expected);
        assert_matches_model(&singly, &expected);
    }
}
