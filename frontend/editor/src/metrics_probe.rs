// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::time::{Duration, Instant};

use skia_safe::textlayout::{
    Paragraph, ParagraphBuilder, ParagraphStyle, RectHeightStyle, RectWidthStyle, TextStyle,
};

fn build(fonts: &skia_safe::textlayout::FontCollection, text: &str, width: f32) -> Paragraph {
    let mut text_style = TextStyle::new();
    text_style.set_font_families(&[crate::embedded_fonts::FAMILY]);
    text_style.set_font_size(16.0);
    let mut style = ParagraphStyle::new();
    style.set_text_style(&text_style);
    let mut builder = ParagraphBuilder::new(&style, fonts.clone());
    builder.add_text(text);
    let mut paragraph = builder.build();
    paragraph.layout(width);
    paragraph
}

const WRAPPED: &str = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod \
tempor incididunt ut labore et dolore magna aliqua ut enim ad minim veniam quis nostrud \
exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat duis aute irure";

const SHORT: &str = "let greeting = 1;";

fn bench(name: &str, iterations: usize, mut op: impl FnMut()) -> Duration {
    for _ in 0..iterations.min(16) {
        op();
    }
    let started = Instant::now();
    for _ in 0..iterations {
        op();
    }
    let elapsed = started.elapsed();
    println!(
        "  {name:<34} {:>9.3?} total  {:>9.3?}/op",
        elapsed,
        elapsed / iterations as u32
    );
    elapsed
}

#[test]
#[ignore = "probe"]
fn line_metrics_apis_single_threaded() {
    let fonts = crate::embedded_fonts::collection();
    for (label, text, width) in [("wrapped", WRAPPED, 400.0), ("short", SHORT, 400.0)] {
        let paragraph = build(&fonts, text, width);
        let rows = paragraph.line_number();
        println!("{label}: {rows} row(s), {} bytes", text.len());

        bench("build + layout", 200, || {
            let _ = build(&fonts, text, width);
        });
        bench("get_line_metrics_at × rows", 200, || {
            for row in 0..rows {
                let _ = paragraph.get_line_metrics_at(row);
            }
        });
        bench("get_line_metrics (all at once)", 200, || {
            let _ = paragraph.get_line_metrics();
        });
        bench("get_actual_text_range × rows", 200, || {
            for row in 0..rows {
                let _ = paragraph.get_actual_text_range(row, true);
            }
        });
        bench("get_rects_for_range (all rows)", 200, || {
            let _ = paragraph.get_rects_for_range(
                0..text.len(),
                RectHeightStyle::Max,
                RectWidthStyle::Tight,
            );
        });
        println!();
    }
}

#[test]
#[ignore = "probe"]
fn line_metrics_apis_under_contention() {
    for threads in [1usize, 8] {
        println!("threads: {threads}");
        for (name, op) in [
            ("build + layout", 0u8),
            ("+ get_line_metrics_at × rows", 1),
            ("+ get_actual_text_range × rows", 2),
            ("+ get_rects_for_range", 3),
            ("+ ONE metrics + ranges (new)", 4),
        ] {
            let started = Instant::now();
            let iterations = 400;
            std::thread::scope(|scope| {
                for _ in 0..threads {
                    scope.spawn(move || {
                        let fonts = crate::embedded_fonts::collection();
                        for _ in 0..iterations {
                            let paragraph = build(&fonts, WRAPPED, 400.0);
                            let rows = paragraph.line_number();
                            match op {
                                1 => {
                                    for row in 0..rows {
                                        let _ = paragraph.get_line_metrics_at(row);
                                    }
                                }
                                2 => {
                                    for row in 0..rows {
                                        let _ = paragraph.get_actual_text_range(row, true);
                                    }
                                }
                                3 => {
                                    let _ = paragraph.get_rects_for_range(
                                        0..WRAPPED.len(),
                                        RectHeightStyle::Max,
                                        RectWidthStyle::Tight,
                                    );
                                }

                                4 => {
                                    let _ = paragraph.get_line_metrics_at(0);
                                    for row in 0..rows {
                                        let _ = paragraph.get_actual_text_range(row, true);
                                    }
                                }
                                _ => {}
                            }
                        }
                    });
                }
            });
            let elapsed = started.elapsed();
            let total = iterations * threads;
            println!(
                "  {name:<34} {:>9.3?} total  {:>9.3?}/paragraph",
                elapsed,
                elapsed / total as u32
            );
        }
        println!();
    }
}
