use super::*;

#[test]
fn normalize_sorts_and_merges_overlaps() {
    let carets = MultiCaret::normalized(
        vec![
            Caret::selecting(10, 20),
            Caret::at(5),
            Caret::selecting(15, 30),
            Caret::at(50),
        ],
        2,
    );
    let all = carets.carets();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0], Caret::at(5));
    assert_eq!(all[1].selection(), 10..30);
    assert_eq!(all[2], Caret::at(50));
    assert_eq!(
        carets.primary().selection(),
        10..30,
        "primary follows its merge survivor"
    );
}

#[test]
fn equal_plain_carets_merge() {
    let carets = MultiCaret::normalized(vec![Caret::at(7), Caret::at(7), Caret::at(9)], 0);
    assert_eq!(carets.len(), 2);
}

#[test]
fn a_caret_inside_a_selection_is_absorbed() {
    let carets = MultiCaret::normalized(vec![Caret::selecting(0, 10), Caret::at(5)], 0);
    assert_eq!(carets.len(), 1);
    assert_eq!(carets.primary().selection(), 0..10);
}

#[test]
fn removal_refuses_the_last_caret() {
    let carets = MultiCaret::single(3);
    assert!(carets.with_removed_at(3).is_none());
    let two = carets.with_added(Caret::at(8));
    let one = two.with_removed_at(8).expect("removable");
    assert_eq!(one.carets(), &[Caret::at(3)]);
}

#[test]
fn transform_rides_an_insert_before_both_ends() {
    let operation = Operation::insert_at(2, "xx");
    let carets = MultiCaret::one(Caret::selecting(4, 8)).transformed(&operation);
    assert_eq!(carets.primary().selection(), 6..10);
    assert_eq!(carets.primary().offset(), 10);
}

#[test]
fn moved_to_pivots_around_the_anchor() {
    let caret = Caret::selecting(10, 20);
    let back = caret.moved_to(5, true);
    assert_eq!(back.selection(), 5..10);
    assert_eq!(back.offset(), 5);
    assert_eq!(back.anchor(), 10);
    assert_eq!(caret.moved_to(5, false), Caret::at(5));
}
