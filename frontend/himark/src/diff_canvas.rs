// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The shell half of the diff canvas: the two window commands (open
//! a canvas, open a row's file in a full pane). The canvas itself —
//! rows, header, panel face, router — lives in the `canvas` crate.

use ::canvas::diff_canvas::*;
use changesview::hichanges::CanvasSource;

use imba::store::Store;

use changesview::hichanges::Changes;
use editor::location::ResourceLocation;

/// Open the canvas for a source — or REUSE the one already open (the
/// canvas is found by source in the store; a fresh view of it costs
/// nothing). An armed reveal rides the place.
pub struct OpenDiffCanvas {
    /// The collection whose set this canvas shows.
    pub changes: imba::store::Id<Changes>,
    pub source: CanvasSource,
    pub reveal: Option<ResourceLocation>,
}

impl crate::commands::HimarkCommand for OpenDiffCanvas {
    fn id(&self) -> &'static str {
        "diff.open-canvas"
    }
    fn name(&self) -> String {
        "Open Diff Canvas".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // ONE road for every shell: prefetch, then the SEATED SHELL
        // places the canvas — the workbench's walk-stack navigation,
        // the native side's pushed pane.
        let Some(origin) = crate::commands::bound_origin(self.id(), bound) else {
            return;
        };
        // A commit canvas needs its changeset — the same fetch the
        // tree's expansion runs; the pending set dedups a double ask.
        if let CanvasSource::Commit { folder, id } = &self.source {
            if let Some(history) = Changes::of(store, self.changes).map(|held| held.history()) {
                changesview::hihistory::History::ask(
                    store,
                    history,
                    changesview::hihistory::HistoryAsk::CommitFiles(folder.clone(), id.clone()),
                );
            }
        }
        fx.scope(crate::app::AppCommand::Verb, |fx| {
            crate::shell::navigate_origin(
                store,
                ui,
                origin,
                CanvasPlace {
                    changes: self.changes,
                    source: self.source.clone(),
                    reveal: self.reveal.clone(),
                },
                fx,
            )
        });
    }
}

/// Open a canvas file's live side in an ordinary pane — the header's
/// click (and `workbench.open-in-full` on a focused row).
pub(crate) struct OpenCanvasFile {
    pub location: ResourceLocation,
    /// The caret to land on — carried from the row's diff editor so
    /// cmd-enter opens at the position being read, matching the
    /// standalone split-diff pane (docs/editor/diff-canvas.md §6).
    pub target: Option<std::ops::Range<documents::text_ext::LineCol>>,
}

impl crate::commands::HimarkCommand for OpenCanvasFile {
    fn id(&self) -> &'static str {
        "diff.open-file"
    }
    fn name(&self) -> String {
        "Open File".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        // ONE road for every shell: the SEATED SHELL opens the
        // located document — the open effect reuses a registered
        // document and honors the carried caret either way.
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
                    target: self.target.clone(),
                    focus: true,
                },
                fx,
            )
        });
    }
}
