// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::store::Store;

#[derive(Clone, PartialEq, Debug)]
pub enum FamilyRow {
    Terminal(crate::higent::ChannelUri),

    Pair(crate::DiffViewId),

    Chat(crate::higent::ChatUri),

    Canvas(crate::diff_canvas::CanvasSource),
}

pub type RowMinter =
    dyn Fn(&Store, &FamilyRow) -> Option<Box<dyn crate::DynPanelView>> + Send + Sync;

#[derive(Clone, Default)]
pub struct RowMinters(rpds::VectorSync<Arc<RowMinter>>);

impl RowMinters {
    pub fn register(store: &mut Store, minter: Arc<RowMinter>) {
        store.update::<RowMinters>(|minters| {
            minters.0.push_back_mut(minter);
        });
    }
}

pub fn mint(store: &Store, row: &FamilyRow) -> Option<Box<dyn crate::DynPanelView>> {
    match row {
        FamilyRow::Terminal(channel) => crate::higent::Hosts::session_of_terminal(store, channel)
            .map(|home| {
                Box::new(crate::terminal::TerminalView::new(home, channel.clone()))
                    as Box<dyn crate::DynPanelView>
            }),
        FamilyRow::Chat(chat) => crate::higent::ChatPane::of_chat(store, chat.clone())
            .map(|pane| Box::new(pane) as Box<dyn crate::DynPanelView>),
        row => store
            .get::<RowMinters>()?
            .0
            .iter()
            .find_map(|minter| minter(store, row)),
    }
}

pub fn mint_unfronted(store: &Store, fronted: &[FamilyRow]) -> Vec<Box<dyn crate::DynPanelView>> {
    let mut rows: Vec<FamilyRow> = Vec::new();
    if let Some(session) = crate::Gathered::scope(store) {
        rows.extend(
            crate::terminal::Terminals::list(store, session)
                .into_iter()
                .map(FamilyRow::Terminal),
        );
    }
    rows.extend(
        crate::OpenDocuments::pair_ids(store)
            .into_iter()
            .map(FamilyRow::Pair),
    );
    // The chats of the session this batch is gathered FOR — the family
    // rows are a session's own furniture.
    if let Some(session) = crate::Gathered::scope(store) {
        rows.extend(
            crate::higent::Chats::list(store, session)
                .into_iter()
                .map(FamilyRow::Chat),
        );
    }
    rows.retain(|row| !fronted.contains(row));
    rows.iter().filter_map(|row| mint(store, row)).collect()
}
