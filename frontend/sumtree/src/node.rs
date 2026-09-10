use std::ops::Range;
use std::sync::Arc;

use crate::summary::{Item, Summary};

pub(crate) const BRANCHING_FACTOR: usize = 32;
pub(crate) const LEAVES_CAPACITY: usize = 64;

pub(crate) struct Weight<S> {
    pub(crate) len: usize,
    pub(crate) summary: S,
}

impl<S: Summary> Weight<S> {
    pub(crate) fn empty() -> Self {
        Self {
            len: 0,
            summary: S::empty(),
        }
    }

    pub(crate) fn add(&mut self, other: &Self) {
        self.len += other.len;
        self.summary.add(&other.summary);
    }
}

impl<S: Clone> Clone for Weight<S> {
    fn clone(&self) -> Self {
        Self {
            len: self.len,
            summary: self.summary.clone(),
        }
    }
}

pub(crate) enum Node<T: Item> {
    Internal {
        height: u8,
        children: Vec<Child<T>>,
    },
    Leaf(Vec<T>),
}

pub(crate) struct Child<T: Item> {
    pub(crate) weight: Weight<T::Summary>,
    pub(crate) node: Arc<Node<T>>,
}

impl<T: Item> Clone for Node<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Internal { height, children } => Self::Internal {
                height: *height,
                children: children.clone(),
            },
            Self::Leaf(items) => Self::Leaf(items.clone()),
        }
    }
}

impl<T: Item> Clone for Child<T> {
    fn clone(&self) -> Self {
        Self {
            weight: self.weight.clone(),
            node: Arc::clone(&self.node),
        }
    }
}

impl<T: Item> Child<T> {
    pub(crate) fn of(node: Arc<Node<T>>) -> Self {
        Self {
            weight: node.weight(),
            node,
        }
    }
}

impl<T: Item> Node<T> {
    pub(crate) fn height(&self) -> u8 {
        match self {
            Self::Internal { height, .. } => *height,
            Self::Leaf(_) => 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Internal { children, .. } => children.iter().map(|child| child.weight.len).sum(),
            Self::Leaf(items) => items.len(),
        }
    }

    pub(crate) fn weight(&self) -> Weight<T::Summary> {
        let mut weight = Weight::empty();
        match self {
            Self::Internal { children, .. } => {
                for child in children {
                    weight.add(&child.weight);
                }
            }
            Self::Leaf(items) => {
                for item in items {
                    weight.len += 1;
                    weight.summary.add(&item.summary());
                }
            }
        }
        weight
    }

    pub(crate) fn get(&self, mut index: usize) -> Option<&T> {
        let mut node = self;
        loop {
            match node {
                Self::Internal { children, .. } => {
                    let child = children.iter().find(|child| {
                        let inside = index < child.weight.len;
                        if !inside {
                            index -= child.weight.len;
                        }
                        inside
                    })?;
                    node = &child.node;
                }
                Self::Leaf(items) => return items.get(index),
            }
        }
    }

    pub(crate) fn summarize(&self, range: Range<usize>, out: &mut T::Summary) {
        match self {
            Self::Leaf(items) => {
                for item in &items[range] {
                    out.add(&item.summary());
                }
            }
            Self::Internal { children, .. } => {
                let mut offset = 0;
                for child in children {
                    let end = offset + child.weight.len;
                    if range.end <= offset {
                        break;
                    }
                    if range.start <= offset && end <= range.end {
                        out.add(&child.weight.summary);
                    } else if range.start < end {
                        let local = range.start.saturating_sub(offset)
                            ..(range.end - offset).min(child.weight.len);
                        child.node.summarize(local, out);
                    }
                    offset = end;
                }
            }
        }
    }

    pub(crate) fn push_slice(self: &Arc<Self>, range: Range<usize>, out: &mut Vec<Arc<Self>>) {
        if range.start >= range.end {
            return;
        }
        if range.start == 0 && range.end >= self.len() {
            out.push(Arc::clone(self));
            return;
        }
        match self.as_ref() {
            Self::Leaf(items) => {
                out.push(Arc::new(Self::Leaf(items[range].to_vec())));
            }
            Self::Internal { children, .. } => {
                let mut offset = 0;
                for child in children {
                    let end = offset + child.weight.len;
                    if range.end <= offset {
                        break;
                    }
                    if range.start < end {
                        let local = range.start.saturating_sub(offset)
                            ..(range.end - offset).min(child.weight.len);
                        child.node.push_slice(local, out);
                    }
                    offset = end;
                }
            }
        }
    }
}

pub(crate) fn concat<T: Item>(left: Arc<Node<T>>, right: Arc<Node<T>>) -> Vec<Arc<Node<T>>> {
    use std::cmp::Ordering;

    match left.height().cmp(&right.height()) {
        Ordering::Equal => match (left.as_ref(), right.as_ref()) {
            (Node::Leaf(a), Node::Leaf(b)) => {
                pack_leaves(a.iter().chain(b.iter()).cloned().collect())
            }
            (
                Node::Internal {
                    height, children, ..
                },
                Node::Internal {
                    children: others, ..
                },
            ) => pack_internal(
                *height,
                children.iter().chain(others.iter()).cloned().collect(),
            ),
            _ => unreachable!("equal heights are the same node kind"),
        },
        Ordering::Greater => {
            let Node::Internal { height, children } = left.as_ref() else {
                unreachable!("a taller node is internal");
            };
            let mut children = children.clone();
            let last = children.pop().expect("internal nodes are never empty");
            let merged = concat(last.node, right);
            children.extend(merged.into_iter().map(Child::of));
            pack_internal(*height, children)
        }
        Ordering::Less => {
            let Node::Internal { height, children } = right.as_ref() else {
                unreachable!("a taller node is internal");
            };
            let mut merged = concat(left, Arc::clone(&children[0].node));
            let mut out: Vec<Child<T>> = merged.drain(..).map(Child::of).collect();
            out.extend(children[1..].iter().cloned());
            pack_internal(*height, out)
        }
    }
}

fn pack_leaves<T: Item>(items: Vec<T>) -> Vec<Arc<Node<T>>> {
    if items.len() <= LEAVES_CAPACITY {
        return vec![Arc::new(Node::Leaf(items))];
    }
    let mut items = items;
    let right = items.split_off(items.len() / 2);
    vec![Arc::new(Node::Leaf(items)), Arc::new(Node::Leaf(right))]
}

fn pack_internal<T: Item>(height: u8, children: Vec<Child<T>>) -> Vec<Arc<Node<T>>> {
    if children.len() <= BRANCHING_FACTOR {
        return vec![Arc::new(Node::Internal { height, children })];
    }
    let mut children = children;
    let right = children.split_off(children.len() / 2);
    vec![
        Arc::new(Node::Internal { height, children }),
        Arc::new(Node::Internal {
            height,
            children: right,
        }),
    ]
}
