use std::hash::Hash;

use imba::{
    arena::Arena,
    constraints::Constraints,
    list::{ListSlice, ListView, SearchableList},
    scroll::ScrollView,
    store::Store,
    Thunk, UiCtx, View,
};

use crate::tree_item::{TreeItemView, TreeLabel, TreeListCommand};

pub struct ForestNode<K> {
    pub key: K,
    pub label: String,

    pub pick: bool,

    pub dim: bool,

    pub trail: Vec<(String, skia_safe::Color)>,
    pub children: Vec<ForestNode<K>>,
}

pub type TreeRow = TreeItemView<TreeLabel>;

#[derive(Clone)]
struct Entry<K> {
    label: String,
    pick: bool,
    dim: bool,
    trail: Vec<(String, skia_safe::Color)>,
    depth: u16,
    children: Vec<K>,
}

#[derive(Clone)]
pub struct Forest<K: Clone + Eq + Hash> {
    entries: rpds::HashTrieMapSync<K, Entry<K>>,
    roots: rpds::VectorSync<K>,
    parents: rpds::HashTrieMapSync<K, K>,
    collapsed: rpds::HashTrieSetSync<K>,
    row_height: f32,
}

impl<K: Clone + Eq + Hash + Send + Sync> Forest<K> {
    fn empty() -> Self {
        Self {
            entries: rpds::HashTrieMapSync::new_sync(),
            roots: rpds::VectorSync::new_sync(),
            parents: rpds::HashTrieMapSync::new_sync(),
            collapsed: rpds::HashTrieSetSync::new_sync(),
            row_height: 1.0,
        }
    }

    pub fn new(store: &imba::store::Store) -> Self {
        Self {
            entries: rpds::HashTrieMapSync::new_sync(),
            roots: rpds::VectorSync::new_sync(),
            parents: rpds::HashTrieMapSync::new_sync(),
            collapsed: rpds::HashTrieSetSync::new_sync(),
            row_height: crate::env::Themes::of(store).ui().tree.row_height.max(1.0),
        }
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

    pub fn slice(&self) -> ListSlice<TreeRow, K> {
        let mut slice = ListSlice::new();
        for key in self.roots.iter() {
            self.emit(key, &mut slice);
        }
        slice
    }

    pub fn subtree_slice(&self, key: &K) -> ListSlice<TreeRow, K> {
        let mut slice = ListSlice::new();
        self.emit(key, &mut slice);
        slice
    }

    pub fn folded_slice(&self, key: &K) -> ListSlice<TreeRow, K> {
        let mut slice = ListSlice::new();
        let Some(entry) = self.entries.get(key) else {
            return slice;
        };
        slice.push_keyed(key.clone(), self.row(key, entry, false), self.row_height);
        slice
    }

    fn emit(&self, key: &K, slice: &mut ListSlice<TreeRow, K>) {
        let Some(entry) = self.entries.get(key) else {
            return;
        };
        let start = slice.len();
        let folded = self.collapsed.contains(key);
        slice.push_keyed(
            key.clone(),
            self.row(key, entry, !entry.children.is_empty() && !folded),
            self.row_height,
        );
        if !folded {
            for child in &entry.children {
                self.emit(child, slice);
            }
        }
        if !entry.children.is_empty() {
            slice.cover(key.clone(), start..slice.len());
        }
    }

    fn row(&self, _key: &K, entry: &Entry<K>, expanded: bool) -> TreeRow {
        let mut label = TreeLabel::new(entry.label.clone(), entry.pick, entry.dim)
            .with_trail(entry.trail.clone());
        if !entry.children.is_empty() && !entry.dim {
            label = label.strong();
        }
        let row = match entry.children.is_empty() {
            true => TreeItemView::leaf(label, entry.depth),
            false => TreeItemView::branch(label, entry.depth, expanded),
        };
        match entry.pick {
            true => row,
            false => row.toggling_on_body(),
        }
    }

    pub fn fold(&mut self, key: &K) -> Option<ListSlice<TreeRow, K>> {
        if !self.is_branch(key) {
            return None;
        }
        self.collapsed.insert_mut(key.clone());
        Some(self.folded_slice(key))
    }

    pub fn unfold(&mut self, key: &K) -> Option<ListSlice<TreeRow, K>> {
        if !self.is_branch(key) {
            return None;
        }
        self.collapsed.remove_mut(key);
        Some(self.subtree_slice(key))
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
    pub fn toggle_in(&mut self, list: &mut imba::list::ListView<TreeRow, K>, key: &K) {
        let Some(range) = list.row_range(key) else {
            return;
        };
        let slice = match self.is_collapsed(key) {
            true => self.unfold(key),
            false => self.fold(key),
        };
        if let Some(slice) = slice {
            list.splice_slice_animated(range, slice);
        }
    }

    pub fn fold_cursor(&mut self, list: &mut imba::list::ListView<TreeRow, K>, expand: bool) {
        let Some(key) = list.cursor().cloned() else {
            return;
        };
        let branch = self.is_branch(&key);
        let folded = self.is_collapsed(&key);
        match (expand, branch, folded) {
            (true, true, true) => self.toggle_in(list, &key),
            (false, true, false) => self.toggle_in(list, &key),
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
                ListView::from_measured([]).with_selection(crate::rows::selection_style(store)),
            ),
        }
    }

    pub fn set(&mut self, nodes: &[ForestNode<K>]) {
        self.forest.set(nodes);
        let len = self.list.content().len();
        let slice = self.forest.slice();
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

    pub fn toggle(&mut self, key: &K) {
        let mut forest = std::mem::replace(&mut self.forest, Forest::empty());
        forest.toggle_in(self.list.content_mut(), key);
        self.forest = forest;
    }

    pub fn fold_cursor(&mut self, expand: bool) {
        let mut forest = std::mem::replace(&mut self.forest, Forest::empty());
        forest.fold_cursor(self.list.content_mut(), expand);
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

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        self.list.layout(arena, store, ui, constraints)
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

impl<K: Clone + Eq + Hash + Send + Sync + 'static> SearchableList<K> for ForestList<K> {
    fn set_matches(&mut self, keys: &[K]) {
        self.list.set_matches(keys);
    }
    fn clear_matches(&mut self) {
        self.list.clear_matches();
    }
    fn step_matched(&mut self, delta: isize) {
        self.list.step_matched(delta);
    }
    fn match_count(&self) -> usize {
        self.list.match_count()
    }
}
