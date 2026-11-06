// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The session session's RE-MINTABLE rows: which panes a session can
//! stand back up (terminals, chats, tracked pairs, canvases), erased
//! behind `hikit::pane_row::PaneRow`. The typed rows live here while their
//! features still do; each moves out with its feature crate.

use ::ahp_chat::chats::ChatRow;
use ::canvas::PairRow;
use ::terminals::pane::TerminalRow;
use hikit::pane_row::PaneRow;

use imba::store::Store;

/// A chat pane's row: the collection and the conversation.

/// The rows of one session that no pane fronts yet — the caller hands
/// the session (a window's), never a session to look up.
pub fn mint_unfronted(
    store: &Store,
    state: &ahp_session::session::state::SessionState,
    fronted: &[PaneRow],
) -> Vec<Box<dyn hikit::panel::DynPanelView>> {
    let mut rows: Vec<PaneRow> = Vec::new();
    {
        rows.extend(
            terminals::Terminals::list(store, state.terminals())
                .into_iter()
                .map(|id| PaneRow::new(TerminalRow(state.terminals(), id))),
        );
        rows.extend(
            documents::OpenDocuments::pair_ids(store, state.documents())
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
    rows.iter()
        .filter_map(|row| ::workbench::rows::mint(store, row))
        .collect()
}
