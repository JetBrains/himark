// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The save command over the documents collection: the store lane
//! per document (superseding that document's in-flight save), the
//! save-as pick, and the landing fold — shell vocabulary (what else
//! is excluded, what tracks a moved location) arrives as injected
//! roads.

use std::sync::Arc;

use editor::location::ResourceLocation;
use imba::effect::AnyEffect;
use imba::store::Store;

use crate::StoreDocumentEffect;

pub struct SaveDocument {
    save_as: bool,

    /// Locations the save never offers at, beyond synthetic ones —
    /// the shell injects its vocabulary (changeset-scoped refs) here;
    /// this crate knows none of it.
    excluded: Arc<dyn Fn(&ResourceLocation) -> bool + Send + Sync>,

    /// The save-as aftermath: the document moved to a new location —
    /// the shell updates whatever tracks locations (recents, watches).
    relocated: Arc<
        dyn Fn(&mut Store, imba::store::Id<crate::OpenDocuments>, &ResourceLocation, &ResourceLocation)
            + Send
            + Sync,
    >,
}

impl SaveDocument {
    pub fn new(
        save_as: bool,
        excluded: Arc<dyn Fn(&ResourceLocation) -> bool + Send + Sync>,
        relocated: Arc<
            dyn Fn(
                    &mut Store,
                    imba::store::Id<crate::OpenDocuments>,
                    &ResourceLocation,
                    &ResourceLocation,
                ) + Send
                + Sync,
        >,
    ) -> Self {
        Self {
            save_as,
            excluded,
            relocated,
        }
    }
}

impl crate::dynamic::DocumentCommand for SaveDocument {
    fn id(&self) -> &'static str {
        "file.save"
    }
    fn name(&self) -> String {
        "Save".to_owned()
    }
    fn offers_at(&self, location: &ResourceLocation) -> bool {
        self.save_as || (!crate::is_synthetic(location) && !(self.excluded)(location))
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        documents: imba::store::Id<crate::OpenDocuments>,
        document_id: crate::DocumentId,
        document: &mut editor::document::Document,
        _editor: editor::editor::EditorId,
        location: &editor::location::ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, editor::editor_view::EditorCommand>,
    ) {
        if !crate::dynamic::DocumentCommand::offers_at(self, location) {
            return;
        }
        if let Some(payload) = payload {
            let payload = match payload.downcast::<(bool, u64, text::text::Text)>() {
                Ok(landing) => {
                    let (stored, revision, snapshot) = *landing;
                    match stored {
                        true => crate::OpenDocuments::mark_saved(
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
                crate::OpenDocuments::set_location(
                    store,
                    documents,
                    document_id,
                    new_location.clone(),
                );
                (self.relocated)(store, documents, location, &new_location);
                Self::launch_store(store, documents, document, document_id, &new_location, fx);
            }
            return;
        }
        if crate::is_synthetic(location) {
            let mut suggested = location.name().to_owned();
            if !suggested.contains('.') {
                suggested.push_str(".md");
            }
            fx.push(
                AnyEffect::new(crate::PickSaveEffect { suggested }).map(|picked| {
                    editor::editor_view::EditorCommand::Dynamic {
                        id: "file.save",
                        payload: Some(editor::dynamic::DynPayload::new(picked)),
                    }
                }),
            );
            return;
        }
        let Some(document_id) = crate::OpenDocuments::by_location(store, documents, location)
        else {
            return;
        };
        Self::launch_store(store, documents, document, document_id, location, fx);
    }
}

impl SaveDocument {
    fn launch_store(
        store: &mut Store,
        documents: imba::store::Id<crate::OpenDocuments>,
        document: &editor::document::Document,
        document_id: crate::DocumentId,
        location: &ResourceLocation,
        fx: &mut imba::effect::Effects<'_, editor::editor_view::EditorCommand>,
    ) {
        let Some(entity) = crate::OpenDocuments::entity(store, documents, document_id) else {
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
        crate::OpenDocuments::set_save_token(store, documents, document_id, Some(token));
    }
}
