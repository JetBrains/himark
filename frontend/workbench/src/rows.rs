// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Pane-row minting: a registered minter turns a feature's `PaneRow`
//! into the pane it names. The rows themselves (terminal, chat, pair,
//! canvas) live with the shell that knows the features.

use std::sync::Arc;

use hikit::pane_row::PaneRow;
use hikit::panel::RowMinter;
use imba::store::Store;

#[derive(Clone, Default)]
pub struct RowMinters(pub(crate) rpds::VectorSync<Arc<RowMinter>>);

impl RowMinters {
    pub fn register(store: &mut Store, minter: Arc<RowMinter>) {
        crate::registry::Registry::update(store, |registry| {
            registry.row_minters.0.push_back_mut(minter);
        });
    }
}

pub fn mint(store: &Store, row: &PaneRow) -> Option<Box<dyn hikit::panel::DynPanelView>> {
    // The row carries its collection: a pane is minted off the id,
    // and a dismantled chat has no home to walk back to.
    if let Some(ahp_chat::chats::ChatRow(chats, chat)) = row.row::<ahp_chat::chats::ChatRow>() {
        return store
            .entity(*chats)
            .filter(|rows| rows.holds(chat))
            .map(|_| {
                Box::new(ahp_chat::chats::ChatPane::new(*chats, chat.clone()))
                    as Box<dyn hikit::panel::DynPanelView>
            });
    }
    crate::registry::Registry::of(store)?
        .row_minters
        .0
        .iter()
        .find_map(|minter| minter(store, row))
}
