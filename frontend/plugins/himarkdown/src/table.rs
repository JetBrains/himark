// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use himark::{Document, EditorCommand, EditorView, Markup};
use imba::{
    arena::Arena, constraints::Constraints, store::Store, thunk_ext::ThunkExt, UiCtx, View, Widget,
};
use operation::{Op, Operation};
use skia_safe::{
    textlayout::{FontCollection, ParagraphBuilder, ParagraphStyle, TextDirection, TextStyle},
    Canvas, Paint, Rect, Size,
};

use crate::inline_decorations;

type TableChrome = himark::theme::TableChrome;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CellAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone)]
pub(crate) struct CellSource {
    pub content: String,

    pub source: String,

    pub span: std::ops::Range<u32>,
}

#[derive(Clone)]
pub(crate) struct TableSource {
    pub rows: Vec<Vec<CellSource>>,
    pub alignments: Vec<CellAlign>,

    pub lines: Vec<TableLine>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TableLine {
    pub text: String,
    pub span: std::ops::Range<u32>,
}

pub(crate) fn parse_table(raw: &str) -> Option<TableSource> {
    let mut offsets = line_offsets(raw);
    let (header, header_at) = offsets.next()?;
    let (delimiter, delimiter_at) = offsets.next()?;
    if !header.contains('|') {
        return None;
    }

    let mut lines: Vec<TableLine> = Vec::new();

    let alignments: Vec<CellAlign> = split_row(delimiter, 0)?
        .into_iter()
        .map(|cell| alignment_of(&cell.source))
        .collect::<Option<Vec<_>>>()?;
    if alignments.is_empty() {
        return None;
    }

    let columns = alignments.len();
    let mut rows = Vec::new();
    let body = offsets.map(|(line, at)| (line, at, false));
    for (line, at, is_delimiter) in std::iter::once((header, header_at, false))
        .chain(std::iter::once(("", 0, true)))
        .chain(body)
    {
        if is_delimiter || line.trim().is_empty() {
            continue;
        }
        let mut cells = split_row(line, at)?;
        let line_end = at + line.len() as u32;
        cells.resize(
            columns,
            CellSource {
                content: String::new(),
                source: String::new(),
                span: line_end..line_end,
            },
        );
        cells.truncate(columns);
        rows.push(cells);
        lines.push(TableLine {
            text: line.to_owned(),
            span: at..at + line.len() as u32,
        });
    }
    lines.insert(
        1,
        TableLine {
            text: delimiter.to_owned(),
            span: delimiter_at..delimiter_at + delimiter.len() as u32,
        },
    );

    Some(TableSource {
        rows,
        alignments,
        lines,
    })
}

fn line_offsets(raw: &str) -> impl Iterator<Item = (&str, u32)> {
    let mut at = 0u32;
    raw.split('\n').map(move |line| {
        let start = at;
        at += line.len() as u32 + 1;
        (line, start)
    })
}

fn split_row(line: &str, line_at: u32) -> Option<Vec<CellSource>> {
    if !line.contains('|') {
        return None;
    }

    let mut cells: Vec<(usize, usize)> = Vec::new();
    let mut segment_start = 0usize;
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'|' => {
                cells.push((segment_start, i));
                i += 1;
                segment_start = i;
            }
            _ => i += 1,
        }
    }
    cells.push((segment_start, line.len()));

    let mut cells: Vec<CellSource> = cells
        .into_iter()
        .map(|(start, end)| {
            let segment = &line[start..end.min(line.len())];

            let trimmed_start = start + (segment.len() - segment.trim_start().len());
            let trimmed = segment.trim();
            CellSource {
                content: unescape(trimmed),
                source: trimmed.to_owned(),
                span: (line_at + trimmed_start as u32)
                    ..(line_at + (trimmed_start + trimmed.len()) as u32),
            }
        })
        .collect();

    if cells.first().is_some_and(|cell| cell.source.is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|cell| cell.source.is_empty()) {
        cells.pop();
    }
    match cells.is_empty() {
        true => None,
        false => Some(cells),
    }
}

fn alignment_of(cell: &str) -> Option<CellAlign> {
    let left = cell.starts_with(':');
    let right = cell.ends_with(':');
    let dashes = cell.trim_matches(':');
    if dashes.is_empty() || !dashes.chars().all(|c| c == '-') {
        return None;
    }
    Some(match (left, right) {
        (true, true) => CellAlign::Center,
        (false, true) => CellAlign::Right,
        _ => CellAlign::Left,
    })
}

fn unescape(cell: &str) -> String {
    cell.replace("<br/>", "\n")
        .replace("<br>", "\n")
        .replace("\\|", "|")
}

fn escape(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', "<br>")
}

fn source_offset(source: &str, content_offset: u32) -> u32 {
    let mut content_at = 0u32;
    let mut source_at = 0u32;
    let bytes = source.as_bytes();
    while (source_at as usize) < bytes.len() && content_at < content_offset {
        let rest = &source[source_at as usize..];
        let (source_step, content_step) = if rest.starts_with("<br/>") {
            (5u32, 1u32)
        } else if rest.starts_with("<br>") {
            (4, 1)
        } else if rest.starts_with("\\|") {
            (2, 1)
        } else {
            let len = rest.chars().next().map(char::len_utf8).unwrap_or(1) as u32;
            (len, len)
        };
        source_at += source_step;
        content_at += content_step;
    }
    source_at
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ColumnIntrinsics {
    pub min: f32,

    pub max: f32,
}

pub(crate) fn column_widths(
    columns: &[ColumnIntrinsics],
    available: f32,
    chrome: &TableChrome,
) -> Vec<f32> {
    let cap = (available * chrome.column_cap).max(chrome.column_floor);
    let clamp = |width: f32| width.clamp(chrome.column_floor, cap);
    let mins: Vec<f32> = columns.iter().map(|c| clamp(c.min)).collect();
    let maxes: Vec<f32> = columns
        .iter()
        .zip(&mins)
        .map(|(c, min)| clamp(c.max).max(*min))
        .collect();

    let total_min: f32 = mins.iter().sum();
    let total_max: f32 = maxes.iter().sum();

    if total_max <= available {
        let slack = available - total_max;
        return maxes
            .iter()
            .map(|max| max + slack * max / total_max.max(1.0))
            .collect();
    }

    let widths: Vec<f32> = if total_min >= available {
        mins
    } else {
        let squeeze = available - total_min;
        let compressibility: f32 = maxes
            .iter()
            .zip(&mins)
            .map(|(max, min)| max - min)
            .sum::<f32>()
            .max(1.0);
        mins.iter()
            .zip(&maxes)
            .map(|(min, max)| min + squeeze * (max - min) / compressibility)
            .collect()
    };

    widths
        .into_iter()
        .map(|w| (w / chrome.quantum).round().max(1.0) * chrome.quantum)
        .collect()
}

fn cell_view(
    text: &str,
    width: f32,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> EditorView {
    let mut markup = Markup::builder();
    for token in inline_decorations(text) {
        markup.push_inline(
            token.decoration.range.start..token.decoration.range.end,
            token.decoration.id,
        );
        for span in token.syntax {
            markup.push_hidden(span.start as u32..span.end as u32);
        }
    }
    let document = Document::new(text::Text::from_string_exact(text), markup.finish());
    let mut view = EditorView::of_document(document, width.max(1.0), fonts, theme);
    view.blur();
    view
}

fn cell_intrinsics(text: &str, fonts: &FontCollection, theme: &himark::Theme) -> ColumnIntrinsics {
    if text.is_empty() {
        return ColumnIntrinsics { min: 0.0, max: 0.0 };
    }

    let base = theme.base();
    let mut text_style = TextStyle::new();
    if let Some(families) = &base.font_families {
        text_style.set_font_families(families);
    }
    if let Some(size) = base.font_size {
        text_style.set_font_size(size);
    }
    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_direction(TextDirection::LTR);
    paragraph_style.set_text_style(&text_style);
    let mut builder = ParagraphBuilder::new(&paragraph_style, fonts);
    builder.push_style(&text_style);
    builder.add_text(text);
    let mut paragraph = builder.build();
    paragraph.layout(f32::MAX);
    ColumnIntrinsics {
        min: paragraph.min_intrinsic_width().ceil(),
        max: paragraph.max_intrinsic_width().ceil(),
    }
}

pub enum TableCommand {
    Cell {
        row: usize,
        col: usize,
        command: EditorCommand,
    },

    InsertRow(usize),
    RemoveRow(usize),
    InsertColumn(usize),
    RemoveColumn(usize),

    Relayout {
        width: f32,
    },

    Relaid(Box<TableEditor>),
}

#[derive(Clone)]
struct Cell {
    view: EditorView,

    content: String,

    source: String,

    span: std::ops::Range<u32>,
}

pub struct TableEditor {
    rows: Vec<Vec<Cell>>,

    lines: Vec<TableLine>,
    alignments_source: Vec<CellAlign>,
    widths: Vec<f32>,
    row_heights: Vec<f32>,

    intrinsics: Vec<ColumnIntrinsics>,

    available: AtomicU32,
    #[allow(dead_code)]
    alignments: Vec<CellAlign>,
    focused: Option<(usize, usize)>,

    range: std::ops::Range<u32>,

    pending_edit: Option<Operation>,

    painted_focused: AtomicBool,

    version: u64,

    relayout_token: Option<imba::effect::CancellationToken>,
    relayout_inflight: Option<f32>,
    chrome: TableChrome,
}

fn fresh_version() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Clone for TableEditor {
    fn clone(&self) -> Self {
        Self {
            lines: self.lines.clone(),
            alignments_source: self.alignments_source.clone(),
            rows: self.rows.clone(),
            widths: self.widths.clone(),
            row_heights: self.row_heights.clone(),
            intrinsics: self.intrinsics.clone(),
            available: AtomicU32::new(self.available.load(Ordering::Relaxed)),
            alignments: self.alignments.clone(),
            focused: self.focused,
            range: self.range.clone(),
            pending_edit: self.pending_edit.clone(),
            painted_focused: AtomicBool::new(self.painted_focused.load(Ordering::Relaxed)),
            version: self.version,
            relayout_token: self.relayout_token,
            relayout_inflight: self.relayout_inflight,
            chrome: self.chrome.clone(),
        }
    }
}

impl TableEditor {
    pub(crate) fn new(
        source: TableSource,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) -> Self {
        let columns = source.alignments.len();
        let chrome = theme.ui().table.clone();
        let intrinsics = Self::intrinsics_from(
            source
                .rows
                .iter()
                .map(|row| row.iter().map(|cell| cell.content.as_str())),
            columns,
            &fonts,
            theme,
            &chrome,
        );
        let widths = Self::natural_widths(&intrinsics, &chrome);

        let mut rows = Vec::with_capacity(source.rows.len());
        for source_row in source.rows {
            let row: Vec<Cell> = source_row
                .into_iter()
                .enumerate()
                .map(|(col, cell)| Cell {
                    view: cell_view(
                        &cell.content,
                        widths[col] - chrome.cell_pad_x * 2.0,
                        fonts,
                        theme,
                    ),
                    content: cell.content,
                    source: cell.source,
                    span: cell.span,
                })
                .collect();
            rows.push(row);
        }
        let row_heights = Self::heights_for(&rows, &chrome);

        Self {
            lines: source.lines.clone(),
            alignments_source: source.alignments.clone(),
            rows,
            widths,
            row_heights,
            intrinsics,
            available: AtomicU32::new(chrome.fallback_width.to_bits()),
            alignments: source.alignments,
            focused: None,
            range: 0..0,
            pending_edit: None,
            painted_focused: AtomicBool::new(false),
            version: fresh_version(),
            relayout_token: None,
            relayout_inflight: None,
            chrome,
        }
    }

    fn intrinsics_from<'c>(
        rows: impl Iterator<Item = impl Iterator<Item = &'c str>>,
        columns: usize,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
        chrome: &TableChrome,
    ) -> Vec<ColumnIntrinsics> {
        let collection = fonts.clone();
        let mut intrinsics = vec![ColumnIntrinsics { min: 0.0, max: 0.0 }; columns];
        for row in rows {
            for (col, content) in row.enumerate().take(columns) {
                let cell = cell_intrinsics(content, &collection, theme);
                intrinsics[col].min = intrinsics[col].min.max(cell.min);
                intrinsics[col].max = intrinsics[col].max.max(cell.max);
            }
        }
        for column in &mut intrinsics {
            column.min += chrome.cell_pad_x * 2.0;
            column.max += chrome.cell_pad_x * 2.0;
        }
        intrinsics
    }

    fn natural_widths(intrinsics: &[ColumnIntrinsics], chrome: &TableChrome) -> Vec<f32> {
        intrinsics
            .iter()
            .map(|c| c.max.max(chrome.column_floor))
            .collect()
    }

    fn available(&self) -> f32 {
        f32::from_bits(self.available.load(Ordering::Relaxed))
    }

    fn relay_all(
        &mut self,
        available: f32,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) {
        self.available.store(available.to_bits(), Ordering::Relaxed);
        let lay = self.lay_widths(available);
        for col in 0..self.widths.len().min(lay.len()) {
            if (lay[col] - self.widths[col]).abs() < 0.5 {
                continue;
            }
            for (row_index, row) in self.rows.iter_mut().enumerate() {
                let Some(cell) = row.get_mut(col) else {
                    continue;
                };
                let caret = cell.view.caret_byte();
                let focused = self.focused == Some((row_index, col));
                cell.view = cell_view(
                    &cell.content,
                    (lay[col] - self.chrome.cell_pad_x * 2.0).max(1.0),
                    fonts,
                    theme,
                );
                if focused {
                    cell.view.focus_text();
                    cell.view.set_caret(caret);
                }
            }
        }
        self.widths = lay;
        self.row_heights = Self::heights_for(&self.rows, &self.chrome);
    }

    fn lay_widths(&self, available: f32) -> Vec<f32> {
        Self::widths_at(&self.intrinsics, available, &self.chrome)
            .into_iter()
            .zip(&self.intrinsics)
            .map(|(width, column)| width.min(column.max.max(self.chrome.column_floor)))
            .collect()
    }

    fn needs_relay(&self, available: f32) -> bool {
        let lay = self.lay_widths(available);
        lay.len() != self.widths.len()
            || lay
                .iter()
                .zip(&self.widths)
                .any(|(fresh, laid)| (fresh - laid).abs() >= 0.5)
    }

    fn widths_at(
        intrinsics: &[ColumnIntrinsics],
        available: f32,
        chrome: &TableChrome,
    ) -> Vec<f32> {
        let rules = chrome.thickness * (intrinsics.len() as f32 + 1.0);
        column_widths(intrinsics, available - rules, chrome)
    }

    fn heights_for(rows: &[Vec<Cell>], chrome: &TableChrome) -> Vec<f32> {
        rows.iter()
            .map(|row| {
                (row.iter()
                    .map(|cell| cell.view.content_height())
                    .fold(0.0f32, f32::max)
                    + chrome.cell_pad_y * 2.0)
                    .max(chrome.row_floor)
            })
            .collect()
    }

    fn write_through_insert(&mut self, row: usize, col: usize, text: &str) {
        let cell = &self.rows[row][col];
        let caret = cell.view.caret_byte().min(cell.content.len() as u32);
        let in_source = source_offset(&cell.source, caret);
        let escaped = escape(text);
        let at = self.range.start + cell.span.start + in_source;
        self.queue_edit(Operation::insert_at(at, escaped.clone()));

        self.version = fresh_version();
        let cell = &mut self.rows[row][col];
        cell.content.insert_str(caret as usize, text);
        cell.source.insert_str(in_source as usize, &escaped);
        let grew = escaped.len() as u32;
        let edited_at = cell.span.start + in_source;
        cell.span.end += grew;
        self.patch_lines(edited_at, 0, &escaped);
        self.shift_spans_after(edited_at, grew as i64, (row, col));
    }

    fn write_through_backspace(&mut self, row: usize, col: usize) {
        let cell = &self.rows[row][col];
        let caret = cell.view.caret_byte().min(cell.content.len() as u32);
        let Some(previous) = cell.content[..caret as usize].chars().next_back() else {
            return;
        };
        let content_start = caret - previous.len_utf8() as u32;
        let source_start = source_offset(&cell.source, content_start);
        let source_end = source_offset(&cell.source, caret);
        let deleted = cell.source[source_start as usize..source_end as usize].to_owned();
        let at = self.range.start + cell.span.start + source_start;
        self.queue_edit(Operation::delete_at(at, deleted));

        self.version = fresh_version();
        let cell = &mut self.rows[row][col];
        cell.content
            .replace_range(content_start as usize..caret as usize, "");
        cell.source
            .replace_range(source_start as usize..source_end as usize, "");
        let shrank = (source_end - source_start) as i64;
        let edited_at = cell.span.start + source_start;
        cell.span.end -= shrank as u32;
        self.patch_lines(edited_at, shrank as usize, "");
        self.shift_spans_after(edited_at, -shrank, (row, col));
    }

    fn queue_edit(&mut self, operation: Operation) {
        self.pending_edit = Some(match self.pending_edit.take() {
            Some(pending) => pending.compose(&operation),
            None => operation,
        });
    }

    fn place_controls<'a>(
        &'a self,
        container: &mut imba::container::Container<'a, TableCommand>,
        strip: f32,
        widths: &[f32],
    ) {
        let size = self.chrome.control_size;
        let rule = self.chrome.thickness;
        let glyph = self.chrome.control_glyph.0;
        let remove = self.chrome.control_remove.0;
        let fill = self.chrome.control_fill.0;

        let button = |plus: bool, color: skia_safe::Color, command: TableCommand| {
            imba::eager(ControlButton {
                size,
                plus,
                color,
                fill,
                command: std::cell::Cell::new(Some(command)),
                armed: &self.painted_focused,
            })
        };

        let mut x = strip + rule;
        for (index, width) in widths.iter().enumerate() {
            container.place(
                x - rule * 0.5 - size * 0.5,
                0.0,
                button(true, glyph, TableCommand::InsertColumn(index)),
            );
            if widths.len() > 1 {
                container.place(
                    x + width * 0.5 - size * 0.5,
                    0.0,
                    button(false, remove, TableCommand::RemoveColumn(index)),
                );
            }
            x += width + rule;
        }
        container.place(
            x - rule * 0.5 - size * 0.5,
            0.0,
            button(true, glyph, TableCommand::InsertColumn(widths.len())),
        );

        let mut y = strip + rule + self.row_heights.first().copied().unwrap_or(0.0) + rule;
        for row in 1..self.rows.len() {
            container.place(
                0.0,
                y - rule * 0.5 - size * 0.5,
                button(true, glyph, TableCommand::InsertRow(row)),
            );
            let height = self.row_heights[row];
            if self.rows.len() > 2 {
                container.place(
                    0.0,
                    y + height * 0.5 - size * 0.5,
                    button(false, remove, TableCommand::RemoveRow(row)),
                );
            }
            y += height + rule;
        }
        container.place(
            0.0,
            y - rule * 0.5 - size * 0.5,
            button(true, glyph, TableCommand::InsertRow(self.rows.len())),
        );
    }

    fn rebuild_from_lines(
        &mut self,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) {
        self.version = fresh_version();
        let source: String = self
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let Some(parsed) = parse_table(&source) else {
            return;
        };
        let mut fresh = Self::new(parsed, fonts, theme);
        fresh.range = self.range.clone();
        fresh.pending_edit = self.pending_edit.take();

        fresh.relayout_token = self.relayout_token;
        fresh.painted_focused.store(
            self.painted_focused.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        let available = self.available();
        fresh.relay_all(available, fonts, theme);
        *self = fresh;
    }

    fn delimiter_line(alignments: &[CellAlign]) -> String {
        let parts: Vec<&str> = alignments
            .iter()
            .map(|alignment| match alignment {
                CellAlign::Left => "---",
                CellAlign::Center => ":-:",
                CellAlign::Right => "--:",
            })
            .collect();
        format!("| {} |", parts.join(" | "))
    }

    fn insert_row(
        &mut self,
        at: usize,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) {
        let at = at.clamp(1, self.rows.len());
        let cols = self.alignments_source.len().max(1);
        let new_line = format!("|{}", "   |".repeat(cols));
        let line_index = at + 1;
        if line_index < self.lines.len() {
            let pos = self.lines[line_index].span.start;
            self.queue_edit(Operation::insert_at(
                self.range.start + pos,
                format!("{new_line}\n"),
            ));
            self.lines.insert(
                line_index,
                TableLine {
                    text: new_line,
                    span: pos..pos,
                },
            );
        } else {
            let pos = self.lines.last().map(|line| line.span.end).unwrap_or(0);
            self.queue_edit(Operation::insert_at(
                self.range.start + pos,
                format!("\n{new_line}"),
            ));
            self.lines.push(TableLine {
                text: new_line,
                span: pos..pos,
            });
        }
        self.rebuild_from_lines(fonts, theme);
    }

    fn remove_row(
        &mut self,
        at: usize,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) {
        if at == 0 || at >= self.rows.len() || self.rows.len() < 2 {
            return;
        }
        let line_index = at + 1;
        let line = self.lines[line_index].clone();
        if line_index + 1 < self.lines.len() {
            self.queue_edit(Operation::delete_at(
                self.range.start + line.span.start,
                format!("{}\n", line.text),
            ));
        } else {
            self.queue_edit(Operation::delete_at(
                self.range.start + line.span.start - 1,
                format!("\n{}", line.text),
            ));
        }
        self.lines.remove(line_index);
        self.rebuild_from_lines(fonts, theme);
    }

    fn insert_column(
        &mut self,
        at: usize,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) {
        let cols = self.alignments_source.len();
        let at = at.min(cols);
        let mut alignments = self.alignments_source.clone();
        alignments.insert(at, CellAlign::Left);
        let mut edits: Vec<(u32, String, String)> = Vec::new();
        for (row_index, row) in self.rows.iter().enumerate() {
            let (pos, text) = if at < cols {
                (row[at].span.start, " | ".to_owned())
            } else {
                (row[cols - 1].span.end, " |".to_owned())
            };
            let _ = row_index;
            edits.push((pos, String::new(), text));
        }
        let delimiter = self.lines[1].clone();
        edits.push((
            delimiter.span.start,
            delimiter.text.clone(),
            Self::delimiter_line(&alignments),
        ));
        self.apply_structural(edits, fonts, theme);
    }

    fn remove_column(
        &mut self,
        at: usize,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) {
        let cols = self.alignments_source.len();
        if at >= cols || cols < 2 {
            return;
        }
        let mut alignments = self.alignments_source.clone();
        alignments.remove(at);
        let mut edits: Vec<(u32, String, String)> = Vec::new();
        for row in &self.rows {
            let span = match at {
                0 => row[0].span.start..row[1].span.start,
                _ => row[at - 1].span.end..row[at].span.end,
            };
            let line = self
                .lines
                .iter()
                .find(|line| line.span.start <= span.start && span.end <= line.span.end)
                .expect("a cell's line is retained");
            let local =
                (span.start - line.span.start) as usize..(span.end - line.span.start) as usize;
            edits.push((span.start, line.text[local].to_owned(), String::new()));
        }
        let delimiter = self.lines[1].clone();
        edits.push((
            delimiter.span.start,
            delimiter.text.clone(),
            Self::delimiter_line(&alignments),
        ));
        self.apply_structural(edits, fonts, theme);
    }

    fn apply_structural(
        &mut self,
        mut edits: Vec<(u32, String, String)>,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) {
        edits.sort_by_key(|(pos, _, _)| *pos);
        let mut ops = Vec::new();
        let mut retained = 0u32;
        for (pos, delete, insert) in &edits {
            let absolute = self.range.start + pos;
            if absolute > retained {
                ops.push(Op::Retain(absolute - retained));
                retained = absolute;
            }
            if !delete.is_empty() {
                ops.push(Op::Delete(delete.clone()));
                retained += delete.len() as u32;
            }
            if !insert.is_empty() {
                ops.push(Op::Insert(insert.clone()));
            }
        }
        self.queue_edit(Operation::from_ops(ops));

        for (pos, delete, insert) in edits.iter().rev() {
            for line in &mut self.lines {
                if line.span.start <= *pos && *pos <= line.span.end {
                    let local = (*pos - line.span.start) as usize;
                    line.text.replace_range(local..local + delete.len(), insert);
                    break;
                }
            }
        }
        self.rebuild_from_lines(fonts, theme);
    }

    fn patch_lines(&mut self, at: u32, delete: usize, insert: &str) {
        let delta = insert.len() as i64 - delete as i64;
        for line in &mut self.lines {
            if line.span.start <= at && at <= line.span.end {
                let local = (at - line.span.start) as usize;
                line.text.replace_range(local..local + delete, insert);
                line.span.end = (line.span.end as i64 + delta) as u32;
            } else if line.span.start > at {
                line.span.start = (line.span.start as i64 + delta) as u32;
                line.span.end = (line.span.end as i64 + delta) as u32;
            }
        }
    }

    fn shift_spans_after(&mut self, edited_at: u32, delta: i64, edited: (usize, usize)) {
        for (row_index, row) in self.rows.iter_mut().enumerate() {
            for (col_index, cell) in row.iter_mut().enumerate() {
                if (row_index, col_index) == edited || cell.span.start < edited_at {
                    continue;
                }
                cell.span.start = (cell.span.start as i64 + delta) as u32;
                cell.span.end = (cell.span.end as i64 + delta) as u32;
            }
        }
    }

    fn relayout_after_edit(
        &mut self,
        _row: usize,
        _col: usize,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) {
        let columns = self.widths.len();
        self.intrinsics = Self::intrinsics_from(
            self.rows
                .iter()
                .map(|row| row.iter().map(|cell| cell.content.as_str())),
            columns,
            fonts,
            theme,
            &self.chrome,
        );
        self.relay_all(self.available(), fonts, theme);
    }

    fn launch_relayout(&mut self, width: f32, fx: &mut imba::effect::Effects<'_, TableCommand>) {
        let already_inflight = self
            .relayout_inflight
            .is_some_and(|inflight| (inflight - width).abs() < 0.5);
        if !already_inflight {
            self.relayout_inflight = Some(width);
            let effect = TableRelayoutEffect {
                table: self.clone(),
                width,
            };
            fx.relaunch(&mut self.relayout_token, effect);
        }
    }

    fn land_relaid(&mut self, relaid: TableEditor) {
        self.relayout_inflight = None;
        if relaid.version == self.version {
            let mut fresh = relaid;
            fresh.range = self.range.clone();
            fresh.pending_edit = self.pending_edit.take();
            fresh.relayout_token = self.relayout_token;
            fresh.relayout_inflight = None;
            fresh.painted_focused.store(
                self.painted_focused.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );

            if fresh.focused != self.focused {
                if let Some((row, col)) = fresh.focused {
                    if let Some(cell) = fresh.rows.get_mut(row).and_then(|cells| cells.get_mut(col))
                    {
                        cell.view.blur();
                    }
                }
                fresh.focused = self.focused;
            }
            if let Some((row, col)) = self.focused {
                if let (Some(cell), Some(live)) = (
                    fresh.rows.get_mut(row).and_then(|cells| cells.get_mut(col)),
                    self.rows.get(row).and_then(|cells| cells.get(col)),
                ) {
                    cell.view.focus_text();
                    cell.view.set_caret(live.view.caret_byte());
                }
            }
            *self = fresh;
        }
    }

    fn perform_cell(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        row: usize,
        col: usize,
        command: EditorCommand,
    ) {
        if row >= self.rows.len() || col >= self.rows[row].len() {
            return;
        }

        if self.focused != Some((row, col)) {
            if let Some((prev_row, prev_col)) = self.focused {
                if let Some(prev) = self
                    .rows
                    .get_mut(prev_row)
                    .and_then(|cells| cells.get_mut(prev_col))
                {
                    prev.view.blur();
                }
            }
            self.rows[row][col].view.focus_text();
            self.focused = Some((row, col));
        }

        let command = match command {
            EditorCommand::Enter { .. } => EditorCommand::InsertText {
                text: "\n".to_owned(),
            },
            command => command,
        };

        let applies = match &command {
            EditorCommand::InsertText { text } | EditorCommand::Paste { text } => {
                self.write_through_insert(row, col, text);
                true
            }
            EditorCommand::Backspace => {
                self.write_through_backspace(row, col);
                true
            }
            EditorCommand::DeleteForward
            | EditorCommand::DeleteWordBack
            | EditorCommand::DeleteWordForward
            | EditorCommand::DeleteSelections
            | EditorCommand::Indent
            | EditorCommand::Outdent
            | EditorCommand::Undo
            | EditorCommand::Redo => false,

            _ => true,
        };
        if applies {
            let cell = &mut self.rows[row][col];
            let mut discarded = imba::effect::Batch::new();
            imba::View::perform(&mut cell.view, store, ui, command, &mut discarded.effects());
            let fonts = himark::env::ui_collection(store, ui);
            let theme = himark::env::Themes::of(store);
            self.relayout_after_edit(row, col, &fonts, &theme);
        }
    }

    fn table_size(&self, widths: &[f32]) -> Size {
        let rule = self.chrome.thickness;
        let width: f32 = widths.iter().sum::<f32>() + rule * (widths.len() as f32 + 1.0);
        let height: f32 =
            self.row_heights.iter().sum::<f32>() + rule * (self.row_heights.len() as f32 + 1.0);
        Size::new(width, height)
    }

    fn paint_grid(&self, canvas: &Canvas, rect: Rect, widths: &[f32]) {
        let rule = self.chrome.thickness;
        let mut paint = Paint::default();
        paint.set_anti_alias(true);

        paint.set_color(self.chrome.grid.0);
        canvas.draw_rect(
            Rect::from_xywh(0.0, 0.0, rect.width(), self.row_heights[0] + rule * 2.0),
            &paint,
        );

        paint.set_color(self.chrome.grid_strong.0);
        let mut y = 0.0;
        for height in std::iter::once(&0.0).chain(&self.row_heights) {
            y += height
                + if y == 0.0 && *height == 0.0 {
                    0.0
                } else {
                    rule
                };
            canvas.draw_rect(Rect::from_xywh(0.0, y - rule, rect.width(), rule), &paint);
        }
        let mut x = 0.0;
        for width in std::iter::once(&0.0).chain(widths) {
            x += width + if x == 0.0 && *width == 0.0 { 0.0 } else { rule };
            canvas.draw_rect(Rect::from_xywh(x - rule, 0.0, rule, rect.height()), &paint);
        }
    }
}

pub struct TableRelayoutEffect {
    table: TableEditor,
    width: f32,
}

impl imba::effect::Effect for TableRelayoutEffect {
    type Result = TableCommand;
}

pub struct TableRelayoutHandler(pub std::sync::Arc<himark::Workshop>);

impl imba::effect::EffectHandler<TableRelayoutEffect> for TableRelayoutHandler {
    async fn handle(&self, effect: TableRelayoutEffect) -> TableCommand {
        let mut table = effect.table;
        table.relay_all(effect.width, &self.0.fonts(), &self.0.theme());
        TableCommand::Relaid(Box::new(table))
    }
}

impl himark::InlayEditing for TableEditor {
    fn take_edit(&mut self) -> Option<Operation> {
        self.pending_edit.take()
    }

    fn set_range(&mut self, range: std::ops::Range<u32>) {
        self.range = range;
    }

    fn passive(&self, command: &himark::InlayCommand) -> bool {
        matches!(
            command.downcast_ref::<TableCommand>(),
            Some(TableCommand::Relayout { .. } | TableCommand::Relaid(_))
        )
    }

    fn adopt_from(
        &mut self,
        previous: &Self,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) -> bool {
        if let Some((row, col)) = previous.focused {
            if let Some(cell) = self.rows.get_mut(row).and_then(|cells| cells.get_mut(col)) {
                cell.view
                    .set_caret(previous.rows[row][col].view.caret_byte());
                self.focused = Some((row, col));
            }
        }
        self.painted_focused.store(
            previous.painted_focused.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );

        self.relay_all(previous.available(), fonts, theme);
        self.lines == previous.lines
    }
}

impl View for TableEditor {
    type Command = TableCommand;
    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            TableCommand::Cell { row, col, command } => {
                self.perform_cell(store, ui, row, col, command)
            }
            TableCommand::Relayout { width } => self.launch_relayout(width, fx),
            TableCommand::Relaid(relaid) => self.land_relaid(*relaid),
            structural => {
                let fonts = himark::env::ui_collection(store, ui);
                let theme = himark::env::Themes::of(store);
                match structural {
                    TableCommand::InsertRow(at) => self.insert_row(at, &fonts, &theme),
                    TableCommand::RemoveRow(at) => self.remove_row(at, &fonts, &theme),
                    TableCommand::InsertColumn(at) => self.insert_column(at, &fonts, &theme),
                    TableCommand::RemoveColumn(at) => self.remove_column(at, &fonts, &theme),
                    TableCommand::Cell { .. }
                    | TableCommand::Relayout { .. }
                    | TableCommand::Relaid(_) => unreachable!(),
                }
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let strip = self.chrome.control_size + 6.0;

            let trailing = self.chrome.control_size * 0.5 + 2.0;

            let available = (constraints.max.width - strip - trailing).max(60.0);
            self.available.store(available.to_bits(), Ordering::Relaxed);
            let widths = Self::widths_at(&self.intrinsics, available, &self.chrome);
            let table = self.table_size(&widths);
            let size = Size::new(
                table.width + strip + trailing,
                table.height + strip + trailing,
            );
            let mut container = imba::container::container(arena, size);
            self.place_controls(&mut container, strip, &widths);

            let mut y = strip + self.chrome.thickness;
            for (row_index, (row, height)) in self.rows.iter().zip(&self.row_heights).enumerate() {
                let mut x = strip + self.chrome.thickness;
                for (col_index, (cell, width)) in row.iter().zip(&widths).enumerate() {
                    let cell_width = cell.view.layout_width().max(1.0);
                    let inner_width = (width - self.chrome.cell_pad_x * 2.0).max(1.0);
                    let inner_height = (height - self.chrome.cell_pad_y * 2.0).max(1.0);
                    let widget = imba::Layout::layout(
                        cell.view.display(arena, store, ui),
                        arena,
                        Constraints {
                            min: Size::new(inner_width.max(cell_width), inner_height),
                            max: Size::new(cell_width, f32::MAX),
                        },
                    )
                    .map(move |command| TableCommand::Cell {
                        row: row_index,
                        col: col_index,
                        command,
                    });
                    container.place(
                        x + self.chrome.cell_pad_x,
                        y + self.chrome.cell_pad_y,
                        widget,
                    );
                    x += width + self.chrome.thickness;
                }
                y += height + self.chrome.thickness;
            }

            let relayout = self.needs_relay(available).then_some(available);
            let painted = &self.painted_focused;
            container
                .paint_below(move |_arena, canvas, _rect| {
                    canvas.save();
                    canvas.translate((strip, strip));
                    self.paint_grid(canvas, Rect::from_size(table), &widths);
                    canvas.restore();
                })
                .wrap(move |inner| PaintedFocus {
                    painted,
                    relayout,
                    inner,
                })
                .commands(move || {
                    let Some((row, col)) = self.focused else {
                        return Vec::new();
                    };
                    let rows = self.rows.len();
                    let cols = self.rows.first().map_or(0, Vec::len);
                    let mut commands = vec![
                        imba::PresentableCommand::new(
                            "table.insert-row-below",
                            "Table: Insert Row Below",
                            TableCommand::InsertRow((row + 1).max(1)),
                        ),
                        imba::PresentableCommand::new(
                            "table.insert-column-left",
                            "Table: Insert Column Left",
                            TableCommand::InsertColumn(col),
                        ),
                        imba::PresentableCommand::new(
                            "table.insert-column-right",
                            "Table: Insert Column Right",
                            TableCommand::InsertColumn(col + 1),
                        ),
                    ];
                    if row >= 1 {
                        commands.push(imba::PresentableCommand::new(
                            "table.insert-row-above",
                            "Table: Insert Row Above",
                            TableCommand::InsertRow(row),
                        ));
                        if rows > 2 {
                            commands.push(imba::PresentableCommand::new(
                                "table.remove-row",
                                "Table: Remove Row",
                                TableCommand::RemoveRow(row),
                            ));
                        }
                    }
                    if cols > 1 {
                        commands.push(imba::PresentableCommand::new(
                            "table.remove-column",
                            "Table: Remove Column",
                            TableCommand::RemoveColumn(col),
                        ));
                    }
                    commands
                })
        })
    }
}

pub struct InsertTable;

const INSERT_TABLE_TEMPLATE: &str = "|   |   |\n| --- | --- |\n|   |   |";

impl himark::DynamicEditorCommand for InsertTable {
    fn id(&self) -> &'static str {
        "table.insert"
    }

    fn name(&self) -> String {
        "Table: Insert".to_owned()
    }

    fn offers_at(&self, _location: &himark::ResourceLocation) -> bool {
        true
    }

    fn perform(
        &self,
        store: &mut Store,
        document: &mut Document,
        editor: himark::EditorId,
        _location: &himark::ResourceLocation,
        _payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut himark::EditorEffects<'_>,
    ) {
        if document
            .syntax()
            .is_none_or(|syntax| syntax.language != "markdown")
        {
            return;
        }
        let selection = document.carets(editor).primary().selection();
        let text = document.text();

        let newlines_before = {
            let mut count = 0usize;
            let mut at = selection.start;
            while count < 2 {
                match himark::text_cursor::previous_char_before(text, at) {
                    Some((start, ch)) if ch == "\n" => {
                        count += 1;
                        at = start;
                    }

                    None => count = 2,
                    Some(_) => break,
                }
            }
            count
        };
        let newlines_after = {
            let mut count = 0usize;
            let mut at = selection.end;
            while count < 2 {
                match himark::text_cursor::next_char_after(text, at) {
                    Some((end, ch)) if ch == "\n" => {
                        count += 1;
                        at = end;
                    }

                    None => count = 2,
                    Some(_) => break,
                }
            }
            count
        };
        let snippet = format!(
            "{}{}{}",
            "\n".repeat(2 - newlines_before),
            INSERT_TABLE_TEMPLATE,
            "\n".repeat(2 - newlines_after),
        );
        let fonts = himark::env::Fonts::of(store)();
        let theme = himark::env::Themes::of(store);
        document.insert(editor, &snippet, &fonts, &theme, fx);
    }
}

struct PaintedFocus<'a, Inner> {
    painted: &'a AtomicBool,
    relayout: Option<f32>,
    inner: Inner,
}

impl<'a, Inner> Widget<'a, TableCommand> for PaintedFocus<'a, Inner>
where
    Inner: Widget<'a, TableCommand>,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, TableCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &imba::event::Event<'_>,
        viewport: Rect,
    ) -> imba::event::EventResult<TableCommand> {
        if let imba::event::Event::Paint { focused, .. } = event {
            self.painted.store(*focused, Ordering::Relaxed);
            let result = self.inner.handle_event(arena, event, viewport);
            return match self.relayout {
                Some(width) => {
                    result.merge(imba::event::EventResult::Command(TableCommand::Relayout {
                        width,
                    }))
                }
                None => result,
            };
        }
        self.inner.handle_event(arena, event, viewport)
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, TableCommand>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}

struct ControlButton<'a> {
    size: f32,
    plus: bool,
    color: skia_safe::Color,
    fill: skia_safe::Color,
    command: std::cell::Cell<Option<TableCommand>>,
    armed: &'a AtomicBool,
}

impl<'a> Widget<'a, TableCommand> for ControlButton<'a> {
    fn size(&self) -> Size {
        Size::new(self.size, self.size)
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &imba::event::Event<'_>,
        _viewport: Rect,
    ) -> imba::event::EventResult<TableCommand> {
        match event {
            imba::event::Event::Paint {
                canvas,
                focused: true,
            } => {
                let size = self.size;
                let mut paint = skia_safe::Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(self.fill);
                let center = (size * 0.5, size * 0.5);
                canvas.draw_circle(center, size * 0.5, &paint);
                paint.set_color(self.color);
                paint.set_style(skia_safe::PaintStyle::Stroke);
                paint.set_stroke_width(2.0);
                paint.set_stroke_cap(skia_safe::PaintCap::Round);
                let arm = size * 0.22;
                canvas.draw_line(
                    (center.0 - arm, center.1),
                    (center.0 + arm, center.1),
                    &paint,
                );
                if self.plus {
                    canvas.draw_line(
                        (center.0, center.1 - arm),
                        (center.0, center.1 + arm),
                        &paint,
                    );
                }
                imba::event::EventResult::Handled
            }
            imba::event::Event::MouseDown { .. } if self.armed.load(Ordering::Relaxed) => {
                match self.command.take() {
                    Some(command) => imba::event::EventResult::Command(command),
                    None => imba::event::EventResult::Handled,
                }
            }
            _ => imba::event::EventResult::Ignored,
        }
    }
}

#[cfg(test)]
impl TableEditor {
    pub(crate) fn cell_content(&self, row: usize, col: usize) -> &str {
        &self.rows[row][col].content
    }

    pub(crate) fn laid_widths(&self) -> &[f32] {
        &self.widths
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod hitbox {
    use super::*;
    use imba::event::{Event, EventResult, MouseButton};

    #[test]
    fn clicking_anywhere_in_an_empty_cell_reaches_it() {
        use himark::InlayEditing;
        let fonts = himark::embedded_fonts::collection();
        let theme = himark::Theme::embedded();
        let source = "| alpha | beta gamma |\n| --- | --- |\n| one | two |";
        let mut editor = TableEditor::new(parse_table(source).expect("a table"), &fonts, &theme);
        editor.set_range(0..source.len() as u32);
        editor.relay_all(600.0, &fonts, &theme);
        editor.insert_row(2, &fonts, &theme);

        let arena = Arena::default();
        let store = Store::new();
        let ui = UiCtx::cold();
        let constraints = Constraints {
            min: Size::default(),
            max: Size::new(600.0, f32::MAX),
        };

        let _ = imba::Layout::layout(
            imba::View::display(&editor, &arena, &store, &ui),
            &arena,
            constraints,
        );
        let widths = solved_widths(&editor);
        let chrome = &editor.chrome;
        let strip = chrome.control_size + 6.0;

        let row_top =
            strip + chrome.thickness * 3.0 + editor.row_heights[0] + editor.row_heights[1];
        let y = row_top + editor.row_heights[2] * 0.5;

        let mut x_left = strip + chrome.thickness;
        for (col, width) in widths.iter().enumerate() {
            let point = skia_safe::Point::new(x_left + width - chrome.cell_pad_x - 2.0, y);
            let widget = imba::Layout::layout(
                imba::View::display(&editor, &arena, &store, &ui),
                &arena,
                constraints,
            );
            let viewport = Rect::from_size(imba::Thunk::size(&widget));
            let widget = imba::Thunk::realize(widget, &arena, viewport);
            let result = imba::Widget::handle_event(
                &widget,
                &arena,
                &Event::MouseDown {
                    mods: Default::default(),
                    point,
                    button: MouseButton::Left,
                    count: 1,
                },
                viewport,
            );
            match result {
                EventResult::Command(TableCommand::Cell { row, col: hit, .. }) => {
                    assert_eq!((row, hit), (2, col), "the click lands in the empty cell");
                }
                _ => panic!("the click at {point:?} missed the empty cell in column {col}"),
            }
            x_left += width + chrome.thickness;
        }
    }

    fn solved_widths(editor: &TableEditor) -> Vec<f32> {
        TableEditor::widths_at(&editor.intrinsics, editor.available(), &editor.chrome)
    }

    #[test]
    fn controls_arm_only_after_a_focused_paint() {
        use himark::InlayEditing;
        use imba::event::{Event, EventResult, MouseButton};

        let fonts = himark::embedded_fonts::collection();
        let theme = himark::Theme::embedded();
        let source = "| a | b |\n| --- | --- |\n| 1 | 2 |";
        let mut editor = TableEditor::new(parse_table(source).expect("a table"), &fonts, &theme);
        editor.set_range(0..source.len() as u32);
        editor.relay_all(600.0, &fonts, &theme);

        let arena = Arena::default();
        let store = Store::new();
        let ui = UiCtx::cold();
        let constraints = Constraints {
            min: Size::default(),
            max: Size::new(600.0, f32::MAX),
        };
        let unfocused_size = imba::Thunk::size(&imba::Layout::layout(
            imba::View::display(&editor, &arena, &store, &ui),
            &arena,
            constraints,
        ));

        let strip = editor.chrome.control_size + 6.0;
        let click = Event::MouseDown {
            mods: Default::default(),
            point: skia_safe::Point::new(
                strip + editor.chrome.thickness * 0.5,
                editor.chrome.control_size * 0.5,
            ),
            button: MouseButton::Left,
            count: 1,
        };
        let send = |editor: &TableEditor, event: &Event<'_>| {
            let widget = imba::Layout::layout(
                imba::View::display(editor, &arena, &store, &ui),
                &arena,
                constraints,
            );
            let viewport = Rect::from_size(imba::Thunk::size(&widget));
            let widget = imba::Thunk::realize(widget, &arena, viewport);
            match imba::Widget::handle_event(&widget, &arena, event, viewport) {
                EventResult::Command(command) => vec![command],
                EventResult::Commands(commands) => commands,
                _ => Vec::new(),
            }
        };

        assert!(
            send(&editor, &click).is_empty(),
            "unpainted controls are inert"
        );

        let mut surface = skia_safe::surfaces::raster_n32_premul((640, 480)).expect("a surface");
        let _ = send(
            &editor,
            &Event::Paint {
                canvas: surface.canvas(),
                focused: true,
            },
        );
        assert!(
            send(&editor, &click)
                .iter()
                .any(|command| matches!(command, TableCommand::InsertColumn(0))),
            "a click after a focused paint inserts the column"
        );

        let _ = send(
            &editor,
            &Event::Paint {
                canvas: surface.canvas(),
                focused: false,
            },
        );
        assert!(
            send(&editor, &click).is_empty(),
            "invisible controls are inert"
        );

        editor.focused = Some((1, 0));
        assert_eq!(
            imba::Thunk::size(&imba::Layout::layout(
                imba::View::display(&editor, &arena, &store, &ui),
                &arena,
                constraints
            )),
            unfocused_size,
            "focus never changes the table's size"
        );
    }
}
