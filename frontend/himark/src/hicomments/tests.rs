// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

use std::ops::Range;

use crate::test_document::plain_document;
use crate::{AppCommand, AppExt, AppFonts, Application, Caret, MultiCaret, OpenedDocument};

fn document_location(name: &str) -> crate::ResourceLocation {
    crate::ResourceLocation::new(
        crate::ResourceType::document(),
        crate::Authority::new("local"),
        vec!["project".to_owned(), name.to_owned()],
    )
}

struct SelectRange(Range<u32>);

impl crate::DynamicEditorCommand for SelectRange {
    fn id(&self) -> &'static str {
        "test.select"
    }
    fn name(&self) -> String {
        "Test: Select".to_owned()
    }
    fn perform(
        &self,
        _store: &mut Store,
        _ui: &imba::UiCtx,
        document: &mut Document,
        editor: crate::EditorId,
        _location: &crate::ResourceLocation,
        _payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        _fx: &mut imba::effect::Effects<'_, EditorCommand>,
    ) {
        document.set_carets(
            editor,
            MultiCaret::one(Caret::selecting(self.0.start, self.0.end)),
        );
    }
}

fn app_with_located_document(source: &str) -> (Application, crate::WindowId) {
    let mut app = Application::new(AppFonts::embedded());
    app.register_syntax_languages(himarkdown::markdown_languages(crate::SyntaxLanguages::new()));
    // `comments.add` arrives via the family ceremony (a scoped,
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

fn invoke(app: &mut Application, window: crate::WindowId, id: &str) {
    let command = crate::palette_commands(app.store(), &app.ui_handle(), window)
        .into_iter()
        .find(|presentable| presentable.id == id)
        .expect("the located editor offers the command")
        .command;
    assert!(app.perform_command(command));
}

fn commented_document(app: &Application) -> (crate::DocumentId, Vec<(InlayKey, Range<u32>)>) {
    for (id, entity) in crate::OpenDocuments::list(app.store(), app.sole_documents()) {
        let document = entity.document();
        let byte_count = document.text().byte_count().min(u32::MAX as usize) as u32;

        let Some(comments) = document.feature_markup(crate::hicomments::comments_markup()) else {
            continue;
        };
        let extras = [(crate::hicomments::comments_markup(), comments)];
        let inlays: Vec<(InlayKey, Range<u32>)> =
            crate::OverlaidMarkup::new(document.markup(), &extras)
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

    for (_, entity) in crate::OpenDocuments::list(app.store(), app.sole_documents()) {
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
    let document = crate::OpenDocuments::document(app.store(), app.sole_documents(), document)
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

    assert!(app.perform_command(AppCommand::Dynamic(
        window,
        std::sync::Arc::new(RemoveComment {
            comments: app.sole_family().comments(),
            document,
            key: inlays[0].0,
            annotation: None,
        }),
    )));

    let doc = crate::OpenDocuments::document(app.store(), app.sole_documents(), document)
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
        let doc = crate::OpenDocuments::document(app.store(), app.sole_documents(), document)
            .expect("the document");
        let mut view = doc.text().view();
        let end = view.byte_count().min(u32::MAX as usize) as u32;
        view.substring(0..end)
    };

    let mut store = app.store().clone();
    let ui = UiCtx::dont_use_too_slow();
    let doc = crate::OpenDocuments::document(app.store(), app.sole_documents(), document)
        .expect("the document");
    let byte_count = doc.text().byte_count().min(u32::MAX as usize) as u32;
    let comments = doc
        .feature_markup(crate::hicomments::comments_markup())
        .expect("the comments markup");
    let extras = [(crate::hicomments::comments_markup(), comments)];
    let interval = crate::OverlaidMarkup::new(doc.markup(), &extras)
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

    let workshop = crate::test_support::test_workshop(crate::Theme::embedded());
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
        let doc = crate::OpenDocuments::document(app.store(), app.sole_documents(), document)
            .expect("the document");
        let mut text = doc.text().view();
        let end = text.byte_count().min(u32::MAX as usize) as u32;
        text.substring(0..end)
    };
    assert_eq!(host_before, host_after, "the host text never changes");
}

struct InertSeat;

macro_rules! unreached {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $out:ty;)*) => {
        $(fn $name(&self, $($arg: $ty),*) -> $out {
            $(let _ = $arg;)*
            unreachable!("the comment tests never reach the seat")
        })*
    };
}

impl crate::higent::AhpServer for InertSeat {
    unreached! {
        connect() -> crate::higent::SeatFuture<Result<crate::higent::RootInfo, String>>;
        list_sessions(cursor: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::SessionsPage, String>>;
        poll_root() -> crate::higent::SeatFuture<Vec<crate::higent::ServerEvent>>;
        create_session(dirs: Vec<String>, options: crate::higent::SessionOptions) -> crate::higent::SeatFuture<Result<crate::higent::SessionUri, String>>;
        resolve_session_config(working_directory: Option<String>, config: Option<serde_json::Map<String, serde_json::Value>>) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::commands::ResolveSessionConfigResult, String>>;
        dispose_session(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<(), String>>;
        subscribe_session(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::SessionState, String>>;
        poll_session(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Vec<crate::higent::ahp_types::actions::StateAction>>;
        create_chat(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<crate::higent::ChatUri, String>>;
        subscribe_chat(chat: crate::higent::ChatUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::ChatState, String>>;
        fetch_turns(chat: crate::higent::ChatUri, cursor: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::TurnsPage, String>>;
        start_turn(chat: crate::higent::ChatUri, text: String, attachments: Option<Vec<crate::higent::ahp_types::state::MessageAttachment>>, model: Option<crate::higent::ahp_types::state::ModelSelection>) -> crate::higent::SeatFuture<Result<(), String>>;
        poll_chat(chat: crate::higent::ChatUri) -> crate::higent::SeatFuture<Vec<crate::higent::ahp_types::actions::StateAction>>;
        cancel_turn(chat: crate::higent::ChatUri, turn: crate::higent::TurnId) -> crate::higent::SeatFuture<()>;
        dispatch_action(chat: crate::higent::ChannelUri, action: crate::higent::ahp_types::actions::StateAction) -> crate::higent::SeatFuture<Result<(), String>>;
        read_file_edit(before: Option<String>, after: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::FileEditContents, String>>;
        resource_read(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri) -> crate::higent::SeatFuture<Option<String>>;
        resource_write(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri, text: String) -> crate::higent::SeatFuture<bool>;
        resource_list(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri) -> crate::higent::SeatFuture<Option<Vec<(String, bool)>>>;
        resource_watch(session: crate::higent::SessionUri, uri: crate::higent::ResourceUri, events: std::sync::Arc<dyn Fn() + Send + Sync>) -> crate::higent::SeatFuture<Option<crate::higent::WatchHandle>>;
        resource_unwatch(handle: crate::higent::WatchHandle) -> crate::higent::SeatFuture<()>;
        search(session: crate::higent::SessionUri, ask: crate::higent::SearchAsk) -> crate::higent::SeatFuture<Option<crate::higent::SearchResult>>;
        terminal_input(channel: &crate::higent::ChannelUri, data: String) -> ();
        terminal_resize(channel: &crate::higent::ChannelUri, cols: u16, rows: u16) -> ();
        terminal_dispose(channel: &crate::higent::ChannelUri) -> ();
        subscribe_changeset(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::ChangesetState, String>>;
        poll_changeset(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Vec<crate::higent::ahp_types::actions::StateAction>>;
        unsubscribe_changeset(channel: &crate::higent::ChannelUri) -> ();
        subscribe_annotations(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Result<crate::higent::ahp_types::state::AnnotationsState, String>>;
        poll_annotations(session: crate::higent::SessionUri) -> crate::higent::SeatFuture<Vec<crate::higent::ahp_types::actions::StateAction>>;
        dispatch_annotations(session: &crate::higent::SessionUri, action: crate::higent::ahp_types::actions::StateAction) -> ();
        unsubscribe_annotations(session: &crate::higent::SessionUri) -> ();
        open_document(session: crate::higent::SessionUri, uri: Option<crate::higent::ResourceUri>, text: Option<String>) -> crate::higent::SeatFuture<Result<crate::higent::seat::OpenDocumentResult, String>>;
        subscribe_document(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Result<crate::higent::seat::DocumentState, String>>;
        poll_document(channel: crate::higent::ChannelUri) -> crate::higent::SeatFuture<Vec<crate::higent::seat::DocumentApplied>>;
        dispatch_document(channel: &crate::higent::ChannelUri, action: crate::higent::seat::DocumentApplied) -> ();
        unsubscribe_document(channel: &crate::higent::ChannelUri) -> crate::higent::SeatFuture<()>;
        lsp(session: crate::higent::SessionUri, method: String, params: serde_json::Value) -> crate::higent::SeatFuture<Result<serde_json::Value, String>>;
    }

    fn terminal_open(
        &self,
        _session: crate::higent::SessionUri,
        _channel: crate::higent::ChannelUri,
        _cwd: Option<String>,
        _cols: u16,
        _rows: u16,
        _events: std::sync::Arc<dyn Fn(crate::higent::TerminalEvent) + Send + Sync>,
    ) -> crate::higent::SeatFuture<Option<crate::higent::TerminalHandle>> {
        unreachable!("the comment tests never reach the seat")
    }
}

#[test]
fn sending_never_consumes_what_it_cannot_deliver() {
    let (mut app, window) = app_with_located_document("hello brave new world\n");

    let server = app.register_seat(std::sync::Arc::new(InertSeat));
    app.store_mut()
        .update::<crate::higent::LocalHost>(|local| local.0 = Some(server));
    crate::hicomments::Comments::install(&mut app.store_mut());
    invoke(&mut app, window, "test.select");
    invoke(&mut app, window, "comments.add");
    let (_, inlays) = commented_document(&app);
    assert_eq!(inlays.len(), 1);
    let ids: Vec<crate::hicomments::AnnotationId> =
        crate::hicomments::Comments::records(app.store(), app.sole_family().comments())
            .into_iter()
            .map(|(id, _)| id)
            .collect();
    assert_eq!(ids.len(), 1, "the record registered");

    assert!(app.perform_command(AppCommand::Dynamic(
        window,
        std::sync::Arc::new(crate::hicomments::SendComments {
            comments: app.sole_family().comments(),
            ids: ids.clone(),
        }),
    )));
    let (_, inlays) = commented_document(&app);
    assert_eq!(inlays.len(), 1, "the card stands");
    assert_eq!(
        crate::hicomments::Comments::records(app.store(), app.sole_family().comments()).len(),
        1,
        "the record stands"
    );

    // The send outcome lands through the model door; the batch-tail
    // comments lane drains the card work on the next tick.
    let comments = app.sole_family().comments();
    crate::hicomments::Comments::sent_outcome(
        &mut app.store_mut(),
        comments,
        &ids,
        Err("wire died".to_owned()),
    );
    assert!(app.perform_command(AppCommand::Content(
        window,
        crate::WindowCommand::Focus(crate::LayerFocus::Content),
    )));
    let (_, inlays) = commented_document(&app);
    assert_eq!(inlays.len(), 1, "a failed send keeps the card");

    crate::hicomments::Comments::sent_outcome(&mut app.store_mut(), comments, &ids, Ok(()));
    assert!(app.perform_command(AppCommand::Content(
        window,
        crate::WindowCommand::Focus(crate::LayerFocus::Content),
    )));
    assert!(
        crate::hicomments::Comments::records(app.store(), app.sole_family().comments()).is_empty(),
        "the sent comment's record is consumed"
    );
    let survives = crate::OpenDocuments::list(app.store(), app.sole_documents())
        .into_iter()
        .any(|(_, entity)| {
            entity
                .document()
                .feature_markup(crate::hicomments::comments_markup())
                .is_some_and(|comments| {
                    let byte_count =
                        entity.document().text().byte_count().min(u32::MAX as usize) as u32;
                    let extras = [(crate::hicomments::comments_markup(), comments)];
                    crate::OverlaidMarkup::new(entity.document().markup(), &extras)
                        .all_inlays_in(0..byte_count)
                        .into_iter()
                        .any(|interval| interval.inlay.view_as::<CommentView>().is_some())
                })
        });
    assert!(!survives, "the sent comment's card is consumed");
}
