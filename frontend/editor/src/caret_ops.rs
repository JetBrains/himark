use std::ops::Range;

use operation::{Operation, OperationBuilder};
use skia_safe::textlayout::FontCollection;
use text::Text;

use crate::caret::{Caret, DragOrigin, MultiCaret};
use crate::document::Document;
use crate::editor::{EditorEffects, EditorId};
use crate::editor_view::{ClickKind, Motion};
use crate::text_cursor;

impl Document {
    pub fn insert_at_carets(
        &mut self,
        editor: EditorId,
        text: &str,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        if text.is_empty() {
            return;
        }
        let carets = self.carets(editor).clamped(&self.window(editor));
        let ranges: Vec<Range<u32>> = carets
            .carets()
            .iter()
            .map(|caret| caret.selection())
            .collect();
        self.replace_at_carets(editor, carets, ranges, text, fonts, theme, fx)
    }

    pub fn delete_selections(
        &mut self,
        editor: EditorId,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let carets = self.carets(editor).clamped(&self.window(editor));
        let ranges: Vec<Range<u32>> = carets
            .carets()
            .iter()
            .map(|caret| caret.selection())
            .collect();
        self.replace_at_carets(editor, carets, ranges, "", fonts, theme, fx)
    }

    pub fn delete_at_carets(
        &mut self,
        editor: EditorId,
        motion: Motion,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let window = self.window(editor);
        let carets = self.carets(editor).clamped(&window);
        let text = self.text();
        let ranges: Vec<Range<u32>> = carets
            .carets()
            .iter()
            .map(|caret| {
                if caret.has_selection() {
                    return caret.selection();
                }
                let offset = caret.offset();
                let target = match motion {
                    Motion::Left => text_cursor::previous_char_before(text, offset)
                        .map_or(offset, |(start, _)| start),
                    Motion::Right => {
                        text_cursor::next_char_after(text, offset).map_or(offset, |(end, _)| end)
                    }
                    Motion::WordLeft => {
                        text_cursor::previous_word_start(text, offset).unwrap_or(window.start)
                    }
                    Motion::WordRight => {
                        text_cursor::next_word_end(text, offset).unwrap_or(window.end)
                    }
                    Motion::LineStart => text_cursor::hard_line_range(text, offset).start,
                    Motion::LineEnd => {
                        line_end_before_newline(text, text_cursor::hard_line_range(text, offset))
                    }
                    Motion::DocumentStart => window.start,
                    Motion::DocumentEnd => window.end,
                    Motion::Up | Motion::Down => unreachable!("delete spans are horizontal"),
                };
                let target = target.clamp(window.start, window.end);
                target.min(offset)..target.max(offset)
            })
            .collect();
        self.replace_at_carets(editor, carets, ranges, "", fonts, theme, fx)
    }

    pub fn replace_at_carets(
        &mut self,
        editor: EditorId,
        carets: MultiCaret,
        ranges: Vec<Range<u32>>,
        insert: &str,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) {
        let (operation, after) = bulk_replace(self.text(), &carets, ranges, insert);
        if operation.is_empty() {
            return;
        }
        self.edit(&operation, fonts, theme, fx);
        self.set_carets(editor, after);
    }

    pub fn move_carets(
        &mut self,
        editor: EditorId,
        motion: Motion,
        select: bool,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) {
        if matches!(motion, Motion::Up | Motion::Down) {
            return self.move_carets_vertically(
                editor,
                matches!(motion, Motion::Down),
                select,
                fonts,
                theme,
            );
        }
        let window = self.window(editor);
        let carets = self.carets(editor).clamped(&window);
        let text = self.text();
        let moved = carets.map(|caret| {
            let offset = caret.offset();
            let target = match motion {
                Motion::Left if !select && caret.has_selection() => caret.selection().start,
                Motion::Right if !select && caret.has_selection() => caret.selection().end,
                Motion::Left => text_cursor::previous_char_before(text, offset)
                    .map_or(window.start, |(start, _)| start),
                Motion::Right => {
                    text_cursor::next_char_after(text, offset).map_or(window.end, |(end, _)| end)
                }
                Motion::WordLeft => {
                    text_cursor::previous_word_start(text, offset).unwrap_or(window.start)
                }
                Motion::WordRight => text_cursor::next_word_end(text, offset).unwrap_or(window.end),
                Motion::LineStart => text_cursor::hard_line_range(text, offset).start,
                Motion::LineEnd => {
                    line_end_before_newline(text, text_cursor::hard_line_range(text, offset))
                }
                Motion::DocumentStart => window.start,
                Motion::DocumentEnd => window.end,
                Motion::Up | Motion::Down => unreachable!("handled above"),
            };
            caret.moved_to(target.clamp(window.start, window.end), select)
        });
        self.set_carets(editor, moved);
    }

    pub fn move_carets_vertically(
        &mut self,
        editor: EditorId,
        down: bool,
        select: bool,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) {
        let window = self.window(editor);
        let carets = self.carets(editor).clamped(&window);
        let primary = carets.primary_index();
        let mut moved = Vec::with_capacity(carets.len());
        for caret in carets.carets() {
            let goal = caret.goal_x();
            let (target, x) = match self.vertical_caret_target(
                editor,
                caret.offset(),
                goal,
                down,
                fonts,
                theme,
            ) {
                Some(hit) => hit,
                None => (
                    if down { window.end } else { window.start },
                    goal.unwrap_or(0.0),
                ),
            };
            moved.push(
                caret
                    .moved_to(target.clamp(window.start, window.end), select)
                    .with_goal(x),
            );
        }
        self.set_carets(editor, MultiCaret::normalized(moved, primary));
    }

    pub fn add_caret_vertically(
        &mut self,
        editor: EditorId,
        above: bool,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) {
        let carets = self.carets(editor).clamped(&self.window(editor));
        let all = carets.carets();
        let edge = match above {
            true => all.first().copied(),
            false => all.last().copied(),
        };
        let Some(edge) = edge else {
            return;
        };
        let Some((target, x)) =
            self.vertical_caret_target(editor, edge.offset(), edge.goal_x(), !above, fonts, theme)
        else {
            return;
        };
        self.set_carets(editor, carets.with_added(Caret::at(target).with_goal(x)));
    }

    pub fn select_all(&mut self, editor: EditorId) {
        let window = self.window(editor);
        self.set_carets(
            editor,
            MultiCaret::one(Caret::selecting(window.start, window.end)),
        );
    }

    pub fn collapse_carets(&mut self, editor: EditorId) {
        self.set_carets(editor, self.carets(editor).collapsed_to_primary());
    }

    pub fn select_next_occurrence(&mut self, editor: EditorId) {
        let carets = self.carets(editor).clamped(&self.window(editor));
        let primary = carets.primary();
        if !primary.has_selection() {
            if let Some(word) = text_cursor::word_around(self.text(), primary.offset()) {
                let mut all: Vec<Caret> = carets.carets().to_vec();
                let index = carets.primary_index();
                all[index] = Caret::selecting(word.start, word.end);
                self.set_carets(editor, MultiCaret::normalized(all, index));
            }
            return;
        }
        let needle = slice_of(self.text(), primary.selection());
        if needle.is_empty() {
            return;
        }
        let from = carets
            .carets()
            .iter()
            .map(|caret| caret.selection().end)
            .max()
            .unwrap_or(0);
        let Some(found) = next_occurrence(self.text(), needle.as_bytes(), from, &carets) else {
            return;
        };
        self.set_carets(
            editor,
            carets.with_added(Caret::selecting(found.start, found.end)),
        );
    }

    pub fn select_all_occurrences(&mut self, editor: EditorId) {
        let window = self.window(editor);
        let carets = self.carets(editor).clamped(&window);
        let primary = carets.primary();
        let needle = match primary.has_selection() {
            true => slice_of(self.text(), primary.selection()),
            false => match text_cursor::word_around(self.text(), primary.offset()) {
                Some(word) => slice_of(self.text(), word),
                None => return,
            },
        };
        if needle.is_empty() {
            return;
        }
        let mut found = Vec::new();
        let mut primary_index = 0;

        each_occurrence(
            self.text(),
            needle.as_bytes(),
            window.clone(),
            |occurrence| {
                if occurrence.start >= primary.selection().start
                    && occurrence.start <= primary.selection().end
                {
                    primary_index = found.len();
                }
                found.push(Caret::selecting(occurrence.start, occurrence.end));
                true
            },
        );
        if !found.is_empty() {
            self.set_carets(editor, MultiCaret::normalized(found, primary_index));
        }
    }

    pub fn click_carets(
        &mut self,
        editor: EditorId,
        point: skia_safe::Point,
        kind: ClickKind,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) {
        let Some(byte) = self.byte_at_point(editor, point.x, point.y, fonts, theme) else {
            return;
        };
        let window = self.window(editor);
        let byte = byte.clamp(window.start, window.end);
        let carets = self.carets(editor).clamped(&window);
        let (clicked, origin) = match kind {
            ClickKind::Set => (MultiCaret::single(byte), Some(DragOrigin::Char(byte))),
            ClickKind::Extend => {
                let mut all: Vec<Caret> = carets.carets().to_vec();
                let index = carets.primary_index();
                all[index] = all[index].moved_to(byte, true);

                let origin = DragOrigin::Char(all[index].anchor());
                (MultiCaret::normalized(all, index), Some(origin))
            }
            ClickKind::Add => match carets.with_removed_at(byte) {
                Some(remaining) => (remaining, None),
                None => (
                    carets.with_added(Caret::at(byte)),
                    Some(DragOrigin::Char(byte)),
                ),
            },

            ClickKind::Word => match crate::text_cursor::word_around(&self.text, byte) {
                Some(range) => {
                    let range = range.start.max(window.start)..range.end.min(window.end);
                    (
                        MultiCaret::one(Caret::selecting(range.start, range.end)),
                        Some(DragOrigin::Word(range)),
                    )
                }
                None => (MultiCaret::single(byte), Some(DragOrigin::Char(byte))),
            },
            ClickKind::Line => {
                let range = crate::text_cursor::hard_line_range(&self.text, byte);
                let range = range.start.max(window.start)..range.end.min(window.end);
                (
                    MultiCaret::one(Caret::selecting(range.start, range.end)),
                    Some(DragOrigin::Line(range)),
                )
            }
        };
        self.set_carets(editor, clicked);
        if let Some(state) = self.editors.get_mut(&editor) {
            state.drag = origin;
        }
    }

    pub fn drag_carets(
        &mut self,
        editor: EditorId,
        point: skia_safe::Point,
        fonts: &FontCollection,
        theme: &crate::theme::Theme,
    ) {
        let Some(origin) = self
            .editors
            .get(&editor)
            .and_then(|state| state.drag.clone())
        else {
            return;
        };
        let Some(byte) = self.byte_at_point(editor, point.x, point.y, fonts, theme) else {
            return;
        };
        let window = self.window(editor);
        let byte = byte.clamp(window.start, window.end);
        let unit_extend = |origin: &std::ops::Range<u32>, span: std::ops::Range<u32>| -> Caret {
            let span = span.start.max(window.start)..span.end.min(window.end);
            if span.start < origin.start {
                Caret::selecting(origin.end, span.start)
            } else if span.end > origin.end {
                Caret::selecting(origin.start, span.end)
            } else {
                Caret::selecting(origin.start, origin.end)
            }
        };
        let primary = match &origin {
            DragOrigin::Char(anchor) => {
                Caret::selecting((*anchor).clamp(window.start, window.end), byte)
            }
            DragOrigin::Word(range) => unit_extend(
                range,
                crate::text_cursor::word_around(&self.text, byte).unwrap_or(byte..byte),
            ),
            DragOrigin::Line(range) => {
                unit_extend(range, crate::text_cursor::hard_line_range(&self.text, byte))
            }
        };
        let carets = self.carets(editor).clamped(&window);
        let mut all: Vec<Caret> = carets.carets().to_vec();
        let index = carets.primary_index();
        all[index] = primary;
        self.set_carets(editor, MultiCaret::normalized(all, index));
        if let Some(state) = self.editors.get_mut(&editor) {
            state.reveal = true;
        }
    }
}

fn bulk_replace(
    text: &Text,
    carets: &MultiCaret,
    ranges: Vec<Range<u32>>,
    insert: &str,
) -> (Operation, MultiCaret) {
    let byte_count = text_cursor::byte_count(text);
    let insert_len = insert.len().min(u32::MAX as usize) as u32;
    let mut builder = OperationBuilder::new();
    let mut view = text.view();
    let mut scratch: Vec<u8> = Vec::new();
    let mut consumed = 0u32;
    let mut grown = 0i64;
    let mut after = Vec::with_capacity(ranges.len());
    for range in ranges {
        let start = range.start.clamp(consumed, byte_count);
        let end = range.end.clamp(start, byte_count);
        builder.push_retain(start - consumed);
        if end > start {
            scratch.clear();
            view.byte_range_into(start as usize, end as usize, &mut scratch);
            let deleted = String::from_utf8(scratch.clone())
                .expect("caret selections lie on char boundaries");
            builder.push_delete(deleted);
        }
        if insert_len > 0 {
            builder.push_insert(insert.to_owned());
        }
        grown += insert_len as i64 - (end - start) as i64;
        consumed = end;
        after.push(Caret::at((end as i64 + grown) as u32));
    }
    (
        builder.finish(),
        MultiCaret::normalized(after, carets.primary_index()),
    )
}

fn line_end_before_newline(text: &Text, line: Range<u32>) -> u32 {
    if line.end == line.start {
        return line.end;
    }
    let mut last = Vec::with_capacity(1);
    text.view()
        .byte_range_into(line.end as usize - 1, line.end as usize, &mut last);
    match last.first() {
        Some(b'\n') => line.end - 1,
        _ => line.end,
    }
}

fn slice_of(text: &Text, range: Range<u32>) -> String {
    let byte_count = text_cursor::byte_count(text);
    let start = range.start.min(byte_count);
    let end = range.end.min(byte_count).max(start);
    text.view().byte_string(start as usize, end as usize)
}

fn next_occurrence(
    text: &Text,
    needle: &[u8],
    from: u32,
    carets: &MultiCaret,
) -> Option<Range<u32>> {
    let byte_count = text_cursor::byte_count(text);
    let mut cursor = from;
    let mut wrapped = false;
    loop {
        let limit = if wrapped { from } else { byte_count };
        match find_occurrence(text, needle, cursor..limit) {
            Some(found) => {
                let covered = carets.carets().iter().any(|caret| {
                    caret.selection().start < found.end && found.start < caret.selection().end
                });
                if !covered {
                    return Some(found);
                }
                cursor = found.end.max(found.start + 1);
            }
            None if !wrapped => {
                wrapped = true;
                cursor = 0;
            }
            None => return None,
        }
    }
}

pub(crate) fn each_occurrence(
    text: &Text,
    needle: &[u8],
    range: Range<u32>,
    mut visit: impl FnMut(Range<u32>) -> bool,
) {
    if needle.is_empty() || range.start >= range.end {
        return;
    }
    let byte_count = text_cursor::byte_count(text) as usize;
    let needle_len = needle.len();
    let end = (range.end as usize).min(byte_count);
    let mut view = text.view();
    let mut buffer = Vec::new();
    const WINDOW: usize = 64 * 1024;
    let mut start = range.start as usize;

    let mut resume = start;
    while start < end {
        let chunk_end = (start + WINDOW + needle_len - 1).min(byte_count);
        buffer.clear();
        view.byte_range_into(start, chunk_end, &mut buffer);
        let mut local = resume.saturating_sub(start);
        while local < buffer.len() {
            let Some(at) = buffer[local..]
                .windows(needle_len)
                .position(|candidate| candidate == needle)
            else {
                break;
            };
            let found = local + at;
            if start + found >= end {
                return;
            }
            if found >= WINDOW {
                break;
            }
            let absolute = start + found;
            if !visit(absolute as u32..(absolute + needle_len) as u32) {
                return;
            }
            local = found + needle_len.max(1);
            resume = start + local;
        }
        start += WINDOW;
        resume = resume.max(start);
    }
}

pub(crate) fn find_occurrence(text: &Text, needle: &[u8], range: Range<u32>) -> Option<Range<u32>> {
    if needle.is_empty() || range.start >= range.end {
        return None;
    }
    let byte_count = text_cursor::byte_count(text) as usize;
    let needle_len = needle.len();
    let end = (range.end as usize).min(byte_count);
    let mut view = text.view();
    let mut buffer = Vec::new();
    let mut start = range.start as usize;
    const WINDOW: usize = 64 * 1024;
    while start < end {
        let chunk_end = (start + WINDOW + needle_len - 1).min(byte_count);
        buffer.clear();
        view.byte_range_into(start, chunk_end, &mut buffer);
        if let Some(at) = buffer
            .windows(needle_len)
            .position(|window| window == needle)
        {
            let found = start + at;
            if found >= end {
                return None;
            }
            return Some(found as u32..(found + needle_len) as u32);
        }
        if chunk_end >= byte_count {
            return None;
        }
        start += WINDOW;
    }
    None
}
