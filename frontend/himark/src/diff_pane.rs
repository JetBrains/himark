// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The shell half of the split-diff pane: the palette's "diff two
//! recent documents" command. The faces live in the `canvas` crate.

use imba::store::Store;

use documents::OpenDocuments;

/// The split-diff pane as a plugin: the palette command and the
/// chat's diff-pair row minter.
pub struct DiffPanePlugin;

impl crate::plugin::Plugin for DiffPanePlugin {
    fn install(&self, app: &mut crate::app::Himark, _cx: &crate::plugin::PluginContext) {
        use crate::app_ext::AppExt;
        app.register_command(std::sync::Arc::new(OpenDiff));
        app.register_row_minter(::canvas::diff_pane::pair_row_minter());
    }
}

/// Open an opened pair as a standalone split-diff pane (the changes
/// view's "Open Diff", the diff navigator). Shares the whole road with
/// the canvas — only the face differs.

pub struct OpenDiff;

impl crate::commands::HimarkCommand for OpenDiff {
    fn id(&self) -> &'static str {
        "diff.open"
    }
    fn name(&self) -> String {
        "Diff Two Recent Documents".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // The SESSION road picks the pair; the SEATED SHELL places
        // the ready view — panel in a workbench, pushed screen on
        // the native side.
        let Some(origin) = crate::commands::bound_origin(self.id(), bound) else {
            return;
        };
        let Some(state) = crate::commands::bound_session(self.id(), bound) else {
            return;
        };
        let documents = state.documents();
        let recent = OpenDocuments::list_recent(store, documents);
        let (Some(newest), Some(older)) = (recent.first(), recent.get(1)) else {
            return;
        };
        let Some(view) = documents::diff_views::build_diff_view(
            store,
            documents,
            ui,
            older.0,
            newest.0,
            documents::diff_views::OPEN_HALF_WIDTH,
            false,
        ) else {
            return;
        };
        let title = OpenDocuments::name(store, documents, newest.0).unwrap_or_default();
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            crate::shell::navigate_origin(
                store,
                ui,
                origin,
                crate::places::DiffViewPlace {
                    documents,
                    view,
                    title,
                },
                fx,
            )
        });
    }
}
