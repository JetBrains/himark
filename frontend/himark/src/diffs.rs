// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;
use imba::View as _;

use crate::{AppCommand, AppFx};

pub use documents::diffs::{
    adopt_base_location, land_base_built, land_normalized, rearm_base_asks, DiffHandle,
    DiffNormalizeEffect, DiffNormalizeHandler, DiffView, DiffViewId, StripeBases,
};

/// Assemble the per-ask facade over a tracked pair: live documents +
/// the store-held `DiffViewState` (attached on first gather). Moved up
/// from hidiff — the DRESSING is model machinery, not a panel's.
pub fn gather_diff_view(pair: &DiffView, store: &Store) -> Option<crate::UnifiedDiffView> {
    let left_view = crate::EditorView {
        document: crate::OpenDocuments::document(store, pair.left.document())?,
        editor: pair.left.editor(),
        reports_geometry: true,
        location: crate::OpenDocuments::location(store, pair.left.document()),
        gutter_width: 0.0,
        base: None,
    };
    let right_view = crate::EditorView {
        document: crate::OpenDocuments::document(store, pair.right.document())?,
        editor: pair.right.editor(),
        reports_geometry: true,
        location: crate::OpenDocuments::location(store, pair.right.document()),
        gutter_width: 0.0,
        base: None,
    };
    let state = match &pair.state {
        Some(state) => state.clone(),
        None => crate::DiffViewState::attach(
            pair.diff,
            &left_view.document,
            &right_view.document,
            crate::OpenDocuments::diff_handle(store, pair.diff)?.base_markup,
            pair.right_extras,
            None,
        )?,
    };
    Some(crate::UnifiedDiffView::new(crate::SplitDiffView::new(
        left_view, right_view, state,
    )))
}

/// Perform one command against a STORE-HELD diff view — the panel-free
/// road (docs/model-view.md step 1): take the record, gather, perform
/// with effects routed home BY ID (`AppCommand::DiffViewCommand`), put
/// the documents and the state back. Panels keep their own routed
/// perform for interaction; the dressing flows through here.
pub(crate) fn perform_diff_view(
    store: &mut Store,
    ui: &imba::UiCtx,
    session: crate::SessionId,
    id: DiffViewId,
    command: crate::UnifiedDiffCommand,
    fx: &mut AppFx<'_>,
) {
    let Some(mut pair) = crate::OpenDocuments::take_diff_view(store, id) else {
        return;
    };
    let Some(mut view) = gather_diff_view(&pair, store) else {
        crate::OpenDocuments::put_diff_view(store, id, pair);
        return;
    };
    fx.scope(
        move |command: crate::UnifiedDiffCommand| AppCommand::DiffViewCommand {
            session: session.clone(),
            view: id,
            command: Box::new(command),
        },
        |fx| view.perform(store, ui, command, fx),
    );
    crate::OpenDocuments::put_document(store, pair.left.document(), view.split.left.document);
    crate::OpenDocuments::put_document(store, pair.right.document(), view.split.right.document);
    pair.state = Some(view.split.state);
    crate::OpenDocuments::put_diff_view(store, id, pair);
    store.update::<DressedViews>(|dressed| dressed.0.push(id));
}

/// The diff views the dressing touched THIS batch — written by the
/// sweep and the id-routed landings, read by later tail lanes (the
/// canvas resizes exactly these rows). Reset at the next sweep.
#[derive(Clone, Default)]
pub struct DressedViews(pub Vec<DiffViewId>);

/// The batch-tail DRESSING sweep (docs/model-view.md step 1): any
/// tracked view whose basis lags its pair — a normalize landed, or
/// another editor moved a shared document — resyncs NOW, id-routed,
/// no paint probe. Runs right after the diff lanes, so a landing and
/// its re-dress share a batch. O(views) stale checks on refs.
pub(crate) fn sync_diff_dressing(store: &mut Store, ui: &imba::UiCtx, fx: &mut AppFx<'_>) {
    let Some(session) = crate::Gathered::scope(store).cloned() else {
        return;
    };
    for id in crate::OpenDocuments::diff_view_ids(store) {
        let stale = crate::OpenDocuments::diff_view_ref(store, id).is_some_and(|pair| {
            let Some(state) = &pair.state else {
                // Never gathered: no face was built, nothing owes.
                return false;
            };
            let Some(left) = crate::OpenDocuments::document_ref(store, pair.left.document()) else {
                return false;
            };
            let Some(right) = crate::OpenDocuments::document_ref(store, pair.right.document())
            else {
                return false;
            };
            state.stale(left, right)
        });
        if !stale {
            continue;
        }
        perform_diff_view(
            store,
            ui,
            session.clone(),
            id,
            crate::UnifiedDiffCommand::Split(crate::SplitDiffCommand::Resync),
            fx,
        );
    }
}

pub(crate) fn sync_diff_lanes(store: &mut Store, fx: &mut AppFx<'_>) {
    documents::diffs::sync_diff_lanes(store, fx, |normalized| AppCommand::DiffNormalized {
        diff: normalized.diff,
        operation: normalized.operation,
        markup: normalized.markup,
        changed: normalized.changed,
        base_revision: normalized.base_revision,
        target_revision: normalized.target_revision,
    });
}

pub fn sync_stripe_bases(store: &mut Store, ui: &imba::UiCtx, fx: &mut AppFx<'_>) {
    documents::diffs::sync_stripe_bases(store, fx, |store, document, base, fx| {
        land_base_located(store, ui, document, base, fx);
    });
}

pub(crate) fn land_base_located(
    store: &mut Store,
    ui: &imba::UiCtx,
    document: crate::DocumentId,
    base: Option<crate::ResourceLocation>,
    fx: &mut AppFx<'_>,
) {
    let Some(base) = adopt_base_location(store, ui, document, base, fx) else {
        return;
    };
    let _ = fx.push(
        imba::effect::AnyEffect::new(crate::FetchDocumentEffect {
            location: base.clone(),
        })
        .map(move |text| AppCommand::BaseFetched {
            document,
            base,
            text,
        }),
    );
}

/// The standalone pane's half width — shared by every pair build.
pub const OPEN_HALF_WIDTH: f32 = 420.0;

/// Tear down a tracked pair: drop the store-held `DiffView`, remove the
/// pair's editors from the (possibly shared) registered documents, and
/// untrack the diff from the Diffs subsystem. Does NOT close the
/// documents — they may be open elsewhere. Shared by `DiffPanelView`
/// and the diff canvas.
pub fn teardown_diff_view(store: &mut Store, id: crate::DiffViewId) {
    // Teardown-only road (dismantle/destroy/retire carry no UiCtx);
    // the release may reshape a surviving base document's markup once.
    let ui = &imba::UiCtx::dont_use_too_slow();
    let Some(pair) = crate::OpenDocuments::take_diff_view(store, id) else {
        return;
    };
    if let Some(inline) = pair.state.as_ref().and_then(|state| state.inline_editor()) {
        if let Some(mut document) = crate::OpenDocuments::document(store, pair.right.document()) {
            document.remove_editor(inline);
            crate::OpenDocuments::put_document(store, pair.right.document(), document);
        }
    }
    for entity in [pair.left, pair.right] {
        if let Some(mut document) = crate::OpenDocuments::document(store, entity.document()) {
            document.remove_editor(entity.editor());
            crate::OpenDocuments::put_document(store, entity.document(), document);
        }
    }
    crate::OpenDocuments::untrack_diff(
        store,
        ui,
        pair.diff,
        &mut imba::effect::Batch::<crate::UnifiedDiffCommand>::new().effects(),
    );
}

/// Re-wrap the inline face of a tracked pair to `width` (the row-level
/// rewrap, docs/editor/diff-canvas.md §4): gather, resize the half + inline
/// editors on the registered documents, resync, write back. The split
/// face owns its half widths and is left alone.
pub fn rewrap_pair(
    store: &mut Store,
    ui: &imba::UiCtx,
    id: crate::DiffViewId,
    width: f32,
    fx: &mut imba::effect::Effects<'_, crate::UnifiedDiffCommand>,
) {
    let Some(mut pair) = crate::OpenDocuments::take_diff_view(store, id) else {
        return;
    };
    let Some(mut view) = gather_diff_view(&pair, store) else {
        crate::OpenDocuments::put_diff_view(store, id, pair);
        return;
    };
    if view.layout == crate::DiffLayout::Split {
        crate::OpenDocuments::put_diff_view(store, id, pair);
        return;
    }
    let fonts = crate::env::Fonts::of(store)();
    let theme = crate::env::Themes::of(store);
    let left_editor = view.split.left.editor;
    let right_editor = view.split.right.editor;
    let inline = view.inline_editor;
    fx.scope(
        |c: crate::EditorCommand| {
            crate::UnifiedDiffCommand::Split(crate::SplitDiffCommand::Left(c))
        },
        |fx| {
            view.split
                .left
                .document
                .resize(left_editor, width, 0, store, ui, &fonts, &theme, fx)
        },
    );
    fx.scope(
        |c: crate::EditorCommand| {
            crate::UnifiedDiffCommand::Split(crate::SplitDiffCommand::Right(c))
        },
        |fx| {
            view.split
                .right
                .document
                .resize(right_editor, width, 0, store, ui, &fonts, &theme, fx)
        },
    );
    if let Some(inline) = inline {
        fx.scope(
            |c: crate::EditorCommand| crate::UnifiedDiffCommand::Inline(c),
            |fx| {
                view.split
                    .right
                    .document
                    .resize(inline, width, 0, store, ui, &fonts, &theme, fx)
            },
        );
    }
    view.perform(
        store,
        ui,
        crate::UnifiedDiffCommand::Split(crate::SplitDiffCommand::Resync),
        fx,
    );
    crate::OpenDocuments::put_document(store, pair.left.document(), view.split.left.document);
    crate::OpenDocuments::put_document(store, pair.right.document(), view.split.right.document);
    pair.state = Some(view.split.state);
    crate::OpenDocuments::put_diff_view(store, id, pair);
}

fn register_or_reuse(store: &mut Store, side: crate::DiffSide) -> crate::DocumentId {
    match side {
        crate::DiffSide::Open(id) => id,
        crate::DiffSide::Built { location, document } => {
            match crate::OpenDocuments::by_location(store, &location) {
                Some(id) => id,
                None => {
                    let revision = document.document.revision();
                    crate::OpenDocuments::register(
                        store,
                        document.document,
                        Some(location.clone()),
                        location.name().to_owned(),
                        revision,
                    )
                }
            }
        }
    }
}

/// Install an opened pair: register/reuse both sides, then
/// `build_diff_view` (which tracks the diff — the normalize lane
/// computes and dresses it — and mounts the `UnifiedDiffView`). The
/// one landing behind the split-diff pane AND the diff canvas; each
/// wraps the returned id in its own face.
pub fn install_opened_pair(
    store: &mut Store,
    ui: &imba::UiCtx,
    pair: crate::OpenedDiffPair,
    embedded: bool,
) -> Option<crate::DiffViewId> {
    let old_id = register_or_reuse(store, pair.old);
    let new_id = register_or_reuse(store, pair.new);
    let half_width = match embedded {
        true => {
            let gutter = crate::env::Themes::of(store).ui().editor_gutter.width;
            (pair.width - gutter).max(120.0)
        }
        false => OPEN_HALF_WIDTH,
    };
    build_diff_view(store, ui, old_id, new_id, half_width, embedded)
}

/// Make a diff view over two ALREADY-REGISTERED documents and mint the
/// store-held `DiffView`: track the diff through the Diffs subsystem
/// (which SEEDS it — the normalize lane computes the real diff from the
/// documents and dresses it), add a bounded editor per half, and attach
/// the `DiffViewState`. No diff is computed here (docs/no-diff-on-ui-thread).
/// The reusable core the split-diff pane and the diff canvas both mount.
pub fn build_diff_view(
    store: &mut Store,
    ui: &imba::UiCtx,
    left: crate::DocumentId,
    right: crate::DocumentId,
    half_width: f32,
    embedded: bool,
) -> Option<crate::DiffViewId> {
    let fonts = crate::env::Fonts::of(store)();
    let theme = crate::env::Themes::of(store);

    let diff = crate::OpenDocuments::track_diff(store, left, right, false)?;
    let handle = crate::OpenDocuments::diff_handle(store, diff)?;
    let target_markup = crate::OpenDocuments::document_ref(store, right)
        .and_then(|document| document.diff(diff).map(|entry| entry.markup()))?;

    let mut open =
        |document_id: crate::DocumentId, marks: crate::MarkupId| -> Option<crate::EditorIdView> {
            let mut document = crate::OpenDocuments::document(store, document_id)?;

            let editor = document.add_editor(
                half_width,
                None,
                crate::EditorBuild::Bounded,
                &[marks],
                store,
                ui,
                &fonts,
                &theme,
                &mut imba::effect::Batch::new().effects(),
            );

            document.manage_repairs_in_pair(editor);

            crate::OpenDocuments::put_document(store, document_id, document);
            Some(crate::EditorIdView::new(document_id, editor))
        };
    let (Some(left_view), Some(right_view)) =
        (open(left, handle.base_markup), open(right, target_markup))
    else {
        return None;
    };
    // The pane's own right-half extras (word tints + fold strips) —
    // editor-owned, dying with the half; derived by the marks job on
    // settle. THE diff markup (`target_markup`, the hunk washes) stays
    // the diff machinery's — seeded by `track_diff`, minimized by the
    // normalize lane.
    let right_extras = {
        let mut document = crate::OpenDocuments::document(store, right)?;
        let id = document.add_owned_markup(right_view.editor());
        crate::OpenDocuments::put_document(store, right, document);
        id
    };

    let state = {
        let left_document = crate::OpenDocuments::document_ref(store, left)?;
        let right_document = crate::OpenDocuments::document_ref(store, right)?;
        crate::DiffViewState::attach(
            diff,
            left_document,
            right_document,
            handle.base_markup,
            right_extras,
            None,
        )
    };
    let id = crate::DiffViewId::mint();
    crate::OpenDocuments::put_diff_view(
        store,
        id,
        crate::DiffView {
            left: left_view,
            right: right_view,
            diff: handle.id,
            right_extras,
            state,
            embedded,
        },
    );
    Some(id)
}
