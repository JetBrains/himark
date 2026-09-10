use std::{ops::Range, str};

use skia_safe::{
    textlayout::{
        FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, PlaceholderAlignment,
        PlaceholderStyle, RectHeightStyle, RectWidthStyle, TextBaseline, TextDecoration,
        TextDecorationStyle, TextDirection, TextStyle,
    },
    Canvas, FontStyle, Point, Rect,
};
use text::TextView;

use crate::markup::{
    BlockStyle, InlayMode, InlayPlaceholder, InsteadKind, OverlaidMarkup, TextDecorationInterval,
};

const RENDER_LINE_WIDTH: f32 = 1_000_000.0;

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
                })
            })
            .collect()
    }

    pub(crate) fn add_inline_placeholders(
        &mut self,
        markup: OverlaidMarkup<'_, '_>,
        range: Range<u32>,
        width: f32,
    ) {
        for placeholder in markup.inline_placeholders_in(range.clone(), width) {
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

    pub(crate) fn line_text_end(
        &self,
        paragraph: &Paragraph,
        line_index: usize,
        is_last: bool,
    ) -> usize {
        if is_last {
            return self.text.len();
        }

        let mut text_end = paragraph
            .get_actual_text_range(line_index, true)
            .end
            .min(self.text.len());
        if starts_with_hard_break(&self.text, text_end) {
            text_end = text_end.saturating_add(next_char_len(&self.text, text_end));
        }
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
    range: Range<usize>,
    color: skia_safe::Color,
    extent: crate::theme::BackgroundExtent,

    height: crate::theme::BackgroundHeight,

    through_end: bool,
}

pub(crate) struct ShapedLine {
    byte_start: u32,
    byte_size: u32,
    x: f32,
    display: DisplayText,
    paragraph: Paragraph,
    placeholder_rects: Vec<InlinePlaceholderRect>,

    backgrounds: Vec<Band>,

    extent_left: f32,
    extent_right: f32,

    line_height_shrink: f32,

    first_baseline: Option<f32>,
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
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        layout_width: f32,
        document_x: f32,
        map_utf16: bool,
    ) -> Self {
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
        display.add_inline_placeholders(markup, line_range.clone(), layout_width);

        let inline = display.map_decorations(inline);

        let shape_text = if display.as_str().is_empty() && display.placeholders().is_empty() {
            "\u{200b}"
        } else {
            display.as_str()
        };
        let mut paragraph = line_paragraph_with_placeholders(
            &resolved,
            shape_text,
            fonts.clone(),
            theme,
            &inline,
            display.placeholders(),
        );
        paragraph.layout(RENDER_LINE_WIDTH);
        let placeholder_rects = match display.placeholders().is_empty() {
            true => Vec::new(),
            false => placeholder_rects(&paragraph, display.placeholders()),
        };

        let backgrounds: Vec<Band> = band_styled
            .iter()
            .filter_map(|decoration| {
                let background = theme.attributes(decoration.id).background?;
                let start = display.display_utf16_at_raw(decoration.range.start);
                let end = display.display_utf16_at_raw(decoration.range.end);

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
            horizontal_geometry(document_x, &resolved, alignment, &paragraph, layout_width);

        let first_baseline = paragraph
            .get_line_metrics_at(0)
            .map(|metrics| metrics.baseline as f32);

        Self {
            byte_start: line_range.start,
            byte_size,
            x,
            display,
            paragraph,
            placeholder_rects,
            backgrounds,
            extent_left,
            extent_right,
            line_height_shrink: resolved.line_height.unwrap_or(1.0).max(0.5),
            first_baseline,
        }
    }

    pub(crate) fn placeholder(
        text: &str,
        color: skia_safe::Color,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        layout_width: f32,
        document_x: f32,
    ) -> Self {
        let mut resolved = BlockStyle::default().resolved(theme);
        resolved.color = Some(color);
        let alignment = resolved.alignment.take().unwrap_or_default();
        let segments = [(text.to_owned(), 0)];
        let display = DisplayText::build_segments(false, &segments, &[], true, text.len());
        let mut paragraph = line_paragraph_with_placeholders(
            &resolved,
            display.as_str(),
            fonts.clone(),
            theme,
            &[],
            &[],
        );
        paragraph.layout(RENDER_LINE_WIDTH);
        let (x, extent_left, extent_right) =
            horizontal_geometry(document_x, &resolved, alignment, &paragraph, layout_width);

        let first_baseline = paragraph
            .get_line_metrics_at(0)
            .map(|metrics| metrics.baseline as f32);

        Self {
            byte_start: 0,
            byte_size: text.len().min(u32::MAX as usize) as u32,
            x,
            display,
            paragraph,
            placeholder_rects: Vec::new(),
            backgrounds: Vec::new(),
            extent_left,
            extent_right,
            line_height_shrink: resolved.line_height.unwrap_or(1.0).max(0.5),
            first_baseline,
        }
    }

    pub(crate) fn paint(&self, canvas: &Canvas, top: f32) {
        self.paint_in_slot(canvas, top, top + self.paragraph.height());
    }

    pub(crate) fn paint_in_slot(&self, canvas: &Canvas, top: f32, slot_bottom: f32) {
        if !self.backgrounds.is_empty() {
            let mut paint = skia_safe::Paint::default();
            let display_end = self.display.as_str().encode_utf16().count();

            let gap_below = (slot_bottom - top - self.paragraph.height()).max(0.0);
            for band in &self.backgrounds {
                paint.set_color(band.color);
                match band.extent {
                    crate::theme::BackgroundExtent::Glyphs => {
                        let style = match band.height {
                            crate::theme::BackgroundHeight::Tight => RectHeightStyle::Tight,
                            crate::theme::BackgroundHeight::Line => RectHeightStyle::Max,
                        };
                        for rect in self.band_rects(band.range.clone(), style) {
                            canvas.draw_rect(rect.with_offset((0.0, top)), &paint);
                        }
                    }
                    crate::theme::BackgroundExtent::ToLineEnd
                    | crate::theme::BackgroundExtent::WholeLine => {
                        let gap = if band.through_end { gap_below } else { 0.0 };
                        self.extended_bands(band, display_end, gap, |rect| {
                            canvas.draw_rect(rect.with_offset((0.0, top)), &paint);
                        });
                    }
                }
            }
        }
        self.paragraph.paint(canvas, (self.x, top));
    }

    pub(crate) fn layout_height(&self, theme: &crate::theme::Theme) -> f32 {
        let text_height = self
            .paragraph
            .get_line_metrics_at(0)
            .map(|metrics| (metrics.ascent.abs() + metrics.descent).ceil().max(1.0) as f32)
            .unwrap_or(0.0);
        text_height + block_gap(theme.base())
    }

    pub(crate) fn caret_geometry(&self, byte: u32) -> (f32, f32, f32) {
        let local = byte.saturating_sub(self.byte_start).min(self.byte_size);
        let display = self.display.display_utf16_at_raw(local);
        let x = self.x + caret_x_for_display_position(&self.paragraph, display);

        let tight = |range: Range<usize>| {
            self.paragraph
                .get_rects_for_range(range, RectHeightStyle::Tight, RectWidthStyle::Tight)
                .into_iter()
                .next()
                .map(|text_box| text_box.rect)
        };
        let anchor = display
            .checked_sub(1)
            .and_then(|before| tight(before..display))
            .or_else(|| tight(display..display + 1));
        if let Some(rect) = anchor {
            return (x, rect.top, rect.bottom - rect.top);
        }

        match self.paragraph.get_line_metrics_at(0) {
            Some(metrics) => {
                let ascent = metrics.ascent as f32 / self.line_height_shrink;
                let descent = metrics.descent as f32 / self.line_height_shrink;
                (x, metrics.baseline as f32 - ascent, ascent + descent)
            }
            None => (x, 0.0, 0.0),
        }
    }

    pub(crate) fn first_baseline(&self) -> Option<f32> {
        self.first_baseline
    }

    #[cfg(test)]
    pub(crate) fn queried_first_baseline(&self) -> Option<f32> {
        self.paragraph
            .get_line_metrics_at(0)
            .map(|metrics| metrics.baseline as f32)
    }

    pub(crate) fn x_at_byte(&self, byte: u32) -> f32 {
        let local = byte.saturating_sub(self.byte_start).min(self.byte_size);
        let display = self.display.display_utf16_at_raw(local);
        self.x + caret_x_for_display_position(&self.paragraph, display)
    }

    pub(crate) fn rects_for_range(&self, range: Range<u32>) -> Vec<Rect> {
        let start = self.display.display_utf16_at_raw(
            range
                .start
                .saturating_sub(self.byte_start)
                .min(self.byte_size),
        );
        let end = self.display.display_utf16_at_raw(
            range
                .end
                .saturating_sub(self.byte_start)
                .min(self.byte_size),
        );
        if start >= end {
            return Vec::new();
        }
        self.line_bands(start..end)
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
                    self.paragraph.height() + gap_below,
                ));
            }
            return;
        }
        for line in self.paragraph.get_line_metrics() {
            let line_end = line.end_index.min(display_end);
            if band.range.start >= line_end || band.range.end <= line.start_index {
                continue;
            }
            let seg = band.range.start.max(line.start_index)..band.range.end.min(line_end);
            let rects = self.line_bands(seg);
            let Some(mut union) = rects.first().copied() else {
                continue;
            };
            for rect in &rects[1..] {
                union.join(*rect);
            }
            let last_line = line.end_index >= display_end;
            let covers_end = band.range.end >= line_end && (!last_line || band.through_end);

            let stretch = if last_line && covers_end { gap_below } else { 0.0 };
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

    fn line_bands(&self, range: Range<usize>) -> Vec<Rect> {
        self.band_rects(range, RectHeightStyle::Max)
    }

    fn band_rects(&self, range: Range<usize>, height: RectHeightStyle) -> Vec<Rect> {
        self.paragraph
            .get_rects_for_range(range, height, RectWidthStyle::Tight)
            .into_iter()
            .map(|text_box| {
                let rect = text_box.rect;
                Rect::from_ltrb(
                    self.x + rect.left,
                    rect.top,
                    self.x + rect.right,
                    rect.bottom,
                )
            })
            .collect()
    }

    pub(crate) fn byte_at_point(&self, x: f32, y: f32) -> u32 {
        let paragraph_x = (x - self.x).max(0.0);
        let position = self
            .paragraph
            .get_glyph_position_at_coordinate(Point::new(paragraph_x, y));
        let local = self
            .display
            .raw_byte_at_utf16(position.position.max(0) as usize);
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

fn placeholder_rects(
    paragraph: &Paragraph,
    placeholders: &[ParagraphPlaceholder],
) -> Vec<InlinePlaceholderRect> {
    paragraph
        .get_rects_for_placeholders()
        .into_iter()
        .zip(placeholders.iter())
        .map(|(text_box, placeholder)| InlinePlaceholderRect {
            byte: placeholder.byte,
            rect: text_box.rect,
        })
        .collect()
}

pub(crate) fn line_text_x(document_x: f32, resolved: &crate::theme::TextAttributes) -> f32 {
    document_x + resolved.inset.unwrap_or(0.0)
}

fn horizontal_geometry(
    document_x: f32,
    resolved: &crate::theme::TextAttributes,
    alignment: crate::theme::TextAlignment,
    paragraph: &Paragraph,
    layout_width: f32,
) -> (f32, f32, f32) {
    let aligned_box = (layout_width - resolved.inset.unwrap_or(0.0) * 2.0).max(60.0);
    let slack = (aligned_box - paragraph.longest_line()).max(0.0);
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

fn caret_x_for_display_position(paragraph: &Paragraph, display_position: usize) -> f32 {
    if display_position == 0 {
        return 0.0;
    }

    paragraph
        .get_rects_for_range(
            0..display_position,
            RectHeightStyle::Tight,
            RectWidthStyle::Tight,
        )
        .into_iter()
        .map(|text_box| text_box.rect.right)
        .fold(0.0, f32::max)
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
    placeholders.is_empty()
        && decorations.iter().all(|decoration| {
            let attributes = theme.attributes(decoration.id);
            !attributes.bold
                && !attributes.italic
                && attributes.font_families.is_none()
                && attributes.font_size.is_none()
        })
}

#[cfg(test)]
pub(crate) fn line_paragraph(
    marks: &BlockStyle,
    text: &str,
    fonts: FontCollection,
    theme: &crate::theme::Theme,
    decorations: &[TextDecorationInterval],
) -> Paragraph {
    line_paragraph_with_placeholders(&marks.resolved(theme), text, fonts, theme, decorations, &[])
}

fn line_paragraph_with_placeholders(
    resolved: &crate::theme::TextAttributes,
    text: &str,
    fonts: FontCollection,
    theme: &crate::theme::Theme,
    decorations: &[TextDecorationInterval],
    placeholders: &[ParagraphPlaceholder],
) -> Paragraph {
    paragraph_with_max_lines(
        resolved,
        text,
        fonts,
        theme,
        decorations,
        placeholders,
        Some(1),
    )
}

pub(crate) fn paragraph(
    resolved: &crate::theme::TextAttributes,
    text: &str,
    fonts: FontCollection,
    theme: &crate::theme::Theme,
    decorations: &[TextDecorationInterval],
    placeholders: &[ParagraphPlaceholder],
) -> Paragraph {
    paragraph_with_max_lines(
        resolved,
        text,
        fonts,
        theme,
        decorations,
        placeholders,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn paragraph_with_max_lines(
    resolved: &crate::theme::TextAttributes,
    text: &str,
    fonts: FontCollection,
    theme: &crate::theme::Theme,
    decorations: &[TextDecorationInterval],
    placeholders: &[ParagraphPlaceholder],
    max_lines: Option<usize>,
) -> Paragraph {
    let text_style = text_style(resolved);
    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_direction(TextDirection::LTR);
    paragraph_style.set_text_style(&text_style);
    paragraph_style.set_text_align(match resolved.alignment.unwrap_or_default() {
        crate::theme::TextAlignment::Left => skia_safe::textlayout::TextAlign::Left,
        crate::theme::TextAlignment::Center => skia_safe::textlayout::TextAlign::Center,
        crate::theme::TextAlignment::Right => skia_safe::textlayout::TextAlign::Right,
    });
    if let Some(max_lines) = max_lines {
        paragraph_style.set_max_lines(max_lines);
    }

    let mut builder = ParagraphBuilder::new(&paragraph_style, fonts);
    add_text_runs(
        &mut builder,
        text,
        &text_style,
        decorations,
        placeholders,
        theme,
    );
    builder.build()
}

fn add_text_runs(
    builder: &mut ParagraphBuilder,
    text: &str,
    base_style: &TextStyle,
    decorations: &[TextDecorationInterval],
    placeholders: &[ParagraphPlaceholder],
    theme: &crate::theme::Theme,
) {
    if decorations.is_empty() && placeholders.is_empty() {
        builder.add_text(text);
        return;
    }

    let mut offset = 0;
    for placeholder in placeholders {
        let start = placeholder.range.start.min(text.len());
        let end = placeholder.range.end.min(text.len());
        if start < offset
            || end < start
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            continue;
        }

        add_text_slice(builder, text, base_style, decorations, theme, offset..start);
        builder.add_placeholder(&PlaceholderStyle::new(
            placeholder.width,
            placeholder.height,
            match placeholder.align {
                crate::markup::InlayAlignment::Top => PlaceholderAlignment::Top,
                crate::markup::InlayAlignment::Middle => PlaceholderAlignment::Middle,
                crate::markup::InlayAlignment::Bottom => PlaceholderAlignment::Bottom,
            },
            TextBaseline::Alphabetic,
            0.0,
        ));
        offset = end;
    }

    add_text_slice(
        builder,
        text,
        base_style,
        decorations,
        theme,
        offset..text.len(),
    );
}

fn add_text_slice(
    builder: &mut ParagraphBuilder,
    text: &str,
    base_style: &TextStyle,
    decorations: &[TextDecorationInterval],
    theme: &crate::theme::Theme,
    range: Range<usize>,
) {
    if range.start >= range.end {
        return;
    }

    if decorations.is_empty() {
        builder.add_text(&text[range]);
        return;
    }

    let mut cuts: Vec<usize> = Vec::with_capacity(decorations.len() * 2 + 1);
    for decoration in decorations {
        for edge in [
            decoration.range.start as usize,
            decoration.range.end as usize,
        ] {
            if edge > range.start && edge < range.end && text.is_char_boundary(edge) {
                cuts.push(edge);
            }
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts.push(range.end);

    let mut active: Vec<&TextDecorationInterval> = Vec::new();
    let mut next = 0usize;
    let mut start = range.start;
    for &end in &cuts {
        if start >= end {
            continue;
        }
        while let Some(decoration) = decorations.get(next) {
            if (decoration.range.start as usize).max(range.start) <= start {
                active.push(decoration);
                next += 1;
            } else {
                break;
            }
        }
        active.retain(|decoration| (decoration.range.end as usize) > start);
        if active.is_empty() {
            builder.push_style(base_style);
        } else {
            let mut run_style = base_style.clone();

            for decoration in &active {
                apply_attributes(&mut run_style, theme.attributes(decoration.id));
            }
            builder.push_style(&run_style);
        }
        builder.add_text(&text[start..end]);
        builder.pop();
        start = end;
    }
}

pub(crate) fn apply_attributes(
    text_style: &mut TextStyle,
    attributes: &crate::theme::TextAttributes,
) {
    if attributes.bold || attributes.italic {
        let current = text_style.font_style();
        let weight = if attributes.bold {
            skia_safe::font_style::Weight::BOLD
        } else {
            current.weight()
        };
        let slant = if attributes.italic {
            skia_safe::font_style::Slant::Italic
        } else {
            current.slant()
        };
        text_style.set_font_style(FontStyle::new(weight, current.width(), slant));
    }
    let mut decorations = TextDecoration::empty();
    if attributes.underline {
        decorations |= TextDecoration::UNDERLINE;
    }
    if attributes.strikethrough {
        decorations |= TextDecoration::LINE_THROUGH;
    }
    if !decorations.is_empty() {
        text_style.set_decoration_type(decorations);
        text_style.set_decoration_style(TextDecorationStyle::Solid);
        text_style.set_decoration_color(attributes.color.unwrap_or_else(|| text_style.color()));
    }
    if let Some(color) = attributes.color {
        text_style.set_color(color);
    }

    if let Some(families) = &attributes.font_families {
        text_style.set_font_families(families);
    }
    if let Some(size) = attributes.font_size {
        text_style.set_font_size(size);
    }
    if let Some(height) = attributes.line_height {
        text_style.set_height(height);
        text_style.set_height_override(true);
    }
}

fn text_style(resolved: &crate::theme::TextAttributes) -> TextStyle {
    let mut text_style = TextStyle::new();
    text_style.set_height_override(true);
    apply_attributes(&mut text_style, resolved);
    text_style
}

pub(crate) fn block_gap(resolved: &crate::theme::TextAttributes) -> f32 {
    resolved.block_gap.unwrap_or(0.0)
}

#[cfg(test)]
mod tests;
