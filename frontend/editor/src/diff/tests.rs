// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use myersdiff::diff;

fn text(source: &str) -> Text {
    Text::from_string_exact(source)
}

fn apply(operation: &Operation, source: &str) -> String {
    let mut out = String::new();
    let mut at = 0usize;
    for op in operation.iter() {
        match op {
            Op::Retain(len) => {
                out.push_str(&source[at..at + len as usize]);
                at += len as usize;
            }
            Op::Delete(deleted) => {
                assert_eq!(
                    &source[at..at + deleted.len()],
                    deleted,
                    "the op deletes the bytes it claims"
                );
                at += deleted.len();
            }
            Op::Insert(inserted) => out.push_str(&inserted),
        }
    }
    assert_eq!(at, source.len(), "the op spans the whole left text");
    out
}

#[test]
fn diff_transforms_left_into_right() {
    let cases = [
        ("", ""),
        ("same\n", "same\n"),
        ("", "added\n"),
        ("removed\n", ""),
        ("a\nb\nc\n", "a\nB\nc\n"),
        (
            "# Title\n\nprose here\n",
            "# Title!\n\nmore prose here — with a dash\n",
        ),
        ("line one\nline two\nline three\n", "line one\nline three\n"),
        ("emoji 😀 line\n", "emoji 😀 line changed\n"),
        ("é\u{301}combining\n", "e\u{301}combining\n"),
        ("no trailing newline", "no trailing newline!"),
    ];
    for (left, right) in cases {
        let op = diff(&text(left), &text(right));
        assert_eq!(apply(&op, left), right, "{left:?} -> {right:?}");
    }
}

#[test]
fn diff_round_trips_scrambled_documents() {
    let mut seed = 0x9e3779b97f4a7c15u64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let words = [
        "alpha",
        "béta",
        "— dash",
        "😀",
        "line",
        "## header",
        "| a | b |",
    ];
    let build = |rand: &mut dyn FnMut() -> u64| {
        let mut out = String::new();
        for _ in 0..rand() % 40 {
            out.push_str(words[(rand() % 7) as usize]);
            if rand() % 3 == 0 {
                out.push('\n');
            } else {
                out.push(' ');
            }
        }
        out
    };
    for _ in 0..200 {
        let left = build(&mut rand);
        let right = build(&mut rand);
        let op = diff(&text(&left), &text(&right));
        assert_eq!(apply(&op, &left), right);
    }
}

#[test]
fn composed_edits_keep_the_diff_valid() {
    let left = "one\ntwo\nthree\n";
    let right = "one\nTWO\nthree\n";
    let mut op = diff(&text(left), &text(right));

    let edit = Operation::from_ops([Op::Retain(7), Op::Insert("!".to_owned()), Op::Retain(7)]);
    op = op.compose(&edit);
    assert_eq!(apply(&op, left), "one\nTWO!\nthree\n");

    let edit = Operation::from_ops([Op::Delete("one\n".to_owned()), Op::Retain(10)]);
    let new_left = "two\nthree\n";
    op = edit.invert().compose(&op);
    assert_eq!(apply(&op, new_left), "one\nTWO!\nthree\n");
}

#[test]
fn fragments_group_by_lines_and_classify() {
    let left = "unchanged\nold middle word\nbetween\ndeleted line\ntail\n";
    let right = "unchanged\nnew middle word\nbetween\ntail\nappended\n";
    let left_text = text(left);
    let op = diff(&left_text, &text(right));
    let fragments: Vec<_> = fragments_from(&op, &left_text, 0).collect();

    assert!(
        fragments.iter().any(|fragment| {
            fragment.kind == FragmentKind::Modified
                && fragment.words.iter().any(|(l, r)| {
                    &left[l.start as usize..l.end as usize] == "old"
                        && &right[r.start as usize..r.end as usize] == "new"
                })
        }),
        "the in-line rewrite is a Modified fragment pairing old→new: {fragments:?}"
    );
    assert!(
        fragments
            .iter()
            .any(|fragment| fragment.kind == FragmentKind::Deleted
                && left[fragment.left.start as usize..fragment.left.end as usize]
                    .contains("deleted line")),
        "the removed line is Deleted: {fragments:?}"
    );
    assert!(
        fragments
            .iter()
            .any(|fragment| fragment.kind == FragmentKind::Added && fragment.left.is_empty()),
        "the appended line is Added with an empty left range: {fragments:?}"
    );

    for fragment in &fragments {
        assert!(fragment.left.end as usize <= left.len());
        assert!(fragment.right.end as usize <= right.len());
        for (l, r) in &fragment.words {
            assert!(l.start >= fragment.left.start && l.end <= fragment.left.end);
            assert!(r.start >= fragment.right.start && r.end <= fragment.right.end);
        }
    }
}

#[test]
fn fragments_from_skips_earlier_changes() {
    let left = "a\nb\nc\nd\ne\n";
    let right = "A\nb\nc\nD\ne\n";
    let left_text = text(left);
    let op = diff(&left_text, &text(right));
    let all: Vec<_> = fragments_from(&op, &left_text, 0).collect();
    assert_eq!(all.len(), 2);
    let late: Vec<_> = fragments_from(&op, &left_text, 4).collect();
    assert_eq!(late.len(), 1, "only the change at/after byte 4");
    assert_eq!(late[0], all[1]);
}

#[test]
fn fragments_at_matches_the_full_walk() {
    let mut seed = 0xA24BAED4963EE407u64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let words = ["alpha", "béta", "😀", "line", "tail", "mid word", "x"];
    let build = |rand: &mut dyn FnMut() -> u64| {
        let mut out = String::new();
        for _ in 0..rand() % 30 {
            out.push_str(words[(rand() % 7) as usize]);
            if rand() % 3 == 0 {
                out.push('\n');
            } else {
                out.push(' ');
            }
        }
        out
    };
    let is_subsequence = |needle: &[Fragment], hay: &[Fragment]| {
        let mut at = 0usize;
        for fragment in needle {
            match hay[at..].iter().position(|candidate| candidate == fragment) {
                Some(found) => at += found + 1,
                None => return false,
            }
        }
        true
    };
    for _ in 0..120 {
        let left = build(&mut rand);
        let right = build(&mut rand);
        let left_text = text(&left);
        let op = diff(&left_text, &text(&right));
        let mut from_left = 0u32;
        loop {
            let full: Vec<Fragment> = fragments_from(&op, &left_text, from_left).collect();
            let seeded: Vec<Fragment> = fragments_at(&op, &left_text, from_left).collect();
            assert!(
                is_subsequence(&seeded, &full),
                "seeded walk invented fragments at {from_left}:\n  seeded {seeded:?}\n  full {full:?}"
            );
            for fragment in full
                .iter()
                .filter(|fragment| fragment.left.end >= from_left)
            {
                assert!(
                    seeded.contains(fragment),
                    "seeded walk lost {fragment:?} at {from_left}:\n  seeded {seeded:?}"
                );
            }
            if from_left >= left.len() as u32 + 2 {
                break;
            }
            from_left += 1 + (rand() % 5) as u32;
        }
    }
}

fn document_string(document: &crate::Document) -> String {
    let mut view = document.text().view();
    let count = view.byte_count();
    view.byte_string(0, count)
}

macro_rules! fx {
    () => {
        &mut imba::effect::Batch::new().effects()
    };
}

#[test]
fn the_edit_door_keeps_live_diffs_valid() {
    let fonts = crate::embedded_fonts::collection();
    let theme = crate::theme::Theme::embedded();
    let base = "one\ntwo\nthree\n";
    let mut target = crate::test_document::plain_document("one\nTWO\nthree\n");
    let operation = diff(&text(base), target.text());
    let id = target.add_diff(operation, 0);

    let edits = [
        Operation::insert_at(0, "head\n"),
        Operation::delete_at(9, "TWO"),
        Operation::from_ops([
            Op::Retain(5),
            Op::Delete("one".to_owned()),
            Op::Insert("ONE!".to_owned()),
        ]),
    ];
    for edit in edits {
        target.edit(&edit, &fonts, &theme, fx!());
        let live = target.diff(id).expect("the entry rides the document");
        assert_eq!(
            apply(live.operation(), base),
            document_string(&target),
            "the new side is current after every edit"
        );
        assert_eq!(
            live.base_revision(),
            0,
            "the old side's cursor is untouched"
        );
    }
}

#[test]
fn apply_base_edits_brings_the_old_side_current_idempotently() {
    let fonts = crate::embedded_fonts::collection();
    let theme = crate::theme::Theme::embedded();
    let mut base = crate::test_document::plain_document("one\ntwo\nthree\n");
    let mut target = crate::test_document::plain_document("one\nTWO\nthree\n");
    let operation = diff(base.text(), target.text());
    let id = target.add_diff(operation, base.revision());

    base.edit(&Operation::insert_at(4, "1.5\n"), &fonts, &theme, fx!());
    target.edit(&Operation::insert_at(0, "zero\n"), &fonts, &theme, fx!());
    base.edit(&Operation::delete_at(0, "one\n"), &fonts, &theme, fx!());

    assert!(target.apply_diff_base_edits(id, base.log()));
    let live = target.diff(id).expect("tracked");
    assert_eq!(live.base_revision(), base.revision());
    assert_eq!(
        apply(live.operation(), &document_string(&base)),
        document_string(&target)
    );

    let before = live.operation().clone();
    assert!(target.apply_diff_base_edits(id, base.log()));
    assert_eq!(target.diff(id).expect("tracked").operation(), &before);
}

#[test]
fn install_normalized_diff_bumps_the_generation_and_guards_lengths() {
    let base = crate::test_document::plain_document("a\nb\nc\n");
    let mut target = crate::test_document::plain_document("a\nB\nc\n");
    let operation = diff(base.text(), target.text());
    let id = target.add_diff(operation, base.revision());
    assert_eq!(target.diff(id).expect("tracked").generation(), 0);

    let minimal = diff(base.text(), target.text());
    assert!(target.install_normalized_diff(id, minimal, base.revision()));
    let live = target.diff(id).expect("tracked");
    assert_eq!(live.generation(), 1, "a landing bumps the generation");
    assert_eq!(
        apply(live.operation(), "a\nb\nc\n"),
        document_string(&target)
    );

    let stale = diff(&text("a\nb\nc\n"), &text("something else entirely"));
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        target.install_normalized_diff(id, stale, 0)
    }));
    match refused {
        Ok(landed) => assert!(!landed, "a mismatched landing must be refused"),
        Err(_) => {}
    }
    assert_eq!(
        target.diff(id).expect("tracked").generation(),
        1,
        "a refused landing changes nothing"
    );
}

#[test]
fn remove_diff_takes_its_markup_with_it() {
    let base = crate::test_document::plain_document("a\n");
    let mut target = crate::test_document::plain_document("b\n");
    let fonts = crate::embedded_fonts::collection();
    let theme = crate::theme::Theme::embedded();
    let operation = diff(base.text(), target.text());
    let id = target.add_diff(operation, 0);
    let markup = target.diff(id).expect("tracked").markup();
    assert!(target.feature_markup(markup).is_some());
    target.remove_diff(id, &[], &fonts, &theme, fx!());
    assert!(target.diff(id).is_none());
    assert!(
        target.feature_markup(markup).is_none(),
        "the wash markup leaves with the entry"
    );
}
