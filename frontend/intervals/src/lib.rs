use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    ops::Range,
    sync::Arc,
};

mod tree;

pub type Offset = u32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditStep {
    Retain(Offset),
    Insert(Offset),
    Delete(Offset),
}

const OPEN_ROOT_ID: i64 = -1;
const CLOSED_ROOT_ID: i64 = -2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Interval<K, V> {
    pub range: Range<Offset>,
    pub greedy_left: bool,
    pub greedy_right: bool,
    pub key: K,
    pub value: V,
}

#[derive(Clone, Debug)]
pub struct IntervalRef<'a, K, V> {
    pub range: Range<Offset>,
    pub greedy_left: bool,
    pub greedy_right: bool,
    pub key: &'a K,
    pub value: &'a V,
}

impl<K, V> IntervalRef<'_, K, V>
where
    K: Clone,
    V: Clone,
{
    pub fn cloned(&self) -> Interval<K, V> {
        Interval {
            range: self.range.clone(),
            greedy_left: self.greedy_left,
            greedy_right: self.greedy_right,
            key: self.key.clone(),
            value: self.value.clone(),
        }
    }
}

#[derive(Debug)]
pub struct Intervals<K, V> {
    open_root: Arc<tree::Node<K, V>>,
    closed_root: Arc<tree::Node<K, V>>,

    parents: rpds::HashTrieMapSync<i64, i64>,
    keys: rpds::HashTrieMapSync<K, i64>,
    next_leaf_id: i64,
    next_inner_id: i64,
    drop_empty: bool,
}

impl<K: Eq + Hash, V> Clone for Intervals<K, V> {
    fn clone(&self) -> Self {
        Self {
            open_root: Arc::clone(&self.open_root),
            closed_root: Arc::clone(&self.closed_root),
            parents: self.parents.clone(),
            keys: self.keys.clone(),
            next_leaf_id: self.next_leaf_id,
            next_inner_id: self.next_inner_id,
            drop_empty: self.drop_empty,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Order {
    Ascending,
    Descending,
}

pub trait IntervalQuery<K, V> {
    type Iter<'a>: Iterator<Item = IntervalRef<'a, K, V>>
    where
        Self: 'a,
        K: 'a,
        V: 'a;

    fn query(&self, range: Range<Offset>, order: Order) -> Self::Iter<'_>;
}

impl<K, V, T: IntervalQuery<K, V>> IntervalQuery<K, V> for &T {
    type Iter<'a>
        = T::Iter<'a>
    where
        Self: 'a,
        K: 'a,
        V: 'a;

    fn query(&self, range: Range<Offset>, order: Order) -> Self::Iter<'_> {
        (*self).query(range, order)
    }
}

pub struct Merged<A, B>(pub A, pub B);

impl<K, V, A, B> IntervalQuery<K, V> for Merged<A, B>
where
    A: IntervalQuery<K, V>,
    B: IntervalQuery<K, V>,
{
    type Iter<'a>
        = MergedQuery<A::Iter<'a>, B::Iter<'a>>
    where
        Self: 'a,
        K: 'a,
        V: 'a;

    fn query(&self, range: Range<Offset>, order: Order) -> Self::Iter<'_> {
        MergedQuery::new(
            self.0.query(range.clone(), order),
            self.1.query(range, order),
            order,
        )
    }
}

pub struct MergedQuery<A: Iterator, B: Iterator<Item = A::Item>> {
    order: Order,
    a: A,
    b: B,
    a_next: Option<A::Item>,
    b_next: Option<A::Item>,
}

impl<A: Iterator, B: Iterator<Item = A::Item>> MergedQuery<A, B> {
    pub fn new(a: A, b: B, order: Order) -> Self {
        Self {
            order,
            a,
            b,
            a_next: None,
            b_next: None,
        }
    }
}

impl<'a, K: 'a, V: 'a, A, B> Iterator for MergedQuery<A, B>
where
    A: Iterator<Item = IntervalRef<'a, K, V>>,
    B: Iterator<Item = IntervalRef<'a, K, V>>,
{
    type Item = IntervalRef<'a, K, V>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.a_next.is_none() {
            self.a_next = self.a.next();
        }
        if self.b_next.is_none() {
            self.b_next = self.b.next();
        }

        match (&self.a_next, &self.b_next) {
            (None, None) => None,
            (Some(_), None) => self.a_next.take(),
            (None, Some(_)) => self.b_next.take(),
            (Some(a), Some(b)) => {
                let take_a = match self.order {
                    Order::Ascending => a.range.start <= b.range.start,
                    Order::Descending => a.range.start >= b.range.start,
                };
                if take_a {
                    self.a_next.take()
                } else {
                    self.b_next.take()
                }
            }
        }
    }
}

pub struct Query<'a, K, V> {
    order: Order,
    open: tree::Query<'a, K, V>,
    closed: tree::Query<'a, K, V>,
    open_next: Option<IntervalRef<'a, K, V>>,
    closed_next: Option<IntervalRef<'a, K, V>>,
}

impl<K: Eq + Hash, V> Intervals<K, V> {
    pub fn new() -> Self {
        Self {
            open_root: Arc::new(tree::Node::new()),
            closed_root: Arc::new(tree::Node::new()),
            parents: rpds::HashTrieMapSync::new_sync(),
            keys: rpds::HashTrieMapSync::new_sync(),
            next_leaf_id: 1,
            next_inner_id: tree::FIRST_INNER_ID,
            drop_empty: true,
        }
    }

    pub fn keeping_empties() -> Self {
        Self {
            drop_empty: false,
            ..Self::new()
        }
    }
}

impl<K: Eq + Hash, V> Intervals<K, V> {
    pub fn len(&self) -> usize {
        self.keys.size()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl<K, V> Intervals<K, V>
where
    K: Eq + Hash,
{
    pub fn find_by_id(&self, id: &K) -> Option<IntervalRef<'_, K, V>> {
        let leaf_id = *self.keys.get(id)?;
        let mut current = leaf_id;
        let mut path = vec![leaf_id];

        loop {
            let parent = *self.parents.get(&current)?;
            if parent == OPEN_ROOT_ID {
                path.reverse();
                return tree::find_by_path(&self.open_root, &path);
            }
            if parent == CLOSED_ROOT_ID {
                path.reverse();
                return tree::find_by_path(&self.closed_root, &path);
            }
            path.push(parent);
            current = parent;
        }
    }
}

impl<K, V> Intervals<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    pub fn edit(&mut self, steps: impl IntoIterator<Item = EditStep>) {
        let mut offset: Offset = 0;
        for step in steps {
            match step {
                EditStep::Retain(len) => {
                    offset = offset.saturating_add(len);
                }
                EditStep::Insert(len) => {
                    self.expand(offset, len);
                    offset = offset.saturating_add(len);
                }
                EditStep::Delete(len) => {
                    self.collapse(offset, len);
                }
            }
        }
    }

    pub fn insert(&mut self, intervals: impl IntoIterator<Item = Interval<K, V>>) {
        let mut intervals = last_intervals_by_key(intervals);
        if intervals.is_empty() {
            return;
        }

        let old_ids: Vec<_> = intervals
            .iter()
            .filter_map(|interval| {
                let old = self.keys.get(&interval.key).copied();
                if old.is_some() {
                    self.keys.remove_mut(&interval.key);
                }
                old
            })
            .collect();
        self.remove_by_inner_ids(old_ids);

        intervals.sort_by_key(|interval| interval.range.start);

        let mut open_entries = Vec::new();
        let mut closed_entries = Vec::new();
        for interval in intervals {
            let id = self.next_leaf_id;
            self.next_leaf_id += 1;
            let key = interval.key.clone();
            let entry = tree::InsertEntry { id, interval };
            if entry.interval.greedy_left {
                closed_entries.push(entry);
            } else {
                open_entries.push(entry);
            }
            self.keys.insert_mut(key, id);
        }

        if !open_entries.is_empty() {
            tree::insert(
                Arc::make_mut(&mut self.open_root),
                OPEN_ROOT_ID,
                open_entries,
                &mut self.parents,
                &mut self.next_inner_id,
            );
        }
        if !closed_entries.is_empty() {
            tree::insert(
                Arc::make_mut(&mut self.closed_root),
                CLOSED_ROOT_ID,
                closed_entries,
                &mut self.parents,
                &mut self.next_inner_id,
            );
        }
    }

    pub fn graft(&mut self, offset: u32, other: &Intervals<K, V>) {
        let shifted = other.query(0..u32::MAX, Order::Ascending).map(|interval| {
            let mut interval = interval.cloned();
            interval.range.start = interval.range.start.saturating_add(offset);
            interval.range.end = interval.range.end.saturating_add(offset);
            interval
        });
        self.insert(shifted.collect::<Vec<_>>());
    }

    pub fn ordered_keys(&self) -> Vec<K> {
        self.query(0..u32::MAX, Order::Ascending)
            .map(|interval| interval.key.clone())
            .collect()
    }

    pub fn remove<'a>(&mut self, keys: impl IntoIterator<Item = &'a K>)
    where
        K: 'a,
    {
        let ids: Vec<i64> = keys
            .into_iter()
            .filter_map(|key| {
                let old = self.keys.get(key).copied();
                if old.is_some() {
                    self.keys.remove_mut(key);
                }
                old
            })
            .collect();
        self.remove_by_inner_ids(ids);
    }

    fn expand(&mut self, offset: Offset, len: Offset) {
        tree::expand(Arc::make_mut(&mut self.open_root), offset, len);
        tree::expand(Arc::make_mut(&mut self.closed_root), offset, len);
    }

    fn collapse(&mut self, offset: Offset, len: Offset) {
        let mut dropped = Vec::new();
        tree::collapse(
            Arc::make_mut(&mut self.open_root),
            OPEN_ROOT_ID,
            offset,
            len,
            &mut self.parents,
            self.drop_empty,
            &mut dropped,
        );
        tree::collapse(
            Arc::make_mut(&mut self.closed_root),
            CLOSED_ROOT_ID,
            offset,
            len,
            &mut self.parents,
            self.drop_empty,
            &mut dropped,
        );
        for key in dropped {
            self.keys.remove_mut(&key);
        }
    }

    fn remove_by_inner_ids(&mut self, ids: impl IntoIterator<Item = i64>) {
        let deletion_subtree = deletion_subtree(&self.parents, ids);
        if deletion_subtree.is_empty() {
            return;
        }

        tree::remove(
            Arc::make_mut(&mut self.open_root),
            OPEN_ROOT_ID,
            &deletion_subtree,
            &mut self.parents,
        );
        tree::remove(
            Arc::make_mut(&mut self.closed_root),
            CLOSED_ROOT_ID,
            &deletion_subtree,
            &mut self.parents,
        );
    }
}

impl<K, V> IntervalQuery<K, V> for Intervals<K, V> {
    type Iter<'a>
        = Query<'a, K, V>
    where
        Self: 'a,
        K: 'a,
        V: 'a;

    fn query(&self, range: Range<Offset>, order: Order) -> Self::Iter<'_> {
        Query {
            order,
            open: tree::query(&self.open_root, range.clone(), order),
            closed: tree::query(&self.closed_root, range, order),
            open_next: None,
            closed_next: None,
        }
    }
}

impl<'a, K, V> Iterator for Query<'a, K, V> {
    type Item = IntervalRef<'a, K, V>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.open_next.is_none() {
            self.open_next = self.open.next();
        }
        if self.closed_next.is_none() {
            self.closed_next = self.closed.next();
        }

        match (&self.open_next, &self.closed_next) {
            (None, None) => None,
            (Some(_), None) => self.open_next.take(),
            (None, Some(_)) => self.closed_next.take(),
            (Some(open), Some(closed)) => {
                let take_open = match self.order {
                    Order::Ascending => open.range.start <= closed.range.start,
                    Order::Descending => open.range.start >= closed.range.start,
                };
                if take_open {
                    self.open_next.take()
                } else {
                    self.closed_next.take()
                }
            }
        }
    }
}

fn deletion_subtree(
    parents: &rpds::HashTrieMapSync<i64, i64>,
    ids: impl IntoIterator<Item = i64>,
) -> HashMap<i64, HashSet<i64>> {
    let mut subtree = HashMap::new();
    for id in ids {
        let mut current = id;
        if !parents.contains_key(&current) {
            continue;
        }

        while current != OPEN_ROOT_ID && current != CLOSED_ROOT_ID {
            let Some(&parent) = parents.get(&current) else {
                break;
            };
            subtree
                .entry(parent)
                .or_insert_with(HashSet::new)
                .insert(current);
            current = parent;
        }
    }
    subtree
}

fn last_intervals_by_key<K, V>(
    intervals: impl IntoIterator<Item = Interval<K, V>>,
) -> Vec<Interval<K, V>>
where
    K: Clone + Eq + Hash,
{
    let intervals: Vec<_> = intervals.into_iter().collect();
    let mut seen = HashSet::with_capacity(intervals.len());
    let mut result = Vec::with_capacity(intervals.len());
    for interval in intervals.into_iter().rev() {
        if seen.insert(interval.key.clone()) {
            result.push(interval);
        }
    }
    result.reverse();
    result
}

impl<K: Eq + Hash, V> Default for Intervals<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl<K, V> Intervals<K, V> {
    pub(crate) fn open_tree_depth(&self) -> usize {
        tree::depth(&self.open_root)
    }

    pub(crate) fn open_tree_len(&self) -> usize {
        tree::leaf_len(&self.open_root)
    }
}

#[cfg(test)]
mod tests;
