// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use editor::{Document, EditorEffects, EditorId};
use imba::store::Store;

use crate::{DocumentId, OpenDocuments};

/// A dynamic editor command that acts on the COLLECTION: it is handed
/// the ids of the pane it fired in (docs/entities.md law 3 — the
/// pane's view is the record that holds them), so it never resolves
/// an owner through the catalog. Commands that only touch their
/// Document stay on `editor::DynamicEditorCommand`; anything that
/// registers, releases or files siblings belongs here.
pub trait DocumentCommand: Send + Sync + 'static {
    fn id(&self) -> &'static str;

    fn name(&self) -> String;

    fn offers_at(&self, location: &editor::ResourceLocation) -> bool {
        !location.is_synthetic()
    }

    #[allow(clippy::too_many_arguments)]
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::UiCtx,
        documents: imba::store::Id<OpenDocuments>,
        document_id: DocumentId,
        document: &mut Document,
        editor: EditorId,
        location: &editor::ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut EditorEffects<'_>,
    );
}

#[derive(Clone, Default)]
pub struct DocumentCommands(Vec<Arc<dyn DocumentCommand>>);

impl DocumentCommands {
    pub fn of(store: &Store) -> DocumentCommands {
        store.get::<DocumentCommands>().cloned().unwrap_or_default()
    }

    pub fn register(store: &mut Store, command: Arc<dyn DocumentCommand>) {
        store.update::<DocumentCommands>(|commands| commands.0.push(command));
    }

    pub fn find(&self, id: &str) -> Option<&Arc<dyn DocumentCommand>> {
        self.0.iter().find(|command| command.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn DocumentCommand>> {
        self.0.iter()
    }
}
