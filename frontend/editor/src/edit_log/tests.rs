// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;


#[test]
fn revisions_are_positions_in_the_log() {
    let mut log = EditLog::new();
    assert_eq!(log.revision(), 0);
    log.record(&Operation::insert_in(0, 0, "ab"), 0);
    log.record(&Operation::insert_in(2, 2, "c"), 2);
    assert_eq!(log.revision(), 2);
    assert_eq!(log.since(1).count(), 1);
    assert_eq!(log.since(5).count(), 0);
}

#[test]
fn composition_chains_padded_entries() {
    let mut log = EditLog::new();
    log.record(&Operation::insert_in(0, 0, "hello"), 0);
    log.record(&Operation::insert_in(5, 5, " world"), 5);
    let composed = log.compose_since(0).expect("two entries");
    assert_eq!(composed.old_len(), 0);
    assert_eq!(composed.new_len(), 11);
    assert!(log.compose_since(2).is_none());
}

#[test]
fn ranges_since_land_in_current_coordinates() {
    let mut log = EditLog::new();

    log.record(&Operation::insert_in(6, 3, "XY"), 6);
    log.record(&Operation::insert_in(8, 0, "__"), 8);
    let ranges = log.ranges_since(0);

    assert!(ranges.contains(&(0..2)));
    assert!(ranges.contains(&(5..7)));
}

#[test]
fn range_ends_extend_conservatively() {
    let range = EditLog::transform_range(2..5, &Operation::insert_in(5, 5, "!!"));
    assert_eq!(range, 2..7);

    let range = EditLog::transform_range(2..5, &Operation::insert_in(5, 2, "!!"));
    assert_eq!(range, 2..7);
}

fn text_of(source: &str, log: &EditLog, from: u64) -> String {
    let text = text::Text::from_string_exact(source);
    let edited = match log.compose_since(from) {
        Some(operation) => text.edit(&operation),
        None => text,
    };
    let end = edited.byte_count() as u32;
    edited.view().substring(0..end)
}

#[test]
fn a_shared_edit_carries_one_identity() {
    let mut mine = EditLog::new();
    let identity = mine.record(&Operation::insert_in(0, 0, "hello"), 0);
    let mut theirs = EditLog::new();
    theirs.record_as(identity, &Operation::insert_in(0, 0, "hello"), 0);

    assert_eq!(mine.head(), theirs.head());
    assert_eq!(common_base(&mine, &theirs), Some((0, 0)));
    assert!(
        bridge(&mine, &theirs).expect("one history").is_empty(),
        "one history: nothing to carry"
    );
}

#[test]
fn the_bridge_undoes_mine_and_redoes_theirs() {
    let mut common = EditLog::new();
    common.record(&Operation::insert_in(0, 0, "hello"), 0);

    let mut mine = common.clone();
    mine.record(&Operation::insert_in(5, 5, "!"), 5);
    let mut theirs = common.clone();
    theirs.record(&Operation::insert_in(5, 0, ">> "), 5);

    assert_eq!(
        common_base(&mine, &theirs),
        Some((0, 0)),
        "the shared prefix"
    );
    let arrow = bridge(&mine, &theirs).expect("one history");
    assert_eq!(
        text_of("", &mine, 0),
        "hello!",
        "the state my log describes"
    );
    let carried = text::Text::from_string_exact("hello!").edit(&arrow);
    let end = carried.byte_count() as u32;
    assert_eq!(
        carried.view().substring(0..end),
        ">> hello",
        "the arrow lands on the state their log describes"
    );

    let late = Operation::insert_in(6, 6, "?");
    let placed = late.transform(&arrow);
    let landed = text::Text::from_string_exact(">> hello").edit(&placed);
    let end = landed.byte_count() as u32;
    assert_eq!(landed.view().substring(0..end), ">> hello?");
}

#[test]
fn strangers_bridge_across_everything() {
    let mut mine = EditLog::new();
    mine.record(&Operation::insert_in(0, 0, "mine"), 0);
    let mut theirs = EditLog::new();
    theirs.record(&Operation::insert_in(0, 0, "theirs"), 0);
    assert_eq!(common_base(&mine, &theirs), None);

    let arrow = bridge(&mine, &theirs).expect("their histories meet end to end");
    let landed = text::Text::from_string_exact("mine").edit(&arrow);
    let end = landed.byte_count() as u32;
    assert_eq!(landed.view().substring(0..end), "theirs");
}
