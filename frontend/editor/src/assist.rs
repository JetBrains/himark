// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use imba::store::Store;
use operation::{Op, Operation, OperationBuilder};

use crate::caret::{Caret, MultiCaret};
use crate::document::Document;
use crate::editor::{EditorEffects, EditorId};
use crate::markup::Syntax;
use crate::reparse::{Assist, AssistKind, AssistRequest};
use crate::text_cursor;

pub(crate) const INDENT_UNIT: &str = "    ";

pub(crate) fn single_typed_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let ch = chars.next()?;
    if chars.next().is_some() || ch.is_control() {
        return None;
    }
    Some(ch)
}

impl Document {
    pub fn syntax_enclosing(&self, byte: u32) -> Option<(&Syntax, Range<u32>)> {
        let byte_count = text_cursor::byte_count(&self.text);
        let mut syntax = self.syntax()?;
        let mut range = 0u32..byte_count;
        loop {
            match syntax.markup.child_syntax_at(byte, range.start) {
                Some((child, child_range)) => {
                    syntax = child;
                    range = child_range;
                }
                None => return Some((syntax, range)),
            }
        }
    }

    pub(crate) fn assist_at_carets(
        &mut self,
        store: &Store,
        editor: EditorId,
        kind: AssistKind,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
        fx: &mut EditorEffects<'_>,
    ) -> bool {
        let Some(languages) = crate::env::Parsers::of(store) else {
            return false;
        };
        let carets = self.carets(editor).clamped(&self.window(editor));
        let mut assisted = false;
        let mut edits: Vec<CaretEdit> = Vec::with_capacity(carets.carets().len());
        for caret in carets.carets() {
            let location = caret.selection();
            let assist = self
                .syntax_enclosing(location.start)
                .and_then(|(syntax, range)| {
                    let tree = syntax.tree.as_deref()?;
                    let language = languages.get(&syntax.language)?;
                    language.assist(&AssistRequest {
                        kind,
                        text: &self.text,
                        tree,
                        range,
                        location: location.clone(),
                    })
                });
            let edit = match assist {
                Some(assist) => {
                    assisted = true;
                    CaretEdit::from_assist(assist, location.start)
                }
                None => fallback_edit(&self.text, kind, &location),
            };
            edits.push(edit);
        }
        if !assisted {
            return false;
        }

        let mut builder = OperationBuilder::new();
        let mut cursor = 0u32;
        let mut shift = 0i64;
        let mut after: Vec<Caret> = Vec::with_capacity(edits.len());
        for edit in edits {
            match edit.operation {
                Some(operation) if edit.start >= cursor => {
                    let mut position = 0u32;
                    for run in operation.iter() {
                        match run {
                            Op::Retain(len) => position += len,
                            Op::Insert(text) => {
                                builder.push_retain(position - cursor);
                                cursor = position;
                                builder.push_insert(text);
                            }
                            Op::Delete(text) => {
                                builder.push_retain(position - cursor);
                                let len = text.len().min(u32::MAX as usize) as u32;
                                builder.push_delete(text);
                                position += len;
                                cursor = position;
                            }
                        }
                    }
                    after.push(Caret::at(shifted(edit.caret, shift)));
                    shift += edit.delta;
                }
                _ => {
                    after.push(Caret::at(shifted(edit.anchor, shift)));
                }
            }
        }
        let operation = builder.finish();
        if !operation.is_empty() {
            self.edit(&operation,
                store, ui, fonts, theme, fx);
        }
        self.set_carets(
            editor,
            MultiCaret::normalized(after, carets.primary_index()),
        );
        true
    }
}

fn shifted(offset: u32, shift: i64) -> u32 {
    (offset as i64 + shift).max(0) as u32
}

struct CaretEdit {
    start: u32,
    operation: Option<Operation>,

    caret: u32,

    anchor: u32,

    delta: i64,
}

impl CaretEdit {
    fn stay(anchor: u32) -> Self {
        Self {
            start: u32::MAX,
            operation: None,
            caret: anchor,
            anchor,
            delta: 0,
        }
    }

    fn from_assist(assist: Assist, anchor: u32) -> Self {
        let mut position = 0u32;
        let mut delta = 0i64;
        let mut first = None;
        let mut post_end = 0i64;
        for run in assist.operation.iter() {
            match run {
                Op::Retain(len) => position += len,
                Op::Insert(text) => {
                    first.get_or_insert(position);
                    delta += text.len() as i64;
                    post_end = position as i64 + delta;
                }
                Op::Delete(text) => {
                    first.get_or_insert(position);
                    let len = text.len().min(u32::MAX as usize) as u32;
                    position += len;
                    delta -= len as i64;
                    post_end = position as i64 + delta;
                }
            }
        }
        let Some(first) = first else {
            return Self::stay(anchor);
        };
        Self {
            start: first,
            operation: Some(assist.operation),
            caret: assist.caret.unwrap_or(post_end.max(0) as u32),
            anchor,
            delta,
        }
    }

    fn replace(text: &text::Text, range: &Range<u32>, insert: &str) -> Self {
        let byte_count = text_cursor::byte_count(text);
        let start = range.start.min(byte_count);
        let end = range.end.clamp(start, byte_count);
        let mut builder = OperationBuilder::new();
        builder.push_retain(start);
        if end > start {
            let mut deleted = Vec::with_capacity((end - start) as usize);
            text.view()
                .byte_range_into(start as usize, end as usize, &mut deleted);
            builder.push_delete(
                String::from_utf8(deleted).expect("caret selections lie on char boundaries"),
            );
        }
        builder.push_insert(insert.to_owned());
        let insert_len = insert.len().min(u32::MAX as usize) as u32;
        Self {
            start,
            operation: Some(builder.finish()),
            caret: start + insert_len,
            anchor: start,
            delta: insert_len as i64 - (end - start) as i64,
        }
    }
}

fn fallback_edit(text: &text::Text, kind: AssistKind, location: &Range<u32>) -> CaretEdit {
    match kind {
        AssistKind::Enter { .. } => CaretEdit::replace(text, location, "\n"),
        AssistKind::Indent => CaretEdit::replace(text, location, INDENT_UNIT),
        AssistKind::Outdent => CaretEdit::stay(location.start),
        AssistKind::Typed(ch) => CaretEdit::replace(text, location, ch.encode_utf8(&mut [0u8; 4])),
    }
}
