// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The shell half of the changes feature: the dock toggle, the
//! palette refetch, the diff-for-pair open — everything that holds a
//! window. The collection and the tree live in the `changesview`
//! crate.

use changesview::hichanges::*;

use std::sync::Arc;

use editor::location::ResourceLocation;
use imba::store::Store;

/// The changes feature's ENGINE half as a plugin: the palette
/// refetch and the working-copy open — the dock toggle and its
/// toolbar button are the workbench-shell crate's.
pub struct ChangesPlugin;

impl crate::plugin::Plugin for ChangesPlugin {
    fn install(&self, app: &mut crate::app::Himark, _cx: &crate::plugin::PluginContext) {
        use crate::app_ext::AppExt;
        app.register_command(Arc::new(RefetchChanges::default()));
        app.register_document_command(Arc::new(OpenWorkingCopy));
    }
}

pub struct OpenDiffForPair {
    pub old: ResourceLocation,
    pub new: ResourceLocation,
}

impl crate::commands::HimarkCommand for OpenDiffForPair {
    fn id(&self) -> &'static str {
        "changes.open-diff"
    }
    fn name(&self) -> String {
        "Open Diff".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // The SEATED SHELL resolves and places the pair — the
        // workbench arm resolves against the window's session, the
        // mounted arm against the origin's.
        let Some(origin) = crate::commands::bound_origin(self.id(), bound) else {
            return;
        };
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            crate::shell::navigate_origin(
                store,
                ui,
                origin,
                crate::places::DiffPlace {
                    old: self.old.clone(),
                    new: self.new.clone(),
                },
                fx,
            )
        });
    }
}

/// Refetch changesets — one repository's when `folder` names it, every
/// riding folder's otherwise (the palette / test road).
#[derive(Default)]
pub struct RefetchChanges {
    /// The wire to refetch through. `None` means "the window's
    /// session's", resolved when the command performs — a command
    /// registered into the palette holds no id at registration.
    pub wire: Option<imba::store::Id<ahp_changes::changes::ChangesWire>>,
    pub folder: Option<ResourceLocation>,
}

impl crate::commands::HimarkCommand for RefetchChanges {
    fn id(&self) -> &'static str {
        "changes.refetch"
    }
    fn name(&self) -> String {
        "Refresh Changes".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let _ = fx;
        let Some(changes) = bound
            .bound::<ahp_session::session::state::SessionState>()
            .map(|state| state.changes())
            .or_else(|| {
                // An addressed refetch (the view's chip) names its
                // wire's collection directly.
                self.wire
                    .and_then(|wire| ahp_changes::changes::changes_of(store, wire))
            })
        else {
            return;
        };
        Changes::ask_refetch(store, changes, self.folder.clone());
    }
}


#[cfg(test)]
mod tests;

/// Open a scoped (vcs) location's WORKING COPY in an ordinary pane —
/// the diff editor's "open in full" for changes-scoped documents.
pub(crate) struct OpenWorkingCopy;

impl documents::dynamic::DocumentCommand for OpenWorkingCopy {
    fn id(&self) -> &'static str {
        "workbench.open-in-full"
    }
    fn name(&self) -> String {
        "Open Working Copy".to_owned()
    }

    fn offers_at(&self, location: &ResourceLocation) -> bool {
        changesview::hichanges::scoped(location)
    }
    #[allow(clippy::too_many_arguments)]
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _documents: imba::store::Id<documents::OpenDocuments>,
        _document_id: documents::DocumentId,
        document: &mut editor::document::Document,
        editor: editor::editor::EditorId,
        location: &editor::location::ResourceLocation,
        _payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        _fx: &mut imba::effect::Effects<'_, editor::editor_view::EditorCommand>,
    ) {
        let working =
            changesview::hichanges::working_copy(location).unwrap_or_else(|| location.clone());
        let byte = document.caret_byte(editor);
        let mut view = document.text().view();
        let at = documents::text_ext::line_col_at(&mut view, byte as usize);
        crate::commands::AppRequests::push(
            store,
            Arc::new(ShowWorkingCopy {
                location: working,
                target: at..at,
            }),
        );
    }
}

struct ShowWorkingCopy {
    location: editor::location::ResourceLocation,
    target: std::ops::Range<documents::text_ext::LineCol>,
}

impl crate::commands::HimarkCommand for ShowWorkingCopy {
    fn id(&self) -> &'static str {
        "vcs.apply-open-working-copy"
    }
    fn name(&self) -> String {
        "Open Working Copy".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // The SEATED SHELL opens the located document — the open
        // effect reuses a registered document and honors the caret.
        let Some(origin) = crate::commands::bound_origin(self.id(), bound) else {
            return;
        };
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            crate::shell::navigate_origin(
                store,
                ui,
                origin,
                crate::places::DocumentPlace {
                    location: self.location.clone(),
                    target: Some(self.target.clone()),
                    focus: true,
                },
                fx,
            )
        });
    }
}
