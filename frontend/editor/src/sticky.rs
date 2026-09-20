// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use imba::arena::Arena;
use imba::event::{Event, EventResult};
use skia_safe::{Point, Rect, Size};

use crate::document::{Document, DocumentToken};
use crate::editor::EditorId;
use crate::editor_view::EditorCommand;
use crate::shaped_line::ShapedLine;

pub const HOST: imba::overlay::OverlayHost = imba::overlay::OverlayHost("editor.sticky");

const MAX_ROWS: usize = 5;

struct StickyRow {
    scope_start: u32,

    line_start: u32,

    number: u32,

    height: f32,

    shaped: Rc<ShapedLine>,
}

pub(crate) struct StickyViewport {
    rows: Vec<StickyRow>,
    height: f32,
}

#[derive(PartialEq, Eq)]
struct Stamp {
    revision: u64,
    markup_generation: u64,
    theme: Arc<str>,
    width_bits: u32,
    origin_bits: u32,
}

impl Stamp {
    fn same_but_markup(&self, other: &Stamp) -> bool {
        self.revision == other.revision
            && self.theme == other.theme
            && self.width_bits == other.width_bits
            && self.origin_bits == other.origin_bits
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
struct Band {
    top_bits: u32,
    bottom_bits: u32,
}

struct Entry {
    stamp: Stamp,
    band: Band,
    viewport: Rc<StickyViewport>,
    last_use: u64,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<(DocumentToken, EditorId), Entry>,
    clock: u64,
}

const CACHE_CAPACITY: usize = 64;
const CACHE_TTL: u64 = 256;

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

impl StickyViewport {
    pub(crate) fn shared(
        document: &Document,
        editor: EditorId,
        viewport: Rect,
        origin_x: f32,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Rc<StickyViewport> {
        let width_bits = document
            .editors
            .get(&editor)
            .map(|state| state.layout.layout_width().to_bits())
            .unwrap_or(0);
        let stamp = Stamp {
            revision: document.revision(),
            markup_generation: document.markup_generation(),
            theme: theme.name_shared(),
            width_bits,
            origin_bits: origin_x.to_bits(),
        };
        let band = Band {
            top_bits: viewport.top.to_bits(),
            bottom_bits: viewport.bottom.to_bits(),
        };
        let key = (document.token(), editor);
        CACHE.with(|cell| {
            let previous = {
                let mut cache = cell.borrow_mut();
                cache.clock += 1;
                let clock = cache.clock;
                match cache.entries.get_mut(&key) {
                    Some(entry) if entry.stamp == stamp => {
                        entry.last_use = clock;
                        if entry.band == band {
                            return Rc::clone(&entry.viewport);
                        }
                        Some((Rc::clone(&entry.viewport), None))
                    }
                    Some(entry) if entry.stamp.same_but_markup(&stamp) => Some((
                        Rc::clone(&entry.viewport),
                        Some(entry.stamp.markup_generation),
                    )),
                    _ => None,
                }
            };
            let built = Rc::new(Self::build(
                document,
                editor,
                viewport,
                origin_x,
                store,
                ui,
                fonts,
                theme,
                previous
                    .as_ref()
                    .map(|(viewport, since)| (viewport.as_ref(), *since)),
            ));
            let mut cache = cell.borrow_mut();
            let clock = cache.clock;
            if cache.entries.len() >= CACHE_CAPACITY {
                cache
                    .entries
                    .retain(|_, entry| entry.last_use + CACHE_TTL >= clock);
            }
            cache.entries.insert(
                key,
                Entry {
                    stamp,
                    band,
                    viewport: Rc::clone(&built),
                    last_use: clock,
                },
            );
            built
        })
    }

    fn build(
        document: &Document,
        editor: EditorId,
        viewport: Rect,
        origin_x: f32,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        previous: Option<(&StickyViewport, Option<u64>)>,
    ) -> Self {
        let mut view = document.text().view();
        let mut row_for = |line_start: u32, line: std::ops::Range<u32>| -> (u32, Rc<ShapedLine>) {
            if let Some(row) = previous.and_then(|(previous, since)| {
                previous
                    .rows
                    .iter()
                    .find(|row| row.line_start == line_start)
                    .filter(|_| {
                        since.is_none_or(|since| !document.markup_changed_in(since, line.clone()))
                    })
            }) {
                return (row.number, Rc::clone(&row.shaped));
            }
            let number = view.line_at(line_start as usize).0 as u32 + 1;
            let shaped = Rc::new(
                document.shape_line(editor, line, origin_x, false, store, ui, fonts, theme),
            );
            (number, shaped)
        };
        let rows = sticky_rows(document, editor, store, ui, viewport, &mut row_for);
        let height = rows.iter().map(|row| row.height).sum();
        Self { rows, height }
    }

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

#[allow(clippy::too_many_arguments)]
pub(crate) fn sticky_overlays<'a>(
    document: &Document,
    editor: EditorId,
    store: &imba::store::Store,
    ui: &imba::UiCtx,
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
    let sticky = StickyViewport::shared(
        document, editor, viewport, origin_x, store, ui, fonts, theme,
    );
    if sticky.rows.is_empty() {
        return Vec::new();
    }
    let height = sticky.height;
    let seed = StickySeed {
        sticky,
        theme: theme.clone(),
        gutter_width,
        arena,
    };
    vec![imba::overlay::Overlay {
        host: HOST,

        anchor: Rect::from_xywh(0.0, viewport.top, width.max(1.0), height),
        content: Box::new(move |host_size: Size, anchor: Rect| seed.layout(host_size, anchor)),
    }]
}

fn sticky_rows(
    document: &Document,
    editor: EditorId,
    store: &imba::store::Store,
    ui: &imba::UiCtx,
    viewport: Rect,
    row_for: &mut dyn FnMut(u32, std::ops::Range<u32>) -> (u32, Rc<ShapedLine>),
) -> Vec<StickyRow> {
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
            || rows.last().is_some_and(|row| row.line_start == byte_start)
        {
            break;
        }

        if document_y + item.spacer_above >= probe_y {
            break;
        }
        let byte_end = byte_start.saturating_add(item.byte_size).min(text_len);
        let inlays = {
            let measure = crate::markup::InlayMeasure {
                width: state.layout.layout_width(),
                store,
                ui,
            };
            crate::markup::OverlaidMarkup::new(document.markup(), &extras)
                .inlay_metrics_in(byte_start..byte_end, measure)
        };
        if inlays.has_instead() {
            break;
        }
        let height = inlays.content_height_from_total(item.height);
        if height <= 0.0 {
            break;
        }
        let (number, shaped) = row_for(byte_start, byte_start..byte_end);
        rows.push(StickyRow {
            scope_start: scope.start,
            line_start: byte_start,
            number,
            height,
            shaped,
        });
        consumed += height;
    }
    rows
}

struct StickySeed<'a> {
    sticky: Rc<StickyViewport>,
    theme: crate::theme::Theme,
    gutter_width: f32,
    arena: &'a Arena,
}

impl<'a> StickySeed<'a> {
    fn layout(
        self,
        host_size: Size,
        anchor: Rect,
    ) -> Vec<(Point, imba::ThunkBox<'a, EditorCommand>)> {
        // The band and its divider run EDGE TO EDGE of the hosting
        // pane; the anchor only says where the editor's content
        // starts inside it.
        let height = self.sticky.height;
        let arena = self.arena;
        let widget = StickyWidget {
            sticky: self.sticky,
            theme: self.theme,
            gutter_width: self.gutter_width,
            content_left: anchor.left.max(0.0),
            size: Size::new(host_size.width.max(1.0), height),
        };
        vec![(
            Point::new(0.0, anchor.top),
            imba::ThunkBox::new(arena, imba::eager(widget)),
        )]
    }
}

struct StickyWidget {
    sticky: Rc<StickyViewport>,
    theme: crate::theme::Theme,

    gutter_width: f32,

    /// Where the editor's content starts inside the pane-wide band.
    content_left: f32,
    size: Size,
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

                // Text and gutter numbers live in EDITOR coordinates;
                // the band itself spans the whole pane.
                canvas.save();
                canvas.translate((self.content_left, 0.0));
                let text_right = bounds.right - self.content_left;

                canvas.save();
                canvas.clip_rect(
                    Rect::from_ltrb(self.gutter_width, 0.0, text_right, bounds.bottom),
                    None,
                    false,
                );
                let mut top = 0.0;
                for row in &self.sticky.rows {
                    canvas.save();
                    canvas.clip_rect(
                        Rect::from_ltrb(self.gutter_width, top, text_right, top + row.height),
                        None,
                        false,
                    );
                    row.shaped.paint(canvas, top);
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
                    for row in &self.sticky.rows {
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
                canvas.restore();

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
            } => match self.sticky.row_at(point.y) {
                Some((row, _)) => EventResult::Command(EditorCommand::RevealAt {
                    byte: row.scope_start,
                }),
                None => EventResult::Handled,
            },

            _ => EventResult::Ignored,
        }
    }
}
