// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;



pub struct ReloadDocument;

impl crate::commands::WindowedCommand for ReloadDocument {
    fn id(&self) -> &'static str {
        "file.reload"
    }

    fn name(&self) -> String {
        "Reload from Disk".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(document) = ::workbench::window::Windows::window_ref(store, window)
            .and_then(|entity| entity.focused_document_id())
        else {
            return;
        };
        let Some(state) = ::workbench::window::Windows::session_state(store, window) else {
            return;
        };
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            documents::lanes::refetch_document(store, state.documents(), document, fx)
        });
    }
}

#[cfg(test)]
mod tests;
