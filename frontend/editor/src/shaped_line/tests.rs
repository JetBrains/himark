// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use skia_safe::textlayout::FontCollection;

use super::*;

#[test]
fn caret_x_includes_trailing_whitespace() {
    let fonts = font_collection();
    let marks = BlockStyle::default();

    let mut without_spaces = line_paragraph(
        &marks,
        "a",
        fonts.clone(),
        &crate::theme::Theme::embedded(),
        &[],
    );
    without_spaces.layout(800.0);
    let mut with_spaces = line_paragraph(
        &marks,
        "a    ",
        fonts,
        &crate::theme::Theme::embedded(),
        &[],
    );
    with_spaces.layout(800.0);

    let after_a = caret_x_for_display_position(&without_spaces, "a".encode_utf16().count());
    let after_spaces = caret_x_for_display_position(&with_spaces, "a    ".encode_utf16().count());

    assert!(after_spaces > after_a);
}

#[test]
fn caret_x_includes_whitespace_after_emoji() {
    let fonts = font_collection();
    let marks = BlockStyle::default();
    let text = "a😀    ";

    let mut paragraph = line_paragraph(&marks, text, fonts, &crate::theme::Theme::embedded(), &[]);
    paragraph.layout(800.0);

    let after_emoji = caret_x_for_display_position(&paragraph, "a😀".encode_utf16().count());
    let after_spaces = caret_x_for_display_position(&paragraph, "a😀    ".encode_utf16().count());

    assert!(after_spaces > after_emoji);
}

fn font_collection() -> FontCollection {
    crate::test_document::test_fonts_collection().clone()
}

#[test]
fn placeholder_geometry_is_identical_to_typed_text() {
    let fonts = font_collection();
    let theme = crate::theme::Theme::embedded();
    let source = "Message the agent";
    let text = text::text::Text::from_string_exact(source);
    let markup = crate::markup::Markup::new();
    let marks = BlockStyle::default();
    let typed = ShapedLine::new(
        &mut text.view(),
        OverlaidMarkup::plain(&markup),
        0..source.len() as u32,
        &marks,
        &[],
        &[],
        &fonts,
        &theme,
        crate::markup::InlayMeasure {
            width: 500.0,
            store: &imba::store::Store::new(),
            ui: crate::test_document::test_ui(),
        },
        0.0,
        true,
    );
    let placeholder = ShapedLine::placeholder(
        source,
        theme.ui().peeker.dim_text.0,
        &fonts,
        &theme,
        500.0,
        0.0,
    );

    assert_eq!(placeholder.first_baseline(), typed.first_baseline());
    for byte in source
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(source.len()))
    {
        let byte = byte as u32;
        assert_eq!(placeholder.x_at_byte(byte), typed.x_at_byte(byte));
        assert_eq!(placeholder.caret_geometry(byte), typed.caret_geometry(byte));
    }
}

#[test]
fn hard_break_detection_matches_skia() {
    let fonts = font_collection();
    let theme = crate::theme::Theme::embedded();
    let resolved = BlockStyle::default().resolved(&theme);
    let samples = [
        "plain line without any break",
        "trailing newline\n",
        "embedded\nnewline",
        "two\nembedded\nnewlines\n",
        "carriage\rreturn",
        "windows\r\nbreak",
        "vertical\u{b}tab",
        "form\u{c}feed",
        "next\u{85}line",
        "line\u{2028}separator",
        "paragraph\u{2029}separator",
        "a long line that certainly wraps across several rows of output here",
        "a long line that wraps across rows and then ends with a break\n",
        "wrapping\nwith a break early and then a long soft-wrapped tail after it",
        "",
        "\n",
    ];

    let mut compared = 0usize;
    for sample in samples {
        let mut paragraph = super::paragraph_with_max_lines(
            &resolved,
            sample,
            fonts.clone(),
            &theme,
            &[],
            &[],
            None,
            &mut Vec::new(),
        );
        paragraph.layout(220.0);
        let mut rows = paragraph.line_number();
        if rows > 1 && sample.ends_with('\n') {
            rows -= 1;
        }
        for row in 0..rows.saturating_sub(1) {
            let metrics = paragraph
                .get_line_metrics_at(row)
                .expect("laid-out row has metrics");
            let text_end = paragraph
                .get_actual_text_range(row, true)
                .end
                .min(sample.len());
            assert_eq!(
                super::starts_with_hard_break(sample, text_end),
                metrics.hard_break,
                "row {row} of {sample:?} (text ends at {text_end})"
            );
            compared += 1;
        }
    }
    assert!(
        compared >= 12,
        "the battery must reach non-final rows to prove anything: {compared} compared"
    );
}

#[test]
fn blob_runs_paint_exactly_like_the_paragraph() {
    let fonts = font_collection();
    let theme = crate::theme::Theme::embedded();
    // Mixed scripts and an emoji: several fonts, several runs.
    let source = "fn main() выход 🚀 done";
    let text = text::text::Text::from_string_exact(source);
    let markup = crate::markup::Markup::new();
    let marks = BlockStyle::default();
    let mut line = ShapedLine::new(
        &mut text.view(),
        OverlaidMarkup::plain(&markup),
        0..source.len() as u32,
        &marks,
        &[],
        &[],
        &fonts,
        &theme,
        crate::markup::InlayMeasure {
            width: 700.0,
            store: &imba::store::Store::new(),
            ui: crate::test_document::test_ui(),
        },
        0.0,
        true,
    );
    assert!(
        line.blob_runs.is_some(),
        "an undecorated line bakes blob runs"
    );

    let shoot = |line: &ShapedLine| -> Vec<u8> {
        let mut surface = skia_safe::surfaces::raster_n32_premul((700, 60)).expect("surface");
        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color::WHITE);
        line.paint(canvas, 4.0);
        let image = surface.image_snapshot();
        let pixmap = image.peek_pixels().expect("raster pixels");
        pixmap.bytes().expect("pixel bytes").to_vec()
    };

    let blobs = shoot(&line);
    line.drop_blobs_for_test();
    let paragraph = shoot(&line);

    assert!(
        blobs.iter().any(|byte| *byte != 0xff),
        "the line painted something"
    );
    let differing = blobs.iter().zip(&paragraph).filter(|(a, b)| a != b).count();
    assert_eq!(
        differing,
        0,
        "the blob road and Paragraph::paint disagree on {differing} of {} bytes",
        blobs.len()
    );
}

/// The selection law: a band's edges sit exactly where the caret
/// sits — one geometry source for ink, caret and selection. (The
/// skparagraph band road drifted off parley ink on proportional
/// faces; this pins band x to caret x through the whole display
/// mapping.)
#[cfg(feature = "parley-text")]
#[test]
fn selection_edges_sit_on_the_caret() {
    let fonts = font_collection();
    let theme = crate::theme::Theme::embedded();
    let source = "The quick brown fox jumps over the lazy dog";
    let shaped = ShapedLine::placeholder(
        source,
        theme.ui().peeker.dim_text.0,
        &fonts,
        &theme,
        900.0,
        0.0,
    );
    for (from, to) in [(0usize, 9usize), (4, 15), (10, source.len()), (0, source.len())] {
        let rects = shaped.rects_for_range(from as u32..to as u32);
        assert!(!rects.is_empty(), "no band for {from}..{to}");
        let left = rects.iter().map(|r| r.left).fold(f32::MAX, f32::min);
        let right = rects.iter().map(|r| r.right).fold(f32::MIN, f32::max);
        let caret_from = shaped.x_at_byte(from as u32);
        let caret_to = shaped.x_at_byte(to as u32);
        assert!(
            (left - caret_from).abs() < 0.26 && (right - caret_to).abs() < 0.26,
            "band {from}..{to} spans {left}..{right} but carets sit at {caret_from}..{caret_to}"
        );
    }
}
