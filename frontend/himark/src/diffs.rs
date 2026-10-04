// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use documents::diffs::{
    rearm_base_asks, DiffHandle, DiffNormalizeEffect, DiffNormalizeHandler, DiffView, DiffViewId,
    StripeBaseResolver, StripeBases,
};
use documents::diff_views::{
    build_diff_view, gather_diff_view, install_opened_pair, rewrap_pair, teardown_diff_view,
    OPEN_HALF_WIDTH,
};

use imba::store::Store;

use crate::app::AppCommand;
use crate::app::AppFx;



/// The batch-tail DRESSING sweep, scoped onto the At road — the
/// machinery lives with `documents`; this adapter only folds its
/// commands into the app stream.
pub(crate) fn sync_diff_dressing(
    store: &mut Store,
    documents: imba::store::Id<documents::OpenDocuments>,
    ui: &imba::ui::UiCtx,
    fx: &mut AppFx<'_>,
) {
    fx.scope(
        move |command| AppCommand::at(documents, command),
        |fx| documents::diff_views::sync_diff_dressing(store, documents, ui, fx),
    );
}

pub(crate) fn sync_diff_lanes(
    store: &mut Store,
    documents: imba::store::Id<documents::OpenDocuments>,
    fx: &mut AppFx<'_>,
) {
    documents::diffs::sync_diff_lanes(store, documents, fx, move |normalized| {
        AppCommand::at(
            documents,
            documents::DocumentsCommand::Normalized {
                diff: normalized.diff,
                operation: normalized.operation,
                markup: normalized.markup,
                changed: normalized.changed,
                base_revision: normalized.base_revision,
                target_revision: normalized.target_revision,
            },
        )
    });
}

