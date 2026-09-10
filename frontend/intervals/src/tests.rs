use std::collections::HashSet;

use crate::EditStep;
use crate::{Interval, IntervalQuery, Intervals, Order};

fn interval(key: u32, from: u32, to: u32) -> Interval<u32, &'static str> {
    Interval {
        range: from..to,
        greedy_left: false,
        greedy_right: false,
        key,
        value: "x",
    }
}

fn keys(intervals: &Intervals<u32, &'static str>, from: u32, to: u32) -> Vec<u32> {
    keys_order(intervals, from..to, Order::Ascending)
}

fn keys_order(
    intervals: &Intervals<u32, &'static str>,
    range: std::ops::Range<u32>,
    order: Order,
) -> Vec<u32> {
    intervals
        .query(range, order)
        .map(|interval| *interval.key)
        .collect()
}

fn interval_greedy(
    key: u32,
    from: u32,
    to: u32,
    greedy_left: bool,
    greedy_right: bool,
) -> Interval<u32, &'static str> {
    Interval {
        range: from..to,
        greedy_left,
        greedy_right,
        key,
        value: "x",
    }
}

fn interval_value(
    key: u32,
    from: u32,
    to: u32,
    greedy_left: bool,
    greedy_right: bool,
    value: &'static str,
) -> Interval<u32, &'static str> {
    Interval {
        range: from..to,
        greedy_left,
        greedy_right,
        key,
        value,
    }
}

fn large_intervals(count: u32) -> impl Iterator<Item = Interval<u32, &'static str>> {
    (0..count).map(|key| interval(key, key * 10, key * 10 + 4))
}

fn permuted_intervals(count: u32) -> Vec<Interval<u32, &'static str>> {
    (0..count)
        .map(|index| {
            let key = (index * 7_919) % count;
            let start = key * 4 + key % 3;
            interval(key, start, start + 1 + key % 31)
        })
        .collect()
}

fn assert_deep_tree(intervals: &Intervals<u32, &'static str>, len: usize) {
    assert!(
        3 <= intervals.open_tree_depth(),
        "test data must force at least three internal levels",
    );
    assert_eq!(intervals.open_tree_len(), len);
}

fn expected_keys(
    model: &[Interval<u32, &'static str>],
    range: std::ops::Range<u32>,
    order: Order,
) -> Vec<u32> {
    let mut hits: Vec<_> = model
        .iter()
        .filter(|interval| model_intersects(range.clone(), interval))
        .map(|interval| (interval.range.start, interval.key))
        .collect();

    match order {
        Order::Ascending => hits.sort_by_key(|(start, key)| (*start, *key)),
        Order::Descending => {
            hits.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)))
        }
    }

    hits.into_iter().map(|(_, key)| key).collect()
}

fn model_intersects(range: std::ops::Range<u32>, interval: &Interval<u32, &'static str>) -> bool {
    let query_start = i64::from(range.start) * 2;
    let query_end = i64::from(range.end) * 2;
    let start = i64::from(interval.range.start) * 2 - i64::from(interval.greedy_left);
    let end = i64::from(interval.range.end) * 2 + i64::from(interval.greedy_right);

    if query_start <= start {
        start <= query_end
    } else {
        query_start <= end
    }
}

fn assert_queries_match_model(
    intervals: &Intervals<u32, &'static str>,
    model: &[Interval<u32, &'static str>],
    queries: &[std::ops::Range<u32>],
) {
    for range in queries {
        assert_eq!(
            keys_order(intervals, range.clone(), Order::Ascending),
            expected_keys(model, range.clone(), Order::Ascending),
            "ascending query {range:?}",
        );
        assert_eq!(
            keys_order(intervals, range.clone(), Order::Descending),
            expected_keys(model, range.clone(), Order::Descending),
            "descending query {range:?}",
        );
    }
}

fn model_insert(model: &mut [Interval<u32, &'static str>], offset: u32, len: u32) {
    for interval in model {
        if interval.range.start < offset && offset < interval.range.end {
            interval.range.end += len;
        } else if offset <= interval.range.start {
            interval.range.start += len;
            interval.range.end += len;
        }
    }
}

fn model_delete(model: &mut [Interval<u32, &'static str>], offset: u32, len: u32) {
    for interval in model {
        if interval.range.end <= offset {
            continue;
        }

        let delete_end = offset + len;
        if delete_end <= interval.range.start {
            interval.range.start -= len;
            interval.range.end -= len;
            continue;
        }

        interval.range.start = if offset < interval.range.start {
            offset.max(interval.range.start - len)
        } else {
            interval.range.start
        };
        interval.range.end = offset.max(interval.range.end - len);
    }
}

fn all_ranges(intervals: &Intervals<u32, &'static str>) -> Vec<(u32, std::ops::Range<u32>)> {
    intervals
        .query(0..1_000, Order::Ascending)
        .map(|interval| (*interval.key, interval.range))
        .collect()
}

fn insert_at(offset: u32, text: &str) -> [EditStep; 2] {
    [
        EditStep::Retain(offset),
        EditStep::Insert(text.len() as u32),
    ]
}

fn delete_at(offset: u32, len: usize) -> [EditStep; 2] {
    [EditStep::Retain(offset), EditStep::Delete(len as u32)]
}

macro_rules! query_case_tests {
    ($(
        $name:ident {
            intervals: [$($interval:expr),* $(,)?],
            range: $range:expr,
            order: $order:expr,
            keys: [$($key:expr),* $(,)?],
        }
    )*) => {
        $(
            #[test]
            fn $name() {
                let mut intervals: Intervals<u32, &'static str> = Intervals::new();
                intervals.insert(vec![$($interval),*]);
                assert_eq!(
                    keys_order(&intervals, $range, $order),
                    vec![$($key),*],
                );
            }
        )*
    };
}

query_case_tests! {
    query_empty_ascending {
        intervals: [],
        range: 0..10,
        order: Order::Ascending,
        keys: [],
    }

    query_empty_descending {
        intervals: [],
        range: 0..10,
        order: Order::Descending,
        keys: [],
    }

    query_before_first_interval_is_empty {
        intervals: [interval(1, 10, 20)],
        range: 0..9,
        order: Order::Ascending,
        keys: [],
    }

    query_reaches_start_boundary {
        intervals: [interval(1, 10, 20)],
        range: 0..10,
        order: Order::Ascending,
        keys: [1],
    }

    query_inside_interval {
        intervals: [interval(1, 10, 20)],
        range: 12..13,
        order: Order::Ascending,
        keys: [1],
    }

    query_reaches_end_boundary {
        intervals: [interval(1, 10, 20)],
        range: 20..20,
        order: Order::Ascending,
        keys: [1],
    }

    query_after_interval_is_empty {
        intervals: [interval(1, 10, 20)],
        range: 21..22,
        order: Order::Ascending,
        keys: [],
    }

    query_covering_multiple_intervals {
        intervals: [interval(1, 10, 12), interval(2, 20, 22), interval(3, 30, 32)],
        range: 0..25,
        order: Order::Ascending,
        keys: [1, 2],
    }

    query_between_intervals_is_empty {
        intervals: [interval(1, 10, 12), interval(2, 20, 22)],
        range: 13..19,
        order: Order::Ascending,
        keys: [],
    }

    query_nested_intervals_ascending_start_order {
        intervals: [interval(1, 10, 50), interval(2, 20, 30), interval(3, 15, 45)],
        range: 25..26,
        order: Order::Ascending,
        keys: [1, 3, 2],
    }

    query_nested_intervals_descending_start_order {
        intervals: [interval(1, 10, 50), interval(2, 20, 30), interval(3, 15, 45)],
        range: 25..26,
        order: Order::Descending,
        keys: [2, 3, 1],
    }

    query_same_start_ascending_keeps_insert_order {
        intervals: [interval(1, 10, 20), interval(2, 10, 30), interval(3, 10, 15)],
        range: 11..12,
        order: Order::Ascending,
        keys: [1, 2, 3],
    }

    query_same_start_descending_reverses_insert_order {
        intervals: [interval(1, 10, 20), interval(2, 10, 30), interval(3, 10, 15)],
        range: 11..12,
        order: Order::Descending,
        keys: [3, 2, 1],
    }

    query_zero_length_interval_at_point {
        intervals: [interval(1, 10, 10)],
        range: 10..10,
        order: Order::Ascending,
        keys: [1],
    }

    query_zero_length_interval_outside_point_is_empty {
        intervals: [interval(1, 10, 10)],
        range: 11..11,
        order: Order::Ascending,
        keys: [],
    }

    query_open_interval_wins_same_decoded_start_tie {
        intervals: [interval(1, 10, 20), interval_greedy(2, 10, 20, true, false)],
        range: 10..11,
        order: Order::Ascending,
        keys: [1, 2],
    }
}

#[test]
fn query_crosses_root_split() {
    let mut intervals = Intervals::new();
    intervals.insert((0..96).map(|key| interval(key, key * 10, key * 10 + 4)));

    assert_eq!(keys(&intervals, 451, 452), vec![45]);
}

#[test]
fn query_reversed_uses_descending_start_order() {
    let mut intervals = Intervals::new();
    intervals.insert([
        interval(1, 10, 30),
        interval(2, 20, 40),
        interval(3, 30, 50),
    ]);

    let keys: Vec<u32> = intervals
        .query(25..35, Order::Descending)
        .map(|interval| *interval.key)
        .collect();
    assert_eq!(keys, vec![3, 2, 1]);
}

#[test]
fn remove_deletes_by_key() {
    let mut intervals = Intervals::new();
    intervals.insert((0..40).map(|key| interval(key, key * 10, key * 10 + 4)));

    intervals.remove([&17, &18]);

    assert_eq!(keys(&intervals, 171, 172), Vec::<u32>::new());
    assert_eq!(keys(&intervals, 191, 192), vec![19]);
}

#[test]
fn edit_expands_and_collapses_lazily() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20), interval(2, 30, 40)]);

    intervals.edit([
        EditStep::Retain(15),
        EditStep::Insert(3),
        EditStep::Retain(25),
    ]);
    assert_eq!(keys(&intervals, 21, 22), vec![1]);
    assert_eq!(keys(&intervals, 34, 35), vec![2]);

    intervals.edit([
        EditStep::Retain(12),
        EditStep::Delete(5),
        EditStep::Retain(31),
    ]);
    let ranges: Vec<_> = intervals
        .query(10..40, Order::Ascending)
        .map(|interval| (interval.key, interval.range))
        .collect();
    assert_eq!(ranges, vec![(&1, 10..18), (&2, 28..38)]);
}

#[test]
fn clone_is_cheap_and_mutations_are_isolated() {
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Payload(&'static str);

    let mut intervals = Intervals::new();
    intervals.insert([Interval {
        range: 10..20,
        greedy_left: false,
        greedy_right: false,
        key: 1,
        value: Payload("one"),
    }]);

    let snapshot = intervals.clone();
    intervals.insert([Interval {
        range: 30..40,
        greedy_left: false,
        greedy_right: false,
        key: 2,
        value: Payload("two"),
    }]);

    let snapshot_keys: Vec<u32> = snapshot
        .query(0..100, Order::Ascending)
        .map(|interval| *interval.key)
        .collect();
    let current_keys: Vec<u32> = intervals
        .query(0..100, Order::Ascending)
        .map(|interval| *interval.key)
        .collect();

    assert_eq!(snapshot_keys, vec![1]);
    assert_eq!(current_keys, vec![1, 2]);
}

#[test]
fn large_tree_queries_first_leaf() {
    let mut intervals = Intervals::new();
    intervals.insert(large_intervals(200));

    assert_eq!(keys(&intervals, 0, 1), vec![0]);
}

#[test]
fn large_tree_queries_middle_leaf() {
    let mut intervals = Intervals::new();
    intervals.insert(large_intervals(200));

    assert_eq!(keys(&intervals, 999, 1_000), vec![100]);
}

#[test]
fn large_tree_queries_last_leaf() {
    let mut intervals = Intervals::new();
    intervals.insert(large_intervals(200));

    assert_eq!(keys(&intervals, 1_991, 1_992), vec![199]);
}

#[test]
fn find_by_id_in_split_tree() {
    let mut intervals = Intervals::new();
    intervals.insert(large_intervals(200));

    let found = intervals.find_by_id(&137).expect("interval in split tree");

    assert_eq!(found.range, 1_370..1_374);
}

#[test]
fn large_tree_descending_window_crosses_children() {
    let mut intervals = Intervals::new();
    intervals.insert(large_intervals(200));

    assert_eq!(
        keys_order(&intervals, 100..150, Order::Descending),
        vec![15, 14, 13, 12, 11, 10],
    );
}

#[test]
fn large_tree_same_start_descending_crosses_splits() {
    let mut intervals = Intervals::new();
    intervals.insert((0..96).map(|key| interval(key, 10, 20)));

    let expected: Vec<_> = (0..96).rev().collect();
    assert_eq!(keys_order(&intervals, 11..12, Order::Descending), expected);
}

#[test]
fn remove_missing_key_keeps_intervals() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20), interval(2, 30, 40)]);

    intervals.remove([&9]);

    assert_eq!(keys(&intervals, 0, 50), vec![1, 2]);
}

#[test]
fn remove_first_from_split_tree() {
    let mut intervals = Intervals::new();
    intervals.insert(large_intervals(80));

    intervals.remove([&0]);

    assert_eq!(keys(&intervals, 0, 1), Vec::<u32>::new());
    assert_eq!(keys(&intervals, 10, 11), vec![1]);
}

#[test]
fn remove_last_from_split_tree() {
    let mut intervals = Intervals::new();
    intervals.insert(large_intervals(80));

    intervals.remove([&79]);

    assert_eq!(keys(&intervals, 791, 792), Vec::<u32>::new());
    assert_eq!(keys(&intervals, 781, 782), vec![78]);
}

#[test]
fn remove_middle_from_split_tree() {
    let mut intervals = Intervals::new();
    intervals.insert(large_intervals(80));

    intervals.remove([&41]);

    assert_eq!(keys(&intervals, 411, 412), Vec::<u32>::new());
    assert_eq!(keys(&intervals, 401, 402), vec![40]);
    assert_eq!(keys(&intervals, 421, 422), vec![42]);
}

#[test]
fn remove_many_from_split_tree() {
    let mut intervals = Intervals::new();
    intervals.insert(large_intervals(80));
    let removed: Vec<_> = (0..80).step_by(3).collect();

    intervals.remove(removed.iter());

    let expected: Vec<_> = (0..80).filter(|key| key % 3 != 0).collect();
    assert_eq!(keys(&intervals, 0, 800), expected);
}

#[test]
fn remove_all_from_split_tree() {
    let mut intervals = Intervals::new();
    intervals.insert(large_intervals(80));
    let removed: Vec<_> = (0..80).collect();

    intervals.remove(removed.iter());

    assert_eq!(keys(&intervals, 0, 800), Vec::<u32>::new());
}

#[test]
fn inserting_same_key_replaces_old_interval() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);
    intervals.insert([interval(1, 30, 40)]);

    assert_eq!(keys(&intervals, 10, 11), Vec::<u32>::new());
    assert_eq!(keys(&intervals, 30, 31), vec![1]);
}

#[test]
fn find_by_id_returns_interval() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20), interval(2, 30, 40)]);

    let found = intervals.find_by_id(&2).expect("interval by id");

    assert_eq!(*found.key, 2);
    assert_eq!(found.range, 30..40);
    assert_eq!(*found.value, "x");
}

#[test]
fn find_by_id_returns_replacement() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);
    intervals.insert([interval(1, 30, 40)]);

    let found = intervals.find_by_id(&1).expect("replacement by id");

    assert_eq!(found.range, 30..40);
}

#[test]
fn find_by_id_returns_none_for_missing_id() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    assert!(intervals.find_by_id(&9).is_none());
}

#[test]
fn inserting_duplicate_keys_in_same_batch_keeps_last() {
    let mut intervals = Intervals::new();

    intervals.insert([
        interval(1, 10, 20),
        interval(2, 15, 25),
        interval(1, 30, 40),
    ]);

    assert_eq!(keys(&intervals, 10, 11), Vec::<u32>::new());
    assert_eq!(keys(&intervals, 15, 16), vec![2]);
    assert_eq!(keys(&intervals, 30, 31), vec![1]);
}

#[test]
fn remove_then_reinsert_same_key() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    intervals.remove([&1]);
    intervals.insert([interval(1, 40, 50)]);

    assert_eq!(keys(&intervals, 10, 11), Vec::<u32>::new());
    assert_eq!(keys(&intervals, 40, 41), vec![1]);
}

#[test]
fn remove_from_current_does_not_mutate_snapshot() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20), interval(2, 30, 40)]);
    let snapshot = intervals.clone();

    intervals.remove([&1]);

    assert_eq!(keys(&snapshot, 0, 50), vec![1, 2]);
    assert_eq!(keys(&intervals, 0, 50), vec![2]);
}

#[test]
fn edit_insert_before_interval_shifts_it() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    intervals.edit(insert_at(5, "abc"));

    assert_eq!(all_ranges(&intervals), vec![(1, 13..23)]);
}

#[test]
fn edit_insert_after_interval_keeps_it() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    intervals.edit(insert_at(25, "abc"));

    assert_eq!(all_ranges(&intervals), vec![(1, 10..20)]);
}

#[test]
fn edit_insert_inside_interval_expands_it() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    intervals.edit(insert_at(15, "abc"));

    assert_eq!(all_ranges(&intervals), vec![(1, 10..23)]);
}

#[test]
fn edit_insert_at_open_start_shifts_interval() {
    let mut intervals = Intervals::new();
    intervals.insert([interval_greedy(1, 10, 20, false, false)]);

    intervals.edit(insert_at(10, "abc"));

    assert_eq!(all_ranges(&intervals), vec![(1, 13..23)]);
}

#[test]
fn edit_insert_at_greedy_left_start_expands_interval() {
    let mut intervals = Intervals::new();
    intervals.insert([interval_greedy(1, 10, 20, true, false)]);

    intervals.edit(insert_at(10, "abc"));

    assert_eq!(all_ranges(&intervals), vec![(1, 10..23)]);
}

#[test]
fn edit_insert_at_open_end_keeps_interval() {
    let mut intervals = Intervals::new();
    intervals.insert([interval_greedy(1, 10, 20, false, false)]);

    intervals.edit(insert_at(20, "abc"));

    assert_eq!(all_ranges(&intervals), vec![(1, 10..20)]);
}

#[test]
fn edit_insert_at_greedy_right_end_expands_interval() {
    let mut intervals = Intervals::new();
    intervals.insert([interval_greedy(1, 10, 20, false, true)]);

    intervals.edit(insert_at(20, "abc"));

    assert_eq!(all_ranges(&intervals), vec![(1, 10..23)]);
}

#[test]
fn edit_insert_counts_unicode_bytes() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    intervals.edit(insert_at(5, "åß"));

    assert_eq!(all_ranges(&intervals), vec![(1, 14..24)]);
}

#[test]
fn edit_delete_before_interval_shifts_left() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    intervals.edit(delete_at(5, 3));

    assert_eq!(all_ranges(&intervals), vec![(1, 7..17)]);
}

#[test]
fn edit_delete_after_interval_keeps_it() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    intervals.edit(delete_at(25, 3));

    assert_eq!(all_ranges(&intervals), vec![(1, 10..20)]);
}

#[test]
fn edit_delete_inside_interval_shrinks_it() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    intervals.edit(delete_at(12, 3));

    assert_eq!(all_ranges(&intervals), vec![(1, 10..17)]);
}

#[test]
fn edit_delete_from_interval_start_shrinks_end() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    intervals.edit(delete_at(10, 3));

    assert_eq!(all_ranges(&intervals), vec![(1, 10..17)]);
}

#[test]
fn edit_delete_across_start_boundary_moves_start_to_delete_start() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    intervals.edit(delete_at(5, 10));

    assert_eq!(all_ranges(&intervals), vec![(1, 5..10)]);
}

#[test]
fn edit_delete_across_end_boundary_moves_end_to_delete_start() {
    let mut intervals = Intervals::new();
    intervals.insert([interval(1, 10, 20)]);

    intervals.edit(delete_at(15, 10));

    assert_eq!(all_ranges(&intervals), vec![(1, 10..15)]);
}

#[test]
fn query_preserves_greedy_flags_and_value() {
    let mut intervals = Intervals::new();
    intervals.insert([interval_value(1, 10, 20, true, true, "payload")]);

    let records: Vec<_> = intervals
        .query(11..12, Order::Ascending)
        .map(|interval| {
            (
                *interval.key,
                interval.range,
                interval.greedy_left,
                interval.greedy_right,
                *interval.value,
            )
        })
        .collect();

    assert_eq!(records, vec![(1, 10..20, true, true, "payload")]);
}

#[test]
fn interval_ref_cloned_copies_data() {
    let mut intervals = Intervals::new();
    intervals.insert([interval_value(1, 10, 20, true, false, "payload")]);

    let cloned = intervals
        .query(11..12, Order::Ascending)
        .next()
        .expect("interval")
        .cloned();

    assert_eq!(cloned, interval_value(1, 10, 20, true, false, "payload"),);
}

#[test]
fn deep_tree_build_reaches_multiple_internal_levels() {
    let model = permuted_intervals(50_000);
    let mut intervals = Intervals::new();

    intervals.insert(model);

    assert_deep_tree(&intervals, 50_000);
}

#[test]
fn deep_tree_queries_match_flat_model() {
    let model = permuted_intervals(50_000);
    let mut intervals = Intervals::new();
    intervals.insert(model.clone());
    assert_deep_tree(&intervals, 50_000);

    assert_queries_match_model(
        &intervals,
        &model,
        &[
            0..1,
            17..211,
            2_047..2_349,
            16_383..16_911,
            65_537..66_913,
            131_071..132_549,
            199_000..200_200,
        ],
    );
}

#[test]
fn deep_tree_full_range_descending_matches_flat_model() {
    let model = permuted_intervals(40_000);
    let mut intervals = Intervals::new();
    intervals.insert(model.clone());
    assert_deep_tree(&intervals, 40_000);

    assert_eq!(
        keys_order(&intervals, 0..200_000, Order::Descending),
        expected_keys(&model, 0..200_000, Order::Descending),
    );
}

#[test]
fn deep_tree_removals_match_flat_model() {
    let mut model = permuted_intervals(50_000);
    let mut intervals = Intervals::new();
    intervals.insert(model.clone());
    let removed: HashSet<_> = (0..50_000).filter(|key| key % 7 == 0).collect();
    let removed_keys: Vec<_> = removed.iter().copied().collect();

    intervals.remove(removed_keys.iter());
    model.retain(|interval| !removed.contains(&interval.key));
    assert_deep_tree(&intervals, model.len());

    assert_queries_match_model(
        &intervals,
        &model,
        &[
            0..500,
            9_000..10_000,
            42_000..44_000,
            99_000..101_000,
            150_000..152_000,
            198_000..200_000,
        ],
    );
}

#[test]
fn deep_tree_replacements_match_flat_model() {
    let mut model = permuted_intervals(50_000);
    let mut intervals = Intervals::new();
    intervals.insert(model.clone());

    let replacements: Vec<_> = (0..50_000)
        .step_by(13)
        .map(|key| {
            let start = 250_000 + key * 3;
            interval(key, start, start + 17)
        })
        .collect();
    let replacement_keys: HashSet<_> = replacements.iter().map(|interval| interval.key).collect();

    intervals.insert(replacements.clone());
    model.retain(|interval| !replacement_keys.contains(&interval.key));
    model.extend(replacements);

    for key in (0..50_000).step_by(13) {
        let start = 250_000 + key * 3;
        assert_eq!(keys(&intervals, start, start + 1), vec![key]);
    }
    assert_deep_tree(&intervals, model.len());

    assert_queries_match_model(
        &intervals,
        &model,
        &[
            0..2_000,
            45_000..47_000,
            110_000..112_000,
            250_000..252_500,
            300_000..303_000,
            337_000..340_000,
        ],
    );
}

#[test]
fn deep_tree_insert_edit_matches_flat_model() {
    let mut model = permuted_intervals(40_000);
    let mut intervals = Intervals::new();
    intervals.insert(model.clone());

    intervals.edit(insert_at(60_000, "abcdefghijklmnopqrstuvwxyz"));
    model_insert(&mut model, 60_000, 26);
    assert_deep_tree(&intervals, model.len());

    assert_queries_match_model(
        &intervals,
        &model,
        &[
            59_900..60_100,
            60_100..60_500,
            80_000..81_000,
            120_000..121_000,
            159_000..160_500,
        ],
    );
}

#[test]
fn deep_tree_delete_edit_matches_flat_model() {
    let mut model = permuted_intervals(40_000);

    let mut intervals = Intervals::keeping_empties();
    intervals.insert(model.clone());

    intervals.edit(delete_at(60_000, 123));
    model_delete(&mut model, 60_000, 123);
    assert_deep_tree(&intervals, model.len());

    assert_queries_match_model(
        &intervals,
        &model,
        &[
            59_700..60_100,
            60_100..60_700,
            80_000..81_000,
            120_000..121_000,
            158_000..160_000,
        ],
    );
}

#[test]
fn bulk_insert_into_nonempty_matches_singles() {
    let mut rand = {
        let mut seed = 0x1b5e_c7ed_u64;
        move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        }
    };
    for round in 0..200 {
        let mut bulk = Intervals::new();
        let seeded = (rand() % 60 + 1) as u32;
        for key in 0..seeded {
            let from = (rand() % 10_000) as u32;
            let len = (rand() % 90 + 1) as u32;
            bulk.insert([interval(key, from, from + len)]);
        }
        let mut singles = bulk.clone();

        let batch: Vec<_> = (0..(rand() % 30 + 2) as u32)
            .map(|extra| {
                let from = (rand() % 10_000) as u32;
                let len = (rand() % 90 + 1) as u32;
                interval(1_000 + extra, from, from + len)
            })
            .collect();
        bulk.insert(batch.clone());
        for entry in &batch {
            singles.insert([entry.clone()]);
        }
        assert_eq!(
            keys_order(&bulk, 0..u32::MAX, Order::Ascending),
            keys_order(&singles, 0..u32::MAX, Order::Ascending),
            "round {round}"
        );
        assert_eq!(bulk.len(), singles.len(), "round {round}");
        for entry in &batch {
            let found = bulk.find_by_id(&entry.key).expect("bulk key addressable");
            assert_eq!(found.range, entry.range, "round {round}");
        }

        let victims: Vec<u32> = batch.iter().map(|entry| entry.key).step_by(2).collect();
        let mut bulk_removed = bulk.clone();
        bulk_removed.remove(victims.iter());
        let mut single_removed = bulk.clone();
        for victim in &victims {
            single_removed.remove([victim]);
        }
        assert_eq!(
            keys_order(&bulk_removed, 0..u32::MAX, Order::Ascending),
            keys_order(&single_removed, 0..u32::MAX, Order::Ascending),
            "round {round}"
        );
        for victim in &victims {
            assert!(bulk_removed.find_by_id(victim).is_none(), "round {round}");
        }
    }
}

#[test]
fn graft_into_nonempty_matches_singles() {
    let mut target = Intervals::new();
    for key in 0..50u32 {
        target.insert([interval(key, key * 100, key * 100 + 40)]);
    }
    let mut local = Intervals::new();
    for key in 0..7u32 {
        local.insert([interval(500 + key, key * 10, key * 10 + 5)]);
    }
    let mut singles = target.clone();
    target.graft(2_345, &local);
    for entry in local.query(0..u32::MAX, Order::Ascending) {
        let mut entry = entry.cloned();
        entry.range.start += 2_345;
        entry.range.end += 2_345;
        singles.insert([entry]);
    }
    assert_eq!(
        keys_order(&target, 0..u32::MAX, Order::Ascending),
        keys_order(&singles, 0..u32::MAX, Order::Ascending),
    );
    for key in 500..507u32 {
        assert_eq!(
            target.find_by_id(&key).expect("grafted key").range,
            singles.find_by_id(&key).expect("grafted key").range,
        );
    }
}

#[test]
fn a_collapse_drops_the_markers_it_empties() {
    let mut intervals = Intervals::new();
    intervals.insert([
        interval(1, 10, 20),
        interval(2, 30, 40),
        interval(3, 100, 110),
    ]);
    assert_eq!(intervals.len(), 3);

    intervals.edit(delete_at(0, 50));
    assert_eq!(intervals.len(), 1, "the emptied markers left the set");
    assert!(intervals.find_by_id(&1).is_none());
    assert!(intervals.find_by_id(&2).is_none());
    assert_eq!(
        intervals.find_by_id(&3).map(|found| found.range.clone()),
        Some(50..60),
        "the untouched marker shifted, nothing else"
    );

    let mut deep = Intervals::new();
    deep.insert(large_intervals(40_000));
    assert_eq!(deep.len(), 40_000);
    deep.edit(delete_at(0, 400_000));
    assert_eq!(deep.len(), 0, "the key map emptied with the tree");
    assert_eq!(
        deep.query(0..u32::MAX, Order::Ascending).count(),
        0,
        "and the tree itself is empty"
    );

    let mut anchors = Intervals::new();
    anchors.insert([interval(7, 5, 5)]);
    anchors.edit(delete_at(0, 2));
    assert_eq!(
        anchors.find_by_id(&7).map(|found| found.range.clone()),
        Some(3..3),
        "the anchor rode the edit, it was not collapsed by it"
    );
}
