// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Generative properties over the rope cursor. The oracle is a
//! `Vec<char>`: every seek is checked against a linear scan, every
//! edit against a splice. The sequences drive ONE cursor through
//! many steps — the production callers (text, layout, lists) keep a
//! cursor across a whole operation, so the tree shapes an edit
//! leaves behind (emptied leaves, past-the-end positions, deep
//! unbalanced paths) are the shapes the next seek has to cope with.

use proptest::prelude::*;

use crate::cursor::{Cursor, SeekMode};
use crate::metrics::{Measure as _, MetricId, Metrics};
use crate::rope::Rope;
use crate::tests::{
    assert_cursor_well_balanced, model_seek, model_seek_before, rope_from_iter,
    rope_from_iter_with_branching, rope_to_string, TextMeasure,
};

#[derive(Debug, Clone, Copy)]
enum SeekSpec {
    /// `seek_to_index`, raw value taken modulo `len + 2` at run time
    /// so a shrunk sequence stays in range of its own model.
    Index(u32),
    /// A metric seek: metric 0 (count) or 1 (newlines), value taken
    /// modulo `total + 2`.
    Metric(bool, u32, SeekMode),
    /// Exactly the rope's end in metric 0 — the boundary that
    /// exposed the emptied-trailing-leaf seek (commit 63ff887a).
    End(SeekMode),
    /// Exactly the last element's start in metric 0.
    Last(SeekMode),
}

#[derive(Debug, Clone)]
struct Step {
    seek: SeekSpec,
    delete: u8,
    insert: Vec<char>,
}

fn element() -> impl Strategy<Value = char> {
    prop_oneof![Just('a'), Just('b'), Just('c'), Just('\n')]
}

fn seek_mode() -> impl Strategy<Value = SeekMode> {
    prop_oneof![Just(SeekMode::Before), Just(SeekMode::After)]
}

fn seek_spec() -> impl Strategy<Value = SeekSpec> {
    prop_oneof![
        2 => any::<u32>().prop_map(SeekSpec::Index),
        4 => (any::<bool>(), any::<u32>(), seek_mode())
            .prop_map(|(lines, value, mode)| SeekSpec::Metric(lines, value, mode)),
        3 => seek_mode().prop_map(SeekSpec::End),
        2 => seek_mode().prop_map(SeekSpec::Last),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    (
        seek_spec(),
        // Mostly tiny deletes: single-element deletes are what empty
        // a leaf one element at a time.
        prop_oneof![5 => 0u8..2, 2 => 2u8..6, 1 => 6u8..40],
        prop::collection::vec(element(), 0..6),
    )
        .prop_map(|(seek, delete, insert)| Step {
            seek,
            delete,
            insert,
        })
}

fn content() -> impl Strategy<Value = Vec<char>> {
    prop::collection::vec(element(), 0..200)
}

fn steps() -> impl Strategy<Value = Vec<Step>> {
    prop::collection::vec(step(), 1..60)
}

fn metrics_of(chars: &[char]) -> Metrics<2> {
    let mut out = Metrics::zero();
    for ch in chars {
        out.add_assign(TextMeasure::measure(ch));
    }
    out
}

/// What the model expects of a seek: `None` when the rope is empty
/// (the seek reports false), otherwise the element index the cursor
/// lands on and its position.
fn model_landing(model: &[char], spec: SeekSpec) -> Option<(usize, Metrics<2>)> {
    if model.is_empty() {
        return None;
    }
    let len = model.len() as u32;
    let total = metrics_of(model);
    match spec {
        SeekSpec::Index(raw) => {
            let index = (raw % (len + 2)).min(len - 1) as usize;
            Some((index, metrics_of(&model[..index])))
        }
        SeekSpec::Metric(lines, raw, mode) => {
            let id = MetricId(usize::from(lines));
            let value = raw % (total.metric_at(id) + 2);
            seek_model(model, id, value, mode)
        }
        SeekSpec::End(mode) => seek_model(model, MetricId(0), len, mode),
        SeekSpec::Last(mode) => seek_model(model, MetricId(0), len - 1, mode),
    }
}

fn seek_model(
    model: &[char],
    id: MetricId,
    value: u32,
    mode: SeekMode,
) -> Option<(usize, Metrics<2>)> {
    match mode {
        SeekMode::After => model_seek(model, id, value),
        SeekMode::Before => model_seek_before(model, id, value),
    }
}

fn perform_seek(cursor: &mut Cursor<char, TextMeasure>, model: &[char], spec: SeekSpec) -> bool {
    let len = model.len() as u32;
    let total = metrics_of(model);
    match spec {
        SeekSpec::Index(raw) => cursor.seek_to_index(raw % (len + 2)),
        SeekSpec::Metric(lines, raw, mode) => {
            let id = MetricId(usize::from(lines));
            cursor.seek(id, raw % (total.metric_at(id) + 2), mode)
        }
        SeekSpec::End(mode) => cursor.seek(MetricId(0), len, mode),
        SeekSpec::Last(mode) => cursor.seek(MetricId(0), len.saturating_sub(1), mode),
    }
}

/// One step of the model walk: seek, check the landing, delete,
/// insert, mirror both into the model. Returns the model index the
/// edit happened at.
fn run_step(
    cursor: &mut Cursor<char, TextMeasure>,
    model: &mut Vec<char>,
    step: &Step,
) -> Result<(), TestCaseError> {
    let expected = model_landing(model, step.seek);
    let found = perform_seek(cursor, model, step.seek);
    prop_assert_eq!(
        found,
        expected.is_some(),
        "seek {:?} over {} elements reports {}",
        step.seek,
        model.len(),
        found
    );
    let index = match expected {
        Some((index, position)) => {
            prop_assert_eq!(
                *cursor.element(),
                model[index],
                "seek {:?} landed on the wrong element (expected index {})",
                step.seek,
                index
            );
            prop_assert_eq!(
                cursor.position(),
                position,
                "seek {:?} landed at the wrong position (expected index {})",
                step.seek,
                index
            );
            index
        }
        None => 0,
    };

    let deleted = (step.delete as usize).min(model.len().saturating_sub(index));
    cursor.delete(step.delete as u32);
    model.drain(index..index + deleted);
    if !step.insert.is_empty() {
        cursor.insert(rope_from_iter(step.insert.iter().copied()));
        model.splice(index..index, step.insert.iter().copied());
    }

    let rope = cursor.clone().rope();
    let expected_text: String = model.iter().collect();
    prop_assert_eq!(rope_to_string(&rope), expected_text);
    prop_assert_eq!(rope.metrics(), metrics_of(model));
    assert_cursor_well_balanced(cursor);
    Ok(())
}

fn walk(
    rope: Rope<char, TextMeasure>,
    mut model: Vec<char>,
    steps: &[Step],
) -> Result<(), TestCaseError> {
    let mut cursor = rope.cursor();
    for step in steps {
        run_step(&mut cursor, &mut model, step)?;
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// One cursor, many seek-delete-insert steps, each seek from
    /// wherever the previous edit left the cursor: every landing
    /// matches the linear model and the rope stays the model's
    /// contents and metrics. Default branching, so emptied trailing
    /// leaves need more than 64 elements — the content range allows it.
    #[test]
    fn a_cursor_walked_through_edits_matches_the_vec_model(
        content in content(),
        steps in steps(),
    ) {
        let rope = rope_from_iter(content.iter().copied());
        walk(rope, content, &steps)?;
    }

    /// The same walk over a rope built with tiny leaves and narrow
    /// branching: every tree shape — deep paths, emptied leaves,
    /// single-child chains — is a few elements away.
    #[test]
    fn a_cursor_walked_through_edits_matches_the_vec_model_on_a_deep_tree(
        // Below 37 elements: from 37 on, the layered builder itself
        // leaves a single-child chain (see
        // `the_layered_builder_leaves_no_single_child_chain`), which
        // would fail the balance check before any edit ran.
        content in prop::collection::vec(element(), 0..37),
        steps in steps(),
    ) {
        let rope = rope_from_iter_with_branching(content.iter().copied(), 4, 3);
        walk(rope, content, &steps)?;
    }

    /// A seek answers the same element whether it starts from the
    /// root of a freshly rebalanced rope or from a cursor standing
    /// wherever a run of edits left it in the unbalanced tree — and
    /// that element is the model's.
    #[test]
    fn a_seek_from_a_standing_cursor_agrees_with_a_seek_from_the_root(
        content in content(),
        steps in prop::collection::vec(step(), 0..30),
        probe in seek_spec(),
    ) {
        let mut model = content.clone();
        let mut standing = rope_from_iter(content.iter().copied()).cursor();
        for step in &steps {
            run_step(&mut standing, &mut model, step)?;
        }
        let expected = model_landing(&model, probe);
        let fresh_rope = standing.clone().rope();
        let mut from_root = fresh_rope.cursor();
        let root_found = perform_seek(&mut from_root, &model, probe);
        let standing_found = perform_seek(&mut standing, &model, probe);
        prop_assert_eq!(root_found, expected.is_some());
        prop_assert_eq!(standing_found, expected.is_some());
        if let Some((index, position)) = expected {
            prop_assert_eq!(*from_root.element(), model[index]);
            prop_assert_eq!(from_root.position(), position);
            prop_assert_eq!(*standing.element(), model[index]);
            prop_assert_eq!(standing.position(), position);
        }
    }
}

/// Hand-confirmed minimal cases the properties above shrank to,
/// kept as plain tests so each divergence has a name. They are RED
/// as of 2026-10-26; the parent decides the fixes.
mod shrunk_cases {
    use super::*;

    fn standing_and_root(
        content: &[char],
        stand_at: u32,
        id: MetricId,
        value: u32,
        mode: SeekMode,
    ) -> (bool, Metrics<2>, bool, Metrics<2>) {
        let mut standing = rope_from_iter(content.iter().copied()).cursor();
        assert!(standing.seek(MetricId(0), stand_at, SeekMode::After));
        let standing_found = standing.seek(id, value, mode);
        let mut root = rope_from_iter(content.iter().copied()).cursor();
        let root_found = root.seek(id, value, mode);
        (
            standing_found,
            standing.position(),
            root_found,
            root.position(),
        )
    }

    /// A seek on a metric the elements have zero width in (the
    /// newline count, for letters) from a cursor standing in a later
    /// leaf of the same run lands on that LEAF's first element; from
    /// the root it lands on the run's first element. The start
    /// shortcut compares metric locations only, so a run that spans
    /// leaves hides the earlier leaves.
    #[test]
    fn a_zero_width_metric_seek_from_a_later_leaf_rewinds_to_the_run_start() {
        let content: Vec<char> = std::iter::repeat('a').take(70).collect();
        for mode in [SeekMode::After, SeekMode::Before] {
            let (sf, sp, rf, rp) = standing_and_root(&content, 68, MetricId(1), 0, mode);
            assert!(sf && rf);
            assert_eq!(sp, rp, "{mode:?}: standing {sp:?} vs root {rp:?}");
        }
        let content: Vec<char> = std::iter::repeat('a')
            .take(30)
            .chain(std::iter::once('\n'))
            .chain(std::iter::repeat('a').take(70))
            .collect();
        let (sf, sp, rf, rp) = standing_and_root(&content, 99, MetricId(1), 1, SeekMode::After);
        assert!(sf && rf);
        assert_eq!(
            sp, rp,
            "line 1 across leaves: standing {sp:?} vs root {rp:?}"
        );
    }

    /// Deleting the last element empties the trailing leaf and leaves
    /// the cursor in it. A `Before` seek whose target metric is 0
    /// there (line 0 of a one-line rope) reports false: the climb
    /// guard reads the metric location, which is 0, and never climbs.
    #[test]
    fn a_before_seek_at_metric_zero_from_an_emptied_trailing_leaf_lands() {
        let mut cursor = rope_from_iter(std::iter::repeat('a').take(65)).cursor();
        assert!(cursor.seek(MetricId(0), 65, SeekMode::Before));
        cursor.delete(1u32);
        assert!(cursor.seek(MetricId(1), 0, SeekMode::Before));
        assert_eq!(cursor.position(), Metrics([0, 0]));
    }

    /// The same emptied trailing leaf, `After` mode, target the
    /// rope's end: from the root (after rebalance) the seek lands on
    /// the last element; standing in the empty leaf it reports false.
    #[test]
    fn an_after_seek_to_the_end_from_an_emptied_trailing_leaf_lands() {
        let mut cursor = rope_from_iter(std::iter::repeat('a').take(129)).cursor();
        assert!(cursor.seek(MetricId(0), 129, SeekMode::Before));
        cursor.delete(1u32);
        assert!(cursor.seek(MetricId(0), 128, SeekMode::After));
        assert_eq!(cursor.position(), Metrics([127, 0]));
    }

    /// The layered builder chunks each level greedily, so a level
    /// whose count is 1 modulo the branching factor ends in a
    /// single-child node, and two such levels in a row make the
    /// chain `assert_well_balanced` forbids. With the test
    /// branching (4, 3) that is 37 elements; with the production
    /// constants it is 65,600 elements (1025 full leaves).
    #[test]
    fn the_layered_builder_leaves_no_single_child_chain() {
        Rope::<char, TextMeasure>::from_iter_with_branching(std::iter::repeat('a').take(37), 4, 3)
            .assert_well_balanced();
        Rope::<char, TextMeasure>::from_iter(std::iter::repeat('a').take(65_600))
            .assert_well_balanced();
    }
}
