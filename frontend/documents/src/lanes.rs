// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Verb-flavored sync lanes over the documents collection: the
//! watch lane, the refetch, and the stripe-base ask — landings route
//! At the collection, no window, no app command.

/// The watch lane, Verb-flavored: landings route At the documents
/// collection — no window, no app command.
pub fn sync_document_watches(
    store: &mut imba::store::Store,
    documents: imba::store::Id<crate::OpenDocuments>,
    fx: &mut imba::command::Fx<'_>,
) {
    crate::watch::sync_document_watches(store, documents, fx, move |document, subscription| {
        imba::command::Verb::at(
            documents,
            crate::DocumentsCommand::Watched(document, subscription),
        )
    });
}

/// Refetch one document from its host — the landing routes At the
/// collection like every other watch landing.
pub fn refetch_document(
    store: &mut imba::store::Store,
    documents_id: imba::store::Id<crate::OpenDocuments>,
    document: crate::DocumentId,
    fx: &mut imba::command::Fx<'_>,
) {
    let Some(location) = crate::OpenDocuments::location(store, documents_id, document) else {
        return;
    };
    if crate::is_synthetic(&location) {
        return;
    }
    let serial = crate::OpenDocuments::stamp_refetch(store, documents_id, document);
    let _ = fx.push(
        imba::effect::AnyEffect::new(crate::FetchDocumentEffect { location }).map(
            move |text| {
                imba::command::Verb::at(
                    documents_id,
                    crate::DocumentsCommand::Refetched {
                        document,
                        serial,
                        text,
                    },
                )
            },
        ),
    );
}

/// Ask bases for every registered document that has none yet — the
/// landings route At the collection (himark re-exports this as its
/// own lane door).
pub fn sync_stripe_bases(
    store: &mut imba::store::Store,
    documents: imba::store::Id<crate::OpenDocuments>,
    ui: &imba::ui::UiCtx,
    fx: &mut imba::command::Fx<'_>,
) {
    fx.scope(
        move |command| imba::command::Verb::at(documents, command),
        |fx| {
            crate::diffs::sync_stripe_bases(
                store,
                documents,
                fx,
                |store, document, base, fx| {
                    crate::diffs::land_base_located(store, documents, ui, document, base, fx);
                },
            );
        },
    );
}

/// Refetch the document a WATCH fired for — the landing routes At
/// the collection like every other watch landing.
pub fn refetch_watched(
    store: &mut imba::store::Store,
    documents: imba::store::Id<crate::OpenDocuments>,
    subscription: crate::watch::Subscription,
    fx: &mut imba::command::Fx<'_>,
) {
    crate::watch::refetch_watched(store, documents, subscription, fx, move |document, serial, text| {
        imba::command::Verb::at(
            documents,
            crate::DocumentsCommand::Refetched {
                document,
                serial,
                text,
            },
        )
    });
}
