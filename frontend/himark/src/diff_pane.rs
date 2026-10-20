// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The shell half of the split-diff pane: the palette's "diff two
//! recent documents" command. The faces live in the `canvas` crate.

use ::canvas::diff_pane::*;

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

/// Resolve one opened side to a registered `DocumentId`: reuse the open
/// one, or REGISTER the freshly-built one (register-at-display — a
/// located document is always an OpenDocuments document,
/// docs/editor/diff-canvas.md §7). A `Built` side whose location was
/// opened by someone else meanwhile reuses the winner and drops the
/// build — the one genuine (and rare) throwaway, a lost open race.
pub(crate) fn open_opened_diff_pane(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    window: ::workbench::window::WindowId,
    documents: imba::store::Id<OpenDocuments>,
    pair: documents::diff_views::OpenedDiffPair,
    fx: &mut crate::app::AppFx<'_>,
) -> bool {
    let Some(id) = documents::diff_views::install_opened_pair(store, documents, ui, pair, false)
    else {
        return false;
    };
    let panel = DiffPanelView::over(documents, id);
    let mut entity =
        ::workbench::window::Windows::window(store, window).expect("the window entity");
    let opened = entity.open_panel(store, ui, Box::new(hikit::modal::Scrolled::new(panel)), fx);
    ::workbench::window::Windows::put(store, window, entity);
    opened
}

pub(crate) fn open_diff_documents(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::ui::UiCtx,
    window: ::workbench::window::WindowId,
    left: documents::DocumentId,
    right: documents::DocumentId,
    fx: &mut crate::app::AppFx<'_>,
) -> bool {
    let Some(panel) = diff_panel(store, documents, ui, left, right) else {
        return false;
    };
    let mut entity =
        ::workbench::window::Windows::window(store, window).expect("the window entity");
    let opened = entity.open_panel(store, ui, Box::new(hikit::modal::Scrolled::new(panel)), fx);
    ::workbench::window::Windows::put(store, window, entity);
    opened
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
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
        let ui = ui;
        let Some(state) = crate::workspace::session_state(store, window) else {
            return;
        };
        let documents = state.documents();
        let recent = OpenDocuments::list_recent(store, documents);
        let (Some(newest), Some(older)) = (recent.first(), recent.get(1)) else {
            return;
        };
        let _ = open_diff_documents(store, documents, ui, window, older.0, newest.0, _fx);
    }
}
