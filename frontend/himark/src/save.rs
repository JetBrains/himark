// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{effect::AnyEffect, store::Store};

use crate::app::AppFx;
use crate::commands::DynamicCommand;
use documents::StoreDocumentEffect;
use editor::location::ResourceLocation;

pub struct SaveDocument {
    save_as: bool,
}

impl SaveDocument {
    pub fn existing_files() -> Self {
        Self { save_as: false }
    }

    pub fn with_save_as() -> Self {
        Self { save_as: true }
    }
}

impl documents::dynamic::DocumentCommand for SaveDocument {
    fn id(&self) -> &'static str {
        "file.save"
    }
    fn name(&self) -> String {
        "Save".to_owned()
    }
    fn offers_at(&self, location: &ResourceLocation) -> bool {
        self.save_as || (!documents::is_synthetic(location) && !changesview::hichanges::scoped(location))
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        documents: imba::store::Id<documents::OpenDocuments>,
        document_id: documents::DocumentId,
        document: &mut editor::document::Document,
        _editor: editor::editor::EditorId,
        location: &editor::location::ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, editor::editor_view::EditorCommand>,
    ) {
        if !documents::dynamic::DocumentCommand::offers_at(self, location) {
            return;
        }
        if let Some(payload) = payload {
            let payload = match payload.downcast::<(bool, u64, text::text::Text)>() {
                Ok(landing) => {
                    let (stored, revision, snapshot) = *landing;
                    match stored {
                        true => documents::OpenDocuments::mark_saved(
                            store,
                            documents,
                            document_id,
                            revision,
                            snapshot,
                        ),
                        false => eprintln!("[himark] store failed for an open document"),
                    }
                    return;
                }
                Err(payload) => payload,
            };

            if let Ok(picked) = payload.downcast::<Option<ResourceLocation>>() {
                let Some(new_location) = *picked else {
                    return;
                };
                documents::OpenDocuments::set_location(
                    store,
                    documents,
                    document_id,
                    new_location.clone(),
                );
                // The RECENTS next to the documents the save ran in —
                // the sibling of the collection the command closes over.
                if let Some(recents) = ahp_session::session::state::Hosts::owner_of_documents(store, documents)
                    .map(|state| state.recents())
                {
                    ahp_chat::recents::RecentLocations::replace(store, recents, location, &new_location);
                }

                crate::commands::AppRequests::push(store, std::sync::Arc::new(SyncWatches));
                Self::launch_store(store, documents, document, document_id, &new_location, fx);
            }
            return;
        }
        if documents::is_synthetic(location) {
            let mut suggested = location.name().to_owned();
            if !suggested.contains('.') {
                suggested.push_str(".md");
            }
            fx.push(
                AnyEffect::new(documents::PickSaveEffect { suggested }).map(|picked| {
                    editor::editor_view::EditorCommand::Dynamic {
                        id: "file.save",
                        payload: Some(editor::dynamic::DynPayload::new(picked)),
                    }
                }),
            );
            return;
        }
        let Some(document_id) = documents::OpenDocuments::by_location(store, documents, location)
        else {
            return;
        };
        Self::launch_store(store, documents, document, document_id, location, fx);
    }
}

impl SaveDocument {
    fn launch_store(
        store: &mut Store,
        documents: imba::store::Id<documents::OpenDocuments>,
        document: &editor::document::Document,
        document_id: documents::DocumentId,
        location: &ResourceLocation,
        fx: &mut imba::effect::Effects<'_, editor::editor_view::EditorCommand>,
    ) {
        let Some(entity) = documents::OpenDocuments::entity(store, documents, document_id) else {
            return;
        };

        let revision = document.revision();
        let end = document.text().byte_count().min(u32::MAX as usize) as u32;
        let text = document.text().view().substring(0..end);

        let snapshot = document.text().clone();
        let previous = entity.save_token();
        let token = fx.push(
            AnyEffect::new(StoreDocumentEffect {
                location: location.clone(),
                text,
            })
            .map(move |stored| editor::editor_view::EditorCommand::Dynamic {
                id: "file.save",
                payload: Some(editor::dynamic::DynPayload::new((stored, revision, snapshot))),
            }),
        );
        if let Some(previous) = previous {
            fx.cancel(previous);
        }
        documents::OpenDocuments::set_save_token(store, documents, document_id, Some(token));
    }
}

/// Stores every open, modified, file-backed document — each through
/// its own lane (superseding that document's in-flight save), landing
/// one `AppCommand::DocumentStored` per store.
pub struct SaveAll;

impl DynamicCommand for SaveAll {
    fn id(&self) -> &'static str {
        "file.save-all"
    }
    fn name(&self) -> String {
        "Save All".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::app::Application,
        store: &mut Store,
        _window: crate::window::WindowId,
        fx: &mut AppFx<'_>,
    ) {
        save_all(store, _window, fx);
    }
}

pub(crate) fn save_all(store: &mut Store, window: crate::window::WindowId, fx: &mut AppFx<'_>) {
    let Some(documents) =
        crate::window::Windows::session_state(store, window).map(|state| state.documents())
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

impl DynamicCommand for SyncWatches {
    fn id(&self) -> &'static str {
        "file.sync-watches"
    }
    fn name(&self) -> String {
        "Sync Watches".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::app::Application,
        store: &mut Store,
        window: crate::window::WindowId,
        fx: &mut AppFx<'_>,
    ) {
        if let Some(state) = crate::window::Windows::session_state(store, window) {
            crate::watch::sync_document_watches(store, state.documents(), fx);
            fx.scope(crate::app::AppCommand::Verb, |fx| {
                documents::lanes::sync_stripe_bases(store, state.documents(), &app.ui_ctx(), fx)
            });
        }
    }
}

#[cfg(test)]
mod tests;
