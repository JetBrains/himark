// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::hash::Hash;
use std::sync::Arc;

use imba::{
    arena::Arena,
    list::{ActivateTrigger, Edge, ListOps, ListSlice, ListView},
    scroll::ScrollView,
    store::Store,
    ui::UiCtx,
    View,
};

use crate::tree_item::{TreeItemView, TreeLabel, TreeListCommand, TreeTint};

pub struct ForestNode<K> {
    pub key: K,
    pub label: String,

    pub pick: bool,

    pub dim: bool,

    pub trail: Vec<(String, skia_safe::Color)>,
    pub tint: TreeTint,

    /// A right-aligned action chip on the row (`tree_action` decodes
    /// its press).
    pub action: Option<String>,
    pub children: Vec<ForestNode<K>>,
}

pub type TreeRow = TreeItemView<TreeLabel>;

/// How a surface dresses the rows a forest emits: the plain row, or
/// the row with a tooltip or an overlay of its own (keyed by the
/// node, so a row knows what it stands for).
pub type Wrap<K, R> = Arc<dyn Fn(TreeRow, &K) -> R + Send + Sync>;

#[derive(Clone)]
struct Entry<K> {
    label: String,
    pick: bool,
    dim: bool,
    trail: Vec<(String, skia_safe::Color)>,
    tint: TreeTint,
    action: Option<String>,
    depth: u16,
    children: Vec<K>,
}

#[derive(Clone)]
pub struct Forest<K: Clone + Eq + Hash> {
    entries: rpds::HashTrieMapSync<K, Entry<K>>,
    roots: rpds::VectorSync<K>,
    parents: rpds::HashTrieMapSync<K, K>,
    collapsed: rpds::HashTrieSetSync<K>,
}

impl<K: Clone + Eq + Hash + Send + Sync> Forest<K> {
    fn empty() -> Self {
        Self {
            entries: rpds::HashTrieMapSync::new_sync(),
            roots: rpds::VectorSync::new_sync(),
            parents: rpds::HashTrieMapSync::new_sync(),
            collapsed: rpds::HashTrieSetSync::new_sync(),
        }
    }

    pub fn new(_store: &imba::store::Store) -> Self {
        Self::empty()
    }

    pub fn set(&mut self, forest: &[ForestNode<K>]) {
        self.entries = rpds::HashTrieMapSync::new_sync();
        self.roots = rpds::VectorSync::new_sync();
        self.parents = rpds::HashTrieMapSync::new_sync();
        for node in forest {
            self.roots.push_back_mut(node.key.clone());
            self.record(node, 0);
        }
    }

    fn record(&mut self, node: &ForestNode<K>, depth: u16) {
        self.entries.insert_mut(
            node.key.clone(),
            Entry {
                label: node.label.clone(),
                pick: node.pick,
                dim: node.dim,
                trail: node.trail.clone(),
                tint: node.tint,
                action: node.action.clone(),
                depth,
                children: node
                    .children
                    .iter()
                    .map(|child| child.key.clone())
                    .collect(),
            },
        );
        for child in &node.children {
            self.parents.insert_mut(child.key.clone(), node.key.clone());
            self.record(child, depth + 1);
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    pub(crate) fn is_branch(&self, key: &K) -> bool {
        self.entries
            .get(key)
            .is_some_and(|entry| !entry.children.is_empty())
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn is_collapsed(&self, key: &K) -> bool {
        self.collapsed.contains(key)
    }

    pub fn preset_collapsed(&mut self, key: &K) {
        self.collapsed.insert_mut(key.clone());
    }

    pub fn parent(&self, key: &K) -> Option<&K> {
        self.parents.get(key)
    }

    pub fn slice<R: View + Clone>(
        &self,
        store: &imba::store::Store,
        ui: &imba::ui::UiCtx,
        wrap: &Wrap<K, R>,
    ) -> ListSlice<R, K> {
        let mut slice = ListSlice::new();
        for key in self.roots.iter() {
            self.emit(key, &mut slice, store, ui, wrap);
        }
        slice
    }

    pub(crate) fn subtree_slice<R: View + Clone>(
        &self,
        key: &K,
        store: &imba::store::Store,
        ui: &imba::ui::UiCtx,
        wrap: &Wrap<K, R>,
    ) -> ListSlice<R, K> {
        let mut slice = ListSlice::new();
        self.emit(key, &mut slice, store, ui, wrap);
        slice
    }

    pub(crate) fn folded_slice<R: View + Clone>(
        &self,
        key: &K,
        store: &imba::store::Store,
        ui: &imba::ui::UiCtx,
        wrap: &Wrap<K, R>,
    ) -> ListSlice<R, K> {
        let mut slice = ListSlice::new();
        let Some(entry) = self.entries.get(key) else {
            return slice;
        };
        slice.push_keyed(key.clone(), wrap(self.row(entry, false), key), store, ui);
        slice
    }

    fn emit<R: View + Clone>(
        &self,
        key: &K,
        slice: &mut ListSlice<R, K>,
        store: &imba::store::Store,
        ui: &imba::ui::UiCtx,
        wrap: &Wrap<K, R>,
    ) {
        let Some(entry) = self.entries.get(key) else {
            return;
        };
        let start = slice.len();
        let folded = self.collapsed.contains(key);
        slice.push_keyed(
            key.clone(),
            wrap(self.row(entry, !entry.children.is_empty() && !folded), key),
            store,
            ui,
        );
        if !folded {
            for child in &entry.children {
                self.emit(child, slice, store, ui, wrap);
            }
        }
        if !entry.children.is_empty() {
            slice.cover(key.clone(), start..slice.len());
        }
    }

    fn row(&self, entry: &Entry<K>, expanded: bool) -> TreeRow {
        let label = TreeLabel::new(entry.label.clone(), entry.pick, entry.dim)
            .with_trail(entry.trail.clone())
            .with_action(entry.action.clone())
            .tinted(entry.tint);
        let row = match entry.children.is_empty() {
            true => TreeItemView::leaf(label, entry.depth),
            false => TreeItemView::branch(label, entry.depth, expanded),
        };
        let row = match entry.action.is_some() {
            true => row.with_action_priority(),
            false => row,
        };
        match entry.pick {
            true => row,
            false => row.toggling_on_body(),
        }
    }

    pub fn fold<R: View + Clone>(
        &mut self,
        key: &K,
        store: &imba::store::Store,
        ui: &imba::ui::UiCtx,
        wrap: &Wrap<K, R>,
    ) -> Option<ListSlice<R, K>> {
        if !self.is_branch(key) {
            return None;
        }
        self.collapsed.insert_mut(key.clone());
        Some(self.folded_slice(key, store, ui, wrap))
    }

    pub fn unfold<R: View + Clone>(
        &mut self,
        key: &K,
        store: &imba::store::Store,
        ui: &imba::ui::UiCtx,
        wrap: &Wrap<K, R>,
    ) -> Option<ListSlice<R, K>> {
        if !self.is_branch(key) {
            return None;
        }
        self.collapsed.remove_mut(key);
        Some(self.subtree_slice(key, store, ui, wrap))
    }

    pub fn flatten(&self) -> Vec<(u8, String, bool, K)> {
        fn walk<K: Clone + Eq + Hash + Send + Sync>(
            forest: &Forest<K>,
            keys: &[K],
            depth: u8,
            out: &mut Vec<(u8, String, bool, K)>,
        ) {
            for key in keys {
                let Some(entry) = forest.entries.get(key) else {
                    continue;
                };
                out.push((depth, entry.label.clone(), entry.pick, key.clone()));
                walk(forest, &entry.children, depth.saturating_add(1), out);
            }
        }
        let mut out = Vec::new();
        let roots: Vec<K> = self.roots.iter().cloned().collect();
        walk(self, &roots, 0, &mut out);
        out
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rows(&self) -> Vec<(u8, String, bool)> {
        self.flatten()
            .into_iter()
            .map(|(depth, label, pick, _)| (depth, label, pick))
            .collect()
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn rows_trailed(&self) -> Vec<(u8, String, bool)> {
        self.flatten()
            .into_iter()
            .map(|(depth, mut label, pick, key)| {
                if let Some(entry) = self.entries.get(&key) {
                    for (text, _) in &entry.trail {
                        label.push(' ');
                        label.push_str(text);
                    }
                }
                (depth, label, pick)
            })
            .collect()
    }
}

impl<K: Clone + Eq + Hash + Send + Sync + 'static> Forest<K> {
    pub(crate) fn toggle_in<R>(
        &mut self,
        list: &mut imba::list::ListView<R, K>,
        key: &K,
        store: &Store,
        ui: &imba::ui::UiCtx,
        wrap: &Wrap<K, R>,
    ) where
        R: View + Clone,
        R::Command: Send + 'static,
    {
        let Some(range) = list.row_range(key) else {
            return;
        };
        let slice = match self.is_collapsed(key) {
            true => self.unfold(key, store, ui, wrap),
            false => self.fold(key, store, ui, wrap),
        };
        if let Some(slice) = slice {
            list.splice_slice_animated(range, slice);
        }
    }

    pub fn fold_cursor<R>(
        &mut self,
        list: &mut imba::list::ListView<R, K>,
        expand: bool,
        store: &Store,
        ui: &imba::ui::UiCtx,
        wrap: &Wrap<K, R>,
    ) where
        R: View + Clone,
        R::Command: Send + 'static,
    {
        let Some(key) = list.cursor().cloned() else {
            return;
        };
        let branch = self.is_branch(&key);
        let folded = self.is_collapsed(&key);
        match (expand, branch, folded) {
            (true, true, true) => self.toggle_in(list, &key, store, ui, wrap),
            (false, true, false) => self.toggle_in(list, &key, store, ui, wrap),
            (false, _, _) => {
                if let Some(parent) = self.parent(&key).cloned() {
                    list.select_only(parent);
                }
            }
            _ => {}
        }
    }
}

pub struct ForestList<K, R = TreeRow>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    R: View + Clone,
{
    pub forest: Forest<K>,
    list: ScrollView<ListView<R, K>>,
    wrap: Wrap<K, R>,
}

impl<K, R> Clone for ForestList<K, R>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    R: View + Clone,
{
    fn clone(&self) -> Self {
        Self {
            forest: self.forest.clone(),
            list: self.list.clone(),
            wrap: Arc::clone(&self.wrap),
        }
    }
}

impl<K: Clone + Eq + Hash + Send + Sync + 'static> ForestList<K> {
    pub fn new(store: &Store) -> Self {
        Self::wrapping(store, Arc::new(|row, _| row))
    }
}

impl<K, R> ForestList<K, R>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    R: View + Clone,
    R::Command: Send + 'static,
{
    /// A forest whose rows the surface dresses (`Wrap`).
    pub fn wrapping(store: &Store, wrap: Wrap<K, R>) -> Self {
        Self {
            forest: Forest::new(store),
            list: ScrollView::new(
                ListView::empty().with_selection(crate::rows::selection_style(store)),
            ),
            wrap,
        }
    }

    pub fn set(&mut self, nodes: &[ForestNode<K>], store: &Store, ui: &imba::ui::UiCtx) {
        self.forest.set(nodes);
        let len = self.list.content().len();
        let slice = self.forest.slice(store, ui, &self.wrap);
        self.list.content_mut().splice_slice(0..len, slice);
        if self.list.content().cursor().is_none() {
            if let Some(first) = self.first_pickable(nodes) {
                self.list.content_mut().select_only(first);
            }
        }
    }

    fn first_pickable(&self, nodes: &[ForestNode<K>]) -> Option<K> {
        for node in nodes {
            if node.pick {
                return Some(node.key.clone());
            }
            if let Some(found) = self.first_pickable(&node.children) {
                return Some(found);
            }
        }
        None
    }

    pub fn list(&self) -> &ListView<R, K> {
        self.list.content()
    }

    pub fn scroll_y(&self) -> f32 {
        self.list.scroll_y()
    }

    pub fn list_mut(&mut self) -> &mut ListView<R, K> {
        self.list.content_mut()
    }

    pub fn toggle(&mut self, key: &K, store: &Store, ui: &imba::ui::UiCtx) {
        let mut forest = std::mem::replace(&mut self.forest, Forest::empty());
        forest.toggle_in(self.list.content_mut(), key, store, ui, &self.wrap);
        self.forest = forest;
    }

    pub fn fold_cursor(&mut self, expand: bool, store: &Store, ui: &imba::ui::UiCtx) {
        let mut forest = std::mem::replace(&mut self.forest, Forest::empty());
        forest.fold_cursor(self.list.content_mut(), expand, store, ui, &self.wrap);
        self.forest = forest;
    }
}

impl<K, R> View for ForestList<K, R>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    R: View + Clone,
    R::Command: Send + 'static,
{
    type Command = TreeListCommand<R::Command>;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        self.list
            .content_mut()
            .set_selection_style(crate::rows::selection_style(store));
        self.list.perform(store, ui, command, fx)
    }

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        self.list.focus_data(store, ui)
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        self.list.display(arena, store, ui)
    }
}

pub struct ForestSearcher<K, R = TreeRow>(std::marker::PhantomData<fn() -> (K, R)>);

impl<K, R> Clone for ForestSearcher<K, R> {
    fn clone(&self) -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<K, R> Default for ForestSearcher<K, R> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<K, R> crate::list_keyboard::Searcher for ForestSearcher<K, R>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    R: View + Clone + 'static,
    R::Command: Send + 'static,
{
    type View = ForestList<K, R>;
    type Key = K;

    fn capture(&self, view: &Self::View) -> crate::list_keyboard::ItemSource<K> {
        let forest = view.forest.clone();
        Box::new(move || {
            forest
                .flatten()
                .into_iter()
                .filter(|(_, _, pick, _)| *pick)
                .map(|(_, label, _, key)| (label, key))
                .collect()
        })
    }

    fn generation(&self, view: &Self::View) -> u64 {
        view.list().generation()
    }
}

impl<K, R> ListOps for ForestList<K, R>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    R: View + Clone,
    R::Command: Send + 'static,
{
    type Key = K;

    fn set_matches(&mut self, keys: &[K]) {
        self.list.set_matches(keys);
    }
    fn clear_matches(&mut self) {
        self.list.clear_matches();
    }
    fn match_count(&self) -> usize {
        self.list.match_count()
    }
    fn cursor_index(&self) -> Option<usize> {
        self.list.cursor_index()
    }
    fn step_index(&self, delta: isize) -> Option<usize> {
        self.list.step_index(delta)
    }
    fn matched_step_index(&self, delta: isize) -> Option<usize> {
        self.list.matched_step_index(delta)
    }
    fn edge_index(&self, edge: Edge) -> Option<usize> {
        self.list.edge_index(edge)
    }
    fn matched_edge_index(&self, edge: Edge) -> Option<usize> {
        self.list.matched_edge_index(edge)
    }
    fn page_index(&self, direction: isize) -> Option<usize> {
        self.list.page_index(direction)
    }
    fn select_command(&self, index: usize) -> Self::Command {
        self.list.select_command(index)
    }
    fn activate_command(&self, index: usize, trigger: ActivateTrigger) -> Self::Command {
        self.list.activate_command(index, trigger)
    }
    fn selected_index(command: &Self::Command) -> Option<usize> {
        <ScrollView<ListView<R, K>>>::selected_index(command)
    }
    fn activated(command: &Self::Command) -> Option<(usize, ActivateTrigger)> {
        <ScrollView<ListView<R, K>>>::activated(command)
    }
}
