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
        let mut start = layout.edit(&operation);
        for line in document.edit(&operation, &store, ui, &theme(), fx!()) {
            start = start.min(layout.mark_modified_in(line));
        }
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
    #![proptest_config(ProptestConfig { cases: 48, failure_persistence: None, .. ProptestConfig::default() })]

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

    /// Wrapping at a narrower width never gives a hard line fewer
    /// rows. (Not "never a lower height": a row is as tall as the
    /// tallest script's font in it, and a narrower wrap can part a
    /// CJK word from a Hebrew one that shared a row, leaving one
    /// row a pixel shorter — the 2026-10-27 finding, kept below.)
    #[test]
    fn a_narrower_wrap_never_gives_a_line_fewer_rows(
        spec in gen::document_spec(),
        widths in (gen::wrap_width(), gen::wrap_width()),
    ) {
        let (narrow, wide) = (widths.0.min(widths.1), widths.0.max(widths.1));
        let document = gen::build(&spec);
        let narrow_rows = rows_per_line(&fresh(&document, narrow, None), &document);
        let wide_rows = rows_per_line(&fresh(&document, wide, None), &document);
        prop_assert_eq!(
            narrow_rows.len(),
            wide_rows.len(),
            "same hard lines at {} and {}",
            narrow,
            wide
        );
        for (line, (narrow_count, wide_count)) in narrow_rows.iter().zip(&wide_rows).enumerate() {
            prop_assert!(
                narrow_count >= wide_count,
                "line {line}: {narrow_count} rows at {narrow}, {wide_count} at {wide}"
            );
        }
    }
}

/// How many rows each hard line of the text takes in `layout`: a row
/// ends a line when its last byte is a newline, and the layout's last
/// row ends the last line.
fn rows_per_line(layout: &DocumentLayout, document: &Document) -> Vec<usize> {
    let mut view = document.text().view();
    let mut counts = Vec::new();
    let mut rows = 0;
    let ranges = layout.element_byte_ranges();
    for (index, range) in ranges.iter().enumerate() {
        rows += 1;
        let mut last = Vec::new();
        if range.end > range.start {
            view.byte_range_into(range.end as usize - 1, range.end as usize, &mut last);
        }
        if last.first() == Some(&b'\n') || index + 1 == ranges.len() {
            counts.push(rows);
            rows = 0;
        }
    }
    counts
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
            document.apply_repair(repaired, &theme());
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, failure_persistence: None, .. ProptestConfig::default() })]

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

    /// Runs `steps` through the document's own editor, settling the
    /// repair worker after each, and checks the editor's layout against
    /// a fresh one (the settled-editor property's body).
    fn settled(spec: &Spec, width: f32, steps: &[Step]) {
        let store = imba::store::Store::new();
        let ui = test_ui();
        let theme = theme();
        let mut document = gen::build(spec);
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
            assert_eq!(
                document.element_heights(editor),
                document.fresh_layout_heights(editor, &store, ui, &theme),
                "step {index} {step:?}"
            );
            assert_eq!(
                document.find_misaligned_boundary(editor),
                None,
                "step {index} {step:?}"
            );
        }
    }

    /// A deletion took a bidi line's whole first row; the row that
    /// survived at the line's start kept its continuation flag, so
    /// the bidi rule walked the repair back to the previous line's
    /// start, where it converged before reaching the damage — every
    /// worker round, forever. Fix: a deletion that begins at a line's
    /// first byte leaves a line start at its landing.
    #[test]
    fn a_deletion_that_takes_a_bidi_line_s_first_row_leaves_a_line_start() {
        settled(
            &spec(
                "dolor 日本語 lorem xdshazzqgqnosaodnviohqgmrrdtjnmxefocuzdzahqbszscexfq lorem שלום gamma ipsum 1. - daivrhaeognyeqbpywatgxdeu\n😀 qddixyvnzsbojbxvitjpkqbywbyp lorem - I مرحبا\na # - ufjbbxlplddytuoufznnnmxmpwmnviueqjzxbkcalggubg alpha delta dolor dolor\nlorem  x😀y dgdelhfxwdnkmeoaggdedxbakbbqwxkltfwptrjk שלום ipsum \n\nx😀y 😀 ipsum  مرحبا  yxablqnardpplweblxjhckswlwzimiwahteosjveonmliyflrxwqxtofgjt tmjrptupqhfyzboxsrvjm a-שלום a-שלום\nnaïve a  lorem 🎉x azzafnempzqkrenpmwabtjiygprkijgcpylorkzzqchkbyc\n😀 naïve",
                vec![Block::Plain; 8],
                vec![],
            ),
            573.0,
            &edits(&[
                (0.48035145, 0.068869166, ""),
                (0.9842734, 0.13078359, "atdljupduuwacouomkcctoshczhpolmxdkneqjmosut"),
                (0.20090021, 0.0, "🎉x naïve עברית a-שלום tab\tbed altvizsnbharzyhsqbburcfg עברית beta gamma  ymcidasvdwhnhsrtzpazmoodjvywyegvmvc\n"),
                (0.26186842, 0.0, "naïve"),
                (0.15366876, 0.0, "a-שלום dolor tab\tbed naïve ipsum I lorem 😀 sxcaasnontfyurpmohorljcrkfdiuqwhamklmzmnulrhegqozzmdvyys\n"),
                (0.2313143, 0.03697908, "שלום"),
                (0.33288872, 0.26861355, "\n"),
            ])
            .into_iter()
            .map(Step::Edit)
            .collect::<Vec<_>>(),
        );
    }

    /// A code block's first row held a CJK word and a Hebrew one at
    /// the wide width; the narrow wrap parted them, and the row left
    /// with only CJK came out a pixel shorter than one with Hebrew:
    /// 1018 against 1019. Not a fault — a row is as tall as the
    /// tallest font in it — so the property counts rows per line.
    #[test]
    fn a_narrower_wrap_may_be_a_pixel_shorter_but_never_has_fewer_rows() {
        let spec = spec(
            " tab\tbed 日本語 leghedvrnoegkkkobdowmbe שלום עברית xbdhzldhoezgzypopvgkqcnrmkgyyywtgcfehyw dolor  ivtvsjdgntzpaaribqhstunikfdykgimqdaopisrfqaojoqdnsp\n\n# 😀 héllo lorem\n- ztihcpenrepavfebilmwkfnqzqofpkmzoyzsnnmouzquzumddbb a-שלום amet héllo  alpha tab\tbed\njyotgvhwfdsarsliwiadaidqioxeemxmvivalwagxfeiighklmwjuzaymjmv 1. 🎉x  x😀y مرحبا\ngamma\n- alpha héllo a-שלום uogfuuywondnrqwfpspalazkldkgwhxbyzaxupszrpyznoahogm sit\ntab\tbed עברית ipsum # 🎉x\n\n",
            vec![
                Block::CodeBlock,
                Block::Plain,
                Block::Plain,
                Block::Plain,
                Block::Plain,
                Block::Plain,
                Block::Plain,
                Block::Plain,
                Block::Plain,
            ],
            vec![(0, 0.2680288, 0.17642334, Inline::InlineCode)],
        );
        let document = gen::build(&spec);
        let narrow = rows_per_line(&fresh(&document, 635.0, None), &document);
        let wide = rows_per_line(&fresh(&document, 738.0, None), &document);
        assert_eq!(narrow.len(), wide.len());
        assert!(
            narrow.iter().zip(&wide).all(|(n, w)| n >= w),
            "{narrow:?} vs {wide:?}"
        );
    }

    /// A deletion ran from inside a header block's second line to
    /// far past the block, so the block now ends exactly where its
    /// first line does — and that line's last row carries the block
    /// gap now. The repair started at the landing, the row before it
    /// kept the gapless height. Fix: the interval set reports the
    /// intervals a deletion shortened as well as the ones it took,
    /// and the lines on both sides of the cut are re-laid.
    #[test]
    fn a_deletion_that_ends_a_block_at_a_line_boundary_gives_the_line_its_gap() {
        check(
            &spec(
                "héllo مرحبا مرحبا alpha\nhéllo gamma\nnaïve # amet alpha 1. dolor alpha naïve dolor\ndbnktlcqknxcbsbozmcadykdxwbfvpbep  kvootipvcamyidfqtsfnsejqduwasnpontrwxyuzmprgkwefyghp 😀 مرحبا 🎉x gfbdnngpucfwjzssggnoomvilc  😀 🎉x\nifajekkntdmxphxilqssvcgkbejgmhkjmdgjrx amet naïve  a-שלום héllo lorem x😀y 🎉x",
                vec![
                    Block::Header(1),
                    Block::Plain,
                    Block::Plain,
                    Block::Plain,
                    Block::Plain,
                ],
                vec![],
            ),
            886.0,
            &edits(&[
                (
                    0.044226028,
                    0.0,
                    "x😀y mczhctfutxwgqlwguqsvfjwmgnqkqrlbuyeempcdb  gamma sit\n",
                ),
                (0.18581016, 0.9276276, ""),
            ]),
            Some(4000.0),
        );
    }

    /// A header wrapped into two rows; a deletion inside row two
    /// shortens its first word so it fits on row one again. The
    /// repair started at row two and row one kept its early break.
    /// Fix: a repair starts a row before the change.
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

    /// A deletion joined two lines, landing on what had been line
    /// four's first row; the join is laid from a row before it and
    /// the landing row, damaged, is re-laid rather than resumed on.
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

    /// A 40 px budget against a 98 px header row: a pass must still
    /// make progress, so it starts exactly at its pending byte and
    /// never backs up on its own.
    #[test]
    fn a_budget_below_one_row_still_finishes_the_line() {
        check(
            &spec("日本語 beta ipsum", vec![Block::Header(1)], vec![]),
            447.0,
            &edits(&[(0.09760137, 0.0, ""), (0.96863896, 0.08269944, "dolor")]),
            Some(40.0),
        );
    }

    /// Newlines carved an old row loose at a position where a hard
    /// line now starts; a budgeted repair must converge on it all the
    /// same.
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

    /// A deletion took the bytes a header's block interval lived on,
    /// so the merged line is plain in a fresh build; the rows before
    /// the edit kept their header height. Fix: the interval set
    /// reports what a deletion dropped (`Intervals::edit`), the markup
    /// keeps the drops that style a whole line, and the document
    /// damages those lines like any other markup change.
    #[test]
    fn a_deletion_that_takes_a_block_interval_re_styles_the_whole_line() {
        check(
            &spec(
                "sit  ipsum alpha I tab\tbed x😀y xvntjnkkwhijtgtekcjrgbxuheobiqccpgsrodlnubudv vlnhupcdgxihcixqdymyvfqwegqzyngmyuhfjx\n日本語  lorem مرحبا bttcqtjilozwfbqybznqbli 1. \nhéllo amet naïve hijiczcfkytckigzatdvxmeoerkqbvbddfepbpnlajyjytwawfsvjkgjsdpd 🎉x\n1. 日本語 amet  ipsum עברית jfalkmtvhhagkpcafdjgjixeqcxiagze 日本語 ypcmrjqlradsrcijhqycgupvxnuqjqlwuuuroonhoasyzay delta a\n🎉x   x😀y\naydpunvchbdtwhtrkrvoiyidpgzvptix 😀 delta  naïve delta\n",
                vec![
                    Block::Plain,
                    Block::Header(1),
                    Block::Quote,
                    Block::Plain,
                    Block::Plain,
                    Block::Plain,
                ],
                vec![],
            ),
            438.0,
            &edits(&[
                (0.17902827, 0.20235488, "日本語"),
                (0.045013815, 0.0, "שלום"),
                (0.2689071, 0.10581228, "مرحبا"),
                (0.46812055, 0.27262074, "gamma"),
                (0.13750376, 0.2261995, "-"),
            ]),
            None,
        );
    }

    /// A join of a centered line with a quoted one: the merged line
    /// carries the quote's inset, so even the rows before the
    /// deletion wrap differently. A newline edit re-lays the whole
    /// line it lands in, backwards too.
    #[test]
    fn a_join_re_styles_the_rows_before_the_deletion_too() {
        check(
            &spec(
                "lorem delta nchrajcilkkffwwygwuzntxqgitisbmdbobcuaigrvhsnkqyswzfsconfu  alpha\ngamma # -\n1.  unsipbeidubifawtwayfqirpbhfyrbempoq x😀y uomovvrfwlgojkxbzrsnffcuheak 🎉x héllo amet\na beta 🎉x 日本語 🎉x amet mehdhypkilpotogjezve dolor 🎉x\nalpha\naiwxxxyjxirjcrbafwdflfufhrvzyxzkmmfmlirmxyatuqmezkrwgk amet obfnatsqchofbqlwqgscqyloomixaxpuga a 1. - # icwysvdhnbkpcinotsbayylqjclfqkkfekacckycajpjwxbu gamma",
                vec![
                    Block::Plain,
                    Block::Plain,
                    Block::Plain,
                    Block::Centered,
                    Block::Quote,
                    Block::Quote,
                ],
                vec![
                    (3, 0.63789535, 0.66176593, Inline::Emphasis),
                    (4, 0.49812177, 0.11850176, Inline::Strong),
                ],
            ),
            123.0,
            &edits(&[
                (0.47366163, 0.043680135, "gamma saxlrkiueghhstcbeiqummvgojabuexspp\n"),
                (0.58046025, 0.6494543, "#"),
                (0.6335012, 0.2628099, ""),
            ]),
            None,
        );
    }

    /// An insert ending in a newline: its end is the next line's
    /// start, so reshaping only there left the line that received
    /// the text — now bidi, now a different shape — half old. An
    /// insert reshapes the line it starts in and the one it ends in.
    #[test]
    fn an_insert_with_a_newline_re_styles_both_halves() {
        check(
            &spec(
                "I vbqoicxyqitgrrompfabjxamubzcucheeaqeaxnonncntllgxnadjpq alpha\n日本語 delta - naïve dolor sit ikzwvtbvhherbznimttzshugeaqjzkb alpha dolor 😀 \n\na\n",
                vec![Block::Plain, Block::Header(1), Block::Indent(1), Block::Centered],
                vec![
                    (2, 0.7287753, 0.91773003, Inline::Emphasis),
                    (1, 0.8337991, 0.42689058, Inline::Emphasis),
                ],
            ),
            626.0,
            &edits(&[
                (0.24734986, 0.24392039, "dolor"),
                (0.93752664, 0.0, "\n"),
                (0.79139507, 0.0, "\n"),
                (0.45724383, 0.17338389, "1. a-שלום 1. amet dolor amet sit gamma مرحبا עברית \n"),
                (0.941923, 0.0, "שלום"),
                (0.79902154, 0.8486985, "ipsum"),
                (0.91219944, 0.0, "\n"),
            ]),
            None,
        );
    }

    /// A budget cut inside a line with right-to-left text: the suffix
    /// measured a shade differently from the whole line even under
    /// the line's base direction. A bidi line is laid whole, and a
    /// spent budget runs to its end.
    #[test]
    fn a_budgeted_repair_lays_a_bidi_line_whole() {
        check(
            &spec(
                "\nمرحبا itgicnhlqoldozlettazgklplv beta مرحبا amet\nمرحبا x😀y gamma a  1. odlhoisirdmbjrmgsxfngdenonbjxgdhnjwaua עברית עברית\n",
                vec![Block::Plain, Block::Plain, Block::Centered],
                vec![
                    (2, 0.22460741, 0.6417349, Inline::Link),
                    (0, 0.9011329, 0.56087565, Inline::Strikethrough),
                    (0, 0.31081036, 0.5334279, Inline::Link),
                    (0, 0.6717885, 0.8252064, Inline::Emphasis),
                ],
            ),
            380.0,
            &edits(&[
                (0.023788484, 0.0, ""),
                (0.7814382, 0.3889426, "\n\n"),
                (0.63694817, 0.28769583, "oedshrtoinzuyjribbsejdwj"),
            ]),
            Some(40.0),
        );
    }

    /// Old rows flagged bidi after the right-to-left text left their
    /// line: a repair that trusted the flag snapped back to the line
    /// start every pass and never reached its pending byte. The text
    /// decides whether a line is bidi.
    #[test]
    fn a_line_that_lost_its_right_to_left_text_still_converges() {
        check(
            &spec(
                " delta amet x😀y uavxntiuhqozszqzwrfgbhqagmntbekjy \n🎉x beta sit a 1. שלום 🎉x dolor vnozlhnzzgcmacmmvizxnjmlqliydatfls dnajhfazyinkzsuaqdnuvodivegaigotxmuyhfyksgvgqswpgjdegnhgectt lorem\nsit snmdgqqjiszpmqvmkvlubgfdvncvxulb - beta - I lorem a-שלום I dolor 🎉x\n amet - 🎉x gamma 1. amet alpha x😀y alpha\nשלום 🎉x\n\nlorem 1. dolor 日本語 ipsum\n",
                vec![
                    Block::Plain,
                    Block::Header(2),
                    Block::Plain,
                    Block::Right,
                    Block::ListItem,
                    Block::Plain,
                    Block::CodeBlock,
                ],
                vec![
                    (5, 0.6956627, 0.50254697, Inline::Strong),
                    (3, 0.1095485, 0.7720866, Inline::Hidden),
                    (0, 0.2270711, 0.7755435, Inline::Link),
                    (2, 0.6690437, 0.24562815, Inline::Emphasis),
                ],
            ),
            432.0,
            &edits(&[
                (0.12955065, 0.0, "mbhsnwvalbavrwmjaaegepylurpxxbzbzyk"),
                (0.2918115, 0.27800044, "\n\n"),
                (0.8736551, 0.0, "😀 - I iruelxhaahkuhxtqadpbjptjtye 1. I wudbrbyohkcgglffqgukuystllucembsxjloatayssfsvjlwlauseekl\n"),
            ]),
            Some(40.0),
        );
    }
}
