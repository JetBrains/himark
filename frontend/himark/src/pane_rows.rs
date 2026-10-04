// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The session session's RE-MINTABLE rows: which panes a session can
//! stand back up (terminals, chats, tracked pairs, canvases), erased
//! behind `hikit::pane_row::PaneRow`. The typed rows live here while their
//! features still do; each moves out with its feature crate.

use std::sync::Arc;

use imba::store::Store;

pub use ::canvas::{CanvasRow, PairRow};
pub use ::terminals::pane::TerminalRow;
pub use hikit::pane_row::PaneRow;
use hikit::panel::RowMinter;

/// A chat pane's row: the collection and the conversation.
pub use ::ahp_chat::chats::ChatRow;

#[derive(Clone, Default)]
pub struct RowMinters(pub(crate) rpds::VectorSync<Arc<RowMinter>>);

impl RowMinters {
    pub fn register(store: &mut Store, minter: Arc<RowMinter>) {
        crate::registry::Registry::update(store, |registry| {
            registry.row_minters.0.push_back_mut(minter);
        });
    }
}

pub fn mint(store: &Store, row: &PaneRow) -> Option<Box<dyn crate::DynPanelView>> {
    // The row carries its collection: a pane is minted off the id,
    // and a dismantled chat has no home to walk back to.
    if let Some(ChatRow(chats, chat)) = row.row::<ChatRow>() {
        return store
            .entity(*chats)
            .filter(|rows| rows.holds(chat))
            .map(|_| {
                Box::new(ahp_chat::chats::ChatPane::new(*chats, chat.clone()))
                    as Box<dyn crate::DynPanelView>
            });
    }
    crate::registry::Registry::of(store)?
        .row_minters
        .0
        .iter()
        .find_map(|minter| minter(store, row))
}

/// The rows of one session that no pane fronts yet — the caller hands
/// the session (a window's), never a session to look up.
pub fn mint_unfronted(
    store: &Store,
    state: &ahp_session::session::SessionState,
    fronted: &[PaneRow],
) -> Vec<Box<dyn crate::DynPanelView>> {
    let mut rows: Vec<PaneRow> = Vec::new();
    {
        rows.extend(
            crate::terminal::Terminals::list(store, state.terminals())
                .into_iter()
                .map(|id| PaneRow::new(TerminalRow(state.terminals(), id))),
        );
        rows.extend(
            crate::OpenDocuments::pair_ids(store, state.documents())
                .into_iter()
                .map(|pair| PaneRow::new(PairRow(state.documents(), pair))),
        );
        rows.extend(
            ahp_chat::chats::Chats::list(store, state.chats())
                .into_iter()
                .map(|chat| PaneRow::new(ChatRow(state.chats(), chat))),
        );
    }
    rows.retain(|row| !fronted.contains(row));
    rows.iter().filter_map(|row| mint(store, row)).collect()
}
