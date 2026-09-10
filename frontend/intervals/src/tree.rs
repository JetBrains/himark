use std::{
    cmp::max,
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use crate::{Interval, IntervalRef, Offset, Order};

const MAX_CHILDREN: usize = 32;
pub(crate) const FIRST_INNER_ID: i64 = -3;

#[derive(Debug)]
pub(crate) enum Node<K, V> {
    Internal(Vec<Child<K, V>>),
    Leaf(Vec<Leaf<K, V>>),
}

#[derive(Clone, Copy, Debug)]
struct RawInterval {
    id: i64,
    start: i64,
    end: i64,
}

impl RawInterval {
    fn shift(&mut self, delta: i64) {
        self.start += delta;
        self.end += delta;
    }
}

#[derive(Debug)]
pub(crate) struct Child<K, V> {
    raw: RawInterval,
    node: Arc<Node<K, V>>,
}

#[derive(Debug)]
pub(crate) struct Leaf<K, V> {
    raw: RawInterval,
    key: K,
    value: V,
}

impl<K, V> Clone for Node<K, V>
where
    K: Clone,
    V: Clone,
{
    fn clone(&self) -> Self {
        match self {
            Self::Internal(children) => Self::Internal(children.clone()),
            Self::Leaf(leaves) => Self::Leaf(leaves.clone()),
        }
    }
}

impl<K, V> Clone for Child<K, V> {
    fn clone(&self) -> Self {
        Self {
            raw: self.raw,
            node: Arc::clone(&self.node),
        }
    }
}

impl<K, V> Clone for Leaf<K, V>
where
    K: Clone,
    V: Clone,
{
    fn clone(&self) -> Self {
        Self {
            raw: self.raw,
            key: self.key.clone(),
            value: self.value.clone(),
        }
    }
}

struct CollapseContext<'a, K> {
    parents: &'a mut rpds::HashTrieMapSync<i64, i64>,
    drop_empty: bool,

    dropped: &'a mut Vec<K>,
}

pub(crate) struct Query<'a, K, V> {
    order: Order,
    query_from: i64,
    query_to: i64,
    stack: Vec<Frame<'a, K, V>>,
}

struct Frame<'a, K, V> {
    node: &'a Node<K, V>,
    index: isize,
    delta: i64,
}

pub(crate) struct InsertEntry<K, V> {
    pub(crate) id: i64,
    pub(crate) interval: Interval<K, V>,
}

struct BuildNode<K, V> {
    id: i64,
    start: i64,
    end: i64,
    node: Node<K, V>,
}

pub(crate) fn query<K, V>(
    root: &Node<K, V>,
    range: Range<Offset>,
    order: Order,
) -> Query<'_, K, V> {
    Query::new(root, range, order)
}

pub(crate) fn find_by_path<'a, K, V>(
    root: &'a Node<K, V>,
    path: &[i64],
) -> Option<IntervalRef<'a, K, V>> {
    let mut node = root;
    let mut delta = 0;

    for id in path {
        match node {
            Node::Internal(children) => {
                let child = children.iter().find(|child| child.raw.id == *id)?;
                delta += child.raw.start;
                node = &child.node;
            }
            Node::Leaf(leaves) => {
                let leaf = leaves.iter().find(|leaf| leaf.raw.id == *id)?;
                let start = delta + leaf.raw.start;
                let end = delta + leaf.raw.end;
                return Some(IntervalRef {
                    range: decode_start(start)..decode_end(end),
                    greedy_left: start % 2 != 0,
                    greedy_right: end % 2 != 0,
                    key: &leaf.key,
                    value: &leaf.value,
                });
            }
        }
    }

    None
}

pub(crate) fn insert<K, V>(
    root: &mut Node<K, V>,
    root_id: i64,
    entries: Vec<InsertEntry<K, V>>,
    parents: &mut rpds::HashTrieMapSync<i64, i64>,
    next_inner_id: &mut i64,
) where
    K: Clone,
    V: Clone,
{
    if entries.is_empty() {
        return;
    }

    if !root.is_empty() {
        for entry in entries {
            insert_single(
                root,
                root_id,
                Leaf::from_entry(entry),
                parents,
                next_inner_id,
            );
        }
        return;
    }

    let mut leaves: Vec<Leaf<K, V>> = entries.into_iter().map(Leaf::from_entry).collect();
    leaves.sort_by_key(|leaf| leaf.raw.start);
    extinct_node(root, parents, None);
    *root = build_root(leaves, next_inner_id);
    adopt_all(root, root_id, parents);
}

pub(crate) fn expand<K, V>(root: &mut Node<K, V>, offset: Offset, len: Offset)
where
    K: Clone,
    V: Clone,
{
    if len == 0 {
        return;
    }
    let offset = i64::from(offset) * 2;
    let len = i64::from(len) * 2;
    expand_node(root, offset, len);
}

pub(crate) fn collapse<K, V>(
    root: &mut Node<K, V>,
    root_id: i64,
    offset: Offset,
    len: Offset,
    parents: &mut rpds::HashTrieMapSync<i64, i64>,
    drop_empty: bool,
    dropped: &mut Vec<K>,
) where
    K: Clone,
    V: Clone,
{
    if len == 0 {
        return;
    }

    let offset = i64::from(offset) * 2;
    let len = i64::from(len) * 2;
    let mut ctx = CollapseContext {
        parents,
        drop_empty,
        dropped,
    };

    collapse_node(root, 0, offset, len, &mut ctx);
    shrink_root(root, root_id, ctx.parents);
}

pub(crate) fn remove<K, V>(
    root: &mut Node<K, V>,
    root_id: i64,
    deletion_subtree: &HashMap<i64, HashSet<i64>>,
    parents: &mut rpds::HashTrieMapSync<i64, i64>,
) where
    K: Clone,
    V: Clone,
{
    remove_from_subtree(root, root_id, deletion_subtree, parents);
    shrink_root(root, root_id, parents);
}

impl<K, V> Node<K, V> {
    pub(crate) fn new() -> Self {
        Self::Leaf(Vec::new())
    }

    fn len(&self) -> usize {
        match self {
            Self::Internal(children) => children.len(),
            Self::Leaf(leaves) => leaves.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn max_end(&self) -> i64 {
        match self {
            Self::Internal(children) => children
                .iter()
                .map(|child| child.raw.end)
                .max()
                .unwrap_or(0),
            Self::Leaf(leaves) => leaves.iter().map(|leaf| leaf.raw.end).max().unwrap_or(0),
        }
    }

    fn normalize(&mut self) -> i64 {
        let delta = match self {
            Self::Internal(children) => children.first().map(|child| child.raw.start),
            Self::Leaf(leaves) => leaves.first().map(|leaf| leaf.raw.start),
        };
        let Some(delta) = delta else {
            return 0;
        };
        if delta != 0 {
            self.shift(-delta);
        }
        delta
    }

    fn shift(&mut self, delta: i64) {
        if delta == 0 {
            return;
        }
        match self {
            Self::Internal(children) => {
                for child in children {
                    child.raw.shift(delta);
                }
            }
            Self::Leaf(leaves) => {
                for leaf in leaves {
                    leaf.raw.shift(delta);
                }
            }
        }
    }
}

impl<K, V> Leaf<K, V> {
    fn from_entry(entry: InsertEntry<K, V>) -> Self {
        let key = entry.interval.key;
        let value = entry.interval.value;
        let start = encode_start(entry.interval.range.start, entry.interval.greedy_left);
        let end = encode_end(entry.interval.range.end, entry.interval.greedy_right);
        Self {
            raw: RawInterval {
                id: entry.id,
                start,
                end,
            },
            key,
            value,
        }
    }
}

fn build_root<K, V>(leaves: Vec<Leaf<K, V>>, next_inner_id: &mut i64) -> Node<K, V> {
    if leaves.len() <= MAX_CHILDREN {
        return Node::Leaf(leaves);
    }

    let mut layer = leaf_layer(leaves, next_inner_id);
    while layer.len() > MAX_CHILDREN {
        layer = parent_layer(layer, next_inner_id);
    }

    Node::Internal(
        layer
            .into_iter()
            .map(|item| Child {
                raw: RawInterval {
                    id: item.id,
                    start: item.start,
                    end: item.end,
                },
                node: Arc::new(item.node),
            })
            .collect(),
    )
}

fn leaf_layer<K, V>(leaves: Vec<Leaf<K, V>>, next_inner_id: &mut i64) -> Vec<BuildNode<K, V>> {
    let mut layer = Vec::with_capacity(leaves.len().div_ceil(MAX_CHILDREN));
    let mut leaves = leaves.into_iter();

    loop {
        let mut chunk = Vec::with_capacity(MAX_CHILDREN);
        for _ in 0..MAX_CHILDREN {
            let Some(leaf) = leaves.next() else {
                break;
            };
            chunk.push(leaf);
        }
        if chunk.is_empty() {
            break;
        }

        let mut node = Node::Leaf(chunk);
        let start = node.normalize();
        layer.push(BuildNode {
            id: new_inner_id(next_inner_id),
            start,
            end: start + node.max_end(),
            node,
        });
    }

    layer
}

fn parent_layer<K, V>(
    layer: Vec<BuildNode<K, V>>,
    next_inner_id: &mut i64,
) -> Vec<BuildNode<K, V>> {
    let mut next = Vec::with_capacity(layer.len().div_ceil(MAX_CHILDREN));
    let mut layer = layer.into_iter();

    loop {
        let mut children = Vec::with_capacity(MAX_CHILDREN);
        for _ in 0..MAX_CHILDREN {
            let Some(item) = layer.next() else {
                break;
            };
            children.push(Child {
                raw: RawInterval {
                    id: item.id,
                    start: item.start,
                    end: item.end,
                },
                node: Arc::new(item.node),
            });
        }
        if children.is_empty() {
            break;
        }

        let mut node = Node::Internal(children);
        let start = node.normalize();
        next.push(BuildNode {
            id: new_inner_id(next_inner_id),
            start,
            end: start + node.max_end(),
            node,
        });
    }

    next
}

fn insert_single<K, V>(
    root: &mut Node<K, V>,
    root_id: i64,
    leaf: Leaf<K, V>,
    parents: &mut rpds::HashTrieMapSync<i64, i64>,
    next_inner_id: &mut i64,
) where
    K: Clone,
    V: Clone,
{
    let Some((right_id, mut right)) = insert_rec(root, root_id, 0, leaf, parents, next_inner_id)
    else {
        return;
    };
    let mut left = std::mem::replace(root, Node::new());

    let left_id = new_inner_id(next_inner_id);
    adopt(&left, left_id, parents);
    parents.insert_mut(left_id, root_id);
    parents.insert_mut(right_id, root_id);

    let left_delta = left.normalize();
    let right_delta = right.normalize();
    *root = Node::Internal(vec![
        Child {
            raw: RawInterval {
                id: left_id,
                start: left_delta,
                end: left_delta + left.max_end(),
            },
            node: Arc::new(left),
        },
        Child {
            raw: RawInterval {
                id: right_id,
                start: right_delta,
                end: right_delta + right.max_end(),
            },
            node: Arc::new(right),
        },
    ]);
}

fn insert_rec<K, V>(
    node: &mut Node<K, V>,
    node_id: i64,
    node_delta: i64,
    mut leaf: Leaf<K, V>,
    parents: &mut rpds::HashTrieMapSync<i64, i64>,
    next_inner_id: &mut i64,
) -> Option<(i64, Node<K, V>)>
where
    K: Clone,
    V: Clone,
{
    let local_start = leaf.raw.start - node_delta;

    match node {
        Node::Leaf(leaves) => {
            leaf.raw.start = local_start;
            leaf.raw.end -= node_delta;
            let insert_index = leaves.partition_point(|leaf| leaf.raw.start <= local_start);
            parents.insert_mut(leaf.raw.id, node_id);
            leaves.insert(insert_index, leaf);
        }
        Node::Internal(children) if children.is_empty() => {
            *node = Node::new();
            return insert_rec(node, node_id, node_delta, leaf, parents, next_inner_id);
        }
        Node::Internal(children) => {
            let insert_index = children.partition_point(|child| child.raw.start <= local_start);
            let child_index = insert_index.saturating_sub(1).min(children.len() - 1);
            let child_id = children[child_index].raw.id;
            let child_start = children[child_index].raw.start;
            let (split, left_delta, left_max_end) = {
                let child = Arc::make_mut(&mut children[child_index].node);
                let split = insert_rec(
                    child,
                    child_id,
                    node_delta + child_start,
                    leaf,
                    parents,
                    next_inner_id,
                );
                let left_delta = child.normalize();
                (split, left_delta, child.max_end())
            };
            let left_start = child_start + left_delta;
            children[child_index].raw.start = left_start;
            children[child_index].raw.end = left_start + left_max_end;

            if let Some((right_id, mut right)) = split {
                let right_delta = right.normalize();
                let right_start = child_start + right_delta;
                children.insert(
                    child_index + 1,
                    Child {
                        raw: RawInterval {
                            id: right_id,
                            start: right_start,
                            end: right_start + right.max_end(),
                        },
                        node: Arc::new(right),
                    },
                );
                parents.insert_mut(right_id, node_id);
            }
        }
    }

    if node.len() > MAX_CHILDREN {
        let right_id = new_inner_id(next_inner_id);
        let right = split_node_off(node, node.len() / 2);
        adopt(&right, right_id, parents);
        Some((right_id, right))
    } else {
        None
    }
}

fn split_node_off<K, V>(node: &mut Node<K, V>, at: usize) -> Node<K, V> {
    match node {
        Node::Internal(children) => Node::Internal(children.split_off(at)),
        Node::Leaf(leaves) => Node::Leaf(leaves.split_off(at)),
    }
}

fn expand_node<K, V>(node: &mut Node<K, V>, offset: i64, len: i64)
where
    K: Clone,
    V: Clone,
{
    match node {
        Node::Internal(children) => {
            for child in children {
                let start = child.raw.start;
                let end = child.raw.end;
                if start < offset && offset < end {
                    expand_node(Arc::make_mut(&mut child.node), offset - start, len);
                    child.raw.end += len;
                } else if offset <= start {
                    child.raw.shift(len);
                }
            }
        }
        Node::Leaf(leaves) => {
            for leaf in leaves {
                let start = leaf.raw.start;
                let end = leaf.raw.end;
                if start < offset && offset < end {
                    leaf.raw.end += len;
                } else if offset <= start {
                    leaf.raw.shift(len);
                }
            }
        }
    }
}

fn collapse_node<K, V>(
    node: &mut Node<K, V>,
    d: i64,
    offset: i64,
    len: i64,
    ctx: &mut CollapseContext<'_, K>,
) where
    K: Clone,
    V: Clone,
{
    match node {
        Node::Internal(children) => {
            let mut result = Vec::with_capacity(children.len());
            let old_children = std::mem::take(children);

            for mut child in old_children {
                if child.raw.end <= offset {
                    result.push(child);
                } else if offset + len <= child.raw.start {
                    child.raw.shift(-len);
                    result.push(child);
                } else if ctx.drop_empty
                    && offset <= child.raw.start
                    && child.raw.end <= offset + len
                {
                    extinct_child(child, ctx.parents, Some(ctx.dropped));
                } else {
                    let is_empty = {
                        let child_node = Arc::make_mut(&mut child.node);
                        collapse_node(
                            child_node,
                            d + child.raw.start,
                            offset - child.raw.start,
                            len,
                            ctx,
                        );
                        child_node.is_empty()
                    };
                    if is_empty {
                        extinct_child(child, ctx.parents, Some(ctx.dropped));
                    } else {
                        let (delta, max_end) = {
                            let child_node = Arc::make_mut(&mut child.node);
                            let delta = child_node.normalize();
                            (delta, child_node.max_end())
                        };
                        child.raw.start += delta;
                        child.raw.end = child.raw.start + max_end;
                        result.push(child);
                    }
                }
            }

            *children = result;
        }
        Node::Leaf(leaves) => {
            let mut result = Vec::with_capacity(leaves.len());
            let old_leaves = std::mem::take(leaves);

            for mut leaf in old_leaves {
                if leaf.raw.end <= offset {
                    result.push(leaf);
                } else if offset + len <= leaf.raw.start {
                    leaf.raw.shift(-len);
                    result.push(leaf);
                } else if ctx.drop_empty && offset <= leaf.raw.start && leaf.raw.end <= offset + len
                {
                    ctx.parents.remove_mut(&leaf.raw.id);
                    ctx.dropped.push(leaf.key);
                } else {
                    let new_start = if offset < leaf.raw.start {
                        max(offset - ((d + leaf.raw.start) % 2), leaf.raw.start - len)
                    } else {
                        leaf.raw.start
                    };
                    let new_end = max(offset + ((d + leaf.raw.end) % 2), leaf.raw.end - len);
                    if ctx.drop_empty && new_end - new_start < 2 {
                        ctx.parents.remove_mut(&leaf.raw.id);
                        ctx.dropped.push(leaf.key);
                    } else {
                        leaf.raw.start = new_start;
                        leaf.raw.end = new_end;
                        result.push(leaf);
                    }
                }
            }

            *leaves = result;
        }
    }
}

fn remove_from_subtree<K, V>(
    node: &mut Node<K, V>,
    node_id: i64,
    deletion_subtree: &HashMap<i64, HashSet<i64>>,
    parents: &mut rpds::HashTrieMapSync<i64, i64>,
) where
    K: Clone,
    V: Clone,
{
    let Some(victims) = deletion_subtree.get(&node_id) else {
        return;
    };

    match node {
        Node::Leaf(leaves) => {
            let old_leaves = std::mem::take(leaves);
            for leaf in old_leaves {
                if victims.contains(&leaf.raw.id) {
                    parents.remove_mut(&leaf.raw.id);
                } else {
                    leaves.push(leaf);
                }
            }
        }
        Node::Internal(children) => {
            let old_children = std::mem::take(children);
            for mut child in old_children {
                if victims.contains(&child.raw.id) {
                    if deletion_subtree.contains_key(&child.raw.id) {
                        let child_node = Arc::make_mut(&mut child.node);
                        remove_from_subtree(child_node, child.raw.id, deletion_subtree, parents);
                        if child_node.is_empty() {
                            extinct_child(child, parents, None);
                        } else {
                            let (delta, max_end) = {
                                let delta = child_node.normalize();
                                (delta, child_node.max_end())
                            };
                            child.raw.start += delta;
                            child.raw.end = child.raw.start + max_end;
                            children.push(child);
                        }
                    } else {
                        extinct_child(child, parents, None);
                    }
                    continue;
                }

                if deletion_subtree.contains_key(&child.raw.id) {
                    let is_empty = {
                        let child_node = Arc::make_mut(&mut child.node);
                        remove_from_subtree(child_node, child.raw.id, deletion_subtree, parents);
                        child_node.is_empty()
                    };
                    if is_empty {
                        extinct_child(child, parents, None);
                    } else {
                        let (delta, max_end) = {
                            let child_node = Arc::make_mut(&mut child.node);
                            let delta = child_node.normalize();
                            (delta, child_node.max_end())
                        };
                        child.raw.start += delta;
                        child.raw.end = child.raw.start + max_end;
                        children.push(child);
                    }
                } else {
                    children.push(child);
                }
            }
        }
    }
}

fn shrink_root<K, V>(
    root: &mut Node<K, V>,
    root_id: i64,
    parents: &mut rpds::HashTrieMapSync<i64, i64>,
) where
    K: Clone,
    V: Clone,
{
    loop {
        match root {
            Node::Internal(children) if children.is_empty() => {
                *root = Node::new();
            }
            Node::Internal(children) if children.len() == 1 => {
                let child = children.pop().expect("single child");
                let mut child_node = match Arc::try_unwrap(child.node) {
                    Ok(child) => child,
                    Err(child) => child.as_ref().clone(),
                };
                child_node.shift(child.raw.start);
                adopt(&child_node, root_id, parents);
                parents.remove_mut(&child.raw.id);
                *root = child_node;
            }
            _ => break,
        }
    }
}

fn new_inner_id(next_inner_id: &mut i64) -> i64 {
    let id = *next_inner_id;
    *next_inner_id -= 1;
    id
}

fn adopt_all<K, V>(
    node: &Node<K, V>,
    parent_id: i64,
    parents: &mut rpds::HashTrieMapSync<i64, i64>,
) {
    match node {
        Node::Internal(children) => {
            for child in children {
                parents.insert_mut(child.raw.id, parent_id);
                adopt_all(&child.node, child.raw.id, parents);
            }
        }
        Node::Leaf(leaves) => {
            for leaf in leaves {
                parents.insert_mut(leaf.raw.id, parent_id);
            }
        }
    }
}

fn adopt<K, V>(node: &Node<K, V>, parent_id: i64, parents: &mut rpds::HashTrieMapSync<i64, i64>) {
    match node {
        Node::Internal(children) => {
            for child in children {
                parents.insert_mut(child.raw.id, parent_id);
            }
        }
        Node::Leaf(leaves) => {
            for leaf in leaves {
                parents.insert_mut(leaf.raw.id, parent_id);
            }
        }
    }
}

fn extinct_child<K, V>(
    child: Child<K, V>,
    parents: &mut rpds::HashTrieMapSync<i64, i64>,
    dropped: Option<&mut Vec<K>>,
) where
    K: Clone,
{
    parents.remove_mut(&child.raw.id);
    extinct_node(&child.node, parents, dropped);
}

fn extinct_node<K, V>(
    node: &Node<K, V>,
    parents: &mut rpds::HashTrieMapSync<i64, i64>,
    mut dropped: Option<&mut Vec<K>>,
) where
    K: Clone,
{
    match node {
        Node::Internal(children) => {
            for child in children {
                parents.remove_mut(&child.raw.id);
                extinct_node(&child.node, parents, dropped.as_deref_mut());
            }
        }
        Node::Leaf(leaves) => {
            for leaf in leaves {
                parents.remove_mut(&leaf.raw.id);
                if let Some(dropped) = dropped.as_deref_mut() {
                    dropped.push(leaf.key.clone());
                }
            }
        }
    }
}

fn encode_start(offset: Offset, greedy_left: bool) -> i64 {
    i64::from(offset) * 2 - i64::from(greedy_left)
}

fn encode_end(offset: Offset, greedy_right: bool) -> i64 {
    i64::from(offset) * 2 + i64::from(greedy_right)
}

fn decode_start(offset: i64) -> Offset {
    (offset / 2 + max(0, offset % 2)) as Offset
}

fn decode_end(offset: i64) -> Offset {
    (offset / 2) as Offset
}

fn intersects(query_start: i64, query_end: i64, start: i64, end: i64) -> bool {
    if query_start <= start {
        start <= query_end
    } else {
        query_start <= end
    }
}

#[cfg(test)]
pub(crate) fn depth<K, V>(node: &Node<K, V>) -> usize {
    match node {
        Node::Internal(children) => children
            .iter()
            .map(|child| depth(&child.node) + 1)
            .max()
            .unwrap_or(0),
        Node::Leaf(_) => 0,
    }
}

#[cfg(test)]
pub(crate) fn leaf_len<K, V>(node: &Node<K, V>) -> usize {
    match node {
        Node::Internal(children) => children.iter().map(|child| leaf_len(&child.node)).sum(),
        Node::Leaf(leaves) => leaves.len(),
    }
}

impl<'a, K, V> Query<'a, K, V> {
    fn new(root: &'a Node<K, V>, range: Range<Offset>, order: Order) -> Self {
        let index = match order {
            Order::Ascending => 0,
            Order::Descending => root.len() as isize - 1,
        };
        Self {
            order,
            query_from: i64::from(range.start) * 2,
            query_to: i64::from(range.end) * 2,
            stack: vec![Frame {
                node: root,
                index,
                delta: 0,
            }],
        }
    }

    fn push_child(&mut self, node: &'a Node<K, V>, delta: i64) {
        if node.is_empty() {
            return;
        }
        let index = match self.order {
            Order::Ascending => 0,
            Order::Descending => node.len() as isize - 1,
        };
        self.stack.push(Frame { node, index, delta });
    }
}

impl<'a, K, V> Iterator for Query<'a, K, V> {
    type Item = IntervalRef<'a, K, V>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let frame_index = self.stack.len().checked_sub(1)?;
            let (node, child_index, delta) = {
                let frame = &mut self.stack[frame_index];
                if frame.index < 0 || frame.index as usize >= frame.node.len() {
                    self.stack.pop();
                    continue;
                }
                let child_index = frame.index as usize;
                match self.order {
                    Order::Ascending => frame.index += 1,
                    Order::Descending => frame.index -= 1,
                }
                (frame.node, child_index, frame.delta)
            };

            match node {
                Node::Internal(children) => {
                    let child = &children[child_index];
                    let start = delta + child.raw.start;
                    let end = delta + child.raw.end;
                    if start > self.query_to {
                        continue;
                    }
                    if intersects(self.query_from, self.query_to, start, end) {
                        self.push_child(&child.node, start);
                    }
                }
                Node::Leaf(leaves) => {
                    let leaf = &leaves[child_index];
                    let start = delta + leaf.raw.start;
                    let end = delta + leaf.raw.end;
                    if start > self.query_to {
                        continue;
                    }
                    if !intersects(self.query_from, self.query_to, start, end) {
                        continue;
                    }
                    return Some(IntervalRef {
                        range: decode_start(start)..decode_end(end),
                        greedy_left: start % 2 != 0,
                        greedy_right: end % 2 != 0,
                        key: &leaf.key,
                        value: &leaf.value,
                    });
                }
            }
        }
    }
}
