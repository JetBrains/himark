// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::store::Store;

use crate::document::Document;
use crate::editor::{EditorEffects, EditorId};

/// A dynamic command's argument, erased behind a clone-shared slot:
/// commands must clone (`imba::Command`), but a payload is a one-shot
/// hand-off — clones share the slot and the first `take` wins.
#[derive(Clone)]
pub struct DynPayload(Arc<std::sync::Mutex<Option<Box<dyn std::any::Any + Send + Sync>>>>);

impl DynPayload {
    pub fn new(payload: impl std::any::Any + Send + Sync) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Some(Box::new(payload)))))
    }

    pub fn take(self) -> Option<Box<dyn std::any::Any + Send + Sync>> {
        self.0.lock().unwrap().take()
    }
}

pub trait DynamicEditorCommand: Send + Sync + 'static {
    fn id(&self) -> &'static str;

    fn name(&self) -> String;

    fn offers_at(&self, location: &crate::location::ResourceLocation) -> bool {
        !location.is_synthetic()
    }
    #[allow(clippy::too_many_arguments)]
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        document: &mut Document,
        editor: EditorId,
        location: &crate::location::ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut EditorEffects<'_>,
    );
}

#[derive(Clone, Default)]
pub struct EditorCommands(Vec<Arc<dyn DynamicEditorCommand>>);

impl EditorCommands {
    pub fn of(store: &Store) -> EditorCommands {
        store.get::<EditorCommands>().cloned().unwrap_or_default()
    }

    pub fn register(store: &mut Store, command: Arc<dyn DynamicEditorCommand>) {
        store.update::<EditorCommands>(|commands| commands.0.push(command));
    }

    pub fn find(&self, id: &str) -> Option<&Arc<dyn DynamicEditorCommand>> {
        self.0.iter().find(|command| command.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn DynamicEditorCommand>> {
        self.0.iter()
    }
}
