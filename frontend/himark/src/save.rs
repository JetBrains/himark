use imba::{effect::AnyEffect, store::Store};

use crate::{AppFx, DynamicCommand, ResourceLocation, StoreDocumentEffect};

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

impl crate::DynamicEditorCommand for SaveDocument {
    fn id(&self) -> &'static str {
        "file.save"
    }
    fn name(&self) -> String {
        "Save".to_owned()
    }
    fn offers_at(&self, location: &ResourceLocation) -> bool {
        self.save_as || (!crate::is_synthetic(location) && !crate::hichanges::scoped(location))
    }
    fn perform(
        &self,
        store: &mut Store,
        document: &mut crate::Document,
        _editor: crate::EditorId,
        location: &crate::ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, crate::EditorCommand>,
    ) {
        if !self.offers_at(location) {
            return;
        }
        if let Some(payload) = payload {
            let payload = match payload.downcast::<(bool, u64, crate::Text)>() {
                Ok(landing) => {
                    let Some(document_id) = crate::OpenDocuments::by_location(store, location)
                    else {
                        return;
                    };
                    let (stored, revision, snapshot) = *landing;
                    match stored {
                        true => {
                            crate::OpenDocuments::mark_saved(store, document_id, revision, snapshot)
                        }
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
                let Some(document_id) = crate::OpenDocuments::by_location(store, location) else {
                    return;
                };
                crate::OpenDocuments::set_location(store, document_id, new_location.clone());
                crate::RecentLocations::replace(store, location, &new_location);

                crate::AppRequests::push(store, std::sync::Arc::new(SyncWatches));
                Self::launch_store(store, document, document_id, &new_location, fx);
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
                    crate::EditorCommand::Dynamic {
                        id: "file.save",
                        payload: Some(Box::new(picked)),
                    }
                }),
            );
            return;
        }
        let Some(document_id) = crate::OpenDocuments::by_location(store, location) else {
            return;
        };
        Self::launch_store(store, document, document_id, location, fx);
    }
}

impl SaveDocument {
    fn launch_store(
        store: &mut Store,
        document: &crate::Document,
        document_id: crate::DocumentId,
        location: &ResourceLocation,
        fx: &mut imba::effect::Effects<'_, crate::EditorCommand>,
    ) {
        let Some(entity) = crate::OpenDocuments::entity(store, document_id) else {
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
            .map(move |stored| crate::EditorCommand::Dynamic {
                id: "file.save",
                payload: Some(Box::new((stored, revision, snapshot))),
            }),
        );
        if let Some(previous) = previous {
            fx.cancel(previous);
        }
        crate::OpenDocuments::set_save_token(store, document_id, Some(token));
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
        _app: &mut crate::Application,
        store: &mut Store,
        _window: crate::WindowId,
        fx: &mut AppFx<'_>,
    ) {
        crate::sync_document_watches(store, fx);
        crate::sync_stripe_bases(store, fx);
    }
}

#[cfg(test)]
mod tests;
