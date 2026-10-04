// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench half of comments: the card inlays — their mint,
//! refresh and removal in open documents — and the document hooks
//! that keep cards and records aligned. The collection itself is the
//! `comments` crate's; the wire is `drivers::comments`.

use std::ops::Range;
use std::sync::Arc;

use crate::{AnnotationId, CardWork, CommentRecord, Comments};
use documents::{DocumentId, text_ext::LineCol};
use editor::markup::InlayKey;
use imba::command::{Fx, Verb};
use imba::store::Store;

use crate::view::{comments_markup, CommentView};

pub fn run_card_work(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    comments: imba::store::Id<Comments>,
    work: CardWork,
    fx: &mut Fx<'_>,
) {
    if let Some(documents) = Comments::documents_of(store, comments) {
        for (document, key) in work.dead {
            remove_card(store, documents, ui, document, key, fx);
        }
    }
    if work.settle {
        settle_cards(store, ui, comments, fx);
    }
}

/// Behind a landing, after the lease: unsynced records reach the live
/// feed, and every record whose document is open settles into a card.
fn settle_cards(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    comments: imba::store::Id<Comments>,
    fx: &mut Fx<'_>,
) {
    let records: Vec<(AnnotationId, CommentRecord)> = Comments::records(store, comments);
    for (id, record) in records {
        match Comments::card(store, comments, &id) {
            Some(_) => refresh_card(store, ui, comments, &id, &record),
            None => {
                if let Some(document) =
                    Comments::documents_of(store, comments).and_then(|documents| {
                        documents::OpenDocuments::by_location(store, documents, &record.location)
                    })
                {
                    materialize(store, ui, comments, &id, &record, document, fx);
                }
            }
        }
    }
}

fn remove_card(
    store: &mut Store,
    documents: imba::store::Id<documents::OpenDocuments>,
    ui: &imba::ui::UiCtx,
    document: DocumentId,
    key: InlayKey,
    fx: &mut Fx<'_>,
) {
    let Some(mut doc) = documents::OpenDocuments::document(store, documents, document) else {
        return;
    };
    let fonts = editor::env::Fonts::of(store)();
    let theme = editor::env::Themes::of(store);
    fx.scope(
        move |command| {
            Verb::at(
                documents,
                documents::DocumentsCommand::Editor(document, command),
            )
        },
        |fx| doc.remove_inlay(key, store, ui, &fonts, &theme, fx),
    );
    documents::OpenDocuments::put_document(store, documents, document, doc);
}

/// The document observer, WIRED: minted by the session ceremony with
/// the cards' collection in hand, installed SCOPED to the session's
/// documents — it fires only for its own collection and dies with it
/// (docs/entities.md law 4).
pub struct CommentsHook {
    pub comments: imba::store::Id<Comments>,
}

impl documents::DocumentHook for CommentsHook {
    fn opened(
        &self,
        store: &mut Store,
        _documents: imba::store::Id<documents::OpenDocuments>,
        document: DocumentId,
        location: Option<&editor::location::ResourceLocation>,
    ) {
        let Some(location) = location else {
            return;
        };
        // Scoped install: this hook fires only for its own session's
        // documents, and the cards' collection is its own record.
        let comments = self.comments;
        if Comments::owes_cards_at(store, comments, location) {
            imba::command::Requests::push(store, Arc::new(MaterializeFor { comments, document }));
        }
    }

    fn closing(
        &self,
        store: &mut Store,
        _documents: imba::store::Id<documents::OpenDocuments>,
        document: DocumentId,
        _location: Option<&editor::location::ResourceLocation>,
        doc: &editor::document::Document,
    ) {
        let comments = self.comments;
        for (id, key) in Comments::cards_in(store, comments, document) {
            if let Some(range) = live_card_range(doc, key) {
                Comments::update_record(store, comments, &id, move |record| {
                    record.range = Some(range);
                });
            }
            Comments::card_closed(store, comments, &id);
        }
    }
}

struct MaterializeFor {
    comments: imba::store::Id<Comments>,
    document: DocumentId,
}

impl imba::command::DynamicCommand for MaterializeFor {
    fn id(&self) -> &'static str {
        "comments.materialize"
    }
    fn name(&self) -> String {
        "Materialize Comments".to_owned()
    }
    fn perform(&self, store: &mut Store, ui: &imba::ui::UiCtx, fx: &mut Fx<'_>) {
        let Some(documents) = Comments::documents_of(store, self.comments) else {
            return;
        };
        let Some(location) = documents::OpenDocuments::location(store, documents, self.document)
        else {
            return;
        };
        let owed: Vec<(AnnotationId, CommentRecord)> = Comments::records(store, self.comments)
            .into_iter()
            .filter(|(id, record)| {
                record.location == location && Comments::card(store, self.comments, id).is_none()
            })
            .collect();
        for (id, record) in owed {
            materialize(store, ui, self.comments, &id, &record, self.document, fx);
        }
    }
}

/// The card's live range read off the CLOSING row itself — the hook
/// is handed the document; a store read here would be lease
/// reentrancy (docs/entities.md law 5).
fn live_card_range(doc: &editor::document::Document, key: InlayKey) -> Option<Range<LineCol>> {
    let markup = doc.feature_markup(comments_markup())?;

    let (range, _) = markup.inlay_at_key(comments_markup(), key)?;
    let mut view = doc.text().view();
    Some(
        documents::text_ext::line_col_at(&mut view, range.start as usize)
            ..documents::text_ext::line_col_at(&mut view, range.end as usize),
    )
}

fn materialize(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    comments: imba::store::Id<Comments>,
    id: &AnnotationId,
    record: &CommentRecord,
    document: DocumentId,
    fx: &mut Fx<'_>,
) {
    let Some(documents) = Comments::documents_of(store, comments) else {
        return;
    };
    let Some(mut doc) = documents::OpenDocuments::document(store, documents, document) else {
        return;
    };
    let fonts = editor::env::Fonts::of(store)();
    let theme = editor::env::Themes::of(store);
    let byte_count = doc.text().byte_count().min(u32::MAX as usize) as u32;
    let range = match &record.range {
        Some(range) => {
            let mut view = doc.text().view();
            let start =
                documents::text_ext::offset_at(&mut view, range.start).min(byte_count as usize) as u32;
            let end = documents::text_ext::offset_at(&mut view, range.end).min(byte_count as usize) as u32;
            start..end.max(start)
        }
        None => 0..byte_count,
    };
    let view = CommentView::materialized(
        comments,
        Some(document),
        crate::view::FALLBACK_WIDTH,
        store,
        ui,
        &fonts,
        &theme,
        id.clone(),
        record.own_entry().map(|entry| entry.text.clone()),
        record.foreign_texts(),
        record.thread_stamp,
    );
    let markup = comments_markup();
    doc.ensure_document_markup(markup);
    let mut minted = None;
    fx.scope(
        move |command| {
            Verb::at(
                documents,
                documents::DocumentsCommand::Editor(document, command),
            )
        },
        |fx| {
            let key = doc.push_inlay(
                markup,
                range.clone(),
                editor::markup::Inlay::new(editor::markup::InlayMode::Under, view.clone()),
                store,
                ui,
                &fonts,
                &theme,
                fx,
            );
            doc.swap_inlay(
                key,
                range.clone(),
                editor::markup::Inlay::new(editor::markup::InlayMode::Under, view.clone().keyed(key)),
            );
            minted = Some(key);
        },
    );
    documents::OpenDocuments::put_document(store, documents, document, doc);
    if let Some(key) = minted {
        Comments::card_born(store, comments, id, document, key);
    }
}

fn refresh_card(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    comments: imba::store::Id<Comments>,
    id: &AnnotationId,
    record: &CommentRecord,
) {
    let Some((document, key)) = Comments::card(store, comments, id) else {
        return;
    };
    let Some(documents) = Comments::documents_of(store, comments) else {
        return;
    };
    let Some(doc) = documents::OpenDocuments::document_ref(store, documents, document) else {
        return;
    };
    let Some(markup) = doc.feature_markup(comments_markup()) else {
        return;
    };

    let Some((range, inlay)) = markup.inlay_at_key(comments_markup(), key) else {
        return;
    };
    let view = inlay.view_as::<CommentView>();

    if !view.is_some_and(|view| view.thread_stamp() != record.thread_stamp) {
        return;
    }
    let live_text = view.map(|view| view.text_rope());
    let fonts = editor::env::Fonts::of(store)();
    let theme = editor::env::Themes::of(store);
    let rebuilt = CommentView::materialized(
        comments,
        Some(document),
        crate::view::FALLBACK_WIDTH,
        store,
        ui,
        &fonts,
        &theme,
        id.clone(),
        live_text.or(record.own_entry().map(|entry| entry.text.clone())),
        record.foreign_texts(),
        record.thread_stamp,
    )
    .keyed(key);
    let mut doc =
        documents::OpenDocuments::document(store, documents, document).expect("held above");
    doc.swap_inlay(
        key,
        range,
        editor::markup::Inlay::new(editor::markup::InlayMode::Under, rebuilt),
    );
    documents::OpenDocuments::put_document(store, documents, document, doc);
}
