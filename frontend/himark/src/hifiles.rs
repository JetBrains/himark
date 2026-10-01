// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::menu::{MenuCommand, MenuView, PopupMenuView};
use crate::{
    ListKeyCommand, ListKeyboardController, ModalRequest, ModalView, ResourceLocation, TreeRow,
};
use imba::list::{ActivateTrigger, ListOps};
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
    list: ListKeyboardController<TreeList, LocationSearcher>,

    pending: rpds::HashTrieSetSync<ResourceLocation>,

    watches: rpds::HashTrieMapSync<ResourceLocation, crate::Subscription>,

    by_subscription: rpds::HashTrieMapSync<crate::Subscription, ResourceLocation>,
}

impl LocationTree {
    fn new(store: &Store, ui: &imba::UiCtx) -> Self {
        Self {
            list: ListKeyboardController::searchable(
                ScrollView::new(ListView::empty().with_selection(crate::selection_style(store))),
                LocationSearcher,
                store,
                ui,
                crate::env::Fonts::of(store),
            )
            .with_folds(),
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

    /// Addressed by its own id, threaded from the owning session's
    /// family row — never by whatever session the batch was gathered
    /// for.
    fn find_or_create(
        store: &mut Store,
        ui: &imba::UiCtx,
        trees: imba::store::Id<Self>,
    ) -> LocationTree {
        store
            .entity(trees)
            .and_then(|trees| trees.0.clone())
            .unwrap_or_else(|| LocationTree::new(store, ui))
    }

    fn persist(store: &mut Store, trees: imba::store::Id<Self>, tree: &LocationTree) {
        store.update_entity(trees, |trees| {
            *trees = SessionTree(Some(tree.clone()));
        });
    }
}

pub enum TreeCommand {
    Rows(ListKeyCommand<crate::TreeListCommand>),

    Retheme,

    Listed {
        parent: ResourceLocation,
        entries: Option<Vec<ResourceLocation>>,
    },

    Watched {
        parent: ResourceLocation,
        subscription: Option<crate::Subscription>,
    },

    Changed(Vec<crate::Subscription>),

    /// The roots drifted from the session's folders — folders join
    /// and leave the session while the dock stands open.
    SyncRoots {
        missing: Vec<ResourceLocation>,
        stale: Vec<ResourceLocation>,
    },

    /// The row context menu's traffic while one stands open.
    Menu(MenuCommand),

    /// The inline row editor's traffic while a rename/create rides.
    Edit(::editor::EditorCommand),

    CommitEdit,

    CancelEdit,

    /// A file operation landed — relist the owning folder and, on
    /// success, reveal what the operation produced.
    Mutated {
        parent: ResourceLocation,
        select: Option<ResourceLocation>,
        ok: bool,
    },

    /// The remove-from-session dispatch answered; the row itself
    /// leaves via the stale-roots gate when the echo lands.
    Dispatched(Result<(), String>),

    Dismiss,

    Follow {
        location: ResourceLocation,
        generation: u64,
    },
}

/// The context menu standing over a row, with the row it serves.
#[derive(Clone)]
struct TreeMenu {
    target: ResourceLocation,
    view: PopupMenuView,
}

#[derive(Clone)]
enum EditTarget {
    Rename(ResourceLocation),

    /// Creating under `parent`: a transient placeholder row hosts
    /// the editor until commit or cancel removes it.
    Create {
        parent: ResourceLocation,
        placeholder: ResourceLocation,
    },
}

#[derive(Clone)]
struct RowEdit {
    target: EditTarget,
    input: ::editor::EditorView,
}

impl RowEdit {
    fn row_key(&self) -> &ResourceLocation {
        match &self.target {
            EditTarget::Rename(location) => location,
            EditTarget::Create { placeholder, .. } => placeholder,
        }
    }
}

pub struct SessionTreeView {
    tree: LocationTree,

    /// The persisted tree's id, threaded at open; `workspace` stays
    /// for the PROTOCOL roles (seat, uris, folders) — the session
    /// context this panel legitimately is.
    trees: imba::store::Id<SessionTree>,

    workspace: crate::SessionId,

    pending_reveal: Option<ResourceLocation>,

    window: Option<crate::WindowId>,

    followed: u64,
    request: Option<ModalRequest>,

    menu: Option<TreeMenu>,
    edit: Option<RowEdit>,
}

impl Clone for SessionTreeView {
    fn clone(&self) -> Self {
        Self {
            tree: self.tree.clone(),
            trees: self.trees,
            workspace: self.workspace.clone(),
            pending_reveal: self.pending_reveal.clone(),
            window: self.window,
            followed: self.followed,

            request: None,
            menu: self.menu.clone(),
            edit: self.edit.clone(),
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
        let trees = crate::higent::Hosts::ensure_family(store, &workspace).trees();
        let mut tree = SessionTree::find_or_create(store, ui, trees);
        tree.ensure_roots(
            &crate::higent::session_folders(store, &workspace),
            store,
            ui,
        );
        let mut panel = Self {
            tree,
            trees,
            workspace,
            pending_reveal: reveal,
            window: None,
            followed: 0,
            request: None,
            menu: None,
            edit: None,
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
        SessionTree::persist(store, self.trees, &self.tree);
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

    fn open_menu(&mut self, index: usize, store: &Store, ui: &UiCtx) {
        let Some(target) = self.tree.list.inner().content().key_at(index).cloned() else {
            return;
        };
        let root = self.tree.list.inner().content().depth_at(index) == 0;
        self.tree
            .list
            .inner_mut()
            .content_mut()
            .select_only(target.clone());
        self.pending_reveal = None;
        let items = menu_items(&target, root);
        self.menu = Some(TreeMenu {
            target,
            view: PopupMenuView::new(store, ui, items),
        });
    }

    fn menu_pick(
        &mut self,
        id: &str,
        target: ResourceLocation,
        store: &Store,
        ui: &UiCtx,
        fx: &mut imba::effect::Effects<'_, TreeCommand>,
    ) {
        match id {
            NEW_FILE => self.start_create(target, store, ui),
            RENAME => self.start_rename(&target, store, ui),
            DELETE => {
                let Some(parent) = parent_of(&target) else {
                    return;
                };
                let recursive = target.kind().is_directory();
                let _ = fx.push(
                    imba::effect::AnyEffect::new(crate::DeleteResourceEffect {
                        location: target,
                        recursive,
                    })
                    .map(move |ok| TreeCommand::Mutated {
                        parent,
                        select: None,
                        ok,
                    }),
                );
            }
            REMOVE_ROOT => {
                let Some(seat) = crate::higent::Servers::seat(store, self.workspace.host) else {
                    return;
                };
                let Some(uris) = crate::higent::Hosts::uris(store, self.workspace.host) else {
                    return;
                };
                use crate::higent::ahp_types::actions as wire;
                let directory = uris.uri_of(&target).as_str().to_owned();
                let _ = fx.push(
                    imba::effect::AnyEffect::new(crate::higent::DispatchChatActionEffect {
                        seat,
                        channel: self.workspace.session.as_channel(),
                        action: wire::StateAction::SessionWorkingDirectoryRemoved(
                            wire::SessionWorkingDirectoryRemovedAction { directory },
                        ),
                    })
                    .map(TreeCommand::Dispatched),
                );
            }
            _ => {}
        }
    }

    fn start_rename(&mut self, target: &ResourceLocation, store: &Store, ui: &UiCtx) {
        let name = target.name().to_owned();
        let mut input = seeded_input(store, ui, &name);
        // The stem is the likely edit; the extension stays put until
        // typed over.
        let stem = name.rfind('.').filter(|at| *at > 0).unwrap_or(name.len());
        input.document.set_carets(
            input.editor,
            ::editor::MultiCaret::one(::editor::Caret::selecting(0, stem as u32)),
        );
        self.edit = Some(RowEdit {
            target: EditTarget::Rename(target.clone()),
            input,
        });
        self.pending_reveal = None;
    }

    fn start_create(&mut self, parent: ResourceLocation, store: &Store, ui: &UiCtx) {
        let Some(range) = self.tree.list.inner().content().row_range(&parent) else {
            return;
        };
        let depth = self.tree.list.inner().content().depth_at(range.start) as u16;
        let placeholder = parent.child(crate::ResourceType::document(), "");
        if self.tree.is_visible(&placeholder) {
            return;
        }
        let mut slice: ListSlice<TreeRow, ResourceLocation> = ListSlice::new();
        slice.push_keyed(
            placeholder.clone(),
            self.tree.row(&placeholder, depth + 1, false),
            store,
            ui,
        );
        let at = range.start + 1;
        self.tree
            .list
            .inner_mut()
            .content_mut()
            .splice_slice(at..at, slice);
        let mut input = ::editor::EditorView::input(600.0, store, ui, crate::fonts::source());
        input.focus_text();
        self.edit = Some(RowEdit {
            target: EditTarget::Create {
                parent,
                placeholder,
            },
            input,
        });
        self.pending_reveal = None;
    }

    fn commit_edit(&mut self, fx: &mut imba::effect::Effects<'_, TreeCommand>) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        let name = edit_text(&edit.input).trim().to_owned();
        if !valid_file_name(&name) {
            // An unusable name keeps the editor for another try.
            self.edit = Some(edit);
            return;
        }
        match edit.target {
            EditTarget::Rename(location) => {
                let Some(parent) = parent_of(&location) else {
                    return;
                };
                if location.name() == name {
                    return;
                }
                let to = parent.child(location.kind().clone(), name);
                let reveal = to.clone();
                let _ = fx.push(
                    imba::effect::AnyEffect::new(crate::MoveResourceEffect { from: location, to })
                        .map(move |ok| TreeCommand::Mutated {
                            parent,
                            select: ok.then_some(reveal),
                            ok,
                        }),
                );
            }
            EditTarget::Create {
                parent,
                placeholder,
            } => {
                self.remove_row(&placeholder);
                let location = parent.child(crate::ResourceType::document(), name);
                let reveal = location.clone();
                let _ = fx.push(
                    imba::effect::AnyEffect::new(crate::CreateDocumentEffect { location }).map(
                        move |ok| TreeCommand::Mutated {
                            parent,
                            select: ok.then_some(reveal),
                            ok,
                        },
                    ),
                );
            }
        }
    }

    fn abort_edit(&mut self) {
        if let Some(edit) = self.edit.take() {
            if let EditTarget::Create { placeholder, .. } = edit.target {
                self.remove_row(&placeholder);
            }
        }
    }

    /// Removes a single childless row — the create placeholder.
    fn remove_row(&mut self, key: &ResourceLocation) {
        let Some(range) = self
            .tree
            .list
            .inner()
            .content()
            .row_range(key)
            .filter(|range| range.len() == 1)
        else {
            return;
        };
        let empty: ListSlice<TreeRow, ResourceLocation> = ListSlice::new();
        self.tree
            .list
            .inner_mut()
            .content_mut()
            .splice_slice(range, empty);
    }

    /// Splices a departed root and its whole subtree out, with the
    /// watches, pending fetches and reveal that pointed under it.
    fn remove_root(
        &mut self,
        root: &ResourceLocation,
        fx: &mut imba::effect::Effects<'_, TreeCommand>,
    ) {
        let Some(range) = self.tree.list.inner().content().row_range(root) else {
            return;
        };
        if self.tree.list.inner().content().depth_at(range.start) != 0 {
            return;
        }
        if self
            .edit
            .as_ref()
            .is_some_and(|edit| edit.row_key().path().starts_with(root.path()))
        {
            self.abort_edit();
        }
        self.drop_watches_under(root, fx);
        let Some(range) = self.tree.list.inner().content().row_range(root) else {
            return;
        };
        let empty: ListSlice<TreeRow, ResourceLocation> = ListSlice::new();
        self.tree
            .list
            .inner_mut()
            .content_mut()
            .splice_slice(range, empty);
        let under: Vec<ResourceLocation> = self
            .tree
            .pending
            .iter()
            .filter(|held| held.path().starts_with(root.path()))
            .cloned()
            .collect();
        for held in under {
            self.tree.pending.remove_mut(&held);
        }
        if self
            .pending_reveal
            .as_ref()
            .is_some_and(|held| held.path().starts_with(root.path()))
        {
            self.pending_reveal = None;
        }
    }
}

const NEW_FILE: &str = "new-file";
const RENAME: &str = "rename";
const DELETE: &str = "delete";
const REMOVE_ROOT: &str = "remove-root";

fn menu_items(target: &ResourceLocation, root: bool) -> Vec<crate::combo::ComboOption> {
    use crate::combo::ComboOption;
    let mut items = Vec::new();
    if target.kind().is_directory() {
        items.push(ComboOption::plain(NEW_FILE, "New File"));
    }
    match root {
        true => items.push(ComboOption::plain(REMOVE_ROOT, "Remove from Session")),
        false => {
            items.push(ComboOption::plain(RENAME, "Rename"));
            items.push(ComboOption::plain(DELETE, "Delete"));
        }
    }
    items
}

fn parent_of(location: &ResourceLocation) -> Option<ResourceLocation> {
    (location.path().len() > 1).then(|| {
        ResourceLocation::new(
            crate::ResourceType::directory(),
            location.authority().clone(),
            location.path()[..location.path().len() - 1].to_vec(),
        )
    })
}

fn valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.chars().any(|c| matches!(c, '/' | '\\' | '\0'))
}

fn edit_text(input: &::editor::EditorView) -> String {
    let text = input.document.text();
    let end = text.byte_count().min(u32::MAX as usize) as u32;
    text.view().substring(0..end)
}

fn seeded_input(store: &Store, ui: &UiCtx, text: &str) -> ::editor::EditorView {
    let mut markup = crate::Markup::new();
    markup.push_styled_covering(0..text.len() as u32, crate::theme::StyleId::Input);
    let document = crate::Document::new(crate::Text::from_string_exact(text), markup);
    let fonts = crate::fonts::source();
    let mut input = ::editor::EditorView::of_document(
        document,
        600.0,
        store,
        ui,
        &fonts(),
        &crate::theme::Theme::embedded(),
    );
    input.set_caret(text.len() as u32);
    input.focus_text();
    input
}

/// The visible depth-0 keys that no longer belong to the session —
/// O(roots × log n): each root's range jumps the walk past its
/// subtree.
fn stale_roots(
    content: &ListView<TreeRow, ResourceLocation>,
    folders: &[ResourceLocation],
) -> Vec<ResourceLocation> {
    let mut stale = Vec::new();
    let mut index = 0;
    while index < content.len() {
        let Some(key) = content.key_at(index) else {
            break;
        };
        let Some(range) = content.row_range(key) else {
            break;
        };
        if !folders.contains(key) {
            stale.push(key.clone());
        }
        index = range.end.max(index + 1);
    }
    stale
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
        if let Some(edit) = &self.edit {
            let own = FocusData {
                on_key: Some(Box::new(|key, _mods| match key {
                    InputKey::Enter => EventResult::Command(TreeCommand::CommitEdit),
                    InputKey::Escape => EventResult::Command(TreeCommand::CancelEdit),
                    _ => EventResult::Ignored,
                })),
                ..FocusData::default()
            };
            return own.merge_under(edit.input.focus_data(store, ui).map(TreeCommand::Edit));
        }
        if let Some(menu) = &self.menu {
            return menu.view.focus_data(store, ui).map(TreeCommand::Menu);
        }
        // The key table is the controller's; the surface keeps only
        // its own dismissal.
        let searching = self.tree.list.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => EventResult::Command(TreeCommand::Dismiss),
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
                // Any other row interaction is a focus shift off the
                // inline editor — the edit cancels first.
                if self.edit.is_some() {
                    self.abort_edit();
                }
                type Rows = ListKeyboardController<TreeList, LocationSearcher>;
                match &command {
                    // The bespoke lazy fold: Right lists an unlisted
                    // directory, Left collapses a listed one or walks
                    // to the visible parent.
                    ListKeyCommand::Fold { expand, .. } => {
                        let expand = *expand;
                        self.pending_reveal = None;
                        let Some(location) = self.tree.list.inner().content().cursor().cloned()
                        else {
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
                        return;
                    }
                    ListKeyCommand::Inner(inner) => {
                        if let Some(index) = crate::tree_context(inner) {
                            self.open_menu(index, store, ui);
                            return self.persist(store);
                        }
                        if let Some(index) = crate::tree_toggle(inner) {
                            self.activate(index, store, ui, fx);
                            return self.persist(store);
                        }
                    }
                    _ => {}
                }
                if Rows::selected_index(&command).is_some() {
                    self.pending_reveal = None;
                }
                if let Some((index, trigger)) = Rows::activated(&command) {
                    let searching = self.tree.list.searching();
                    self.activate(index, store, ui, fx);
                    self.persist(store);
                    match trigger {
                        // The deliberate pick ends the search in the
                        // same stroke.
                        ActivateTrigger::Enter if searching => {
                            return self.perform(
                                store,
                                ui,
                                TreeCommand::Rows(ListKeyCommand::Clear),
                                fx,
                            );
                        }
                        ActivateTrigger::Enter | ActivateTrigger::Click => {}
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
            TreeCommand::SyncRoots { missing, stale } => {
                for root in &stale {
                    self.remove_root(root, fx);
                }
                self.tree.ensure_roots(&missing, store, ui);
            }
            TreeCommand::Menu(command) => {
                let Some(menu) = self.menu.as_mut() else {
                    return;
                };
                if let Some(id) = menu.view.picked(&command) {
                    let target = menu.target.clone();
                    self.menu = None;
                    self.menu_pick(&id, target, store, ui, fx);
                } else if MenuView::closes(&command) {
                    self.menu = None;
                } else {
                    fx.scope(TreeCommand::Menu, |fx| {
                        menu.view.perform(store, ui, command, fx)
                    });
                }
            }
            TreeCommand::Edit(command) => {
                let Some(edit) = self.edit.as_mut() else {
                    return;
                };
                fx.scope(TreeCommand::Edit, |fx| {
                    edit.input.perform(store, ui, command, fx)
                });
            }
            TreeCommand::CommitEdit => {
                self.commit_edit(fx);
            }
            TreeCommand::CancelEdit => {
                self.abort_edit();
            }
            TreeCommand::Mutated { parent, select, ok } => {
                if !ok {
                    eprintln!("[hifiles] file operation failed under {parent:?}");
                }
                if let Some(select) = select {
                    self.pending_reveal = Some(select);
                }
                // The relist is the one truth either way — a success
                // shows the outcome, a failure resyncs the rows.
                if !self.tree.pending.contains(&parent) {
                    self.tree.pending.insert_mut(parent.clone());
                    let _ = fx.push(self.list_effect(parent));
                }
            }
            TreeCommand::Dispatched(result) => {
                if let Err(error) = result {
                    eprintln!("[hifiles] session folder removal failed: {error}");
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

            // The key table lives in the controller's own overlay;
            // the surface keeps only its dismissal and retheme.
            let searching = self.tree.list.searching();
            let editing = self.edit.is_some();
            let menu_open = self.menu.is_some();
            let keymap =
                leaf::<TreeCommand>(size.width, size.height).event(move |_arena, event, _size| {
                    if editing {
                        return match event {
                            Event::KeyDown {
                                key: InputKey::Enter,
                                ..
                            } => EventResult::Command(TreeCommand::CommitEdit),
                            Event::KeyDown {
                                key: InputKey::Escape,
                                ..
                            } => EventResult::Command(TreeCommand::CancelEdit),
                            Event::ThemeChanged => EventResult::Command(TreeCommand::Retheme),
                            _ => EventResult::Ignored,
                        };
                    }
                    match event {
                        Event::KeyDown {
                            key: InputKey::Escape,
                            ..
                        } if menu_open => {
                            EventResult::Command(TreeCommand::Menu(crate::menu::MenuCommand::Close))
                        }
                        Event::KeyDown {
                            key: InputKey::Escape,
                            ..
                        } if !searching => EventResult::Command(TreeCommand::Dismiss),

                        Event::ThemeChanged => EventResult::Command(TreeCommand::Retheme),
                        _ => EventResult::Ignored,
                    }
                });
            overlay.place(0.0, 0.0, keymap);

            if let Some(menu) = &self.menu {
                if let Some(range) = self.tree.list.inner().content().row_range(&menu.target) {
                    if let Some((top, height)) =
                        self.tree.list.inner().content().row_span(range.start)
                    {
                        let tree_theme = crate::env::Themes::of(store).ui().tree.clone();
                        let depth = self.tree.list.inner().content().depth_at(range.start) as f32;
                        let x = (depth * tree_theme.indent + tree_theme.text_x).min(size.width);
                        let y = (PANEL_PAD + top + height - self.tree.list.inner().scroll_y())
                            .clamp(0.0, size.height);
                        overlay.place(
                            x,
                            y,
                            menu.view
                                .overlay_at(arena, store, ui)
                                .map(TreeCommand::Menu),
                        );
                    }
                }
            }

            if let Some(edit) = &self.edit {
                if let Some(range) = self.tree.list.inner().content().row_range(edit.row_key()) {
                    if let Some((top, height)) =
                        self.tree.list.inner().content().row_span(range.start)
                    {
                        let theme = crate::env::Themes::of(store);
                        let tree_theme = theme.ui().tree.clone();
                        let search = theme.ui().search.clone();
                        let depth = self.tree.list.inner().content().depth_at(range.start) as f32;
                        let x = depth * tree_theme.indent + tree_theme.text_x;
                        let y = PANEL_PAD + top - self.tree.list.inner().scroll_y();
                        let well_width = (size.width - x - PANEL_PAD).max(1.0);
                        let input_fill = search.input_fill;
                        let well = leaf::<TreeCommand>(well_width, height).paint_instead(
                            move |_arena, canvas, rect| {
                                let mut paint = Paint::default();
                                paint.set_anti_alias(true);
                                paint.set_color(input_fill.0);
                                canvas.draw_round_rect(rect, 4.0, 4.0, &paint);
                            },
                        );
                        overlay.place(x, y, well);
                        let inner_height = (height - search.input_pad_y * 2.0).max(1.0);
                        overlay.place(
                            x + search.input_pad_x,
                            y + search.input_pad_y,
                            imba::Layout::layout(
                                edit.input.display(arena, store, ui),
                                arena,
                                Constraints {
                                    min: Size::new(0.0, inner_height),
                                    max: Size::new(
                                        (well_width - search.input_pad_x * 2.0).max(1.0),
                                        inner_height,
                                    ),
                                },
                            )
                            .map(TreeCommand::Edit)
                            .focus_scope(true),
                        );
                    }
                }
            }

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
            // Folders join AND leave the session while the dock
            // stands open — the paint gate folds the drift in as
            // soon as the channel mirror carries it, and closes
            // itself once the roots match.
            let folders = crate::higent::session_folders(store, &self.workspace);
            let missing_roots: Vec<ResourceLocation> = folders
                .iter()
                .filter(|folder| !self.tree.is_visible(folder))
                .cloned()
                .collect();
            let stale_roots = stale_roots(self.tree.list.inner().content(), &folders);
            let editing = self.edit.is_some();
            inner.wrap(move |inner| FollowShell {
                inner,
                follow,
                missing_roots,
                stale_roots,
                editing,
            })
        })
    }
}

struct FollowShell<Inner> {
    inner: Inner,
    follow: Option<(ResourceLocation, u64)>,
    missing_roots: Vec<ResourceLocation>,
    stale_roots: Vec<ResourceLocation>,

    /// An inline edit rides — an unfocused paint is its focus loss.
    editing: bool,
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
        let mut result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) {
            if let Some((location, generation)) = &self.follow {
                result = result.merge(EventResult::Command(TreeCommand::Follow {
                    location: location.clone(),
                    generation: *generation,
                }));
            }
            if !self.missing_roots.is_empty() || !self.stale_roots.is_empty() {
                result = result.merge(EventResult::Command(TreeCommand::SyncRoots {
                    missing: self.missing_roots.clone(),
                    stale: self.stale_roots.clone(),
                }));
            }
            if let Event::Paint { focused: false, .. } = event {
                if self.editing {
                    result = result.merge(EventResult::Command(TreeCommand::CancelEdit));
                }
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
