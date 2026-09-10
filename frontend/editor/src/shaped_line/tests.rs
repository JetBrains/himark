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
    crate::embedded_fonts::collection()
}

#[test]
fn placeholder_geometry_is_identical_to_typed_text() {
    let fonts = font_collection();
    let theme = crate::theme::Theme::embedded();
    let source = "Message the agent";
    let text = text::Text::from_string_exact(source);
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
        500.0,
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
