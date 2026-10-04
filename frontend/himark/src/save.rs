// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{effect::AnyEffect, store::Store};

use crate::app::AppFx;
use crate::commands::WindowedCommand;
use documents::StoreDocumentEffect;
use editor::location::ResourceLocation;

pub fn save_document(save_as: bool) -> documents::save::SaveDocument {
    documents::save::SaveDocument::new(
        save_as,
        std::sync::Arc::new(changesview::hichanges::scoped),
        std::sync::Arc::new(|store, documents, from, to| {
            // The RECENTS next to the documents the save ran in — the
            // sibling of the collection the command closes over.
            if let Some(recents) =
                ahp_session::session::state::Hosts::owner_of_documents(store, documents)
                    .map(|state| state.recents())
            {
                ahp_chat::recents::RecentLocations::replace(store, recents, from, to);
            }
            crate::commands::AppRequests::push(store, std::sync::Arc::new(SyncWatches));
        }),
    )
}

/// Stores every open, modified, file-backed document — each through
/// its own lane (superseding that document's in-flight save), landing
/// one `AppCommand::DocumentStored` per store.
pub struct SaveAll;

impl WindowedCommand for SaveAll {
    fn id(&self) -> &'static str {
        "file.save-all"
    }
    fn name(&self) -> String {
        "Save All".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        save_all(store, _window, fx);
    }
}

pub(crate) fn save_all(store: &mut Store, window: ::workbench::window::WindowId, fx: &mut AppFx<'_>) {
    let Some(documents) =
        ::workbench::window::Windows::session_state(store, window).map(|state| state.documents())
    else {
        return;
    };
    let owed: Vec<(documents::DocumentId, ResourceLocation)> =
        documents::OpenDocuments::list(store, documents)
            .into_iter()
            .filter(|(_, entity)| entity.modified())
            .filter_map(|(id, entity)| {
                let location = entity.location()?.clone();
                (!documents::is_synthetic(&location) && !changesview::hichanges::scoped(&location))
                    .then_some((id, location))
            })
            .collect();
    for (id, location) in owed {
        let Some(entity) = documents::OpenDocuments::entity(store, documents, id) else {
            continue;
        };
        let document = entity.document();
        let revision = document.revision();
        let end = document.text().byte_count().min(u32::MAX as usize) as u32;
        let text = document.text().view().substring(0..end);
        let snapshot = document.text().clone();
        let previous = entity.save_token();
        let token = fx.push(AnyEffect::new(StoreDocumentEffect { location, text }).map(
            move |stored| {
                crate::app::AppCommand::at(
                    documents,
                    documents::DocumentsCommand::Stored {
                        document: id,
                        revision,
                        snapshot,
                        stored,
                    },
                )
            },
        ));
        if let Some(previous) = previous {
            fx.cancel(previous);
        }
        documents::OpenDocuments::set_save_token(store, documents, id, Some(token));
    }
}

struct SyncWatches;

impl WindowedCommand for SyncWatches {
    fn id(&self) -> &'static str {
        "file.sync-watches"
    }
    fn name(&self) -> String {
        "Sync Watches".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        if let Some(state) = ::workbench::window::Windows::session_state(store, window) {
            fx.scope(crate::app::AppCommand::Verb, |fx| {
                documents::lanes::sync_document_watches(store, state.documents(), fx)
            });
            fx.scope(crate::app::AppCommand::Verb, |fx| {
                documents::lanes::sync_stripe_bases(store, state.documents(), ui, fx)
            });
        }
    }
}

#[cfg(test)]
mod tests;
