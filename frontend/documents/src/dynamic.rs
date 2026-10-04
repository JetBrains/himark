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
pub struct DocumentCommands {
    global: Vec<Arc<dyn DocumentCommand>>,

    /// Commands WIRED to one collection's panes — installed by the
    /// family ceremony with their sibling ids in hand (docs/entities.md
    /// law 4), retired with the collection. The scope key is this
    /// crate's own id, so the registry stays layering-clean.
    scoped: rpds::HashTrieMapSync<imba::store::Id<OpenDocuments>, Vec<Arc<dyn DocumentCommand>>>,
}

impl DocumentCommands {
    pub fn of(store: &Store) -> DocumentCommands {
        crate::Registry::of(store)
            .map(|registry| registry.commands.clone())
            .unwrap_or_default()
    }

    pub fn register(store: &mut Store, command: Arc<dyn DocumentCommand>) {
        crate::Registry::update(store, |registry| registry.commands.global.push(command));
    }

    pub fn register_scoped(
        store: &mut Store,
        scope: imba::store::Id<OpenDocuments>,
        command: Arc<dyn DocumentCommand>,
    ) {
        crate::Registry::update(store, |registry| {
            let mut entries = registry
                .commands
                .scoped
                .get(&scope)
                .cloned()
                .unwrap_or_default();
            entries.push(command);
            registry.commands.scoped.insert_mut(scope, entries);
        });
    }

    pub(crate) fn retire_scope(store: &mut Store, scope: imba::store::Id<OpenDocuments>) {
        crate::Registry::update(store, |registry| {
            registry.commands.scoped.remove_mut(&scope);
        });
    }

    pub fn find(
        &self,
        scope: imba::store::Id<OpenDocuments>,
        id: &str,
    ) -> Option<&Arc<dyn DocumentCommand>> {
        self.iter(scope).find(|command| command.id() == id)
    }

    pub fn iter(
        &self,
        scope: imba::store::Id<OpenDocuments>,
    ) -> impl Iterator<Item = &Arc<dyn DocumentCommand>> {
        self.scoped
            .get(&scope)
            .into_iter()
            .flatten()
            .chain(self.global.iter())
    }
}
