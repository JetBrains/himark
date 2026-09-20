// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;

use crate::{AppCommand, AppFx};

pub use documents::diffs::{
    adopt_base_location, land_base_built, land_normalized, rearm_base_asks, DiffChanged,
    DiffHandle, DiffNormalizeEffect, DiffNormalizeHandler, DiffView, DiffViewId, FetchBaseEffect,
    StripeBases,
};

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

pub fn sync_stripe_bases(store: &mut Store, fx: &mut AppFx<'_>) {
    documents::diffs::sync_stripe_bases(store, fx, |document, base| AppCommand::BaseLocated {
        document,
        base,
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
