// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

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

/// THE by-key index: the `keys` map that answers "which leaf is this
/// key?" and the `parents` map that walks a leaf back up to a root.
///
/// Both exist ONLY to serve by-key traffic — `find_by_id`, removal by
/// key, and the dedupe an insert does when a key already stands. The
/// tree itself never consults them: queries descend by RANGE. So a
/// lane with no by-key traffic should pay nothing for them, and
/// measured on a real parse that is most of what a span costs: 186 of
/// 247 bytes, two persistent hash tries carrying one entry each per
/// interval (see `tests::cost`).
pub trait ParentLinks {
    fn parent_of(&self, id: i64) -> Option<i64>;
    fn adopt(&mut self, id: i64, parent: i64);
    fn orphan(&mut self, id: i64);
}

pub trait IntervalIndex<K>: ParentLinks + Clone {
    fn empty() -> Self;

    fn id_of(&self, key: &K) -> Option<i64>;
    fn remember(&mut self, key: K, id: i64);
    /// Forget a key, answering the leaf id it held.
    fn forget(&mut self, key: &K) -> Option<i64>;

    /// How many keys stand. A keyless lane counts its leaves itself.
    fn len(&self) -> usize;
    fn note_len(&mut self, _len: usize) {}

    /// Does a key IDENTIFY an interval? A keyed lane dedupes on
    /// insert — the same key twice replaces. A keyless lane has one
    /// key value for every span, so deduping would collapse a whole
    /// parse into a single interval.
    const IDENTIFIES: bool = true;
}

/// The keyed lane: both maps, as before.
#[derive(Debug)]
pub struct Keyed<K> {
    parents: rpds::HashTrieMapSync<i64, i64>,
    keys: rpds::HashTrieMapSync<K, i64>,
}

impl<K: Eq + Hash> Clone for Keyed<K> {
    fn clone(&self) -> Self {
        Self {
            parents: self.parents.clone(),
            keys: self.keys.clone(),
        }
    }
}

impl<K: Eq + Hash> ParentLinks for Keyed<K> {
    fn parent_of(&self, id: i64) -> Option<i64> {
        self.parents.get(&id).copied()
    }

    fn adopt(&mut self, id: i64, parent: i64) {
        self.parents.insert_mut(id, parent);
    }

    fn orphan(&mut self, id: i64) {
        self.parents.remove_mut(&id);
    }
}

impl<K: Eq + Hash> IntervalIndex<K> for Keyed<K> {
    fn empty() -> Self {
        Self {
            parents: rpds::HashTrieMapSync::new_sync(),
            keys: rpds::HashTrieMapSync::new_sync(),
        }
    }

    fn id_of(&self, key: &K) -> Option<i64> {
        self.keys.get(key).copied()
    }

    fn remember(&mut self, key: K, id: i64) {
        self.keys.insert_mut(key, id);
    }

    fn forget(&mut self, key: &K) -> Option<i64> {
        let held = self.keys.get(key).copied();
        if held.is_some() {
            self.keys.remove_mut(key);
        }
        held
    }

    fn len(&self) -> usize {
        self.keys.size()
    }
}

/// The key type of a lane that is never looked up by key — a syntax
/// highlighter's spans. Zero-sized, so it costs nothing in the leaf,
/// and `Keyless` answers every by-key question with `None`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct NoKey;

/// The keyless lane: NO maps at all, just the leaf count. Every
/// by-key answer is `None`, so a lane may only choose this when
/// nothing looks it up — a syntax highlighter's spans, which are
/// written whole and deleted by range (`remove_in_range`).
///
/// Generic over the key so a lane can keep a typed key as DATA in the
/// leaf (queries carry it) while paying nothing to index it.
#[derive(Debug)]
pub struct Keyless<K = NoKey> {
    len: usize,
    _key: std::marker::PhantomData<K>,
}

impl<K> Clone for Keyless<K> {
    fn clone(&self) -> Self {
        Self {
            len: self.len,
            _key: std::marker::PhantomData,
        }
    }
}

impl<K> ParentLinks for Keyless<K> {
    fn parent_of(&self, _id: i64) -> Option<i64> {
        None
    }

    fn adopt(&mut self, _id: i64, _parent: i64) {}

    fn orphan(&mut self, _id: i64) {}
}

impl<K> IntervalIndex<K> for Keyless<K> {
    fn empty() -> Self {
        Self {
            len: 0,
            _key: std::marker::PhantomData,
        }
    }

    fn id_of(&self, _key: &K) -> Option<i64> {
        None
    }

    fn remember(&mut self, _key: K, _id: i64) {}

    fn forget(&mut self, _key: &K) -> Option<i64> {
        None
    }

    fn len(&self) -> usize {
        self.len
    }

    fn note_len(&mut self, len: usize) {
        self.len = len;
    }

    const IDENTIFIES: bool = false;
}

pub struct Intervals<K, V, I = Keyed<K>> {
    open_root: Arc<tree::Node<K, V>>,
    closed_root: Arc<tree::Node<K, V>>,

    index: I,
    /// The live interval count, maintained rather than derived: the
    /// keyed lane could read `keys.size()`, the keyless one has no
    /// map to ask.
    len: usize,
    next_leaf_id: i64,
    next_inner_id: i64,
    drop_empty: bool,
}

impl<K, V, I: Clone> Clone for Intervals<K, V, I> {
    fn clone(&self) -> Self {
        Self {
            open_root: Arc::clone(&self.open_root),
            closed_root: Arc::clone(&self.closed_root),
            index: self.index.clone(),
            len: self.len,
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

impl<K, V, I: IntervalIndex<K>> Intervals<K, V, I> {
    fn fresh() -> Self {
        Self {
            open_root: Arc::new(tree::Node::new()),
            closed_root: Arc::new(tree::Node::new()),
            index: I::empty(),
            len: 0,
            next_leaf_id: 1,
            next_inner_id: tree::FIRST_INNER_ID,
            drop_empty: true,
        }
    }
}

/// The KEYED flavour — the default, and what every existing caller
/// gets from `new()`.
impl<K: Eq + Hash, V> Intervals<K, V, Keyed<K>> {
    pub fn new() -> Self {
        Self::fresh()
    }

    pub fn keeping_empties() -> Self {
        Self {
            drop_empty: false,
            ..Self::fresh()
        }
    }
}

/// The KEYLESS flavour: a lane nobody looks up by key, so it carries
/// neither map. A syntax highlighter's spans are this — measured at
/// 186 of 247 bytes per span saved (`tests::cost`).
impl<K, V> Intervals<K, V, Keyless<K>> {
    pub fn keyless() -> Self {
        Self::fresh()
    }

    pub fn keyless_keeping_empties() -> Self {
        Self {
            drop_empty: false,
            ..Self::fresh()
        }
    }
}

impl<K, V, I: IntervalIndex<K>> Intervals<K, V, I> {
    pub fn len(&self) -> usize {
        debug_assert!(
            !I::IDENTIFIES || self.index.len() == self.len,
            "the keyed index and the maintained count disagree: {} vs {}",
            self.index.len(),
            self.len,
        );
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// TEST SUPPORT: the widest coordinate stored, (leaves, children).
    #[doc(hidden)]
    pub fn probe_extremes(&self) -> (i64, i64) {
        let open = self.open_root.probe_extremes();
        let closed = self.closed_root.probe_extremes();
        (open.0.max(closed.0), open.1.max(closed.1))
    }

    /// TEST SUPPORT: the tree's own shape, open root then closed.
    #[doc(hidden)]
    pub fn probe_shape(&self) -> (usize, usize, usize, usize, usize) {
        let open = self.open_root.probe_shape();
        let closed = self.closed_root.probe_shape();
        (
            open.0 + closed.0,
            open.1 + closed.1,
            open.2 + closed.2,
            open.3 + closed.3,
            open.4 + closed.4,
        )
    }
}

impl<K, V, I: IntervalIndex<K>> Intervals<K, V, I>
where
    K: Eq + Hash,
{
    pub fn find_by_id(&self, id: &K) -> Option<IntervalRef<'_, K, V>> {
        let leaf_id = self.index.id_of(id)?;
        let mut current = leaf_id;
        let mut path = vec![leaf_id];

        loop {
            let parent = self.index.parent_of(current)?;
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

impl<K, V, I: IntervalIndex<K>> Intervals<K, V, I>
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
        let mut intervals = match I::IDENTIFIES {
            true => last_intervals_by_key(intervals),
            // Every span carries the same keyless key; deduping by it
            // would keep exactly one of them.
            false => intervals.into_iter().collect(),
        };
        if intervals.is_empty() {
            return;
        }

        if I::IDENTIFIES {
            let old_ids: Vec<_> = intervals
                .iter()
                .filter_map(|interval| {
                    let old = self.index.id_of(&interval.key);
                    if old.is_some() {
                        self.index.forget(&interval.key);
                    }
                    old
                })
                .collect();
            self.len -= old_ids.len().min(self.len);
            self.remove_by_inner_ids(old_ids);
        }
        self.len += intervals.len();

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
            self.index.remember(key, id);
        }

        if !open_entries.is_empty() {
            tree::insert(
                Arc::make_mut(&mut self.open_root),
                OPEN_ROOT_ID,
                open_entries,
                &mut self.index,
                &mut self.next_inner_id,
            );
        }
        if !closed_entries.is_empty() {
            tree::insert(
                Arc::make_mut(&mut self.closed_root),
                CLOSED_ROOT_ID,
                closed_entries,
                &mut self.index,
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
                self.index.forget(key)
            })
            .collect();
        self.len -= ids.len().min(self.len);
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
            &mut self.index,
            self.drop_empty,
            &mut dropped,
        );
        tree::collapse(
            Arc::make_mut(&mut self.closed_root),
            CLOSED_ROOT_ID,
            offset,
            len,
            &mut self.index,
            self.drop_empty,
            &mut dropped,
        );
        self.len -= dropped.len().min(self.len);
        for key in dropped {
            self.index.forget(&key);
        }
    }

    /// Drop every interval intersecting `range` — the removal a
    /// KEYLESS lane uses, and a cheaper one for any lane: it needs no
    /// id lookup and no parent walk, only the region.
    pub fn remove_in_range(&mut self, range: Range<Offset>) {
        if range.start >= range.end {
            return;
        }
        let len = range.end - range.start;
        let mut dropped = Vec::new();
        tree::remove_in_range(
            Arc::make_mut(&mut self.open_root),
            OPEN_ROOT_ID,
            range.start,
            len,
            &mut self.index,
            &mut dropped,
        );
        tree::remove_in_range(
            Arc::make_mut(&mut self.closed_root),
            CLOSED_ROOT_ID,
            range.start,
            len,
            &mut self.index,
            &mut dropped,
        );
        self.len -= dropped.len().min(self.len);
        for key in dropped {
            self.index.forget(&key);
        }
    }

    fn remove_by_inner_ids(&mut self, ids: impl IntoIterator<Item = i64>) {
        let deletion_subtree = deletion_subtree(&self.index, ids);
        if deletion_subtree.is_empty() {
            return;
        }

        tree::remove(
            Arc::make_mut(&mut self.open_root),
            OPEN_ROOT_ID,
            &deletion_subtree,
            &mut self.index,
        );
        tree::remove(
            Arc::make_mut(&mut self.closed_root),
            CLOSED_ROOT_ID,
            &deletion_subtree,
            &mut self.index,
        );
    }
}

impl<K, V, I> IntervalQuery<K, V> for Intervals<K, V, I> {
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
    parents: &dyn ParentLinks,
    ids: impl IntoIterator<Item = i64>,
) -> HashMap<i64, HashSet<i64>> {
    let mut subtree = HashMap::new();
    for id in ids {
        let mut current = id;
        if parents.parent_of(current).is_none() {
            continue;
        }

        while current != OPEN_ROOT_ID && current != CLOSED_ROOT_ID {
            let Some(parent) = parents.parent_of(current) else {
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
