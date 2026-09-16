// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::store::Store;

#[derive(Clone, PartialEq, Debug)]
pub enum FamilyRow {
    Terminal(String),

    List(crate::ListId),

    Pair(crate::DiffViewId),

    Chat(crate::higent::ahp_types::common::Uri),

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
        FamilyRow::Terminal(channel) => Some(Box::new(crate::terminal::TerminalView::new(
            channel.clone(),
        ))),
        FamilyRow::List(id) => Some(Box::new(crate::location_list::ListPanel::new(*id))),
        FamilyRow::Chat(chat) => Some(Box::new(crate::higent::ChatPane::new(chat.clone()))),
        row => store
            .get::<RowMinters>()?
            .0
            .iter()
            .find_map(|minter| minter(store, row)),
    }
}

pub fn mint_unfronted(store: &Store, fronted: &[FamilyRow]) -> Vec<Box<dyn crate::DynPanelView>> {
    let mut rows: Vec<FamilyRow> = Vec::new();
    rows.extend(
        crate::terminal::Terminals::list(store)
            .into_iter()
            .map(FamilyRow::Terminal),
    );
    rows.extend(
        crate::location_list::LocationLists::titles(store)
            .into_iter()
            .map(|(id, _)| FamilyRow::List(id)),
    );
    rows.extend(
        crate::OpenDocuments::pair_ids(store)
            .into_iter()
            .map(FamilyRow::Pair),
    );
    rows.extend(
        crate::higent::Chats::list(store)
            .into_iter()
            .map(FamilyRow::Chat),
    );
    rows.retain(|row| !fronted.contains(row));
    rows.iter().filter_map(|row| mint(store, row)).collect()
}
