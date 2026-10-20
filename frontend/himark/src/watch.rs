// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;

pub struct ReloadDocument;

impl crate::commands::HimarkCommand for ReloadDocument {
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
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // The focused document and its session both ride the walk —
        // the reload works from any shell.
        let Some(&document) = bound.bound::<documents::DocumentId>() else {
            eprintln!(
                "[shell] reload landed with no focused document bound — dropped: {}",
                self.id()
            );
            return;
        };
        let Some(state) = crate::commands::bound_session(self.id(), bound) else {
            return;
        };
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            documents::lanes::refetch_document(store, state.documents(), document, fx)
        });
    }
}

#[cfg(test)]
mod tests;
