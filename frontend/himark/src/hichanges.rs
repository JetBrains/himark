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

/// The changes feature as a plugin: the dock toggle and its
/// toolbar button, the palette refetch, the working-copy open.
pub struct ChangesPlugin;

impl crate::plugin::Plugin for ChangesPlugin {
    fn install(&self, app: &mut crate::app::Himark, _cx: &crate::plugin::PluginContext) {
        use crate::app_ext::AppExt;
        app.register_command(Arc::new(ToggleChangesView));
        app.register_command(Arc::new(RefetchChanges::default()));
        app.register_toolbar_button(toolbar_button());
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

pub struct ToggleChangesView;

impl crate::commands::HimarkCommand for ToggleChangesView {
    fn id(&self) -> &'static str {
        "changes.view"
    }
    fn name(&self) -> String {
        "Changes".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
        let mut entity =
            ::workbench::window::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            ::workbench::window::Windows::put(store, window, entity);
            return;
        }
        // The pane closes over the window's session: its change sets
        // by id, the session only as the catalog's name for the folders.
        let workspace = crate::workspace::entity_session(&entity);
        let changes = crate::workspace::entity_state(&entity).changes();
        let wire = crate::workspace::entity_state(&entity).changes_wire();
        let folders = ahp_session::session::folders::session_folders(store, &workspace);
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            ahp_changes::changes::ensure(store, wire, folders, fx)
        });
        // The canvas-open verb the tree emits — the window rides in
        // the closure; the view never holds one.
        let open_canvas: changesview::changes_view::CanvasOpener =
            Arc::new(move |source, reveal| {
                crate::app::shell_verb(crate::shell::windowed(
                    window,
                    Arc::new(crate::diff_canvas::OpenDiffCanvas {
                        changes,
                        source,
                        reveal,
                    }),
                ))
            });

        fx.scope(
            move |command| crate::shell::content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let view = Changes::mint_view(
            store,
            changes,
            changesview::changes_view::ChangesView::open(
                store,
                ui,
                changes,
                changesview::changes_view::ViewSets::WorkingCopies,
                open_canvas,
            ),
        );
        let owner = self.id();
        fx.scope(
            move |command| crate::shell::content(window, command),
            |fx| {
                entity.show_dock(
                    store,
                    Box::new(hikit::modal::Scrolled::new(
                        changesview::changes_view::ChangesPane::new(changes, view),
                    )),
                    owner,
                    fx,
                )
            },
        );
        ::workbench::window::Windows::put(store, window, entity);
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

pub fn toolbar_button() -> ::workbench::toolbar::ToolbarButton {
    ::workbench::toolbar::ToolbarButton {
        command: "changes.view",
        order: 1.0,
        side: ::workbench::toolbar::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());
            let mut path = skia_safe::PathBuilder::new();

            let (px, py, arm) = (l + w * 0.32, t + h * 0.32, w * 0.17);
            path.move_to((px - arm, py));
            path.line_to((px + arm, py));
            path.move_to((px, py - arm));
            path.line_to((px, py + arm));

            let (mx, my) = (l + w * 0.68, t + h * 0.74);
            path.move_to((mx - arm, my));
            path.line_to((mx + arm, my));
            canvas.draw_path(&path.detach(), &paint);
        }),
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
