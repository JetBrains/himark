// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;
use imba::View as _;

use crate::{AppCommand, AppFx};

pub use documents::diffs::{
    adopt_base_location, land_base_built, land_normalized, rearm_base_asks,
    DiffHandle, DiffNormalizeEffect, DiffNormalizeHandler, DiffView, DiffViewId, StripeBases,
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
            let Some(left) = crate::OpenDocuments::document_ref(store, pair.left.document())
            else {
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
