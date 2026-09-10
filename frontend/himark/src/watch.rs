use imba::store::Store;

use crate::{AppCommand, AppFx};

pub use documents::watch::{
    FileChanged, FilesChanged, RefetchDiffEffect, RefetchDiffHandler, RefetchRebase,
    SubscribeEffect, Subscription, UnsubscribeEffect, Watching,
};

pub fn sync_document_watches(store: &mut Store, fx: &mut AppFx<'_>) {
    documents::watch::sync_document_watches(store, fx, |document, subscription| {
        AppCommand::Watched(document, subscription)
    });
}

pub(crate) fn refetch_watched(store: &mut Store, subscription: Subscription, fx: &mut AppFx<'_>) {
    documents::watch::refetch_watched(store, subscription, fx, |document, serial, text| {
        AppCommand::Refetched {
            document,
            serial,
            text,
        }
    });
}

pub(crate) fn apply_refetched(
    store: &mut Store,
    document_id: crate::DocumentId,
    serial: u64,
    text: Option<String>,
    fx: &mut AppFx<'_>,
) {
    documents::watch::apply_refetched(
        store,
        document_id,
        serial,
        text,
        fx,
        |document, base_revision, serial, rebase| AppCommand::RefetchDiffed {
            document,
            base_revision,
            serial,
            rebase,
        },
    );
}

pub fn refetch_document(store: &mut Store, document: crate::DocumentId, fx: &mut AppFx<'_>) {
    let Some(location) = documents::OpenDocuments::location(store, document) else {
        return;
    };
    if crate::is_synthetic(&location) {
        return;
    }
    let serial = documents::OpenDocuments::stamp_refetch(store, document);
    let _ = fx.push(imba::effect::AnyEffect::new(crate::FetchDocumentEffect { location }).map(
        move |text| AppCommand::Refetched {
            document,
            serial,
            text,
        },
    ));
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
        refetch_document(store, document, fx);
    }
}

#[cfg(test)]
mod tests;
