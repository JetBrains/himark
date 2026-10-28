// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::theme::ThemeStyles as _;
use std::{ops::Range, str};

use skia_safe::{Canvas, Rect};

use imba::shape::{BoxAlign, DecorationSpec, InlineBoxSpec, ParleyLine, ShapeSpec, SpanStyle};
use text::text_view::TextView;

use crate::markup::{
    BlockStyle, InlayMode, InlayPlaceholder, InsteadKind, OverlaidMarkup, TextDecorationInterval,
};

pub(crate) struct DisplayText {
    text: String,
    raw_end_by_byte: Vec<u32>,
    raw_end_by_utf16: Option<Vec<u32>>,
    raw_len: usize,
    placeholders: Vec<ParagraphPlaceholder>,
}

#[derive(Clone)]
pub(crate) struct ParagraphPlaceholder {
    pub(crate) byte: u32,
    pub(crate) range: Range<usize>,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) align: crate::markup::InlayAlignment,
}

impl DisplayText {
    fn new(visible_capacity: usize, raw_len: usize, map_utf16: bool) -> Self {
        let mut raw_end_by_byte = Vec::with_capacity(visible_capacity + 1);
        raw_end_by_byte.push(0);
        let raw_end_by_utf16 = if map_utf16 {
            let mut raw_end_by_utf16 = Vec::with_capacity(visible_capacity + 1);
            raw_end_by_utf16.push(0);
            Some(raw_end_by_utf16)
        } else {
            None
        };

        Self {
            text: String::with_capacity(visible_capacity),
            raw_end_by_byte,
            raw_end_by_utf16,
            raw_len,
            placeholders: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn build(rule: bool, raw: &str, hidden: &[Range<u32>], map_utf16: bool) -> Self {
        let mut out = Self::new(raw.len(), raw.len(), map_utf16);
        if !rule {
            out.push_verbatim(raw, hidden);
        }
        out
    }

    pub(crate) fn build_segments(
        rule: bool,
        segments: &[(String, u32)],
        hidden: &[Range<u32>],
        map_utf16: bool,
        raw_len: usize,
    ) -> Self {
        let capacity = segments.iter().map(|(text, _)| text.len()).sum();
        let mut out = Self::new(capacity, raw_len, map_utf16);
        if rule {
            return out;
        }
        for (text, base) in segments {
            let len = text.len().min(u32::MAX as usize) as u32;
            let local: Vec<Range<u32>> = hidden
                .iter()
                .filter_map(|range| {
                    let start = range.start.max(*base);
                    let end = range.end.min(base.saturating_add(len));
                    (start < end).then(|| start - base..end - base)
                })
                .collect();
            for segment in visible_segments(text, 0..text.len(), &local) {
                out.push_raw_offset(text, segment, *base as usize);
            }
        }
        out
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn placeholders(&self) -> &[ParagraphPlaceholder] {
        &self.placeholders
    }

    #[cfg(test)]
    fn push_raw(&mut self, raw: &str, range: Range<usize>) {
        self.push_raw_offset(raw, range, 0);
    }

    fn push_raw_offset(&mut self, raw: &str, range: Range<usize>, base: usize) {
        for (byte_offset, ch) in raw[range.clone()].char_indices() {
            self.push_char(ch, base + range.start + byte_offset + ch.len_utf8());
        }
    }

    fn push_char(&mut self, ch: char, raw_end: usize) {
        self.text.push(ch);
        let raw_end = raw_end.min(u32::MAX as usize) as u32;
        for _ in 0..ch.len_utf8() {
            self.raw_end_by_byte.push(raw_end);
        }
        if let Some(raw_end_by_utf16) = &mut self.raw_end_by_utf16 {
            for _ in 0..ch.len_utf16() {
                raw_end_by_utf16.push(raw_end);
            }
        }
    }

    #[cfg(test)]
    fn push_verbatim(&mut self, raw: &str, hidden: &[Range<u32>]) {
        for segment in visible_segments(raw, 0..raw.len(), hidden) {
            self.push_raw(raw, segment);
        }
    }

    pub(crate) fn map_decorations(
        &self,
        decorations: &[TextDecorationInterval],
    ) -> Vec<TextDecorationInterval> {
        decorations
            .iter()
            .filter_map(|decoration| {
                let start = self.display_byte_at_raw(decoration.range.start);
                let end = self.display_byte_at_raw(decoration.range.end);
                (start < end).then(|| TextDecorationInterval {
                    range: start as u32..end as u32,
                    id: decoration.id,
                    weight: decoration.weight,
                })
            })
            .collect()
    }

    pub(crate) fn add_inline_placeholders(
        &mut self,
        markup: OverlaidMarkup<'_, '_>,
        range: Range<u32>,
        measure: crate::markup::InlayMeasure<'_>,
    ) {
        for placeholder in markup.inline_placeholders_in(range.clone(), measure) {
            self.insert_placeholder(range.start, placeholder);
        }
    }

    fn insert_placeholder(&mut self, absolute_start: u32, placeholder: InlayPlaceholder) {
        let raw_byte = placeholder.byte.saturating_sub(absolute_start);
        let display_byte = self.display_byte_at_raw(raw_byte);
        if display_byte > self.text.len() || !self.text.is_char_boundary(display_byte) {
            return;
        }

        let marker = '\u{FFFC}';
        let marker_len = marker.len_utf8();
        let display_utf16 = self.display_utf16_at_display_byte(display_byte);
        self.text.insert(display_byte, marker);

        let raw_end = raw_byte.min(u32::MAX);
        for _ in 0..marker_len {
            self.raw_end_by_byte.insert(display_byte + 1, raw_end);
        }
        if let Some(raw_end_by_utf16) = &mut self.raw_end_by_utf16 {
            raw_end_by_utf16.insert(display_utf16 + 1, raw_end);
        }

        for existing in &mut self.placeholders {
            if existing.range.start >= display_byte {
                existing.range.start += marker_len;
                existing.range.end += marker_len;
            }
        }

        self.placeholders.push(ParagraphPlaceholder {
            byte: placeholder.byte,
            range: display_byte..display_byte + marker_len,
            width: placeholder.width,
            height: placeholder.height,
            align: placeholder.align,
        });
        self.placeholders
            .sort_by_key(|placeholder| placeholder.range.start);
    }

    fn display_byte_at_raw(&self, raw_byte: u32) -> usize {
        self.raw_end_by_byte
            .partition_point(|end| *end <= raw_byte)
            .saturating_sub(1)
            .min(self.text.len())
    }

    fn display_utf16_at_display_byte(&self, display_byte: usize) -> usize {
        self.text[..display_byte].encode_utf16().count()
    }

    pub(crate) fn display_utf16_at_raw(&self, raw_byte: u32) -> usize {
        let raw_end_by_utf16 = self
            .raw_end_by_utf16
            .as_ref()
            .expect("UTF-16 raw map must be enabled");
        raw_end_by_utf16
            .partition_point(|end| *end <= raw_byte)
            .saturating_sub(1)
    }

    pub(crate) fn raw_byte_at_display_byte(&self, display_byte: usize) -> usize {
        self.raw_end_by_byte
            .get(display_byte.min(self.raw_end_by_byte.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0) as usize
    }

    pub(crate) fn raw_byte_at_utf16(&self, utf16: usize) -> usize {
        let raw_end_by_utf16 = self
            .raw_end_by_utf16
            .as_ref()
            .expect("UTF-16 raw map must be enabled");
        raw_end_by_utf16
            .get(utf16.min(raw_end_by_utf16.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0) as usize
    }

    /// A visual row's end in display bytes. The shaper's row ranges
    /// partition the text (the explicit break rides its row), so
    /// unlike skia's "actual text range" no hard-break re-adding is
    /// needed — only the char-boundary walk.
    pub(crate) fn line_text_end_at(&self, row_end: usize, is_last: bool) -> usize {
        if is_last {
            return self.text.len();
        }
        let mut text_end = row_end.min(self.text.len());
        while text_end < self.text.len() && !self.text.is_char_boundary(text_end) {
            text_end += 1;
        }
        text_end
    }

    pub(crate) fn line_byte_end(&self, text_end: usize, byte_start: usize, is_last: bool) -> usize {
        if is_last {
            return self.raw_len;
        }

        self.raw_end_by_byte
            .get(text_end)
            .copied()
            .map(|byte| byte as usize)
            .unwrap_or(self.raw_len)
            .clamp(byte_start, self.raw_len)
    }
}

struct Band {
    /// DISPLAY-BYTE range into the shape text.
    range: Range<usize>,
    color: skia_safe::Color,
    extent: crate::theme::BackgroundExtent,

    height: crate::theme::BackgroundHeight,

    through_end: bool,
}

/// A placeholder EXCISION: the display-byte range the inline box
/// replaced, and where it sits in the shape text.
#[derive(Clone, Copy)]
pub(crate) struct Excision {
    display_start: usize,
    display_end: usize,
    shape_start: usize,
}

pub(crate) fn shape_byte_of(excisions: &[Excision], display_byte: usize) -> usize {
    let mut removed = 0usize;
    for excision in excisions {
        if display_byte >= excision.display_end {
            removed += excision.display_end - excision.display_start;
        } else if display_byte > excision.display_start {
            removed += display_byte - excision.display_start;
        } else {
            break;
        }
    }
    display_byte - removed
}

pub(crate) fn display_byte_of(excisions: &[Excision], shape_byte: usize) -> usize {
    let mut added = 0usize;
    for excision in excisions {
        if shape_byte >= excision.shape_start {
            added += excision.display_end - excision.display_start;
        } else {
            break;
        }
    }
    shape_byte + added
}

/// Everything a shape call needs, computed from the display text,
/// the resolved block style and the inline decorations — shared by
/// the single-row road here and the wrapping road in
/// `document_layout`.
pub(crate) struct ShapeInputs {
    pub(crate) text: String,
    pub(crate) families: Vec<String>,
    pub(crate) size: f32,
    pub(crate) line_height: Option<f32>,
    pub(crate) spans: Vec<SpanStyle>,
    pub(crate) ink: Vec<DecorationSpec>,
    pub(crate) boxes: Vec<InlineBoxSpec>,
    pub(crate) excisions: Vec<Excision>,
    pub(crate) base_direction: imba::shape::BaseDirection,
}

impl ShapeInputs {
    pub(crate) fn shape(&self, ui: &imba::ui::UiCtx, max_width: Option<f32>) -> ParleyLine {
        let families: Vec<&str> = self.families.iter().map(String::as_str).collect();
        let mut spec = ShapeSpec::new(&self.text, &families, self.size);
        spec.line_height = self.line_height;
        spec.max_width = max_width;
        spec.spans = &self.spans;
        spec.boxes = &self.boxes;
        spec.decorations = &self.ink;
        spec.base_direction = self.base_direction;
        crate::parley_line::shaper(ui).shape_spec(&spec)
    }
}

/// One run's resolved attributes over the base — the style algebra
/// that used to live in skia `TextStyle` pushes.
#[derive(Clone)]
struct RunStyle {
    color: skia_safe::Color,
    bold: bool,
    weight: Option<f32>,
    width: Option<f32>,
    italic: bool,
    size: f32,
    families: Option<Vec<String>>,
    line_height: Option<f32>,
    underline: bool,
    strikethrough: bool,
    squiggle: Option<skia_safe::Color>,
}

/// The attributes a run INHERITS from any layer beneath it — the
/// cascade a paragraph base hands its runs. The booleans (bold,
/// italic, the inks) are span-local and only an inline span sets
/// them.
fn apply_inherited(run: &mut RunStyle, attributes: &crate::theme::TextAttributes) {
    if let Some(color) = attributes.color {
        run.color = color;
    }
    if let Some(size) = attributes.font_size {
        run.size = size;
    }
    if let Some(weight) = attributes.font_weight {
        run.weight = Some(weight);
    }
    if let Some(width) = attributes.font_width {
        run.width = Some(width);
    }
    if let Some(families) = &attributes.font_families {
        run.families = Some(families.to_vec());
    }
    if let Some(height) = attributes.line_height {
        run.line_height = Some(height);
    }
}

/// One styled layer covering a run, carried with its sort key: the
/// weight first, block layers before inline at a tie, document order
/// last — the same order `BlockStyle::resolved` merges, so a run and
/// its line never disagree about who wins.
enum RunLayer<'a> {
    Block(crate::theme::StyleId),
    Inline(&'a TextDecorationInterval),
}

pub(crate) fn shape_inputs(
    marks: &BlockStyle,
    resolved: &crate::theme::TextAttributes,
    display: &DisplayText,
    decorations: &[TextDecorationInterval],
    theme: &crate::theme::Theme,
) -> ShapeInputs {
    let text = display.as_str();
    let base_size = resolved.font_size.unwrap_or(14.0);
    let base_color = resolved.color.unwrap_or(skia_safe::Color::BLACK);
    let base_families: Vec<String> = resolved
        .font_families
        .as_ref()
        .map(|families| families.to_vec())
        .unwrap_or_default();
    let base_height = resolved.line_height;
    let base_weight = resolved.font_weight;
    let base_width = resolved.font_width;

    // ─── spans + ink over the DISPLAY text ───
    let mut spans: Vec<SpanStyle> = Vec::new();
    let mut ink: Vec<DecorationSpec> = Vec::new();
    if decorations.is_empty() {
        spans.push(SpanStyle {
            weight: base_weight,
            width: base_width,
            ..SpanStyle::plain(base_color)
        });
    } else {
        let mut cuts: Vec<usize> = Vec::with_capacity(decorations.len() * 2 + 2);
        cuts.push(0);
        for decoration in decorations {
            for edge in [
                decoration.range.start as usize,
                decoration.range.end as usize,
            ] {
                if edge < text.len() && !text.is_char_boundary(edge) {
                    continue;
                }
                cuts.push(edge.min(text.len()));
            }
        }
        cuts.push(text.len());
        cuts.sort_unstable();
        cuts.dedup();

        // Every layer the line carries, lightest weight first. The
        // block layers already live in `resolved`; re-woven here at
        // their own weights, they stand between the inline layers
        // they outweigh and the ones that outweigh them — an
        // underlay never repaints over the block above it.
        let mut layers: Vec<(crate::markup::StyleWeight, u8, usize, RunLayer<'_>)> = Vec::new();
        for (seq, (id, weight)) in marks.layers().into_iter().enumerate() {
            layers.push((weight, 0, seq, RunLayer::Block(id)));
        }
        for (seq, decoration) in decorations.iter().enumerate() {
            layers.push((decoration.weight, 1, seq, RunLayer::Inline(decoration)));
        }
        layers.sort_by_key(|(weight, class, seq, _)| (*weight, *class, *seq));

        for window in cuts.windows(2) {
            let (start, end) = (window[0], window[1]);
            if start >= end {
                continue;
            }
            let mut run = RunStyle {
                color: base_color,
                bold: false,
                weight: base_weight,
                width: base_width,
                italic: false,
                size: base_size,
                families: None,
                line_height: None,
                underline: false,
                strikethrough: false,
                squiggle: None,
            };
            let styles = theme.styles();
            for (_, _, _, layer) in &layers {
                let decoration = match layer {
                    RunLayer::Block(id) => {
                        apply_inherited(&mut run, styles.attributes(*id));
                        continue;
                    }
                    RunLayer::Inline(decoration) => decoration,
                };
                let range = decoration.range.start as usize..decoration.range.end as usize;
                if range.start <= start && range.end >= end {
                    let attributes = styles.attributes(decoration.id);
                    apply_inherited(&mut run, attributes);
                    if attributes.bold {
                        run.bold = true;
                    }
                    if attributes.italic {
                        run.italic = true;
                    }
                    if attributes.underline {
                        run.underline = true;
                    }
                    if attributes.strikethrough {
                        run.strikethrough = true;
                    }
                    if let Some(color) = attributes.squiggle {
                        run.squiggle = Some(color);
                    }
                }
            }
            spans.push(SpanStyle {
                start,
                color: run.color,
                bold: run.bold,
                italic: run.italic,
                size: ((run.size - base_size).abs() > f32::EPSILON).then_some(run.size),
                families: run
                    .families
                    .clone()
                    .filter(|families| *families != base_families),
                line_height: run
                    .line_height
                    .filter(|height| base_height.map_or(true, |base| (height - base).abs() > 1e-3)),
                weight: run.weight,
                width: run.width,
            });
            if run.underline || run.strikethrough || run.squiggle.is_some() {
                ink.push(DecorationSpec {
                    range: start..end,
                    underline: run.underline || run.squiggle.is_some(),
                    strikethrough: run.strikethrough,
                    squiggle: run.squiggle,
                    color: run.color,
                });
            }
        }
    }

    // ─── placeholders become excisions + inline boxes ───
    let mut excisions: Vec<Excision> = Vec::new();
    let mut boxes: Vec<InlineBoxSpec> = Vec::new();
    let mut shape_text = String::new();
    let mut at = 0usize;
    for placeholder in display.placeholders() {
        let start = placeholder.range.start.min(text.len());
        let end = placeholder.range.end.min(text.len());
        if start < at || end < start || !text.is_char_boundary(start) || !text.is_char_boundary(end)
        {
            continue;
        }
        shape_text.push_str(&text[at..start]);
        let shape_start = shape_text.len();
        excisions.push(Excision {
            display_start: start,
            display_end: end,
            shape_start,
        });
        boxes.push(InlineBoxSpec {
            index: shape_start,
            width: placeholder.width,
            height: placeholder.height,
            align: match placeholder.align {
                crate::markup::InlayAlignment::Top => BoxAlign::Top,
                crate::markup::InlayAlignment::Middle => BoxAlign::Middle,
                crate::markup::InlayAlignment::Bottom => BoxAlign::Bottom,
            },
        });
        at = end;
    }
    shape_text.push_str(&text[at..]);

    // remap spans and ink into shape bytes
    for span in &mut spans {
        span.start = shape_byte_of(&excisions, span.start);
    }
    spans.dedup_by(|b, a| a.start == b.start);
    for decoration in &mut ink {
        decoration.range = shape_byte_of(&excisions, decoration.range.start)
            ..shape_byte_of(&excisions, decoration.range.end);
    }
    ink.retain(|decoration| decoration.range.start < decoration.range.end);

    ShapeInputs {
        base_direction: imba::shape::BaseDirection::Auto,
        text: shape_text,
        families: base_families,
        size: base_size,
        line_height: base_height,
        spans,
        ink,
        boxes,
        excisions,
    }
}

pub(crate) struct ShapedLine {
    byte_start: u32,
    byte_size: u32,
    x: f32,
    display: DisplayText,
    excisions: Vec<Excision>,
    line: ParleyLine,
    placeholder_rects: Vec<InlinePlaceholderRect>,

    backgrounds: Vec<Band>,

    extent_left: f32,
    extent_right: f32,

    first_baseline: Option<f32>,

    /// The visual rows (shape-byte ranges + verticals), snapshotted
    /// at shape time for lines that carry bands. Empty when there
    /// are no bands.
    band_line_ranges: Vec<imba::shape::ShapedRow>,
}

impl ShapedLine {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        view: &mut TextView,
        markup: OverlaidMarkup<'_, '_>,
        line_range: Range<u32>,
        marks: &BlockStyle,
        inline: &[TextDecorationInterval],
        hidden: &[Range<u32>],
        theme: &crate::theme::Theme,
        measure: crate::markup::InlayMeasure<'_>,
        document_x: f32,
        map_utf16: bool,
    ) -> Self {
        let layout_width = measure.width;
        debug_assert!(
            line_range.end as usize <= view.byte_count(),
            "shaped line {line_range:?} beyond text ({} bytes)",
            view.byte_count()
        );

        debug_assert!(
            view.is_char_boundary(line_range.start as usize)
                && view.is_char_boundary(line_range.end as usize),
            "shaped line {line_range:?} not on char boundaries: a committed              layout element boundary is stale against the text (landing or              edit byte accounting)"
        );
        let byte_count = view.byte_count() as u32;
        let line_range = line_range.start.min(byte_count)..line_range.end.min(byte_count);
        let byte_size = line_range.end.saturating_sub(line_range.start);

        let segments = visible_unit_segments(view, markup, &line_range);

        let band_styled: Vec<&TextDecorationInterval> = inline
            .iter()
            .filter(|decoration| {
                theme
                    .styles()
                    .attributes(decoration.id)
                    .background
                    .is_some_and(|background| background.kind == crate::theme::BackgroundKind::Text)
            })
            .collect();
        let map_utf16 = map_utf16 || !band_styled.is_empty();

        let mut resolved = marks.resolved(theme);

        let alignment = resolved.alignment.take().unwrap_or_default();
        let mut display = DisplayText::build_segments(
            resolved.rule.is_some(),
            &segments,
            hidden,
            map_utf16,
            byte_size as usize,
        );
        display.add_inline_placeholders(markup, line_range.clone(), measure);

        let inline = display.map_decorations(inline);

        let mut inputs = shape_inputs(marks, &resolved, &display, &inline, theme);
        if inputs.text.is_empty() && inputs.boxes.is_empty() {
            // An empty line still answers heights and carets — the
            // zero-width space gives the shaper one real cluster.
            inputs.text = "\u{200b}".to_string();
        }
        let line = inputs.shape(measure.ui, None);

        let placeholder_rects = display
            .placeholders()
            .iter()
            .zip(line.boxes())
            .map(|(placeholder, (_, rect))| InlinePlaceholderRect {
                byte: placeholder.byte,
                rect,
            })
            .collect();

        let backgrounds: Vec<Band> = band_styled
            .iter()
            .filter_map(|decoration| {
                let background = theme.styles().attributes(decoration.id).background?;
                let start = shape_byte_of(
                    &inputs.excisions,
                    display.display_byte_at_raw(decoration.range.start),
                );
                let end = shape_byte_of(
                    &inputs.excisions,
                    display.display_byte_at_raw(decoration.range.end),
                );

                let through_end = decoration.range.end >= byte_size;
                let stands = start < end
                    || (background.extent != crate::theme::BackgroundExtent::Glyphs && through_end);
                stands.then_some(Band {
                    range: start..end,
                    color: background.color,
                    extent: background.extent,
                    height: background.height,
                    through_end,
                })
            })
            .collect();

        let (x, extent_left, extent_right) =
            horizontal_geometry(document_x, &resolved, alignment, line.width(), layout_width);

        let first_baseline = line.first_baseline();

        let band_line_ranges = if backgrounds.is_empty() {
            Vec::new()
        } else {
            line.rows()
        };

        Self {
            byte_start: line_range.start,
            byte_size,
            x,
            display,
            excisions: inputs.excisions,
            line,
            placeholder_rects,
            backgrounds,
            extent_left,
            extent_right,
            first_baseline,
            band_line_ranges,
        }
    }

    /// The placeholder wears the input's own style (its covering
    /// `StyleId`), in the placeholder ink.
    pub(crate) fn placeholder(
        text: &str,
        style: Option<crate::theme::StyleId>,
        color: skia_safe::Color,
        ui: &imba::ui::UiCtx,
        theme: &crate::theme::Theme,
        layout_width: f32,
        document_x: f32,
    ) -> Self {
        let block = match style {
            Some(id) => BlockStyle::with(id),
            None => BlockStyle::default(),
        };
        let mut resolved = block.resolved(theme);
        resolved.color = Some(color);
        let alignment = resolved.alignment.take().unwrap_or_default();
        let segments = [(text.to_owned(), 0)];
        let display = DisplayText::build_segments(false, &segments, &[], true, text.len());
        let inputs = shape_inputs(&block, &resolved, &display, &[], theme);
        let line = inputs.shape(ui, None);
        let (x, extent_left, extent_right) =
            horizontal_geometry(document_x, &resolved, alignment, line.width(), layout_width);

        let first_baseline = line.first_baseline();

        Self {
            byte_start: 0,
            byte_size: text.len().min(u32::MAX as usize) as u32,
            x,
            display,
            excisions: inputs.excisions,
            line,
            placeholder_rects: Vec::new(),
            backgrounds: Vec::new(),
            extent_left,
            extent_right,
            first_baseline,
            band_line_ranges: Vec::new(),
        }
    }

    pub(crate) fn paint(&self, canvas: &Canvas, top: f32) {
        self.paint_in_slot(canvas, top, top, top + self.line.content_height());
    }

    pub(crate) fn paint_in_slot(&self, canvas: &Canvas, slot_top: f32, top: f32, slot_bottom: f32) {
        if !self.backgrounds.is_empty() {
            let mut paint = skia_safe::Paint::default();
            let display_end = self.line_text_len();

            let gap_below = (slot_bottom - top - self.line.content_height()).max(0.0);
            // The line's own rects can stand TALLER than the layout
            // slot (shrunk line heights, mixed metrics); a
            // translucent band bleeding into the next row doubles up
            // with its neighbor's — the horizontal-stripes artifact.
            // Bands never leave the slot. (They do not stretch up to
            // `slot_top` either: the space above the text belongs to
            // the line's ABOVE inlays, never to its wash.)
            let _ = slot_top;
            let clamped = |rect: Rect| {
                let rect = rect.with_offset((0.0, top));
                Rect::new(
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom.min(slot_bottom),
                )
            };
            // Whole-line bands take their VERTICAL edges from the
            // layout slot, not from line metrics: the line boxes sit
            // a sub-pixel inside the slot, and at fractional scroll
            // offsets those slivers surface as row-seam stripes that
            // come and go with the scroll position. Snapping makes
            // adjacent rows share exact edges, which rasterize
            // seamlessly at any offset.
            let seamless = |rect: Rect| {
                let local_top = rect.top;
                let mut out = clamped(rect);
                if local_top < 1.0 {
                    out.top = top;
                }
                if (out.bottom - slot_bottom).abs() < 1.5 {
                    out.bottom = slot_bottom;
                }
                out
            };
            for band in &self.backgrounds {
                paint.set_color(band.color);
                match band.extent {
                    crate::theme::BackgroundExtent::Glyphs => {
                        for rect in self.band_rects(band.range.clone(), band.height) {
                            let rect = clamped(rect);
                            if rect.bottom > rect.top {
                                canvas.draw_rect(rect, &paint);
                            }
                        }
                    }
                    crate::theme::BackgroundExtent::ToLineEnd
                    | crate::theme::BackgroundExtent::WholeLine => {
                        let gap = if band.through_end { gap_below } else { 0.0 };
                        self.extended_bands(band, display_end, gap, |rect| {
                            let rect = seamless(rect);
                            if rect.bottom > rect.top {
                                canvas.draw_rect(rect, &paint);
                            }
                        });
                    }
                }
            }
        }
        self.line.paint(canvas, (self.x, top));
    }

    fn line_text_len(&self) -> usize {
        self.display.as_str().len()
            - self
                .excisions
                .iter()
                .map(|excision| excision.display_end - excision.display_start)
                .sum::<usize>()
    }

    pub(crate) fn layout_height(&self, theme: &crate::theme::Theme) -> f32 {
        let text_height = self
            .line
            .rows()
            .first()
            .map(|row| (row.bottom - row.top).ceil().max(1.0))
            .unwrap_or(0.0);
        text_height + block_gap(theme.styles().base())
    }

    fn shape_byte_at_raw(&self, local: u32) -> usize {
        shape_byte_of(&self.excisions, self.display.display_byte_at_raw(local))
    }

    pub(crate) fn caret_geometry(&self, byte: u32) -> (f32, f32, f32) {
        let local = byte.saturating_sub(self.byte_start).min(self.byte_size);
        let shape_byte = self.shape_byte_at_raw(local);
        let (x, top, height) = self.line.caret_geometry(shape_byte);
        (self.x + x, top, height)
    }

    /// TEST SUPPORT for the parley parity meter.
    #[doc(hidden)]
    pub fn extent_for_test(&self) -> (f32, f32) {
        (self.extent_left, self.extent_right)
    }

    pub(crate) fn first_baseline(&self) -> Option<f32> {
        self.first_baseline
    }

    #[cfg(test)]
    pub(crate) fn queried_first_baseline(&self) -> Option<f32> {
        self.line.first_baseline()
    }

    pub(crate) fn x_at_byte(&self, byte: u32) -> f32 {
        let local = byte.saturating_sub(self.byte_start).min(self.byte_size);
        self.x + self.line.caret_x(self.shape_byte_at_raw(local))
    }

    pub(crate) fn rects_for_range(&self, range: Range<u32>) -> Vec<Rect> {
        let start = self.shape_byte_at_raw(
            range
                .start
                .saturating_sub(self.byte_start)
                .min(self.byte_size),
        );
        let end = self.shape_byte_at_raw(
            range
                .end
                .saturating_sub(self.byte_start)
                .min(self.byte_size),
        );
        if start >= end {
            return Vec::new();
        }
        self.band_rects(start..end, crate::theme::BackgroundHeight::Line)
    }

    fn extended_bands(
        &self,
        band: &Band,
        display_end: usize,
        gap_below: f32,
        mut draw: impl FnMut(Rect),
    ) {
        let whole_line = band.extent == crate::theme::BackgroundExtent::WholeLine;
        if display_end == 0 {
            if band.through_end || whole_line {
                draw(Rect::new(
                    self.extent_left,
                    0.0,
                    self.extent_right,
                    self.line.content_height() + gap_below,
                ));
            }
            return;
        }
        for row in &self.band_line_ranges {
            let line = &row.text_range;
            let line_end = line.end.min(display_end);
            if band.range.start >= line_end || band.range.end <= line.start {
                continue;
            }
            let seg = band.range.start.max(line.start)..band.range.end.min(line_end);
            let rects = self.band_rects(seg.clone(), crate::theme::BackgroundHeight::Line);
            // A segment of pure line-break has no selection geometry
            // — the band still stands on the row, anchored at the
            // caret (the fill to the extent does the painting).
            let mut union = match rects.first().copied() {
                Some(first) => first,
                None => {
                    let anchor = self.x + self.line.caret_x(seg.start);
                    Rect::from_ltrb(anchor, row.top, anchor, row.bottom)
                }
            };
            for rect in &rects[1..] {
                union.join(*rect);
            }
            let last_line = line.end >= display_end;
            let covers_end = band.range.end >= line_end && (!last_line || band.through_end);

            let stretch = if last_line && covers_end {
                gap_below
            } else {
                0.0
            };
            for rect in rects {
                draw(Rect::new(
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom + stretch,
                ));
            }
            if (covers_end || whole_line) && union.right < self.extent_right {
                draw(Rect::new(
                    union.right,
                    union.top,
                    self.extent_right,
                    union.bottom + stretch,
                ));
            }
            if whole_line && self.extent_left < union.left {
                draw(Rect::new(
                    self.extent_left,
                    union.top,
                    union.left,
                    union.bottom + stretch,
                ));
            }
        }
    }

    /// Band rects for a SHAPE-BYTE range. `Line` height takes the
    /// full line box; `Tight` hugs the content (the caret rect's
    /// vertical at the range start).
    fn band_rects(&self, range: Range<usize>, height: crate::theme::BackgroundHeight) -> Vec<Rect> {
        let rects = self.line.rects(range.clone());
        match height {
            crate::theme::BackgroundHeight::Line => rects
                .into_iter()
                .map(|rect| {
                    Rect::from_ltrb(
                        self.x + rect.left,
                        rect.top,
                        self.x + rect.right,
                        rect.bottom,
                    )
                })
                .collect(),
            crate::theme::BackgroundHeight::Tight => self
                .line
                .content_rects(range)
                .into_iter()
                .map(|rect| {
                    Rect::from_ltrb(
                        self.x + rect.left,
                        rect.top,
                        self.x + rect.right,
                        rect.bottom,
                    )
                })
                .collect(),
        }
    }

    pub(crate) fn byte_at_point(&self, x: f32, y: f32) -> u32 {
        let paragraph_x = (x - self.x).max(0.0);
        let shape_byte = self.line.byte_at_point(paragraph_x, y);
        let display_byte = display_byte_of(&self.excisions, shape_byte);
        let local = self.display.raw_byte_at_display_byte(display_byte);
        self.byte_start
            .saturating_add(local.min(self.byte_size as usize) as u32)
    }

    pub(crate) fn placeholder_rect_at_byte(&self, byte: u32) -> Option<Rect> {
        self.placeholder_rects
            .iter()
            .find(|placeholder| placeholder.byte == byte)
            .map(|placeholder| {
                let rect = placeholder.rect;
                Rect::from_xywh(
                    self.x + rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                )
            })
    }
}

struct InlinePlaceholderRect {
    byte: u32,
    rect: Rect,
}

pub(crate) fn block_gap(resolved: &crate::theme::TextAttributes) -> f32 {
    resolved.block_gap.unwrap_or(0.0)
}

pub(crate) fn line_text_x(document_x: f32, resolved: &crate::theme::TextAttributes) -> f32 {
    document_x + resolved.inset.unwrap_or(0.0)
}

fn horizontal_geometry(
    document_x: f32,
    resolved: &crate::theme::TextAttributes,
    alignment: crate::theme::TextAlignment,
    longest_line: f32,
    layout_width: f32,
) -> (f32, f32, f32) {
    let aligned_box = (layout_width - resolved.inset.unwrap_or(0.0) * 2.0).max(60.0);
    let slack = (aligned_box - longest_line).max(0.0);
    let aligned_x = match alignment {
        crate::theme::TextAlignment::Left => 0.0,
        crate::theme::TextAlignment::Center => slack / 2.0,
        crate::theme::TextAlignment::Right => slack,
    };
    let extent_left = line_text_x(document_x, resolved);
    (
        extent_left + aligned_x,
        extent_left,
        extent_left + aligned_box,
    )
}

pub(crate) fn visible_unit_segments(
    view: &mut TextView,
    markup: OverlaidMarkup<'_, '_>,
    range: &Range<u32>,
) -> Vec<(String, u32)> {
    if range.start >= range.end {
        return vec![(String::new(), 0)];
    }
    let read = |view: &mut TextView, from: u32, to: u32| -> String {
        let mut bytes: Vec<u8> = Vec::with_capacity((to - from) as usize);
        view.byte_range_into(from as usize, to as usize, &mut bytes);
        String::from_utf8(bytes).unwrap_or_else(|error| {
            panic!("text rope must remain valid UTF-8 at [{from}, {to}): {error:?}")
        })
    };
    if !markup.has_inlays() {
        return vec![(read(view, range.start, range.end), 0)];
    }
    let mut segments = Vec::new();
    let mut at = range.start;
    for hit in markup.all_inlays_in(range.clone()) {
        if hit.inlay.mode() != InlayMode::Instead(InsteadKind::Inline) {
            continue;
        }
        let interior = hit.range.start.clamp(range.start, range.end)
            ..hit.range.end.clamp(range.start, range.end);
        if interior.start >= interior.end {
            continue;
        }
        if interior.start > at {
            segments.push((read(view, at, interior.start), at - range.start));
        }
        at = at.max(interior.end);
    }
    if at < range.end {
        segments.push((read(view, at, range.end), at - range.start));
    }
    segments
}

fn visible_segments(raw: &str, range: Range<usize>, hidden: &[Range<u32>]) -> Vec<Range<usize>> {
    let mut segments = Vec::new();
    let mut offset = range.start;

    for hidden_range in hidden {
        let start = (hidden_range.start as usize).clamp(range.start, range.end);
        let end = (hidden_range.end as usize).clamp(range.start, range.end);
        push_visible_segment(raw, offset..start, &mut segments);
        offset = offset.max(end);
    }
    push_visible_segment(raw, offset..range.end, &mut segments);
    segments
}

fn push_visible_segment(raw: &str, segment: Range<usize>, segments: &mut Vec<Range<usize>>) {
    let mut start = segment.start;
    let end = segment.end;
    while start < end && !raw.is_char_boundary(start) {
        start += 1;
    }
    let mut end = end.max(start);
    while end > start && !raw.is_char_boundary(end) {
        end -= 1;
    }
    if start < end {
        segments.push(start..end);
    }
}

fn next_char_len(text: &str, byte: usize) -> usize {
    text.get(byte..)
        .and_then(|text| text.chars().next())
        .map(char::len_utf8)
        .unwrap_or(0)
}

fn starts_with_hard_break(text: &str, byte: usize) -> bool {
    text.get(byte..)
        .and_then(|text| text.chars().next())
        .is_some_and(|char| {
            matches!(
                char,
                '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'
            )
        })
}

pub(crate) fn rows_share_metrics(
    decorations: &[TextDecorationInterval],
    placeholders: &[ParagraphPlaceholder],
    theme: &crate::theme::Theme,
) -> bool {
    let styles = theme.styles();
    placeholders.is_empty()
        && decorations.iter().all(|decoration| {
            let attributes = styles.attributes(decoration.id);
            !attributes.bold
                && !attributes.italic
                && attributes.font_families.is_none()
                && attributes.font_size.is_none()
        })
}

#[cfg(test)]
mod tests;
