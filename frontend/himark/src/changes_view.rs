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
use crate::{
    ActivateTrigger, AppCommand, ForestList, ForestNode, ForestSearcher, ListKeyCommand,
    ListKeyboardController, ModalRequest, ResourceLocation, TreeListCommand,
};
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
    UiCtx, View,
};
use skia_safe::Size;

const PANEL_PAD: f32 = 6.0;

/// What a row DOES when activated — the whole changes/history
/// behavioural difference, carried as data minted by the node
/// builders (`hichanges::folder_node`, `hihistory::graph_node`).
#[derive(Clone)]
pub(crate) enum RowItem {
    /// A toggling branch row. `select` mirrors the builder's habit:
    /// history rows move the selection, changes directories don't.
    Branch {
        select: bool,
    },

    /// A row that opens (or reveals inside) a diff canvas.
    Open {
        source: crate::diff_canvas::CanvasSource,
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

pub(crate) type Rows = TooltipView<
    ListKeyboardController<ForestList<ResourceLocation>, ForestSearcher<ResourceLocation>>,
    crate::hihistory::CommitTip,
>;

pub enum ChangesViewCommand {
    Rows(TooltipCommand<ListKeyCommand<TreeListCommand>>),

    /// Refetch one repository's changesets (the root-row chip).
    Refetch(ResourceLocation),

    /// The layout-bound load-more probe hit the tail.
    AutoGrow,

    Dismiss,
}

/// The unified tree view record — store truth, owned by the
/// `ChangeSets` collection, referenced from panes by id.
pub struct ChangesView {
    list: Rows,
    items: rpds::HashTrieMapSync<ResourceLocation, RowItem>,

    workspace: crate::SessionId,
    window: crate::WindowId,

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
            workspace: self.workspace.clone(),
            window: self.window,
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
        window: crate::WindowId,
        workspace: crate::SessionId,
        sets: ViewSets,
    ) -> Self {
        let mut view = Self {
            list: TooltipView::new(
                ListKeyboardController::searchable(
                    ForestList::new(store),
                    ForestSearcher::default(),
                    store,
                    ui,
                    crate::env::Fonts::of(store),
                )
                .with_folds(),
                {
                    let home = workspace.clone();
                    move |rows, store, point| {
                        crate::hihistory::commit_tip(rows, store, &home, point)
                    }
                },
            ),
            items: rpds::HashTrieMapSync::new_sync(),
            workspace,
            window,
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

    pub(crate) fn refresh(&mut self, store: &Store, ui: &UiCtx) {
        let mut items = rpds::HashTrieMapSync::new_sync();
        let chat = crate::env::Themes::of(store).ui().chat.clone();
        let counts = (chat.added_color.0, chat.removed_color.0);
        let nodes: Vec<ForestNode<ResourceLocation>> =
            crate::higent::session_folders(store, &self.workspace)
                .iter()
                .map(|folder| match self.sets {
                    ViewSets::WorkingCopies => crate::hichanges::folder_node(
                        folder,
                        Changes::folder(store, &self.workspace, folder).as_ref(),
                        &mut items,
                        counts,
                    ),
                    ViewSets::History => crate::hihistory::graph_node(
                        store,
                        &self.workspace,
                        folder,
                        History::folder(store, &self.workspace, folder).as_ref(),
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
        for folder in crate::higent::session_folders(store, &self.workspace) {
            sets.extend(Changes::id_for_folder(store, &self.workspace, &folder));
            if self.sets == ViewSets::History {
                if let Some(entry) = History::folder(store, &self.workspace, &folder) {
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
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(crate::diff_canvas::OpenDiffCanvas {
                        home: self.workspace.clone(),
                        source,
                        reveal,
                    }),
                )));
            }
            Some(RowItem::Grow { folder }) => {
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(crate::hihistory::GrowHistory {
                        home: self.workspace.clone(),
                        folder,
                    }),
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
        let ui = &imba::UiCtx::dont_use_too_slow();
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
                                    source: crate::diff_canvas::CanvasSource::Commit { folder, id },
                                    reveal: None,
                                    ..
                                }) = self.items.get(&key).cloned()
                                {
                                    let unfetched = Changes::commit_generation(
                                        store,
                                        &self.workspace,
                                        &folder,
                                        &id,
                                    ) == 0;
                                    if unfetched {
                                        self.request =
                                            Some(ModalRequest::Perform(AppCommand::Dynamic(
                                                self.window,
                                                Arc::new(crate::hihistory::FetchCommitFiles {
                                                    home: self.workspace.clone(),
                                                    folder,
                                                    commit: id,
                                                }),
                                            )));
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
                        if let Some(index) = crate::tree_action(inner) {
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
                        if let Some(index) = crate::tree_toggle(inner) {
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
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(crate::hichanges::RefetchChanges {
                        home: Some(self.workspace.clone()),
                        folder: Some(folder),
                    }),
                )));
            }

            ChangesViewCommand::AutoGrow => {
                for folder in crate::higent::session_folders(store, &self.workspace) {
                    let Some(entry) = History::folder(store, &self.workspace, &folder) else {
                        continue;
                    };
                    let Some(more) = entry.more else { continue };
                    if self.grown.get(&folder).map(String::as_str) == Some(more.as_str()) {
                        continue;
                    }
                    self.grown.insert_mut(folder.clone(), more);
                    self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                        self.window,
                        Arc::new(crate::hihistory::GrowHistory {
                            home: self.workspace.clone(),
                            folder,
                        }),
                    )));

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
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let mut section = container(arena, size);

            // The commit composer lives in the diff canvas's first
            // row and REFRESH rides each repository's root row — the
            // dock is just the tree.
            let band = PANEL_PAD;
            let rows = imba::Layout::layout(
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
                    >= list.list().total_height() - 2.0 * crate::ui::space::XL
            };
            let pageable = self.sets == ViewSets::History
                && crate::higent::session_folders(store, &self.workspace)
                    .iter()
                    .any(|folder| {
                        History::folder(store, &self.workspace, folder).is_some_and(|entry| {
                            entry.more.as_deref().is_some_and(|more| {
                                self.grown.get(folder).map(String::as_str) != Some(more)
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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChangesViewId(u64);

impl ChangesViewId {
    fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// The view registry rides the `ChangeSets` collection — the
/// records themselves, the many-to-many set↔view join, and the
/// stale marks the batch-tail lane consumes.
impl ChangeSets {
    pub fn mint_view(
        store: &mut Store,
        home: &crate::SessionId,
        view: ChangesView,
    ) -> ChangesViewId {
        let id = ChangesViewId::mint();
        Self::update(store, home, |views| {
            views.views.insert_mut(id, view);
        });
        Self::register(store, home, id);
        id
    }

    pub fn view_ref<'a>(
        store: &'a Store,
        home: &crate::SessionId,
        id: ChangesViewId,
    ) -> Option<&'a ChangesView> {
        Self::of(store, home)?.views.get(&id)
    }

    fn take_view(
        store: &mut Store,
        home: &crate::SessionId,
        id: ChangesViewId,
    ) -> Option<ChangesView> {
        let view = Self::view_ref(store, home, id)?.clone();
        Self::update(store, home, |views| {
            views.views.remove_mut(&id);
        });
        Some(view)
    }

    fn put_view(store: &mut Store, home: &crate::SessionId, id: ChangesViewId, view: ChangesView) {
        Self::update(store, home, |views| {
            views.views.insert_mut(id, view);
        });
    }

    fn remove_view(store: &mut Store, home: &crate::SessionId, id: ChangesViewId) {
        Self::update(store, home, |views| {
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

    fn view_ids(store: &Store, home: &crate::SessionId) -> Vec<ChangesViewId> {
        Self::of(store, home)
            .map(|views| views.views.keys().copied().collect())
            .unwrap_or_default()
    }

    /// Rebuild the join rows for one view from what it now displays.
    fn register(store: &mut Store, home: &crate::SessionId, id: ChangesViewId) {
        let displayed = match Self::view_ref(store, home, id) {
            Some(view) => view.displayed_sets(store),
            None => return,
        };
        Self::update(store, home, |views| {
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
    pub(crate) fn nudge_set(
        store: &mut Store,
        home: &crate::SessionId,
        set: crate::hichanges::ChangeSetId,
    ) {
        Self::update(store, home, |views| {
            let Some(viewing) = views.viewers.get(&set).cloned() else {
                return;
            };
            for id in viewing.iter() {
                views.stale.insert_mut(*id);
            }
        });
    }

    /// A folder-level structural shift (catalog adopted, status
    /// flipped, commit page landed): route through the folder's
    /// working-copy ANCHOR set — every view showing the folder joins
    /// on it. No anchor yet means no view derived rows from the
    /// folder either, but the views still show its placeholder
    /// notes, so fall back to marking everything.
    pub(crate) fn nudge_folder(
        store: &mut Store,
        home: &crate::SessionId,
        folder: &ResourceLocation,
    ) {
        match Changes::id_for_folder(store, home, folder) {
            Some(anchor) => Self::nudge_set(store, home, anchor),
            None => Self::nudge_all(store, home),
        }
    }

    pub(crate) fn nudge_all(store: &mut Store, home: &crate::SessionId) {
        let ids = Self::view_ids(store, home);
        Self::update(store, home, |views| {
            for id in ids {
                views.stale.insert_mut(id);
            }
        });
    }
}

/// The PUSH lane for both docks: at the batch tail, refresh every
/// stale view of the gathered session — the same batch its feed
/// landed in, no paint probe. `ChangeSets` is session state
/// (gather/park), so every view in the record belongs to the
/// gathered session already; the workspace guard just asserts that
/// invariant.
pub(crate) fn sync_changes_views(store: &mut Store, ui: &UiCtx) {
    let Some(scope) = crate::Gathered::scope(store).cloned() else {
        return;
    };
    let stale: Vec<ChangesViewId> = Changes::of(store, &scope)
        .map(|views| {
            views
                .stale
                .iter()
                .copied()
                .filter(|id| {
                    views
                        .views
                        .get(id)
                        .is_some_and(|view| view.workspace == scope)
                })
                .collect()
        })
        .unwrap_or_default();
    for id in stale {
        let Some(mut view) = Changes::take_view(store, &scope, id) else {
            continue;
        };
        view.refresh(store, ui);
        Changes::put_view(store, &scope, id, view);
        Changes::register(store, &scope, id);
        Changes::update(store, &scope, |views| {
            views.stale.remove_mut(&id);
        });
    }
}

/// The dock's reference view over a store-held record (no store).
/// Its death removes the record: tree rows are pure derivation,
/// nothing is lost on close.
pub struct ChangesPane {
    /// The session whose registry holds this dock's view record.
    home: crate::SessionId,
    view: ChangesViewId,
    request: Option<ModalRequest>,
}

impl ChangesPane {
    pub fn new(home: crate::SessionId, view: ChangesViewId) -> Self {
        Self {
            home,
            view,
            request: None,
        }
    }

    pub fn view(&self) -> ChangesViewId {
        self.view
    }

    /// The session whose registry holds this dock's view.
    pub fn home(&self) -> &crate::SessionId {
        &self.home
    }
}

impl Clone for ChangesPane {
    fn clone(&self) -> Self {
        Self {
            home: self.home.clone(),
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
        match Changes::view_ref(store, &self.home, self.view) {
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
        let Some(mut view) = Changes::take_view(store, &self.home, self.view) else {
            return;
        };
        view.perform(store, ui, command, fx);
        // Requests are the PANE's ask (`take_request` has no store):
        // pull what the record minted into the reference view.
        if let Some(request) = view.request.take() {
            self.request = Some(request);
        }
        Changes::put_view(store, &self.home, self.view, view);
    }

    fn destroy(&mut self, store: &mut Store, _fx: &mut imba::effect::Effects<'_, Self::Command>) {
        Changes::remove_view(store, &self.home, self.view);
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a Arena, constraints: imba::constraints::Constraints| {
                let widget: imba::ThunkBox<'a, ChangesViewCommand> =
                    match Changes::view_ref(store, &self.home, self.view) {
                        Some(view) => imba::ThunkBox::new(
                            arena,
                            imba::Layout::layout(
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

impl crate::ModalView for ChangesPane {
    fn clone_modal(&self) -> Box<dyn crate::ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
