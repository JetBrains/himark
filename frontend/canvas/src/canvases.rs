// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The canvases COLLECTION: store-held `Canvas` state on the change
//! sets (the `OpenDocuments` discipline — views hold ids, a second
//! open reuses), the diff-pair landing, and the At router the
//! batch-tail sync and the off-thread builds reach a canvas through.

use crate::canvas::{Canvas, CanvasCommand, CanvasKey};
use editor::location::ResourceLocation;
use imba::event::Placement;
use imba::{store::Store, ui::UiCtx};

use changesview::hichanges::{CanvasId, CanvasSource};

/// Stateless FACADE over the sets' owned canvases
/// (docs/model-view.md: `ChangeSet.canvases`): at most one canvas per
/// source — and a source IS a set now, so reuse is the set lookup.
/// Canvases live while views retain them; the working-copy canvas
/// stays once opened.
pub struct Canvases;

use changesview::hichanges::{ChangeSetId, ChangeSetSource, Changes};

fn set_source(source: &CanvasSource) -> ChangeSetSource {
    match source {
        CanvasSource::WorkingCopy { folder } => ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        },
        CanvasSource::Commit { folder, id } => ChangeSetSource::Commit {
            folder: folder.clone(),
            revision: id.clone(),
        },
    }
}

impl Canvases {
    pub fn by_source(
        store: &Store,
        changes: imba::store::Id<Changes>,
        source: &CanvasSource,
    ) -> Option<CanvasId> {
        let set = Changes::id_for_source(store, changes, &set_source(source))?;
        Changes::set_ref(store, changes, set)?
            .canvases
            .keys()
            .next()
            .copied()
    }

    pub(crate) fn owner_of(
        store: &Store,
        changes: imba::store::Id<Changes>,
        id: CanvasId,
    ) -> Option<ChangeSetId> {
        Changes::canvas_ids(store, changes)
            .into_iter()
            .find(|(_, canvas)| *canvas == id)
            .map(|(set, _)| set)
    }

    pub(crate) fn find_or_create(
        store: &mut Store,
        changes: imba::store::Id<Changes>,
        source: &CanvasSource,
    ) -> CanvasId {
        let set = Changes::ensure_set_for_source(store, changes, &set_source(source));
        if let Some(id) = Changes::set_ref(store, changes, set)
            .and_then(|held| held.canvases.keys().next().copied())
        {
            return id;
        }
        let id = CanvasId::mint();
        Changes::put_canvas(
            store,
            changes,
            set,
            id,
            Canvas::fresh(changes, source.clone()),
        );
        id
    }

    pub(crate) fn get<'a>(
        store: &'a Store,
        changes: imba::store::Id<Changes>,
        id: CanvasId,
    ) -> Option<&'a Canvas> {
        let set = Self::owner_of(store, changes, id)?;
        Changes::canvas_ref(store, changes, set, id)
    }

    pub(crate) fn take(
        store: &mut Store,
        changes: imba::store::Id<Changes>,
        id: CanvasId,
    ) -> Option<Canvas> {
        let set = Self::owner_of(store, changes, id)?;
        Changes::take_canvas(store, changes, set, id)
    }

    pub(crate) fn put(
        store: &mut Store,
        changes: imba::store::Id<Changes>,
        id: CanvasId,
        canvas: Canvas,
    ) {
        // A put without a surviving owner re-homes by source (the set
        // always exists — sources mint their sets).
        let set = Self::owner_of(store, changes, id).unwrap_or_else(|| {
            Changes::ensure_set_for_source(store, changes, &set_source(&canvas.source))
        });
        Changes::put_canvas(store, changes, set, id, canvas);
    }

    pub(crate) fn retain(store: &mut Store, changes: imba::store::Id<Changes>, id: CanvasId) {
        if let Some(mut canvas) = Self::take(store, changes, id) {
            canvas.refs += 1;
            Self::put(store, changes, id, canvas);
        }
    }

    pub(crate) fn release(store: &mut Store, changes: imba::store::Id<Changes>, id: CanvasId) {
        let Some(mut canvas) = Self::take(store, changes, id) else {
            return;
        };
        canvas.refs = canvas.refs.saturating_sub(1);
        // View-retained lifetime — except the WORKING-COPY canvas,
        // which stays around once opened (its diffs keep serving the
        // next open for free).
        if canvas.refs > 0 || matches!(canvas.source, CanvasSource::WorkingCopy { .. }) {
            Self::put(store, changes, id, canvas);
            return;
        }
        // The last view is gone. The rows' substance — documents,
        // editors, tracked diffs — lives in the session's shared
        // OpenDocuments, not in this struct: dropping the canvas alone
        // leaks all of it for the session's life. Tear every row down:
        // live in the list, parked in the collapse stash, and the
        // off-row successors alike.
        let keys: Vec<ResourceLocation> = canvas.files.keys().cloned().collect();
        for key in keys {
            canvas.teardown_row(store, &key);
        }
    }

    pub(crate) fn canvas_ref(
        store: &Store,
        changes: imba::store::Id<Changes>,
        id: CanvasId,
    ) -> Option<&Canvas> {
        let set = Self::owner_of(store, changes, id)?;
        Changes::canvas_ref::<Canvas>(store, changes, set, id)
    }

    /// Arm a row reveal: applied at populate, or picked up by the
    /// paint probe — the walk-back and code-navigation road.
    pub fn set_reveal(
        store: &mut Store,
        changes: imba::store::Id<Changes>,
        id: CanvasId,
        key: ResourceLocation,
    ) {
        if let Some(mut canvas) = Self::take(store, changes, id) {
            match canvas.populated {
                // ALREADY populated: arm the rows NOW. The listing
                // often precedes the open (the changes tree shares
                // it), so populate ran before this arm — and the
                // sweep that would pick a parked key up only visits
                // views a batch re-dressed; a fresh open's reveal
                // sat waiting for the NEXT batch.
                true => {
                    canvas
                        .rows
                        .reveal_row(CanvasKey::File(key), Placement::TopLeftAt);
                }
                false => canvas.reveal = Some(key),
            }
            Self::put(store, changes, id, canvas);
        }
    }
}

/// Where a code-navigation target lands when the focused panel shows
/// a diff (docs/ui/UI.md): INSIDE it. A canvas reveals the target's
/// row and, once that row is built, goes to the byte in its right
/// half; a diff panel goes to the byte when the target is its right
/// document. `None` says the target is not in the diff — the plain
/// road opens an editor.
pub struct DiffLanding {
    /// The built pair to land in, if the row is built (a placeholder
    /// row reveals only; the arm builds it next).
    pub pair: Option<documents::diffs::DiffViewId>,
    /// The canvas row to reveal.
    pub reveal: Option<(imba::store::Id<Changes>, CanvasId, ResourceLocation)>,
}

pub fn diff_landing(
    store: &Store,
    panel: &dyn std::any::Any,
    documents: imba::store::Id<documents::OpenDocuments>,
    target: &ResourceLocation,
) -> Option<DiffLanding> {
    if let Some(view) = crate::faces::canvas_of(panel) {
        let canvas = Canvases::canvas_ref(store, view.changes(), view.id())?;
        if !canvas.files.contains_key(target) {
            return None;
        }
        let pair = canvas
            .pairs
            .iter()
            .find(|(_, key)| *key == target)
            .map(|(pair, _)| *pair)
            .filter(|pair| {
                documents::OpenDocuments::diff_view_ref(store, documents, *pair).is_some()
            });
        return Some(DiffLanding {
            pair,
            reveal: Some((view.changes(), view.id(), target.clone())),
        });
    }
    if let Some(view) = panel.downcast_ref::<crate::diff_pane::DiffPanelView>() {
        let pair = view.pair();
        let right = documents::OpenDocuments::diff_view_ref(store, view.documents(), pair)?
            .right
            .document();
        let shown = documents::OpenDocuments::location(store, view.documents(), right)?;
        if shown != *target {
            return None;
        }
        return Some(DiffLanding {
            pair: Some(pair),
            reveal: None,
        });
    }
    None
}

/// The canvases' own At address — a stateless ROUTER row minted by
/// the session ceremony beside the collection: canvas commands land
/// on canvas code here, and the model (which stores canvases as
/// SLOTS) never calls up into a face.
#[derive(Clone)]
pub struct CanvasRouter {
    changes: imba::store::Id<Changes>,
}

impl CanvasRouter {
    pub fn wired(changes: imba::store::Id<Changes>) -> Self {
        Self { changes }
    }

    pub fn is_empty(&self) -> bool {
        true
    }
}

#[derive(Clone)]
pub enum CanvasRouted {
    Canvas(ChangeSetId, CanvasId, Box<CanvasCommand>),
}

impl std::fmt::Display for CanvasRouted {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CanvasRouted::Canvas(_, _, command) => command.fmt(out),
        }
    }
}

impl imba::store::Entity for CanvasRouter {
    type Command = CanvasRouted;

    fn perform(
        &mut self,
        _id: imba::store::Id<Self>,
        command: CanvasRouted,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        fx: &mut imba::effect::Effects<'_, CanvasRouted>,
    ) {
        // The router row is stateless: the canvases live on the SETS
        // (slot doors), so no lease escape is owed here.
        match command {
            CanvasRouted::Canvas(set, canvas, command) => {
                perform_canvas(store, ui, self.changes, set, canvas, *command, fx);
            }
        }
    }

    fn destroy(&mut self, _store: &mut Store) {}
}

/// The batch-tail canvas sweep — a DIRECT lane now (the sets own their
/// canvases; no plugin observer, no window): reconcile every canvas in
/// the gathered session, launching builds through the canvases' own At
/// address (`CanvasRouted`, the DiffView shape).
pub fn sync_canvases(
    store: &mut Store,
    router: imba::store::Id<CanvasRouter>,
    ui: &UiCtx,
    dressed: &[documents::diffs::DiffViewId],
    fx: &mut imba::command::Fx<'_>,
) {
    let Some(changes) = store.entity::<CanvasRouter>(router).map(|row| row.changes) else {
        return;
    };
    fx.scope(
        move |command| imba::command::Verb::at(router, command),
        |fx| {
            for (set, id) in Changes::canvas_ids(store, changes) {
                let Some(mut canvas) = Changes::take_canvas::<Canvas>(store, changes, set, id)
                else {
                    continue;
                };
                let route = route_canvas(set, id);
                fx.scope(route, |fx| canvas.sync_in_place(store, ui, &dressed, fx));
                Changes::put_canvas(store, changes, set, id, canvas);
            }
        },
    );
}

/// Map a canvas's commands home BY IDS — the set and the canvas; the
/// router is the At address wrapping outside (no window, no landing
/// box, no session).
fn route_canvas(
    set: ChangeSetId,
    canvas: CanvasId,
) -> impl Fn(CanvasCommand) -> CanvasRouted + Clone {
    move |command| CanvasRouted::Canvas(set, canvas, Box::new(command))
}

/// Perform one command against a SET-OWNED canvas — the panel-free
/// road (`perform_diff_view`'s twin), reached through the router.
pub(crate) fn perform_canvas(
    store: &mut Store,
    ui: &UiCtx,
    changes: imba::store::Id<Changes>,
    set: ChangeSetId,
    id: CanvasId,
    command: CanvasCommand,
    fx: &mut imba::effect::Effects<'_, CanvasRouted>,
) {
    let Some(mut canvas) = Changes::take_canvas::<Canvas>(store, changes, set, id) else {
        return;
    };
    let route = route_canvas(set, id);
    fx.scope(route, |fx| canvas.perform(store, ui, command, fx));
    Changes::put_canvas(store, changes, set, id, canvas);
}
