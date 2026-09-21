// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::document::Document;
use crate::test_document::{plain_document, test_fonts};

fn fonts() -> skia_safe::textlayout::FontCollection {
    test_fonts()()
}

fn theme() -> Theme {
    Theme::embedded()
}

fn hundred_lines() -> String {
    (0..100).fold(String::new(), |mut text, index| {
        use std::fmt::Write;
        let _ = writeln!(text, "line {index:03}");
        text
    })
}

fn pane(document: &mut Document) -> crate::editor::EditorId {
    let store = &imba::store::Store::new();
    let ui = crate::test_document::test_ui();
    let editor = document.add_editor(
        400.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        store,
        ui,
        &fonts(),
        &theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    document.enable_scroll_stripes(editor);
    editor
}

fn tinted(
    document: &mut Document,
    editor: crate::editor::EditorId,
    ranges: &[Range<u32>],
    style: StyleId,
) -> crate::MarkupId {
    let id = document.add_markup();
    document.show_markup(editor, id);
    document.mark_scroll_stripes(editor, id);
    retint(document, id, ranges, style);
    id
}

fn retint(document: &mut Document, id: crate::MarkupId, ranges: &[Range<u32>], style: StyleId) {
    let store = &imba::store::Store::new();
    let ui = crate::test_document::test_ui();
    let mut tints = Markup::new();
    for range in ranges {
        tints.push_styled(range.clone(), style);
    }
    document.replace_markup(
        id,
        tints,
        &[],
        store,
        ui,
        &fonts(),
        &theme(),
        &mut imba::effect::Batch::new().effects(),
    );
}

fn styled_hits(markup: &Markup) -> Vec<(Range<u32>, StyleId)> {
    use intervals::{IntervalQuery, Order};
    markup
        .query(0..u32::MAX, Order::Ascending)
        .filter_map(|hit| match hit.value {
            crate::markup::Decoration::Styled(id) => Some((hit.range.clone(), *id)),
            _ => None,
        })
        .collect()
}

fn derive_once(document: &mut Document, editor: crate::editor::EditorId) -> StripeOutcome {
    let mut launches = document.scroll_stripe_launches(&theme());
    let launch = launches.pop().expect("one launch owed");
    assert_eq!(launch.editor, editor);
    run_work(launch.effect.work, &theme())
}

#[test]
fn flagged_styled_ranges_project_onto_the_track() {
    let mut document = plain_document(&hundred_lines());
    let editor = pane(&mut document);
    let markup = tinted(&mut document, editor, &[90..95, 810..815], StyleId::Match);
    let outcome = derive_once(&mut document, editor);

    let stripes = &outcome.stripes;
    assert!(stripes.content_height > 0.0);
    assert_eq!(stripes.segments.len(), 2, "two marks, far apart");
    assert_eq!(stripes.segments[0].byte, 90);
    assert_eq!(stripes.segments[1].byte, 810);
    assert_eq!(stripes.segments[0].style, StyleId::Match);
    // Line 10 of 100 sits a tenth of the way down; line 90 nine tenths.
    let at = |segment: &StripeSegment| segment.y.start / stripes.content_height;
    assert!((at(&stripes.segments[0]) - 0.1).abs() < 0.03);
    assert!((at(&stripes.segments[1]) - 0.9).abs() < 0.03);

    // A whole-markup rewrite moves the marks with it.
    retint(&mut document, markup, &[400..405], StyleId::Match);
    let outcome = derive_once(&mut document, editor);
    assert_eq!(outcome.stripes.segments.len(), 1);
    assert_eq!(outcome.stripes.segments[0].byte, 400);
}

#[test]
fn unflagged_entries_and_stripeless_styles_are_invisible() {
    let mut document = plain_document(&hundred_lines());
    let editor = pane(&mut document);

    // An unflagged entry never enters the capture...
    let unflagged = document.add_markup();
    retint(&mut document, unflagged, &[80..85], StyleId::Match);
    // ...and a flagged entry whose style carries no stripe color
    // contributes nothing.
    tinted(&mut document, editor, &[160..165], StyleId::Keyword);

    let outcome = derive_once(&mut document, editor);
    assert!(outcome.stripes.segments.is_empty());
}

#[test]
fn touching_same_style_marks_merge_and_keep_the_first_byte() {
    let mut document = plain_document(&hundred_lines());
    let editor = pane(&mut document);
    tinted(&mut document, editor, &[90..95, 100..105], StyleId::Match);

    let outcome = derive_once(&mut document, editor);
    assert_eq!(outcome.stripes.segments.len(), 1, "adjacent lines merge");
    assert_eq!(outcome.stripes.segments[0].byte, 90);
}

#[test]
fn the_lane_relaunches_only_when_the_fingerprint_moves() {
    let store = &imba::store::Store::new();
    let ui = crate::test_document::test_ui();
    let mut document = plain_document(&hundred_lines());
    let editor = pane(&mut document);
    let markup = tinted(&mut document, editor, &[90..95], StyleId::Match);

    assert_eq!(document.scroll_stripe_launches(&theme()).len(), 1);
    assert_eq!(
        document.scroll_stripe_launches(&theme()).len(),
        0,
        "an unmoved fingerprint owes nothing"
    );

    document.insert(
        editor,
        "typed",
        store,
        ui,
        &fonts(),
        &theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        document.scroll_stripe_launches(&theme()).len(),
        1,
        "an edit moves the revision"
    );

    retint(&mut document, markup, &[8..13], StyleId::Match);
    assert_eq!(
        document.scroll_stripe_launches(&theme()).len(),
        1,
        "a flagged swap moves the generation"
    );

    let unflagged = document.add_markup();
    retint(&mut document, unflagged, &[16..21], StyleId::Match);
    assert_eq!(
        document.scroll_stripe_launches(&theme()).len(),
        0,
        "an unflagged swap moves nothing"
    );
}

#[test]
fn stale_serials_drop_and_the_last_launch_lands() {
    let mut document = plain_document(&hundred_lines());
    let editor = pane(&mut document);
    let markup = tinted(&mut document, editor, &[90..95], StyleId::Match);

    let stale = derive_once(&mut document, editor);
    retint(&mut document, markup, &[810..815], StyleId::Match);
    let fresh = derive_once(&mut document, editor);

    document.apply_scroll_stripes(stale);
    assert!(
        document.scroll_stripes(editor).is_none(),
        "a superseded landing drops"
    );
    document.apply_scroll_stripes(fresh);
    let landed = document
        .scroll_stripes(editor)
        .expect("the last launch lands");
    assert_eq!(landed.segments[0].byte, 810);
}

#[test]
fn the_diff_markup_is_derived_from_the_operation_and_classifies_hunks() {
    // An inserted line, a word swap, a trailing deletion — the
    // operation is built first (`diff`), the markup derived FROM it
    // (`hunk_markup`, the presentation stage).
    let left = text::Text::from_string_exact("aaa\nbbb\nccc dog fox\nddd\neee\n");
    let right = text::Text::from_string_exact("aaa\nNEW LINE\nbbb\nccc cat fox\nddd\n");
    let operation = myersdiff::diff(&left, &right);
    let markup = crate::diff::hunk_markup(&operation, &right);
    assert_eq!(operation.new_len() as usize, right.byte_count());

    let hunks = styled_hits(&markup);
    assert_eq!(hunks.len(), 3, "three hunks: {hunks:?}");
    let (added, modified, deleted) = (&hunks[0], &hunks[1], &hunks[2]);
    assert_eq!(
        (added.0.clone(), added.1),
        (4..13, StyleId::DiffAdded),
        "the inserted line, whole"
    );
    assert_eq!(modified.1, StyleId::DiffModified);
    assert!(
        modified.0.start >= 17 && modified.0.end <= 29 && !modified.0.is_empty(),
        "the word swap stays inside its line: {modified:?}"
    );
    assert_eq!(
        (deleted.0.clone(), deleted.1),
        (33..33, StyleId::DiffDeleted),
        "the trailing deletion is a marker at its point"
    );
}

#[test]
fn removing_the_last_diff_owes_one_clearing_relaunch() {
    let store = &imba::store::Store::new();
    let ui = crate::test_document::test_ui();
    // The commit road: the diff is untracked, the gutter clears at
    // once — and the TRACK must not keep the stale marks. The leaving
    // markup bumps the generation while the diff still vouches for
    // it, and the painted track keeps the document in the sweep until
    // the clearing landing empties it.
    let source = hundred_lines();
    let base = text::Text::from_string_exact(&source.replace("line 050", "line ~50"));
    let mut document = plain_document(&source);
    let editor = pane(&mut document);
    let operation = myersdiff::diff(&base, document.text());
    let id = document.add_diff(operation, 0);
    document.mark_scroll_stripes(editor, document.diff(id).expect("tracked").markup());

    let outcome = derive_once(&mut document, editor);
    assert!(!outcome.stripes.segments.is_empty(), "the hunk projects");
    document.apply_scroll_stripes(outcome);

    let generation = document.scroll_stripe_generation();
    document.remove_diff(
        id,
        &[],
        store,
        ui,
        &fonts(),
        &theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    assert!(
        document.scroll_stripe_generation() > generation,
        "the leaving diff markup moves the fingerprint"
    );
    assert!(
        document.wants_scroll_stripes(),
        "the painted track still owes its clearing pass"
    );

    let outcome = derive_once(&mut document, editor);
    assert!(
        outcome.stripes.segments.is_empty(),
        "nothing left to project"
    );
    document.apply_scroll_stripes(outcome);
    assert!(
        !document.wants_scroll_stripes(),
        "cleared and contributor-less, the sweep forgets the document"
    );
}

#[test]
fn a_diff_carries_its_change_map_from_birth() {
    let store = &imba::store::Store::new();
    let ui = crate::test_document::test_ui();
    // The base is the target with line 50 spelled differently and one
    // EXTRA line after line 5 — so the operation carries one modified
    // hunk and one pure deletion (target side empty).
    let source = hundred_lines();
    let mut base = source.replace("line 050", "line ~50");
    base.insert_str(9 * 6, "only in the base\n");
    base.push_str("trailing, gone from the target\n");
    let base = text::Text::from_string_exact(&base);
    let mut document = plain_document(&source);
    let editor = pane(&mut document);

    let operation = myersdiff::diff(&base, document.text());
    let id = document.add_diff(operation, 0);
    let map = document.diff(id).expect("tracked").markup();
    assert!(
        document.feature_markup(map).is_some(),
        "THE diff markup rides the document"
    );
    // The stripes role is a REGISTRATION (the documents layer's
    // track_diff/enable doors do this in the app): a diff off the
    // register — a split panel's — never reaches a pane's track.
    document.mark_scroll_stripes(editor, map);

    let outcome = derive_once(&mut document, editor);
    let styles: Vec<StyleId> = outcome
        .stripes
        .segments
        .iter()
        .map(|segment| segment.style)
        .collect();
    assert!(
        styles.contains(&StyleId::DiffModified),
        "the rewritten line marks modified: {styles:?}"
    );
    assert!(
        styles.contains(&StyleId::DiffDeleted),
        "the dropped line leaves its marker: {styles:?}"
    );
    let eof_marker = outcome
        .stripes
        .segments
        .iter()
        .rfind(|segment| segment.style == StyleId::DiffDeleted)
        .expect("checked above");
    assert!(
        eof_marker.y.start / outcome.stripes.content_height > 0.9,
        "the trailing deletion marks the BOTTOM of the track"
    );

    // A normalize landing installs the worker-derived refresh.
    let generation = document.scroll_stripe_generation();
    let base_now = document.text().clone();
    let identity = myersdiff::diff(&base_now, document.text());
    let fresh = crate::diff::hunk_markup(&identity, document.text());
    assert!(document.install_normalized_diff(id, identity, 0));
    document.install_diff_markup(
        id,
        fresh,
        vec![0..u32::MAX],
        document.revision(),
        store,
        ui,
        &fonts(),
        &theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    assert!(document.scroll_stripe_generation() > generation);
    let outcome = derive_once(&mut document, editor);
    assert!(
        outcome.stripes.segments.is_empty(),
        "an identity diff maps no changes"
    );
}
