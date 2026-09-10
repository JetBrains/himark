use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::cursor::{Cursor, CursorIter};
use crate::node::{Child, MetricsWithLength, Node, BRANCHING_FACTOR, LEAVES_CAPACITY};
use crate::zipper::Zipper;
use crate::Measure;

pub struct Rope<T: Clone, M: Measure<T>> {
    pub(crate) metrics: MetricsWithLength<M::Metrics>,
    pub(crate) root: Arc<Node<T, M>>,
    pub(crate) _measure: PhantomData<fn() -> M>,
}

impl<T: Clone, M: Measure<T>> Clone for Rope<T, M> {
    fn clone(&self) -> Self {
        Self {
            metrics: self.metrics.clone(),
            root: Arc::clone(&self.root),
            _measure: PhantomData,
        }
    }
}

impl<T: Clone, M: Measure<T>> fmt::Debug for Rope<T, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rope")
            .field("length", &self.metrics.length)
            .field("metrics", &self.metrics.metrics)
            .finish()
    }
}

struct BuildNode<T: Clone, M: Measure<T>> {
    metrics: MetricsWithLength<M::Metrics>,
    node: Node<T, M>,
}

impl<T: Clone, M: Measure<T>> Rope<T, M> {
    pub fn new() -> Self {
        Self {
            metrics: MetricsWithLength {
                length: 0,
                metrics: M::zero(),
            },
            root: Arc::new(Node::empty()),
            _measure: PhantomData,
        }
    }

    pub fn metrics(&self) -> M::Metrics {
        self.metrics.metrics
    }

    pub fn len(&self) -> usize {
        self.metrics.length as usize
    }

    pub fn is_empty(&self) -> bool {
        self.metrics.length == 0
    }

    pub fn cursor(&self) -> Cursor<T, M> {
        Cursor::from_zipper(Zipper::new(Arc::clone(&self.root)))
            .expect("rope root must contain a leaf")
    }

    pub fn iter(&self) -> CursorIter<T, M> {
        self.cursor().iter()
    }

    pub fn from_iter(elements: impl IntoIterator<Item = T>) -> Self {
        Self::from_iter_with_branching(elements, LEAVES_CAPACITY, BRANCHING_FACTOR)
    }

    pub(crate) fn from_node(node: Node<T, M>) -> Self {
        let node = node.grow_tree().shrink_tree();
        let metrics = node.metrics();

        Self {
            metrics,
            root: Arc::new(node),
            _measure: PhantomData,
        }
    }

    pub(crate) fn into_node(self) -> Node<T, M> {
        match Arc::try_unwrap(self.root) {
            Ok(root) => root,
            Err(root) => root.as_ref().clone(),
        }
    }

    pub fn from_leaves_with_branching(
        leaves: impl IntoIterator<Item = Vec<T>>,
        leaf_capacity: usize,
        branching_factor: usize,
    ) -> Self {
        assert!(0 < leaf_capacity, "leaf capacity must be positive");
        assert!(0 < branching_factor, "branching factor must be positive");

        let leaves = leaves.into_iter();
        let (lower_bound, _) = leaves.size_hint();
        let mut layer = Vec::with_capacity(lower_bound);

        for leaf in leaves {
            assert!(leaf.len() <= leaf_capacity, "leaf exceeds leaf capacity");

            if !leaf.is_empty() {
                let length = u32::try_from(leaf.len()).expect("leaf length exceeds u32");
                let mut metrics = M::zero();

                for element in &leaf {
                    M::add_assign(&mut metrics, M::measure(element));
                }

                layer.push(BuildNode {
                    metrics: MetricsWithLength { length, metrics },
                    node: Node::Leaf(leaf),
                });
            }
        }

        match layer.is_empty() {
            true => Self::new(),
            false => {
                while 1 < layer.len() {
                    layer = Self::parent_layer(layer, branching_factor);
                }

                let root = layer.pop().expect("root node");
                Self {
                    metrics: root.metrics,
                    root: Arc::new(root.node),
                    _measure: PhantomData,
                }
            }
        }
    }

    pub(crate) fn from_iter_with_branching(
        elements: impl IntoIterator<Item = T>,
        leaf_capacity: usize,
        branching_factor: usize,
    ) -> Self {
        assert!(0 < leaf_capacity, "leaf capacity must be positive");
        assert!(0 < branching_factor, "branching factor must be positive");

        let mut elements = elements.into_iter();
        let (lower_bound, _) = elements.size_hint();
        let mut layer = Vec::with_capacity(lower_bound.div_ceil(leaf_capacity));

        loop {
            let mut leaf = Vec::with_capacity(leaf_capacity);
            let mut metrics = M::zero();

            while leaf.len() < leaf_capacity {
                match elements.next() {
                    Some(element) => {
                        M::add_assign(&mut metrics, M::measure(&element));
                        leaf.push(element);
                    }
                    None => break,
                }
            }

            match leaf.is_empty() {
                true => break,
                false => {
                    let length = u32::try_from(leaf.len()).expect("leaf length exceeds u32");
                    layer.push(BuildNode {
                        metrics: MetricsWithLength { length, metrics },
                        node: Node::Leaf(leaf),
                    });
                }
            }
        }

        match layer.is_empty() {
            true => Self::new(),
            false => {
                while 1 < layer.len() {
                    layer = Self::parent_layer(layer, branching_factor);
                }

                let root = layer.pop().expect("root node");
                Self {
                    metrics: root.metrics,
                    root: Arc::new(root.node),
                    _measure: PhantomData,
                }
            }
        }
    }

    fn parent_layer(layer: Vec<BuildNode<T, M>>, branching_factor: usize) -> Vec<BuildNode<T, M>> {
        let mut next = Vec::with_capacity(layer.len().div_ceil(branching_factor));
        let mut nodes = layer.into_iter();

        loop {
            let mut length = 0;
            let mut metrics = M::zero();
            let mut children = Vec::with_capacity(branching_factor);

            while children.len() < branching_factor {
                match nodes.next() {
                    Some(child) => {
                        length += child.metrics.length;
                        M::add_assign(&mut metrics, child.metrics.metrics);
                        children.push(Child {
                            metrics: child.metrics,
                            node: Arc::new(child.node),
                        });
                    }
                    None => break,
                }
            }

            match children.is_empty() {
                true => break,
                false => next.push(BuildNode {
                    metrics: MetricsWithLength { length, metrics },
                    node: Node::Internal(children),
                }),
            }
        }

        next
    }

    #[cfg(test)]
    pub(crate) fn assert_well_balanced(&self) {
        let (metrics, _) = Self::assert_node_well_balanced(self.root.as_ref(), true, 0);
        assert_eq!(metrics.length, self.metrics.length);
        assert_eq!(metrics.metrics, self.metrics.metrics);
    }

    #[cfg(test)]
    fn assert_node_well_balanced(
        node: &Node<T, M>,
        is_root: bool,
        unary_depth: usize,
    ) -> (MetricsWithLength<M::Metrics>, usize) {
        match node {
            Node::Leaf(elements) => {
                assert!(
                    is_root || !elements.is_empty(),
                    "non-root leaf must not be empty"
                );
                assert!(elements.len() <= LEAVES_CAPACITY, "leaf exceeds capacity");

                let mut metrics = M::zero();
                for element in elements {
                    M::add_assign(&mut metrics, M::measure(element));
                }

                (
                    MetricsWithLength {
                        length: u32::try_from(elements.len()).expect("leaf length exceeds u32"),
                        metrics,
                    },
                    0,
                )
            }
            Node::Internal(children) => {
                assert!(!children.is_empty(), "internal node must not be empty");
                assert!(
                    children.len() <= BRANCHING_FACTOR,
                    "internal node exceeds branching factor"
                );
                assert!(
                    !is_root || 1 < children.len(),
                    "root internal node must not have a single child"
                );
                assert!(
                    children.len() != 1 || unary_depth == 0,
                    "internal node must not continue a single-child chain"
                );

                let mut length = 0;
                let mut metrics = M::zero();
                let mut depth = None;
                let child_unary_depth = match children.len() {
                    1 => unary_depth + 1,
                    _ => 0,
                };

                for child in children {
                    let (child_metrics, child_depth) = Self::assert_node_well_balanced(
                        child.node.as_ref(),
                        false,
                        child_unary_depth,
                    );
                    assert_eq!(child.metrics.length, child_metrics.length);
                    assert_eq!(child.metrics.metrics, child_metrics.metrics);

                    match depth {
                        Some(depth) => assert_eq!(
                            depth, child_depth,
                            "internal node children must have equal depth"
                        ),
                        None => depth = Some(child_depth),
                    }

                    length += child.metrics.length;
                    M::add_assign(&mut metrics, child.metrics.metrics);
                }

                (MetricsWithLength { length, metrics }, depth.unwrap() + 1)
            }
        }
    }
}

impl<T: Clone, M: Measure<T>> FromIterator<T> for Rope<T, M> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from_iter(iter)
    }
}

pub fn from_leaves_with_branching<T, M>(
    leaves: impl IntoIterator<Item = Vec<T>>,
    leaf_capacity: usize,
    branching_factor: usize,
) -> Rope<T, M>
where
    T: Clone,
    M: Measure<T>,
{
    Rope::from_leaves_with_branching(leaves, leaf_capacity, branching_factor)
}
