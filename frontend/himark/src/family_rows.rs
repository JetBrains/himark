// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::store::Store;

#[derive(Clone, PartialEq, Debug)]
pub enum FamilyRow {
    Terminal(String),

    Pair(crate::DiffViewId),

    Chat(crate::higent::ahp_types::common::Uri),

    Canvas(crate::diff_canvas::CanvasSource),
}

pub type RowMinter =
    dyn Fn(&Store, &FamilyRow) -> Option<Box<dyn crate::DynPanelView>> + Send + Sync;

/// A store-state sync callback, invoked on the app's sync tick. Lets a
/// plugin keep its own store-held collection current in step with the
/// document/diff/changeset state — e.g. hidiff reconciling its
/// `Canvases` when the change set moves, independent of any panel
/// painting (docs/editor/diff-canvas.md §7).
pub type SyncObserver = dyn Fn(&mut Store, &imba::UiCtx) + Send + Sync;

#[derive(Clone, Default)]
pub struct SyncObservers(rpds::VectorSync<Arc<SyncObserver>>);

impl SyncObservers {
    pub fn register(store: &mut Store, observer: Arc<SyncObserver>) {
        store.update::<SyncObservers>(|observers| {
            observers.0.push_back_mut(observer);
        });
    }

    /// Run every registered observer against the store. Called once per
    /// sync tick, after the diff/stripe lanes.
    pub fn run(store: &mut Store, ui: &imba::UiCtx) {
        let observers: Vec<Arc<SyncObserver>> = match store.get::<SyncObservers>() {
            Some(observers) => observers.0.iter().cloned().collect(),
            None => return,
        };
        for observer in observers {
            observer(store, ui);
        }
    }
}

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
