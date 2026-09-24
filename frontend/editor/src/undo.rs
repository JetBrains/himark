// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use operation::{Op, Operation};

use crate::caret::MultiCaret;
use crate::editor::EditorId;

#[derive(Clone)]
pub(crate) struct UndoHistory {
    undo: rpds::VectorSync<UndoEntry>,
    redo: rpds::VectorSync<UndoEntry>,

    pub(crate) replaying: bool,
}

#[derive(Clone)]
pub(crate) struct UndoEntry {
    pub(crate) operation: Operation,
    pub(crate) snapshot: Option<CaretSnapshot>,
}

#[derive(Clone)]
pub(crate) struct CaretSnapshot {
    pub(crate) editor: EditorId,
    pub(crate) before: MultiCaret,
    pub(crate) after: MultiCaret,
}

impl Default for UndoHistory {
    fn default() -> Self {
        Self {
            undo: rpds::VectorSync::new_sync(),
            redo: rpds::VectorSync::new_sync(),
            replaying: false,
        }
    }
}

impl UndoHistory {
    pub(crate) fn note_edit(&mut self, operation: &Operation, old_len: u32, composing: bool) {
        if self.replaying {
            return;
        }
        self.redo = rpds::VectorSync::new_sync();
        // The edit door admits only exact-coverage operations — a
        // mismatch here is a bug upstream, never padded over.
        debug_assert_eq!(operation.old_len(), old_len);
        let padded = operation.clone();
        let coalesced = self
            .undo
            .last()
            .filter(|top| composing || continues_word(&top.operation, &padded));
        match coalesced {
            Some(top) => {
                let merged = UndoEntry {
                    operation: top.operation.compose(&padded),
                    ..top.clone()
                };
                let index = self.undo.len() - 1;
                self.undo.set_mut(index, merged);
            }
            None => {
                self.undo.push_back_mut(UndoEntry {
                    operation: padded,
                    snapshot: None,
                });
            }
        }
    }

    pub(crate) fn stamp(&mut self, editor: EditorId, before: &MultiCaret, after: &MultiCaret) {
        if self.replaying {
            return;
        }
        let Some(top) = self.undo.last() else {
            return;
        };
        let mut stamped = top.clone();
        match &mut stamped.snapshot {
            Some(snapshot) => snapshot.after = after.clone(),
            None => {
                stamped.snapshot = Some(CaretSnapshot {
                    editor,
                    before: before.clone(),
                    after: after.clone(),
                })
            }
        }
        let index = self.undo.len() - 1;
        self.undo.set_mut(index, stamped);
    }

    pub(crate) fn take_undo(&mut self) -> Option<UndoEntry> {
        let entry = self.undo.last()?.clone();
        self.undo.drop_last_mut();
        Some(entry)
    }

    pub(crate) fn take_redo(&mut self) -> Option<UndoEntry> {
        let entry = self.redo.last()?.clone();
        self.redo.drop_last_mut();
        Some(entry)
    }

    pub(crate) fn park_redo(&mut self, entry: UndoEntry) {
        self.redo.push_back_mut(entry);
    }

    pub(crate) fn restore_undo(&mut self, entry: UndoEntry) {
        self.undo.push_back_mut(entry);
    }

    pub(crate) fn reset(&mut self) {
        self.undo = rpds::VectorSync::new_sync();
        self.redo = rpds::VectorSync::new_sync();
    }

    pub(crate) fn carry_across(&mut self, foreign: &Operation, old_len: u32) {
        debug_assert_eq!(foreign.old_len(), old_len);
        let foreign = foreign.clone();

        let mut carried = foreign.clone();
        let mut undo: Vec<UndoEntry> = self.undo.iter().cloned().collect();
        for entry in undo.iter_mut().rev() {
            let inverse = entry.operation.invert();
            let undo_after = inverse.transform(&carried);
            let below = carried.transform(&inverse);
            entry.snapshot = entry.snapshot.take().map(|snapshot| CaretSnapshot {
                before: snapshot.before.transformed(&below),
                after: snapshot.after.transformed(&carried),
                ..snapshot
            });
            entry.operation = undo_after.invert();
            carried = below;
        }
        self.undo = undo.into_iter().collect();

        let mut carried = foreign;
        let mut redo: Vec<UndoEntry> = self.redo.iter().cloned().collect();
        for entry in redo.iter_mut().rev() {
            let redone = entry.operation.transform(&carried);
            let above = carried.transform(&entry.operation);
            entry.snapshot = entry.snapshot.take().map(|snapshot| CaretSnapshot {
                before: snapshot.before.transformed(&carried),
                after: snapshot.after.transformed(&above),
                ..snapshot
            });
            entry.operation = redone;
            carried = above;
        }
        self.redo = redo.into_iter().collect();
    }
}

fn continues_word(top: &Operation, next: &Operation) -> bool {
    let mut position = 0u32;
    let mut insert: Option<u32> = None;
    for op in next.iter() {
        match op {
            Op::Retain(len) => position += len,
            Op::Insert(text) => {
                if insert.is_some() || text.chars().any(char::is_whitespace) {
                    return false;
                }
                insert = Some(position);
            }
            Op::Delete(_) => return false,
        }
    }
    insert.is_some_and(|at| at == end_of_last_change(top))
}

fn end_of_last_change(operation: &Operation) -> u32 {
    let mut position = 0u32;
    let mut end = 0u32;
    for op in operation.iter() {
        match op {
            Op::Retain(len) => position += len,
            Op::Insert(text) => {
                position += text.len().min(u32::MAX as usize) as u32;
                end = position;
            }
            Op::Delete(_) => end = position,
        }
    }
    end
}

#[cfg(test)]
mod tests;
