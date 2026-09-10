use std::sync::Arc;

use crate::metrics::Measure;

pub(crate) const BRANCHING_FACTOR: usize = 32;
pub(crate) const LEAVES_CAPACITY: usize = 64;

#[derive(Clone, Copy)]
pub(crate) struct MetricsWithLength<M: Copy> {
    pub(crate) length: u32,
    pub(crate) metrics: M,
}

pub(crate) enum Node<T: Clone, M: Measure<T>> {
    Internal(Vec<Child<T, M>>),
    Leaf(Vec<T>),
}

pub(crate) struct Child<T: Clone, M: Measure<T>> {
    pub(crate) metrics: MetricsWithLength<M::Metrics>,
    pub(crate) node: Arc<Node<T, M>>,
}

impl<T: Clone, M: Measure<T>> Clone for Node<T, M> {
    fn clone(&self) -> Self {
        match self {
            Self::Internal(children) => Self::Internal(children.clone()),
            Self::Leaf(elements) => Self::Leaf(elements.clone()),
        }
    }
}

impl<T: Clone, M: Measure<T>> Clone for Child<T, M> {
    fn clone(&self) -> Self {
        Self {
            metrics: self.metrics.clone(),
            node: Arc::clone(&self.node),
        }
    }
}

impl<T: Clone, M: Measure<T>> Node<T, M> {
    pub(crate) fn empty() -> Self {
        Self::Leaf(Vec::new())
    }

    pub(crate) fn into_option(self) -> Option<Self> {
        match self {
            Self::Internal(children) if children.is_empty() => None,
            Self::Leaf(elements) if elements.is_empty() => None,
            node => Some(node),
        }
    }

    pub(crate) fn depth(&self) -> usize {
        match self {
            Self::Internal(children) => children.first().map_or(1, |child| child.node.depth() + 1),
            Self::Leaf(_) => 0,
        }
    }

    pub(crate) fn delete_prefix(&mut self, mut length: u32) -> u32 {
        let original_length = length;

        match self {
            Self::Internal(children) => {
                while 0 < length {
                    match children.first() {
                        Some(child) if child.metrics.length <= length => {
                            length -= child.metrics.length;
                            children.remove(0);
                        }
                        Some(_) => {
                            let deleted =
                                Arc::make_mut(&mut children[0].node).delete_prefix(length);
                            children[0].metrics = children[0].node.metrics();
                            length -= deleted;
                        }
                        None => break,
                    }
                }
            }
            Self::Leaf(elements) => {
                let deleted = (length as usize).min(elements.len());
                elements.drain(..deleted);
                length -= deleted as u32;
            }
        }

        original_length - length
    }

    pub(crate) fn metrics(&self) -> MetricsWithLength<M::Metrics> {
        let mut length = 0;
        let mut metrics = M::zero();

        match self {
            Self::Internal(children) => {
                for child in children {
                    length += child.metrics.length;
                    M::add_assign(&mut metrics, child.metrics.metrics);
                }
            }
            Self::Leaf(elements) => {
                length = u32::try_from(elements.len()).expect("leaf length exceeds u32");
                for element in elements {
                    M::add_assign(&mut metrics, M::measure(element));
                }
            }
        }

        MetricsWithLength { length, metrics }
    }

    pub(crate) fn balance_children(self) -> Self {
        let Self::Internal(children) = self else {
            return self;
        };

        let mut balance_needed = false;
        for child in &children {
            balance_needed = match child.node.as_ref() {
                Self::Internal(child) => child.is_empty() || child.len() > BRANCHING_FACTOR,
                Self::Leaf(elements) => elements.is_empty() || elements.len() > LEAVES_CAPACITY,
            };
            if balance_needed {
                break;
            }
        }

        if !balance_needed {
            return Self::Internal(children);
        }

        let mut balanced = Vec::with_capacity(children.len());

        for child in children {
            let empty = match child.node.as_ref() {
                Self::Internal(children) => children.is_empty(),
                Self::Leaf(elements) => elements.is_empty(),
            };
            let overfull = match child.node.as_ref() {
                Self::Internal(children) => children.len() > BRANCHING_FACTOR,
                Self::Leaf(elements) => elements.len() > LEAVES_CAPACITY,
            };

            match (empty, overfull) {
                (true, _) => {}
                (false, false) => {
                    balanced.push(child);
                }
                (false, true) => {
                    let node = match Arc::try_unwrap(child.node) {
                        Ok(child) => child,
                        Err(child) => child.as_ref().clone(),
                    };

                    for node in node.split() {
                        balanced.push(Child {
                            metrics: node.metrics(),
                            node: Arc::new(node),
                        });
                    }
                }
            }
        }

        match balanced.is_empty() {
            true => Self::Leaf(Vec::new()),
            false => Self::Internal(balanced),
        }
    }

    pub(crate) fn grow_tree(mut self) -> Self {
        loop {
            if matches!(&self, Self::Leaf(elements) if elements.len() > LEAVES_CAPACITY) {
                let metrics = self.metrics();
                self = Self::Internal(vec![Child {
                    metrics,
                    node: Arc::new(self),
                }]);
            }

            self = self.balance_children();

            match &self {
                Self::Internal(children) if children.len() > BRANCHING_FACTOR => {
                    let metrics = self.metrics();
                    self = Self::Internal(vec![Child {
                        metrics,
                        node: Arc::new(self),
                    }]);
                }
                _ => return self,
            }
        }
    }

    pub(crate) fn shrink_tree(mut self) -> Self {
        loop {
            match self {
                Self::Internal(mut children) if children.len() == 1 => {
                    let child = children.pop().expect("single child");
                    self = match Arc::try_unwrap(child.node) {
                        Ok(child) => child,
                        Err(child) => child.as_ref().clone(),
                    };
                }
                _ => return self,
            }
        }
    }

    fn split(self) -> Vec<Self> {
        match self {
            Self::Internal(children) => {
                if children.len() <= BRANCHING_FACTOR {
                    return vec![Self::Internal(children)];
                }

                let mut out = Vec::with_capacity(children.len().div_ceil(BRANCHING_FACTOR));
                let mut stack = vec![children];

                while let Some(mut children) = stack.pop() {
                    if children.len() <= BRANCHING_FACTOR {
                        out.push(Self::Internal(children));
                    } else {
                        let right = children.split_off(children.len() / 2);
                        stack.push(right);
                        stack.push(children);
                    }
                }

                out
            }
            Self::Leaf(elements) => {
                if elements.len() <= LEAVES_CAPACITY {
                    return vec![Self::Leaf(elements)];
                }

                let mut out = Vec::with_capacity(elements.len().div_ceil(LEAVES_CAPACITY));
                let mut stack = vec![elements];

                while let Some(mut elements) = stack.pop() {
                    if elements.len() <= LEAVES_CAPACITY {
                        out.push(Self::Leaf(elements));
                    } else {
                        let right = elements.split_off(elements.len() / 2);
                        stack.push(right);
                        stack.push(elements);
                    }
                }

                out
            }
        }
    }
}
