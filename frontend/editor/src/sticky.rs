use imba::arena::Arena;
use imba::event::{Event, EventResult};
use skia_safe::{Point, Rect, Size};

use crate::document::Document;
use crate::editor::EditorId;
use crate::editor_view::EditorCommand;

pub const HOST: imba::overlay::OverlayHost = imba::overlay::OverlayHost("editor.sticky");

const MAX_ROWS: usize = 5;

struct StickyRow {
    scope_start: u32,

    line: std::ops::Range<u32>,

    number: u32,

    height: f32,
}

pub(crate) fn sticky_overlays<'a>(
    document: &Document,
    editor: EditorId,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &crate::theme::Theme,
    arena: &'a Arena,
    viewport: Rect,
    origin_x: f32,
    gutter_width: f32,
    width: f32,
) -> Vec<imba::overlay::Overlay<'a, EditorCommand>> {
    if viewport.top <= 0.0 || viewport.height() <= 0.0 {
        return Vec::new();
    }
    let rows = sticky_rows(document, editor, viewport);
    if rows.is_empty() {
        return Vec::new();
    }
    let height: f32 = rows.iter().map(|row| row.height).sum();
    let seed = StickySeed {
        document: document.clone(),
        editor,
        rows,
        fonts: fonts.clone(),
        theme: theme.clone(),
        origin_x,
        gutter_width,
        arena,
    };
    vec![imba::overlay::Overlay {
        host: HOST,

        anchor: Rect::from_xywh(0.0, viewport.top, width.max(1.0), height),
        content: Box::new(move |host_size: Size, anchor: Rect| seed.layout(host_size, anchor)),
    }]
}

fn sticky_rows(document: &Document, editor: EditorId, viewport: Rect) -> Vec<StickyRow> {
    let Some(state) = document.editors.get(&editor) else {
        return Vec::new();
    };
    let text_len = document.text().byte_count().min(u32::MAX as usize) as u32;
    let extras = document.extras_keyed(editor);
    let mut rows: Vec<StickyRow> = Vec::new();
    let mut consumed = 0.0;
    while rows.len() < MAX_ROWS {
        let probe_y = viewport.top + consumed;
        if probe_y >= viewport.bottom {
            break;
        }
        let probe = document.first_visible_byte(editor, probe_y);
        let chain = document.outline_enclosing(probe);
        if chain.len() <= rows.len() {
            break;
        }

        if !rows
            .iter()
            .zip(chain.iter())
            .all(|(row, scope)| row.scope_start == scope.start)
        {
            break;
        }
        let scope = chain[rows.len()].clone();
        let Some((cursor, document_y, byte_start)) = state.layout.cursor_at_caret(scope.start)
        else {
            break;
        };
        let item = cursor.element();

        if item.height <= 0.0
            || byte_start >= text_len
            || rows.last().is_some_and(|row| row.line.start == byte_start)
        {
            break;
        }

        if document_y + item.spacer_above >= probe_y {
            break;
        }
        let byte_end = byte_start.saturating_add(item.byte_size).min(text_len);
        let inlays = crate::markup::OverlaidMarkup::new(document.markup(), &extras)
            .inlay_metrics_in(byte_start..byte_end, state.layout.layout_width());
        if inlays.has_instead() {
            break;
        }
        let height = inlays.content_height_from_total(item.height);
        if height <= 0.0 {
            break;
        }
        let number = document.text().view().line_at(byte_start as usize).0 as u32 + 1;
        rows.push(StickyRow {
            scope_start: scope.start,
            line: byte_start..byte_end,
            number,
            height,
        });
        consumed += height;
    }
    rows
}

struct StickySeed<'a> {
    document: Document,
    editor: EditorId,
    rows: Vec<StickyRow>,
    fonts: skia_safe::textlayout::FontCollection,
    theme: crate::theme::Theme,
    origin_x: f32,
    gutter_width: f32,
    arena: &'a Arena,
}

impl<'a> StickySeed<'a> {
    fn layout(
        self,
        host_size: Size,
        anchor: Rect,
    ) -> Vec<(Point, imba::ThunkBox<'a, EditorCommand>)> {
        let width = anchor.width().min((host_size.width - anchor.left).max(0.0));
        let height: f32 = self.rows.iter().map(|row| row.height).sum();
        let arena = self.arena;
        let widget = StickyWidget {
            document: self.document,
            editor: self.editor,
            rows: self.rows,
            fonts: self.fonts,
            theme: self.theme,
            origin_x: self.origin_x,
            gutter_width: self.gutter_width,
            size: Size::new(width, height),
        };
        vec![(
            Point::new(anchor.left, anchor.top),
            imba::ThunkBox::new(arena, imba::eager(widget)),
        )]
    }
}

struct StickyWidget {
    document: Document,
    editor: EditorId,
    rows: Vec<StickyRow>,
    fonts: skia_safe::textlayout::FontCollection,
    theme: crate::theme::Theme,

    origin_x: f32,
    gutter_width: f32,
    size: Size,
}

impl StickyWidget {
    fn row_at(&self, y: f32) -> Option<(&StickyRow, f32)> {
        let mut top = 0.0;
        for row in &self.rows {
            if y < top + row.height {
                return Some((row, top));
            }
            top += row.height;
        }
        None
    }
}

impl<'a> imba::Widget<'a, EditorCommand> for StickyWidget {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<EditorCommand> {
        match event {
            Event::Paint { canvas, .. } => {
                let window = &self.theme.ui().window;
                let bounds = Rect::from_size(self.size);
                let mut paint = skia_safe::Paint::default();
                paint.set_anti_alias(false);

                paint.set_color(window.background.0);
                canvas.draw_rect(bounds, &paint);

                canvas.save();
                canvas.clip_rect(
                    Rect::from_ltrb(self.gutter_width, 0.0, bounds.right, bounds.bottom),
                    None,
                    false,
                );
                let mut top = 0.0;
                for row in &self.rows {
                    canvas.save();
                    canvas.clip_rect(
                        Rect::from_ltrb(self.gutter_width, top, bounds.right, top + row.height),
                        None,
                        false,
                    );
                    let shaped = self.document.shape_line(
                        self.editor,
                        row.line.clone(),
                        self.origin_x,
                        false,
                        &self.fonts,
                        &self.theme,
                    );
                    shaped.paint(canvas, top);
                    canvas.restore();
                    top += row.height;
                }
                canvas.restore();

                if self.gutter_width > 0.0 {
                    let chrome = self.theme.ui().editor_gutter.clone();
                    let font = crate::editor_view::gutter_font(chrome.number_size);
                    let (_, metrics) = font.metrics();
                    let mut number_paint = skia_safe::Paint::default();
                    number_paint.set_anti_alias(true);
                    number_paint.set_color(chrome.number_color.0);
                    let right = self.gutter_width - chrome.pad - chrome.fold_size;
                    let mut top = 0.0;
                    let mut digits = String::with_capacity(8);
                    for row in &self.rows {
                        let baseline = top
                            + (row.height - (metrics.descent - metrics.ascent)) * 0.5
                            - metrics.ascent;
                        digits.clear();
                        use std::fmt::Write;
                        let _ = write!(digits, "{}", row.number);
                        let width = font.measure_str(&digits, Some(&number_paint)).0;
                        canvas.draw_str(&digits, (right - width, baseline), &font, &number_paint);
                        top += row.height;
                    }
                }

                paint.set_color(window.divider.0);
                canvas.draw_rect(
                    Rect::from_xywh(
                        0.0,
                        bounds.bottom - window.divider_width,
                        bounds.width(),
                        window.divider_width,
                    ),
                    &paint,
                );
                EventResult::Handled
            }

            Event::MouseDown {
                button: imba::event::MouseButton::Left,
                point,
                ..
            } => match self.row_at(point.y) {
                Some((row, _)) => EventResult::Command(EditorCommand::RevealAt {
                    byte: row.scope_start,
                }),
                None => EventResult::Handled,
            },

            _ => EventResult::Ignored,
        }
    }
}
