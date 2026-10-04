// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use ::comments::view::{CommentCommand, CommentView, RemoveComment};

use super::*;
use editor::document::Document;
use editor::editor_view::EditorCommand;
use editor::editor_view::EditorFocus;
use editor::markup::InlayKey;
use imba::ui::UiCtx;

use std::ops::Range;

use ::editor::test_document::plain_document;
use crate::app::AppCommand;
use crate::app_ext::AppExt;
use crate::app::AppFonts;
use crate::app::Application;
use crate::app::OpenedDocument;
use editor::caret::Caret;
use editor::caret::MultiCaret;

fn document_location(name: &str) -> editor::location::ResourceLocation {
    editor::location::ResourceLocation::new(
        editor::location::ResourceType::document(),
        editor::location::Authority::new("local"),
        vec!["project".to_owned(), name.to_owned()],
    )
}

struct SelectRange(Range<u32>);

impl editor::dynamic::DynamicEditorCommand for SelectRange {
    fn id(&self) -> &'static str {
        "test.select"
    }
    fn name(&self) -> String {
        "Test: Select".to_owned()
    }
    fn perform(
        &self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        document: &mut Document,
        editor: editor::editor::EditorId,
        _location: &editor::location::ResourceLocation,
        _payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        _fx: &mut imba::effect::Effects<'_, EditorCommand>,
    ) {
        document.set_carets(
            editor,
            MultiCaret::one(Caret::selecting(self.0.start, self.0.end)),
        );
    }
}

fn app_with_located_document(source: &str) -> (Application, ::workbench::window::WindowId) {
    let mut app = Application::new(AppFonts::embedded());
    app.register_syntax_languages(himarkdown::markdown_languages(editor::reparse::SyntaxLanguages::new()));
    // `comments.add` arrives via the session ceremony (a scoped,
    // sibling-wired instance) — the same road production takes.
    app.register_editor_command(std::sync::Arc::new(SelectRange(6..11)));
    let window = app.add_window();
    assert!(app.perform_command(AppCommand::Opened(
        window,
        OpenedDocument {
            documents: app.sole_documents(),
            name: "notes.md".to_owned(),
            document: plain_document(source),
            location: Some(document_location("notes.md")),
            primary: true,
            target: None,
            focus: false,
        },
    )));
    (app, window)
}

fn invoke(app: &mut Application, window: ::workbench::window::WindowId, id: &str) {
    let command = crate::commands::palette_commands(app.store(), &app.ui_handle(), window)
        .into_iter()
        .find(|presentable| presentable.id == id)
        .expect("the located editor offers the command")
        .command;
    assert!(app.perform_command(command));
}

fn commented_document(app: &Application) -> (documents::DocumentId, Vec<(InlayKey, Range<u32>)>) {
    for (id, entity) in documents::OpenDocuments::list(app.store(), app.sole_documents()) {
        let document = entity.document();
        let byte_count = document.text().byte_count().min(u32::MAX as usize) as u32;

        let Some(comments) = document.feature_markup(comments::view::comments_markup()) else {
            continue;
        };
        let extras = [(comments::view::comments_markup(), comments)];
        let inlays: Vec<(InlayKey, Range<u32>)> =
            editor::markup::OverlaidMarkup::new(document.markup(), &extras)
                .all_inlays_in(0..byte_count)
                .into_iter()
                .filter(|interval| interval.inlay.view_as::<CommentView>().is_some())
                .map(|interval| (interval.key, interval.range.clone()))
                .collect();
        if !inlays.is_empty() {
            return (id, inlays);
        }
    }
    panic!("no document carries a comment inlay");
}

#[test]
fn add_comment_attaches_a_below_card_over_the_selection() {
    let (mut app, window) = app_with_located_document("alpha beta gamma\nsecond line\n");
    invoke(&mut app, window, "test.select");
    invoke(&mut app, window, "comments.add");

    let (_, inlays) = commented_document(&app);
    assert_eq!(inlays.len(), 1, "one card per add");
    assert_eq!(inlays[0].1, 6..11, "the card spans the selection");
}

#[test]
fn without_a_selection_add_comment_is_a_no_op() {
    let (mut app, window) = app_with_located_document("alpha beta gamma\n");
    invoke(&mut app, window, "comments.add");

    for (_, entity) in documents::OpenDocuments::list(app.store(), app.sole_documents()) {
        let document = entity.document();
        let byte_count = document.text().byte_count().min(u32::MAX as usize) as u32;
        assert!(
            document
                .markup()
                .all_inlays_in(0..byte_count)
                .into_iter()
                .next()
                .is_none(),
            "a collapsed caret adds nothing"
        );
    }
}

#[test]
fn the_fresh_card_takes_the_focus() {
    let (mut app, window) = app_with_located_document("alpha beta gamma\n");
    invoke(&mut app, window, "test.select");
    invoke(&mut app, window, "comments.add");

    let (document, inlays) = commented_document(&app);
    let document = documents::OpenDocuments::document(app.store(), app.sole_documents(), document)
        .expect("the commented document");
    let focused = document
        .editor_ids()
        .any(|editor| document.focus(editor) == EditorFocus::Inlay(inlays[0].0));
    assert!(focused, "typing goes straight into the fresh card");
}

#[test]
fn remove_comment_clears_the_card_and_returns_focus() {
    let (mut app, window) = app_with_located_document("alpha beta gamma\n");
    invoke(&mut app, window, "test.select");
    invoke(&mut app, window, "comments.add");
    let (document, inlays) = commented_document(&app);

    let _ = window;
    assert!(
        app.perform_command(AppCommand::Verb(imba::command::Verb::Dynamic(
            std::sync::Arc::new(RemoveComment {
                comments: app.sole_family().comments(),
                document,
                key: inlays[0].0,
                annotation: None,
            }),
        )))
    );

    let doc = documents::OpenDocuments::document(app.store(), app.sole_documents(), document)
        .expect("the document");
    let byte_count = doc.text().byte_count().min(u32::MAX as usize) as u32;
    assert!(
        doc.markup()
            .all_inlays_in(0..byte_count)
            .into_iter()
            .next()
            .is_none(),
        "the card is gone"
    );
    let all_text = doc
        .editor_ids()
        .all(|editor| doc.focus(editor) == EditorFocus::Text);
    assert!(all_text, "focus falls back to the text");
}

#[test]
fn typing_lands_in_the_card_not_the_host_document() {
    let (mut app, window) = app_with_located_document("alpha beta gamma\n");
    invoke(&mut app, window, "test.select");
    invoke(&mut app, window, "comments.add");
    let (document, inlays) = commented_document(&app);
    let host_before = {
        let doc = documents::OpenDocuments::document(app.store(), app.sole_documents(), document)
            .expect("the document");
        let mut view = doc.text().view();
        let end = view.byte_count().min(u32::MAX as usize) as u32;
        view.substring(0..end)
    };

    let mut store = app.store().clone();
    let ui = UiCtx::dont_use_too_slow();
    let doc = documents::OpenDocuments::document(app.store(), app.sole_documents(), document)
        .expect("the document");
    let byte_count = doc.text().byte_count().min(u32::MAX as usize) as u32;
    let comments = doc
        .feature_markup(comments::view::comments_markup())
        .expect("the comments markup");
    let extras = [(comments::view::comments_markup(), comments)];
    let interval = editor::markup::OverlaidMarkup::new(doc.markup(), &extras)
        .all_inlays_in(0..byte_count)
        .into_iter()
        .find(|interval| interval.key == inlays[0].0)
        .expect("the card");
    let mut view = interval
        .inlay
        .view_as::<CommentView>()
        .expect("a comment card")
        .clone();
    let mut batch = imba::effect::Batch::new();
    imba::View::perform(
        &mut view,
        &mut store,
        &ui,
        CommentCommand::Editor(EditorCommand::InsertText {
            text: "looks wrong".to_owned(),
        }),
        &mut batch.effects(),
    );
    assert_eq!(view.text(), "looks wrong", "the card holds the comment");

    let workshop = crate::test_support::test_workshop(editor::theme::Theme::embedded());
    let mut pending = crate::test_support::surviving_launches(batch);
    while let Some(effect) = pending.pop() {
        let landing = crate::test_support::handle_effect(effect, &workshop);
        let mut batch = imba::effect::Batch::new();
        imba::View::perform(&mut view, &mut store, &ui, landing, &mut batch.effects());
        pending.extend(crate::test_support::surviving_launches(batch));
    }
    assert!(
        view.parsed_markdown(),
        "the card's markdown parsed through the ordinary background lane"
    );
    let host_after = {
        let doc = documents::OpenDocuments::document(app.store(), app.sole_documents(), document)
            .expect("the document");
        let mut text = doc.text().view();
        let end = text.byte_count().min(u32::MAX as usize) as u32;
        text.substring(0..end)
    };
    assert_eq!(host_before, host_after, "the host text never changes");
}


#[test]
fn sending_never_consumes_what_it_cannot_deliver() {
    let (mut app, window) = app_with_located_document("hello brave new world\n");

    let server = app.register_client(ahp_wire::client::inert());
    app.store_mut()
        .update::<ahp_wire::client::LocalHost>(|local| local.0 = Some(server));
    comments::install(&mut app.store_mut());
    invoke(&mut app, window, "test.select");
    invoke(&mut app, window, "comments.add");
    let (_, inlays) = commented_document(&app);
    assert_eq!(inlays.len(), 1);
    let ids: Vec<comments::AnnotationId> =
        comments::Comments::records(app.store(), app.sole_family().comments())
            .into_iter()
            .map(|(id, _)| id)
            .collect();
    assert_eq!(ids.len(), 1, "the record registered");

    assert!(
        app.perform_command(AppCommand::Verb(imba::command::Verb::Dynamic(
            std::sync::Arc::new(comments::view::SendComments {
                comments: app.sole_family().comments(),
                ids: ids.clone(),
            }),
        )))
    );
    let (_, inlays) = commented_document(&app);
    assert_eq!(inlays.len(), 1, "the card stands");
    assert_eq!(
        comments::Comments::records(app.store(), app.sole_family().comments()).len(),
        1,
        "the record stands"
    );

    // The send outcome lands through the model door; the batch-tail
    // comments lane drains the card work on the next tick.
    let comments = app.sole_family().comments();
    comments::Comments::sent_outcome(
        &mut app.store_mut(),
        comments,
        &ids,
        Err("wire died".to_owned()),
    );
    assert!(app.perform_command(AppCommand::Content(
        window,
        ::workbench::window::WindowCommand::Focus(::workbench::window::LayerFocus::Content),
    )));
    let (_, inlays) = commented_document(&app);
    assert_eq!(inlays.len(), 1, "a failed send keeps the card");

    comments::Comments::sent_outcome(&mut app.store_mut(), comments, &ids, Ok(()));
    assert!(app.perform_command(AppCommand::Content(
        window,
        ::workbench::window::WindowCommand::Focus(::workbench::window::LayerFocus::Content),
    )));
    assert!(
        comments::Comments::records(app.store(), app.sole_family().comments()).is_empty(),
        "the sent comment's record is consumed"
    );
    let survives = documents::OpenDocuments::list(app.store(), app.sole_documents())
        .into_iter()
        .any(|(_, entity)| {
            entity
                .document()
                .feature_markup(comments::view::comments_markup())
                .is_some_and(|comments| {
                    let byte_count =
                        entity.document().text().byte_count().min(u32::MAX as usize) as u32;
                    let extras = [(comments::view::comments_markup(), comments)];
                    editor::markup::OverlaidMarkup::new(entity.document().markup(), &extras)
                        .all_inlays_in(0..byte_count)
                        .into_iter()
                        .any(|interval| interval.inlay.view_as::<CommentView>().is_some())
                })
        });
    assert!(!survives, "the sent comment's card is consumed");
}
