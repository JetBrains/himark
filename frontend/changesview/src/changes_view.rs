// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The ONE tree view over change sets (docs/model-view.md): the
//! changes dock and the history dock are the same keyed tree — a
//! `ChangesView` shows an arbitrary amount of change sets at once,
//! and the changes/history difference is the DATA it derives its
//! rows from (`ViewSets`), not a type. Rows are keyed by
//! `ResourceLocation`; a change set's rows are locatable inside any
//! uniting view through the row items it minted.
//!
//! Views live on the `ChangeSets` COLLECTION (not inside one
//! `ChangeSet` — a uniting view spans many sets): the collection is
//! the point of gravity, and it also keeps the many-to-many
//! `ChangeSetId ↔ ChangesViewId` join. A set mutation marks exactly
//! its viewers stale (`nudge_set`), and the batch-tail lane
//! (`sync_changes_views`) rolls them in the same batch the feed
//! landed in. History just manages the commit sets, populating them
//! from commits — the views it once owned live here.

use std::sync::Arc;

use crate::hichanges::{ChangeSets, Changes};
use crate::hihistory::History;
use editor::location::ResourceLocation;
use hikit::{
    forest::ForestList, forest::ForestNode, forest::ForestSearcher, list_keyboard::ListKeyCommand,
    list_keyboard::ListKeyboardController, modal::ModalRequest, tree_item::TreeListCommand,
};
use imba::list::ActivateTrigger;
use imba::list::ListOps;
use imba::thunk_ext::ThunkExt;
use imba::tooltip::{TooltipCommand, TooltipView};
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::Effects,
    event::{Event, EventResult, Key as InputKey},
    leaf::leaf,
    store::Store,
    ui::UiCtx,
    View,
};
use skia_safe::Size;

const PANEL_PAD: f32 = 6.0;

/// What a row DOES when activated — the whole changes/history
/// behavioural difference, carried as data minted by the node
/// builders (`hichanges::folder_node`, `hihistory::graph_node`).
#[derive(Clone)]
pub enum RowItem {
    /// A toggling branch row. `select` mirrors the builder's habit:
    /// history rows move the selection, changes directories don't.
    Branch {
        select: bool,
    },

    /// A row that opens (or reveals inside) a diff canvas.
    Open {
        source: crate::hichanges::CanvasSource,
        reveal: Option<ResourceLocation>,
        toggle: bool,
        select: bool,
    },

    /// The history tail row: grow the folder's commit page.
    Grow {
        folder: ResourceLocation,
    },

    Note,
}

/// The data a view derives its rows from — the only place the
/// changes and history docks differ.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewSets {
    /// One working-copy change set per session folder.
    WorkingCopies,

    /// Each folder's commit list, every commit backed by its own
    /// change set (content lazy).
    History,
}

pub type Rows = TooltipView<
    ListKeyboardController<ForestList<ResourceLocation>, ForestSearcher<ResourceLocation>>,
    crate::hihistory::CommitTip,
>;

#[derive(Clone)]
pub enum ChangesViewCommand {
    Rows(TooltipCommand<ListKeyCommand<TreeListCommand>>),

    /// Refetch one repository's changesets (the root-row chip).
    Refetch(ResourceLocation),

    /// The layout-bound load-more probe hit the tail.
    AutoGrow,

    Dismiss,
}

impl std::fmt::Display for ChangesViewCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangesViewCommand::Rows(command) => command.fmt(out),
            ChangesViewCommand::Refetch(_) => out.write_str("changes refetch"),
            ChangesViewCommand::AutoGrow => out.write_str("changes auto grow"),
            ChangesViewCommand::Dismiss => out.write_str("changes dismiss"),
        }
    }
}

/// How a row OPENS a canvas: a verb built by whoever mounted the
/// view — the shell wires its window in; the tree never holds one.
pub type CanvasOpener = Arc<
    dyn Fn(crate::hichanges::CanvasSource, Option<ResourceLocation>) -> imba::command::Verb
        + Send
        + Sync,
>;

/// The unified tree view record — store truth, owned by the
/// `ChangeSets` collection, referenced from panes by id.
pub struct ChangesView {
    list: Rows,
    items: rpds::HashTrieMapSync<ResourceLocation, RowItem>,

    /// The collection whose sets this view unites — the store road.
    changes: imba::store::Id<ChangeSets>,

    /// The canvas-open verb, injected at mount (the shell's window
    /// rides inside the closure, never in the view).
    open_canvas: CanvasOpener,

    sets: ViewSets,

    /// The last `more` cursor auto-grown per folder — history data;
    /// inert while the view shows working copies.
    grown: rpds::HashTrieMapSync<ResourceLocation, String>,

    request: Option<ModalRequest>,
}

impl Clone for ChangesView {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            items: self.items.clone(),
            changes: self.changes,
            open_canvas: self.open_canvas.clone(),
            sets: self.sets,
            grown: self.grown.clone(),

            request: None,
        }
    }
}

impl ChangesView {
    pub fn open(
        store: &Store,
        ui: &UiCtx,
        changes: imba::store::Id<ChangeSets>,
        sets: ViewSets,
        open_canvas: CanvasOpener,
    ) -> Self {
        let mut view = Self {
            list: TooltipView::new(
                ListKeyboardController::searchable(
                    ForestList::new(store),
                    ForestSearcher::default(),
                    store,
                    ui,
                    editor::env::Fonts::of(store),
                )
                .with_folds(),
                move |rows, store, point| {
                    let history = Changes::of(store, changes)?.history();
                    crate::hihistory::commit_tip(rows, store, history, point)
                },
            ),
            items: rpds::HashTrieMapSync::new_sync(),
            changes,
            open_canvas,
            sets,
            grown: rpds::HashTrieMapSync::new_sync(),
            request: None,
        };
        view.refresh(store, ui);
        view
    }

    pub fn sets(&self) -> ViewSets {
        self.sets
    }

    pub fn row_count(&self) -> usize {
        self.list.view().inner().list().len()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rows(&self) -> Vec<(u8, String, bool)> {
        self.list.view().inner().forest.rows_trailed()
    }

    #[doc(hidden)]
    pub fn cursor_name(&self) -> Option<String> {
        self.list
            .view()
            .inner()
            .list()
            .cursor()
            .map(|key| key.name().to_owned())
    }

    pub fn refresh(&mut self, store: &Store, ui: &UiCtx) {
        let mut items = rpds::HashTrieMapSync::new_sync();
        let chat = editor::env::Themes::of(store).ui().chat.clone();
        let counts = (chat.added_color.0, chat.removed_color.0);
        let history = Changes::of(store, self.changes).map(|held| held.history());
        let nodes: Vec<ForestNode<ResourceLocation>> = Changes::folders(store, self.changes)
            .iter()
            .map(|folder| match (self.sets, history) {
                (ViewSets::History, Some(history)) => crate::hihistory::graph_node(
                    store,
                    history,
                    folder,
                    History::folder(store, history, folder).as_ref(),
                    &mut items,
                    counts,
                ),
                _ => crate::hichanges::folder_node(
                    folder,
                    Changes::folder(store, self.changes, folder).as_ref(),
                    &mut items,
                    counts,
                ),
            })
            .collect();

        // A commit row arrives collapsed; only rows the view has
        // never shown are preset, so a user's expansion survives.
        for (key, item) in items.iter() {
            if matches!(item, RowItem::Open { toggle: true, .. }) && !self.items.contains_key(key) {
                self.list
                    .view_mut()
                    .inner_mut()
                    .forest
                    .preset_collapsed(key);
            }
        }
        self.items = items;
        self.list.view_mut().inner_mut().set(&nodes, store, ui);
    }

    /// The change sets this view currently unites — the join rows
    /// `Changes::register` records. Every displayed folder's
    /// working-copy set anchors the view (folder-structural nudges
    /// route through it); a history view adds each commit's set.
    fn displayed_sets(&self, store: &Store) -> Vec<crate::hichanges::ChangeSetId> {
        let mut sets = Vec::new();
        let history = Changes::of(store, self.changes).map(|held| held.history());
        for folder in Changes::folders(store, self.changes) {
            sets.extend(Changes::id_for_folder(store, self.changes, &folder));
            if let (ViewSets::History, Some(history)) = (self.sets, history) {
                if let Some(entry) = History::folder(store, history, &folder) {
                    sets.extend(entry.commits.iter().map(|commit| commit.change_set));
                }
            }
        }
        sets
    }

    fn activate(&mut self, index: usize, store: &Store, ui: &UiCtx) {
        let Some(key) = self.list.view().inner().list().key_at(index).cloned() else {
            return;
        };
        self.activate_key(&key, store, ui);
    }

    fn activate_key(&mut self, key: &ResourceLocation, store: &Store, ui: &UiCtx) {
        match self.items.get(key).cloned() {
            Some(RowItem::Branch { select }) => {
                if select {
                    self.list
                        .view_mut()
                        .inner_mut()
                        .list_mut()
                        .select_only(key.clone());
                }
                self.list.view_mut().inner_mut().toggle(key, store, ui);
            }
            Some(RowItem::Open {
                source,
                reveal,
                toggle,
                select,
            }) => {
                if select {
                    self.list
                        .view_mut()
                        .inner_mut()
                        .list_mut()
                        .select_only(key.clone());
                }
                if toggle {
                    self.list.view_mut().inner_mut().toggle(key, store, ui);
                }
                self.request = Some(ModalRequest::Perform((self.open_canvas)(source, reveal)));
            }
            Some(RowItem::Grow { folder }) => {
                let Some(history) = Changes::of(store, self.changes).map(|held| held.history())
                else {
                    return;
                };
                self.request = Some(ModalRequest::Perform(imba::command::Verb::Dynamic(
                    Arc::new(crate::hihistory::GrowHistory { history, folder }),
                )));
            }
            Some(RowItem::Note) | None => {}
        }
    }
}

impl View for ChangesView {
    type Command = ChangesViewCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ChangesViewCommand> {
        use imba::focus::FocusData;
        // The key table is the controller's; the surface keeps only
        // its own dismissal.
        let searching = self.list.view().searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => EventResult::Command(ChangesViewCommand::Dismiss),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(
            self.list
                .focus_data(store, ui)
                .map(ChangesViewCommand::Rows),
        )
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        // Teardown-only: `View::destroy` carries no UiCtx.
        let ui = &imba::ui::UiCtx::dont_use_too_slow();
        fx.scope(
            |command| ChangesViewCommand::Rows(TooltipCommand::Host(command)),
            |fx| self.list.view_mut().clear(store, ui, fx),
        );
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            ChangesViewCommand::Rows(command) => {
                match &command {
                    // The bespoke lazy fold: expanding an unfetched
                    // commit launches its file listing first.
                    TooltipCommand::Host(ListKeyCommand::Fold { expand, .. }) => {
                        let expand = *expand;
                        if expand {
                            if let Some(key) = self.list.view().inner().list().cursor().cloned() {
                                if let Some(RowItem::Open {
                                    source: crate::hichanges::CanvasSource::Commit { folder, id },
                                    reveal: None,
                                    ..
                                }) = self.items.get(&key).cloned()
                                {
                                    let unfetched = Changes::commit_generation(
                                        store,
                                        self.changes,
                                        &folder,
                                        &id,
                                    ) == 0;
                                    let history =
                                        Changes::of(store, self.changes).map(|held| held.history());
                                    if let (true, Some(history)) = (unfetched, history) {
                                        History::ask(
                                            store,
                                            history,
                                            crate::hihistory::HistoryAsk::CommitFiles(folder, id),
                                        );
                                    }
                                }
                            }
                        }
                        return self
                            .list
                            .view_mut()
                            .inner_mut()
                            .fold_cursor(expand, store, ui);
                    }
                    TooltipCommand::Host(ListKeyCommand::Inner(inner)) => {
                        if let Some(index) = hikit::tree_item::tree_action(inner) {
                            if let Some(folder) =
                                self.list.view().inner().list().key_at(index).cloned()
                            {
                                return self.perform(
                                    store,
                                    ui,
                                    ChangesViewCommand::Refetch(folder),
                                    fx,
                                );
                            }
                        }
                        if let Some(index) = hikit::tree_item::tree_toggle(inner) {
                            return self.activate(index, store, ui);
                        }
                    }
                    _ => {}
                }
                if let Some((index, trigger)) = Rows::activated(&command) {
                    let searching = self.list.view().searching();
                    self.activate(index, store, ui);
                    match trigger {
                        // The deliberate pick ends the search in the
                        // same stroke.
                        ActivateTrigger::Enter if searching => {
                            return self.perform(
                                store,
                                ui,
                                ChangesViewCommand::Rows(TooltipCommand::Host(
                                    ListKeyCommand::Clear,
                                )),
                                fx,
                            );
                        }
                        ActivateTrigger::Enter | ActivateTrigger::Click => {}
                    }
                }
                fx.scope(ChangesViewCommand::Rows, |fx| {
                    self.list.perform(store, ui, command, fx)
                });
            }

            ChangesViewCommand::Refetch(folder) => {
                Changes::ask_refetch(store, self.changes, Some(folder));
            }

            ChangesViewCommand::AutoGrow => {
                let Some(history) = Changes::of(store, self.changes).map(|held| held.history())
                else {
                    return;
                };
                for folder in Changes::folders(store, self.changes) {
                    let Some(entry) = History::folder(store, history, &folder) else {
                        continue;
                    };
                    let Some(more) = entry.more else { continue };
                    if self.grown.get(&folder).map(String::as_str) == Some(more.as_str()) {
                        continue;
                    }
                    self.grown.insert_mut(folder.clone(), more);
                    History::ask(store, history, crate::hihistory::HistoryAsk::Grow(folder));

                    break;
                }
            }

            ChangesViewCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let mut section = container(arena, size);

            // The commit composer lives in the diff canvas's first
            // row and REFRESH rides each repository's root row — the
            // dock is just the tree.
            let band = PANEL_PAD;
            let rows = imba::layout::Layout::layout(
                self.list.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(size.width, (size.height - band).max(1.0))),
            )
            .map(ChangesViewCommand::Rows);
            section.place(0.0, band, rows);

            // The key table lives in the controller's own overlay;
            // the surface keeps only its dismissal.
            let searching = self.list.view().searching();
            let keymap = leaf::<ChangesViewCommand>(size.width, size.height).event(
                move |_arena, event, _size| match event {
                    Event::KeyDown {
                        key: InputKey::Escape,
                        ..
                    } if !searching => EventResult::Command(ChangesViewCommand::Dismiss),
                    _ => EventResult::Ignored,
                },
            );
            section.place(0.0, 0.0, keymap);

            // The load-more probe is genuinely LAYOUT-BOUND (it
            // reads the laid viewport's nearness to the tail), so it
            // stays on paint. Working-copy views never page — `grow`
            // is simply never true for them.
            let rows_height = (size.height - band).max(1.0);
            let near_tail = {
                let list = self.list.view().inner();
                list.scroll_y() + rows_height
                    >= list.list().total_height() - 2.0 * hikit::ui::space::XL
            };
            let history = Changes::of(store, self.changes).map(|held| held.history());
            let pageable = self.sets == ViewSets::History
                && history.is_some_and(|history| {
                    Changes::folders(store, self.changes).iter().any(|folder| {
                        History::folder(store, history, folder).is_some_and(|entry| {
                            entry.more.as_deref().is_some_and(|more| {
                                self.grown.get(folder).map(String::as_str) != Some(more)
                            })
                        })
                    })
                });
            let grow = near_tail && pageable;
            section.wrap(move |inner| GrowShell { inner, grow })
        })
    }
}

struct GrowShell<Inner> {
    inner: Inner,

    grow: bool,
}

impl<'a, Inner: imba::Widget<'a, ChangesViewCommand>> imba::Widget<'a, ChangesViewCommand>
    for GrowShell<Inner>
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, ChangesViewCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: skia_safe::Rect,
    ) -> EventResult<ChangesViewCommand> {
        let mut result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) && self.grow {
            result = result.merge(EventResult::Command(ChangesViewCommand::AutoGrow));
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, ChangesViewCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

use crate::hichanges::ChangesViewId;

/// The view registry rides the `ChangeSets` collection — the
/// records themselves, the many-to-many set↔view join, and the
/// stale marks the batch-tail lane consumes.
impl ChangeSets {
    pub fn mint_view(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        view: ChangesView,
    ) -> ChangesViewId {
        let id = ChangesViewId::mint();
        let displayed = view.displayed_sets(store);
        Self::update(store, changes, |views| {
            views.views.insert_mut(id, Box::new(view));
        });
        Self::register(store, changes, id, displayed);
        id
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn view_ref<'a>(
        store: &'a Store,
        changes: imba::store::Id<ChangeSets>,
        id: ChangesViewId,
    ) -> Option<&'a ChangesView> {
        Self::of(store, changes)?
            .views
            .get(&id)?
            .as_any()
            .downcast_ref::<ChangesView>()
    }

    fn take_view(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        id: ChangesViewId,
    ) -> Option<ChangesView> {
        let view = Self::view_ref(store, changes, id)?.clone();
        Self::update(store, changes, |views| {
            views.views.remove_mut(&id);
        });
        Some(view)
    }

    fn put_view(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        id: ChangesViewId,
        view: ChangesView,
    ) {
        Self::update(store, changes, |views| {
            views.views.insert_mut(id, Box::new(view));
        });
    }

    fn remove_view(store: &mut Store, changes: imba::store::Id<ChangeSets>, id: ChangesViewId) {
        Self::update(store, changes, |views| {
            views.views.remove_mut(&id);
            views.stale.remove_mut(&id);
            let orphaned: Vec<crate::hichanges::ChangeSetId> = views
                .viewers
                .iter()
                .filter(|(_, viewing)| viewing.contains(&id))
                .map(|(set, _)| *set)
                .collect();
            for set in orphaned {
                let Some(mut viewing) = views.viewers.get(&set).cloned() else {
                    continue;
                };
                viewing.remove_mut(&id);
                match viewing.is_empty() {
                    true => {
                        views.viewers.remove_mut(&set);
                    }
                    false => {
                        views.viewers.insert_mut(set, viewing);
                    }
                }
            }
        });
    }

    /// Rebuild the join rows for one view from what it now displays
    /// — the view's code computes `displayed`; the join is model
    /// bookkeeping.
    fn register(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        id: ChangesViewId,
        displayed: Vec<crate::hichanges::ChangeSetId>,
    ) {
        Self::update(store, changes, |views| {
            // Drop the view's old rows…
            let stale_rows: Vec<crate::hichanges::ChangeSetId> = views
                .viewers
                .iter()
                .filter(|(set, viewing)| viewing.contains(&id) && !displayed.contains(set))
                .map(|(set, _)| *set)
                .collect();
            for set in stale_rows {
                let Some(mut viewing) = views.viewers.get(&set).cloned() else {
                    continue;
                };
                viewing.remove_mut(&id);
                match viewing.is_empty() {
                    true => {
                        views.viewers.remove_mut(&set);
                    }
                    false => {
                        views.viewers.insert_mut(set, viewing);
                    }
                }
            }
            // …and record the fresh ones.
            for set in displayed {
                let mut viewing = views.viewers.get(&set).cloned().unwrap_or_default();
                viewing.insert_mut(id);
                views.viewers.insert_mut(set, viewing);
            }
        });
    }

    /// The update rule: a change set moved — mark exactly its
    /// uniting views stale. The batch-tail lane rolls them.
    pub fn nudge_set(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        set: crate::hichanges::ChangeSetId,
    ) {
        Self::update(store, changes, |views| views.nudge_set_in_place(set));
    }

    /// The same marks, on the row itself — what a landing under the
    /// collection's own lease does.
    pub(crate) fn nudge_set_in_place(&mut self, set: crate::hichanges::ChangeSetId) {
        let Some(viewing) = self.viewers.get(&set).cloned() else {
            return;
        };
        for id in viewing.iter() {
            self.stale.insert_mut(*id);
        }
    }

    pub(crate) fn nudge_folder_in_place(&mut self, folder: &ResourceLocation) {
        let source = crate::hichanges::ChangeSetSource::WorkingCopy {
            folder: folder.clone(),
        };
        match self.by_source.get(&source).copied() {
            Some(anchor) => self.nudge_set_in_place(anchor),
            None => self.nudge_all(),
        }
    }

    pub(crate) fn nudge_all(&mut self) {
        let ids: Vec<ChangesViewId> = self.views.keys().copied().collect();
        for id in ids {
            self.stale.insert_mut(id);
        }
    }

    /// A folder-level structural shift (catalog adopted, status
    /// flipped, commit page landed): route through the folder's
    /// working-copy ANCHOR set — every view showing the folder joins
    /// on it. No anchor yet means no view derived rows from the
    /// folder either, but the views still show its placeholder
    /// notes, so fall back to marking everything.
    pub fn nudge_folder(
        store: &mut Store,
        changes: imba::store::Id<ChangeSets>,
        folder: &ResourceLocation,
    ) {
        match Changes::id_for_folder(store, changes, folder) {
            Some(anchor) => Self::nudge_set(store, changes, anchor),
            None => Self::nudge_all_in(store, changes),
        }
    }

    pub fn nudge_all_in(store: &mut Store, changes: imba::store::Id<ChangeSets>) {
        Self::update(store, changes, |views| views.nudge_all());
    }
}

/// The PUSH lane for both docks: at the batch tail, refresh every
/// stale view of the gathered session — the same batch its feed
/// landed in, no paint probe. `ChangeSets` is session state
/// (gather/park), so every view in the record belongs to the
/// gathered session already; the workspace guard just asserts that
/// invariant.
pub fn sync_changes_views(store: &mut Store, changes: imba::store::Id<ChangeSets>, ui: &UiCtx) {
    let stale: Vec<ChangesViewId> = Changes::of(store, changes)
        .map(|views| views.stale.iter().copied().collect())
        .unwrap_or_default();
    for id in stale {
        let Some(mut view) = Changes::take_view(store, changes, id) else {
            continue;
        };
        view.refresh(store, ui);
        let displayed = view.displayed_sets(store);
        Changes::put_view(store, changes, id, view);
        Changes::register(store, changes, id, displayed);
        Changes::update(store, changes, |views| {
            views.stale.remove_mut(&id);
        });
    }
}

/// The dock's reference view over a store-held record (no store).
/// Its death removes the record: tree rows are pure derivation,
/// nothing is lost on close.
pub struct ChangesPane {
    /// The collection whose registry holds this dock's view record.
    changes: imba::store::Id<ChangeSets>,
    view: ChangesViewId,
    request: Option<ModalRequest>,
}

impl ChangesPane {
    pub fn new(changes: imba::store::Id<ChangeSets>, view: ChangesViewId) -> Self {
        Self {
            changes,
            view,
            request: None,
        }
    }

    pub fn view(&self) -> ChangesViewId {
        self.view
    }

    /// The collection whose registry holds this dock's view.
    pub fn changes(&self) -> imba::store::Id<ChangeSets> {
        self.changes
    }
}

impl Clone for ChangesPane {
    fn clone(&self) -> Self {
        Self {
            changes: self.changes,
            view: self.view,
            request: None,
        }
    }
}

impl View for ChangesPane {
    type Command = ChangesViewCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ChangesViewCommand> {
        match Changes::view_ref(store, self.changes, self.view) {
            Some(view) => view.focus_data(store, ui),
            None => imba::focus::FocusData::default(),
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        let Some(mut view) = Changes::take_view(store, self.changes, self.view) else {
            return;
        };
        view.perform(store, ui, command, fx);
        // Requests are the PANE's ask (`take_request` has no store):
        // pull what the record minted into the reference view.
        if let Some(request) = view.request.take() {
            self.request = Some(request);
        }
        Changes::put_view(store, self.changes, self.view, view);
    }

    fn destroy(&mut self, store: &mut Store, _fx: &mut imba::effect::Effects<'_, Self::Command>) {
        Changes::remove_view(store, self.changes, self.view);
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(
            move |_arena: &'a Arena, constraints: imba::constraints::Constraints| {
                let widget: imba::ThunkBox<'a, ChangesViewCommand> =
                    match Changes::view_ref(store, self.changes, self.view) {
                        Some(view) => imba::ThunkBox::new(
                            arena,
                            imba::layout::Layout::layout(
                                view.display(arena, store, ui),
                                arena,
                                constraints,
                            ),
                        ),
                        None => imba::ThunkBox::new(
                            arena,
                            leaf(constraints.max.width, constraints.max.height),
                        ),
                    };
                widget
            },
        )
    }
}

impl hikit::modal::ModalView for ChangesPane {
    fn clone_modal(&self) -> Box<dyn hikit::modal::ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
