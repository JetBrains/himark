// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::store::Store;

#[derive(Clone, PartialEq, Debug)]
pub enum FamilyRow {
    Terminal(
        imba::store::Id<crate::terminal::Terminals>,
        crate::terminal::TerminalId,
    ),

    Pair(imba::store::Id<crate::OpenDocuments>, crate::DiffViewId),

    Chat(
        imba::store::Id<crate::higent::Chats>,
        crate::higent::ChatUri,
    ),

    Canvas(crate::diff_canvas::CanvasSource),
}

pub type RowMinter =
    dyn Fn(&Store, &FamilyRow) -> Option<Box<dyn crate::DynPanelView>> + Send + Sync;

#[derive(Clone, Default)]
pub struct RowMinters(pub(crate) rpds::VectorSync<Arc<RowMinter>>);

impl RowMinters {
    pub fn register(store: &mut Store, minter: Arc<RowMinter>) {
        crate::registry::Registry::update(store, |registry| {
            registry.row_minters.0.push_back_mut(minter);
        });
    }
}

pub fn mint(store: &Store, row: &FamilyRow) -> Option<Box<dyn crate::DynPanelView>> {
    match row {
        // A row carries its collection: the pane is minted off the id
        // while the record still stands.
        FamilyRow::Terminal(terminals, id) => store
            .entity(*terminals)
            .filter(|rows| rows.holds(*id))
            .map(|_| {
                Box::new(crate::terminal::TerminalView::new(*terminals, *id))
                    as Box<dyn crate::DynPanelView>
            }),
        // The row carries its collection: a pane is minted off the id,
        // and a dismantled chat has no home to walk back to.
        FamilyRow::Chat(chats, chat) => {
            store
                .entity(*chats)
                .filter(|rows| rows.holds(chat))
                .map(|_| {
                    Box::new(crate::higent::ChatPane::new(*chats, chat.clone()))
                        as Box<dyn crate::DynPanelView>
                })
        }
        row => crate::registry::Registry::of(store)?
            .row_minters
            .0
            .iter()
            .find_map(|minter| minter(store, row)),
    }
}

/// The rows of one family that no pane fronts yet — the caller hands
/// the family (a window's), never a session to look up.
pub fn mint_unfronted(
    store: &Store,
    family: &crate::higent::SessionState,
    fronted: &[FamilyRow],
) -> Vec<Box<dyn crate::DynPanelView>> {
    let mut rows: Vec<FamilyRow> = Vec::new();
    {
        rows.extend(
            crate::terminal::Terminals::list(store, family.terminals())
                .into_iter()
                .map(|id| FamilyRow::Terminal(family.terminals(), id)),
        );
        rows.extend(
            crate::OpenDocuments::pair_ids(store, family.documents())
                .into_iter()
                .map(|pair| FamilyRow::Pair(family.documents(), pair)),
        );
        rows.extend(
            crate::higent::Chats::list(store, family.chats())
                .into_iter()
                .map(|chat| FamilyRow::Chat(family.chats(), chat)),
        );
    }
    rows.retain(|row| !fronted.contains(row));
    rows.iter().filter_map(|row| mint(store, row)).collect()
}
