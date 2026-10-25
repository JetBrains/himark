// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn plain_shaped(text: &str) -> ParleyLine {
    let resolved = BlockStyle::default().resolved(&crate::theme::embedded());
    let display =
        DisplayText::build_segments(false, &[(text.to_owned(), 0)], &[], true, text.len());
    shape_inputs(&resolved, &display, &[], &crate::theme::embedded())
        .shape(crate::test_document::test_ui(), None)
}

#[test]
fn caret_x_includes_trailing_whitespace() {
    let without_spaces = plain_shaped("a");
    let with_spaces = plain_shaped("a    ");

    let after_a = without_spaces.caret_x(1);
    let after_spaces = with_spaces.caret_x(5);

    assert!(after_spaces > after_a);
}

#[test]
fn caret_x_includes_whitespace_after_emoji() {
    let line = plain_shaped("a😀    ");

    let after_emoji = line.caret_x("a😀".len());
    let after_spaces = line.caret_x("a😀    ".len());

    assert!(after_spaces > after_emoji);
}

#[test]
fn placeholder_geometry_is_identical_to_typed_text() {
    let theme = crate::theme::embedded();
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
        None,
        theme.part(crate::theme::PopupChrome::KEY).dim_text.0,
        crate::test_document::test_ui(),
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
fn rows_partition_the_text_at_breaks_and_wraps() {
    let theme = crate::theme::embedded();
    let resolved = BlockStyle::default().resolved(&theme);
    let samples = [
        "plain line without any break",
        "trailing newline\n",
        "embedded\nnewline",
        "two\nembedded\nnewlines\n",
        "carriage\rreturn",
        "windows\r\nbreak",
        "a long line that certainly wraps across several rows of output here",
        "a long line that wraps across rows and then ends with a break\n",
        "wrapping\nwith a break early and then a long soft-wrapped tail after it",
        "",
        "\n",
    ];
    for sample in samples {
        let display = DisplayText::build_segments(
            false,
            &[(sample.to_string(), 0)],
            &[],
            false,
            sample.len(),
        );
        let line = shape_inputs(&resolved, &display, &[], &theme)
            .shape(crate::test_document::test_ui(), Some(220.0));
        let rows = line.rows();
        if sample.is_empty() {
            // The empty layout's synthetic row has no text to cover.
            continue;
        }
        // Rows PARTITION the text: no gaps, no overlaps, full cover.
        let mut at = 0usize;
        for row in &rows {
            assert_eq!(row.text_range.start, at, "gap in rows of {sample:?}");
            at = row.text_range.end;
        }
        assert_eq!(at, sample.len(), "rows do not cover {sample:?}");
        // A LINE FEED always ENDS its row (a \r may precede it as
        // part of the same windows break).
        for row in &rows {
            let inner = &sample[row.text_range.start..row.text_range.end];
            for (offset, ch) in inner.char_indices() {
                if ch == '\n' && offset + 1 < inner.len() {
                    panic!("break inside a row of {sample:?}");
                }
            }
        }
    }
}

#[test]
fn paint_is_deterministic_and_inked() {
    let theme = crate::theme::embedded();
    // Mixed scripts and an emoji: several fonts, several runs.
    let source = "fn main() выход 🚀 done";
    let text = text::text::Text::from_string_exact(source);
    let markup = crate::markup::Markup::new();
    let marks = BlockStyle::default();
    let line = ShapedLine::new(
        &mut text.view(),
        OverlaidMarkup::plain(&markup),
        0..source.len() as u32,
        &marks,
        &[],
        &[],
        &theme,
        crate::markup::InlayMeasure {
            width: 700.0,
            store: &imba::store::Store::new(),
            ui: crate::test_document::test_ui(),
        },
        0.0,
        true,
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

    let first = shoot(&line);
    let second = shoot(&line);
    assert!(
        first.iter().any(|byte| *byte != 0xff),
        "the line painted something"
    );
    assert_eq!(first, second, "painting the same line twice differs");
}

/// The selection law: a band's edges sit exactly where the caret
/// sits — one geometry source for ink, caret and selection. (The
/// skparagraph band road drifted off parley ink on proportional
/// faces; this pins band x to caret x through the whole display
/// mapping.)
#[test]
fn selection_edges_sit_on_the_caret() {
    let theme = crate::theme::embedded();
    let source = "The quick brown fox jumps over the lazy dog";
    let shaped = ShapedLine::placeholder(
        source,
        None,
        theme.part(crate::theme::PopupChrome::KEY).dim_text.0,
        crate::test_document::test_ui(),
        &theme,
        900.0,
        0.0,
    );
    for (from, to) in [
        (0usize, 9usize),
        (4, 15),
        (10, source.len()),
        (0, source.len()),
    ] {
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
