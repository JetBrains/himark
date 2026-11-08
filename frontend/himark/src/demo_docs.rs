// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The demo-document registry — a store resource the demo plugin
//! fills and the C door reads (`himark_open_demo`).

use std::sync::Arc;

/// them up.
#[derive(Clone, Default)]
pub struct DemoDocs(
    pub  rpds::HashTrieMapSync<
        String,
        (
            editor::location::ResourceLocation,
            Arc<
                dyn Fn(
                        &imba::store::Store,
                        &imba::ui::UiCtx,
                        &editor::theme::Theme,
                    ) -> editor::document::Document
                    + Send
                    + Sync,
            >,
        ),
    >,
);

impl DemoDocs {
    pub fn register(
        store: &mut imba::store::Store,
        name: &str,
        location: editor::location::ResourceLocation,
        build: Arc<
            dyn Fn(
                    &imba::store::Store,
                    &imba::ui::UiCtx,
                    &editor::theme::Theme,
                ) -> editor::document::Document
                + Send
                + Sync,
        >,
    ) {
        store.update::<DemoDocs>(|docs| {
            docs.0.insert_mut(name.to_owned(), (location, build));
        });
    }
}
