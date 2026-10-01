// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;

use crate::{AppCommand, AppFx};

pub use documents::watch::{
    FileChanged, FilesChanged, RefetchDiffEffect, RefetchDiffHandler, RefetchRebase,
    SubscribeEffect, Subscription, UnsubscribeEffect, Watching,
};

use crate::app::DocumentsCommand;

pub fn sync_document_watches(
    store: &mut Store,
    documents: imba::store::Id<crate::OpenDocuments>,
    fx: &mut AppFx<'_>,
) {
    documents::watch::sync_document_watches(store, documents, fx, move |document, subscription| {
        AppCommand::At(documents, DocumentsCommand::Watched(document, subscription))
    });
}

pub(crate) fn refetch_watched(
    store: &mut Store,
    documents: imba::store::Id<crate::OpenDocuments>,
    subscription: Subscription,
    fx: &mut AppFx<'_>,
) {
    documents::watch::refetch_watched(store, documents, subscription, fx, move |document, serial, text| {
        AppCommand::At(
            documents,
            DocumentsCommand::Refetched {
                document,
                serial,
                text,
            },
        )
    });
}

pub(crate) fn apply_refetched(
    store: &mut Store,
    documents: imba::store::Id<crate::OpenDocuments>,
    document_id: crate::DocumentId,
    serial: u64,
    text: Option<String>,
    fx: &mut AppFx<'_>,
) {
    documents::watch::apply_refetched(
        store,
        documents,
        document_id,
        serial,
        text,
        fx,
        move |document, base_revision, serial, rebase| {
            AppCommand::At(
                documents,
                DocumentsCommand::RefetchDiffed {
                    document,
                    base_revision,
                    serial,
                    rebase,
                },
            )
        },
    );
}

pub fn refetch_document(
    store: &mut Store,
    documents_id: imba::store::Id<crate::OpenDocuments>,
    document: crate::DocumentId,
    fx: &mut AppFx<'_>,
) {
    let Some(location) = documents::OpenDocuments::location(store, documents_id, document) else {
        return;
    };
    if crate::is_synthetic(&location) {
        return;
    }
    let serial = documents::OpenDocuments::stamp_refetch(store, documents_id, document);
    let _ = fx.push(
        imba::effect::AnyEffect::new(crate::FetchDocumentEffect { location }).map(move |text| {
            AppCommand::At(
                documents_id,
                DocumentsCommand::Refetched {
                    document,
                    serial,
                    text,
                },
            )
        }),
    );
}

pub struct ReloadDocument;

impl crate::DynamicCommand for ReloadDocument {
    fn id(&self) -> &'static str {
        "file.reload"
    }

    fn name(&self) -> String {
        "Reload from Disk".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let Some(document) = crate::Windows::window_ref(store, window)
            .and_then(|entity| entity.focused_document_id())
        else {
            return;
        };
        let Some(family) = crate::Windows::session_family(store, window) else {
            return;
        };
        refetch_document(store, family.documents(), document, fx);
    }
}

#[cfg(test)]
mod tests;
