// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Find results tree, as a component: the dirs → files →
//! occurrences forest over ONE feed, with its keyboard table, fold
//! handling, pick lookup and by-key rebuild — shared by the Search
//! dock tab and the go-to peeker. The faces differ in head and
//! policy (the dock navigates on every selection; the peeker
//! previews and opens on the pick), so the component answers
//! OUTCOMES and the face decides.

use imba::{arena::Arena, store::Store, ui::UiCtx, View as _};

use crate::views::{locations_forest, result_row, ResultRow};
use crate::{FeedId, FoundLocation, LocationKey, LocationLists};
use hikit::{forest::ForestList, forest::ForestSearcher};
use hikit::{list_keyboard::ListKeyCommand, list_keyboard::ListKeyboardController};
use hikit::{tree_item::tree_toggle, tree_item::TreeListCommand};
use imba::list::{ActivateTrigger, ListOps};

/// The results tree with its key table — the half of a face that
/// SCROLLS; whatever stands above it (a query band, a count line) is
/// the face's own.
pub(crate) type ResultTree = ListKeyboardController<
    ForestList<LocationKey, ResultRow>,
    ForestSearcher<LocationKey, ResultRow>,
>;

pub type ResultsCommand = imba::scroll::ScrollCommand<ListKeyCommand<TreeListCommand>>;

/// What a performed command amounted to — the face's policy hook.
pub enum ResultsOutcome {
    Quiet,
    /// The cursor's row changed (a keyboard step, a click's select, a
    /// fold re-aiming it, a search step).
    Selected,
    /// A pickable row was activated: Enter is the deliberate jump, a
    /// click browses (docs/ui/location-list.md).
    Picked {
        key: LocationKey,
        trigger: ActivateTrigger,
    },
}

#[derive(Clone)]
pub struct ResultsView {
    lists: imba::store::Id<LocationLists>,

    /// The tree in its OWN scroller: the face's head never scrolls,
    /// and the tree's reveals (cursor steps, door corrections) land
    /// here instead of dragging the whole panel.
    scroll: imba::scroll::ScrollView<ResultTree>,

    /// Pick lookup: a hit key answers its INDEX into the feed (a
    /// file key its first occurrence) — indices, never copies; the
    /// feed row is the one holder of the contexts.
    targets: rpds::HashTrieMapSync<LocationKey, usize>,
    files: usize,
    shown: Option<(FeedId, u64)>,
}

impl ResultsView {
    pub fn new(store: &Store, ui: &UiCtx, lists: imba::store::Id<LocationLists>) -> Self {
        Self {
            lists,
            scroll: imba::scroll::ScrollView::new(
                ListKeyboardController::searchable(
                    ForestList::wrapping(store, result_row(lists)),
                    ForestSearcher::default(),
                    store,
                    ui,
                )
                .with_folds(),
            ),
            targets: rpds::HashTrieMapSync::new_sync(),
            files: 0,
            shown: None,
        }
    }

    /// Keys only — for a face whose own input owns typing (the
    /// peeker): movement, folds and Enter, no type-ahead lane.
    pub fn keys_only(store: &Store, lists: imba::store::Id<LocationLists>) -> Self {
        Self {
            lists,
            scroll: imba::scroll::ScrollView::new(
                ListKeyboardController::keys_only(ForestList::wrapping(store, result_row(lists)))
                    .with_folds(),
            ),
            targets: rpds::HashTrieMapSync::new_sync(),
            files: 0,
            shown: None,
        }
    }

    pub(crate) fn tree(&self) -> &ResultTree {
        self.scroll.content()
    }

    pub(crate) fn tree_mut(&mut self) -> &mut ResultTree {
        self.scroll.content_mut()
    }

    #[cfg(test)]
    pub(crate) fn shown(&self) -> Option<(FeedId, u64)> {
        self.shown
    }

    /// Distinct files among the shown locations.
    pub fn files(&self) -> usize {
        self.files
    }

    /// Rows currently emitted by the forest.
    pub fn len(&self) -> usize {
        self.tree().inner().list().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The type-ahead search is open over the rows.
    pub fn searching(&self) -> bool {
        self.tree().searching()
    }

    pub fn cursor(&self) -> Option<LocationKey> {
        self.tree().inner().list().cursor().cloned()
    }

    /// The feed moved past what the tree shows — the face's refresh
    /// probe reads this each paint.
    pub fn stale(&self, store: &Store, feed: Option<FeedId>) -> bool {
        let generation = feed
            .and_then(|feed| LocationLists::row_ref(store, self.lists, feed))
            .map(|row| row.generation)
            .unwrap_or(0);
        self.shown != feed.map(|feed| (feed, generation))
    }

    /// The location a key stands for, read from the feed row.
    pub fn found(
        &self,
        store: &Store,
        feed: Option<FeedId>,
        key: &LocationKey,
    ) -> Option<FoundLocation> {
        let index = self.targets.get(key).copied()?;
        feed.and_then(|feed| LocationLists::row_ref(store, self.lists, feed))
            .and_then(|row| row.locations.get(index))
            .cloned()
    }

    /// Rebuild the tree and the pick table from the feed — cursor
    /// and fold state survive by key. Location keys ride Arc'd
    /// paths (O(1) clones); the contexts are never copied here.
    pub fn rebuild(&mut self, store: &Store, ui: &UiCtx, feed: Option<FeedId>) {
        let row = feed
            .and_then(|feed| LocationLists::row(store, self.lists, feed))
            .unwrap_or_default();
        let forest = locations_forest(store, row.locations.iter());

        let mut targets = rpds::HashTrieMapSync::new_sync();
        let mut files = 0usize;
        for (index, found) in row.locations.iter().enumerate() {
            let hit = LocationKey::Hit(found.location.clone(), found.line, found.column);
            targets.insert_mut(hit, index);
            let file = LocationKey::Node(found.location.clone());
            if !targets.contains_key(&file) {
                files += 1;
                targets.insert_mut(file, index);
            }
        }
        self.targets = targets;
        self.files = files;
        self.shown = feed.map(|feed| (feed, row.generation));

        let cursor = self.tree().inner().list().cursor().cloned();
        self.tree_mut().inner_mut().set(&forest, store, ui);
        if let Some(cursor) = cursor {
            if self.tree().inner().forest.contains(&cursor) {
                // QUIETLY: a rebuild re-asserts, it never scrolls —
                // landing batches teleported the tree to the cursor's
                // row on every landing when this revealed.
                self.tree_mut()
                    .inner_mut()
                    .list_mut()
                    .reselect_quiet(cursor);
            }
        }
    }

    /// A NEW result set: the old cursor, door anchor and scroll mean
    /// nothing against it — preserving them re-aimed the tree at
    /// whatever row shared an old key. The tree starts at the top;
    /// the first landing re-seeds the cursor, quietly.
    pub fn reset(&mut self, store: &Store, ui: &UiCtx, feed: Option<FeedId>) {
        self.tree_mut().inner_mut().list_mut().clear_selection();
        self.tree_mut().inner_mut().list_mut().reset_viewport();
        self.scroll.set_scroll_y(0.0);
        self.rebuild(store, ui, feed);
    }

    /// Run a tree command: folds, toggle chips and activations are
    /// interpreted here; everything else forwards to the scroller.
    /// Answers what it amounted to — navigation is the FACE's.
    pub fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: ResultsCommand,
        fx: &mut imba::effect::Effects<'_, ResultsCommand>,
    ) -> ResultsOutcome {
        let mut selected = false;
        // The tree's own commands ride inside the scroller's; the
        // scroll commands themselves just pass through.
        if let imba::scroll::ScrollCommand::Content(list_command) = &command {
            match list_command {
                ListKeyCommand::Fold { expand, .. } => {
                    self.tree_mut().inner_mut().fold_cursor(*expand, store, ui);
                    return ResultsOutcome::Selected;
                }
                ListKeyCommand::Inner(inner) => {
                    if let Some(index) = tree_toggle(inner) {
                        let Some(key) = self.tree().inner().list().key_at(index).cloned() else {
                            return ResultsOutcome::Quiet;
                        };
                        self.tree_mut()
                            .inner_mut()
                            .list_mut()
                            .select_only(key.clone());
                        self.tree_mut().inner_mut().toggle(&key, store, ui);
                        return ResultsOutcome::Quiet;
                    }
                }
                _ => {}
            }
            if let Some((index, trigger)) = ResultTree::activated(list_command) {
                if let Some(key) = self.tree().inner().list().key_at(index).cloned() {
                    let branch = matches!(&key, LocationKey::Node(location)
                        if location.kind().is_directory());
                    if branch {
                        self.tree_mut().inner_mut().toggle(&key, store, ui);
                        return ResultsOutcome::Quiet;
                    }
                    return ResultsOutcome::Picked { key, trigger };
                }
            }
            selected = ResultTree::selected_index(list_command).is_some();
        }
        self.scroll.perform(store, ui, command, fx);
        match selected {
            true => ResultsOutcome::Selected,
            false => ResultsOutcome::Quiet,
        }
    }

    pub fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ResultsCommand> {
        self.scroll.focus_data(store, ui)
    }

    /// The tree band itself; the face lays it tight, maps the command
    /// and plants its sticky bands (`imba::list::STICKY_HOST`).
    pub fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, ResultsCommand> + imba::layout::LayoutValue + 'a {
        let _ = arena;
        self.scroll.display(arena, store, ui)
    }

    /// The tree band's own scroll offset.
    pub fn scroll_y(&self) -> f32 {
        self.scroll.scroll_y()
    }

    /// The tree's own extent, for faces probed unbounded.
    pub fn laid_width(&self) -> f32 {
        self.tree().inner().list().laid_width()
    }

    pub fn total_height(&self) -> f32 {
        self.tree().inner().list().total_height()
    }
}
