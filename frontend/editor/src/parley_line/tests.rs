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

    let line = ParleyLine::shape(text, crate::embedded_fonts::FAMILY, size, &[(0, color)]);
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
