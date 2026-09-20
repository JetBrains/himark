// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use skia_safe::{Canvas, Paint, Rect};

use crate::shaped_line::ShapedLine;
use crate::viewport::EditorViewport;

const SELECTION_RECT_CAP: usize = 256;

impl crate::document::Document {
    #[doc(hidden)]
    pub fn paint(
        &self,
        editor: crate::editor::EditorId,
        canvas: &Canvas,
        visible: Rect,
        focused: bool,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) {
        let viewport = EditorViewport::build(
            self,
            editor,
            visible.top..visible.bottom,
            focused,
            false,
            None,
            store,
            ui,
            fonts,
            theme,
        );
        self.paint_with(editor, &viewport, canvas, focused, store, ui, fonts, theme);
    }

    pub(crate) fn paint_with(
        &self,
        editor: crate::editor::EditorId,
        viewport: &EditorViewport,
        canvas: &Canvas,
        focused: bool,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) {
        viewport.paint(canvas, theme);
        if let Some(placeholder) = self.placeholder_line(editor, fonts, theme) {
            placeholder.paint(canvas, 0.0);
        }
        let text_focused = focused && self.focus(editor) == crate::EditorFocus::Text;
        if text_focused {
            self.paint_carets(editor, canvas, store, ui, fonts, theme);
        }
    }

    pub(crate) fn placeholder_line(
        &self,
        editor: crate::editor::EditorId,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Option<ShapedLine> {
        if self.text().byte_count() != 0 {
            return None;
        }
        let state = self.editor(editor);
        let text = &state.placeholder.as_ref()?.text;
        Some(ShapedLine::placeholder(
            text,
            theme.ui().peeker.dim_text.0,
            fonts,
            theme,
            state.layout.layout_width(),
            0.0,
        ))
    }

    fn paint_carets(
        &self,
        editor: crate::editor::EditorId,
        canvas: &Canvas,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) {
        let state = self.editor(editor);
        let layout = &state.layout;
        let extras = self.extras_keyed(editor);
        let overlaid = crate::markup::OverlaidMarkup::new(self.markup(), &extras);
        let caret_chrome = &theme.ui().caret;
        let mut paint = Paint::default();
        paint.set_anti_alias(false);
        paint.set_color(caret_chrome.color.0);

        let mut shaped_line: Option<(u32, ShapedLine)> = None;

        if let Some((caret_x, caret_top, caret_height)) =
            self.empty_caret_geometry(editor, store, ui, fonts, theme)
        {
            canvas.draw_rect(
                Rect::from_xywh(
                    caret_x,
                    caret_top,
                    caret_chrome.width,
                    caret_height.max(caret_chrome.min_height),
                ),
                &paint,
            );
            return;
        }

        for caret in state.carets.carets() {
            let byte = caret.offset();
            let Some((cursor, document_y, byte_start)) = layout.cursor_at_caret(byte) else {
                continue;
            };

            let item = cursor.element();
            if item.height <= 0.0 {
                continue;
            }
            let item_top = document_y + item.spacer_above;
            let byte_end = byte_start.saturating_add(item.byte_size);
            let inlays = {
                let measure = crate::markup::InlayMeasure {
                    width: layout.layout_width(),
                    store,
                    ui,
                };
                overlaid.inlay_metrics_in(byte_start..byte_end, measure)
            };
            let text_top = item_top + inlays.above_height;
            let text_height = inlays.content_height_from_total(item.height);
            if inlays.has_instead() {
                continue;
            }

            if shaped_line.as_ref().map(|(start, _)| *start) != Some(byte_start) {
                shaped_line = Some((
                    byte_start,
                    self.shape_line(
                        editor,
                        byte_start..byte_end,
                        0.0,
                        true,
                        store,
                        ui,
                        fonts,
                        theme,
                    ),
                ));
            }

            let (caret_x, caret_top, caret_height) = shaped_line
                .as_ref()
                .map(|(_, shaped)| shaped.caret_geometry(byte))
                .unwrap_or((0.0, 0.0, 0.0));
            let _ = text_height;

            canvas.draw_rect(
                Rect::from_xywh(
                    caret_x,
                    text_top + caret_top,
                    caret_chrome.width,
                    caret_height.max(caret_chrome.min_height),
                ),
                &paint,
            );
        }
    }

    fn empty_caret_geometry(
        &self,
        editor: crate::editor::EditorId,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Option<(f32, f32, f32)> {
        if self.text().byte_count() != 0 || !self.editor(editor).layout.is_empty() {
            return None;
        }
        Some(
            self.shape_line(editor, 0..0, 0.0, true, store, ui, fonts, theme)
                .caret_geometry(0),
        )
    }

    pub(crate) fn vertical_caret_target(
        &self,
        editor: crate::editor::EditorId,
        offset: u32,
        goal_x: Option<f32>,
        down: bool,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Option<(u32, f32)> {
        let layout = &self.editor(editor).layout;
        let (cursor, document_y, byte_start) = layout.cursor_at_caret(offset)?;
        let item = cursor.element();
        let byte_end = byte_start.saturating_add(item.byte_size);
        let x = goal_x.unwrap_or_else(|| {
            self.shape_line(
                editor,
                byte_start..byte_end,
                0.0,
                true,
                store,
                ui,
                fonts,
                theme,
            )
            .x_at_byte(offset)
        });

        let y = match down {
            true => document_y + item.spacer_above + item.height + 0.5,
            false => document_y - 0.5,
        };
        if y < 0.0 || y >= layout.height() {
            return None;
        }
        let (mut target_cursor, _, mut target_start) = layout.cursor_at_y(y);
        while target_cursor.element().height <= 0.0 {
            target_start = target_start.saturating_add(target_cursor.element().byte_size);
            if !target_cursor.advance() {
                return None;
            }
        }

        if target_start == byte_start {
            return None;
        }
        let target =
            self.caret_at_point_in(editor, x, y, 0.0, 0.0, 0.0, store, ui, fonts, theme)?;
        Some((target, x))
    }

    pub fn caret_content_rect(
        &self,
        editor: crate::editor::EditorId,
        byte: u32,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Option<(f32, f32, f32, f32)> {
        let layout = &self.editor(editor).layout;
        if let Some((x, top, height)) = self.empty_caret_geometry(editor, store, ui, fonts, theme) {
            return Some((x, top, 2.0, height.max(18.0)));
        }
        let (cursor, document_y, byte_start) = layout.cursor_at_caret(byte)?;
        let item = cursor.element();
        if item.height <= 0.0 {
            return None;
        }

        let text_len = self.text().byte_count().min(u32::MAX as usize) as u32;
        if byte_start >= text_len && !(item.byte_size == 0 && byte_start == text_len) {
            return None;
        }
        let byte_end = byte_start.saturating_add(item.byte_size).min(text_len);
        let extras = self.extras_keyed(editor);
        let inlays = {
            let measure = crate::markup::InlayMeasure {
                width: layout.layout_width(),
                store,
                ui,
            };
            crate::markup::OverlaidMarkup::new(self.markup(), &extras)
                .inlay_metrics_in(byte_start..byte_end, measure)
        };
        if inlays.has_instead() {
            return None;
        }
        let text_top = document_y + item.spacer_above + inlays.above_height;
        let shaped = self.shape_line(
            editor,
            byte_start..byte_end,
            0.0,
            true,
            store,
            ui,
            fonts,
            theme,
        );

        let (x, top, height) = shaped.caret_geometry(byte);
        Some((x, text_top + top, 2.0, height.max(18.0)))
    }

    pub fn selection_content_rects(
        &self,
        editor: crate::editor::EditorId,
        range: Range<u32>,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Vec<(f32, f32, f32, f32)> {
        let mut rects = Vec::new();
        if range.start >= range.end {
            return rects;
        }
        let layout = &self.editor(editor).layout;
        let text_len = self.text().byte_count().min(u32::MAX as usize) as u32;
        let mut start = range.start;
        let mut end = range.end.min(text_len);

        let band = self.viewport(editor).map(|band| {
            let slack = (band.end - band.start).max(0.0);
            (band.start - slack)..(band.end + slack)
        });
        if let Some(band) = &band {
            let bytes = layout.byte_band(band.start.max(0.0), band.end);
            start = start.max(bytes.start);
            end = end.min(bytes.end.max(bytes.start));
        }
        if start >= end {
            return rects;
        }
        let Some((mut cursor, mut document_y, mut byte_start)) = layout.cursor_at_caret(start)
        else {
            return rects;
        };
        let extras = self.extras_keyed(editor);
        let overlaid = crate::markup::OverlaidMarkup::new(self.markup(), &extras);
        loop {
            if byte_start >= end || rects.len() >= SELECTION_RECT_CAP {
                break;
            }
            let item = cursor.element();
            let byte_end = byte_start.saturating_add(item.byte_size);
            let bottom = document_y + item.spacer_above + item.height;
            let visible = band
                .as_ref()
                .is_none_or(|band| bottom >= band.start && document_y <= band.end);
            if item.height > 0.0 && visible && byte_start < text_len {
                let inlays = {
                    let measure = crate::markup::InlayMeasure {
                        width: layout.layout_width(),
                        store,
                        ui,
                    };
                    overlaid.inlay_metrics_in(byte_start..byte_end, measure)
                };
                if !inlays.has_instead() {
                    let text_top = document_y + item.spacer_above + inlays.above_height;
                    let line = byte_start..byte_end.min(text_len);
                    let clamped = line.start.max(start)..line.end.min(end);
                    if clamped.start < clamped.end {
                        let shaped =
                            self.shape_line(editor, line, 0.0, true, store, ui, fonts, theme);
                        for rect in shaped.rects_for_range(clamped) {
                            rects.push((
                                rect.left,
                                text_top + rect.top,
                                (rect.right - rect.left).max(2.0),
                                rect.bottom - rect.top,
                            ));
                        }
                    }
                }
            }
            if !cursor.advance() {
                break;
            }
            document_y = bottom;
            byte_start = byte_end;
        }
        rects
    }

    pub(crate) fn caret_at_point_in(
        &self,
        editor: crate::editor::EditorId,
        x: f32,
        y: f32,
        document_x: f32,
        document_top: f32,
        scroll_y: f32,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Option<u32> {
        let layout = &self.editor(editor).layout;
        if layout.is_empty() {
            return None;
        }

        let document_y = (y - document_top + scroll_y).max(0.0);
        let (mut cursor, line_y, mut byte_start) = layout.cursor_at_y(document_y);

        while cursor.element().height <= 0.0 {
            byte_start = byte_start.saturating_add(cursor.element().byte_size);
            if !cursor.advance() {
                return None;
            }
        }
        let item = cursor.element();
        let byte_end = byte_start.saturating_add(item.byte_size);
        let extras = self.extras_keyed(editor);
        let overlaid = crate::markup::OverlaidMarkup::new(self.markup(), &extras);
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        let (marks, inlays) = {
            let measure = crate::markup::InlayMeasure {
                width: layout.layout_width(),
                store,
                ui,
            };
            overlaid.line_marks_in(
                byte_start..byte_end,
                Some(measure),
                &mut inline,
                &mut hidden,
            )
        };
        if marks.resolved(theme).rule.is_some() || inlays.has_instead() {
            return Some(byte_start);
        }

        let mut view = self.text().view();
        let shaped = ShapedLine::new(
            &mut view,
            overlaid,
            byte_start..byte_end,
            &marks,
            &inline,
            &hidden,
            fonts,
            theme,
            crate::markup::InlayMeasure {
                width: layout.layout_width(),
                store,
                ui,
            },
            document_x,
            true,
        );

        let paragraph_y = (document_y - line_y - item.spacer_above - inlays.above_height)
            .clamp(0.0, inlays.content_height_from_total(item.height).max(1.0));
        let byte = shaped.byte_at_point(x, paragraph_y);

        if byte == byte_end && byte_end > byte_start {
            let mut last = Vec::with_capacity(1);
            view.byte_range_into(byte_end as usize - 1, byte_end as usize, &mut last);
            if last.first() == Some(&b'\n') {
                return Some(byte_end - 1);
            }
        }
        Some(byte)
    }

    pub(crate) fn shape_line(
        &self,
        editor: crate::editor::EditorId,
        line_range: Range<u32>,
        document_x: f32,
        map_utf16: bool,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> ShapedLine {
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        let extras = self.extras_keyed(editor);
        let overlaid = crate::markup::OverlaidMarkup::new(self.markup(), &extras);
        let marks = overlaid.marks_inline_hidden_in(line_range.clone(), &mut inline, &mut hidden);
        let mut view = self.text().view();
        ShapedLine::new(
            &mut view,
            overlaid,
            line_range,
            &marks,
            &inline,
            &hidden,
            fonts,
            theme,
            crate::markup::InlayMeasure {
                width: self.editor(editor).layout.layout_width(),
                store,
                ui,
            },
            document_x,
            map_utf16,
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder_line_exists_only_while_document_is_empty() {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
        let fonts = crate::embedded_fonts::collection();
        let theme = crate::theme::Theme::embedded();
        let mut document = crate::Document::new(
            text::Text::from_string_exact(""),
            crate::markup::Markup::new(),
        );
        let editor = document.add_editor(
            500.0,
            None,
            crate::EditorBuild::Complete,
            &[],
            store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
        document.set_placeholder(editor, "Message the agent", &fonts, &theme);

        let placeholder_height = document.content_height(editor);
        assert!(placeholder_height > 0.0, "the placeholder reserves its row");

        assert!(document.placeholder_line(editor, &fonts, &theme).is_some());
        document.insert(
            editor,
            "M",
            store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
        assert!(document.placeholder_line(editor, &fonts, &theme).is_none());
        assert_eq!(
            document.content_height(editor),
            placeholder_height,
            "typing the first line must not resize the editor"
        );
    }
}
