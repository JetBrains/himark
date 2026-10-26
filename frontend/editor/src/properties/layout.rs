// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The layout is INCREMENTAL: an edit splices the element rope,
//! marks damage, and a repair re-lays only the damaged region. The
//! oracle is a layout built from nothing over the same text and
//! markup — the two must agree element for element, or the
//! incremental bookkeeping (rope, widths, damage, safepoints) has
//! drifted.

use std::ops::Range;

use proptest::prelude::*;

use super::gen::{self, Block, EditSpec, Inline, Spec};
use crate::{
    document::{Document, EditorBuild},
    document_layout::{DocumentLayout, SYNC_LAYOUT_HEIGHT},
    markup::OverlaidMarkup,
    test_document::test_ui,
    text_cursor::byte_count,
};

fn theme() -> crate::theme::Theme {
    crate::theme::embedded()
}

/// The oracle: a complete layout of the document as it stands now.
fn fresh(document: &Document, width: f32, window: Option<Range<u32>>) -> DocumentLayout {
    let extras: Vec<_> = document.document_scoped_markups().collect();
    DocumentLayout::build_complete_slow(
        document.text(),
        OverlaidMarkup::new(document.markup(), &extras),
        width,
        &theme(),
        window,
    )
}

#[derive(Debug, PartialEq)]
struct Shape {
    ranges: Vec<Range<u32>>,
    heights: Vec<(u32, f32)>,
    spacers: Vec<f32>,
    height: f32,
    max_width: f32,
}

fn shape(layout: &DocumentLayout) -> Shape {
    Shape {
        ranges: layout.element_byte_ranges(),
        heights: layout.element_heights(),
        spacers: layout.element_spacers(),
        height: layout.height(),
        max_width: layout.max_width(),
    }
}

fn assert_agrees(
    live: &DocumentLayout,
    document: &Document,
    width: f32,
    context: &str,
) -> Result<(), TestCaseError> {
    let fresh = fresh(document, width, None);
    prop_assert_eq!(live.repair_pending(), None, "{}: repair settled", context);
    prop_assert_eq!(
        live.byte_size(),
        byte_count(document.text()),
        "{}: the layout covers the text",
        context
    );
    prop_assert_eq!(
        live.find_misaligned_boundary(document.text()),
        None,
        "{}: elements end on char boundaries",
        context
    );
    prop_assert_eq!(shape(live), shape(&fresh), "{}", context);
    Ok(())
}

/// Repair from `start` under a height budget, resuming from the
/// pending byte until nothing is pending — the app's shape, where a
/// keystroke repairs a little and the worker finishes.
fn repair_all(
    layout: &mut DocumentLayout,
    document: &Document,
    width: f32,
    start: u32,
    budget: f32,
) {
    let extras: Vec<_> = document.document_scoped_markups().collect();
    let markup = OverlaidMarkup::new(document.markup(), &extras);
    layout.repair_layout_bounded_slow(document.text(), markup, width, &theme(), start, budget);
    let mut rounds = 0;
    while let Some(pending) = layout.repair_pending() {
        rounds += 1;
        assert!(rounds < 10_000, "the repair chain must terminate");
        layout.repair_layout_bounded_slow(
            document.text(),
            markup,
            width,
            &theme(),
            pending,
            budget,
        );
    }
}

/// Edit the layout and the document in step; after every edit the
/// repaired layout is the fresh build. `budget` of `None` repairs
/// fully in one pass, `Some` cuts every pass at that height.
fn repaired_equals_fresh(
    spec: &Spec,
    width: f32,
    edits: &[EditSpec],
    budget: Option<f32>,
) -> Result<(), TestCaseError> {
    let store = imba::store::Store::new();
    let ui = test_ui();
    let mut document = gen::build(spec);
    let mut layout = fresh(&document, width, None);
    assert_agrees(&layout, &document, width, "before any edit")?;
    for (index, edit) in edits.iter().enumerate() {
        let Some(operation) = gen::resolve(document.text(), edit) else {
            continue;
        };
        let start = layout.edit(&operation);
        document.edit(&operation, &store, ui, &theme(), fx!());
        match budget {
            Some(budget) => repair_all(&mut layout, &document, width, start, budget),
            None => {
                let extras: Vec<_> = document.document_scoped_markups().collect();
                layout.repair_layout_slow(
                    document.text(),
                    OverlaidMarkup::new(document.markup(), &extras),
                    width,
                    &theme(),
                    start,
                );
            }
        }
        assert_agrees(
            &layout,
            &document,
            width,
            &format!("edit {index} {edit:?} budget {budget:?}"),
        )?;
    }
    Ok(())
}

fn budget() -> impl Strategy<Value = f32> {
    prop_oneof![Just(40.0f32), Just(200.0f32), Just(SYNC_LAYOUT_HEIGHT),]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    /// Edit the layout and the document in step, repair the layout
    /// from the edit's own repair start, and the result is the layout
    /// a fresh build gives: same elements, same heights, same widths.
    #[test]
    fn a_repaired_layout_equals_a_fresh_build_after_any_edits(
        spec in gen::document_spec(),
        width in gen::width(),
        edits in proptest::collection::vec(gen::edit_spec(), 1..8),
    ) {
        repaired_equals_fresh(&spec, width, &edits, None)?;
    }

    /// The same, with every repair cut short by a height budget and
    /// resumed from its pending byte until nothing is pending.
    /// Budgeted resumption must converge on the same layout.
    #[test]
    fn budgeted_repairs_converge_on_the_fresh_build(
        spec in gen::document_spec(),
        width in gen::width(),
        budget in budget(),
        edits in proptest::collection::vec(gen::edit_spec(), 1..8),
    ) {
        repaired_equals_fresh(&spec, width, &edits, Some(budget))?;
    }

    /// Wrapping at a narrower width never makes a document shorter.
    #[test]
    fn a_narrower_wrap_never_lowers_the_height(
        spec in gen::document_spec(),
        widths in (gen::wrap_width(), gen::wrap_width()),
    ) {
        let (narrow, wide) = (widths.0.min(widths.1), widths.0.max(widths.1));
        let document = gen::build(&spec);
        let narrow_height = fresh(&document, narrow, None).height();
        let wide_height = fresh(&document, wide, None).height();
        prop_assert!(
            narrow_height + 0.01 >= wide_height,
            "narrow {narrow} -> {narrow_height}, wide {wide} -> {wide_height}"
        );
    }
}

#[derive(Clone, Debug)]
enum Step {
    Edit(EditSpec),
    Resize(f32),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        5 => gen::edit_spec().prop_map(Step::Edit),
        1 => gen::wrap_width().prop_map(Step::Resize),
    ]
}

/// Drain the editor's pending repairs the way the app's worker does:
/// collect the repair effect, run the handler, land the repaired
/// layouts. Returns how many rounds it took.
pub(super) fn settle(document: &mut Document) -> usize {
    let handler = crate::repair::RepairHandler(crate::test_document::test_workshop(theme()));
    let mut rounds = 0;
    loop {
        let ids: Vec<_> = document.editor_ids().collect();
        let pairs: Vec<_> = ids.iter().map(|id| (id, document.editor(*id))).collect();
        let Some(effect) = crate::repair::RepairEffect::collect(document, pairs.into_iter()) else {
            return rounds;
        };
        rounds += 1;
        assert!(rounds < 1_000, "settling must terminate");
        for repaired in handler.repairs(effect) {
            document.apply_repair(repaired);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, .. ProptestConfig::default() })]

    /// Through the document's own editor: edits repair within the
    /// keystroke budget and hand the rest to the repair worker; a
    /// resize re-wraps everything the same way. Once settled, the
    /// editor's layout is the fresh layout.
    #[test]
    fn a_settled_editor_matches_the_fresh_layout_after_edits_and_resizes(
        spec in gen::document_spec(),
        width in gen::wrap_width(),
        steps in proptest::collection::vec(step(), 1..8),
    ) {
        let store = imba::store::Store::new();
        let ui = test_ui();
        let theme = theme();
        let mut document = gen::build(&spec);
        let editor = document.add_editor(
            width,
            None,
            EditorBuild::Complete,
            &[],
            &store,
            ui,
            &theme,
            fx!(),
        );
        for (index, step) in steps.iter().enumerate() {
            match step {
                Step::Edit(edit) => {
                    if let Some(operation) = gen::resolve(document.text(), edit) {
                        document.edit(&operation, &store, ui, &theme, fx!());
                    }
                }
                Step::Resize(to) => {
                    document.resize(editor, *to, 0, &store, ui, &theme, fx!());
                }
            }
            settle(&mut document);
            let context = format!("step {index} {step:?}");
            prop_assert_eq!(
                document.element_heights(editor),
                document.fresh_layout_heights(editor, &store, ui, &theme),
                "{}", context
            );
            prop_assert_eq!(document.find_misaligned_boundary(editor), None, "{}", context);
            let covered = document.element_byte_ranges(editor).last().map_or(0, |range| range.end);
            prop_assert_eq!(covered, byte_count(document.text()), "{}", context);
        }
    }
}

/// The properties' shrunk findings, kept as plain tests: each input
/// below failed before its fix on 2026-10-26.
mod shrunk {
    use super::*;

    fn spec(source: &str, blocks: Vec<Block>, spans: Vec<(usize, f32, f32, Inline)>) -> Spec {
        Spec {
            source: source.to_owned(),
            blocks,
            spans,
        }
    }

    fn edits(list: &[(f32, f32, &str)]) -> Vec<EditSpec> {
        list.iter()
            .map(|(at, delete, insert)| EditSpec {
                at: *at,
                delete: *delete,
                insert: (*insert).to_owned(),
            })
            .collect()
    }

    fn check(spec: &Spec, width: f32, edits: &[EditSpec], budget: Option<f32>) {
        if let Err(failure) = repaired_equals_fresh(spec, width, edits, budget) {
            panic!("{failure}");
        }
    }

    /// A header wrapped into two rows; a deletion inside row two
    /// shortens its first word so it fits on row one again. The
    /// repair started at row two and row one kept its early break.
    /// Fix: only a hard line's first row is a safepoint.
    #[test]
    fn a_deletion_in_a_wrapped_row_lets_the_row_above_reflow() {
        check(
            &spec("dolor   1. lorem \n", vec![Block::Header(1)], vec![]),
            542.0,
            &edits(&[
                (0.23259416, 0.0, ""),
                (0.12437968, 0.47167304, ""),
                (0.7854471, 0.0, "dolor"),
                (0.75771, 0.0, "ipsum"),
                (0.5855259, 0.0, ""),
                (0.6183153, 0.7714235, "a"),
            ]),
            None,
        );
    }

    /// A deletion from line one into line two, with an insert at the
    /// same spot. Line two's row lost its head but carried no damage,
    /// so the repair resumed on it with its old width. Fix: the row
    /// a deletion lands in is damaged, at its final position.
    #[test]
    fn a_cross_line_deletion_re_lays_the_row_it_lands_in() {
        check(
            &spec(
                "😀 ipsum 日本語 sit 🎉x tab\tbed 1. delta\nhéllo 1. 日本語 ipsum\n",
                vec![Block::Centered, Block::Plain],
                vec![
                    (1, 0.46736035, 0.75084764, Inline::Strikethrough),
                    (1, 0.4784934, 0.8698922, Inline::Strikethrough),
                    (1, 0.39935377, 0.93615985, Inline::Emphasis),
                ],
            ),
            353.0,
            &edits(&[(0.09424858, 0.9207125, "\n")]),
            None,
        );
    }

    /// Joining a header with the plain line after it makes the
    /// document end in a header; the trailing empty line takes the
    /// header's height in a fresh build but kept the plain one live.
    /// Fix: the trailing empty line is a row of the line before it,
    /// not a safepoint.
    #[test]
    fn the_trailing_empty_line_takes_the_height_of_the_line_it_follows() {
        check(
            &spec(
                "alpha\nx😀y tobvbkrsltgpwqesajrbejzohsqoinjzv qqvvqcexiykgsxarvxdhwluwcipweederdtuuvvgkyztyqlftn tab\tbed lorem\n",
                vec![Block::Header(1), Block::Plain],
                vec![],
            ),
            473.0,
            &edits(&[(0.013384547, 0.13329172, "-")]),
            None,
        );
    }

    /// A deletion joined two lines; what had been line four's first
    /// row still said it started a line, so the repair laid the
    /// join from there as a line start. Fix: the splice clears the
    /// landing row's flag and the repair backs up to the real start.
    #[test]
    fn a_deletion_joining_two_lines_restarts_the_repair_at_the_first() {
        check(
            &spec(
                "lorem lorem a sit\n ipsum gamma 1.\n delta sit 1. héllo naïve héllo delta dolor xsiknuvujdvprjfhjmcltrcyjpwaxulnkekdypi beta\nI # dolor alpha tab\tbed beta twkgoqargdubetuasfkuezetwtbdbfgcqgmlxteljxsnwtr delta héllo héllo\nhéllo a 1.  amet I I\namet  héllo nkbjjizlucqzmflizhlowcpsoqxlygkqchjivmsvljyhbpvvhnrvtzoonym ftoyxqryfskwmuknaghznxttgwdyttsscmyemrxtwdwislnausphcu lorem héllo I\n",
                vec![
                    Block::Plain,
                    Block::Plain,
                    Block::ListItem,
                    Block::Plain,
                    Block::Plain,
                    Block::Plain,
                ],
                vec![],
            ),
            402.0,
            &edits(&[
                (0.8996152, 0.068435684, ""),
                (0.67548805, 0.17754628, ""),
                (0.3867872, 0.0, "delta"),
                (0.679353, 0.10036768, "beta"),
                (0.13379966, 0.0, "naïve"),
                (0.86781186, 0.0, ""),
                (0.23894805, 0.17941631, ""),
            ]),
            None,
        );
    }

    /// A 40 px budget against a 98 px header row: every pass restarted
    /// at the line's first row and ran out before its second. Fix: a
    /// spent budget ends the pass at the next hard line, never inside
    /// one.
    #[test]
    fn a_budget_below_one_row_still_finishes_the_line() {
        check(
            &spec("日本語 beta ipsum", vec![Block::Header(1)], vec![]),
            447.0,
            &edits(&[(0.09760137, 0.0, ""), (0.96863896, 0.08269944, "dolor")]),
            Some(40.0),
        );
    }

    /// Newlines carved an old continuation row loose at a position
    /// where a hard line now starts; still flagged as a continuation,
    /// every budgeted pass backed up before it and stopped short of
    /// it. Fix: the pass promotes the old element at its budget stop.
    #[test]
    fn a_budgeted_pass_promotes_the_old_row_at_a_new_line_start() {
        check(
            &spec("日本語", vec![Block::Indent(2)], vec![]),
            162.0,
            &edits(&[
                (0.1431504, 0.23777409, "\n\n"),
                (0.1737767, 0.7510554, "\n\n"),
                (0.27289498, 0.0, "\n"),
                (0.19751462, 0.22227117, "\n"),
                (0.06641037, 0.1168113, "\n\n"),
                (0.86012447, 0.19899122, ""),
            ]),
            Some(40.0),
        );
    }
}
