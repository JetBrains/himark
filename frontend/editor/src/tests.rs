// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::str;

use operation::{Op, Operation};
use skia_safe::{textlayout::FontCollection, Point};

macro_rules! fx {
    () => {
        &mut imba::effect::Batch::new().effects()
    };
}

use crate::{
    document::Document,
    document_layout::DocumentLayout,
    editor_view::EditorView,
    markup::BlockStyle,
    shaped_line::line_paragraph,
    test_document::{
        fenced_code_document, header_marks, hidden_document, list_document, marked_document,
        plain_document,
    },
};

fn test_theme() -> crate::theme::Theme {
    crate::theme::Theme::embedded()
}

fn test_fonts() -> FontCollection {
    crate::embedded_fonts::collection()
}

#[test]
fn document_layout_wraps_more_at_narrow_width() {
    let heading = "# Title\n";
    let source = format!("{heading}\n{}", "word ".repeat(240));
    let blocks = [(0..heading.len() as u32, header_marks(1))];
    let document = marked_document(&source, &blocks);

    let narrow = layout_of(&document, 180.0);
    let wide = layout_of(&document, 900.0);

    assert_eq!(text_string(&document), source);
    assert_eq!(narrow.layout_width(), 180.0);
    assert_eq!(wide.layout_width(), 900.0);
    assert!(narrow.height() > wide.height());
}

#[test]
fn adjacent_list_items_stay_separate_layout_items() {
    let source = "- [x] Parse a tree-sitter tree\n- [x] Build document elements\n- [ ] Paint real inline spans\n";
    let document = list_document(source);
    let layout = layout_of(&document, 2000.0);
    let ranges = layout_ranges(&document, &layout);

    assert_eq!(ranges.len(), 4, "three items plus the empty last line");
    assert!(ranges[0].starts_with("- [x] Parse"));
    assert!(ranges[1].starts_with("- [x] Build"));
    assert!(ranges[2].starts_with("- [ ] Paint"));
    assert_eq!(ranges[3], "", "the trailing newline's empty last line");
}

#[test]
fn softwrap_toggles_to_a_panning_single_row_layout() {
    let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa ".repeat(8);
    let source = format!("{long}\nshort\n");
    let mut document = plain_document(&source);
    let editor = document.add_editor(
        600.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let mut store = imba::Store::new();
    let ui = imba::UiCtx::cold();

    let wrapped_height = document.content_height(editor);
    assert_eq!(document.layout_width(editor), 600.0);
    assert!(document.softwrap(editor));

    document.perform(
        &mut store,
        &ui,
        editor,
        crate::EditorCommand::ToggleSoftwrap,
        fx!(),
    );
    assert!(!document.softwrap(editor));
    assert_eq!(
        document.layout_width(editor),
        crate::document::NOWRAP_LAYOUT_WIDTH,
        "the layout lays at the nowrap constant"
    );
    assert!(
        document.content_height(editor) < wrapped_height,
        "the monster line collapses to one row: {} vs wrapped {wrapped_height}",
        document.content_height(editor)
    );
    let extent = document.max_width(editor);
    assert!(
        extent > 600.0,
        "the content extends past the pane: {extent}"
    );

    document.perform(
        &mut store,
        &ui,
        editor,
        crate::EditorCommand::HorizontalScroll(120.0),
        fx!(),
    );
    assert_eq!(document.scroll_x(editor), 120.0);
    document.perform(
        &mut store,
        &ui,
        editor,
        crate::EditorCommand::HorizontalScroll(1.0e9),
        fx!(),
    );
    assert!(
        document.scroll_x(editor) <= extent - 600.0 + 1.0,
        "the pan clamps to the content: {}",
        document.scroll_x(editor)
    );
    document.perform(
        &mut store,
        &ui,
        editor,
        crate::EditorCommand::HorizontalScroll(-1.0e9),
        fx!(),
    );
    assert_eq!(document.scroll_x(editor), 0.0);

    document.perform(
        &mut store,
        &ui,
        editor,
        crate::EditorCommand::ToggleSoftwrap,
        fx!(),
    );
    assert!(document.softwrap(editor));
    assert_eq!(document.layout_width(editor), 600.0);
    assert_eq!(
        document.content_height(editor),
        wrapped_height,
        "wrapping back restores the heights exactly"
    );
}

#[test]
fn box_backgrounds_hug_their_content() {
    let width = 800.0;

    let json = include_str!("../assets/theme.json").replace("#401f2937", "#ff2fc142");
    let theme = crate::theme::Theme::from_json(&json).expect("test theme parses");
    let source = "```\nshort\nlonger code line\n```\nplain filler text that runs much much much longer than any code line above\n";
    let mut document = crate::test_document::fenced_code_document(source);
    let editor = document.add_editor(
        width,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let fence_end = source.rfind("```").expect("fence") + 4;
    let layout = &document.editor(editor).layout;
    let box_extent = layout.max_width_in(0..fence_end as u32);
    let document_max = layout.max_width();
    assert!(box_extent > 0.0, "the code lines carry widths");
    assert!(
        box_extent + 30.0 < document_max,
        "the plain line is wider than the box: box {box_extent}, document {document_max}"
    );

    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 300)).expect("surface");
    let canvas = surface.canvas();
    canvas.clear(skia_safe::Color::WHITE);
    document.paint(
        editor,
        canvas,
        skia_safe::Rect::from_xywh(0.0, 0.0, 800.0, 300.0),
        false,
        &test_fonts(),
        &theme,
    );
    let image = surface.image_snapshot();
    let pixmap = image.peek_pixels().expect("raster pixels");
    let bytes = pixmap.bytes().expect("pixel bytes");
    let row_bytes = image.width() as usize * 4;
    let green_column = |x: usize| -> bool {
        (0..image.height() as usize).any(|y| {
            let px = &bytes[y * row_bytes + x * 4..y * row_bytes + x * 4 + 4];
            px[1] > 150 && px[0] < 120 && px[2] < 120
        })
    };
    let right_edge = (0..800).rev().find(|x| green_column(*x));
    let right_edge = right_edge.expect("the box painted") as f32;
    assert!(
        (right_edge - box_extent).abs() < 3.0,
        "the box's right edge sits at the content extent: edge {right_edge}, extent {box_extent}"
    );
    assert!(
        right_edge + 30.0 < document_max,
        "the box ignores the wide plain line: edge {right_edge}, document {document_max}"
    );
}

#[test]
fn the_width_mirror_tracks_the_widest_line_through_edits() {
    let width = 100_000.0;
    let mut document = plain_document("short\nmedium line here\ntiny\n");
    let extras: Vec<_> = document.document_scoped_markups().collect();
    let mut layout = DocumentLayout::build_complete(
        document.text(),
        crate::markup::OverlaidMarkup::new(document.markup(), &extras),
        width,
        &test_fonts(),
        &test_theme(),
        None,
    );
    drop(extras);
    let base = layout.max_width();
    assert!(base > 0.0, "laid lines carry widths");
    let tiny_start = "short\nmedium line here\n".len() as u32;
    assert!(
        layout.max_width_in(tiny_start..tiny_start + 4) < base,
        "the tiny line's range answers narrower than the document"
    );
    assert_eq!(layout.max_width_in(0..u32::MAX), base);

    let long =
        "an unreasonably long line that dwarfs every other line in this document by a margin\n";
    let insert = Operation::from_ops([
        Op::Insert(long.to_owned()),
        Op::Retain(text_string(&document).len() as u32),
    ]);
    layout.edit(&insert);
    document.edit(&insert, &test_fonts(), &test_theme(), fx!());
    let extras: Vec<_> = document.document_scoped_markups().collect();
    layout.repair_layout(
        document.text(),
        crate::markup::OverlaidMarkup::new(document.markup(), &extras),
        width,
        &test_fonts(),
        &test_theme(),
        0,
    );
    drop(extras);
    let grown = layout.max_width();
    assert!(
        grown > base * 2.0,
        "the long line raises the max: {base} -> {grown}"
    );

    let delete = Operation::from_ops([
        Op::Delete(long.to_owned()),
        Op::Retain((text_string(&document).len() - long.len()) as u32),
    ]);
    layout.edit(&delete);
    document.edit(&delete, &test_fonts(), &test_theme(), fx!());
    let extras: Vec<_> = document.document_scoped_markups().collect();
    layout.repair_layout(
        document.text(),
        crate::markup::OverlaidMarkup::new(document.markup(), &extras),
        width,
        &test_fonts(),
        &test_theme(),
        0,
    );
    drop(extras);
    assert!(
        (layout.max_width() - base).abs() < 0.5,
        "the max shrinks back when the widest line dies: {} vs {base}",
        layout.max_width()
    );

    let extras: Vec<_> = document.document_scoped_markups().collect();
    let fresh = DocumentLayout::build_complete(
        document.text(),
        crate::markup::OverlaidMarkup::new(document.markup(), &extras),
        width,
        &test_fonts(),
        &test_theme(),
        None,
    );
    assert!(
        (fresh.max_width() - layout.max_width()).abs() < 0.5,
        "the repaired mirror converges to a from-scratch layout"
    );
}

#[test]
fn repaired_layout_matches_fresh_layout_after_plain_insert() {
    let width = 320.0;
    let mut document = plain_document("alpha beta gamma delta\n\nsecond paragraph stays here");
    let mut layout = layout_of(&document, width);
    let operation = Operation::from_ops([
        Op::Retain("alpha ".len() as u32),
        Op::Insert("inserted words that should wrap ".to_owned()),
    ]);

    let repair_start = layout.edit(&operation);
    document.edit(&operation, &test_fonts(), &test_theme(), fx!());
    let extras: Vec<_> = document.document_scoped_markups().collect();
    layout.repair_layout(
        document.text(),
        crate::markup::OverlaidMarkup::new(document.markup(), &extras),
        width,
        &test_fonts(),
        &test_theme(),
        repair_start,
    );

    let fresh = layout_of(&document, width);

    assert_eq!(layout.layout_width(), fresh.layout_width());
    assert_eq!(layout.height(), fresh.height());
    assert!(layout
        .cursor_at_byte(document.text().byte_count().min(u32::MAX as usize) as u32)
        .is_some());
}

#[test]
fn code_block_with_emoji_keeps_layout_ranges_on_utf8_boundaries_after_edit() {
    let width = 220.0;
    let mut document = fenced_code_document(code_block_with_emoji());
    let mut layout = layout_of(&document, width);
    assert_layout_ranges_are_utf8(&document, &layout);

    let insert_at = code_block_with_emoji()
        .find("abcdefghijklmnopqrstuvwxyz")
        .expect("sample marker") as u32;
    let operation =
        Operation::from_ops([Op::Retain(insert_at), Op::Insert("😀 inserted ".to_owned())]);

    let repair_start = layout.edit(&operation);
    document.edit(&operation, &test_fonts(), &test_theme(), fx!());
    let extras: Vec<_> = document.document_scoped_markups().collect();
    layout.repair_layout(
        document.text(),
        crate::markup::OverlaidMarkup::new(document.markup(), &extras),
        width,
        &test_fonts(),
        &test_theme(),
        repair_start,
    );
    assert_layout_ranges_are_utf8(&document, &layout);

    let fresh = layout_of(&document, width);
    assert_eq!(layout.height(), fresh.height());
}

#[test]
fn newline_is_a_hard_break() {
    let document = plain_document("first\nsecond\n");
    let layout = layout_of(&document, 800.0);

    assert_eq!(
        layout_ranges(&document, &layout),
        vec!["first\n", "second\n", ""],
        "two hard lines plus the trailing newline's empty last line"
    );
}

#[test]
fn blank_line_keeps_its_height() {
    let flat = layout_of(&plain_document("first\nsecond"), 800.0);
    let spaced = layout_of(&plain_document("first\n\nsecond"), 800.0);

    assert!(spaced.height() > flat.height());
}

#[test]
fn hidden_interval_takes_no_space() {
    let width = 200.0;
    let source = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi";
    let visible_end = source.find(" epsilon").expect("marker");

    let hidden_document = hidden_document(source, visible_end as u32..source.len() as u32);
    let full = layout_of(&plain_document(source), width);
    let hidden = layout_of(&hidden_document, width);
    let truncated = layout_of(&plain_document(&source[..visible_end]), width);

    assert_eq!(text_string(&hidden_document), source);
    assert!(hidden.height() < full.height());
    assert_eq!(hidden.height(), truncated.height());
}

#[test]
fn caret_hit_testing_maps_emoji_position_to_source_bytes() {
    let source = "a😀b";
    let view = view_of(plain_document(source), 800.0);
    let target_utf16 = "a😀".encode_utf16().count() as i32;
    let x = x_for_paragraph_position(source, target_utf16);

    let byte = view
        .document
        .byte_at_point(view.editor, x, 10.0, &test_fonts(), &test_theme())
        .expect("caret hit");

    assert_eq!(byte, "a😀".len() as u32);
}

#[test]
fn caret_hit_testing_moves_inside_visible_whitespace() {
    let source = "a    b";
    let view = view_of(plain_document(source), 800.0);
    let target_utf16 = "a  ".encode_utf16().count() as i32;
    let x = x_for_paragraph_position(source, target_utf16);

    let byte = view
        .document
        .byte_at_point(view.editor, x, 10.0, &test_fonts(), &test_theme())
        .expect("caret hit");

    assert_eq!(byte, "a  ".len() as u32);
}

fn layout_of(document: &Document, width: f32) -> DocumentLayout {
    let extras: Vec<_> = document.document_scoped_markups().collect();
    DocumentLayout::build(
        document.text(),
        crate::markup::OverlaidMarkup::new(document.markup(), &extras),
        width,
        &test_fonts(),
        &test_theme(),
        None,
    )
}

fn view_of(document: Document, width: f32) -> EditorView {
    EditorView::of_document(document, width, &test_fonts(), &test_theme())
}

fn text_string(document: &Document) -> String {
    let mut view = document.text().view();
    view.byte_string(0, view.byte_count())
}

fn layout_ranges(document: &Document, layout: &DocumentLayout) -> Vec<String> {
    let (mut cursor, _, mut byte_start) = layout.cursor_at_y(0.0);
    let mut view = document.text().view();
    let mut ranges = Vec::new();

    loop {
        let item = cursor.element();
        let byte_end = byte_start.saturating_add(item.byte_size);
        ranges.push(view.byte_string(byte_start as usize, byte_end as usize));
        byte_start = byte_end;

        if !cursor.advance() {
            break;
        }
    }

    ranges
}

fn code_block_with_emoji() -> &'static str {
    "```rust\nlet x = \"😀😀😀😀😀\"; abcdefghijklmnopqrstuvwxyz\n```\n\noutside"
}

fn assert_layout_ranges_are_utf8(document: &Document, layout: &DocumentLayout) {
    let (mut cursor, _, mut byte_start) = layout.cursor_at_y(0.0);
    let mut view = document.text().view();
    loop {
        let item = cursor.element();
        let byte_end = byte_start.saturating_add(item.byte_size);
        let mut bytes = Vec::new();
        view.byte_range_into(byte_start as usize, byte_end as usize, &mut bytes);
        assert!(
            str::from_utf8(&bytes).is_ok(),
            "layout range {byte_start}..{byte_end} is not UTF-8"
        );
        byte_start = byte_end;

        if !cursor.advance() {
            break;
        }
    }

    assert_eq!(byte_start as usize, document.text().byte_count());
}

fn x_for_paragraph_position(text: &str, position: i32) -> f32 {
    let fonts = font_collection();
    let mut paragraph = line_paragraph(&BlockStyle::default(), text, fonts, &test_theme(), &[]);
    paragraph.layout(800.0);
    let width = paragraph.longest_line().ceil().max(1.0) as usize;

    for step in 0..=width * 4 {
        let x = step as f32 * 0.25;
        let hit = paragraph.get_glyph_position_at_coordinate(Point::new(x, 10.0));
        if hit.position == position {
            return x;
        }
    }

    panic!("could not find paragraph x for position {position}");
}

fn font_collection() -> FontCollection {
    crate::embedded_fonts::collection()
}

#[test]
fn misaligned_boundary_detector_fires() {
    use text::Text;

    let document = Document::new(
        Text::from_string_exact("aa\u{1F600}aa\nbb\u{1F600}bb\n"),
        crate::markup::Markup::builder().finish(),
    );
    let layout = layout_of(&document, 300.0);
    assert_eq!(layout.find_misaligned_boundary(document.text()), None);

    let shifted = Text::from_string_exact("\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}");
    assert!(layout.find_misaligned_boundary(&shifted).is_some());
}

fn ime_doc(source: &str) -> (Document, crate::editor::EditorId) {
    let mut document = plain_document(source);
    let editor = document.add_editor(
        600.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    (document, editor)
}

fn text_of(document: &Document) -> String {
    let count = document.text().byte_count();
    document.text().view().byte_string(0, count)
}

#[test]
fn shaped_lines_are_retained_across_builds() {
    use std::rc::Rc;
    let (mut document, editor) = ime_doc("alpha\nbeta\ngamma\n");
    let build = |document: &Document| {
        crate::viewport::EditorViewport::build(
            document,
            editor,
            0.0..10_000.0,
            false,
            false,
            None,
            &test_fonts(),
            &test_theme(),
        )
    };
    let first = build(&document);
    let second = build(&document);
    assert!(!first.lines.is_empty());
    for (a, b) in first.lines.iter().zip(&second.lines) {
        match (&a.shaped, &b.shaped) {
            (Some(a), Some(b)) => assert!(Rc::ptr_eq(a, b), "a still frame shapes nothing"),
            (None, None) => {}
            _ => panic!("shape presence changed between identical builds"),
        }
    }

    document.set_caret(editor, 0);
    document.insert(editor, "x", &test_fonts(), &test_theme(), fx!());
    let third = build(&document);
    assert!(
        !Rc::ptr_eq(
            second.lines[0].shaped.as_ref().expect("shaped"),
            third.lines[0].shaped.as_ref().expect("shaped"),
        ),
        "the edit re-shaped"
    );

    let markup = document.add_markup();
    document.show_markup(editor, markup);
    let fourth = build(&document);
    assert!(
        !Rc::ptr_eq(
            third.lines[0].shaped.as_ref().expect("shaped"),
            fourth.lines[0].shaped.as_ref().expect("shaped"),
        ),
        "a pick change re-shaped"
    );
}

#[test]
fn repairs_preserve_the_trailing_empty_line() {
    let document = plain_document("a\nb\nc\n");
    let extras: Vec<_> = document.document_scoped_markups().collect();
    let markup = crate::markup::OverlaidMarkup::new(document.markup(), &extras);
    let fresh = layout_of(&document, 600.0);
    let heights = fresh.element_heights();
    assert_eq!(
        heights.len(),
        4,
        "three lines and the empty last: {heights:?}"
    );

    for damage_at in [0u32, 2, 4, 5] {
        let mut layout = fresh.clone();
        let start = layout.mark_modified(damage_at);
        layout.repair_layout(
            document.text(),
            markup,
            600.0,
            &test_fonts(),
            &test_theme(),
            start,
        );
        assert_eq!(
            layout.element_heights(),
            heights,
            "full repair from byte {damage_at} converges"
        );
    }

    let mut layout = fresh.clone();
    let start = layout.mark_modified_in(0..6);
    let mut resume = start;
    for _ in 0..16 {
        layout.repair_layout_bounded(
            document.text(),
            markup,
            600.0,
            &test_fonts(),
            &test_theme(),
            resume,
            1.0,
        );
        match layout.repair_pending() {
            Some(pending) => resume = pending,
            None => break,
        }
    }
    assert_eq!(
        layout.element_heights(),
        heights,
        "budgeted repairs converge on the fresh layout"
    );
}

#[test]
fn backspacing_a_document_to_nothing_settles_clean() {
    for source in ["hello\nworld\n", "line one\nline two", "\n\n", "abc"] {
        let document = plain_document(source);
        let extras: Vec<_> = document.document_scoped_markups().collect();
        let markup = crate::markup::OverlaidMarkup::new(document.markup(), &extras);
        let mut layout = layout_of(&document, 600.0);
        let mut text = source.to_string();

        while !text.is_empty() {
            let last = text.chars().last().expect("char");
            let at = (text.len() - last.len_utf8()) as u32;
            let op = operation::Operation::delete_at(at, last.to_string());
            layout.edit(&op);
            text.truncate(at as usize);
            let value = text::Text::from_string_exact(&text);
            let mut rounds = 0;
            while let Some(pending) = layout.repair_pending() {
                rounds += 1;
                assert!(
                    rounds < 64,
                    "src={source:?} at {at}: chain did not terminate"
                );
                layout.repair_layout_bounded(
                    &value,
                    markup,
                    600.0,
                    &test_fonts(),
                    &test_theme(),
                    pending,
                    crate::document_layout::SYNC_LAYOUT_HEIGHT,
                );
            }
        }

        let empty = text::Text::from_string_exact("");
        let anchor = layout.mark_modified_in(0..0);
        let mut rounds = 0;
        while let Some(pending) = layout.repair_pending() {
            rounds += 1;
            assert!(
                rounds < 64,
                "src={source:?}: resize chain did not terminate"
            );
            let _ = anchor;
            layout.repair_layout_bounded(
                &empty,
                markup,
                600.0,
                &test_fonts(),
                &test_theme(),
                pending,
                crate::document_layout::SYNC_LAYOUT_HEIGHT,
            );
        }
    }
}

#[test]
fn deleting_everything_leaves_no_unhealable_damage() {
    for source in ["hello\n", "a\nb\n", "\n", "hello"] {
        let document = plain_document(source);
        let extras: Vec<_> = document.document_scoped_markups().collect();
        let markup = crate::markup::OverlaidMarkup::new(document.markup(), &extras);
        let mut layout = layout_of(&document, 600.0);
        layout.edit(&operation::Operation::delete_at(0, source));
        assert_eq!(
            layout.repair_pending(),
            None,
            "{source:?}: an emptied layout keeps no damage"
        );

        let mut stale = layout_of(&plain_document("hello world"), 600.0);
        let mut anchor = stale.mark_modified(0);
        for _ in 0..8 {
            stale.repair_layout_bounded(
                document.text(),
                markup,
                600.0,
                &test_fonts(),
                &test_theme(),
                anchor,
                crate::document_layout::SYNC_LAYOUT_HEIGHT,
            );
            match stale.repair_pending() {
                Some(pending) => anchor = pending,
                None => break,
            }
        }
        assert_eq!(
            stale.repair_pending(),
            None,
            "{source:?}: the repair chain terminates — damage a pass cannot \
             reach is dropped, never re-armed at the same byte"
        );
    }
}

#[test]
fn trailing_newline_gets_its_own_last_line() {
    let (mut document, editor) = ime_doc("a\nb\n");
    let heights = document.element_heights(editor);
    assert_eq!(
        heights.len(),
        3,
        "the empty last line is its own row: {heights:?}"
    );
    let (last_start, last_height) = *heights.last().expect("the phantom row");
    assert_eq!(last_start, 4, "the row starts at the document end");
    assert!(last_height > 0.0, "the row has a text line's height");
    let (populated, populated_editor) = ime_doc("a\nb\nx");
    let populated_heights = populated.element_heights(populated_editor);
    assert_eq!(
        heights, populated_heights,
        "typing on the empty last line must not move or resize any row"
    );
    let (document2, editor2) = ime_doc("a\nb");
    assert!(
        document.content_height(editor) > document2.content_height(editor2),
        "the trailing newline adds a line of height"
    );

    let (x, y, _, height) = document
        .caret_content_rect(editor, 4, &test_fonts(), &test_theme())
        .expect("the EOF caret has a rect");
    assert_eq!(x, 0.0, "the EOF caret sits at the line start");
    let empty_row_top = heights[0].1 + heights[1].1;
    assert!(
        y >= empty_row_top && y < empty_row_top + last_height,
        "the EOF caret sits on the empty last line: y={y}, row={empty_row_top}..{}",
        empty_row_top + last_height
    );
    assert!(height > 0.0);
    let (_, populated_y, _, populated_height) = populated
        .caret_content_rect(populated_editor, 5, &test_fonts(), &test_theme())
        .expect("the populated EOF caret has a rect");
    let populated_row_top = populated_heights
        .iter()
        .take(2)
        .map(|(_, height)| height)
        .sum::<f32>();
    assert_eq!(
        (y, height),
        (populated_y, populated_height),
        "typing on the empty last line must not change its vertical position or caret height; \
         row tops were {empty_row_top} and {populated_row_top}"
    );

    let clicked = document
        .caret_at_point_in(
            editor,
            3.0,
            y + 2.0,
            0.0,
            0.0,
            0.0,
            &test_fonts(),
            &test_theme(),
        )
        .expect("the empty row is clickable");
    assert_eq!(clicked, 4);

    document.set_caret(editor, 2);
    document.move_carets_vertically(editor, true, false, &test_fonts(), &test_theme());
    assert_eq!(
        document.caret_byte(editor),
        4,
        "down lands on the empty last line"
    );
}

#[test]
fn composing_then_committing_marked_text() {
    let (mut document, editor) = ime_doc("ab");
    document.set_caret(editor, 1);

    document.set_marked_text(editor, "k", 1..1, None, &test_fonts(), &test_theme(), fx!());
    assert_eq!(text_of(&document), "akb");
    assert_eq!(document.marked_range(editor), Some(1..2));
    assert_eq!(document.caret_byte(editor), 2);

    document.set_marked_text(
        editor,
        "\u{304B}\u{3093}",
        6..6,
        None,
        &test_fonts(),
        &test_theme(),
        fx!(),
    );
    assert_eq!(text_of(&document), "a\u{304B}\u{3093}b");
    assert_eq!(document.marked_range(editor), Some(1..7));

    document.insert(editor, "\u{611F}", &test_fonts(), &test_theme(), fx!());
    assert_eq!(text_of(&document), "a\u{611F}b");
    assert!(!document.marked_range(editor).is_some());
    assert_eq!(document.caret_byte(editor), 1 + "\u{611F}".len() as u32);
}

#[test]
fn unmark_keeps_the_text_and_ends_composition() {
    let (mut document, editor) = ime_doc("");
    document.set_marked_text(
        editor,
        "cafe\u{301}",
        0..0,
        None,
        &test_fonts(),
        &test_theme(),
        fx!(),
    );
    assert!(document.marked_range(editor).is_some());
    document.unmark_text(editor);
    assert!(!document.marked_range(editor).is_some());
    assert_eq!(text_of(&document), "cafe\u{301}");
}

#[test]
fn a_marked_range_shifts_when_a_sibling_editor_edits() {
    let mut document = plain_document("hello world");
    let composing = document.add_editor(
        600.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let sibling = document.add_editor(
        600.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );

    document.set_caret(composing, "hello world".len() as u32);
    document.set_marked_text(
        composing,
        "xy",
        2..2,
        None,
        &test_fonts(),
        &test_theme(),
        fx!(),
    );
    let marked = document.marked_range(composing).expect("composing");

    document.set_caret(sibling, 0);
    let inserted = "PREFIX ";
    document.insert(sibling, inserted, &test_fonts(), &test_theme(), fx!());
    assert_eq!(
        document.marked_range(composing),
        Some(marked.start + inserted.len() as u32..marked.end + inserted.len() as u32),
    );
}

#[test]
fn utf16_and_byte_offsets_round_trip() {
    let (document, _) = ime_doc("a\u{1F600}\u{00E9}");
    assert_eq!(document.text().view().byte_to_utf16(0), 0);
    assert_eq!(document.text().view().byte_to_utf16(1), 1);
    assert_eq!(document.text().view().byte_to_utf16(5), 3);
    assert_eq!(document.text().view().utf16_to_byte(0), 0);
    assert_eq!(document.text().view().utf16_to_byte(1), 1);
    assert_eq!(document.text().view().utf16_to_byte(3), 5);
    assert_eq!(document.text().view().substring(1..5), "\u{1F600}");
}

#[test]
fn caret_rect_tracks_the_composing_caret() {
    let (mut document, editor) = ime_doc("hello");

    let (x0, y0, _, h0) = document
        .caret_content_rect(editor, 0, &test_fonts(), &test_theme())
        .expect("rect at 0");
    let (x3, _, _, _) = document
        .caret_content_rect(editor, 3, &test_fonts(), &test_theme())
        .expect("rect at 3");
    assert!(x0 < x3, "caret advances rightward: {x0} < {x3}");
    assert!(y0 >= 0.0 && h0 > 0.0, "a real line rect");

    document.set_marked_text(
        editor,
        "\u{304B}",
        3..3,
        None,
        &test_fonts(),
        &test_theme(),
        fx!(),
    );
    let caret = document.caret_byte(editor);
    assert!(document
        .caret_content_rect(editor, caret, &test_fonts(), &test_theme())
        .is_some());
}

#[test]
fn the_caret_sizes_to_the_run_not_the_line_box() {
    let (document, editor) = ime_doc("hello world");
    let theme = test_theme();
    let fonts = test_fonts();
    let (_, top, _, height) = document
        .caret_content_rect(editor, 3, &fonts, &theme)
        .expect("mid-word caret");

    let element_height = document.content_height(editor);
    assert!(
        height < element_height,
        "the caret ({height}) must sit inside the line box ({element_height})"
    );
    assert!(height > 6.0, "a real glyph-sized caret, got {height}");
    assert!(top >= 0.0, "the caret starts inside the line, top {top}");
}

#[test]
fn a_focused_empty_document_has_and_paints_a_caret() {
    let (document, editor) = ime_doc("");
    let fonts = test_fonts();
    let theme = test_theme();
    assert_eq!(
        document.content_height(editor),
        0.0,
        "an empty document deliberately has no layout rows"
    );

    let (_, top, width, height) = document
        .caret_content_rect(editor, 0, &fonts, &theme)
        .expect("the empty document still has caret geometry");
    assert!(top >= 0.0 && width > 0.0 && height > 0.0);

    let mut surface = skia_safe::surfaces::raster_n32_premul((40, 40)).expect("surface");
    let background = skia_safe::Color::from_rgb(1, 2, 3);
    surface.canvas().clear(background);
    document.paint(
        editor,
        surface.canvas(),
        skia_safe::Rect::from_wh(40.0, 40.0),
        true,
        &fonts,
        &theme,
    );

    let image = surface.image_snapshot();
    let pixmap = image.peek_pixels().expect("raster pixels");
    let caret = theme.ui().caret.color.0;
    let painted = (0..40).any(|y| (0..40).any(|x| pixmap.get_color((x, y)) == caret));
    assert!(painted, "the focused empty document paints its caret");
}

#[test]
fn typing_into_an_empty_document_shows_the_text() {
    let (mut document, editor) = ime_doc("");
    document.insert(editor, "a", &test_fonts(), &test_theme(), fx!());
    assert_eq!(text_of(&document), "a", "the byte is inserted");
    assert!(
        document.content_height(editor) > 0.0,
        "the inserted line must have height (it must render)"
    );
}

#[test]
fn a_bounded_editor_lays_out_exactly_its_fragment() {
    let mut document = plain_document("alpha\nbravo\ncharlie\ndelta\n");
    let set = document.add_fragment_set();
    let key = document.add_fragment(set, 6..20);
    let mut batch = imba::effect::Batch::new();
    let editor = document.add_editor(
        400.0,
        Some(key),
        crate::document::EditorBuild::Bounded,
        &[],
        &test_fonts(),
        &test_theme(),
        &mut batch.effects(),
    );
    assert!(
        batch.is_empty(),
        "a fragment-sized layout fits the synchronous budget"
    );

    let reference = crate::EditorView::complete(
        plain_document("bravo\ncharlie"),
        400.0,
        &test_fonts(),
        &test_theme(),
    );
    assert!(
        (document.content_height(editor) - reference.content_height()).abs() < 0.5,
        "bounded height {} vs reference {}",
        document.content_height(editor),
        reference.content_height()
    );

    let view = crate::EditorView {
        document: document.clone(),
        editor,
        reports_geometry: false,
        location: None,
        gutter_width: 0.0,
        base: None,
    };
    assert_eq!(view.find_misaligned_boundary(), None);

    assert_eq!(document.caret_byte(editor), 6);
    document.set_caret(editor, 0);
    assert_eq!(
        document.caret_byte(editor),
        6,
        "clamped to the window start"
    );
    document.set_caret(editor, 25);
    assert_eq!(document.caret_byte(editor), 20, "clamped to the window end");
}

#[test]
fn a_bounded_editor_edits_the_shared_document_and_tracks_shifts() {
    let mut document = plain_document("alpha\nbravo\ncharlie\ndelta\n");
    let set = document.add_fragment_set();
    let key = document.add_fragment(set, 6..20);
    let bounded = document.add_editor(
        400.0,
        Some(key),
        crate::document::EditorBuild::Bounded,
        &[],
        &test_fonts(),
        &test_theme(),
        fx!(),
    );
    let whole = document.add_editor(
        400.0,
        None,
        crate::document::EditorBuild::Bounded,
        &[],
        &test_fonts(),
        &test_theme(),
        fx!(),
    );

    document.set_caret(bounded, 7);
    let _ = document.insert(bounded, "X", &test_fonts(), &test_theme(), fx!());
    let end = document.text().byte_count() as u32;
    assert_eq!(
        document.text().view().substring(0..end),
        "alpha\nbXravo\ncharlie\ndelta\n"
    );
    assert_eq!(
        document.fragment_range(key),
        Some(6..21),
        "the fragment grew around the insert"
    );

    let before = document.content_height(bounded);
    document.set_caret(whole, 0);
    let _ = document.insert(whole, "> ", &test_fonts(), &test_theme(), fx!());
    assert_eq!(
        document.fragment_range(key),
        Some(8..23),
        "the fragment shifted under the edit"
    );
    assert!((document.content_height(bounded) - before).abs() < 0.5);

    document.set_caret(bounded, 9);
    let _ = document.insert(bounded, "\n", &test_fonts(), &test_theme(), fx!());
    let reference = crate::EditorView::complete(
        plain_document("b\nXravo\ncharlie"),
        400.0,
        &test_fonts(),
        &test_theme(),
    );
    assert!(
        (document.content_height(bounded) - reference.content_height()).abs() < 0.5,
        "grew to three lines: {} vs {}",
        document.content_height(bounded),
        reference.content_height()
    );

    let view = crate::EditorView {
        document: document.clone(),
        editor: bounded,
        reports_geometry: false,
        location: None,
        gutter_width: 0.0,
        base: None,
    };
    assert_eq!(view.find_misaligned_boundary(), None);
}

#[test]
fn a_bounded_editor_paints_only_its_fragment() {
    let source = format!(
        "{}THE-FRAGMENT-LINE\n{}",
        "before ".repeat(400),
        "after ".repeat(400)
    );
    let start = source.find("THE-FRAGMENT-LINE").unwrap() as u32;
    let end = start + "THE-FRAGMENT-LINE\n".len() as u32;
    let mut document = plain_document(&source);
    let set = document.add_fragment_set();
    let key = document.add_fragment(set, start..end);
    let editor = document.add_editor(
        400.0,
        Some(key),
        crate::document::EditorBuild::Bounded,
        &[],
        &test_fonts(),
        &test_theme(),
        fx!(),
    );
    let height = document.content_height(editor);
    assert!(height > 0.0 && height < 100.0, "one line: {height}");

    let extra = 120;
    let mut surface =
        skia_safe::surfaces::raster_n32_premul((400, height as i32 + extra)).expect("surface");
    let background = skia_safe::Color::from_rgb(1, 2, 3);
    surface.canvas().clear(background);
    document.paint(
        editor,
        surface.canvas(),
        skia_safe::Rect::from_wh(400.0, height + extra as f32),
        true,
        &test_fonts(),
        &test_theme(),
    );

    let image = surface.image_snapshot();
    let pixmap = image.peek_pixels().expect("raster pixels");
    let mut painted_below = 0;
    for y in (height as i32 + 8)..(height as i32 + extra) {
        for x in 0..400 {
            let color: skia_safe::Color = pixmap.get_color((x, y));
            if color != background {
                painted_below += 1;
            }
        }
    }
    assert_eq!(
        painted_below, 0,
        "nothing may paint below the fragment's {height}px"
    );
}

mod injected_syntax {
    use super::{test_fonts, test_theme};
    use crate::markup::Syntax;
    use crate::markup::{Markup, StyleId};
    use crate::test_document::plain_document;
    use operation::Operation;

    fn payload(tokens: &[(std::ops::Range<u32>, StyleId)]) -> Syntax {
        let mut markup = Markup::new();
        for (range, theme) in tokens {
            markup.push_styled(range.clone(), *theme);
        }
        Syntax::new("toy", None, markup)
    }

    fn attributes_on(
        document: &crate::document::Document,
        line: std::ops::Range<u32>,
    ) -> Vec<(std::ops::Range<u32>, StyleId)> {
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        document
            .markup()
            .marks_inline_hidden_in(line, &mut inline, &mut hidden);
        inline
            .iter()
            .filter_map(|interval| match interval.id {
                id @ (StyleId::Keyword
                | StyleId::String
                | StyleId::Comment
                | StyleId::Number
                | StyleId::Type
                | StyleId::Function
                | StyleId::Variable
                | StyleId::Constant
                | StyleId::Operator
                | StyleId::Punctuation
                | StyleId::Attribute
                | StyleId::Embedded) => Some((interval.range.clone(), id)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn injected_attributes_resolve_block_relative() {
        let mut document = plain_document("prefix let x tail");
        document.add_syntax(7..12, payload(&[(0..3, StyleId::Keyword)]));
        assert_eq!(
            attributes_on(&document, 0..17),
            vec![(7..10, StyleId::Keyword)],
            "block-relative 0..3 lands at document 7..10"
        );
    }

    #[test]
    fn markers_shift_and_inside_edits_forward() {
        let mut document = plain_document("prefix let x tail");
        let key = document.add_syntax(7..12, payload(&[(0..3, StyleId::Keyword)]));

        document.edit(
            &Operation::insert_at(0, "AA"),
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        assert_eq!(
            attributes_on(&document, 0..19),
            vec![(9..12, StyleId::Keyword)],
            "the token rides the shifted marker"
        );

        document.edit(
            &Operation::insert_at(13, "yy"),
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        assert_eq!(
            attributes_on(&document, 0..21),
            vec![(9..12, StyleId::Keyword)],
            "an insert past the token leaves it in place"
        );

        document.edit(
            &Operation::insert_at(10, "zz"),
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        assert_eq!(
            attributes_on(&document, 0..23),
            vec![(9..14, StyleId::Keyword)],
            "an insert inside the token grows it within the block"
        );
        let entry = document
            .markup()
            .syntax(key)
            .expect("payload survives inside edits");
        assert_eq!(entry.language, "toy");
    }

    #[test]
    fn boundary_edits_drop_the_payload() {
        let mut document = plain_document("prefix let x tail");
        let key = document.add_syntax(7..12, payload(&[(0..3, StyleId::Keyword)]));

        document.edit(
            &Operation::delete_at(5, "x l"),
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        assert!(
            document.markup().syntax(key).is_none(),
            "a boundary-crossing edit drops the entry"
        );
        assert_eq!(attributes_on(&document, 0..14), vec![]);
    }

    #[test]
    fn theme_colors_reach_the_glyphs() {
        let mut document = plain_document("prefix keyword tail");
        document.add_syntax(7..14, payload(&[(0..7, StyleId::Keyword)]));
        let editor = document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Bounded,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let height = document.content_height(editor);
        let mut surface =
            skia_safe::surfaces::raster_n32_premul((400, height as i32 + 4)).expect("surface");
        let background = skia_safe::Color::from_rgb(1, 2, 3);
        surface.canvas().clear(background);
        document.paint(
            editor,
            surface.canvas(),
            skia_safe::Rect::from_wh(400.0, height + 4.0),
            true,
            &test_fonts(),
            &test_theme(),
        );
        let image = surface.image_snapshot();
        let pixmap = image.peek_pixels().expect("raster pixels");
        let target = test_theme()
            .attributes(StyleId::Keyword)
            .color
            .expect("keyword entry has a color");
        let mut hits = 0;
        for y in 0..(height as i32) {
            for x in 0..400 {
                let color: skia_safe::Color = pixmap.get_color((x, y));
                if color == background {
                    continue;
                }
                let near = |a: u8, b: u8| a.abs_diff(b) < 60;
                if near(color.r(), target.r())
                    && near(color.g(), target.g())
                    && near(color.b(), target.b())
                {
                    hits += 1;
                }
            }
        }
        assert!(hits > 20, "keyword-colored pixels painted: {hits}");
    }

    #[test]
    fn reconcile_inherits_stable_keys_and_launches_only_fresh() {
        use crate::reparse::SyntaxSite;
        use intervals::IntervalQuery;
        let text = text::Text::from_string_exact("prefix let x tail");
        let site = SyntaxSite {
            range: 7..12,
            language: "toy".to_owned(),
        };

        let mut old = Markup::new();
        let minted = old.reconcile_syntaxes(&Markup::new(), std::slice::from_ref(&site), &text);
        assert_eq!(minted.len(), 1, "fresh id: first parse launches");
        let key = minted[0].key;
        assert!(old.set_syntax(key, payload(&[(0..3, StyleId::Keyword)])));

        let mut fresh = Markup::new();
        let dirty = fresh.reconcile_syntaxes(&old, std::slice::from_ref(&site), &text);
        assert!(dirty.is_empty(), "inherited entries never relaunch here");
        let adopted = fresh.syntax(key).expect("same key survives the rebuild");
        assert!(
            adopted
                .markup
                .query(0..5, intervals::Order::Ascending)
                .next()
                .is_some(),
            "the live payload rode over, not an empty mint"
        );

        let mut fresh = Markup::new();
        let dirty = fresh.reconcile_syntaxes(
            &old,
            &[SyntaxSite {
                range: 0..6,
                language: "toy".to_owned(),
            }],
            &text,
        );
        assert_eq!(dirty.len(), 1);
        assert_ne!(dirty[0].key, key);
        assert_eq!(dirty[0].range, 0..6, "a fresh mint carries its range");
        assert!(
            fresh.syntax(key).is_none(),
            "the deleted block's entry was dropped, not leaked"
        );
    }

    #[test]
    fn injections_recurse() {
        let mut inner = payload(&[]);
        inner
            .markup
            .add_syntax(1..4, payload(&[(0..2, StyleId::String)]));
        let mut document = plain_document("prefix let x tail");
        document.add_syntax(7..12, inner);
        assert_eq!(
            attributes_on(&document, 0..17),
            vec![(8..10, StyleId::String)],
            "nested payloads translate through both bases"
        );
    }
}

#[test]
fn typing_in_a_blank_line_free_document_repairs_one_line() {
    let line = "0000 :: lorem ipsum dolor sit amet consectetur adipiscing elit :: 00\n";
    let source = line.repeat(50_000);
    let fonts = test_fonts();
    let mut document = Document::new(
        text::Text::from_string_exact(&source),
        crate::markup::Markup::new(),
    );
    let _editor = document.add_editor(
        700.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );

    let at = (source.len() / 2) as u32;
    let mut samples = Vec::new();
    for i in 0..12u32 {
        let started = std::time::Instant::now();
        let _ = document.edit(
            &operation::Operation::insert_at(at + i, "x"),
            &fonts,
            &test_theme(),
            fx!(),
        );
        samples.push(started.elapsed());
    }
    samples.sort();
    let p50 = samples[samples.len() / 2];
    imba::perf::record(
        "plain-typing",
        "keystroke_p50_ms",
        p50.as_secs_f64() * 1000.0,
    );
    assert!(
        p50 < std::time::Duration::from_millis(50),
        "a keystroke in a blank-line-free document must repair one line, \
         not the document: p50 {p50:?}"
    );
}

#[test]
fn styled_runs_translate_across_hidden_syntax() {
    let raw = "some **bold** words";

    let hidden = vec![5u32..7, 11..13];
    let display = crate::shaped_line::DisplayText::build(false, raw, &hidden, false);
    assert_eq!(display.as_str(), "some bold words");
    let decorations = vec![crate::markup::TextDecorationInterval {
        range: 7..11,
        id: crate::markup::StyleId::Strong,
    }];
    let mapped = display.map_decorations(&decorations);
    assert_eq!(mapped.len(), 1);
    assert_eq!(
        &display.as_str()[mapped[0].range.start as usize..mapped[0].range.end as usize],
        "bold",
        "the run covers exactly the visible word"
    );
}

#[test]
fn paired_layouts_align_retained_boundaries() {
    let fonts = test_fonts();
    let theme = crate::theme::Theme::embedded();
    let shared_head = "shared first line\nshared second line\n";
    let shared_tail = "shared tail one\nshared tail two\nshared tail three\n";
    let left_source = format!(
        "{shared_head}left only: a very long paragraph that certainly wraps a few times at a narrow layout width, words words words words words\n{shared_tail}"
    );
    let right_source = format!("{shared_head}{shared_tail}appended right line\n");

    let mut left = crate::test_document::plain_document(&left_source);
    let left_editor = left.add_editor(
        240.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let mut right = crate::test_document::plain_document(&right_source);
    let right_editor = right.add_editor(
        240.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let diff = crate::diff::diff(left.text(), right.text());
    let left_len = left_source.len() as u32;
    {
        let mut left_layout = left.editors.get(&left_editor).unwrap().layout.clone();
        let mut right_layout = right.editors.get(&right_editor).unwrap().layout.clone();
        let _ = crate::split_diff::align::sync_spacers(
            &mut left_layout,
            &mut right_layout,
            &diff,
            0..left_len,
            usize::MAX,
            &mut 0,
        );

        let mut aligned = 0;
        for range in left_layout.element_byte_ranges() {
            let boundary = range.start;
            if boundary == 0 {
                continue;
            }

            if range.is_empty() {
                continue;
            }

            let mapped = diff.transform_offset(boundary, operation::Bias::Right);
            if diff.transform_offset_back(mapped, operation::Bias::Right) != boundary {
                continue;
            }
            let right_boundary = right_layout
                .element_byte_ranges()
                .iter()
                .any(|r| r.start == mapped);
            if !right_boundary {
                continue;
            }
            aligned += 1;
            let left_top = left_layout.height_before(boundary) as i64
                + spacer_at(&left_layout, boundary) as i64;
            let right_top =
                right_layout.height_before(mapped) as i64 + spacer_at(&right_layout, mapped) as i64;
            assert_eq!(
                left_top, right_top,
                "boundary {boundary}↔{mapped} must share a content top"
            );
        }
        assert!(
            aligned >= 3,
            "the shared runs produce aligned pairs: {aligned}"
        );

        let before: Vec<_> = left_layout.element_heights();
        let _ = crate::split_diff::align::sync_spacers(
            &mut left_layout,
            &mut right_layout,
            &diff,
            0..left_len,
            usize::MAX,
            &mut 0,
        );
        assert_eq!(before, left_layout.element_heights());
    }
}

fn spacer_at(layout: &crate::document_layout::DocumentLayout, byte: u32) -> f32 {
    for (range, spacer) in layout
        .element_byte_ranges()
        .into_iter()
        .zip(layout.element_spacers())
    {
        if range.start == byte {
            return spacer;
        }
    }
    0.0
}

#[test]
fn popup_overlays_carry_projected_inlays() {
    // The aggregated overlay path (list rows, the workbench pane —
    // everything riding `EditorView::popup_overlays` instead of the
    // editor chain) must emit host-targeting inlays too, or a
    // deleted-code card expanded from the gutter stripes never
    // renders.
    let fonts = test_fonts();
    let theme = crate::theme::Theme::embedded();
    let source = "alpha line\nbeta line\ngamma line\ndelta line\n";
    let mut document = crate::test_document::plain_document(source);
    let markup = crate::markup::MarkupId::mint();
    document.ensure_document_markup(markup);
    document.push_inlay(
        markup,
        11..12,
        crate::markup::Inlay::new(
            crate::markup::InlayMode::Above,
            FixedInlay {
                width: 60.0,
                height: 40.0,
            },
        )
        .over(crate::markup::INLAY_HOST),
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let editor = document.add_editor(
        240.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let view = crate::EditorView {
        document,
        editor,
        reports_geometry: false,
        location: None,
        gutter_width: 0.0,
        base: None,
    };
    let store = imba::Store::new();
    let ui = imba::UiCtx::cold();
    ui.set(crate::env::UiFonts(test_fonts()));
    let arena = imba::arena::Arena::default();
    let overlays = view.popup_overlays(
        &arena,
        &store,
        &ui,
        240.0,
        skia_safe::Rect::from_wh(240.0, 400.0),
    );
    let projected: Vec<_> = overlays
        .iter()
        .filter(|overlay| overlay.host == crate::markup::INLAY_HOST)
        .collect();
    assert_eq!(projected.len(), 1, "the projected inlay rides the path");
    assert!(
        (projected[0].anchor.height() - 40.0).abs() < 0.5,
        "anchored at its reserved slot: {:?}",
        projected[0].anchor
    );
}

#[test]
fn paired_layouts_absorb_one_sided_inlays() {
    let fonts = test_fonts();
    let theme = crate::theme::Theme::embedded();
    let source = "alpha line\nbeta line\ngamma line\ndelta line\n";
    let mut left = crate::test_document::plain_document(source);

    let markup = crate::markup::MarkupId::mint();
    left.ensure_document_markup(markup);
    left.push_inlay(
        markup,
        11..12,
        crate::markup::Inlay::new(
            crate::markup::InlayMode::Above,
            FixedInlay {
                width: 60.0,
                height: 40.0,
            },
        ),
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let left_editor = left.add_editor(
        240.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let mut right = crate::test_document::plain_document(source);
    let right_editor = right.add_editor(
        240.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let diff = crate::diff::diff(left.text(), right.text());
    let mut left_layout = left.editors.get(&left_editor).unwrap().layout.clone();
    let mut right_layout = right.editors.get(&right_editor).unwrap().layout.clone();
    let _ = crate::split_diff::align::sync_spacers(
        &mut left_layout,
        &mut right_layout,
        &diff,
        0..source.len() as u32,
        usize::MAX,
        &mut 0,
    );

    let gamma = source.find("gamma").unwrap() as u32;
    let left_top = left_layout.height_before(gamma) + spacer_at(&left_layout, gamma);
    let right_top = right_layout.height_before(gamma) + spacer_at(&right_layout, gamma);
    assert_eq!(left_top as i64, right_top as i64);
    assert!(
        spacer_at(&right_layout, gamma) >= 39.0,
        "the right side absorbed the inlay height: {}",
        spacer_at(&right_layout, gamma)
    );
}

#[derive(Clone)]
struct FixedInlay {
    width: f32,
    height: f32,
}

impl imba::View for FixedInlay {
    type Command = ();

    fn perform(
        &mut self,
        _store: &mut imba::store::Store,
        _ui: &imba::UiCtx,
        _command: (),
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        _store: &'a imba::store::Store,
        _ui: &'a imba::UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a imba::arena::Arena, _constraints: imba::constraints::Constraints| {
                imba::leaf::leaf(self.width, self.height)
            },
        )
    }
}

#[test]
fn alignment_markers_place_the_covered_lines() {
    let fonts = font_collection();
    let theme = test_theme();
    let source = "right\ncente\nplain\n";
    let text = text::Text::from_string_exact(source);
    let mut builder = crate::markup::Markup::builder();
    builder.push_alignment(0..5, crate::theme::TextAlignment::Right);
    builder.push_alignment(6..11, crate::theme::TextAlignment::Center);
    let markup = builder.finish();

    let width = 400.0;
    let line_x = |range: std::ops::Range<u32>| -> f32 {
        let range_start = range.start;
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        let marks = markup.marks_inline_hidden_in(range.clone(), &mut inline, &mut hidden);
        let shaped = crate::shaped_line::ShapedLine::new(
            &mut text.view(),
            crate::markup::OverlaidMarkup::plain(&markup),
            range,
            &marks,
            &inline,
            &hidden,
            &fonts,
            &theme,
            width,
            0.0,
            true,
        );
        shaped.x_at_byte(range_start)
    };

    let right = line_x(0..5);
    let center = line_x(6..11);
    let plain = line_x(12..17);
    assert_eq!(plain, 0.0, "unmarked lines keep reading left");
    assert!(
        center > 10.0 && center < right,
        "centered sits between left and right: center={center} right={right}"
    );
    assert!(
        right > 200.0 && right < width,
        "right-aligned pushes to the laid width, on screen: x={right}"
    );
}

#[test]
fn line_spanning_background_markers_paint_to_the_line_end() {
    let fonts = font_collection();
    let theme = test_theme();
    let source = "washed\n\n";
    let text = text::Text::from_string_exact(source);

    let washed = |line: std::ops::Range<u32>, mark: std::ops::Range<u32>| -> (bool, bool) {
        let mut builder = crate::markup::Markup::builder();
        builder.push_styled(mark, crate::theme::StyleId::DiffAdded);
        let markup = builder.finish();
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        let marks = markup.marks_inline_hidden_in(line.clone(), &mut inline, &mut hidden);
        let shaped = crate::shaped_line::ShapedLine::new(
            &mut text.view(),
            crate::markup::OverlaidMarkup::plain(&markup),
            line,
            &marks,
            &inline,
            &hidden,
            &fonts,
            &theme,
            400.0,
            0.0,
            true,
        );
        let mut surface = skia_safe::surfaces::raster_n32_premul((400, 40)).expect("surface");
        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color::WHITE);
        shaped.paint(canvas, 0.0);
        let image = surface.image_snapshot();
        let pixmap = image.peek_pixels().expect("raster pixels");
        let bytes = pixmap.bytes().expect("pixel bytes");
        let row_bytes = image.width() as usize * 4;
        let inked = |x: usize, y: usize| -> bool {
            let px = &bytes[y * row_bytes + x * 4..y * row_bytes + x * 4 + 4];
            px[0] < 250 || px[1] < 250 || px[2] < 250
        };
        (inked(30, 4), inked(380, 4))
    };

    let (glyphs, edge) = washed(0..7, 0..8);
    assert!(
        glyphs && edge,
        "a wash through the newline paints to the line end: glyphs={glyphs} edge={edge}"
    );
    let (glyphs, edge) = washed(0..7, 0..4);
    assert!(glyphs, "a mid-line marker still paints under its glyphs");
    assert!(!edge, "a marker short of the line end stays glyph-tight");
    let (glyphs, edge) = washed(7..8, 0..8);
    assert!(
        glyphs && edge,
        "the covered BLANK line paints a full band: glyphs={glyphs} edge={edge}"
    );
}

#[test]
fn a_through_end_wash_fills_the_block_gap_below() {
    let fonts = font_collection();
    let theme = test_theme();
    let source = "washed\n";
    let text = text::Text::from_string_exact(source);

    let paints = |style: crate::theme::StyleId, mark: std::ops::Range<u32>| -> (Vec<u8>, Vec<u8>) {
        let mut builder = crate::markup::Markup::builder();
        builder.push_styled(mark, style);
        let markup = builder.finish();
        let line = 0..7u32;
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        let marks = markup.marks_inline_hidden_in(line.clone(), &mut inline, &mut hidden);
        let shaped = crate::shaped_line::ShapedLine::new(
            &mut text.view(),
            crate::markup::OverlaidMarkup::plain(&markup),
            line,
            &marks,
            &inline,
            &hidden,
            &fonts,
            &theme,
            400.0,
            0.0,
            true,
        );
        let render = |slot: Option<f32>| -> Vec<u8> {
            let mut surface = skia_safe::surfaces::raster_n32_premul((400, 120)).expect("surface");
            let canvas = surface.canvas();
            canvas.clear(skia_safe::Color::WHITE);
            match slot {
                None => shaped.paint(canvas, 0.0),
                Some(bottom) => shaped.paint_in_slot(canvas, 0.0, 0.0, bottom),
            }
            let image = surface.image_snapshot();
            let pixmap = image.peek_pixels().expect("raster pixels");
            pixmap.bytes().expect("pixel bytes").to_vec()
        };
        (render(None), render(Some(110.0)))
    };

    let gap_rows = |(bare, slotted): (Vec<u8>, Vec<u8>)| -> usize {
        let row_bytes = 400 * 4;
        (0..120)
            .filter(|row| {
                bare[row * row_bytes..(row + 1) * row_bytes]
                    != slotted[row * row_bytes..(row + 1) * row_bytes]
            })
            .count()
    };

    let filled = gap_rows(paints(crate::theme::StyleId::DiffAdded, 0..7));
    assert!(
        filled > 0,
        "a wash through the terminator fills the slot's gap: {filled} rows"
    );
    assert_eq!(
        gap_rows(paints(crate::theme::StyleId::DiffAdded, 0..4)),
        0,
        "a marker short of the line's end never touches the gap"
    );
    assert_eq!(
        gap_rows(paints(crate::theme::StyleId::BraceMatch, 0..7)),
        0,
        "a glyph-tight marker never touches the gap"
    );
}

#[test]
fn tight_background_markers_hug_their_glyphs() {
    let fonts = font_collection();
    let theme = test_theme();
    let source = "BIG word here\n";
    let text = text::Text::from_string_exact(source);

    let paint_rows = |style: Option<crate::theme::StyleId>| -> Vec<u8> {
        let mut builder = crate::markup::Markup::builder();
        builder.push_styled(0..3, crate::theme::StyleId::DeclarationName);
        if let Some(style) = style {
            builder.push_styled(4..8, style);
        }
        let markup = builder.finish();
        let line = 0..13u32;
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        let marks = markup.marks_inline_hidden_in(line.clone(), &mut inline, &mut hidden);
        let shaped = crate::shaped_line::ShapedLine::new(
            &mut text.view(),
            crate::markup::OverlaidMarkup::plain(&markup),
            line,
            &marks,
            &inline,
            &hidden,
            &fonts,
            &theme,
            400.0,
            0.0,
            true,
        );
        let mut surface = skia_safe::surfaces::raster_n32_premul((400, 80)).expect("surface");
        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color::WHITE);
        shaped.paint(canvas, 0.0);
        let image = surface.image_snapshot();
        let pixmap = image.peek_pixels().expect("raster pixels");
        pixmap.bytes().expect("pixel bytes").to_vec()
    };

    let plain = paint_rows(None);
    let row_bytes = 400 * 4;
    let banded_rows = |bytes: &[u8]| -> Vec<usize> {
        (0..80)
            .filter(|row| {
                bytes[row * row_bytes..(row + 1) * row_bytes]
                    != plain[row * row_bytes..(row + 1) * row_bytes]
            })
            .collect()
    };
    let tight = banded_rows(&paint_rows(Some(crate::theme::StyleId::BraceMatch)));
    let full = banded_rows(&paint_rows(Some(crate::theme::StyleId::Match)));
    assert!(!tight.is_empty(), "the tight band paints");
    assert!(
        tight.len() < full.len(),
        "the tight band hugs the run while the line-box band fills the \
         (DeclarationName-tall) line: tight={} rows, full={} rows",
        tight.len(),
        full.len()
    );
    assert!(
        tight.iter().all(|row| full.contains(row)),
        "the tight band sits inside the line box, never outside it"
    );
}

#[test]
fn inline_marker_styles_are_tight_and_line_washes_are_not() {
    use crate::theme::{BackgroundExtent, BackgroundHeight, BackgroundKind, StyleId};
    for theme in [
        crate::theme::Theme::embedded(),
        crate::theme::Theme::light(),
    ] {
        for id in [
            StyleId::BraceMatch,
            StyleId::Occurrence,
            StyleId::InlineCode,
            StyleId::DiffAddedWord,
            StyleId::DiffDeletedWord,
            StyleId::DiffModifiedWord,
        ] {
            let background = theme
                .attributes(id)
                .background
                .unwrap_or_else(|| panic!("{id:?} carries a background"));
            assert_eq!(background.kind, BackgroundKind::Text, "{id:?}");
            assert_eq!(background.extent, BackgroundExtent::Glyphs, "{id:?}");
            assert_eq!(background.height, BackgroundHeight::Tight, "{id:?}");
        }
        for id in [
            StyleId::DiffAdded,
            StyleId::DiffDeleted,
            StyleId::DiffModified,
        ] {
            let background = theme
                .attributes(id)
                .background
                .unwrap_or_else(|| panic!("{id:?} carries a background"));
            assert_eq!(
                background.height,
                BackgroundHeight::Line,
                "{id:?} is a LINE wash — full height by design"
            );
            assert_ne!(
                background.extent,
                BackgroundExtent::Glyphs,
                "{id:?} spans the line"
            );
        }
        let fence = theme
            .attributes(StyleId::CodeBlock)
            .background
            .expect("the fence panel");
        assert_eq!(
            fence.kind,
            BackgroundKind::Box,
            "the code fence keeps its bounding Box panel"
        );
        assert_eq!(fence.height, BackgroundHeight::Line);
    }
}

#[test]
fn spacers_on_zero_height_elements_shift_and_fill_the_paint_below() {
    let fonts = font_collection();
    let theme = test_theme();
    let mut document = crate::test_document::plain_document("alpha\nbeta\ngamma\ndelta\ntail\n");

    let markup = crate::markup::MarkupId::mint();
    document.ensure_document_markup(markup);
    document.push_inlay(
        markup,
        6..17,
        crate::markup::Inlay::new(
            crate::markup::InlayMode::Instead(crate::markup::InsteadKind::FullLine),
            FixedInlay {
                width: 60.0,
                height: 30.0,
            },
        ),
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let editor = document.add_editor(
        400.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let heights = document.element_heights(editor);
    let (hidden_byte, _) = *heights
        .iter()
        .find(|(_, height)| *height == 0.0)
        .expect("the Instead-covered line is a zero-height element");

    let ink_rows = |document: &Document| -> Vec<usize> {
        let mut surface = skia_safe::surfaces::raster_n32_premul((400, 400)).expect("surface");
        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color::WHITE);
        document.paint(
            editor,
            canvas,
            skia_safe::Rect::from_xywh(0.0, 0.0, 400.0, 400.0),
            false,
            &fonts,
            &test_theme(),
        );
        let image = surface.image_snapshot();
        let pixels = image
            .peek_pixels()
            .expect("raster pixels")
            .bytes()
            .expect("pixel bytes")
            .to_vec();
        let row_bytes = image.width() as usize * 4;
        (0..image.height() as usize)
            .filter(|row| {
                pixels[row * row_bytes..(row + 1) * row_bytes]
                    .chunks(4)
                    .any(|px| px[0] < 240 || px[1] < 240 || px[2] < 240)
            })
            .collect()
    };

    let flush = ink_rows(&document);
    document
        .editors
        .get_mut(&editor)
        .expect("editor")
        .layout
        .set_spacer(hidden_byte, 40.0);
    let spaced = ink_rows(&document);

    let last_flush = *flush.last().expect("the tail line painted");
    let last_spaced = *spaced.last().expect("the tail line still paints");
    assert_eq!(
        last_spaced,
        last_flush + 40,
        "the tail line moves down by exactly the spacer"
    );
    assert!(
        spaced.len() >= flush.len() + 35,
        "the spacer band paints as a filled band, not a blank hole: \
         {} inked rows flush, {} spaced",
        flush.len(),
        spaced.len()
    );
}

#[test]
fn markup_only_languages_ride_an_opaque_parse() {
    use std::sync::{Arc, Mutex};

    use crate::reparse::SyntaxTree;

    struct Marker;

    impl SyntaxTree for Marker {
        fn clone_tree(&self) -> Box<dyn SyntaxTree> {
            Box::new(Marker)
        }
        fn edit(
            &mut self,
            _operation: &operation::Operation,
            _view: &mut text::TextView,
            _base: u32,
        ) {
        }
        fn changed_since(&self, _old: &dyn SyntaxTree) -> Option<Vec<std::ops::Range<u32>>> {
            Some(Vec::new())
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct Recording {
        calls: Mutex<Vec<Vec<std::ops::Range<u32>>>>,
    }

    impl crate::reparse::SyntaxLanguage for Recording {
        fn parse(
            &self,
            _text: &text::Text,
            _range: std::ops::Range<u32>,
            _old: Option<&dyn SyntaxTree>,
        ) -> Option<Box<dyn SyntaxTree>> {
            Some(Box::new(Marker))
        }

        fn markup_for_changes(
            &self,
            _text: &text::Text,
            range: std::ops::Range<u32>,
            _tree: &dyn SyntaxTree,
            changed: &[std::ops::Range<u32>],
            replacement: &mut crate::markup::MarkupBuilder,
            invalidated: &mut Vec<std::ops::Range<u32>>,
            _fonts: &skia_safe::textlayout::FontCollection,
            _theme: &crate::theme::Theme,
        ) {
            self.calls.lock().expect("calls").push(changed.to_vec());
            let len = range.end - range.start;
            invalidated.clear();
            invalidated.push(0..len);
            replacement.push_styled(0..len, crate::theme::StyleId::Keyword);
        }
    }

    let recording = Arc::new(Recording {
        calls: Mutex::new(Vec::new()),
    });
    let mut languages = crate::reparse::SyntaxLanguages::new();
    languages.register(&["stub"], recording.clone());
    let fonts = font_collection();
    let theme = test_theme();
    let text = text::Text::from_string_exact("hello markup-only\n");
    let len = text.byte_count() as u32;

    let (syntax, invalidated, sites) = languages
        .parse_syntax("stub", &text, 0..len, None, &[], &fonts, &theme)
        .expect("a markup-only language parses");
    assert!(syntax.tree.is_some(), "the opaque parse value is stored");
    assert!(sites.is_empty(), "no children by default");
    assert_eq!(invalidated, vec![0..len]);

    let edited = vec![3..4];
    let (syntax, _, _) = languages
        .parse_syntax(
            "stub",
            &text,
            0..len,
            Some(&syntax),
            &edited,
            &fonts,
            &theme,
        )
        .expect("re-parses");
    assert!(syntax.tree.is_some());
    let calls = recording.calls.lock().expect("calls");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], vec![0..len]);
    assert_eq!(calls[1], vec![3..4]);

    struct Unusable;
    impl crate::reparse::SyntaxLanguage for Unusable {
        fn parse(
            &self,
            _text: &text::Text,
            _range: std::ops::Range<u32>,
            _old: Option<&dyn SyntaxTree>,
        ) -> Option<Box<dyn SyntaxTree>> {
            None
        }
        fn markup_for_changes(
            &self,
            _text: &text::Text,
            _range: std::ops::Range<u32>,
            _tree: &dyn SyntaxTree,
            _changed: &[std::ops::Range<u32>],
            _replacement: &mut crate::markup::MarkupBuilder,
            _invalidated: &mut Vec<std::ops::Range<u32>>,
            _fonts: &skia_safe::textlayout::FontCollection,
            _theme: &crate::theme::Theme,
        ) {
        }
    }
    let mut languages = crate::reparse::SyntaxLanguages::new();
    languages.register(&["broken"], Arc::new(Unusable));
    assert!(
        languages
            .parse_syntax("broken", &text, 0..len, None, &[], &fonts, &theme)
            .is_none(),
        "None keeps meaning unusable"
    );
}

#[test]
fn layout_survives_chars_straddling_the_chunk_grid() {
    let mut source = "x".repeat(16382);
    source.push('🚀');
    source.push_str(&"y".repeat(200));
    source.push_str("\ntail line one\ntail line two\n");
    let document = Document::new(
        text::Text::from_string_exact(&source),
        crate::markup::Markup::builder().finish(),
    );
    let mut layout = layout_of(&document, 300.0);

    let mut rounds = 0;
    while let Some(pending) = layout.repair_pending() {
        rounds += 1;
        assert!(rounds < 1_000, "the repair chain must converge");
        layout.repair_layout_bounded(
            document.text(),
            crate::markup::OverlaidMarkup::plain(document.markup()),
            300.0,
            &test_fonts(),
            &test_theme(),
            pending,
            60_000.0,
        );
    }

    let heights = layout.element_heights();
    let covered: u32 = {
        let mut cursor = heights.iter().peekable();
        let mut total = 0u32;
        while let Some((byte, _)) = cursor.next() {
            let end = cursor
                .peek()
                .map(|(next, _)| *next)
                .unwrap_or(source.len() as u32);
            total += end - byte;
        }
        total
    };
    let zero_tail = heights
        .iter()
        .zip(
            heights
                .iter()
                .skip(1)
                .map(|(byte, _)| *byte)
                .chain(std::iter::once(source.len() as u32)),
        )
        .filter(|((_, height), _)| *height <= 0.0)
        .map(|((byte, _), end)| end - byte)
        .max()
        .unwrap_or(0);
    assert_eq!(covered as usize, source.len(), "every byte is laid");
    assert!(
        zero_tail < 8,
        "no giant zero-height element swallowed the tail: {zero_tail}"
    );
    assert_eq!(layout.find_misaligned_boundary(document.text()), None);
}

#[test]
fn a_repair_relaunch_cancels_the_in_flight_lane() {
    use imba::effect::{Batch, Message};
    let source = "word ".repeat(20_000);
    let mut document = plain_document(&source);

    let repair_tokens = |batch: Batch<crate::EditorCommand>| {
        let mut cancels = Vec::new();
        let mut launched = Vec::new();
        for message in batch.drain() {
            match message {
                Message::Launch(token, effect) if effect.is::<crate::repair::RepairEffect>() => {
                    launched.push(token)
                }
                Message::Relaunch(previous, token, effect)
                    if effect.is::<crate::repair::RepairEffect>() =>
                {
                    cancels.push(previous);
                    launched.push(token);
                }
                Message::Cancel(token) => cancels.push(token),
                _ => {}
            }
        }
        (cancels, launched)
    };

    let mut open = Batch::new();
    let editor = document.add_editor(
        300.0,
        None,
        crate::document::EditorBuild::Bounded,
        &[],
        &test_fonts(),
        &test_theme(),
        &mut open.effects(),
    );
    let (_, opened) = repair_tokens(open);
    assert_eq!(opened.len(), 1, "the bounded open defers one repair tail");

    let mut resize = Batch::new();
    assert!(document.resize(
        editor,
        500.0,
        0,
        &test_fonts(),
        &test_theme(),
        &mut resize.effects()
    ));
    let (cancels, relaunched) = repair_tokens(resize);
    assert_eq!(
        cancels, opened,
        "the relaunch cancels exactly the in-flight repair"
    );
    assert_eq!(relaunched.len(), 1, "one fresh repair holds the lane");

    let mut again = Batch::new();
    assert!(document.resize(
        editor,
        400.0,
        0,
        &test_fonts(),
        &test_theme(),
        &mut again.effects()
    ));
    let (cancels, last) = repair_tokens(again);
    assert_eq!(
        cancels, relaunched,
        "every relaunch supersedes its predecessor"
    );
    assert_eq!(last.len(), 1);
}

use crate::caret::{Caret, MultiCaret};
use crate::editor_view::Motion;

#[test]
fn multicaret_insert_is_one_bulk_operation() {
    let (mut document, editor) = ime_doc("aa bb cc");
    document.set_carets(
        editor,
        MultiCaret::normalized(vec![Caret::at(0), Caret::at(3), Caret::at(6)], 0),
    );
    let revision_before = document.revision();
    document.insert(editor, "x", &test_fonts(), &test_theme(), fx!());

    assert_eq!(text_of(&document), "xaa xbb xcc");
    assert_eq!(
        document.revision(),
        revision_before + 1,
        "three carets, ONE operation in the log"
    );
    let offsets: Vec<u32> = document
        .carets(editor)
        .carets()
        .iter()
        .map(|caret| caret.offset())
        .collect();
    assert_eq!(offsets, vec![1, 5, 9], "each caret sits after its insert");
}

#[test]
fn multicaret_insert_replaces_selections() {
    let (mut document, editor) = ime_doc("one two three");
    document.set_carets(
        editor,
        MultiCaret::normalized(vec![Caret::selecting(0, 3), Caret::selecting(4, 7)], 0),
    );
    document.insert(editor, "X", &test_fonts(), &test_theme(), fx!());
    assert_eq!(text_of(&document), "X X three");
    assert!(!document.carets(editor).has_selection());
}

#[test]
fn multicaret_backspace_deletes_at_every_caret() {
    let (mut document, editor) = ime_doc("aa bb cc");
    document.set_carets(
        editor,
        MultiCaret::normalized(vec![Caret::at(2), Caret::at(5), Caret::at(8)], 0),
    );
    document.delete_at_carets(editor, Motion::Left, &test_fonts(), &test_theme(), fx!());
    assert_eq!(text_of(&document), "a b c");
    let offsets: Vec<u32> = document
        .carets(editor)
        .carets()
        .iter()
        .map(|caret| caret.offset())
        .collect();
    assert_eq!(offsets, vec![1, 3, 5]);
}

#[test]
fn multicaret_delete_forward_eats_selections_first() {
    let (mut document, editor) = ime_doc("abcdef");
    document.set_carets(
        editor,
        MultiCaret::normalized(vec![Caret::selecting(1, 3), Caret::at(4)], 0),
    );
    document.delete_at_carets(editor, Motion::Right, &test_fonts(), &test_theme(), fx!());
    assert_eq!(
        text_of(&document),
        "adf",
        "selection deleted; plain caret ate one char"
    );
}

#[test]
fn word_delete_spans_the_word_motion() {
    let (mut document, editor) = ime_doc("alpha beta gamma");
    document.set_carets(editor, MultiCaret::normalized(vec![Caret::at(10)], 0));
    document.delete_at_carets(
        editor,
        Motion::WordLeft,
        &test_fonts(),
        &test_theme(),
        fx!(),
    );
    assert_eq!(
        text_of(&document),
        "alpha  gamma",
        "alt-backspace ate the word behind"
    );
    document.delete_at_carets(
        editor,
        Motion::WordRight,
        &test_fonts(),
        &test_theme(),
        fx!(),
    );
    assert_eq!(
        text_of(&document),
        "alpha ",
        "alt-delete ate the word ahead"
    );

    document.set_carets(
        editor,
        MultiCaret::normalized(vec![Caret::selecting(0, 2)], 0),
    );
    document.delete_at_carets(
        editor,
        Motion::WordLeft,
        &test_fonts(),
        &test_theme(),
        fx!(),
    );
    assert_eq!(text_of(&document), "pha ");
}

#[test]
fn other_editors_carets_transform_through_a_bulk_edit() {
    let (mut document, editor) = ime_doc("aa bb cc");
    let other = document.add_editor(
        600.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    document.set_caret(other, 6);
    document.set_carets(
        editor,
        MultiCaret::normalized(vec![Caret::at(0), Caret::at(3)], 0),
    );
    document.insert(editor, "xx", &test_fonts(), &test_theme(), fx!());
    assert_eq!(text_of(&document), "xxaa xxbb cc");
    assert_eq!(document.caret_byte(other), 10, "rides both insertions");
}

#[test]
fn word_and_line_motions() {
    let (mut document, editor) = ime_doc("alpha beta\ngamma delta");
    document.set_caret(editor, 0);
    document.move_carets(
        editor,
        Motion::WordRight,
        false,
        &test_fonts(),
        &test_theme(),
    );
    assert_eq!(document.caret_byte(editor), 5, "end of alpha");
    document.move_carets(
        editor,
        Motion::WordRight,
        false,
        &test_fonts(),
        &test_theme(),
    );
    assert_eq!(document.caret_byte(editor), 10, "end of beta");
    document.move_carets(editor, Motion::LineEnd, false, &test_fonts(), &test_theme());
    assert_eq!(
        document.caret_byte(editor),
        10,
        "line end stays before the newline"
    );
    document.move_carets(editor, Motion::Right, false, &test_fonts(), &test_theme());
    document.move_carets(editor, Motion::LineEnd, false, &test_fonts(), &test_theme());
    assert_eq!(document.caret_byte(editor), 22, "second line end");
    document.move_carets(
        editor,
        Motion::LineStart,
        false,
        &test_fonts(),
        &test_theme(),
    );
    assert_eq!(document.caret_byte(editor), 11, "second line start");
    document.move_carets(
        editor,
        Motion::WordLeft,
        false,
        &test_fonts(),
        &test_theme(),
    );
    assert_eq!(
        document.caret_byte(editor),
        6,
        "back over the newline into beta"
    );
}

#[test]
fn selecting_motion_grows_from_the_anchor() {
    let (mut document, editor) = ime_doc("abc def");
    document.set_caret(editor, 3);
    document.move_carets(
        editor,
        Motion::WordRight,
        true,
        &test_fonts(),
        &test_theme(),
    );
    let primary = document.carets(editor).primary();
    assert_eq!(primary.selection(), 3..7);
    document.move_carets(editor, Motion::Left, true, &test_fonts(), &test_theme());
    assert_eq!(document.carets(editor).primary().selection(), 3..6);

    document.move_carets(editor, Motion::Right, false, &test_fonts(), &test_theme());
    let collapsed = document.carets(editor).primary();
    assert!(!collapsed.has_selection());
    assert_eq!(collapsed.offset(), 6);
}

#[test]
fn vertical_motion_keeps_the_goal_column() {
    let (mut document, editor) = ime_doc("a long first line\nab\nanother long third line");

    document.set_caret(editor, 12);
    document.move_carets(editor, Motion::Down, false, &test_fonts(), &test_theme());
    let on_short = document.caret_byte(editor);
    assert!(
        (18..=20).contains(&on_short),
        "clamped into the short line (got {on_short})"
    );
    document.move_carets(editor, Motion::Down, false, &test_fonts(), &test_theme());
    let on_third = document.caret_byte(editor);
    assert!(
        on_third > 21 + 5,
        "goal column carries past the short line (got {on_third})"
    );
    document.move_carets(editor, Motion::Up, false, &test_fonts(), &test_theme());
    document.move_carets(editor, Motion::Up, false, &test_fonts(), &test_theme());
    assert_eq!(
        document.caret_byte(editor),
        12,
        "round-trips to the origin column"
    );
}

#[test]
fn vertical_motion_off_the_edges_goes_to_document_ends() {
    let (mut document, editor) = ime_doc("first\nlast");
    document.set_caret(editor, 2);
    document.move_carets(editor, Motion::Up, false, &test_fonts(), &test_theme());
    assert_eq!(document.caret_byte(editor), 0);
    document.set_caret(editor, 8);
    document.move_carets(editor, Motion::Down, false, &test_fonts(), &test_theme());
    assert_eq!(document.caret_byte(editor), 10);
}

#[test]
fn add_caret_below_stacks_carets() {
    let (mut document, editor) = ime_doc("aaa\nbbb\nccc");
    document.set_caret(editor, 1);
    document.add_caret_vertically(editor, false, &test_fonts(), &test_theme());
    document.add_caret_vertically(editor, false, &test_fonts(), &test_theme());
    let offsets: Vec<u32> = document
        .carets(editor)
        .carets()
        .iter()
        .map(|caret| caret.offset())
        .collect();
    assert_eq!(offsets, vec![1, 5, 9], "one caret per line, same column");
}

#[test]
fn select_next_occurrence_walks_matches() {
    let (mut document, editor) = ime_doc("foo bar foo baz foo");
    document.set_caret(editor, 1);
    document.select_next_occurrence(editor);
    assert_eq!(
        document.carets(editor).primary().selection(),
        0..3,
        "first press selects the word under the caret"
    );
    document.select_next_occurrence(editor);
    let selections: Vec<_> = document
        .carets(editor)
        .carets()
        .iter()
        .map(|caret| caret.selection())
        .collect();
    assert_eq!(selections, vec![0..3, 8..11]);
    document.select_next_occurrence(editor);
    let selections: Vec<_> = document
        .carets(editor)
        .carets()
        .iter()
        .map(|caret| caret.selection())
        .collect();
    assert_eq!(selections, vec![0..3, 8..11, 16..19]);
}

#[test]
fn each_occurrence_streams_like_the_one_at_a_time_walk() {
    let body = "ab".repeat(100_000);
    let text = text::Text::from_string_exact(&body);
    let needle = b"abab";
    let range = 0u32..body.len() as u32;
    let mut streamed = Vec::new();
    crate::caret_ops::each_occurrence(&text, needle, range.clone(), |occurrence| {
        streamed.push(occurrence);
        true
    });
    let mut reference = Vec::new();
    let mut at = range.start;
    while let Some(occurrence) = crate::caret_ops::find_occurrence(&text, needle, at..range.end) {
        at = occurrence.end.max(occurrence.start + 1);
        reference.push(occurrence);
    }
    assert_eq!(streamed.len(), 50_000);
    assert_eq!(streamed, reference);

    let mut first = Vec::new();
    crate::caret_ops::each_occurrence(&text, needle, 0..body.len() as u32, |occurrence| {
        first.push(occurrence);
        false
    });
    assert_eq!(first, vec![0..4]);
}

#[test]
fn select_all_occurrences_selects_every_match() {
    let (mut document, editor) = ime_doc("x yy x zz x");
    document.set_caret(editor, 0);
    document.select_all_occurrences(editor);
    let selections: Vec<_> = document
        .carets(editor)
        .carets()
        .iter()
        .map(|caret| caret.selection())
        .collect();
    assert_eq!(selections, vec![0..1, 5..6, 10..11]);
}

#[test]
fn collapse_returns_to_the_primary_caret() {
    let (mut document, editor) = ime_doc("foo foo foo");
    document.set_caret(editor, 0);
    document.select_next_occurrence(editor);
    document.select_next_occurrence(editor);
    assert_eq!(document.carets(editor).len(), 2);
    document.collapse_carets(editor);
    let carets = document.carets(editor);
    assert_eq!(carets.len(), 1);
    assert!(!carets.has_selection());
}

#[test]
fn typing_after_select_all_occurrences_rewrites_every_match() {
    let (mut document, editor) = ime_doc("cat dog cat");
    document.set_caret(editor, 0);
    document.select_all_occurrences(editor);
    document.insert(editor, "bird", &test_fonts(), &test_theme(), fx!());
    assert_eq!(text_of(&document), "bird dog bird");
}

#[test]
fn caret_at_a_line_start_lands_on_the_following_line() {
    let (document, editor) = ime_doc("first\nsecond");
    let layout = document.document_layout(editor).expect("layout");

    let (_, _, byte_start) = layout.cursor_at_caret(6).expect("element");
    assert_eq!(byte_start, 6, "the caret's element is the second line");
    let (_, _, first) = layout.cursor_at_caret(3).expect("element");
    assert_eq!(first, 0, "mid-line carets stay on their line");
}

#[test]
fn click_past_a_line_end_stays_on_that_line() {
    let (document, editor) = ime_doc("ab\nlonger second line");
    let byte = document
        .byte_at_point(editor, 500.0, 5.0, &test_fonts(), &test_theme())
        .expect("hit");
    assert_eq!(
        byte, 2,
        "right of 'ab' means end of 'ab', not the next line"
    );
}

#[test]
fn text_focus_offers_caret_commands_to_the_palette() {
    let document = plain_document("foo bar foo");
    let mut view = crate::EditorView::complete(document, 600.0, &test_fonts(), &test_theme());
    let store = imba::store::Store::new();
    let ui = imba::UiCtx::cold();
    let size = skia_safe::Size::new(600.0, 400.0);

    let ids: Vec<&str> = imba::focus::frame_commands(&view, &store, &ui, size)
        .iter()
        .map(|presentable| presentable.id)
        .collect();
    assert!(ids.contains(&"editor.select-all"));
    assert!(ids.contains(&"editor.select-next-occurrence"));
    assert!(ids.contains(&"editor.add-caret-below"));
    assert!(
        !ids.contains(&"editor.collapse-carets"),
        "collapse is offered only when there is something to collapse"
    );

    view.document.select_all(view.editor);
    let ids: Vec<&str> = imba::focus::frame_commands(&view, &store, &ui, size)
        .iter()
        .map(|presentable| presentable.id)
        .collect();
    assert!(ids.contains(&"editor.collapse-carets"));

    view.blur();
    assert!(
        imba::focus::frame_commands(&view, &store, &ui, size).is_empty(),
        "an unfocused editor offers nothing"
    );
}

#[test]
fn short_lines_straddling_the_chunk_grid_do_not_split() {
    let line = "let clock = std::cell::Cell::new(0.0f64); // filler text\n";
    let source = line.repeat(48 * 1024 / line.len());
    let document = Document::new(
        text::Text::from_string_exact(&source),
        crate::markup::Markup::builder().finish(),
    );
    let mut layout = layout_of(&document, 100_000.0);
    while let Some(pending) = layout.repair_pending() {
        layout.repair_layout_bounded(
            document.text(),
            crate::markup::OverlaidMarkup::plain(document.markup()),
            100_000.0,
            &test_fonts(),
            &test_theme(),
            pending,
            60_000.0,
        );
    }
    let line_starts: std::collections::HashSet<u32> = std::iter::once(0u32)
        .chain(
            source
                .bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(at, _)| at as u32 + 1),
        )
        .collect();
    for range in layout.element_byte_ranges() {
        assert!(
            line_starts.contains(&range.start),
            "element starts mid-line at byte {} (grid line at {}): a short \
             line straddling the grid was split",
            range.start,
            (range.start / 16384) * 16384,
        );
    }
}

#[test]
fn monster_lines_still_tile_on_the_grid() {
    let mut source = "x".repeat(40 * 1024);
    source.push_str("\nshort tail\n");
    let document = Document::new(
        text::Text::from_string_exact(&source),
        crate::markup::Markup::builder().finish(),
    );
    let mut layout = layout_of(&document, 300.0);
    while let Some(pending) = layout.repair_pending() {
        layout.repair_layout_bounded(
            document.text(),
            crate::markup::OverlaidMarkup::plain(document.markup()),
            300.0,
            &test_fonts(),
            &test_theme(),
            pending,
            60_000.0,
        );
    }
    let starts: Vec<u32> = layout
        .element_byte_ranges()
        .into_iter()
        .map(|range| range.start)
        .collect();
    for expected in [0u32, 16384, 32768] {
        assert!(
            starts.contains(&expected),
            "monster interior boundaries stay grid-aligned: {starts:?}"
        );
    }
}

#[test]
fn viewport_numbers_first_soft_rows_only() {
    let width = 220.0;

    let source = format!("first\n{}\nthird", "wrap ".repeat(40));
    let document = {
        let mut document = plain_document(&source);
        document.add_editor(
            width,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        document
    };
    let editor = document.editor_ids().next().expect("one editor");
    let viewport = crate::viewport::EditorViewport::build(
        &document,
        editor,
        0.0..10_000.0,
        false,
        true,
        None,
        &test_fonts(),
        &test_theme(),
    );

    let numbered: Vec<(u32, u32)> = viewport
        .lines
        .iter()
        .filter_map(|line| line.hard_line.map(|number| (number, line.byte_start)))
        .collect();
    assert_eq!(
        numbered
            .iter()
            .map(|(number, _)| *number)
            .collect::<Vec<_>>(),
        vec![1, 2, 3],
        "one number per hard line, sequential"
    );

    let starts: Vec<u32> = numbered.iter().map(|(_, start)| *start).collect();
    assert_eq!(starts[0], 0);
    assert_eq!(starts[1], "first\n".len() as u32);
    assert!(
        viewport.lines.len() > 3,
        "the middle line wraps into several rows: {}",
        viewport.lines.len()
    );
    for line in &viewport.lines {
        if line.hard_line.is_none() {
            continue;
        }
        let baseline = line.baseline.expect("text rows carry a baseline");
        assert!(
            baseline > line.text_top && baseline <= line.text_top + line.height,
            "baseline {baseline} inside the row band {}..{}",
            line.text_top,
            line.text_top + line.height,
        );
    }

    assert!(viewport.lines.iter().any(|line| line.hard_line.is_none()));
}

#[test]
fn viewport_build_is_band_scoped() {
    let source = "line\n".repeat(10_000);
    let mut document = plain_document(&source);
    let editor = document.add_editor(
        600.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &test_theme(),
        fx!(),
    );
    let band_top = document.height_before(editor, 5_000 * 5);
    let viewport = crate::viewport::EditorViewport::build(
        &document,
        editor,
        band_top..band_top + 300.0,
        false,
        true,
        None,
        &test_fonts(),
        &test_theme(),
    );
    assert!(
        viewport.lines.len() < 40,
        "a 300px band is a handful of lines, not the document: {}",
        viewport.lines.len()
    );

    let first = viewport.lines[0].hard_line.expect("hard line start");
    assert!(first > 4_000, "deep in the document: {first}");
}

#[test]
fn gutter_paints_numbers_beside_shifted_text() {
    let chrome_width = test_theme().ui().editor_gutter.width;
    let source = "alpha\nbeta\ngamma\n";
    let mut view =
        crate::EditorView::complete(plain_document(source), 400.0, &test_fonts(), &test_theme());
    view.gutter_width = chrome_width;
    let store = imba::Store::new();
    let ui = imba::UiCtx::cold();
    ui.set(crate::env::UiFonts(test_fonts()));
    let arena = imba::arena::Arena::default();
    let constraints = imba::constraints::Constraints {
        min: skia_safe::Size::default(),
        max: skia_safe::Size::new(400.0 + chrome_width, f32::MAX),
    };
    let widget = imba::Layout::layout(
        imba::View::display(&view, &arena, &store, &ui),
        &arena,
        constraints,
    );
    let size = imba::Thunk::size(&widget);
    let widget = imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_wh(size.width, 200.0));
    assert!(
        (size.width - (400.0 + chrome_width)).abs() < 1.0,
        "gutter + text: {}",
        size.width
    );

    let mut surface =
        skia_safe::surfaces::raster_n32_premul((size.width as i32, 200)).expect("surface");
    let background = skia_safe::Color::from_rgb(9, 9, 9);
    surface.canvas().clear(background);
    let result = imba::Widget::handle_event(
        &widget,
        &arena,
        &imba::event::Event::Paint {
            canvas: surface.canvas(),
            focused: false,
        },
        skia_safe::Rect::from_wh(size.width, 200.0),
    );
    drop(result);

    let image = surface.image_snapshot();
    let pixmap = image.peek_pixels().expect("raster pixels");
    let mut gutter_ink = 0;
    let mut text_gutterband_ink = 0;
    for y in 0..200 {
        for x in 0..(chrome_width as i32) {
            if pixmap.get_color((x, y)) != background {
                gutter_ink += 1;
            }
        }
    }
    assert!(gutter_ink > 0, "line numbers ink the gutter stripe");

    drop(widget);
    view.gutter_width = 0.0;
    let widget = imba::Layout::layout(
        imba::View::display(&view, &arena, &store, &ui),
        &arena,
        constraints,
    );
    let widget = imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_wh(size.width, 200.0));
    surface.canvas().clear(background);
    let _ = imba::Widget::handle_event(
        &widget,
        &arena,
        &imba::event::Event::Paint {
            canvas: surface.canvas(),
            focused: false,
        },
        skia_safe::Rect::from_wh(size.width, 200.0),
    );
    let image = surface.image_snapshot();
    let pixmap = image.peek_pixels().expect("raster pixels");
    for y in 0..200 {
        for x in 0..(chrome_width as i32) {
            if pixmap.get_color((x, y)) != background {
                text_gutterband_ink += 1;
            }
        }
    }
    assert!(
        text_gutterband_ink > gutter_ink,
        "without the gutter the text occupies the band: text {text_gutterband_ink} vs numbers {gutter_ink}"
    );
}

#[test]
fn gutter_numbers_share_the_text_baseline() {
    let chrome = test_theme().ui().editor_gutter.clone();

    let source = "alpha\nbeta\ngamma";
    let mut view =
        crate::EditorView::complete(plain_document(source), 400.0, &test_fonts(), &test_theme());
    view.gutter_width = chrome.width;
    let store = imba::Store::new();
    let ui = imba::UiCtx::cold();
    ui.set(crate::env::UiFonts(test_fonts()));
    let arena = imba::arena::Arena::default();
    let constraints = imba::constraints::Constraints {
        min: skia_safe::Size::default(),
        max: skia_safe::Size::new(400.0 + chrome.width, f32::MAX),
    };
    let widget = imba::Layout::layout(
        imba::View::display(&view, &arena, &store, &ui),
        &arena,
        constraints,
    );
    let size = imba::Thunk::size(&widget);
    let widget = imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_wh(size.width, 200.0));
    let mut surface =
        skia_safe::surfaces::raster_n32_premul((size.width as i32, 200)).expect("surface");
    let background = skia_safe::Color::from_rgb(9, 9, 9);
    surface.canvas().clear(background);
    let _ = imba::Widget::handle_event(
        &widget,
        &arena,
        &imba::event::Event::Paint {
            canvas: surface.canvas(),
            focused: false,
        },
        skia_safe::Rect::from_wh(size.width, 200.0),
    );
    let image = surface.image_snapshot();
    let pixmap = image.peek_pixels().expect("raster pixels");
    let ink_rows = |x0: i32, x1: i32| -> Vec<i32> {
        (0..200)
            .filter(|y| (x0..x1).any(|x| pixmap.get_color((x, *y)) != background))
            .collect()
    };
    let number_rows = ink_rows(0, chrome.width as i32);
    let text_rows = ink_rows(chrome.width as i32, size.width as i32);
    assert!(!number_rows.is_empty() && !text_rows.is_empty());

    let text_top = *text_rows.first().expect("text ink");
    let text_bottom = *text_rows.last().expect("text ink");
    for row in &number_rows {
        assert!(
            *row >= text_top - 1 && *row <= text_bottom + 1,
            "digit ink at row {row} outside text ink {text_top}..{text_bottom}"
        );
    }
}

mod folding {
    use super::{test_fonts, test_theme};
    use crate::document::Document;
    use crate::markup::{Markup, Syntax};
    use crate::viewport::EditorViewport;
    use operation::Operation;

    fn foldable_document(source: &str, foldable: std::ops::Range<u32>) -> Document {
        let mut syntax = Syntax::new("toy", None, Markup::new());
        syntax.folds.insert([intervals::Interval {
            range: foldable,
            greedy_left: false,
            greedy_right: false,
            key: crate::markup::IntervalId(0),
            value: (),
        }]);
        Document::new(text::Text::from_string_exact(source), Markup::new()).with_syntax(syntax, &[])
    }

    const SOURCE: &str = "head\nfn demo() {\n    body\n}\ntail\n";

    fn settle_fold_animation(
        document: &mut Document,
        editor: crate::EditorId,
        range: &std::ops::Range<u32>,
    ) {
        let key = document
            .fold_matching(editor, range)
            .expect("a standing fold");
        let mut store = imba::Store::new();
        let ui = imba::UiCtx::cold();
        for millis in [0.0, 10_000.0] {
            document.perform(
                &mut store,
                &ui,
                editor,
                crate::EditorCommand::Inlay {
                    key,
                    command: Box::new(crate::fold::FoldCommand::Tick(
                        imba::anim::AnimationClock::from_millis(millis),
                    )),
                },
                fx!(),
            );
        }
    }

    fn interior() -> std::ops::Range<u32> {
        let start = SOURCE.find('{').unwrap() as u32 + 1;
        let end = SOURCE.find('}').unwrap() as u32;
        start..end
    }

    fn viewport_of(document: &Document, editor: crate::EditorId) -> EditorViewport {
        EditorViewport::build(
            document,
            editor,
            0.0..10_000.0,
            false,
            true,
            None,
            &test_fonts(),
            &test_theme(),
        )
    }

    #[test]
    fn toggle_folds_into_one_joined_line_and_back() {
        let mut document = foldable_document(SOURCE, interior());
        let editor = document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let open_heights = document.element_heights(editor);
        assert_eq!(
            open_heights.len(),
            6,
            "head, fn, body, brace, tail, empty last line"
        );
        let line_height = open_heights[0].1;

        let viewport = viewport_of(&document, editor);
        let offer: Vec<_> = viewport
            .lines
            .iter()
            .filter_map(|line| line.foldable.clone())
            .collect();
        assert_eq!(offer.len(), 1);
        assert_eq!(offer[0].range, interior());
        assert!(!offer[0].folded);
        assert_eq!(offer[0].spin, 0.0, "open: chevron down");

        document.toggle_fold(editor, interior(), &test_fonts(), &test_theme(), fx!());
        assert!(document.fold_matching(editor, &interior()).is_some());
        let ranges = document.element_byte_ranges(editor);
        assert_eq!(
            ranges.len(),
            4,
            "head, the joined fn line, tail, empty last line: {ranges:?}"
        );
        let fn_start = "head\n".len() as u32;
        let brace_end = SOURCE.find('}').unwrap() as u32 + 2;
        assert_eq!(ranges[1], fn_start..brace_end, "the unit spans the fold");

        let born_heights = document.element_heights(editor);
        let covered: f32 = open_heights[1..4].iter().map(|(_, height)| height).sum();
        assert!(
            (born_heights[1].1 - covered).abs() <= line_height * 0.5,
            "the joined unit is born at the folded rows' height: {} vs {covered}",
            born_heights[1].1
        );
        settle_fold_animation(&mut document, editor, &interior());
        assert_eq!(
            document.focus(editor),
            crate::editor_view::EditorFocus::Text,
            "animation ticks never steal the focus"
        );
        let folded_heights = document.element_heights(editor);
        assert!(
            (folded_heights[1].1 - line_height).abs() <= line_height * 0.5,
            "the joined unit settles to ONE text line (chip inline): {} vs {line_height}",
            folded_heights[1].1
        );

        let viewport = viewport_of(&document, editor);
        let folded: Vec<_> = viewport
            .lines
            .iter()
            .filter_map(|line| line.foldable.clone())
            .collect();
        assert_eq!(folded.len(), 1, "the joined row still offers");
        assert!(folded[0].folded);
        assert_eq!(folded[0].spin, 1.0, "settled fold: chevron fully rotated");

        let numbers: Vec<u32> = viewport
            .lines
            .iter()
            .filter_map(|line| line.hard_line)
            .collect();
        assert_eq!(numbers, vec![1, 2, 5, 6]);

        document.toggle_fold(editor, interior(), &test_fonts(), &test_theme(), fx!());
        let key = document
            .fold_matching(editor, &interior())
            .expect("departing, not yet removed");
        assert!(document
            .fold_chip_at(key)
            .is_some_and(|chip| chip.is_departing()));
        settle_fold_animation(&mut document, editor, &interior());
        assert!(document.fold_matching(editor, &interior()).is_none());
        assert_eq!(document.element_heights(editor), open_heights);
    }

    #[test]
    fn edits_shift_fold_and_foldable_in_step() {
        let mut document = foldable_document(SOURCE, interior());
        let editor = document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        document.toggle_fold(editor, interior(), &test_fonts(), &test_theme(), fx!());

        document.edit(
            &Operation::insert_at(0, "// note\n"),
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let shifted = interior().start + 8..interior().end + 8;
        assert!(
            document.fold_matching(editor, &shifted).is_some(),
            "both intervals rode the insert"
        );
        let viewport = viewport_of(&document, editor);
        let offer: Vec<_> = viewport
            .lines
            .iter()
            .filter_map(|line| line.foldable.clone())
            .collect();
        assert_eq!(offer.len(), 1);
        assert_eq!(offer[0].range, shifted);
        assert!(offer[0].folded, "the match survives the shift");
    }

    #[test]
    fn folds_are_per_editor() {
        let mut document = foldable_document(SOURCE, interior());
        let folder = document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let other = document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let other_open = document.element_heights(other);

        document.toggle_fold(folder, interior(), &test_fonts(), &test_theme(), fx!());
        assert!(document.fold_matching(folder, &interior()).is_some());
        assert!(
            document.fold_matching(other, &interior()).is_none(),
            "folded-ness is the VIEW's"
        );
        assert_eq!(
            document.element_heights(other),
            other_open,
            "the other editor's layout never joins"
        );
        let viewport = viewport_of(&document, other);
        let offer: Vec<_> = viewport
            .lines
            .iter()
            .filter_map(|line| line.foldable.clone())
            .collect();
        assert!(!offer[0].folded);
    }

    #[test]
    fn a_stale_toggle_on_an_unoffered_range_is_a_no_op() {
        let mut document = foldable_document(SOURCE, interior());
        let editor = document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        document.toggle_fold(editor, 1..9, &test_fonts(), &test_theme(), fx!());
        assert!(document.fold_matching(editor, &(1..9)).is_none());
    }

    #[test]
    fn the_chip_click_unfolds_through_the_inlay_arm() {
        let mut document = foldable_document(SOURCE, interior());
        let editor = document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let open_heights = document.element_heights(editor);
        document.toggle_fold(editor, interior(), &test_fonts(), &test_theme(), fx!());
        let key = document.fold_matching(editor, &interior()).expect("folded");

        let mut store = imba::Store::new();
        let ui = imba::UiCtx::cold();
        document.perform(
            &mut store,
            &ui,
            editor,
            crate::EditorCommand::Inlay {
                key,
                command: Box::new(crate::fold::FoldCommand::Unfold),
            },
            fx!(),
        );

        assert!(document
            .fold_chip_at(key)
            .is_some_and(|chip| chip.is_departing()));
        settle_fold_animation(&mut document, editor, &interior());
        assert!(document.fold_matching(editor, &interior()).is_none());
        assert_eq!(document.element_heights(editor), open_heights);
    }

    #[test]
    fn a_gutter_click_toggles_the_fold() {
        let chrome_width = test_theme().ui().editor_gutter.width;
        let mut view = crate::EditorView::complete(
            foldable_document(SOURCE, interior()),
            400.0,
            &test_fonts(),
            &test_theme(),
        );
        view.gutter_width = chrome_width;
        let editor = view.editor;
        let store = imba::Store::new();
        let ui = imba::UiCtx::cold();
        ui.set(crate::env::UiFonts(test_fonts()));
        let arena = imba::arena::Arena::default();
        let constraints = imba::constraints::Constraints {
            min: skia_safe::Size::default(),
            max: skia_safe::Size::new(400.0 + chrome_width, f32::MAX),
        };
        let y = view.document.height_before(editor, interior().start) + 2.0;
        let widget = imba::Layout::layout(
            imba::View::display(&view, &arena, &store, &ui),
            &arena,
            constraints,
        );
        let widget = imba::Thunk::realize(
            widget,
            &arena,
            skia_safe::Rect::from_wh(400.0 + chrome_width, 600.0),
        );
        let result = imba::Widget::handle_event(
            &widget,
            &arena,
            &imba::event::Event::MouseDown {
                button: imba::event::MouseButton::Left,
                point: skia_safe::Point::new(chrome_width * 0.5, y),
                mods: imba::event::Modifiers::default(),
                count: 1,
            },
            skia_safe::Rect::from_wh(400.0 + chrome_width, 600.0),
        );
        let imba::event::EventResult::Command(crate::EditorCommand::ToggleFold { range }) = result
        else {
            panic!("the gutter click answers the toggle");
        };
        assert_eq!(range, interior());
        drop(widget);
        view.document
            .toggle_fold(editor, range, &test_fonts(), &test_theme(), fx!());
        assert!(view.document.fold_matching(editor, &interior()).is_some());
    }

    #[test]
    fn folding_a_monster_interior_stays_bounded() {
        let body = "    filler line of some length\n".repeat(100_000);
        let source = format!("fn monster() {{\n{body}}}\nafter\n");
        let start = source.find('{').unwrap() as u32 + 1;
        let end = source.rfind('}').unwrap() as u32;
        let mut document = foldable_document(&source, start..end);
        let editor = document.add_editor(
            600.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let started = std::time::Instant::now();
        document.toggle_fold(editor, start..end, &test_fonts(), &test_theme(), fx!());
        let folded_in = started.elapsed();

        assert!(
            document.fold_matching(editor, &(start..end)).is_some(),
            "folded"
        );
        imba::perf::record(
            "fold-toggle",
            "monster_ms",
            folded_in.as_secs_f64() * 1000.0,
        );
        assert!(
            folded_in.as_secs_f64() < 1.0,
            "folding a 3MB body must not walk it: {folded_in:?}"
        );
    }
}

mod gutter_stripes {
    use super::*;
    use crate::viewport::{DiffLineKind, EditorViewport};

    macro_rules! fx {
        () => {
            &mut imba::effect::Batch::new().effects()
        };
    }

    fn kind_at(viewport: &EditorViewport, source: &str, needle: &str) -> Option<DiffLineKind> {
        let at = source.find(needle).expect("the needle exists") as u32;
        viewport
            .lines
            .iter()
            .find(|line| line.byte_start <= at && at < line.byte_end)
            .expect("the row is visible")
            .diff
    }

    fn build(
        document: &Document,
        editor: crate::EditorId,
        stripes: Option<crate::diff::DiffId>,
    ) -> EditorViewport {
        EditorViewport::build(
            document,
            editor,
            0.0..10_000.0,
            false,
            true,
            stripes,
            &test_fonts(),
            &test_theme(),
        )
    }

    #[test]
    fn rows_classify_against_the_tracked_base() {
        let base = "one\ntwo\nthree\nfour\nfive\nsix\n";
        let source = "one\nTWO!\nthree\nadded\nfour\nsix\n";
        let mut document = plain_document(source);
        let editor = document.add_editor(
            600.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let operation = crate::diff::diff(&text::Text::from_string_exact(base), document.text());
        let id = document.add_diff(operation, 0);

        let viewport = build(&document, editor, Some(id));
        assert_eq!(kind_at(&viewport, source, "one"), None, "retained");
        assert_eq!(
            kind_at(&viewport, source, "TWO!"),
            Some(DiffLineKind::Modified),
            "the replaced line"
        );
        assert_eq!(kind_at(&viewport, source, "three"), None, "retained");
        assert_eq!(
            kind_at(&viewport, source, "added"),
            Some(DiffLineKind::Added),
            "the pure insertion"
        );
        assert_eq!(kind_at(&viewport, source, "four"), None, "retained");
        assert_eq!(
            kind_at(&viewport, source, "six"),
            Some(DiffLineKind::DeletedAbove),
            "the deletion marks the row below it"
        );
    }

    #[test]
    fn standing_stripes_shift_with_typing_and_fresh_hunks_land_with_the_normalize() {
        let base = "alpha\nbeta\ngamma\n";
        let source = "alpha\nBETA\ngamma\n";
        let mut document = plain_document(source);
        let editor = document.add_editor(
            600.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let operation = crate::diff::diff(&text::Text::from_string_exact(base), document.text());
        let id = document.add_diff(operation, 0);

        // Typing ABOVE the standing hunk shifts its stripe the same
        // frame — the markup rides the edit door.
        document.edit(
            &Operation::insert_at(0, "zero\n"),
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let viewport = build(&document, editor, Some(id));
        let source_now = "zero\nalpha\nBETA\ngamma\n";
        assert_eq!(
            kind_at(&viewport, source_now, "BETA"),
            Some(DiffLineKind::Modified),
            "the standing hunk moved with the edit"
        );
        // The typed line's OWN hunk is one normalize landing behind —
        // the colors contract.
        assert_eq!(kind_at(&viewport, source_now, "zero"), None);

        // The landing installs the fresh derivation and the typed
        // line stripes.
        let minimal = crate::diff::diff(&text::Text::from_string_exact(base), document.text());
        let fresh = crate::diff::hunk_markup(&minimal, document.text());
        assert!(document.install_normalized_diff(id, minimal, 0));
        document.install_diff_markup(
            id,
            fresh,
            vec![0..u32::MAX],
            document.revision(),
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let viewport = build(&document, editor, Some(id));
        assert_eq!(
            kind_at(&viewport, source_now, "zero"),
            Some(DiffLineKind::Added),
            "the fresh hunk landed"
        );
        assert_eq!(
            kind_at(&viewport, source_now, "BETA"),
            Some(DiffLineKind::Modified)
        );
        assert_eq!(kind_at(&viewport, source_now, "alpha"), None);
        assert_eq!(kind_at(&viewport, source_now, "gamma"), None);
    }

    #[test]
    fn stripes_need_both_the_join_and_a_gutter() {
        let source = "one\ntwo\n";
        let mut document = plain_document(source);
        let editor = document.add_editor(
            600.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let operation = crate::diff::diff(&text::Text::from_string_exact("one\n"), document.text());
        let id = document.add_diff(operation, 0);
        let unjoined = build(&document, editor, None);
        assert!(unjoined.lines.iter().all(|line| line.diff.is_none()));

        let gutterless = EditorViewport::build(
            &document,
            editor,
            0.0..10_000.0,
            false,
            false,
            Some(id),
            &test_fonts(),
            &test_theme(),
        );
        assert!(gutterless.lines.iter().all(|line| line.diff.is_none()));
    }
}

mod before_inlay {
    use super::*;

    macro_rules! fx {
        () => {
            &mut imba::effect::Batch::new().effects()
        };
    }

    const BASE: &str = "one\ntwo\nthree\nfour\nfive\nsix\n";
    const SOURCE: &str = "one\nTWO!\nthree\nadded\nfour\nsix\n";

    fn joined() -> crate::EditorView {
        let mut document = plain_document(SOURCE);
        let editor = document.add_editor(
            600.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let base_document = plain_document(BASE);
        let operation = crate::diff::diff(base_document.text(), document.text());
        let id = document.add_diff(operation, base_document.revision());
        crate::EditorView {
            document,
            editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: Some((base_document, id)),
        }
    }

    fn toggle(view: &mut crate::EditorView, at: u32) {
        use imba::View;
        let mut store = imba::store::Store::new();
        let ui = imba::UiCtx::cold();
        view.perform(
            &mut store,
            &ui,
            crate::EditorCommand::ToggleBeforeInlay { at },
            fx!(),
        );
    }

    fn cards(
        view: &crate::EditorView,
    ) -> Vec<(std::ops::Range<u32>, std::ops::Range<u32>, String)> {
        view.document.before_inlays(view.editor)
    }

    fn line_of(source: &str, needle: &str) -> std::ops::Range<u32> {
        let at = source.find(needle).expect("the needle exists");
        let start = source[..at].rfind('\n').map(|nl| nl + 1).unwrap_or(0);
        let end = source[at..]
            .find('\n')
            .map(|nl| at + nl + 1)
            .unwrap_or(source.len());
        start as u32..end as u32
    }

    #[test]
    fn a_modified_row_shows_its_old_lines() {
        let mut view = joined();
        let at = SOURCE.find("TWO!").unwrap() as u32;
        toggle(&mut view, at);
        let cards = cards(&view);
        assert_eq!(cards.len(), 1, "{cards:?}");
        let (anchor, base_range, shown) = &cards[0];
        assert_eq!(
            *anchor,
            line_of(SOURCE, "TWO!"),
            "anchored on the changed row"
        );
        assert_eq!(
            *base_range,
            line_of(BASE, "two"),
            "the operation mapped the range"
        );
        assert_eq!(shown, "two\n", "the card shows the base's line");
    }

    #[test]
    fn a_deletion_marker_shows_the_deleted_lines() {
        let mut view = joined();
        let at = SOURCE.find("six").unwrap() as u32;
        toggle(&mut view, at);
        let cards = cards(&view);
        assert_eq!(cards.len(), 1, "{cards:?}");
        let (anchor, _, shown) = &cards[0];
        assert_eq!(
            *anchor,
            line_of(SOURCE, "six"),
            "anchored on the marker row"
        );
        assert_eq!(shown, "five\n", "the deleted base line");
    }

    #[test]
    fn a_pure_insertion_offers_no_before() {
        let mut view = joined();
        toggle(&mut view, SOURCE.find("added").unwrap() as u32);
        assert!(cards(&view).is_empty());
    }

    #[test]
    fn an_unchanged_row_offers_no_before() {
        let mut view = joined();
        toggle(&mut view, SOURCE.find("three").unwrap() as u32);
        assert!(cards(&view).is_empty());
    }

    #[test]
    fn toggling_again_removes_the_card() {
        let mut view = joined();
        let at = SOURCE.find("TWO!").unwrap() as u32;
        toggle(&mut view, at);
        assert_eq!(cards(&view).len(), 1);
        toggle(&mut view, at);
        assert!(cards(&view).is_empty(), "the toggle removed it");
    }

    #[test]
    fn an_unjoined_view_ignores_the_click() {
        let mut view = joined();
        view.base = None;
        toggle(&mut view, SOURCE.find("TWO!").unwrap() as u32);
        assert!(cards(&view).is_empty());
    }

    #[test]
    fn the_anchor_shifts_with_edits() {
        let mut view = joined();
        let at = SOURCE.find("TWO!").unwrap() as u32;
        toggle(&mut view, at);
        let before = cards(&view)[0].0.clone();
        view.document.edit(
            &Operation::insert_at(0, "head\n"),
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let after = cards(&view)[0].0.clone();
        assert_eq!(after.start, before.start + 5, "the card rode the insert");
        assert_eq!(after.end, before.end + 5);

        assert_eq!(cards(&view)[0].2, "two\n");
    }

    #[test]
    fn a_multi_line_block_expands_whole_from_any_row() {
        let base = "keep\nold a\nold b\nold c\ntail\n";
        let source = "keep\nnew one\nnew two\ntail\n";
        let mut document = plain_document(source);
        let editor = document.add_editor(
            600.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let base_document = plain_document(base);
        let operation = crate::diff::diff(base_document.text(), document.text());
        let id = document.add_diff(operation, base_document.revision());
        let mut view = crate::EditorView {
            document,
            editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: Some((base_document, id)),
        };
        toggle(&mut view, source.find("new two").unwrap() as u32);
        let cards_now = cards(&view);
        assert_eq!(cards_now.len(), 1, "{cards_now:?}");
        assert_eq!(
            cards_now[0].2, "old a\nold b\nold c\n",
            "the whole block's before-text"
        );
        toggle(&mut view, source.find("new one").unwrap() as u32);
        assert!(
            cards(&view).is_empty(),
            "any row of the block toggles it off"
        );
    }
}

mod before_inlay_presentation {
    use super::*;
    use crate::before_inlay::BeforeCommand;

    macro_rules! fx {
        () => {
            &mut imba::effect::Batch::new().effects()
        };
    }

    const BASE: &str = "one\ntwo\nthree\nfour\nfive\nsix\n";
    const SOURCE: &str = "one\nTWO!\nthree\nadded\nfour\nsix\n";

    fn joined() -> crate::EditorView {
        let mut document = plain_document(SOURCE);
        let editor = document.add_editor(
            600.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            fx!(),
        );
        let base_document = plain_document(BASE);
        let operation = crate::diff::diff(base_document.text(), document.text());
        let id = document.add_diff(operation, base_document.revision());
        crate::EditorView {
            document,
            editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: Some((base_document, id)),
        }
    }

    fn perform(view: &mut crate::EditorView, command: crate::EditorCommand) {
        use imba::View;
        let mut store = imba::store::Store::new();
        let ui = imba::UiCtx::cold();
        view.perform(&mut store, &ui, command, fx!());
    }

    fn toggled(view: &mut crate::EditorView, at: u32) -> crate::markup::InlayKey {
        perform(view, crate::EditorCommand::ToggleBeforeInlay { at });
        let cards = view.document.before_inlay_views(view.editor);
        assert_eq!(cards.len(), 1, "one card standing");
        cards[0].0
    }

    fn card(view: &crate::EditorView) -> crate::BeforeInlay {
        view.document.before_inlay_views(view.editor)[0].1.clone()
    }

    fn tick(view: &mut crate::EditorView, key: crate::markup::InlayKey, at_ms: f64) {
        perform(
            view,
            crate::EditorCommand::Inlay {
                key,
                command: Box::new(BeforeCommand::Tick(
                    imba::anim::AnimationClock::from_millis(at_ms),
                )),
            },
        );
    }

    #[test]
    fn the_card_grows_in_and_ticks_keep_the_hosts_focus() {
        let mut view = joined();
        view.focus_text();
        let key = toggled(&mut view, SOURCE.find("TWO!").unwrap() as u32);
        assert!(card(&view).is_appearing(), "born in motion");

        let mut at = 0.0;
        for _ in 0..64 {
            if !card(&view).is_appearing() {
                break;
            }
            tick(&mut view, key, at);
            at += 40.0;
        }
        assert!(!card(&view).is_appearing(), "the growth settles");
        assert_eq!(
            view.focus(),
            crate::EditorFocus::Text,
            "ticks never stole the host's caret"
        );
    }

    #[test]
    fn the_card_responds_to_the_rewrap_door() {
        let mut view = joined();
        let key = toggled(&mut view, SOURCE.find("TWO!").unwrap() as u32);
        let before = card(&view).card_width();
        perform(
            &mut view,
            crate::EditorCommand::Inlay {
                key,
                command: Box::new(BeforeCommand::Rewrap(before + 120.0)),
            },
        );
        assert_eq!(
            card(&view).card_width(),
            before + 120.0,
            "the card re-laid at the reported width"
        );
    }

    #[test]
    fn clicking_the_card_focuses_it() {
        let mut view = joined();
        view.focus_text();
        let key = toggled(&mut view, SOURCE.find("TWO!").unwrap() as u32);
        assert_eq!(
            card(&view).card_focus(),
            crate::EditorFocus::None,
            "born blurred"
        );
        perform(
            &mut view,
            crate::EditorCommand::Inlay {
                key,
                command: Box::new(BeforeCommand::Editor(crate::EditorCommand::Click {
                    kind: crate::ClickKind::Set,
                    point: skia_safe::Point::new(2.0, 2.0),
                })),
            },
        );
        assert_eq!(
            view.focus(),
            crate::EditorFocus::Inlay(key),
            "the host routes position-less events to the card"
        );
        assert_eq!(
            card(&view).card_focus(),
            crate::EditorFocus::Text,
            "the card's own caret is live"
        );
    }
}

#[test]
fn coinciding_decoration_starts_compose_instead_of_dropping() {
    let source = "needle haystack\n";
    let paint = |with_tint: bool| -> Vec<u8> {
        let mut document = plain_document(source);
        let editor = document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            &test_fonts(),
            &test_theme(),
            &mut imba::effect::Batch::new().effects(),
        );

        let token = document.add_markup();
        document.show_markup(editor, token);
        let mut marks = crate::markup::Markup::new();
        marks.push_styled(0..6, crate::theme::StyleId::Keyword);
        document.replace_markup(token, marks, &[0..6], &test_fonts(), &test_theme(), fx!());

        if with_tint {
            let tint = document.add_markup();
            document.show_markup(editor, tint);
            let mut marks = crate::markup::Markup::new();
            marks.push_styled(0..6, crate::theme::StyleId::Match);
            document.replace_markup(tint, marks, &[0..6], &test_fonts(), &test_theme(), fx!());
        }
        let mut surface = skia_safe::surfaces::raster_n32_premul((400, 40)).expect("surface");
        surface.canvas().clear(skia_safe::Color::BLACK);
        document.paint(
            editor,
            surface.canvas(),
            skia_safe::Rect::from_wh(400.0, 40.0),
            false,
            &test_fonts(),
            &test_theme(),
        );
        let image = surface.image_snapshot();
        let pixmap = image.peek_pixels().expect("raster pixels");
        pixmap.bytes().expect("pixel bytes").to_vec()
    };

    assert_ne!(
        paint(true),
        paint(false),
        "a tint starting exactly at a token start must still paint"
    );
}

#[test]
fn editor_scroll_bench() {
    use std::time::{Duration, Instant};
    let body = "alpha beta gamma delta epsilon zeta eta theta\n".repeat(4_000);
    let (mut document, editor) = ime_doc(&body);
    let fonts = test_fonts();
    let theme = test_theme();

    {
        let line_len = "alpha beta gamma delta epsilon zeta eta theta\n".len() as u32;
        let mut tints = crate::markup::Markup::new();
        for line in 0..4_000u32 {
            let base = line * line_len;
            for (len, at) in [
                (5u32, 0u32),
                (4, 6),
                (5, 11),
                (5, 17),
                (7, 23),
                (4, 31),
                (3, 36),
            ] {
                tints.push_styled(base + at..base + at + len, crate::theme::StyleId::Match);
            }
            if line % 3 == 0 {
                tints.push_styled_covering(base..base + line_len, crate::theme::StyleId::Input);
            }
        }
        let markup = document.add_markup();
        document.show_markup(editor, markup);
        document.replace_markup(
            markup,
            tints,
            &[],
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
    }
    let band_height = 800.0f32;
    let frame = |top: f32| -> Duration {
        let started = Instant::now();
        let viewport = crate::viewport::EditorViewport::build(
            &document,
            editor,
            top..top + band_height,
            true,
            true,
            None,
            &fonts,
            &theme,
        );
        std::hint::black_box(&viewport);
        started.elapsed()
    };

    let step = 9.0f32;
    for index in 0..100 {
        frame(index as f32 * step);
    }
    let mut samples: Vec<Duration> = Vec::new();
    for index in 0..800 {
        samples.push(frame(100.0 * step + index as f32 * step));
    }
    samples.sort();
    let p50 = samples[samples.len() / 2].as_secs_f64() * 1000.0;
    let p95 = samples[samples.len() * 95 / 100].as_secs_f64() * 1000.0;
    eprintln!("[bench] editor scroll: frame build p50={p50:.4}ms p95={p95:.4}ms");
    imba::perf::record("editor-scroll", "build_p50_ms", p50);
    imba::perf::record("editor-scroll", "build_p95_ms", p95);
}

#[test]
fn line_marks_sweep_matches_the_per_line_query() {
    let mut seed = 0xdec0_5eedu64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..60 {
        let mut builder = crate::markup::Markup::builder();
        let extent = 2_000u32;
        for _ in 0..(rand() % 60 + 5) {
            let start = (rand() % u64::from(extent)) as u32;
            let len = (rand() % 120) as u32;
            let range = start..(start + len).min(extent);
            match rand() % 5 {
                0 => builder.push_inline(range, crate::theme::StyleId::Match),
                1 => builder.push_styled(range, crate::theme::StyleId::Input),
                2 => builder.push_hidden(range),
                3 => builder.push_block_styles(range, [crate::theme::StyleId::Match]),
                _ => builder.push_alignment(range, crate::theme::TextAlignment::Center),
            }
        }
        let mut markup = builder.finish();
        if rand() % 2 == 0 {
            let start = (rand() % u64::from(extent)) as u32;
            markup.set_unhide(start..(start + 200).min(extent));
        }
        let overlaid = crate::markup::OverlaidMarkup::plain(&markup);

        let mut lines = Vec::new();
        let mut at = 0u32;
        while at < extent {
            let len = (rand() % 90 + 10) as u32;
            lines.push(at..(at + len).min(extent));
            at += len;
        }
        let mut sweep = overlaid.line_marks_sweep(0, Some(480.0));
        let (mut a_inline, mut a_hidden) = (Vec::new(), Vec::new());
        let (mut b_inline, mut b_hidden) = (Vec::new(), Vec::new());
        for line in lines {
            let swept = sweep.line(line.clone(), &mut a_inline, &mut a_hidden);
            let queried = overlaid.line_marks_foldables_in(
                line.clone(),
                Some(480.0),
                &mut b_inline,
                &mut b_hidden,
            );
            assert_eq!(a_inline, b_inline, "round {round} line {line:?}");
            assert_eq!(a_hidden, b_hidden, "round {round} line {line:?}");
            assert_eq!(swept, queried, "round {round} line {line:?}");
        }
    }
}

#[test]
fn popups_mint_from_the_visible_band_with_zero_flow_impact() {
    let fonts = test_fonts();
    let theme = test_theme();
    let source: String = (0..200).map(|i| format!("line number {i}\n")).collect();
    let mut view = crate::EditorView::complete(plain_document(&source), 400.0, &fonts, &theme);
    let editor = view.editor;

    let before_height = view.document.content_height(editor);

    let line_start = source
        .match_indices('\n')
        .nth(49)
        .map(|(at, _)| at as u32 + 1)
        .expect("line 50");
    let range = line_start + 5..line_start + 11;
    let host = imba::overlay::OverlayHost("test-popup-host");
    let markup = crate::markup::MarkupId::mint();
    view.document.ensure_document_markup(markup);
    view.document.push_inlay(
        markup,
        range.clone(),
        crate::markup::Inlay::new(
            crate::markup::InlayMode::Popup(crate::markup::PopupSpec {
                host,
                position: imba::overlay::fit::PreferredPosition::At {
                    x: imba::overlay::fit::RangeEnd::Begin,
                    side: imba::overlay::fit::Side::Bottom,
                    align: imba::overlay::fit::Align::Left,
                },
            }),
            FixedInlay {
                width: 120.0,
                height: 80.0,
            },
        ),
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    assert_eq!(
        view.document.content_height(editor),
        before_height,
        "a popup must not move the layout"
    );

    let store = imba::Store::new();
    let ui = imba::UiCtx::cold();
    ui.set(crate::env::UiFonts(test_fonts()));
    let arena = imba::arena::Arena::default();
    let constraints = imba::constraints::Constraints {
        min: skia_safe::Size::default(),
        max: skia_safe::Size::new(400.0, f32::MAX),
    };

    let marker_y = view.document.height_before(editor, range.start);
    fn drain(
        view: &crate::EditorView,
        arena: &imba::arena::Arena,
        store: &imba::Store,
        ui: &imba::UiCtx,
        constraints: imba::constraints::Constraints,
        viewport: skia_safe::Rect,
    ) -> Vec<(skia_safe::Rect, imba::overlay::OverlayHost)> {
        let thunk = imba::Layout::layout(
            imba::View::display(view, arena, store, ui),
            arena,
            constraints,
        );
        let mut widget = imba::Thunk::realize(thunk, arena, viewport);
        imba::Widget::overlays(&mut widget)
            .into_iter()
            .map(|overlay| (overlay.anchor, overlay.host))
            .collect()
    }

    assert!(
        drain(
            &view,
            &arena,
            &store,
            &ui,
            constraints,
            skia_safe::Rect::from_xywh(0.0, 0.0, 400.0, 300.0)
        )
        .is_empty(),
        "a popup below the band must not mint"
    );

    let band = skia_safe::Rect::from_xywh(0.0, marker_y - 100.0, 400.0, 300.0);
    let minted = drain(&view, &arena, &store, &ui, constraints, band);
    assert_eq!(minted.len(), 1, "one visible popup, one request");
    let (anchor, minted_host) = minted[0];
    assert_eq!(minted_host, host);
    assert!(
        (anchor.top - marker_y).abs() < 1.0,
        "anchored at the marker's line: {} vs {marker_y}",
        anchor.top
    );
    assert!(anchor.left > 0.0, "past the range-begin x");
    assert!(anchor.width() > 4.0, "spans the range: {}", anchor.width());

    view.document.set_caret(editor, 0);
    view.document.insert(
        editor,
        "inserted\nlines\n",
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let moved_range = view
        .document
        .popups_in(editor, 0..u32::MAX)
        .pop()
        .map(|(_, range, _, _)| range)
        .expect("the marker survived the edit");
    let moved_y = view.document.height_before(editor, moved_range.start);
    assert!(moved_y > marker_y, "the marker rode the edit down");
    let minted = drain(
        &view,
        &arena,
        &store,
        &ui,
        constraints,
        skia_safe::Rect::from_xywh(0.0, moved_y - 100.0, 400.0, 300.0),
    );
    assert_eq!(minted.len(), 1);
    assert!(
        (minted[0].0.top - moved_y).abs() < 1.0,
        "the anchor tracks edits: {} vs {moved_y}",
        minted[0].0.top
    );
}

#[test]
fn sticky_lines_pin_the_enclosing_scopes() {
    let fonts = test_fonts();
    let theme = test_theme();
    let source: String = (0..200).map(|i| format!("line number {i}\n")).collect();
    let line_start = |line: usize| -> u32 {
        match line {
            0 => 0,
            _ => source
                .match_indices('\n')
                .nth(line - 1)
                .map(|(at, _)| at as u32 + 1)
                .expect("a line start"),
        }
    };

    let outer = line_start(10)..line_start(150);
    let inner = line_start(40)..line_start(120);
    let mut syntax =
        crate::markup::Syntax::new("toy".to_owned(), None, crate::markup::Markup::new());
    syntax.push_outline_item(
        outer.clone(),
        crate::markup::OutlineItem {
            title: "outer".to_owned(),
        },
    );
    syntax.push_outline_item(
        inner.clone(),
        crate::markup::OutlineItem {
            title: "inner".to_owned(),
        },
    );
    let document = crate::document::Document::new(
        text::Text::from_string_exact(&source),
        crate::markup::Markup::new(),
    )
    .with_syntax(syntax, &[]);
    let mut view = crate::EditorView::complete(document, 400.0, &fonts, &theme);

    view.gutter_width = 100.0;
    let editor = view.editor;
    let line_height = view.document.content_height(editor) / 201.0;

    let probe = line_start(60);
    assert_eq!(
        view.document.outline_enclosing(probe),
        vec![outer.clone(), inner.clone()],
        "outermost first"
    );
    assert!(view.document.outline_enclosing(line_start(5)).is_empty());

    let store = imba::Store::new();
    let ui = imba::UiCtx::cold();
    ui.set(crate::env::UiFonts(test_fonts()));
    let arena = imba::arena::Arena::default();
    let constraints = imba::constraints::Constraints {
        min: skia_safe::Size::default(),
        max: skia_safe::Size::new(400.0, f32::MAX),
    };
    let drain =
        |viewport: skia_safe::Rect| -> Vec<imba::overlay::Overlay<'_, crate::EditorCommand>> {
            let thunk = imba::Layout::layout(
                imba::View::display(&view, &arena, &store, &ui),
                &arena,
                constraints,
            );
            let mut widget = imba::Thunk::realize(thunk, &arena, viewport);
            imba::Widget::overlays(&mut widget)
                .into_iter()
                .filter(|overlay| overlay.host == crate::sticky::HOST)
                .collect()
        };

    assert!(
        drain(skia_safe::Rect::from_xywh(0.0, 0.0, 400.0, 300.0)).is_empty(),
        "unscrolled: no sticky stack"
    );

    assert!(
        drain(skia_safe::Rect::from_xywh(
            0.0,
            line_height * 10.0,
            400.0,
            300.0
        ))
        .is_empty(),
        "a header at the top edge stays a plain line"
    );

    let minted = drain(skia_safe::Rect::from_xywh(
        0.0,
        line_height * 20.0,
        400.0,
        300.0,
    ));
    assert_eq!(minted.len(), 1, "one request for the whole stack");
    assert!(
        (minted[0].anchor.height() - line_height).abs() < 1.0,
        "one row"
    );

    let top = line_height * 60.0;
    let minted = drain(skia_safe::Rect::from_xywh(0.0, top, 400.0, 300.0));
    assert_eq!(minted.len(), 1);
    let overlay = minted.into_iter().next().unwrap();
    assert!((overlay.anchor.top - top).abs() < 0.5, "pinned at the top");
    assert!(
        (overlay.anchor.height() - line_height * 2.0).abs() < 1.0,
        "two rows: {}",
        overlay.anchor.height()
    );

    let placed = overlay.content.layout(
        &arena,
        skia_safe::Size::new(400.0, 300.0),
        skia_safe::Rect::from_xywh(0.0, 0.0, overlay.anchor.width(), overlay.anchor.height()),
    );
    assert_eq!(placed.len(), 1, "the stack is one widget");
    let (at, thunk) = placed.into_iter().next().unwrap();
    assert_eq!((at.x, at.y), (0.0, 0.0));
    let size = imba::Thunk::size(&thunk);
    let widget = imba::Thunk::realize(thunk, &arena, skia_safe::Rect::from_size(size));
    let pressed = imba::Widget::handle_event(
        &widget,
        &arena,
        &imba::event::Event::MouseDown {
            point: skia_safe::Point::new(50.0, line_height * 1.5),
            button: imba::event::MouseButton::Left,
            mods: imba::event::Modifiers::default(),
            count: 1,
        },
        skia_safe::Rect::from_size(size),
    );
    match pressed {
        imba::event::EventResult::Command(crate::EditorCommand::RevealAt { byte }) => {
            assert_eq!(byte, inner.start, "the row's own scope header");
        }
        _ => panic!("a press must command the jump"),
    }

    // The band and its divider run EDGE TO EDGE of the hosting pane,
    // even when the editor sits inset inside a wider host.
    let minted = drain(skia_safe::Rect::from_xywh(0.0, top, 400.0, 300.0));
    let overlay = minted.into_iter().next().unwrap();
    let anchor =
        skia_safe::Rect::from_xywh(120.0, 0.0, overlay.anchor.width(), overlay.anchor.height());
    let placed = overlay
        .content
        .layout(&arena, skia_safe::Size::new(1000.0, 300.0), anchor);
    let (at, thunk) = placed.into_iter().next().unwrap();
    assert_eq!(at.x, 0.0, "the band starts at the pane's left edge");
    assert!(
        (imba::Thunk::size(&thunk).width - 1000.0).abs() < 0.5,
        "the band spans the pane: {}",
        imba::Thunk::size(&thunk).width
    );
}

#[test]
fn lazy_languages_load_on_first_parse_only() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Nothing;
    impl crate::reparse::SyntaxLanguage for Nothing {
        fn parse(
            &self,
            _text: &text::Text,
            _range: std::ops::Range<u32>,
            _old: Option<&dyn crate::reparse::SyntaxTree>,
        ) -> Option<Box<dyn crate::reparse::SyntaxTree>> {
            None
        }
        fn markup_for_changes(
            &self,
            _text: &text::Text,
            _range: std::ops::Range<u32>,
            _tree: &dyn crate::reparse::SyntaxTree,
            _changed: &[std::ops::Range<u32>],
            _replacement: &mut crate::markup::MarkupBuilder,
            _invalidated: &mut Vec<std::ops::Range<u32>>,
            _fonts: &skia_safe::textlayout::FontCollection,
            _theme: &crate::theme::Theme,
        ) {
        }
    }

    let loads = Arc::new(AtomicUsize::new(0));
    let flaky = Arc::new(AtomicUsize::new(0));
    let mut languages = crate::reparse::SyntaxLanguages::new();
    let counted = Arc::clone(&loads);
    languages.register_lazy(
        &["lazy", "lz"],
        None,
        Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Some(Arc::new(Nothing) as Arc<dyn crate::reparse::SyntaxLanguage>)
        }),
    );
    let failing = Arc::clone(&flaky);
    languages.register_lazy(
        &["flaky"],
        None,
        Arc::new(move || match failing.fetch_add(1, Ordering::SeqCst) {
            0 => None,
            _ => Some(Arc::new(Nothing) as Arc<dyn crate::reparse::SyntaxLanguage>),
        }),
    );

    assert!(languages.knows("lazy") && languages.knows("lz"));
    assert!(!languages.knows("unknown"));
    assert!(languages.get("lazy").is_none(), "get is resident-only");
    assert_eq!(
        loads.load(Ordering::SeqCst),
        0,
        "registration loads nothing"
    );

    let clone = languages.clone();
    assert!(languages.ensure("lz").is_some());
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    assert!(clone.get("lazy").is_some(), "clones share the slot");
    assert!(languages.ensure("lazy").is_some());
    assert_eq!(loads.load(Ordering::SeqCst), 1, "resident: no reload");

    assert!(languages.ensure("flaky").is_none());
    assert!(languages.get("flaky").is_none());
    assert!(languages.ensure("flaky").is_some(), "the retry lands");
    assert_eq!(flaky.load(Ordering::SeqCst), 2);
}

#[test]
fn selection_rects_cover_the_range_row_by_row() {
    let source: String = (0..400).map(|i| format!("line number {i}\n")).collect();
    let (document, editor) = ime_doc(&source);
    let fonts = test_fonts();
    let theme = test_theme();
    let line_start = |line: usize| -> u32 {
        source
            .match_indices('\n')
            .nth(line - 1)
            .map(|(at, _)| at as u32 + 1)
            .expect("a line start")
    };

    assert!(document
        .selection_content_rects(editor, 5..5, &fonts, &theme)
        .is_empty());

    let one = document.selection_content_rects(editor, 2..6, &fonts, &theme);
    assert_eq!(one.len(), 1, "a single-line selection is one rect");
    let (x, y, width, height) = one[0];
    assert!(x >= 0.0 && y >= 0.0);
    assert!(width > 0.0 && height > 0.0);
    let caret = document
        .caret_content_rect(editor, 2, &fonts, &theme)
        .expect("the caret at the range start");
    assert!(
        (y - (caret.1 - (height - caret.3).max(0.0))).abs() < height,
        "the rect sits on the caret's line"
    );

    let many =
        document.selection_content_rects(editor, line_start(2)..line_start(5), &fonts, &theme);
    assert_eq!(many.len(), 3, "one rect per covered row, got {many:?}");
    for pair in many.windows(2) {
        assert!(
            pair[1].1 > pair[0].1,
            "rows descend: {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }

    let all = document.selection_content_rects(editor, 0..u32::MAX, &fonts, &theme);
    assert!(
        all.len() <= 256,
        "the cap holds: {} rects for 400 lines",
        all.len()
    );
    assert!(!all.is_empty(), "a select-all still answers what shows");
}

#[test]
fn the_first_baseline_is_cached_at_shape_time() {
    let fonts = test_fonts();
    let theme = test_theme();
    for source in [
        "plain line\n",
        "# A header line\n",
        "\n",
        "a long line that will certainly soft wrap when laid out at a narrow width, yes\n",
    ] {
        let text = text::Text::from_string_exact(source);
        let markup = crate::markup::Markup::builder().finish();
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        let line = 0..source.len() as u32;
        let marks = markup.marks_inline_hidden_in(line.clone(), &mut inline, &mut hidden);
        let shaped = crate::shaped_line::ShapedLine::new(
            &mut text.view(),
            crate::markup::OverlaidMarkup::plain(&markup),
            line,
            &marks,
            &inline,
            &hidden,
            &fonts,
            &theme,
            200.0,
            0.0,
            true,
        );
        assert_eq!(
            shaped.first_baseline(),
            shaped.queried_first_baseline(),
            "the cached baseline matches skia for {source:?}"
        );
        assert!(
            shaped
                .first_baseline()
                .is_some_and(|baseline| baseline > 0.0),
            "and is a real baseline for {source:?}"
        );
    }
}

#[test]
fn deleting_the_text_drops_its_markup_intervals() {
    let mut builder = crate::markup::Markup::builder();
    for block in 0..1000u32 {
        builder.push_block_styles(block * 10..block * 10 + 5, [crate::theme::StyleId::Strong]);
    }
    let mut markup = builder.finish();
    assert_eq!(markup.interval_count(), 1000);
    assert_eq!(markup.query_count(), 1000);

    let source = "x".repeat(10_000);
    let text = text::Text::from_string_exact(&source);
    let mut view = text.view();
    markup.edit(&operation::Operation::delete_at(0, &source), &mut view, 0);

    assert_eq!(markup.query_count(), 0, "no corpse is left to walk");
    assert_eq!(
        markup.interval_count(),
        0,
        "and none is left in the key map"
    );
}

#[test]
fn a_markup_change_reshapes_only_the_lines_it_touches() {
    use std::rc::Rc;
    let source: String = (0..40)
        .map(|i| format!("line number {i} with some words\n"))
        .collect();
    let (mut document, editor) = ime_doc(&source);
    let build = |document: &Document| {
        crate::viewport::EditorViewport::build(
            document,
            editor,
            0.0..10_000.0,
            false,
            false,
            None,
            &test_fonts(),
            &test_theme(),
        )
    };
    let first = build(&document);
    let token = document.add_markup();
    document.show_markup(editor, token);
    let second = build(&document);
    let line30 = first.lines[30].byte_start..first.lines[30].byte_end;
    let mut marks = crate::markup::Markup::new();
    marks.push_styled(line30.clone(), crate::theme::StyleId::Keyword);
    document.replace_markup(
        token,
        marks,
        &[line30.clone()],
        &test_fonts(),
        &test_theme(),
        fx!(),
    );
    let third = build(&document);
    let reshaped: Vec<u32> = second
        .lines
        .iter()
        .zip(&third.lines)
        .filter(|(a, b)| match (&a.shaped, &b.shaped) {
            (Some(a), Some(b)) => !Rc::ptr_eq(a, b),
            _ => false,
        })
        .map(|(a, _)| a.byte_start)
        .collect();
    assert_eq!(
        reshaped,
        vec![line30.start],
        "only the touched line re-shapes after a ranged markup change"
    );
}

#[test]
fn a_small_scroll_keeps_the_overlapping_shaped_lines() {
    use std::rc::Rc;
    let source: String = (0..200)
        .map(|i| format!("line number {i} with some words\n"))
        .collect();
    let (document, editor) = ime_doc(&source);
    let build = |band: std::ops::Range<f32>| {
        crate::viewport::EditorViewport::build(
            &document,
            editor,
            band,
            false,
            false,
            None,
            &test_fonts(),
            &test_theme(),
        )
    };
    let first = build(0.0..800.0);
    let second = build(37.0..837.0);
    let mut overlapping = 0;
    for b in &second.lines {
        let Some(a) = first.lines.iter().find(|a| a.byte_start == b.byte_start) else {
            continue;
        };
        let start = b.byte_start;
        if let (Some(a), Some(b)) = (&a.shaped, &b.shaped) {
            overlapping += 1;
            assert!(Rc::ptr_eq(a, b), "line {start} re-shaped on a small scroll");
        }
    }
    assert!(overlapping >= 10, "the bands overlap: {overlapping}");
}

#[test]
fn translucent_washes_never_double_at_block_seams() {
    // Mixed block metrics (plain text around a fenced code block,
    // plus an inline-code span): the paragraph rects can stand
    // taller than the layout slots, and an overflowing translucent
    // band doubles up with the next row's — horizontal stripes.
    let source =
        "plain one\nplain two\n```\ncode a\ncode b\ncode c\n```\nplain three\nplain four\n";
    let mut document = crate::test_document::fenced_code_document(source);
    let theme = test_theme();
    let editor = document.add_editor(
        400.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let operation = crate::diff::diff(&text::Text::from_string_exact(""), document.text());
    let id = document.add_diff(operation, 0);
    let hunks = document.diff(id).expect("tracked").markup();
    document.show_markup(editor, hunks);
    // The third line renders with DIFFERENT metrics (inline code font).
    let styled = document.add_markup();
    document.show_markup(editor, styled);
    let mut tints = crate::markup::Markup::new();
    tints.push_styled(14..20, crate::theme::StyleId::InlineCode);
    document.replace_markup(
        styled,
        tints,
        &[],
        &test_fonts(),
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let mut surface = skia_safe::surfaces::raster_n32_premul((400, 300)).expect("surface");
    let canvas = surface.canvas();
    canvas.clear(skia_safe::Color::BLACK);
    document.paint(
        editor,
        canvas,
        skia_safe::Rect::from_xywh(0.0, 0.0, 400.0, 300.0),
        false,
        &test_fonts(),
        &theme,
    );
    let image = surface.image_snapshot();
    let pixmap = image.peek_pixels().expect("raster pixels");
    let bytes = pixmap.bytes().expect("pixel bytes");
    let row_bytes = image.width() as usize * 4;
    let x = 300usize; // far right of any glyph
    let content = document.content_height(editor) as usize;
    let first = &bytes[x * 4..x * 4 + 4];
    let first: [u8; 4] = first.try_into().expect("rgba");
    for y in 0..content.min(299) {
        let px = &bytes[y * row_bytes + x * 4..y * row_bytes + x * 4 + 4];
        assert_eq!(
            px, &first,
            "one wash layer everywhere — no doubled band at scanline {y}"
        );
    }
}

#[test]
fn inline_diff_wash_is_seamless_at_retina_scale() {
    // The unified inline face's added wash, painted like production:
    // scale 2 with a fractional pane origin. Any per-row band error
    // (bleed, crown, AA seam) breaks the uniformity.
    let before = "alpha\nbeta\ngamma\n";
    let after = "alpha\n// one\n// two\nlet clamped = x;\nlet rect = y;\nRect::new(\nbeta\ngamma\n";
    let theme = test_theme();
    let left = plain_document(before);
    let mut right = plain_document(after);
    let operation = crate::diff::diff(left.text(), right.text());
    let id = right.add_diff(operation.clone(), left.revision());
    let hunks = right.diff(id).expect("tracked").markup();
    let prepared = crate::split_diff::prepare_marks(&operation, left.text());
    let mut throwaway = imba::effect::Batch::new();
    let quiet = &mut throwaway.effects();
    let extras = right.add_markup();
    right.replace_markup(
        extras,
        prepared.right.clone(),
        &[],
        &test_fonts(),
        &theme,
        quiet,
    );
    let _ = left;
    let editor = right.add_editor(
        400.0,
        None,
        crate::document::EditorBuild::Bounded,
        &[hunks, extras],
        &test_fonts(),
        &theme,
        quiet,
    );

    // Paint at retina scale across a sweep of FRACTIONAL scroll
    // offsets — stripes that come and go with the scroll position
    // are sub-pixel seams between per-row bands.
    for step in 0..20u32 {
        let scroll = step as f32 * 0.05;
        let mut surface = skia_safe::surfaces::raster_n32_premul((800, 1200)).expect("surface");
        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color::BLACK);
        canvas.scale((2.0, 2.0));
        canvas.translate((0.0, -scroll));
        right.paint(
            editor,
            canvas,
            skia_safe::Rect::from_xywh(0.0, 0.0, 400.0, 600.0),
            false,
            &test_fonts(),
            &theme,
        );
        let image = surface.image_snapshot();
        let pixmap = image.peek_pixels().expect("raster pixels");
        let bytes = pixmap.bytes().expect("pixel bytes");
        let row_bytes = image.width() as usize * 4;
        let x = 760usize;
        let content = ((right.content_height(editor) * 2.0) as usize).min(1199);
        let at = |y: usize| -> [u8; 4] {
            bytes[y * row_bytes + x * 4..y * row_bytes + x * 4 + 4]
                .try_into()
                .expect("rgba")
        };
        let washed: Vec<usize> = (0..content).filter(|y| at(*y)[..3] != [0, 0, 0]).collect();
        let (first, last) = (
            *washed.first().expect("the hunk washed"),
            *washed.last().expect("the hunk washed"),
        );
        assert!(last - first > 100, "several washed rows: {first}..{last}");
        // The hunk's own edges may land on partial pixels; the
        // INTERIOR must be one seamless layer.
        for y in first + 2..last.saturating_sub(2) {
            assert_eq!(
                at(y),
                at(first + 2),
                "no stripe at device scanline {y} under scroll {scroll}"
            );
        }
    }
}

#[test]
fn an_edit_above_the_viewport_leaves_an_exact_settle_target() {
    let mut store = imba::store::Store::new();
    let ui = imba::UiCtx::cold();
    let mut document = crate::test_document::plain_document(
        "line 0\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\n",
    );
    let editor = document.add_editor(
        400.0,
        None,
        crate::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &test_theme(),
        fx!(),
    );

    // The widget's Viewport report: the top edge cuts 3px into
    // line 4 — RETAINED state, landed through the command road.
    let line4 = document.text().to_string().find("line 4").unwrap() as u32;
    let top = document.height_before(editor, line4) + 3.0;
    let mut report = |document: &mut Document, top: f32| {
        let width = document.layout_width(editor);
        let mut batch = imba::effect::Batch::new();
        document.perform(
            &mut store,
            &ui,
            editor,
            crate::EditorCommand::Viewport {
                width,
                top,
                bottom: top + 100.0,
                anchor: document.first_visible_byte(editor, top),
            },
            &mut batch.effects(),
        );
    };
    report(&mut document, top);
    assert_eq!(
        document.settle_target(editor),
        None,
        "an untouched viewport has nothing to settle"
    );

    // Two lines land ABOVE the anchor.
    let before = document.height_before(editor, line4);
    let insert = Operation::from_ops([
        Op::Insert("intruder A\nintruder B\n".to_owned()),
        Op::Retain(document.text().to_string().len() as u32),
    ]);
    document.edit(&insert, &test_fonts(), &test_theme(), fx!());
    let after = document.height_before(editor, line4 + "intruder A\nintruder B\n".len() as u32);
    assert!(after > before, "the insert grew the prefix");

    assert_eq!(
        document.settle_target(editor),
        Some(after + 3.0),
        "the door notes exactly where the anchored line went"
    );

    // The next Viewport report (a frame painted at the corrected
    // offset) clears the pending correction.
    report(&mut document, after + 3.0);
    assert_eq!(
        document.settle_target(editor),
        None,
        "the correction landed"
    );

    // An edit BELOW the anchor moves nothing.
    let text_len = document.text().to_string().len() as u32;
    let tail = Operation::from_ops([
        Op::Retain(text_len),
        Op::Insert("\ntrailing noise".to_owned()),
    ]);
    document.edit(&tail, &test_fonts(), &test_theme(), fx!());
    assert_eq!(
        document.settle_target(editor),
        None,
        "content below the anchor never disturbs it"
    );
}

#[test]
fn a_rewrap_keeps_the_viewport_anchor_in_view() {
    use imba::effect::{block_on, Batch, EffectHandler, Message};
    let mut store = imba::store::Store::new();
    let ui = imba::UiCtx::cold();
    let long = "a long enough line that will wrap once the pane narrows down a lot\n";
    let mut document =
        crate::test_document::plain_document(&format!("{}{}", long.repeat(8), "short tail\n"));
    let editor = document.add_editor(
        600.0,
        None,
        crate::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &test_theme(),
        fx!(),
    );

    // The viewport top cuts 2px into the sixth long line.
    let line6 = (long.len() * 5) as u32;
    let top = document.height_before(editor, line6) + 2.0;
    let mut batch = Batch::new();
    document.perform(
        &mut store,
        &ui,
        editor,
        crate::EditorCommand::Viewport {
            width: 600.0,
            top,
            bottom: top + 100.0,
            anchor: document.first_visible_byte(editor, top),
        },
        &mut batch.effects(),
    );
    assert_eq!(document.settle_target(editor), None);

    // The pane narrows. The sync rewrap repairs forward from the
    // anchor; the PREFIX lands through the async repair lane, whose
    // door must note the shift.
    let before = document.height_before(editor, line6);
    let mut batch = Batch::new();
    document.perform(
        &mut store,
        &ui,
        editor,
        crate::EditorCommand::Viewport {
            width: 220.0,
            top,
            bottom: top + 100.0,
            anchor: line6,
        },
        &mut batch.effects(),
    );

    // Drive the repair lane to completion, the way the engine would.
    let workshop = std::sync::Arc::new(crate::env::Workshop::new(
        std::sync::Arc::new(test_fonts),
        crate::theme::Theme::embedded(),
    ));
    let mut rounds = 0;
    loop {
        let launched: Vec<_> = batch
            .drain()
            .into_iter()
            .filter_map(|message| match message {
                Message::Launch(_, effect) | Message::Relaunch(_, _, effect)
                    if effect.is::<crate::repair::RepairEffect>() =>
                {
                    Some(effect)
                }
                _ => None,
            })
            .collect();
        if launched.is_empty() || rounds > 8 {
            break;
        }
        rounds += 1;
        batch = Batch::new();
        for effect in launched {
            let (value, lift) = effect.into_payload().split();
            let repair = value
                .downcast::<crate::repair::RepairEffect>()
                .expect("a repair effect");
            let handler = crate::repair::RepairHandler(std::sync::Arc::clone(&workshop));
            let outcome: Box<dyn std::any::Any + Send + Sync> = Box::new(block_on(Box::pin(
                async move { handler.handle(*repair).await },
            )));
            let command = lift(outcome).expect("a repair lands a command");
            document.perform(&mut store, &ui, editor, command, &mut batch.effects());
        }
    }

    let after = document.height_before(editor, line6);
    assert!(
        after > before + 0.5,
        "the rewrap grew the prefix: {before} -> {after}"
    );
    assert_eq!(
        document.settle_target(editor),
        Some(after + 2.0),
        "the landed rewrap re-aims at the anchored byte"
    );
}
