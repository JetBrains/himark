// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! THE text shaper: the paragraph as a VALUE.
//!
//! `parley::Layout` is plain data — runs, lines, glyphs, styles in
//! vectors, `Clone`, `Send + Sync` by construction. Everything
//! SkParagraph answered through thread-affine C++ with unguarded
//! lazy caches, this answers as queries over an immutable value:
//! caret geometry without FFI, selection rects without a lock, a
//! layout that crosses threads like the store does.
//!
//! Skia keeps the half it is good at: glyph RASTERIZATION — the
//! layout's runs are baked to `SkTextBlob`s at shape time and ride
//! the atlas road.
//!
//! Fonts arrive through [`register_font`]: shells and the editor
//! hand in their embedded faces once; every shaping thread syncs
//! the registry lazily. On platforms with a system backend the
//! fontique collection enumerates installed fonts as well.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use parley::{Alignment, AlignmentOptions, FontContext, LayoutContext, PositionedLayoutItem};

/// RGBA — parley's default brush shape; converts to `skia::Color`.
type Brush = [u8; 4];

/// The PROOF this road exists for: the layout is a value that
/// crosses threads.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<parley::Layout<Brush>>();
    assert_send_sync::<ParleyLine>();
};

// ─── the font registry ───

static FONTS: Mutex<Vec<&'static [u8]>> = Mutex::new(Vec::new());
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Register embedded font bytes for every shaping thread. Additive
/// and idempotent (the same slice registers once); threads pick up
/// new registrations before their next shape.
pub fn register_font(bytes: &'static [u8]) {
    let mut fonts = FONTS.lock().unwrap();
    if fonts
        .iter()
        .any(|known| std::ptr::eq(known.as_ptr(), bytes.as_ptr()) && known.len() == bytes.len())
    {
        return;
    }
    fonts.push(bytes);
    GENERATION.fetch_add(1, Ordering::Release);
}

/// One style run over the text, in utf8 offsets of the shaped text.
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
    /// Line-height multiple override (font-size relative, the old
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

/// An inline BOX reserved in the flow — the inlay road: the shaper
/// leaves a hole of the given size, the widget paints into it.
#[derive(Clone)]
pub struct InlineBoxSpec {
    /// utf8 offset in the shaped text where the box sits.
    pub index: usize,
    pub width: f32,
    pub height: f32,
    pub align: BoxAlign,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoxAlign {
    Top,
    Middle,
    Bottom,
}

/// An ink decoration over a byte range: underline, strikethrough,
/// or the diagnostics SQUIGGLE (which wins the lane when present).
#[derive(Clone)]
pub struct DecorationSpec {
    pub range: Range<usize>,
    pub underline: bool,
    pub strikethrough: bool,
    pub squiggle: Option<skia_safe::Color>,
    pub color: skia_safe::Color,
}

/// Everything one shape call can carry.
pub struct ShapeSpec<'a> {
    pub text: &'a str,
    pub families: &'a [&'a str],
    pub size: f32,
    pub line_height: Option<f32>,
    pub letter_spacing: f32,
    /// `None` = never wrap (one visual line per hard break);
    /// `Some(width)` wraps like a paragraph.
    pub max_width: Option<f32>,
    pub spans: &'a [SpanStyle],
    pub boxes: &'a [InlineBoxSpec],
    pub decorations: &'a [DecorationSpec],
}

impl<'a> ShapeSpec<'a> {
    pub fn new(text: &'a str, families: &'a [&'a str], size: f32) -> Self {
        Self {
            text,
            families,
            size,
            line_height: None,
            letter_spacing: 0.0,
            max_width: None,
            spans: &[],
            boxes: &[],
            decorations: &[],
        }
    }
}

/// One visual line of the layout, in shaped-text utf8 offsets.
#[derive(Clone)]
pub struct ShapedRow {
    pub text_range: Range<usize>,
    pub baseline: f32,
    pub top: f32,
    pub bottom: f32,
    pub advance: f32,
}

/// A decoration baked to paintable geometry at shape time.
#[derive(Clone)]
enum InkKind {
    Rule,
    Squiggle,
}

#[derive(Clone)]
struct Ink {
    rect: skia_safe::Rect,
    color: skia_safe::Color,
    kind: InkKind,
}

/// One shaped line (or paragraph) as a Send value.
pub struct ParleyLine {
    layout: parley::Layout<Brush>,
    /// Built ONCE at shape time: stable blob identity is what keeps
    /// Graphite's atlas sub-run cache warm — rebuilding per paint
    /// minted new blob ids every frame and re-recorded the same
    /// quads.
    blobs: Vec<(skia_safe::TextBlob, skia_safe::Color)>,
    /// Positioned inline boxes, by spec order.
    boxes: Vec<(u64, skia_safe::Rect)>,
    /// Decoration geometry, precomputed — paint stays a value walk.
    ink: Vec<Ink>,
}

/// Per-thread shaping machinery: the font database, the layout
/// scratch space, and the fontique→skia typeface bridge. Mint once
/// per thread, exactly like `UiCtx`.
struct ParleyCtx {
    fonts: FontContext,
    layouts: LayoutContext<Brush>,
    /// How much of the global registry this thread has consumed.
    synced: u64,
    /// The first registered family — the deterministic fallback
    /// when a chain resolves to nothing.
    fallback_family: Option<String>,
    /// ONE platform manager for the context's lifetime. Skia has no
    /// system-manager singleton anymore: every `FontMgr::new()` is
    /// a fresh `SkFontMgr_New_CoreText` — CoreText enumerating and
    /// SORTING every family on the machine. Minting it per cache
    /// miss put that walk on the shaping path.
    manager: skia_safe::FontMgr,
    /// fvar axis defaults per blob — parsed from the bytes ONCE,
    /// not per run per shape.
    axes: HashMap<(u64, u32), Vec<(u32, f32)>>,
    /// Decoration metrics per blob (underline/strikeout position
    /// and thickness, in design units with the upem to scale by).
    decor: HashMap<(u64, u32), DecorMetrics>,
    /// The name-bridged BASE face per blob: the family walk
    /// (instantiating every platform cut to fingerprint them) runs
    /// once per font, not once per variation combo.
    bases: HashMap<(u64, u32), Option<skia_safe::Typeface>>,
    /// fontique blob id + ttc index + variation coords → skia
    /// typeface, so the blob road rasterizes the same INSTANCE
    /// parley shaped with (a variable font's default differs from
    /// its named instances — advances drift, carets follow).
    typefaces: HashMap<(u64, u32, Vec<(u32, i32)>), Option<skia_safe::Typeface>>,
}

#[derive(Clone, Copy)]
struct DecorMetrics {
    upem: f32,
    underline_position: f32,
    underline_thickness: f32,
    strikeout_position: f32,
    strikeout_thickness: f32,
}

thread_local! {
    static CTX: RefCell<ParleyCtx> = RefCell::new(ParleyCtx::new());
}

impl ParleyCtx {
    fn new() -> Self {
        Self {
            fonts: FontContext::new(),
            layouts: LayoutContext::new(),
            synced: 0,
            fallback_family: None,
            manager: skia_safe::FontMgr::new(),
            axes: HashMap::new(),
            decor: HashMap::new(),
            bases: HashMap::new(),
            typefaces: HashMap::new(),
        }
    }

    /// Catch up with [`register_font`] — additive, cheap when
    /// nothing changed.
    fn sync(&mut self) {
        let generation = GENERATION.load(Ordering::Acquire);
        if generation == self.synced {
            return;
        }
        let fonts = FONTS.lock().unwrap().clone();
        for bytes in fonts {
            let blob = parley::fontique::Blob::new(std::sync::Arc::new(bytes));
            let registered = self.fonts.collection.register_fonts(blob, None);
            if self.fallback_family.is_none() {
                if let Some((id, _)) = registered.first() {
                    self.fallback_family =
                        self.fonts.collection.family_name(*id).map(str::to_string);
                }
            }
        }
        self.seed_symbol_fallbacks();
        self.synced = generation;
    }

    /// Fontique's fallback is keyed by SCRIPT SAMPLE text, so the
    /// keycap and technical symbols (⌘ ⌃ ⌥ ⇧ ⌫ ⎋ — script Common,
    /// inheriting the surrounding run's script) never reach the
    /// faces that carry them. Appending the symbol faces to the
    /// common scripts' fallback lists lets per-cluster coverage
    /// matching find them.
    fn seed_symbol_fallbacks(&mut self) {
        #[cfg(target_vendor = "apple")]
        {
            let ids: Vec<parley::fontique::FamilyId> = ["Apple Symbols", "Menlo"]
                .iter()
                .filter_map(|name| {
                    self.fonts
                        .collection
                        .family_by_name(name)
                        .map(|info| info.id())
                })
                .collect();
            if ids.is_empty() {
                return;
            }
            for script in [*b"Latn", *b"Zyyy", *b"Cyrl", *b"Grek"] {
                self.fonts.collection.append_fallbacks(
                    parley::fontique::Script::from_bytes(script),
                    ids.iter().copied(),
                );
            }
        }
    }

    fn position(
        &mut self,
        font: &parley::FontData,
        variations: &[(skrifa::Tag, f32)],
    ) -> Vec<(u32, f32)> {
        let tag_of = |tag: &skrifa::Tag| u32::from_be_bytes(tag.to_be_bytes());
        let defaults = self
            .axes
            .entry((font.data.id(), font.index))
            .or_insert_with(|| {
                use skrifa::MetadataProvider;
                skrifa::FontRef::from_index(font.data.as_ref(), font.index)
                    .map(|face| {
                        face.axes()
                            .iter()
                            .map(|axis| (tag_of(&axis.tag()), axis.default_value()))
                            .collect()
                    })
                    .unwrap_or_default()
            });
        let mut coords = defaults.clone();
        for (tag, value) in variations {
            let tag = tag_of(tag);
            match coords.iter_mut().find(|(candidate, _)| *candidate == tag) {
                Some(slot) => slot.1 = *value,
                None => coords.push((tag, *value)),
            }
        }
        coords
    }

    /// Underline/strikeout geometry from the font's own tables,
    /// parsed once per blob.
    fn decor_metrics(&mut self, font: &parley::FontData) -> DecorMetrics {
        *self
            .decor
            .entry((font.data.id(), font.index))
            .or_insert_with(|| {
                use skrifa::raw::TableProvider;
                let fallback = DecorMetrics {
                    upem: 1000.0,
                    underline_position: -100.0,
                    underline_thickness: 50.0,
                    strikeout_position: 300.0,
                    strikeout_thickness: 50.0,
                };
                let Ok(face) = skrifa::FontRef::from_index(font.data.as_ref(), font.index) else {
                    return fallback;
                };
                let upem = face
                    .head()
                    .map(|head| head.units_per_em() as f32)
                    .unwrap_or(1000.0);
                let (underline_position, underline_thickness) = face
                    .post()
                    .map(|post| {
                        (
                            post.underline_position().to_i16() as f32,
                            post.underline_thickness().to_i16() as f32,
                        )
                    })
                    .unwrap_or((-100.0, 50.0));
                let (strikeout_position, strikeout_thickness) = face
                    .os2()
                    .map(|os2| {
                        (
                            os2.y_strikeout_position() as f32,
                            os2.y_strikeout_size() as f32,
                        )
                    })
                    .unwrap_or((300.0, 50.0));
                DecorMetrics {
                    upem,
                    underline_position,
                    underline_thickness,
                    strikeout_position,
                    strikeout_thickness,
                }
            })
    }

    /// The name-bridged base face for a blob — the expensive walk,
    /// cached per font: read family + postscript name from the
    /// bytes, match through the platform manager, fingerprint the
    /// cut by postscript name with glyph count as the id-space
    /// guard; parse the bytes for faces the platform does not know.
    fn base(&mut self, font: &parley::FontData) -> Option<skia_safe::Typeface> {
        let manager = self.manager.clone();
        self.bases
            .entry((font.data.id(), font.index))
            .or_insert_with(|| {
                let fingerprint = glyph_count_of(font);
                let postscript = postscript_name_of(font);
                family_name_of(font)
                    .and_then(|family| {
                        let count = fingerprint?;
                        let mut cuts = manager.match_family(&family);
                        let cuts: Vec<skia_safe::Typeface> = (0..cuts.count())
                            .filter_map(|index| cuts.new_typeface(index))
                            .collect();
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
            })
            .clone()
    }

    fn typeface(
        &mut self,
        font: &parley::FontData,
        variations: &[(skrifa::Tag, f32)],
    ) -> Option<skia_safe::Typeface> {
        let position = self.position(font, variations);
        let coords: Vec<(u32, i32)> = position
            .iter()
            .map(|(tag, value)| (*tag, value.to_bits() as i32))
            .collect();
        let key = (font.data.id(), font.index, coords);
        if let Some(hit) = self.typefaces.get(&key) {
            return hit.clone();
        }
        let instanced = self.base(font).and_then(|typeface| {
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
            let arguments = skia_safe::FontArguments::new().set_variation_design_position(
                skia_safe::font_arguments::VariationPosition {
                    coordinates: &coordinates,
                },
            );
            typeface.clone_with_arguments(&arguments)
        });
        self.typefaces.insert(key, instanced.clone());
        instanced
    }
}

impl ParleyLine {
    /// Shape one styled text: uniform family + size, colored spans.
    pub fn shape(text: &str, family: &str, size: f32, spans: &[SpanStyle]) -> ParleyLine {
        Self::shape_with_families(text, &[family], size, None, spans)
    }

    /// Shape against a FAMILY CHAIN with CSS semantics: the first
    /// name the collection resolves wins; generics ("monospace")
    /// resolve through fontique's generic map; an unresolvable
    /// chain falls to the first registered face — never to a silent
    /// proportional default.
    pub fn shape_with_families(
        text: &str,
        families: &[&str],
        size: f32,
        line_height: Option<f32>,
        spans: &[SpanStyle],
    ) -> ParleyLine {
        let mut spec = ShapeSpec::new(text, families, size);
        spec.line_height = line_height;
        spec.spans = spans;
        Self::shape_spec(&spec)
    }

    /// The full road: spans, inline boxes, decorations, wrapping.
    pub fn shape_spec(spec: &ShapeSpec<'_>) -> ParleyLine {
        CTX.with(|cell| {
            let ctx = &mut *cell.borrow_mut();
            ctx.sync();
            let family = resolve_family(&mut ctx.fonts.collection, spec.families)
                .or_else(|| ctx.fallback_family.clone());
            let span_families: Vec<Option<String>> = spec
                .spans
                .iter()
                .map(|span| {
                    let names: Vec<&str> =
                        span.families.as_ref()?.iter().map(String::as_str).collect();
                    resolve_family(&mut ctx.fonts.collection, &names)
                })
                .collect();
            let mut builder = ctx
                .layouts
                .ranged_builder(&mut ctx.fonts, spec.text, 1.0, true);
            if let Some(family) = &family {
                builder.push_default(parley::StyleProperty::FontFamily(
                    parley::FontFamily::named(family),
                ));
            }
            builder.push_default(parley::StyleProperty::FontSize(spec.size));
            if spec.letter_spacing != 0.0 {
                builder.push_default(parley::StyleProperty::LetterSpacing(spec.letter_spacing));
            }
            if let Some(height) = spec.line_height {
                builder.push_default(parley::StyleProperty::LineHeight(
                    parley::LineHeight::FontSizeRelative(height),
                ));
            }
            for (index, span) in spec.spans.iter().enumerate() {
                let end = spec
                    .spans
                    .get(index + 1)
                    .map(|next| next.start)
                    .unwrap_or(spec.text.len());
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
            for (number, inline_box) in spec.boxes.iter().enumerate() {
                builder.push_inline_box(parley::InlineBox {
                    id: number as u64,
                    kind: parley::InlineBoxKind::InFlow,
                    index: inline_box.index,
                    width: inline_box.width,
                    height: inline_box.height,
                    baseline: None,
                    vertical_align: match inline_box.align {
                        BoxAlign::Top => parley::VerticalAlign {
                            alignment: parley::AlignmentBaseline::TextTop,
                            ..parley::VerticalAlign::BASELINE
                        },
                        BoxAlign::Middle => parley::VerticalAlign {
                            alignment: parley::AlignmentBaseline::Middle,
                            ..parley::VerticalAlign::BASELINE
                        },
                        BoxAlign::Bottom => parley::VerticalAlign::BASELINE,
                    },
                });
            }
            let mut layout = builder.build(spec.text);
            layout.break_all_lines(spec.max_width);
            layout.align(Alignment::Start, AlignmentOptions::default());
            let blobs = build_blobs(ctx, &layout);
            let boxes = collect_boxes(&layout);
            let ink = build_ink(ctx, &layout, spec.decorations);
            ParleyLine {
                layout,
                blobs,
                boxes,
                ink,
            }
        })
    }

    /// Shape a single-line label, ellipsizing with "…" when wider
    /// than `clip` — the chrome label road.
    pub fn ellipsized(
        text: &str,
        families: &[&str],
        size: f32,
        letter_spacing: f32,
        color: skia_safe::Color,
        bold: bool,
        clip: Option<f32>,
    ) -> ParleyLine {
        let span = SpanStyle {
            bold,
            ..SpanStyle::plain(color)
        };
        let shape = |text: &str| {
            let mut spec = ShapeSpec::new(text, families, size);
            spec.letter_spacing = letter_spacing;
            let spans = [span.clone()];
            spec.spans = &spans;
            ParleyLine::shape_spec(&spec)
        };
        let whole = shape(text);
        let Some(clip) = clip else { return whole };
        if whole.width() <= clip {
            return whole;
        }
        let ellipsis = shape("…");
        let budget = (clip - ellipsis.width()).max(0.0);
        let mut cut = whole.byte_at_point(budget, 0.0).min(text.len());
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        loop {
            let trimmed = shape(&format!("{}…", &text[..cut]));
            if trimmed.width() <= clip || cut == 0 {
                return trimmed;
            }
            cut -= 1;
            while cut > 0 && !text.is_char_boundary(cut) {
                cut -= 1;
            }
        }
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

    /// The visual rows, in shaped-text utf8 offsets.
    pub fn rows(&self) -> Vec<ShapedRow> {
        self.layout
            .lines()
            .map(|line| {
                let metrics = line.metrics();
                ShapedRow {
                    text_range: line.text_range(),
                    baseline: metrics.baseline,
                    top: metrics.block_min_coord,
                    bottom: metrics.block_max_coord,
                    advance: metrics.advance,
                }
            })
            .collect()
    }

    /// Positioned inline boxes, as (spec index, rect).
    pub fn boxes(&self) -> impl Iterator<Item = (usize, skia_safe::Rect)> + '_ {
        self.boxes.iter().map(|(id, rect)| (*id as usize, *rect))
    }

    /// The caret's x for a utf8 byte offset.
    pub fn caret_x(&self, byte: usize) -> f32 {
        let cursor =
            parley::Cursor::from_byte_index(&self.layout, byte, parley::Affinity::Downstream);
        cursor.geometry(&self.layout, 1.0).x0 as f32
    }

    /// The caret's full geometry: (x, top, height) — strong caret
    /// rect of the layout's own metrics.
    pub fn caret_geometry(&self, byte: usize) -> (f32, f32, f32) {
        let cursor =
            parley::Cursor::from_byte_index(&self.layout, byte, parley::Affinity::Downstream);
        let rect = cursor.geometry(&self.layout, 1.0);
        (
            rect.x0 as f32,
            rect.y0 as f32,
            (rect.y1 - rect.y0) as f32,
        )
    }

    /// The caret boundary nearest a point — editor-grade click
    /// semantics, answered by the SAME layout the glyphs were
    /// painted from. When the layout is a single row (the editor's
    /// pre-split lines), pass `clamp_first_row = true`: a trailing
    /// newline mints an empty second line, and a probe half a pixel
    /// past the row's bottom edge would answer TEXT END.
    pub fn byte_at_point(&self, x: f32, y: f32) -> usize {
        self.hit(x, y, true)
    }

    /// Hit against the whole (possibly wrapped) layout.
    pub fn byte_at_point_multiline(&self, x: f32, y: f32) -> usize {
        self.hit(x, y, false)
    }

    fn hit(&self, x: f32, y: f32, clamp_first_row: bool) -> usize {
        let y = match (clamp_first_row, self.layout.get(0).map(|line| *line.metrics())) {
            (true, Some(metrics)) => y.clamp(
                metrics.block_min_coord,
                (metrics.block_max_coord - 0.01).max(metrics.block_min_coord),
            ),
            _ => y,
        };
        parley::Cursor::from_point(&self.layout, x, y).index()
    }

    /// Selection rects for a byte range.
    pub fn rects(&self, range: Range<usize>) -> Vec<skia_safe::Rect> {
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

    /// Paint glyphs and decorations through the atlas road.
    pub fn paint(&self, canvas: &skia_safe::Canvas, origin: (f32, f32)) {
        let mut paint = skia_safe::Paint::default();
        paint.set_anti_alias(true);
        for (blob, color) in &self.blobs {
            paint.set_color(*color);
            canvas.draw_text_blob(blob, origin, &paint);
        }
        for ink in &self.ink {
            paint.set_color(ink.color);
            let rect = ink.rect.with_offset(origin);
            match ink.kind {
                InkKind::Rule => {
                    canvas.draw_rect(rect, &paint);
                }
                InkKind::Squiggle => {
                    let mut path = skia_safe::PathBuilder::new();
                    let amplitude = rect.height();
                    let step = amplitude * 2.0;
                    let mut x = rect.left;
                    let mid = rect.center_y();
                    path.move_to((x, mid + amplitude / 2.0));
                    let mut up = true;
                    while x < rect.right {
                        let next = (x + step).min(rect.right);
                        let peak = if up {
                            mid - amplitude / 2.0
                        } else {
                            mid + amplitude / 2.0
                        };
                        path.quad_to(((x + next) / 2.0, peak), (next, mid));
                        up = !up;
                        x = next;
                    }
                    let path = path.snapshot();
                    let mut stroke = skia_safe::Paint::default();
                    stroke.set_anti_alias(true);
                    stroke.set_style(skia_safe::PaintStyle::Stroke);
                    stroke.set_stroke_width((amplitude / 2.0).max(1.0));
                    stroke.set_color(ink.color);
                    canvas.draw_path(&path, &stroke);
                }
            }
        }
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

    /// TEST SUPPORT: advances of the bridged skia faces per run —
    /// the variable-font parity meter.
    #[doc(hidden)]
    pub fn bridge_advances_for_test(&self) -> Vec<(Option<String>, Vec<(u32, f32)>, f32, f32)> {
        CTX.with(|cell| {
            let ctx = &mut *cell.borrow_mut();
            let mut out = Vec::new();
            for line in self.layout.lines() {
                for item in line.items() {
                    let PositionedLayoutItem::GlyphRun(run) = item else {
                        continue;
                    };
                    let synthesis = run.run().synthesis();
                    let vars: Vec<(u32, f32)> = synthesis
                        .variation_settings()
                        .iter()
                        .map(|(tag, value)| (u32::from_be_bytes(tag.to_be_bytes()), *value))
                        .collect();
                    let Some(typeface) = ctx.typeface(run.run().font(), synthesis.variation_settings())
                    else {
                        continue;
                    };
                    let name = typeface.post_script_name();
                    let font = skia_safe::Font::new(typeface, run.run().font_size());
                    let glyphs: Vec<parley::Glyph> = run.positioned_glyphs().collect();
                    let ids: Vec<u16> = glyphs.iter().map(|glyph| glyph.id as u16).collect();
                    let mut widths = vec![0.0f32; ids.len()];
                    font.get_widths(&ids, &mut widths);
                    let parley_advance: f32 = glyphs.iter().map(|glyph| glyph.advance).sum();
                    let skia_advance: f32 = widths.iter().sum();
                    out.push((name, vars, parley_advance, skia_advance));
                }
            }
            out
        })
    }

    /// TEST SUPPORT: sizes of the bridge caches.
    #[doc(hidden)]
    pub fn bridge_cache_sizes_for_test() -> (usize, usize, usize) {
        CTX.with(|cell| {
            let ctx = cell.borrow();
            (ctx.bases.len(), ctx.typefaces.len(), ctx.axes.len())
        })
    }
}

fn collect_boxes(layout: &parley::Layout<Brush>) -> Vec<(u64, skia_safe::Rect)> {
    let mut out = Vec::new();
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::InlineBox(positioned) = item else {
                continue;
            };
            out.push((
                positioned.id,
                skia_safe::Rect::from_xywh(
                    positioned.x,
                    positioned.y,
                    positioned.width,
                    positioned.height,
                ),
            ));
        }
    }
    out.sort_by_key(|(id, _)| *id);
    out
}

/// Decorations → paintable geometry: horizontal extents from the
/// selection geometry of the range, vertical from the FIRST run's
/// own font tables (underline/strikeout position and thickness).
fn build_ink(
    ctx: &mut ParleyCtx,
    layout: &parley::Layout<Brush>,
    decorations: &[DecorationSpec],
) -> Vec<Ink> {
    if decorations.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for spec in decorations {
        if spec.range.start >= spec.range.end {
            continue;
        }
        // The anchor run: the first glyph run intersecting the range
        // names the font whose tables place the lines.
        let mut anchor: Option<(DecorMetrics, f32, f32)> = None;
        'lines: for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                let range = run.run().text_range();
                if range.start < spec.range.end && spec.range.start < range.end {
                    anchor = Some((
                        ctx.decor_metrics(run.run().font()),
                        run.run().font_size(),
                        line.metrics().baseline,
                    ));
                    break 'lines;
                }
            }
        }
        let Some((metrics, size, _)) = anchor else {
            continue;
        };
        let scale = size / metrics.upem.max(1.0);
        let selection = parley::Selection::new(
            parley::Cursor::from_byte_index(layout, spec.range.start, parley::Affinity::Downstream),
            parley::Cursor::from_byte_index(layout, spec.range.end, parley::Affinity::Upstream),
        );
        for (bounds, line_index) in selection.geometry(layout) {
            let Some(line) = layout.get(line_index) else {
                continue;
            };
            let baseline = line.metrics().baseline;
            let left = bounds.x0 as f32;
            let right = bounds.x1 as f32;
            if right - left < 0.01 {
                continue;
            }
            if let Some(color) = spec.squiggle {
                let thickness = (metrics.underline_thickness * scale).max(1.0) * 2.0;
                let y = baseline - metrics.underline_position * scale;
                out.push(Ink {
                    rect: skia_safe::Rect::from_ltrb(left, y - thickness / 2.0, right, y + thickness / 2.0),
                    color,
                    kind: InkKind::Squiggle,
                });
            } else if spec.underline {
                let thickness = (metrics.underline_thickness * scale).max(1.0);
                let y = baseline - metrics.underline_position * scale;
                out.push(Ink {
                    rect: skia_safe::Rect::from_ltrb(left, y, right, y + thickness),
                    color: spec.color,
                    kind: InkKind::Rule,
                });
            }
            if spec.strikethrough {
                let thickness = (metrics.strikeout_thickness * scale).max(1.0);
                let y = baseline - metrics.strikeout_position * scale;
                out.push(Ink {
                    rect: skia_safe::Rect::from_ltrb(left, y, right, y + thickness),
                    color: spec.color,
                    kind: InkKind::Rule,
                });
            }
        }
    }
    out
}

/// Runs → SkTextBlobs, once per shape. The run's SYNTHESIS travels
/// whole: faux bold/skew as font flags, and VARIATION coordinates
/// into the typeface itself, so skia rasterizes the instance
/// fontique shaped.
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
        let strings = collection.localized_strings(id);
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
    let strings = face.localized_strings(skrifa::string::StringId::POSTSCRIPT_NAME);
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
        // faces — skia special-cased the names, fontique needs the
        // translation.
        "SF Pro Text" | "SF Pro Display" | "SF Pro" | "-apple-system" => GenericFamily::SystemUi,
        "SF Mono" => GenericFamily::UiMonospace,
        _ => return None,
    })
}

fn brush_of(color: skia_safe::Color) -> Brush {
    [color.r(), color.g(), color.b(), color.a()]
}
