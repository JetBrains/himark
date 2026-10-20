// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The shell half of comments: the feature gate (a workbench
//! registry flag), the dock toggle, its toolbar button and the
//! comment-pick navigation drain. Everything else — the collection,
//! the cards, the dock panel — lives in the `comments` crate.

use ::comments::panel::CommentsView;

use std::sync::Arc;

use imba::store::Store;

/// The comments feature's ENGINE half as a plugin: the model hook —
/// the dock toggle and its toolbar button are the workbench-shell
/// crate's.
pub struct CommentsPlugin;

impl crate::plugin::Plugin for CommentsPlugin {
    fn install(&self, app: &mut crate::app::Himark, _cx: &crate::plugin::PluginContext) {
        app.setup(|store| ::comments::install(store));
    }
}

