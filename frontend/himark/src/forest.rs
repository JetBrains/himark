// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::hash::Hash;

use imba::{
    arena::Arena,
    list::{ActivateTrigger, Edge, ListOps, ListSlice, ListView},
    scroll::ScrollView,
    store::Store,
    UiCtx, View,
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

    pub fn is_branch(&self, key: &K) -> bool {
        self.entries
            .get(key)
            .is_some_and(|entry| !entry.children.is_empty())
    }

    pub fn is_collapsed(&self, key: &K) -> bool {
        self.collapsed.contains(key)
    }

    pub fn preset_collapsed(&mut self, key: &K) {
        self.collapsed.insert_mut(key.clone());
    }

    pub fn parent(&self, key: &K) -> Option<&K> {
        self.parents.get(key)
    }

    pub fn slice(&self, store: &imba::store::Store, ui: &imba::UiCtx) -> ListSlice<TreeRow, K> {
        let mut slice = ListSlice::new();
        for key in self.roots.iter() {
            self.emit(key, &mut slice, store, ui);
        }
        slice
    }

    pub fn subtree_slice(
        &self,
        key: &K,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
    ) -> ListSlice<TreeRow, K> {
        let mut slice = ListSlice::new();
        self.emit(key, &mut slice, store, ui);
        slice
    }

    pub fn folded_slice(
        &self,
        key: &K,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
    ) -> ListSlice<TreeRow, K> {
        let mut slice = ListSlice::new();
        let Some(entry) = self.entries.get(key) else {
            return slice;
        };
        slice.push_keyed(key.clone(), self.row(key, entry, false), store, ui);
        slice
    }

    fn emit(
        &self,
        key: &K,
        slice: &mut ListSlice<TreeRow, K>,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
    ) {
        let Some(entry) = self.entries.get(key) else {
            return;
        };
        let start = slice.len();
        let folded = self.collapsed.contains(key);
        slice.push_keyed(
            key.clone(),
            self.row(key, entry, !entry.children.is_empty() && !folded),
            store,
            ui,
        );
        if !folded {
            for child in &entry.children {
                self.emit(child, slice, store, ui);
            }
        }
        if !entry.children.is_empty() {
            slice.cover(key.clone(), start..slice.len());
        }
    }

    fn row(&self, _key: &K, entry: &Entry<K>, expanded: bool) -> TreeRow {
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

    pub fn fold(
        &mut self,
        key: &K,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
    ) -> Option<ListSlice<TreeRow, K>> {
        if !self.is_branch(key) {
            return None;
        }
        self.collapsed.insert_mut(key.clone());
        Some(self.folded_slice(key, store, ui))
    }

    pub fn unfold(
        &mut self,
        key: &K,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
    ) -> Option<ListSlice<TreeRow, K>> {
        if !self.is_branch(key) {
            return None;
        }
        self.collapsed.remove_mut(key);
        Some(self.subtree_slice(key, store, ui))
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

    #[cfg(any(test, feature = "test-support"))]
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
    pub fn toggle_in(
        &mut self,
        list: &mut imba::list::ListView<TreeRow, K>,
        key: &K,
        store: &Store,
        ui: &imba::UiCtx,
    ) {
        let Some(range) = list.row_range(key) else {
            return;
        };
        let slice = match self.is_collapsed(key) {
            true => self.unfold(key, store, ui),
            false => self.fold(key, store, ui),
        };
        if let Some(slice) = slice {
            list.splice_slice_animated(range, slice);
        }
    }

    pub fn fold_cursor(
        &mut self,
        list: &mut imba::list::ListView<TreeRow, K>,
        expand: bool,
        store: &Store,
        ui: &imba::UiCtx,
    ) {
        let Some(key) = list.cursor().cloned() else {
            return;
        };
        let branch = self.is_branch(&key);
        let folded = self.is_collapsed(&key);
        match (expand, branch, folded) {
            (true, true, true) => self.toggle_in(list, &key, store, ui),
            (false, true, false) => self.toggle_in(list, &key, store, ui),
            (false, _, _) => {
                if let Some(parent) = self.parent(&key).cloned() {
                    list.select_only(parent);
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
pub struct ForestList<K: Clone + Eq + Hash + Send + Sync + 'static> {
    pub forest: Forest<K>,
    list: ScrollView<ListView<TreeRow, K>>,
}

impl<K: Clone + Eq + Hash + Send + Sync + 'static> ForestList<K> {
    pub fn new(store: &Store) -> Self {
        Self {
            forest: Forest::new(store),
            list: ScrollView::new(
                ListView::empty().with_selection(crate::rows::selection_style(store)),
            ),
        }
    }

    pub fn set(&mut self, nodes: &[ForestNode<K>], store: &Store, ui: &imba::UiCtx) {
        self.forest.set(nodes);
        let len = self.list.content().len();
        let slice = self.forest.slice(store, ui);
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

    pub fn list(&self) -> &ListView<TreeRow, K> {
        self.list.content()
    }

    pub fn hover_row(&self, point: skia_safe::Point) -> Option<(K, skia_safe::Rect)> {
        let content_y = point.y + self.scroll_y();
        let index = self.list().index_at_y(content_y)?;
        let key = self.list().key_at(index)?.clone();
        let (top, height) = self.list().row_span(index)?;
        let width = self.list().laid_width().max(1.0);
        if point.x < 0.0 || point.x > width {
            return None;
        }
        Some((
            key,
            skia_safe::Rect::from_xywh(0.0, top - self.scroll_y(), width, height),
        ))
    }

    pub fn scroll_y(&self) -> f32 {
        self.list.scroll_y()
    }

    pub fn list_mut(&mut self) -> &mut ListView<TreeRow, K> {
        self.list.content_mut()
    }

    pub fn toggle(&mut self, key: &K, store: &Store, ui: &imba::UiCtx) {
        let mut forest = std::mem::replace(&mut self.forest, Forest::empty());
        forest.toggle_in(self.list.content_mut(), key, store, ui);
        self.forest = forest;
    }

    pub fn fold_cursor(&mut self, expand: bool, store: &Store, ui: &imba::UiCtx) {
        let mut forest = std::mem::replace(&mut self.forest, Forest::empty());
        forest.fold_cursor(self.list.content_mut(), expand, store, ui);
        self.forest = forest;
    }
}

impl<K: Clone + Eq + Hash + Send + Sync + 'static> View for ForestList<K> {
    type Command = TreeListCommand;

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

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        self.list.display(arena, store, ui)
    }
}

pub struct ForestSearcher<K>(std::marker::PhantomData<K>);

impl<K> Clone for ForestSearcher<K> {
    fn clone(&self) -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<K> Default for ForestSearcher<K> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<K: Clone + Eq + Hash + Send + Sync + 'static> crate::Searcher for ForestSearcher<K> {
    type View = ForestList<K>;
    type Key = K;

    fn capture(&self, view: &Self::View) -> crate::ItemSource<K> {
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

impl<K: Clone + Eq + Hash + Send + Sync + 'static> ListOps for ForestList<K> {
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
        <ScrollView<ListView<TreeRow, K>>>::selected_index(command)
    }
    fn activated(command: &Self::Command) -> Option<(usize, ActivateTrigger)> {
        <ScrollView<ListView<TreeRow, K>>>::activated(command)
    }
}
