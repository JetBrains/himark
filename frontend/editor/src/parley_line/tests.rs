// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

/// The spike's parity meter: both roads shape the SAME bytes
/// (the embedded face), paint through skia, and the diff metric
/// prints — the number that prices the migration.
#[test]
fn parley_shapes_paints_and_answers_geometry() {
    let text = "fn main() { простой текст } -> done";
    let theme = crate::theme::Theme::embedded();
    let base = theme.base();
    let size = base.font_size.expect("theme carries a base size");
    let color = base.color.expect("theme carries a base ink");

    let line = ParleyLine::shape(
        text,
        crate::embedded_fonts::FAMILY,
        size,
        &[SpanStyle::plain(color)],
    );
    assert!(line.width() > 0.0, "the line has advance");
    assert!(line.first_baseline().is_some(), "the line has a baseline");

    // Caret x is monotonic over char boundaries.
    let mut last = -1.0f32;
    for (byte, _) in text.char_indices() {
        let x = line.caret_x(byte);
        assert!(
            x >= last - 0.01,
            "caret x goes backwards at byte {byte}: {x} < {last}"
        );
        last = x;
    }

    // Selection rects cover a middle word.
    let from = text.find("простой").unwrap();
    let rects = line.rects(from..from + "простой".len());
    assert!(!rects.is_empty(), "selection has geometry");

    // Paint both roads side by side; the diff metric is the
    // spike's deliverable, printed for the record.
    let paint_to = |painter: &dyn Fn(&skia_safe::Canvas)| -> Vec<u8> {
        let mut surface = skia_safe::surfaces::raster_n32_premul((900, 60)).expect("surface");
        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color::WHITE);
        painter(canvas);
        let image = surface.image_snapshot();
        let pixmap = image.peek_pixels().expect("pixels");
        pixmap.bytes().expect("bytes").to_vec()
    };

    let parley_pixels = paint_to(&|canvas| line.paint(canvas, (0.0, 4.0)));
    assert!(
        parley_pixels.iter().any(|byte| *byte != 0xff),
        "parley painted something"
    );

    let shaped = crate::shaped_line::ShapedLine::placeholder(
        text,
        color,
        &crate::embedded_fonts::collection(),
        &theme,
        900.0,
        0.0,
    );
    let skparagraph_pixels = paint_to(&|canvas| shaped.paint(canvas, 4.0));

    let mut diff_sum: u64 = 0;
    let mut diff_pixels = 0usize;
    for (a, b) in parley_pixels.chunks(4).zip(skparagraph_pixels.chunks(4)) {
        let delta: u64 = a
            .iter()
            .zip(b)
            .map(|(x, y)| (*x as i64 - *y as i64).unsigned_abs())
            .sum();
        diff_sum += delta;
        if delta > 32 {
            diff_pixels += 1;
        }
    }
    let total = parley_pixels.len() / 4;
    eprintln!(
        "[parley-parity] width parley {:.1} vs sk {:.1}; mean channel diff {:.2}; {}/{} pixels differ",
        line.width(),
        {
            let (left, right) = shaped.extent_for_test();
            right - left
        },
        diff_sum as f64 / (total as f64 * 4.0),
        diff_pixels,
        total
    );
    // Catastrophe floor only — tofu or a blank road fails; honest
    // shaper differences do not.
    assert!(
        (diff_pixels as f64) < total as f64 * 0.25,
        "the two roads disagree on more than a quarter of the canvas"
    );
}

/// THE WATCH: fallback quality over the glyphs our users actually
/// type. Pixel parity is not the goal — tofu is the failure.
/// Hard classes (CJK, emoji) fail the test; known-risk classes
/// (U+276F prompt arrows, Nerd Font private-use glyphs — fontique
/// falls back per SCRIPT, not per character) report loudly and
/// keep watching.
#[cfg(target_os = "macos")]
#[test]
fn fallback_corpus_keeps_its_eyes_open() {
    let corpus: &[(&str, &str, bool)] = &[
        ("ascii", "fn main() -> done", true),
        ("cyrillic", "простой текст", true),
        ("cjk-zh", "中文排版测试", true),
        ("cjk-ja", "日本語の組版", true),
        ("korean", "한국어 조판", true),
        ("arabic", "نص عربي", true),
        ("emoji", "👍 🚀 😀", true),
        ("emoji-zwj", "👨‍👩‍👧‍👦", true),
        ("emoji-tone", "👍🏽", true),
        ("prompt-arrow", "❯ ❮", false),
        ("powerline", "\u{e0b0}\u{e0b2}", false),
        ("math", "∑ ∀x ∈ ℝ", false),
        ("box-drawing", "┌─┬─┐│└┘", false),
    ];

    let mut failures = Vec::new();
    for (name, text, must) in corpus {
        let line = ParleyLine::shape(
            text,
            "Menlo",
            24.0,
            &[SpanStyle::plain(skia_safe::Color::BLACK)],
        );
        let (families, notdefs, total) = line.coverage_for_test();
        let verdict = if notdefs == 0 { "ok" } else { "TOFU" };
        eprintln!(
            "[fallback] {name:<13} {verdict:<4} {notdefs}/{total} notdef; fonts: {}",
            families.join(", ")
        );
        if notdefs > 0 && *must {
            failures.push(format!("{name}: {notdefs}/{total} notdef in {text:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "hard fallback classes carry tofu:\n{}",
        failures.join("\n")
    );
}

/// GEOMETRY is correctness (selections and carets must not drift
/// at flip time) — caret x per char boundary against the
/// SkParagraph road on the shared face; tolerance in fractions of
/// the font size, not pixel hunting.
#[test]
fn caret_geometry_tracks_the_skparagraph_road() {
    let text = "fn main() { return 42; } // готово";
    let theme = crate::theme::Theme::embedded();
    let base = theme.base();
    let size = base.font_size.expect("size");
    let color = base.color.expect("ink");

    let parley = ParleyLine::shape(
        text,
        crate::embedded_fonts::FAMILY,
        size,
        &[SpanStyle::plain(color)],
    );
    // The reference must stay RAW skparagraph: a ShapedLine under
    // HIMARK_PARLEY answers geometry from its own twin, and the
    // comparison would measure parley against parley on different
    // family chains.
    let mut paragraph = crate::shaped_line::line_paragraph(
        &crate::markup::BlockStyle::default(),
        text,
        crate::embedded_fonts::collection(),
        &theme,
        &[],
    );
    paragraph.layout(f32::MAX / 2.0);

    let mut worst = 0.0f32;
    for (byte, _) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        let ours = parley.caret_x(byte);
        let display = text[..byte].encode_utf16().count();
        let theirs = crate::shaped_line::caret_x_for_display_position(&paragraph, display);
        worst = worst.max((ours - theirs).abs());
    }
    eprintln!(
        "[parley-geometry] worst caret drift {worst:.3}px over {} chars",
        text.chars().count()
    );
    assert!(
        worst < size * 0.2,
        "caret drift {worst}px exceeds a fifth of the font size"
    );
}

/// Styled spans carry their colors and weights into the blobs.
#[test]
fn styled_spans_reach_the_blobs() {
    let text = "plain BOLD colored";
    let bold_from = text.find("BOLD").unwrap();
    let color_from = text.find("colored").unwrap();
    let line = ParleyLine::shape(
        text,
        crate::embedded_fonts::FAMILY,
        24.0,
        &[
            SpanStyle::plain(skia_safe::Color::BLACK),
            SpanStyle {
                bold: true,
                ..SpanStyle::plain(skia_safe::Color::BLACK)
            }
            .at(bold_from),
            SpanStyle::plain(skia_safe::Color::RED).at(color_from),
        ],
    );
    let blobs = line.blobs();
    assert!(
        blobs.len() >= 2,
        "styled runs split into blobs: {}",
        blobs.len()
    );
    assert!(
        blobs
            .iter()
            .any(|(_, color)| *color == skia_safe::Color::RED),
        "the colored span kept its ink"
    );
}

/// The editor chain must land on a MONOSPACE face even when its
/// first names are unenumerable (the A/B's catch: "SF Mono" is
/// hidden on some systems; falling to a proportional default
/// dragged caret and band rects with it).
#[cfg(target_os = "macos")]
#[test]
fn the_editor_family_chain_lands_monospace() {
    let families = [
        "SF Mono",
        "Menlo",
        "JetBrains Mono",
        "Consolas",
        "monospace",
    ];
    let narrow = ParleyLine::shape_with_families(
        "iiiiiiiiii",
        &families,
        24.0,
        None,
        &[SpanStyle::plain(skia_safe::Color::BLACK)],
    );
    let wide = ParleyLine::shape_with_families(
        "mmmmmmmmmm",
        &families,
        24.0,
        None,
        &[SpanStyle::plain(skia_safe::Color::BLACK)],
    );
    eprintln!(
        "[parley-family] i-run {:.1} vs m-run {:.1}",
        narrow.width(),
        wide.width()
    );
    assert!(
        (narrow.width() - wide.width()).abs() < 0.5,
        "the chain resolved to a proportional face: i {} vs m {}",
        narrow.width(),
        wide.width()
    );
}

/// Which face actually serves BOLD through the editor chain — the
/// "not bold enough" diagnosis.
#[cfg(target_os = "macos")]
#[test]
fn bold_runs_resolve_a_bold_cut() {
    let text = "normal BOLDBOLD normal";
    let from = text.find("BOLD").unwrap();
    let to = from + "BOLDBOLD".len();
    let line = ParleyLine::shape_with_families(
        text,
        &["SF Mono", "Menlo", "monospace"],
        24.0,
        None,
        &[
            SpanStyle::plain(skia_safe::Color::BLACK),
            SpanStyle {
                bold: true,
                ..SpanStyle::plain(skia_safe::Color::BLACK)
            }
            .at(from),
            SpanStyle::plain(skia_safe::Color::BLACK).at(to),
        ],
    );
    let report = line.face_report_for_test();
    for (postscript, embolden, weight) in &report {
        eprintln!("[parley-bold] run face {postscript:?} embolden {embolden} weight {weight:?}");
    }
    assert!(
        report.iter().any(|(name, embolden, _)| {
            name.as_deref().map_or(false, |n| n.contains("Bold")) || *embolden
        }),
        "no run resolved a bold cut nor synthesized one: {report:?}"
    );
}

/// The variable-font law: skia must rasterize the INSTANCE fontique
/// shaped. We measure every run's glyphs with the bridged skia
/// typeface and demand the advances agree with parley's — on a
/// variable font (JetBrains Mono), handing skia the bare default
/// instance fails this by a growing margin along the line.
#[cfg(target_os = "macos")]
#[test]
fn variable_fonts_rasterize_the_shaped_instance() {
    use parley::PositionedLayoutItem;
    let text = "weights wander when instances differ 0123456789";
    // The bundled code font must answer its own name — regular AND
    // a real Bold cut, not an emboldened fake.
    for bold in [false, true] {
        let line = ParleyLine::shape_with_families(
            text,
            &["JetBrains Mono"],
            16.0,
            None,
            &[SpanStyle {
                bold,
                ..SpanStyle::plain(skia_safe::Color::BLACK)
            }],
        );
        let report = line.face_report_for_test();
        let want = if bold {
            "JetBrainsMono-Bold"
        } else {
            "JetBrainsMono-Regular"
        };
        assert!(
            report
                .iter()
                .all(|(name, _, _)| name.as_deref() == Some(want)),
            "bundled code font did not serve bold={bold}: {report:?}"
        );
    }
    // system-ui is NOT in this list on purpose: SF's advances
    // cannot agree across engines (CoreText applies the trak table
    // per point size, harfrust does not) — which is why carets and
    // clicks route through the SAME layout that painted, instead of
    // demanding parity.
    for (families, bold) in [
        (vec!["JetBrains Mono", "Menlo", "monospace"], false),
        (vec!["JetBrains Mono", "Menlo", "monospace"], true),
        (vec!["SF Mono", "Menlo", "monospace"], true),
    ] {
        let line = ParleyLine::shape_with_families(
            text,
            &families.iter().map(|s| *s).collect::<Vec<_>>(),
            16.0,
            None,
            &[SpanStyle {
                bold,
                ..SpanStyle::plain(skia_safe::Color::BLACK)
            }],
        );
        CTX.with(|cell| {
            let ctx = &mut *cell.borrow_mut();
            for parley_line in line.layout.lines() {
                for item in parley_line.items() {
                    let PositionedLayoutItem::GlyphRun(run) = item else {
                        continue;
                    };
                    let synthesis = run.run().synthesis();
                    let vars = synthesis.variation_settings();
                    let typeface = ctx
                        .typeface(run.run().font(), vars)
                        .expect("bridged typeface");
                    let font = skia_safe::Font::new(typeface.clone(), run.run().font_size());
                    let glyphs: Vec<parley::Glyph> = run.positioned_glyphs().collect();
                    let ids: Vec<u16> = glyphs.iter().map(|g| g.id as u16).collect();
                    let mut widths = vec![0.0f32; ids.len()];
                    font.get_widths(&ids, &mut widths);
                    let parley_advance: f32 = glyphs.iter().map(|g| g.advance).sum();
                    let skia_advance: f32 = widths.iter().sum();
                    eprintln!(
                        "[parley-vf] {:?} bold={bold} face {:?} vars {:?} parley {parley_advance:.2} skia {skia_advance:.2}",
                        families,
                        typeface.post_script_name(),
                        vars
                    );
                    assert!(
                        (parley_advance - skia_advance).abs() < 0.5,
                        "advance drift {:?} bold={bold}: parley {parley_advance} vs skia {skia_advance} (face {:?}, vars {vars:?})",
                        families,
                        typeface.post_script_name()
                    );
                }
            }
        });
    }
}

/// The goal-column law: the x a caret reports must map back to the
/// same byte — vertical motion round-trips through (caret_x,
/// byte_at_point).
#[test]
fn caret_x_round_trips_through_hit_testing() {
    let text = "a long first line";
    let line = ParleyLine::shape_with_families(
        text,
        &["JetBrains Mono"],
        14.0,
        None,
        &[SpanStyle::plain(skia_safe::Color::BLACK)],
    );
    for byte in 0..=text.len() {
        let x = line.caret_x(byte);
        let hit = line.byte_at_point(x, line.height() / 2.0);
        assert_eq!(
            hit, byte,
            "byte {byte} reported x {x} but that x hits byte {hit}"
        );
    }
}
