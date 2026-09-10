use std::sync::Arc;

use crate::node::{MetricsWithLength, Node};
use crate::Measure;

pub(crate) enum Siblings<T: Clone, M: Measure<T>> {
    Owned(Node<T, M>),
    Shared(Arc<Node<T, M>>),
}

impl<T: Clone, M: Measure<T>> Clone for Siblings<T, M> {
    fn clone(&self) -> Self {
        match self {
            Self::Owned(node) => Self::Owned(node.clone()),
            Self::Shared(node) => Self::Shared(Arc::clone(node)),
        }
    }
}

impl<T: Clone, M: Measure<T>> Siblings<T, M> {
    pub(crate) fn make_owned(&mut self) -> &mut Node<T, M> {
        if let Siblings::Shared(node) = self {
            *self = Siblings::Owned(node.as_ref().clone());
        }

        match self {
            Siblings::Owned(node) => node,
            Siblings::Shared(_) => unreachable!(),
        }
    }

    pub(crate) fn into_node(self) -> Node<T, M> {
        match self {
            Siblings::Owned(node) => node,
            Siblings::Shared(node) => match Arc::try_unwrap(node) {
                Ok(node) => node,
                Err(node) => node.as_ref().clone(),
            },
        }
    }

    pub(crate) fn arc_clone(&self) -> Arc<Node<T, M>> {
        match self {
            Siblings::Owned(node) => Arc::new(node.clone()),
            Siblings::Shared(node) => Arc::clone(node),
        }
    }

    pub(crate) fn size(&self) -> usize {
        match self {
            Siblings::Owned(Node::Internal(children)) => children.len(),
            Siblings::Owned(Node::Leaf(elements)) => elements.len(),
            Siblings::Shared(node) => match node.as_ref() {
                Node::Internal(children) => children.len(),
                Node::Leaf(elements) => elements.len(),
            },
        }
    }

    pub(crate) fn is_leaf(&self) -> bool {
        match self {
            Siblings::Owned(Node::Leaf(_)) => true,
            Siblings::Shared(node) => matches!(node.as_ref(), Node::Leaf(_)),
            _ => false,
        }
    }

    pub(crate) fn element(&self, sibling_index: u32) -> Option<&T> {
        let index = sibling_index as usize;
        match self {
            Siblings::Owned(Node::Leaf(elements)) => elements.get(index),
            Siblings::Shared(node) => match node.as_ref() {
                Node::Leaf(elements) => elements.get(index),
                Node::Internal(_) => None,
            },
            Siblings::Owned(Node::Internal(_)) => None,
        }
    }

    pub(crate) fn leaf(&self) -> Option<&[T]> {
        match self {
            Siblings::Owned(Node::Leaf(elements)) => Some(elements.as_slice()),
            Siblings::Shared(node) => match node.as_ref() {
                Node::Leaf(elements) => Some(elements.as_slice()),
                Node::Internal(_) => None,
            },
            Siblings::Owned(Node::Internal(_)) => None,
        }
    }

    pub(crate) fn metrics(&self, sibling_index: u32) -> MetricsWithLength<M::Metrics> {
        let index = sibling_index as usize;
        match self {
            Siblings::Owned(Node::Internal(children)) => children
                .get(index)
                .map(|child| child.metrics.clone())
                .unwrap_or(MetricsWithLength {
                    length: 0,
                    metrics: M::zero(),
                }),
            Siblings::Owned(Node::Leaf(elements)) => elements
                .get(index)
                .map(|element| MetricsWithLength {
                    length: 1,
                    metrics: M::measure(element),
                })
                .unwrap_or(MetricsWithLength {
                    length: 0,
                    metrics: M::zero(),
                }),
            Siblings::Shared(node) => match node.as_ref() {
                Node::Internal(children) => children
                    .get(index)
                    .map(|child| child.metrics.clone())
                    .unwrap_or(MetricsWithLength {
                        length: 0,
                        metrics: M::zero(),
                    }),
                Node::Leaf(elements) => elements
                    .get(index)
                    .map(|element| MetricsWithLength {
                        length: 1,
                        metrics: M::measure(element),
                    })
                    .unwrap_or(MetricsWithLength {
                        length: 0,
                        metrics: M::zero(),
                    }),
            },
        }
    }

    pub(crate) fn children(&self, sibling_index: u32) -> Option<Self> {
        let index = sibling_index as usize;
        let child = match self {
            Siblings::Owned(Node::Internal(children)) => {
                children.get(index).map(|child| &child.node)
            }
            Siblings::Owned(Node::Leaf(_)) => None,
            Siblings::Shared(node) => match node.as_ref() {
                Node::Internal(children) => children.get(index).map(|child| &child.node),
                Node::Leaf(_) => None,
            },
        }?;

        let children = Siblings::Shared(Arc::clone(child));
        match children.size() {
            0 => None,
            _ => Some(children),
        }
    }
}
