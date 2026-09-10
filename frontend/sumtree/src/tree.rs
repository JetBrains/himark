use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use crate::node::{concat, Child, Node, Weight, BRANCHING_FACTOR, LEAVES_CAPACITY};
use crate::summary::{Bias, Dimension, Item, Seek, Summary};

pub struct Splice<T> {
    pub range: Range<usize>,
    pub insert: Vec<T>,
}

pub struct SumTree<T: Item> {
    weight: Weight<T::Summary>,
    root: Arc<Node<T>>,
}

impl<T: Item> Clone for SumTree<T> {
    fn clone(&self) -> Self {
        Self {
            weight: self.weight.clone(),
            root: Arc::clone(&self.root),
        }
    }
}

impl<T: Item> Default for SumTree<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Item> fmt::Debug for SumTree<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SumTree")
            .field("len", &self.weight.len)
            .field("height", &self.root.height())
            .finish()
    }
}

impl<T: Item> SumTree<T> {
    pub fn new() -> Self {
        Self {
            weight: Weight::empty(),
            root: Arc::new(Node::Leaf(Vec::new())),
        }
    }

    pub fn from_iter(items: impl IntoIterator<Item = T>) -> Self {
        let mut items = items.into_iter();
        let mut layer: Vec<Arc<Node<T>>> = Vec::new();
        loop {
            let leaf: Vec<T> = items.by_ref().take(LEAVES_CAPACITY).collect();
            if leaf.is_empty() {
                break;
            }
            layer.push(Arc::new(Node::Leaf(leaf)));
        }
        if layer.is_empty() {
            return Self::new();
        }
        let mut height = 0u8;
        while layer.len() > 1 {
            height += 1;
            layer = layer
                .chunks(BRANCHING_FACTOR)
                .map(|nodes| {
                    Arc::new(Node::Internal {
                        height,
                        children: nodes
                            .iter()
                            .map(|node| Child::of(Arc::clone(node)))
                            .collect(),
                    })
                })
                .collect();
        }
        Self::from_root(layer.pop().expect("root layer"))
    }

    fn from_root(mut root: Arc<Node<T>>) -> Self {
        loop {
            let next = match root.as_ref() {
                Node::Internal { children, .. } if children.len() == 1 => {
                    Arc::clone(&children[0].node)
                }
                _ => break,
            };
            root = next;
        }
        Self {
            weight: root.weight(),
            root,
        }
    }

    pub fn len(&self) -> usize {
        self.weight.len
    }

    pub fn is_empty(&self) -> bool {
        self.weight.len == 0
    }

    pub fn summary(&self) -> &T::Summary {
        &self.weight.summary
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        (index < self.len()).then(|| self.root.get(index))?
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            stack: vec![(self.root.as_ref(), 0)],
            remaining: self.len(),
        }
    }

    pub fn iter_in(&self, range: Range<usize>) -> Iter<'_, T> {
        let range = range.start.min(self.len())..range.end.min(self.len());
        let mut stack = Vec::new();
        let mut skip = range.start;
        let mut node = self.root.as_ref();
        loop {
            match node {
                Node::Internal { children, .. } => {
                    let mut cursor = 0;
                    let mut next = None;
                    for child in children {
                        if skip < child.weight.len {
                            next = Some(child.node.as_ref());
                            break;
                        }
                        skip -= child.weight.len;
                        cursor += 1;
                    }
                    match next {
                        Some(child) => {
                            stack.push((node, cursor + 1));
                            node = child;
                        }
                        None => break,
                    }
                }
                Node::Leaf(_) => {
                    stack.push((node, skip));
                    break;
                }
            }
        }
        Iter {
            stack,
            remaining: range.end - range.start,
        }
    }

    pub fn summary_in(&self, range: Range<usize>) -> T::Summary {
        let range = range.start.min(self.len())..range.end.min(self.len());
        let mut out = T::Summary::empty();
        if range.start < range.end {
            self.root.summarize(range, &mut out);
        }
        out
    }

    pub fn offset_of<D: Dimension<T::Summary>>(&self, index: usize) -> D {
        assert!(index <= self.len(), "offset_of past the end");
        let mut skip = index;
        let mut offset = D::default();
        let mut node = self.root.as_ref();
        loop {
            match node {
                Node::Internal { children, .. } => {
                    let mut next = None;
                    for child in children {
                        if skip < child.weight.len {
                            next = Some(child.node.as_ref());
                            break;
                        }
                        skip -= child.weight.len;
                        offset.add(D::from_summary(&child.weight.summary));
                    }
                    match next {
                        Some(child) => node = child,
                        None => return offset,
                    }
                }
                Node::Leaf(items) => {
                    for item in &items[..skip] {
                        offset.add(D::from_summary(&item.summary()));
                    }
                    return offset;
                }
            }
        }
    }

    pub fn find<D: Dimension<T::Summary>>(&self, target: D, bias: Bias) -> Seek<D> {
        let mut index = 0;
        let mut start = D::default();
        let mut node = self.root.as_ref();
        loop {
            match node {
                Node::Internal { children, .. } => {
                    let mut next = None;
                    for child in children {
                        let mut end = start;
                        end.add(D::from_summary(&child.weight.summary));
                        if past(end, target, bias) {
                            next = Some(child.node.as_ref());
                            break;
                        }
                        start = end;
                        index += child.weight.len;
                    }
                    match next {
                        Some(child) => node = child,
                        None => return Seek { index, start },
                    }
                }
                Node::Leaf(items) => {
                    for item in items {
                        let mut end = start;
                        end.add(D::from_summary(&item.summary()));
                        if past(end, target, bias) {
                            return Seek { index, start };
                        }
                        start = end;
                        index += 1;
                    }
                    return Seek { index, start };
                }
            }
        }
    }

    pub fn summary_between<D: Dimension<T::Summary>>(&self, range: Range<D>) -> T::Summary {
        if !(range.start < range.end) {
            return T::Summary::empty();
        }
        let start = self.find(range.start, Bias::Right).index;
        let end_seek = self.find(range.end, Bias::Left);
        let end = if end_seek.index == self.len() {
            self.len()
        } else {
            end_seek.index + 1
        };
        self.summary_in(start..end)
    }

    pub fn set(&self, index: usize, item: T) -> Self {
        assert!(index < self.len(), "set past the end");
        let mut root = Arc::clone(&self.root);
        set_in(&mut root, index, item);
        let weight = root.weight();
        Self { weight, root }
    }

    pub fn slice(&self, range: Range<usize>) -> Self {
        let range = range.start.min(self.len())..range.end.min(self.len());
        if range.start >= range.end {
            return Self::new();
        }
        if range.start == 0 && range.end == self.len() {
            return self.clone();
        }
        let mut parts = Vec::new();
        self.root.push_slice(range, &mut parts);
        Self::assemble(parts)
    }

    pub fn append(&self, other: &Self) -> Self {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }
        let nodes = concat(Arc::clone(&self.root), Arc::clone(&other.root));
        Self::from_parts(nodes)
    }

    pub fn splice(&self, edits: impl IntoIterator<Item = Splice<T>>) -> Self {
        let mut parts: Vec<Arc<Node<T>>> = Vec::new();
        let mut cursor = 0;
        for edit in edits {
            assert!(
                cursor <= edit.range.start,
                "splices must be ascending and non-overlapping"
            );
            assert!(
                edit.range.start <= edit.range.end && edit.range.end <= self.len(),
                "splice out of bounds"
            );
            self.root.push_slice(cursor..edit.range.start, &mut parts);
            if !edit.insert.is_empty() {
                parts.push(Arc::clone(&Self::from_iter(edit.insert).root));
            }
            cursor = edit.range.end;
        }
        self.root.push_slice(cursor..self.len(), &mut parts);
        Self::assemble(parts)
    }

    fn assemble(parts: Vec<Arc<Node<T>>>) -> Self {
        let mut parts = parts.into_iter();
        let Some(first) = parts.next() else {
            return Self::new();
        };
        let mut root = first;
        for part in parts {
            match part.len() {
                0 => {}
                _ => root = Self::from_parts(concat(root, part)).root,
            }
        }
        Self::from_root(root)
    }

    fn from_parts(mut nodes: Vec<Arc<Node<T>>>) -> Self {
        match nodes.len() {
            1 => Self::from_root(nodes.pop().expect("one node")),
            2 => {
                let right = nodes.pop().expect("two nodes");
                let left = nodes.pop().expect("two nodes");
                let height = left.height() + 1;
                Self::from_root(Arc::new(Node::Internal {
                    height,
                    children: vec![Child::of(left), Child::of(right)],
                }))
            }
            n => unreachable!("concat yields one or two nodes, got {n}"),
        }
    }
}

#[cfg(test)]
impl<T: Item> SumTree<T> {
    pub(crate) fn root_for_tests(&self) -> &Node<T> {
        &self.root
    }

    pub(crate) fn root_arc_for_tests(&self) -> &Arc<Node<T>> {
        &self.root
    }
}

fn set_in<T: Item>(node: &mut Arc<Node<T>>, index: usize, item: T) {
    match Arc::make_mut(node) {
        Node::Leaf(items) => items[index] = item,
        Node::Internal { children, .. } => {
            let mut offset = 0;
            for child in children.iter_mut() {
                if index < offset + child.weight.len {
                    set_in(&mut child.node, index - offset, item);
                    child.weight = child.node.weight();
                    return;
                }
                offset += child.weight.len;
            }
            unreachable!("set index inside this subtree");
        }
    }
}

fn past<D: PartialOrd>(end: D, target: D, bias: Bias) -> bool {
    match bias {
        Bias::Left => end >= target,
        Bias::Right => end > target,
    }
}

impl<T: Item> FromIterator<T> for SumTree<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from_iter(iter)
    }
}

pub struct Iter<'a, T: Item> {
    stack: Vec<(&'a Node<T>, usize)>,
    remaining: usize,
}

impl<'a, T: Item> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        if self.remaining == 0 {
            return None;
        }
        loop {
            let (node, cursor) = self.stack.last_mut()?;
            match node {
                Node::Leaf(items) => match items.get(*cursor) {
                    Some(item) => {
                        *cursor += 1;
                        self.remaining -= 1;
                        return Some(item);
                    }
                    None => {
                        self.stack.pop();
                    }
                },
                Node::Internal { children, .. } => match children.get(*cursor) {
                    Some(child) => {
                        *cursor += 1;
                        self.stack.push((child.node.as_ref(), 0));
                    }
                    None => {
                        self.stack.pop();
                    }
                },
            }
        }
    }
}
