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

/// A plugin-owned store value held with a SESSION's family: gathered
/// into every store scoped to that session and taken back out on
/// scatter, exactly like the himark-owned members in
/// higent/session/state.rs. This is how a plugin collection derived
/// from session state (hidiff's `Canvases` over the `Changes` feed)
/// stays in ITS session — a batch gathered for another session simply
/// does not see it, so its staleness counters can never be compared
/// against a foreign session's.
pub type SessionFamilyValue = Arc<dyn std::any::Any + Send + Sync>;

pub struct SessionFamilyMember {
    /// Stable identity inside the per-session family map.
    pub key: &'static str,
    /// Put the held value back into a store gathered for its session.
    pub gather: fn(&SessionFamilyValue, &mut Store),
    /// Take the value out of a scattering store; `None` when empty,
    /// so an empty member leaves no residue in the family.
    pub take: fn(&mut Store) -> Option<SessionFamilyValue>,
}

/// The registry of plugin session-family members — GLOBAL state (it
/// seeds every gather), registered once at the edge beside the row
/// minter and sync observer.
#[derive(Clone, Default)]
pub struct SessionFamilies(rpds::VectorSync<Arc<SessionFamilyMember>>);

impl SessionFamilies {
    pub fn register(store: &mut Store, member: Arc<SessionFamilyMember>) {
        store.update::<SessionFamilies>(|families| {
            families.0.push_back_mut(member);
        });
    }

    pub(crate) fn members(store: &Store) -> Vec<Arc<SessionFamilyMember>> {
        store
            .get::<SessionFamilies>()
            .map(|families| families.0.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// A store-state sync callback, invoked on the app's sync tick. Lets a
/// plugin keep its own store-held collection current in step with the
/// document/diff/changeset state — e.g. hidiff reconciling its
/// `Canvases` when the change set moves, independent of any panel
/// painting (docs/editor/diff-canvas.md §7).
/// The batch's scope, handed to a sync observer so it can route the
/// effects it launches back to the same session (and window) the batch
/// ran in — the feed landing that dirtied this state ran under exactly
/// this scope (`AppCommand::dynamic_in`).
#[derive(Clone, Copy)]
pub struct SyncScope<'a> {
    pub window: Option<crate::WindowId>,
    pub session: Option<&'a crate::SessionId>,
}

/// A store-state observer with a REAL effects sink: it runs at the tail
/// of every perform batch (after the diff/stripe lanes, so it sees the
/// batch's fresh diffs), so a feed landing and the reconcile it owes —
/// membership, row heights, off-thread builds — happen in the same
/// batch. The push road, no paint involved.
pub type SyncObserver =
    dyn Fn(&mut Store, &imba::UiCtx, SyncScope<'_>, &mut crate::AppFx<'_>) + Send + Sync;

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
    pub fn run(store: &mut Store, ui: &imba::UiCtx, scope: SyncScope<'_>, fx: &mut crate::AppFx<'_>) {
        let observers: Vec<Arc<SyncObserver>> = match store.get::<SyncObservers>() {
            Some(observers) => observers.0.iter().cloned().collect(),
            None => return,
        };
        for observer in observers {
            observer(store, ui, scope, fx);
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
