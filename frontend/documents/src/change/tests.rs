// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;
use imba::View;

use super::*;
use crate::{EditorIdView, OpenDocuments};
use ::editor::{test_document::plain_document, EditorCommand};

fn location(name: &str) -> ResourceLocation {
    ResourceLocation::new(
        editor::ResourceType::document(),
        editor::Authority::new("local"),
        vec![name.to_owned()],
    )
}

struct Pane {
    store: Store,
    view: EditorIdView,
}

impl Pane {
    fn new(source: &str, located: bool, observed: bool) -> Self {
        let mut store = Store::new();
        if observed {
            ChangeObserver::install(&mut store, std::sync::Arc::new(|_| Vec::new()));
        }
        Self::with_store(store, plain_document(source), located)
    }

    fn with_store(mut store: Store, mut document: ::editor::Document, located: bool) -> Self {
        let ui = &imba::UiCtx::dont_use_too_slow();
        let editor = document.add_editor(
            400.0,
            None,
            ::editor::EditorBuild::Complete,
            &[],
            &store,
            ui,
            &::editor::embedded_fonts::source()(),
            &::editor::theme::Theme::embedded(),
            &mut imba::effect::Batch::new().effects(),
        );

        let document_id = OpenDocuments::register(
            &mut store,
            document,
            match located {
                true => Some(location("a.md")),
                false => None,
            },
            "a.md".to_owned(),
            0,
        );
        Self {
            view: EditorIdView::new(document_id, editor),
            store,
        }
    }

    fn document(&self) -> ::editor::Document {
        let entity = self.view;
        OpenDocuments::document(&self.store, entity.document()).expect("document")
    }

    fn set_caret(&mut self, byte: u32) {
        let entity = self.view;
        let mut document =
            OpenDocuments::document(&self.store, entity.document()).expect("document");
        document.set_caret(entity.editor(), byte);
        OpenDocuments::put_document(&mut self.store, entity.document(), document);
    }

    fn perform(&mut self, command: EditorCommand) -> Vec<imba::effect::AnyEffect<EditorCommand>> {
        let mut batch = imba::effect::Batch::new();
        self.view.perform(
            &mut self.store,
            &imba::UiCtx::dont_use_too_slow(),
            command,
            &mut batch.effects(),
        );
        batch.surviving_launches()
    }
}

fn notifications(
    launches: &[imba::effect::AnyEffect<EditorCommand>],
) -> Vec<&DocumentChangeEffect> {
    launches
        .iter()
        .filter_map(|effect| effect.get::<DocumentChangeEffect>())
        .collect()
}

fn lc(line: u32, col: u32) -> LineCol {
    LineCol { line, col }
}

#[test]
fn typing_notifies_with_chained_line_col_changes() {
    let mut pane = Pane::new("hello\nworld", true, true);
    let revision_zero = pane.document().revision();

    let launches = pane.perform(EditorCommand::InsertText {
        text: "X".to_owned(),
    });
    let notes = notifications(&launches);
    assert_eq!(notes.len(), 1, "one edit, one notification");
    let first = notes[0];
    assert_eq!(first.location, location("a.md"));
    assert_eq!(first.base_revision, revision_zero);
    assert!(first.revision > revision_zero, "the post-edit revision");
    assert_eq!(
        first.changes,
        vec![TextChange {
            range: lc(0, 0)..lc(0, 0),
            text: "X".to_owned(),
        }],
        "an insertion at the caret: an empty pre-range with the text"
    );
    assert_eq!(
        first
            .text
            .view()
            .byte_string(0, first.text.view().byte_count()),
        "Xhello\nworld",
        "the snapshot is the post-change text"
    );
    let first_revision = first.revision;

    pane.set_caret(9);
    let launches = pane.perform(EditorCommand::InsertText {
        text: "Y".to_owned(),
    });
    let notes = notifications(&launches);
    assert_eq!(notes.len(), 1);
    let second = notes[0];
    assert_eq!(second.base_revision, first_revision, "gapless chain");
    assert_eq!(
        second.changes,
        vec![TextChange {
            range: lc(1, 2)..lc(1, 2),
            text: "Y".to_owned(),
        }],
        "line/col against the text before THIS change"
    );
}

#[test]
fn deleting_notifies_the_deleted_range() {
    let mut pane = Pane::new("hello\nworld", true, true);
    pane.set_caret(1);
    let launches = pane.perform(EditorCommand::Backspace);
    let notes = notifications(&launches);
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0].changes,
        vec![TextChange {
            range: lc(0, 0)..lc(0, 1),
            text: String::new(),
        }],
        "a deletion: the removed pre-range, empty replacement"
    );
}

#[test]
fn multi_caret_edits_emit_back_to_front() {
    let mut pane = Pane::new("ab ab", true, true);
    pane.set_caret(0);
    let launches = pane.perform(EditorCommand::SelectAllOccurrences);
    assert!(
        notifications(&launches).is_empty(),
        "selection moves no text"
    );
    let launches = pane.perform(EditorCommand::InsertText {
        text: "z".to_owned(),
    });
    let notes = notifications(&launches);
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0].changes,
        vec![
            TextChange {
                range: lc(0, 3)..lc(0, 5),
                text: "z".to_owned(),
            },
            TextChange {
                range: lc(0, 0)..lc(0, 2),
                text: "z".to_owned(),
            },
        ],
        "descending: the later range first"
    );
    assert_eq!(
        notes[0]
            .text
            .view()
            .byte_string(0, notes[0].text.view().byte_count()),
        "z z"
    );
}

#[test]
fn caret_motion_notifies_nothing() {
    let mut pane = Pane::new("hello", true, true);
    let launches = pane.perform(EditorCommand::Move {
        motion: ::editor::Motion::Right,
        select: false,
    });
    assert!(notifications(&launches).is_empty(), "no text change");
}

#[test]
fn unlocated_documents_notify_nothing() {
    let mut pane = Pane::new("hello", false, true);
    let launches = pane.perform(EditorCommand::InsertText {
        text: "X".to_owned(),
    });
    assert!(
        notifications(&launches).is_empty(),
        "a scratch document has no name to notify under"
    );
}

#[test]
fn without_the_observer_nothing_launches() {
    let mut pane = Pane::new("hello", true, false);
    let launches = pane.perform(EditorCommand::InsertText {
        text: "X".to_owned(),
    });
    assert!(
        notifications(&launches).is_empty(),
        "no observer installed: no handler is owed"
    );
}

#[test]
fn line_col_conversions_round_trip_and_clamp() {
    let text = text::Text::from_string("ab\ncd\n");
    let mut view = text.view();

    assert_eq!(line_col_at(&mut view, 0), lc(0, 0));
    assert_eq!(line_col_at(&mut view, 2), lc(0, 2));
    assert_eq!(line_col_at(&mut view, 3), lc(1, 0));
    assert_eq!(line_col_at(&mut view, 5), lc(1, 2));
    assert_eq!(
        line_col_at(&mut view, 6),
        lc(2, 0),
        "the trailing empty line"
    );

    assert_eq!(offset_at(&mut view, lc(1, 1)), 4);
    assert_eq!(
        offset_at(&mut view, lc(0, 99)),
        2,
        "clamped before the newline"
    );
    assert_eq!(
        offset_at(&mut view, lc(1, 99)),
        5,
        "clamped before the newline"
    );
    assert_eq!(
        offset_at(&mut view, lc(99, 0)),
        6,
        "clamped to the last line"
    );

    let empty = text::Text::from_string("");
    let mut view = empty.view();
    assert_eq!(line_col_at(&mut view, 0), lc(0, 0));
    assert_eq!(offset_at(&mut view, lc(5, 5)), 0);
}
