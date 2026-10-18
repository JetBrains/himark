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

/// One style run over the text — the parley twin of the blob
/// road's `StyleSpan`, in utf8 offsets.
#[derive(Clone)]
pub struct SpanStyle {
    pub start: usize,
    pub color: skia_safe::Color,
    pub bold: bool,
    pub italic: bool,
    /// `None` inherits the line's base size.
    pub size: Option<f32>,
    /// A family-chain OVERRIDE for this span (inline code switching
    /// to mono mid-line); `None` inherits.
    pub families: Option<Vec<String>>,
    /// Line-height multiple override (font-size relative, the
    /// skparagraph `height` semantics); `None` inherits.
    pub line_height: Option<f32>,
}

impl SpanStyle {
    pub fn at(mut self, start: usize) -> Self {
        self.start = start;
        self
    }

    pub fn plain(color: skia_safe::Color) -> Self {
        Self {
            start: 0,
            color,
            bold: false,
            italic: false,
            size: None,
            families: None,
            line_height: None,
        }
    }
}

/// One shaped line (or paragraph) as a Send value.
pub struct ParleyLine {
    layout: parley::Layout<Brush>,
    /// Built ONCE at shape time: stable blob identity is what
    /// keeps Graphite's atlas sub-run cache warm — rebuilding
    /// per paint minted new blob ids every frame and re-recorded
    /// the same quads (felt as "JetBrains Mono is slow"; its calt
    /// shaping just made the per-paint rebuild visible).
    blobs: Vec<(skia_safe::TextBlob, skia_safe::Color)>,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ParleyLine>();
};

/// Per-thread shaping machinery: the font database, the layout
/// scratch space, and the fontique→skia typeface bridge. Mint once
/// per thread, exactly like `UiCtx`.
struct ParleyCtx {
    fonts: FontContext,
    layouts: LayoutContext<Brush>,
    /// fontique blob id + ttc index + variation coords → skia
    /// typeface, so the blob road rasterizes the same INSTANCE
    /// parley shaped with (a variable font's default differs from
    /// its named instances — advances drift, carets follow).
    typefaces: HashMap<(u64, u32, Vec<(u32, i32)>), Option<skia_safe::Typeface>>,
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
        #[cfg(not(target_os = "emscripten"))]
        for cut in crate::embedded_fonts::code_font_cuts() {
            let blob = parley::fontique::Blob::new(std::sync::Arc::new(*cut));
            fonts.collection.register_fonts(blob, None);
        }
        Self {
            fonts,
            layouts: LayoutContext::new(),
            typefaces: HashMap::new(),
        }
    }

    fn typeface(
        &mut self,
        font: &parley::FontData,
        variations: &[(skrifa::Tag, f32)],
    ) -> Option<skia_safe::Typeface> {
        let position = design_position(font, variations);
        let coords: Vec<(u32, i32)> = position
            .iter()
            .map(|(tag, value)| (*tag, value.to_bits() as i32))
            .collect();
        let key = (font.data.id(), font.index, coords);
        self.typefaces
            .entry(key)
            .or_insert_with(|| {
                // THE NAME BRIDGE first: read the face's family
                // name out of the bytes and ask the PLATFORM
                // manager for its native handle — no 55MB
                // collection re-parse, and CoreText hands back the
                // same face it vended to fontique. Byte parsing
                // stays as the road for registered/custom fonts
                // the platform does not know.
                // THE NAME BRIDGE: shaping used the blob's glyph
                // ids, so the rasterizing face must be the SAME
                // CUT — a family can hold cuts with different
                // glyph spaces (seen: Heiti SC Light vs Regular).
                // Walk the platform family's style set and take
                // the cut whose glyph count fingerprints equal;
                // the byte road stays for faces the platform
                // does not know.
                let manager = skia_safe::FontMgr::new();
                let fingerprint = glyph_count_of(font);
                let postscript = postscript_name_of(font);
                family_name_of(font)
                    .and_then(|family| {
                        let count = fingerprint?;
                        let mut cuts = manager.match_family(&family);
                        let cuts: Vec<skia_safe::Typeface> = (0..cuts.count())
                            .filter_map(|index| cuts.new_typeface(index))
                            .collect();
                        // The POSTSCRIPT NAME names the exact cut;
                        // glyph count alone cannot tell Regular
                        // from Bold (same charset — bold runs were
                        // rasterizing regular). Count stays as the
                        // id-space guard behind the name.
                        cuts.iter()
                            .find(|cut| {
                                postscript.is_some()
                                    && cut.post_script_name() == postscript
                                    && cut.count_glyphs() as u32 == count
                            })
                            .or_else(|| cuts.iter().find(|cut| cut.count_glyphs() as u32 == count))
                            .cloned()
                    })
                    .or_else(|| {
                        manager.new_from_data(
                            skia_safe::Data::new_copy(font.data.as_ref()),
                            Some(font.index),
                        )
                    })
                    .and_then(|typeface| {
                        if position.is_empty() {
                            return Some(typeface);
                        }
                        let coordinates: Vec<skia_safe::font_arguments::variation_position::Coordinate> =
                            position
                                .iter()
                                .map(|(tag, value)| {
                                    skia_safe::font_arguments::variation_position::Coordinate {
                                        axis: skia_safe::FourByteTag::from(*tag),
                                        value: *value,
                                    }
                                })
                                .collect();
                        let arguments = skia_safe::FontArguments::new()
                            .set_variation_design_position(
                                skia_safe::font_arguments::VariationPosition {
                                    coordinates: &coordinates,
                                },
                            );
                        typeface.clone_with_arguments(&arguments)
                    })
            })
            .clone()
    }
}

impl ParleyLine {
    /// Shape one styled text: uniform family + size, colored spans
    /// (utf8 offset → color runs, matching the blob road's span
    /// model).
    pub fn shape(text: &str, family: &str, size: f32, spans: &[SpanStyle]) -> ParleyLine {
        Self::shape_with_families(text, &[family], size, None, spans)
    }

    /// Shape against a FAMILY CHAIN with skia's semantics: the
    /// first name the collection actually resolves wins; CSS
    /// generics ("monospace") resolve through fontique's generic
    /// map; an unresolvable chain falls to the embedded face —
    /// never to a silent proportional default. (The A/B's first
    /// catch: "SF Mono" is unenumerable on some systems; taking
    /// only the first name put code in a proportional face and
    /// dragged the caret and the band rects with it.)
    pub fn shape_with_families(
        text: &str,
        families: &[&str],
        size: f32,
        line_height: Option<f32>,
        spans: &[SpanStyle],
    ) -> ParleyLine {
        CTX.with(|cell| {
            let ctx = &mut *cell.borrow_mut();
            let family = resolve_family(&mut ctx.fonts.collection, families)
                .unwrap_or_else(|| crate::embedded_fonts::FAMILY.to_string());
            let span_families: Vec<Option<String>> = spans
                .iter()
                .map(|span| {
                    let names: Vec<&str> =
                        span.families.as_ref()?.iter().map(String::as_str).collect();
                    resolve_family(&mut ctx.fonts.collection, &names)
                })
                .collect();
            let mut builder = ctx.layouts.ranged_builder(&mut ctx.fonts, text, 1.0, true);
            builder.push_default(parley::StyleProperty::FontFamily(
                parley::FontFamily::named(&family),
            ));
            builder.push_default(parley::StyleProperty::FontSize(size));
            if let Some(height) = line_height {
                builder.push_default(parley::StyleProperty::LineHeight(
                    parley::LineHeight::FontSizeRelative(height),
                ));
            }
            for (index, span) in spans.iter().enumerate() {
                let end = spans
                    .get(index + 1)
                    .map(|next| next.start)
                    .unwrap_or(text.len());
                if span.start >= end {
                    continue;
                }
                let range = span.start..end;
                builder.push(
                    parley::StyleProperty::Brush(brush_of(span.color)),
                    range.clone(),
                );
                if let Some(size) = span.size {
                    builder.push(parley::StyleProperty::FontSize(size), range.clone());
                }
                if let Some(family) = &span_families[index] {
                    builder.push(
                        parley::StyleProperty::FontFamily(parley::FontFamily::named(family)),
                        range.clone(),
                    );
                }
                if let Some(height) = span.line_height {
                    builder.push(
                        parley::StyleProperty::LineHeight(parley::LineHeight::FontSizeRelative(
                            height,
                        )),
                        range.clone(),
                    );
                }
                if span.bold {
                    builder.push(
                        parley::StyleProperty::FontWeight(parley::FontWeight::BOLD),
                        range.clone(),
                    );
                }
                if span.italic {
                    builder.push(
                        parley::StyleProperty::FontStyle(parley::FontStyle::Italic),
                        range,
                    );
                }
            }
            let mut layout = builder.build(text);
            layout.break_all_lines(None);
            layout.align(Alignment::Start, AlignmentOptions::default());
            let blobs = build_blobs(ctx, &layout);
            ParleyLine { layout, blobs }
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

    /// The caret boundary nearest a point — the editor's click
    /// mapping (`getGlyphPositionAtCoordinate`'s twin), answered by
    /// the SAME layout the glyphs were painted from.
    pub fn byte_at_point(&self, x: f32, y: f32) -> usize {
        parley::Cursor::from_point(&self.layout, x, y).index()
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

    /// The runs as blobs for the atlas road — built once at shape
    /// time, so every paint hands Graphite the SAME blob objects
    /// and the atlas sub-run cache keys hold across frames.
    pub fn blobs(&self) -> &[(skia_safe::TextBlob, skia_safe::Color)] {
        &self.blobs
    }

    /// TEST SUPPORT for the bold diagnosis: per run, the serving
    /// face's postscript name, the embolden flag, and the fontique
    /// weight attribute.
    #[doc(hidden)]
    pub fn face_report_for_test(&self) -> Vec<(Option<String>, bool, String)> {
        CTX.with(|cell| {
            let ctx = &mut *cell.borrow_mut();
            let mut out = Vec::new();
            for line in self.layout.lines() {
                for item in line.items() {
                    let PositionedLayoutItem::GlyphRun(run) = item else {
                        continue;
                    };
                    let skia = ctx
                        .typeface(run.run().font(), run.run().synthesis().variation_settings())
                        .and_then(|typeface| typeface.post_script_name())
                        .unwrap_or_else(|| "NONE".into());
                    out.push((
                        postscript_name_of(run.run().font()),
                        run.run().synthesis().embolden(),
                        format!("skia={skia}"),
                    ));
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
                    let family = match ctx
                        .typeface(run.run().font(), run.run().synthesis().variation_settings())
                    {
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
        for (blob, color) in &self.blobs {
            paint.set_color(*color);
            canvas.draw_text_blob(blob, origin, &paint);
        }
    }
}

/// The FULL design position parley shaped at: every fvar axis at
/// its default, overlaid with fontique's synthesis coordinates.
/// Pinning every axis frees the name bridge from caring which cut
/// it matched — CoreText enumerates .SFNS as INSTANCED cuts (opsz
/// and GRAD baked in), and matching one of those then setting only
/// wght kept the stray axes: system-ui bold measured 52px long
/// over a 48-char line. Empty result = not a variable font, serve
/// the bare typeface.
fn design_position(font: &parley::FontData, variations: &[(skrifa::Tag, f32)]) -> Vec<(u32, f32)> {
    use skrifa::MetadataProvider;
    let tag_of = |tag: &skrifa::Tag| u32::from_be_bytes(tag.to_be_bytes());
    let mut coords: Vec<(u32, f32)> = skrifa::FontRef::from_index(font.data.as_ref(), font.index)
        .map(|face| {
            face.axes()
                .iter()
                .map(|axis| (tag_of(&axis.tag()), axis.default_value()))
                .collect()
        })
        .unwrap_or_default();
    for (tag, value) in variations {
        let tag = tag_of(tag);
        match coords.iter_mut().find(|(candidate, _)| *candidate == tag) {
            Some(slot) => slot.1 = *value,
            None => coords.push((tag, *value)),
        }
    }
    coords
}

/// Runs → SkTextBlobs, once per shape. The run's SYNTHESIS travels
/// whole: faux bold/skew as font flags, and — the JetBrains Mono
/// lesson — VARIATION coordinates into the typeface itself, so skia
/// rasterizes the instance fontique shaped. (A variable font's
/// default instance has different advances than its named cuts;
/// shaping one and rasterizing the other walked the caret off the
/// glyphs as the line grew.)
fn build_blobs(
    ctx: &mut ParleyCtx,
    layout: &parley::Layout<Brush>,
) -> Vec<(skia_safe::TextBlob, skia_safe::Color)> {
    let mut out = Vec::new();
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else {
                continue;
            };
            let glyphs: Vec<parley::Glyph> = run.positioned_glyphs().collect();
            if glyphs.is_empty() {
                continue;
            }
            let synthesis = run.run().synthesis();
            let Some(typeface) = ctx.typeface(run.run().font(), synthesis.variation_settings())
            else {
                continue;
            };
            let mut font = skia_safe::Font::new(typeface, run.run().font_size());
            // Faux cuts, matched to the skparagraph road: when the
            // family has no real Bold/Italic, fontique says so and
            // skia synthesizes.
            if synthesis.embolden() {
                font.set_embolden(true);
            }
            if let Some(degrees) = synthesis.skew() {
                font.set_skew_x(-degrees.to_radians().tan());
            }
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
}

/// The face's preferred family name, read from the name table —
/// typographic family first (the unstyled name CoreText matches),
/// plain family as the fallback.
fn family_name_of(font: &parley::FontData) -> Option<String> {
    use skrifa::MetadataProvider;
    let collection = skrifa::FontRef::from_index(font.data.as_ref(), font.index).ok()?;
    for id in [
        skrifa::string::StringId::TYPOGRAPHIC_FAMILY_NAME,
        skrifa::string::StringId::FAMILY_NAME,
    ] {
        let mut strings = collection.localized_strings(id);
        if let Some(name) = strings.english_or_first() {
            let text: String = name.chars().collect();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn postscript_name_of(font: &parley::FontData) -> Option<String> {
    use skrifa::MetadataProvider;
    let face = skrifa::FontRef::from_index(font.data.as_ref(), font.index).ok()?;
    let mut strings = face.localized_strings(skrifa::string::StringId::POSTSCRIPT_NAME);
    let name: String = strings.english_or_first()?.chars().collect();
    (!name.is_empty()).then_some(name)
}

fn glyph_count_of(font: &parley::FontData) -> Option<u32> {
    use skrifa::raw::TableProvider;
    let face = skrifa::FontRef::from_index(font.data.as_ref(), font.index).ok()?;
    Some(face.maxp().ok()?.num_glyphs() as u32)
}

fn resolve_family(collection: &mut parley::fontique::Collection, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        if let Some(generic) = generic_of(name) {
            let id = collection.generic_families(generic).next()?;
            return collection.family_name(id).map(str::to_string);
        }
        collection
            .family_by_name(name)
            .map(|info| info.name().to_string())
    })
}

fn generic_of(name: &str) -> Option<parley::fontique::GenericFamily> {
    use parley::fontique::GenericFamily;
    Some(match name {
        "monospace" => GenericFamily::Monospace,
        "sans-serif" => GenericFamily::SansSerif,
        "serif" => GenericFamily::Serif,
        "system-ui" => GenericFamily::SystemUi,
        // Apple's HIDDEN families: CoreText never enumerates them,
        // but the CSS system generics resolve to exactly these
        // faces — skia special-cases the names, fontique needs the
        // translation. (The A/B's catch: prose silently fell to
        // Helvetica Neue, whose Bold reads lighter than SF Pro's.)
        "SF Pro Text" | "SF Pro Display" | "SF Pro" | "-apple-system" => GenericFamily::SystemUi,
        "SF Mono" => GenericFamily::UiMonospace,
        _ => return None,
    })
}

fn brush_of(color: skia_safe::Color) -> Brush {
    [color.r(), color.g(), color.b(), color.a()]
}

#[cfg(test)]
mod tests;
