// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::{
    ModalRequest, ModalView, ResourceLocation, SpeedSearchCommand, SpeedSearchView, TreeRow,
};
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    event::{Event, EventResult, Key as InputKey},
    leaf::leaf,
    list::{ListSlice, ListView},
    scroll::ScrollView,
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View, Widget,
};
use skia_safe::{Paint, PathBuilder, Size};

mod switcher;
pub use switcher::{SessionSwitcherView, SwitchSession, ToggleSessionSwitcher};

pub(crate) const PANEL_WIDTH: f32 = crate::DRAWER_WIDTH;

pub(crate) const PANEL_PAD: f32 = 6.0;

enum Activation {
    Done,

    List(ResourceLocation),

    Collapsed(ResourceLocation),

    Open(ResourceLocation),
}

type TreeList = ScrollView<ListView<TreeRow, ResourceLocation>>;

#[derive(Clone)]
struct LocationSearcher;

impl crate::Searcher for LocationSearcher {
    type View = TreeList;
    type Key = ResourceLocation;

    fn capture(&self, view: &Self::View) -> crate::ItemSource<ResourceLocation> {
        let keys = view.content().structure_keys();
        Box::new(move || {
            keys.ordered_keys()
                .into_iter()
                .map(|location| (location.name().to_owned(), location))
                .collect()
        })
    }

    fn generation(&self, view: &Self::View) -> u64 {
        view.content().generation()
    }
}

#[derive(Clone)]
struct LocationTree {
    list: SpeedSearchView<TreeList, LocationSearcher>,

    pending: rpds::HashTrieSetSync<ResourceLocation>,

    watches: rpds::HashTrieMapSync<ResourceLocation, crate::Subscription>,

    by_subscription: rpds::HashTrieMapSync<crate::Subscription, ResourceLocation>,
}

impl LocationTree {
    fn new(store: &Store, ui: &imba::UiCtx) -> Self {
        Self {
            list: SpeedSearchView::new(
                ScrollView::new(ListView::empty().with_selection(crate::selection_style(store))),
                LocationSearcher,
                store,
                ui,
                crate::env::Fonts::of(store),
            ),
            pending: rpds::HashTrieSetSync::new_sync(),
            watches: rpds::HashTrieMapSync::new_sync(),
            by_subscription: rpds::HashTrieMapSync::new_sync(),
        }
    }

    fn row(&self, location: &ResourceLocation, depth: u16, expanded: bool) -> TreeRow {
        let directory = location.kind().is_directory();
        let name = location.name().to_owned();
        let label = crate::TreeLabel::new(name, !directory, false).tinted(match directory {
            true => crate::TreeTint::Directory,
            false => crate::TreeTint::File,
        });
        match directory {
            true => crate::TreeItemView::branch(label, depth, expanded).toggling_on_body(),
            false => crate::TreeItemView::leaf(label, depth),
        }
    }

    fn len(&self) -> usize {
        self.list.inner().content().len()
    }

    fn is_visible(&self, location: &ResourceLocation) -> bool {
        self.list.inner().content().row_range(location).is_some()
    }

    fn is_listed(&self, location: &ResourceLocation) -> bool {
        self.list
            .inner()
            .content()
            .row_range(location)
            .is_some_and(|range| range.len() > 1)
    }

    fn select_and_reveal(&mut self, location: &ResourceLocation) {
        self.list
            .inner_mut()
            .content_mut()
            .select_only(location.clone());
    }

    fn descend_toward(&mut self, target: &ResourceLocation) -> Descend {
        if self.is_visible(target) {
            return Descend::Reached(target.clone());
        }

        for len in (1..=target.path().len().saturating_sub(1)).rev() {
            let ancestor = ResourceLocation::new(
                crate::ResourceType::directory(),
                target.authority().clone(),
                target.path()[..len].to_vec(),
            );
            if !self.is_visible(&ancestor) {
                continue;
            }
            if self.pending.contains(&ancestor) {
                return Descend::Wait;
            }
            if self.is_listed(&ancestor) {
                return Descend::Dead;
            }

            self.pending.insert_mut(ancestor.clone());
            return Descend::Fetch(ancestor);
        }

        Descend::Dead
    }

    fn ensure_roots(&mut self, folders: &[ResourceLocation], store: &Store, ui: &UiCtx) {
        let mut slice: ListSlice<TreeRow, ResourceLocation> = ListSlice::new();
        for folder in folders {
            if self.is_visible(folder) {
                continue;
            }
            slice.push_keyed(folder.clone(), self.row(folder, 0, false), store, ui);
        }
        if slice.is_empty() {
            return;
        }
        let len = self.len();
        self.list
            .inner_mut()
            .content_mut()
            .splice_slice(len..len, slice);
    }

    fn activate_key(
        &mut self,
        location: &ResourceLocation,
        store: &Store,
        ui: &UiCtx,
    ) -> Activation {
        self.list
            .inner_mut()
            .content_mut()
            .select_only(location.clone());
        if !location.kind().is_directory() {
            return Activation::Open(location.clone());
        }
        let Some(range) = self.list.inner().content().row_range(location) else {
            return Activation::Done;
        };
        if range.len() > 1 {
            let depth = self.list.inner().content().depth_at(range.start) as u16;
            let mut slice: ListSlice<TreeRow, ResourceLocation> = ListSlice::new();
            slice.push_keyed(
                location.clone(),
                self.row(location, depth, false),
                store,
                ui,
            );
            self.list
                .inner_mut()
                .content_mut()
                .splice_slice_animated(range, slice);
            return Activation::Collapsed(location.clone());
        }

        if self.pending.contains(location) {
            return Activation::Done;
        }
        self.pending.insert_mut(location.clone());
        Activation::List(location.clone())
    }

    fn splice_listing(
        &mut self,
        parent: ResourceLocation,
        entries: Option<Vec<ResourceLocation>>,
        store: &Store,
        ui: &UiCtx,
    ) -> Vec<ResourceLocation> {
        self.pending.remove_mut(&parent);
        let Some(entries) = entries else {
            eprintln!("[hifiles] listing failed: {parent:?}");
            return Vec::new();
        };

        let Some(range) = self.list.inner().content().row_range(&parent) else {
            return Vec::new();
        };
        let depth = self.list.inner().content().depth_at(range.start) as u16;

        let mut blocks: Vec<(ResourceLocation, std::ops::Range<usize>)> = Vec::new();
        {
            let content = self.list.inner().content();
            let mut index = range.start + 1;
            while index < range.end {
                let Some(key) = content.key_at(index).cloned() else {
                    break;
                };
                let end = content
                    .row_range(&key)
                    .map_or(index + 1, |span| span.end.max(index + 1));
                blocks.push((key, index..end));
                index = end;
            }
        }

        if range.len() > 1
            && blocks.len() == entries.len()
            && blocks
                .iter()
                .zip(&entries)
                .all(|((key, _), entry)| key == entry)
        {
            return Vec::new();
        }

        let old_rows: Vec<TreeRow> = self
            .list
            .inner()
            .content()
            .rows_from(range.start)
            .take(range.len())
            .collect();
        let old_heights: Vec<f32> = range
            .clone()
            .map(|index| self.list.inner().content().height_at(index).unwrap_or(1.0))
            .collect();
        let old_keys: Vec<Option<ResourceLocation>> = range
            .clone()
            .map(|index| self.list.inner().content().key_at(index).cloned())
            .collect();
        let expanded = self.list.inner().content().spans_within(range.clone());

        let mut slice: ListSlice<TreeRow, ResourceLocation> = ListSlice::new();
        slice.push_keyed(parent.clone(), self.row(&parent, depth, true), store, ui);

        let by_key: std::collections::HashMap<&ResourceLocation, &std::ops::Range<usize>> =
            blocks.iter().map(|(key, span)| (key, span)).collect();

        let mut carried: Vec<(std::ops::Range<usize>, usize)> = Vec::new();
        for child in &entries {
            match by_key.get(child).map(|span| (child, *span)) {
                Some((_, span)) => {
                    let landing = slice.len();
                    for index in span.clone() {
                        let at = index - range.start;
                        let Some(key) = old_keys[at].clone() else {
                            continue;
                        };
                        // Carried rows BRING their measured heights;
                        // only fresh rows measure themselves.
                        slice.push_keyed_sized(key, old_rows[at].clone(), old_heights[at]);
                    }
                    carried.push((span.clone(), landing));
                }
                None => {
                    slice.push_keyed(child.clone(), self.row(child, depth + 1, false), store, ui);
                }
            }
        }
        slice.cover(parent.clone(), 0..slice.len());
        for (key, span) in expanded {
            if key == parent {
                continue;
            }
            let Some((block, landing)) = carried
                .iter()
                .find(|(block, _)| span.start >= block.start && span.end <= block.end)
            else {
                continue;
            };
            let start = span.start - block.start + landing;
            let end = span.end - block.start + landing;
            slice.cover(key, start..end);
        }
        let survivors: std::collections::HashSet<&ResourceLocation> = entries.iter().collect();
        let removed: Vec<ResourceLocation> = blocks
            .iter()
            .filter(|(key, _)| !survivors.contains(key))
            .map(|(key, _)| key.clone())
            .collect();
        self.list
            .inner_mut()
            .content_mut()
            .splice_slice_animated(range, slice);
        removed
    }
}

enum Descend {
    Reached(ResourceLocation),
    Fetch(ResourceLocation),
    Wait,
    Dead,
}

#[derive(Clone, Default)]
pub struct SessionTree(Option<LocationTree>);

impl SessionTree {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    fn find_or_create(store: &mut Store, ui: &imba::UiCtx) -> LocationTree {
        store
            .get::<SessionTree>()
            .and_then(|tree| tree.0.clone())
            .unwrap_or_else(|| LocationTree::new(store, ui))
    }

    fn persist(store: &mut Store, tree: &LocationTree) {
        store.put(SessionTree(Some(tree.clone())));
    }
}

pub enum TreeCommand {
    Rows(SpeedSearchCommand<crate::TreeListCommand>),

    Retheme,

    Select(isize),

    Fold(bool),

    Pick,

    Listed {
        parent: ResourceLocation,
        entries: Option<Vec<ResourceLocation>>,
    },

    Watched {
        parent: ResourceLocation,
        subscription: Option<crate::Subscription>,
    },

    Changed(Vec<crate::Subscription>),

    Dismiss,

    Follow {
        location: ResourceLocation,
        generation: u64,
    },
}

pub struct SessionTreeView {
    tree: LocationTree,

    workspace: crate::SessionId,

    pending_reveal: Option<ResourceLocation>,

    window: Option<crate::WindowId>,

    followed: u64,
    request: Option<ModalRequest>,
}

impl Clone for SessionTreeView {
    fn clone(&self) -> Self {
        Self {
            tree: self.tree.clone(),
            workspace: self.workspace.clone(),
            pending_reveal: self.pending_reveal.clone(),
            window: self.window,
            followed: self.followed,

            request: None,
        }
    }
}

impl SessionTreeView {
    pub fn open(
        store: &mut Store,
        ui: &UiCtx,
        workspace: crate::SessionId,
        reveal: Option<ResourceLocation>,
        fx: &mut imba::effect::Effects<'_, TreeCommand>,
    ) -> Self {
        let mut tree = SessionTree::find_or_create(store, ui);
        tree.ensure_roots(
            &crate::higent::session_folders(store, &workspace),
            store,
            ui,
        );
        let mut panel = Self {
            tree,
            workspace,
            pending_reveal: reveal,
            window: None,
            followed: 0,
            request: None,
        };
        panel.drive_reveal(fx);
        panel.persist(store);
        panel
    }

    pub fn following(mut self, window: crate::WindowId) -> Self {
        self.window = Some(window);
        self
    }

    fn drive_reveal(&mut self, fx: &mut imba::effect::Effects<'_, TreeCommand>) {
        let Some(target) = self.pending_reveal.clone() else {
            return;
        };
        match self.tree.descend_toward(&target) {
            Descend::Reached(location) => {
                self.pending_reveal = None;
                self.tree.select_and_reveal(&location);
            }
            Descend::Fetch(parent) => {
                let _ = fx.push(self.list_effect(parent));
            }
            Descend::Wait => {}
            Descend::Dead => self.pending_reveal = None,
        }
    }

    pub fn row_count(&self) -> usize {
        self.tree.len()
    }

    pub fn selected_name(&self) -> Option<String> {
        Some(self.tree.list.inner().content().cursor()?.name().to_owned())
    }

    pub fn reveal_pending(&self) -> bool {
        self.pending_reveal.is_some()
    }

    fn persist(&self, store: &mut Store) {
        SessionTree::persist(store, &self.tree);
    }

    fn list_effect(&self, parent: ResourceLocation) -> imba::effect::AnyEffect<TreeCommand> {
        let landing = parent.clone();
        imba::effect::AnyEffect::new(crate::ListDirectoryEffect { location: parent }).map(
            move |entries| TreeCommand::Listed {
                parent: landing,
                entries,
            },
        )
    }

    pub fn activate(
        &mut self,
        index: usize,
        store: &Store,
        ui: &UiCtx,
        fx: &mut imba::effect::Effects<'_, TreeCommand>,
    ) {
        let Some(location) = self.tree.list.inner().content().key_at(index).cloned() else {
            return;
        };
        self.activate_key(&location, store, ui, fx);
    }

    fn activate_key(
        &mut self,
        location: &ResourceLocation,
        store: &Store,
        ui: &UiCtx,
        fx: &mut imba::effect::Effects<'_, TreeCommand>,
    ) {
        self.pending_reveal = None;
        match self.tree.activate_key(location, store, ui) {
            Activation::Done => {}
            Activation::List(parent) => {
                let _ = fx.push(self.list_effect(parent));
            }
            Activation::Collapsed(folder) => self.drop_watches_under(&folder, fx),
            Activation::Open(location) => {
                self.request = Some(ModalRequest::OpenLocations(vec![location]));
            }
        }
    }

    fn drop_watches_under(
        &mut self,
        folder: &ResourceLocation,
        fx: &mut imba::effect::Effects<'_, TreeCommand>,
    ) {
        let stale: Vec<(ResourceLocation, crate::Subscription)> = self
            .tree
            .watches
            .iter()
            .filter(|(watched, _)| watched.path().starts_with(folder.path()))
            .map(|(watched, live)| (watched.clone(), *live))
            .collect();
        for (watched, subscription) in stale {
            self.tree.watches.remove_mut(&watched);
            self.tree.by_subscription.remove_mut(&subscription);
            fx.notify(crate::UnsubscribeEffect { subscription });
        }
    }
}

impl SessionTreeView {
    pub fn search_query(&self) -> String {
        self.tree.list.query()
    }
}

impl View for SessionTreeView {
    type Command = TreeCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, TreeCommand> {
        use imba::focus::FocusData;
        let searching = self.tree.list.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => EventResult::Command(TreeCommand::Dismiss),
                InputKey::Up if !searching => EventResult::Command(TreeCommand::Select(-1)),
                InputKey::Down if !searching => EventResult::Command(TreeCommand::Select(1)),
                InputKey::Left if !searching => EventResult::Command(TreeCommand::Fold(false)),
                InputKey::Right if !searching => EventResult::Command(TreeCommand::Fold(true)),
                InputKey::Enter if searching => EventResult::Commands(vec![
                    TreeCommand::Pick,
                    TreeCommand::Rows(SpeedSearchCommand::Clear),
                ]),
                InputKey::Enter => EventResult::Command(TreeCommand::Pick),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(self.tree.list.focus_data(store, ui).map(TreeCommand::Rows))
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        // Teardown-only: `View::destroy` carries no UiCtx.
        let ui = &imba::UiCtx::dont_use_too_slow();
        fx.scope(TreeCommand::Rows, |fx| self.tree.list.clear(store, ui, fx));
        self.persist(store);
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            TreeCommand::Rows(command) => {
                if let SpeedSearchCommand::Inner(inner) = &command {
                    if let Some((index, _)) = crate::tree_interaction(inner) {
                        self.activate(index, store, ui, fx);
                        return self.persist(store);
                    }
                }
                fx.scope(TreeCommand::Rows, |fx| {
                    self.tree.list.perform(store, ui, command, fx)
                });
            }
            TreeCommand::Retheme => {
                self.tree
                    .list
                    .inner_mut()
                    .content_mut()
                    .set_selection_style(crate::selection_style(store));
            }
            TreeCommand::Select(delta) => {
                self.pending_reveal = None;
                self.tree.list.inner_mut().content_mut().cursor_step(delta);
            }
            TreeCommand::Fold(expand) => {
                self.pending_reveal = None;
                let Some(location) = self.tree.list.inner().content().cursor().cloned() else {
                    return self.persist(store);
                };
                let directory = location.kind().is_directory();
                let listed = self.tree.is_listed(&location);
                match (expand, directory, listed) {
                    (true, true, false) => self.activate_key(&location, store, ui, fx),

                    (false, true, true) => self.activate_key(&location, store, ui, fx),

                    (false, _, _) => {
                        if location.path().len() > 1 {
                            let parent = ResourceLocation::new(
                                crate::ResourceType::directory(),
                                location.authority().clone(),
                                location.path()[..location.path().len() - 1].to_vec(),
                            );
                            if self.tree.is_visible(&parent) {
                                self.tree.select_and_reveal(&parent);
                            }
                        }
                    }
                    _ => {}
                }
            }
            TreeCommand::Pick => {
                if let Some(location) = self.tree.list.inner().content().cursor().cloned() {
                    self.activate_key(&location, store, ui, fx);
                }
            }
            TreeCommand::Listed { parent, entries } => {
                let listed = entries.is_some();
                let removed = self.tree.splice_listing(parent.clone(), entries, store, ui);

                for gone in removed
                    .iter()
                    .filter(|location| location.kind().is_directory())
                {
                    self.drop_watches_under(gone, fx);
                }

                self.drive_reveal(fx);

                if listed
                    && crate::Watching::installed(store)
                    && !self.tree.watches.contains_key(&parent)
                {
                    let landing = parent.clone();
                    let _ = fx.push(
                        imba::effect::AnyEffect::new(crate::SubscribeEffect { location: parent })
                            .map(move |subscription| TreeCommand::Watched {
                                parent: landing,
                                subscription,
                            }),
                    );
                }
            }
            TreeCommand::Watched {
                parent,
                subscription,
            } => {
                if let Some(subscription) = subscription {
                    self.tree.watches.insert_mut(parent.clone(), subscription);
                    self.tree.by_subscription.insert_mut(subscription, parent);
                }
            }
            TreeCommand::Changed(subscriptions) => {
                for subscription in subscriptions {
                    let Some(parent) = self.tree.by_subscription.get(&subscription).cloned() else {
                        continue;
                    };

                    if !self.tree.pending.contains(&parent) {
                        self.tree.pending.insert_mut(parent.clone());
                        let _ = fx.push(self.list_effect(parent));
                    }
                }
            }
            TreeCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
            TreeCommand::Follow {
                location,
                generation,
            } => {
                self.followed = generation;
                let standing = self
                    .tree
                    .list
                    .inner()
                    .content()
                    .cursor()
                    .is_some_and(|held| held == &location);
                if !standing {
                    self.pending_reveal = Some(location);
                    self.drive_reveal(fx);
                }
            }
        };

        self.persist(store);
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let mut overlay = container(arena, size);

            let rows = imba::Layout::layout(
                self.tree.list.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(size.width, size.height - PANEL_PAD)),
            )
            .map(TreeCommand::Rows);
            overlay.place(0.0, PANEL_PAD, rows);

            let searching = self.tree.list.searching();
            let keymap =
                leaf::<TreeCommand>(size.width, size.height).event(move |_arena, event, _size| {
                    match event {
                        Event::KeyDown {
                            key: InputKey::Escape,
                            ..
                        } if !searching => EventResult::Command(TreeCommand::Dismiss),
                        Event::KeyDown {
                            key: InputKey::Up, ..
                        } if !searching => EventResult::Command(TreeCommand::Select(-1)),
                        Event::KeyDown {
                            key: InputKey::Down,
                            ..
                        } if !searching => EventResult::Command(TreeCommand::Select(1)),
                        Event::KeyDown {
                            key: InputKey::Left,
                            ..
                        } if !searching => EventResult::Command(TreeCommand::Fold(false)),
                        Event::KeyDown {
                            key: InputKey::Right,
                            ..
                        } if !searching => EventResult::Command(TreeCommand::Fold(true)),
                        Event::KeyDown {
                            key: InputKey::Enter,
                            ..
                        } if searching => EventResult::Commands(vec![
                            TreeCommand::Pick,
                            TreeCommand::Rows(SpeedSearchCommand::Clear),
                        ]),
                        Event::KeyDown {
                            key: InputKey::Enter,
                            ..
                        } => EventResult::Command(TreeCommand::Pick),

                        Event::ThemeChanged => EventResult::Command(TreeCommand::Retheme),
                        _ => EventResult::Ignored,
                    }
                });
            overlay.place(0.0, 0.0, keymap);

            let watched = self.tree.by_subscription.clone();
            let inner = overlay.event(move |_arena, event, _size| match event {
                Event::UserEvent(payload) => {
                    match payload.downcast_ref::<crate::watch::FilesChanged>() {
                        Some(changed) => {
                            let ours: Vec<crate::Subscription> = changed
                                .0
                                .iter()
                                .copied()
                                .filter(|subscription| watched.contains_key(subscription))
                                .collect();
                            match ours.is_empty() {
                                true => EventResult::Ignored,
                                false => EventResult::Command(TreeCommand::Changed(ours)),
                            }
                        }
                        _ => EventResult::Ignored,
                    }
                }
                _ => EventResult::Ignored,
            });

            let follow = self.window.and_then(|window| {
                let entity = crate::Windows::window_ref(store, window)?;
                match entity.focus_generation() != self.followed {
                    true => entity
                        .focused_location()
                        .cloned()
                        .map(|location| (location, entity.focus_generation())),
                    false => None,
                }
            });
            inner.wrap(move |inner| FollowShell { inner, follow })
        })
    }
}

struct FollowShell<Inner> {
    inner: Inner,
    follow: Option<(ResourceLocation, u64)>,
}

impl<'a, Inner: Widget<'a, TreeCommand>> Widget<'a, TreeCommand> for FollowShell<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, TreeCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: skia_safe::Rect,
    ) -> EventResult<TreeCommand> {
        let result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) {
            if let Some((location, generation)) = &self.follow {
                return result.merge(EventResult::Command(TreeCommand::Follow {
                    location: location.clone(),
                    generation: *generation,
                }));
            }
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, TreeCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

impl ModalView for SessionTreeView {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct ToggleSessionTree;

impl crate::DynamicCommand for ToggleSessionTree {
    fn id(&self) -> &'static str {
        "files.tree"
    }
    fn name(&self) -> String {
        "File Tree".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        // The focused location is a state walk over the views now —
        // nothing is laid to answer it.
        let reveal = {
            let chain_store = app.window_store(window);
            let ui = app.ui_ctx();
            crate::focus::window_focus_data(&chain_store, &ui, window)
                .and_then(|mut data| crate::focus::focused_location(&mut data))
        };
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            crate::Windows::put(store, window, entity);
            return;
        }

        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );

        let workspace = entity.current_session();
        let panel = fx.scope(crate::dock_scope(window), |fx| {
            fx.scope(
                |command: TreeCommand| Box::new(command) as imba::DynCommand,
                |fx| {
                    SessionTreeView::open(store, &app.ui_ctx(), workspace, reveal, fx)
                        .following(window)
                },
            )
        });
        let owner = self.id();
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.show_dock(store, Box::new(panel), owner, fx),
        );
        crate::Windows::put(store, window, entity);
    }
}

pub fn toolbar_button() -> crate::ToolbarButton {
    crate::ToolbarButton {
        command: "files.tree",
        order: 0.0,
        side: crate::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());
            let rows = [t + h * 0.14, t + h * 0.5, t + h * 0.86];
            let mut path = PathBuilder::new();

            path.move_to((l, rows[0]));
            path.line_to((l + w, rows[0]));

            let trunk = l + w * 0.12;
            path.move_to((trunk, rows[0] + h * 0.14));
            path.line_to((trunk, rows[2]));
            for row in &rows[1..] {
                path.move_to((trunk, *row));
                path.line_to((l + w * 0.28, *row));
                path.move_to((l + w * 0.42, *row));
                path.line_to((l + w, *row));
            }
            canvas.draw_path(&path.detach(), &paint);
        }),
    }
}

#[cfg(test)]
mod tests;
