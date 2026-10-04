// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The shell half of the diff canvas: the two window commands (open
//! a canvas, open a row's file in a full pane). The canvas itself —
//! rows, header, panel face, router — lives in the `canvas` crate.


use changesview::hichanges::CanvasSource;
use ::canvas::canvas;
use ::canvas::diff_canvas::*;

use imba::store::Store;

use crate::window::WindowId;
use editor::location::ResourceLocation;
use changesview::hichanges::Changes;



/// Open the canvas for a source — or REUSE the one already open (the
/// canvas is found by source in the store; a fresh view of it costs
/// nothing). An armed reveal rides the place.
pub struct OpenDiffCanvas {
    /// The collection whose set this canvas shows.
    pub changes: imba::store::Id<Changes>,
    pub source: CanvasSource,
    pub reveal: Option<ResourceLocation>,
}

impl crate::commands::DynamicCommand for OpenDiffCanvas {
    fn id(&self) -> &'static str {
        "diff.open-canvas"
    }
    fn name(&self) -> String {
        "Open Diff Canvas".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::app::Application,
        store: &mut Store,
        window: WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
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
        let Some(mut entity) = crate::window::Windows::window(store, window) else {
            return;
        };
        let place = CanvasPlace {
            changes: self.changes,
            source: self.source.clone(),
            reveal: self.reveal.clone(),
        };
        if !entity.navigate(
            store,
            ui,
            window,
            &hikit::navigation::NavigationLocation::new(place),
            fx,
        ) {
            eprintln!("[himark] no canvas navigator registered");
        }
        crate::window::Windows::put(store, window, entity);
    }
}

/// Open a canvas file's live side in an ordinary pane — the header's
/// click (and `workbench.open-in-full` on a focused row).
pub struct OpenCanvasFile {
    pub location: ResourceLocation,
    /// The caret to land on — carried from the row's diff editor so
    /// cmd-enter opens at the position being read, matching the
    /// standalone split-diff pane (docs/editor/diff-canvas.md §6).
    pub target: Option<std::ops::Range<documents::text_ext::LineCol>>,
}

impl crate::commands::DynamicCommand for OpenCanvasFile {
    fn id(&self) -> &'static str {
        "diff.open-file"
    }
    fn name(&self) -> String {
        "Open File".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::app::Application,
        store: &mut Store,
        window: WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let Some(target) = self.target.clone() else {
            // No caret to honor — the plain open, dedup + authority
            // remap included.
            crate::workspace::open_locations(store, ui, window, &[self.location.clone()], fx);
            return;
        };
        // Honor the caret: the canvas row's document is registered
        // (docs/editor/diff-canvas.md §7), so this is a show at target;
        // fall back to a targeted fetch if it somehow is not.
        let documents = crate::window::Windows::session_state(store, window)
            .expect("canvas navigation runs in a window with a session")
            .documents();
        match documents::OpenDocuments::by_location(store, documents, &self.location) {
            Some(document_id) => {
                if let Some(mut entity) = crate::window::Windows::window(store, window) {
                    entity.show_document(store, ui, window, document_id, Some(target), false, fx);
                    crate::window::Windows::put(store, window, entity);
                }
            }
            None => {
                fx.push(crate::workspace::open_by_location_effect(
                    window,
                    documents,
                    self.location.clone(),
                    true,
                    false,
                    Some(target),
                ));
            }
        }
    }
}
