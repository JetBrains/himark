// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;
use operation::{Op, Operation};

use crate::caret::{Caret, MultiCaret};
use crate::document::Document;
use crate::editor::EditorId;
use crate::editor_view::EditorCommand;
use crate::test_document::plain_document;

fn test_fonts() -> skia_safe::textlayout::FontCollection {
    crate::embedded_fonts::collection()
}

struct Pane {
    store: Store,
    ui: imba::UiCtx,
    document: Document,
    editor: EditorId,
}

impl Pane {
    fn new(source: &str) -> Self {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
        let mut document = plain_document(source);
        let editor = document.add_editor(
            400.0,
            None,
            crate::document::EditorBuild::Complete,
            &[],
                store, ui,
            &test_fonts(),
            &crate::theme::Theme::embedded(),
            &mut imba::effect::Batch::new().effects(),
        );
        Self {
            store: Store::new(),
            ui: imba::UiCtx::dont_use_too_slow(),
            document,
            editor,
        }
    }

    fn perform(&mut self, command: EditorCommand) {
        self.document.perform(
            &mut self.store,
            &self.ui,
            self.editor,
            command,
            &mut imba::effect::Batch::new().effects(),
        );
    }

    fn type_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.perform(EditorCommand::InsertText {
                text: ch.to_string(),
            });
        }
    }

    fn text(&self) -> String {
        let mut view = self.document.text().view();
        let count = view.byte_count();
        view.byte_string(0, count)
    }
}

#[test]
fn words_coalesce_and_round_trip() {
    let mut pane = Pane::new("");
    pane.type_str("hi there");
    assert_eq!(pane.text(), "hi there");

    pane.perform(EditorCommand::Undo);
    assert_eq!(pane.text(), "hi");
    pane.perform(EditorCommand::Undo);
    assert_eq!(pane.text(), "");
    pane.perform(EditorCommand::Undo);
    assert_eq!(pane.text(), "", "an empty stack is a no-op");
    pane.perform(EditorCommand::Redo);
    pane.perform(EditorCommand::Redo);
    assert_eq!(pane.text(), "hi there");
    assert_eq!(pane.document.caret_byte(pane.editor), 8);
}

#[test]
fn undo_restores_the_selection_it_replaced() {
    let mut pane = Pane::new("hello world");
    pane.document.set_carets(
        pane.editor,
        MultiCaret::normalized(vec![Caret::selecting(0, 5)], 0),
    );
    pane.perform(EditorCommand::Paste {
        text: "X".to_owned(),
    });
    assert_eq!(pane.text(), "X world");
    pane.perform(EditorCommand::Undo);
    assert_eq!(pane.text(), "hello world");
    let restored = pane.document.carets(pane.editor).primary().selection();
    assert_eq!(restored, 0..5, "the replaced selection is back");
    pane.perform(EditorCommand::Redo);
    assert_eq!(pane.text(), "X world");
    assert_eq!(pane.document.caret_byte(pane.editor), 1);
}

#[test]
fn a_multi_caret_bulk_edit_is_one_entry() {
    let mut pane = Pane::new("hello world");
    pane.document.set_carets(
        pane.editor,
        MultiCaret::normalized(vec![Caret::at(0), Caret::at(6)], 0),
    );
    pane.perform(EditorCommand::InsertText {
        text: "x".to_owned(),
    });
    assert_eq!(pane.text(), "xhello xworld");
    pane.perform(EditorCommand::Undo);
    assert_eq!(pane.text(), "hello world");
    let carets: Vec<u32> = pane
        .document
        .carets(pane.editor)
        .carets()
        .iter()
        .map(|caret| caret.offset())
        .collect();
    assert_eq!(carets, vec![0, 6], "both carets restored exactly");
}

#[test]
fn a_fresh_edit_clears_redo() {
    let mut pane = Pane::new("");
    pane.type_str("one");
    pane.perform(EditorCommand::Undo);
    assert_eq!(pane.text(), "");
    pane.type_str("two");
    pane.perform(EditorCommand::Redo);
    assert_eq!(pane.text(), "two", "redo after a fresh edit is a no-op");
}

#[test]
fn a_composition_is_one_entry() {
    let mut pane = Pane::new("");
    pane.perform(EditorCommand::SetMarkedText {
        text: "\u{306B}".to_owned(),
        selected: (1, 0),
        replacement: None,
    });
    pane.perform(EditorCommand::SetMarkedText {
        text: "\u{306B}\u{307B}".to_owned(),
        selected: (2, 0),
        replacement: None,
    });
    pane.perform(EditorCommand::InsertText {
        text: "\u{65E5}\u{672C}".to_owned(),
    });
    assert_eq!(pane.text(), "\u{65E5}\u{672C}");
    pane.perform(EditorCommand::Undo);
    assert_eq!(pane.text(), "", "the whole composition undoes as one");
}

#[test]
fn clearing_history_disarms_both_stacks() {
    let mut pane = Pane::new("");
    pane.type_str("keep");
    pane.document.clear_undo_history();
    pane.perform(EditorCommand::Undo);
    assert_eq!(pane.text(), "keep");
    pane.perform(EditorCommand::Redo);
    assert_eq!(pane.text(), "keep");
}

#[test]
fn a_shared_edit_carries_the_undo_history_across_itself() {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
    let mut pane = Pane::new("");
    pane.type_str("hello");
    assert_eq!(pane.text(), "hello");

    let foreign = Operation::from_ops([Op::Insert(">> ".to_owned()), Op::Retain(5)]);
    pane.document.edit_shared(
        crate::EditIdentity::mint(),
        &foreign,
                store, ui,
        &test_fonts(),
        &crate::theme::Theme::embedded(),
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(pane.text(), ">> hello");

    pane.perform(EditorCommand::Undo);
    assert_eq!(pane.text(), ">> ", "our word goes, theirs stays");
    pane.perform(EditorCommand::Undo);
    assert_eq!(pane.text(), ">> ", "theirs was never ours to undo");
    pane.perform(EditorCommand::Redo);
    assert_eq!(
        pane.text(),
        ">> hello",
        "and ours comes back where it now belongs"
    );
}
