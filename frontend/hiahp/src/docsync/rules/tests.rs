use super::*;
use documents::sync::SyncState;
use operation::OperationBuilder;

fn insert(at: usize, text: &str, len: usize) -> Operation {
    let mut builder = OperationBuilder::new();
    if at > 0 {
        builder.push_retain(at as u32);
    }
    builder.push_insert(text.to_owned());
    if len > at {
        builder.push_retain((len - at) as u32);
    }
    builder.finish()
}

fn state(source: &str) -> SyncState {
    SyncState::new(himark::Text::from_string_exact(source), EditLog::new())
}

#[test]
fn the_seam_sends_what_the_reader_typed() {
    let mut log = EditLog::new();
    let identity = log.record(&insert(0, "hello", 0), 0);
    let edit = seam_edit(&log, 0, None).expect("a user edit goes out");
    assert_eq!(edit.identity(), Some(identity), "under the entry's own name");
}

#[test]
fn the_seam_swallows_our_own_offer_landing() {
    let mut log = EditLog::new();
    let identity = log.record(&insert(0, "hello", 0), 0);
    assert!(seam_edit(&log, 0, Some(identity)).is_none());
}

#[test]
fn sent_is_derived_not_counted() {
    assert_eq!(sent(7, 3, 1), 3, "seven entries, three before we attached, one ours");
    assert_eq!(sent(3, 3, 0), 0, "nothing since attach");
    assert_eq!(sent(2, 3, 0), 0, "a document that shrank cannot owe us edits");
}

#[test]
fn an_offer_that_predates_a_keystroke_is_dropped() {
    let mut document = EditLog::new();
    document.record(&insert(0, "hello", 0), 0);
    let mut offered = state("hello");
    offered.log = document.clone();
    let identity = offered.log.record(&insert(5, "!", 5), 5);
    offered.text = himark::Text::from_string_exact("hello!");

    let fits = Offer {
        seen_local: 1,
        state: offered.clone(),
    };
    let (recorded, slice) = offer_landing(&document, 1, 0, 0, 5, &fits).expect("it fits");
    assert_eq!(recorded, identity, "recorded as the loop names it");
    assert!(!is_identity(&slice));

    let stale = Offer {
        seen_local: 0,
        state: offered,
    };
    assert!(offer_landing(&document, 1, 0, 0, 5, &stale).is_none());
}

#[test]
fn a_slice_that_misses_the_text_is_dropped() {
    let mut document = EditLog::new();
    document.record(&insert(0, "hello", 0), 0);
    let mut offered = state("hello");
    offered.log = document.clone();
    offered.log.record(&insert(5, "!", 5), 5);
    offered.text = himark::Text::from_string_exact("hello!");
    let offer = Offer {
        seen_local: 1,
        state: offered,
    };
    assert!(
        offer_landing(&document, 1, 0, 0, 999, &offer).is_none(),
        "the document is not the length the slice expects"
    );
}

#[test]
fn an_offer_of_the_same_state_lands_as_nothing() {
    let mut document = EditLog::new();
    document.record(&insert(0, "hello", 0), 0);
    let offer = Offer {
        seen_local: 1,
        state: SyncState::new(himark::Text::from_string_exact("hello"), document.clone()),
    };
    assert!(offer_landing(&document, 1, 0, 0, 5, &offer).is_none());
}
