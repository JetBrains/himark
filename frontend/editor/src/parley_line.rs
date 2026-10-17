// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The PARLEY road: the paragraph as a VALUE.
//!
//! `parley::Layout` is plain data — runs, lines, glyphs, styles in
//! vectors, `Clone`, `Send + Sync` by construction. Everything
//! SkParagraph answers through thread-affine C++ with unguarded
//! lazy caches, this answers as queries over an immutable value:
//! caret geometry without FFI, selection rects without a lock, a
//! layout that crosses threads like the store does.
//!
//! Behind `parley-text` until it proves parity with the
//! SkParagraph road. Skia keeps the half it is good at: glyph
//! RASTERIZATION — the layout's runs feed the same `SkTextBlob`
//! atlas road the blob work built.

use std::cell::RefCell;
use std::collections::HashMap;

use parley::{Alignment, AlignmentOptions, FontContext, LayoutContext, PositionedLayoutItem};

/// RGBA — parley's default brush shape; converts to `skia::Color`.
type Brush = [u8; 4];

/// The PROOF this campaign is for: the layout is a value that
/// crosses threads.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<parley::Layout<Brush>>();
};

/// One shaped line (or paragraph) as a Send value.
pub struct ParleyLine {
    layout: parley::Layout<Brush>,
}

/// Per-thread shaping machinery: the font database, the layout
/// scratch space, and the fontique→skia typeface bridge. Mint once
/// per thread, exactly like `UiCtx`.
struct ParleyCtx {
    fonts: FontContext,
    layouts: LayoutContext<Brush>,
    /// fontique blob id + ttc index → skia typeface, so the blob
    /// road rasterizes with the same bytes parley shaped with.
    typefaces: HashMap<(u64, u32), Option<skia_safe::Typeface>>,
}

thread_local! {
    static CTX: RefCell<ParleyCtx> = RefCell::new(ParleyCtx::new());
}

impl ParleyCtx {
    fn new() -> Self {
        let mut fonts = FontContext::new();
        // The embedded face keeps the spike DETERMINISTIC: both
        // roads shape the same bytes. System enumeration (and the
        // fallback story) joins once the fontconfig links conflict
        // is settled per-target.
        let blob = parley::fontique::Blob::new(std::sync::Arc::new(crate::embedded_fonts::bytes()));
        fonts.collection.register_fonts(blob, None);
        Self {
            fonts,
            layouts: LayoutContext::new(),
            typefaces: HashMap::new(),
        }
    }

    fn typeface(&mut self, font: &parley::FontData) -> Option<skia_safe::Typeface> {
        let key = (font.data.id(), font.index);
        self.typefaces
            .entry(key)
            .or_insert_with(|| {
                // A system face skia's manager refuses to re-parse
                // (seen: CoreText-vended CJK collections) is a WATCH
                // item, not a crash — the run reports unrasterizable
                // and the fallback corpus shows it.
                skia_safe::FontMgr::new().new_from_data(
                    skia_safe::Data::new_copy(font.data.as_ref()),
                    Some(font.index),
                )
            })
            .clone()
    }
}

impl ParleyLine {
    /// Shape one styled text: uniform family + size, colored spans
    /// (utf8 offset → color runs, matching the blob road's span
    /// model).
    pub fn shape(
        text: &str,
        family: &str,
        size: f32,
        spans: &[(usize, skia_safe::Color)],
    ) -> ParleyLine {
        CTX.with(|cell| {
            let ctx = &mut *cell.borrow_mut();
            let mut builder = ctx.layouts.ranged_builder(&mut ctx.fonts, text, 1.0, true);
            builder.push_default(parley::StyleProperty::FontFamily(
                parley::FontFamily::named(family),
            ));
            builder.push_default(parley::StyleProperty::FontSize(size));
            for (index, (start, color)) in spans.iter().enumerate() {
                let end = spans
                    .get(index + 1)
                    .map(|(next, _)| *next)
                    .unwrap_or(text.len());
                if *start < end {
                    builder.push(parley::StyleProperty::Brush(brush_of(*color)), *start..end);
                }
            }
            let mut layout = builder.build(text);
            layout.break_all_lines(None);
            layout.align(Alignment::Start, AlignmentOptions::default());
            ParleyLine { layout }
        })
    }

    pub fn width(&self) -> f32 {
        self.layout.width()
    }

    pub fn height(&self) -> f32 {
        self.layout.height()
    }

    pub fn first_baseline(&self) -> Option<f32> {
        self.layout.first_baseline()
    }

    /// The caret's x for a utf8 byte offset — `Paragraph::
    /// getGlyphPositionAtCoordinate`'s inverse, no FFI, no lazy
    /// cache, callable from any thread.
    pub fn caret_x(&self, byte: usize) -> f32 {
        let cursor =
            parley::Cursor::from_byte_index(&self.layout, byte, parley::Affinity::Downstream);
        cursor.geometry(&self.layout, 1.0).x0 as f32
    }

    /// Selection rects for a byte range — `getRectsForRange`
    /// without the thread affinity.
    pub fn rects(&self, range: std::ops::Range<usize>) -> Vec<skia_safe::Rect> {
        let selection = parley::Selection::new(
            parley::Cursor::from_byte_index(
                &self.layout,
                range.start,
                parley::Affinity::Downstream,
            ),
            parley::Cursor::from_byte_index(&self.layout, range.end, parley::Affinity::Upstream),
        );
        selection
            .geometry(&self.layout)
            .into_iter()
            .map(|(bounds, _)| {
                skia_safe::Rect::new(
                    bounds.x0 as f32,
                    bounds.y0 as f32,
                    bounds.x1 as f32,
                    bounds.y1 as f32,
                )
            })
            .collect()
    }

    /// The runs as blobs for the atlas road — the same shape the
    /// blob work feeds `draw_text_blob` today.
    pub fn blobs(&self) -> Vec<(skia_safe::TextBlob, skia_safe::Color)> {
        CTX.with(|cell| {
            let ctx = &mut *cell.borrow_mut();
            let mut out = Vec::new();
            for line in self.layout.lines() {
                for item in line.items() {
                    let PositionedLayoutItem::GlyphRun(run) = item else {
                        continue;
                    };
                    let glyphs: Vec<parley::Glyph> = run.positioned_glyphs().collect();
                    if glyphs.is_empty() {
                        continue;
                    }
                    let Some(typeface) = ctx.typeface(run.run().font()) else {
                        continue;
                    };
                    let font = skia_safe::Font::new(typeface, run.run().font_size());
                    let mut builder = skia_safe::TextBlobBuilder::new();
                    {
                        let (ids, points) = builder.alloc_run_pos(&font, glyphs.len(), None);
                        for (slot, glyph) in ids.iter_mut().zip(&glyphs) {
                            *slot = glyph.id as u16;
                        }
                        for (slot, glyph) in points.iter_mut().zip(&glyphs) {
                            *slot = skia_safe::Point::new(glyph.x, glyph.y);
                        }
                    }
                    let Some(blob) = builder.make() else { continue };
                    let [r, g, b, a] = run.style().brush;
                    out.push((blob, skia_safe::Color::from_argb(a, r, g, b)));
                }
            }
            out
        })
    }

    /// TEST SUPPORT for the fallback watch: the families that
    /// served this line, plus notdef counts.
    #[doc(hidden)]
    pub fn coverage_for_test(&self) -> (Vec<String>, usize, usize) {
        CTX.with(|cell| {
            let ctx = &mut *cell.borrow_mut();
            let mut families = Vec::new();
            let mut notdefs = 0usize;
            let mut total = 0usize;
            for line in self.layout.lines() {
                for item in line.items() {
                    let PositionedLayoutItem::GlyphRun(run) = item else {
                        continue;
                    };
                    let family = match ctx.typeface(run.run().font()) {
                        Some(typeface) => typeface.family_name().to_string(),
                        None => format!(
                            "UNRASTERIZABLE({} bytes, index {})",
                            run.run().font().data.as_ref().len(),
                            run.run().font().index
                        ),
                    };
                    if !families.contains(&family) {
                        families.push(family);
                    }
                    for glyph in run.positioned_glyphs() {
                        total += 1;
                        if glyph.id == 0 {
                            notdefs += 1;
                        }
                    }
                }
            }
            (families, notdefs, total)
        })
    }

    /// Paint through the atlas road — the PARLEY twin of the blob
    /// branch in `ShapedLine::paint_in_slot`.
    pub fn paint(&self, canvas: &skia_safe::Canvas, origin: (f32, f32)) {
        let mut paint = skia_safe::Paint::default();
        paint.set_anti_alias(true);
        for (blob, color) in self.blobs() {
            paint.set_color(color);
            canvas.draw_text_blob(&blob, origin, &paint);
        }
    }
}

fn brush_of(color: skia_safe::Color) -> Brush {
    [color.r(), color.g(), color.b(), color.a()]
}

#[cfg(test)]
mod tests;
