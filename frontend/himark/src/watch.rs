// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use documents::watch::{
    FileChanged, FilesChanged, RefetchDiffEffect, RefetchDiffHandler, SubscribeEffect,
    Subscription, UnsubscribeEffect, Watching,
};

use imba::store::Store;

use crate::app::AppCommand;
use crate::app::AppFx;


use documents::DocumentsCommand;

pub struct ReloadDocument;

impl crate::commands::DynamicCommand for ReloadDocument {
    fn id(&self) -> &'static str {
        "file.reload"
    }

    fn name(&self) -> String {
        "Reload from Disk".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::app::Application,
        store: &mut Store,
        window: ::workbench::window::WindowId,
        fx: &mut AppFx<'_>,
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
