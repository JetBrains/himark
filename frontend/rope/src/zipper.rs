use crate::Measure;

use crate::node::{Child, MetricsWithLength, Node};
use crate::rope::Rope;
use crate::siblings::Siblings;
use std::marker::PhantomData;
use std::sync::Arc;

struct Frame<T: Clone, M: Measure<T>> {
    location: MetricsWithLength<M::Metrics>,
    sibling_index: u32,
    siblings: Siblings<T, M>,
}

pub(crate) struct Zipper<T: Clone, M: Measure<T>> {
    frames: Vec<Frame<T, M>>,
    frame: Frame<T, M>,
    _measure: PhantomData<fn() -> M>,
}

impl<T: Clone, M: Measure<T>> Clone for Frame<T, M> {
    fn clone(&self) -> Self {
        Self {
            location: self.location.clone(),
            sibling_index: self.sibling_index,
            siblings: self.siblings.clone(),
        }
    }
}

impl<T: Clone, M: Measure<T>> Clone for Zipper<T, M> {
    fn clone(&self) -> Self {
        Self {
            frames: self.frames.clone(),
            frame: self.frame.clone(),
            _measure: PhantomData,
        }
    }
}

impl<T: Clone, M: Measure<T>> Zipper<T, M> {
    pub(crate) fn new(root: Arc<Node<T, M>>) -> Self {
        Self {
            frames: Vec::new(),
            frame: Frame {
                location: MetricsWithLength {
                    length: 0,
                    metrics: M::zero(),
                },
                sibling_index: 0,
                siblings: Siblings::Shared(root),
            },
            _measure: PhantomData,
        }
    }

    pub(crate) fn location(&self) -> MetricsWithLength<M::Metrics> {
        self.frame.location
    }

    pub(crate) fn metrics(&self) -> MetricsWithLength<M::Metrics> {
        self.frame.siblings.metrics(self.frame.sibling_index)
    }

    pub(crate) fn element(&self) -> Option<&T> {
        self.frame.siblings.element(self.frame.sibling_index)
    }

    pub(crate) fn leaf(&self) -> Option<&[T]> {
        self.frame.siblings.leaf()
    }

    pub(crate) fn index_in_leaf(&self) -> Option<usize> {
        self.is_leaf().then_some(self.frame.sibling_index as usize)
    }

    pub(crate) fn has_right(&self) -> bool {
        (self.frame.sibling_index as usize) + 1 < self.frame.siblings.size()
    }

    pub(crate) fn move_right(&mut self) -> bool {
        if !self.has_right() {
            return false;
        }

        let metrics = self.frame.siblings.metrics(self.frame.sibling_index);
        self.frame.location.length += metrics.length;
        M::add_assign(&mut self.frame.location.metrics, metrics.metrics);
        self.frame.sibling_index += 1;
        true
    }

    pub(crate) fn has_left(&self) -> bool {
        0 < self.frame.sibling_index
    }

    pub(crate) fn move_left(&mut self) -> bool {
        if !self.has_left() {
            return false;
        }

        self.frame.sibling_index -= 1;
        let metrics = self.frame.siblings.metrics(self.frame.sibling_index);
        self.frame.location.length -= metrics.length;
        M::sub_assign(&mut self.frame.location.metrics, metrics.metrics);
        true
    }

    pub(crate) fn move_next(&mut self) -> bool {
        match self.has_right() {
            true => self.move_right(),
            false => match self
                .frames
                .iter()
                .rposition(|frame| (frame.sibling_index as usize) + 1 < frame.siblings.size())
            {
                Some(frame_index) => {
                    while self.frames.len() > frame_index {
                        let moved = self.move_up();
                        debug_assert!(moved);
                    }

                    let moved = self.move_right();
                    debug_assert!(moved);

                    while self.move_down() {}
                    self.is_leaf()
                }
                None => false,
            },
        }
    }

    pub(crate) fn move_next_leaf(&mut self) -> bool {
        let Some(frame_index) = self
            .frames
            .iter()
            .rposition(|frame| (frame.sibling_index as usize) + 1 < frame.siblings.size())
        else {
            return false;
        };

        while self.frames.len() > frame_index {
            let moved = self.move_up();
            debug_assert!(moved);
        }

        let moved = self.move_right();
        debug_assert!(moved);

        while self.move_down() {}
        self.is_leaf()
    }

    pub(crate) fn move_prev_leaf(&mut self) -> bool {
        let Some(frame_index) = self
            .frames
            .iter()
            .rposition(|frame| frame.sibling_index > 0)
        else {
            return false;
        };

        while self.frames.len() > frame_index {
            let moved = self.move_up();
            debug_assert!(moved);
        }

        let moved = self.move_left();
        debug_assert!(moved);

        while self.move_down_last() {}
        self.is_leaf()
    }

    pub(crate) fn move_up(&mut self) -> bool {
        let Some(parent) = self.frames.pop() else {
            let siblings = std::mem::replace(
                &mut self.frame.siblings,
                Siblings::Owned(Node::Leaf(Vec::new())),
            );

            let Siblings::Owned(node) = siblings else {
                self.frame.siblings = siblings;
                return false;
            };

            let node = node.grow_tree().shrink_tree();
            self.frame.location = MetricsWithLength {
                length: 0,
                metrics: M::zero(),
            };
            self.frame.sibling_index = 0;
            self.frame.siblings = Siblings::Shared(Arc::new(node));
            return true;
        };

        let frame = std::mem::replace(&mut self.frame, parent);

        if let Siblings::Owned(child) = frame.siblings {
            let child = child.balance_children();
            let child_metrics = child.metrics();

            match self.frame.siblings.make_owned() {
                Node::Internal(parent) => {
                    let index = self.frame.sibling_index as usize;
                    if index < parent.len() {
                        parent[index] = Child {
                            metrics: child_metrics,
                            node: Arc::new(child),
                        };
                    }
                }
                Node::Leaf(_) => panic!("parent frame cannot be a leaf"),
            }
        }

        true
    }

    pub(crate) fn move_down(&mut self) -> bool {
        let Some(siblings) = self.frame.siblings.children(self.frame.sibling_index) else {
            return false;
        };

        let location = self.frame.location.clone();
        let parent = std::mem::replace(
            &mut self.frame,
            Frame {
                location,
                sibling_index: 0,
                siblings,
            },
        );
        self.frames.push(parent);
        true
    }

    pub(crate) fn move_down_last(&mut self) -> bool {
        let Some(siblings) = self.frame.siblings.children(self.frame.sibling_index) else {
            return false;
        };

        let sibling_index = u32::try_from(siblings.size() - 1).expect("sibling index exceeds u32");
        let mut location = self.frame.location.clone();

        for index in 0..sibling_index {
            let metrics = siblings.metrics(index);
            location.length += metrics.length;
            M::add_assign(&mut location.metrics, metrics.metrics);
        }

        let parent = std::mem::replace(
            &mut self.frame,
            Frame {
                location,
                sibling_index,
                siblings,
            },
        );
        self.frames.push(parent);
        true
    }

    pub(crate) fn is_leaf(&self) -> bool {
        self.frame.siblings.is_leaf()
    }

    pub(crate) fn descend_to_index(&mut self, index: u32) -> bool {
        loop {
            while self.frame.location.length + self.metrics().length <= index && self.move_right() {
            }

            if !self.move_down() {
                break self.metrics().length != 0;
            }
        }
    }

    pub(crate) fn insert(&mut self, rope: Rope<T, M>) {
        match rope.metrics.length {
            0 => {}
            _ => {
                let root = match Arc::try_unwrap(rope.root) {
                    Ok(root) => root,
                    Err(root) => root.as_ref().clone(),
                };
                match root.depth() > self.frames.len() {
                    true => {
                        let host = std::mem::replace(self, Self::new(Arc::new(Node::empty())));
                        let (left, right) = host.split();
                        let insertion_index = left.metrics.length;
                        let mut assembled = Self::new(Arc::new(root));

                        while assembled.move_down() {}
                        if !left.is_empty() {
                            assembled.insert_direct(left.into_node());
                        }

                        while assembled.move_up() {}
                        while assembled.move_right() {}
                        while assembled.move_down_last() {}
                        assembled.frame.sibling_index =
                            u32::try_from(assembled.frame.siblings.size())
                                .expect("sibling index exceeds u32");

                        if !right.is_empty() {
                            assembled.insert_direct(right.into_node());
                        }

                        let rope = assembled.rope();
                        *self = Self::new(rope.root);
                        let positioned = self.descend_to_index(insertion_index);
                        debug_assert!(positioned);
                    }
                    false => self.insert_direct(root),
                }
            }
        }
    }

    pub(crate) fn split(self) -> (Rope<T, M>, Rope<T, M>) {
        let Frame {
            sibling_index,
            siblings,
            ..
        } = self.frame;
        let index = sibling_index as usize;
        let (mut left, mut right) = match siblings.into_node() {
            Node::Leaf(mut elements) => {
                let index = index.min(elements.len());
                let right = elements.split_off(index);
                (
                    Node::Leaf(elements).into_option(),
                    Node::Leaf(right).into_option(),
                )
            }
            Node::Internal(_) => panic!("cursor frame cannot be a branch"),
        };

        for frame in self.frames.into_iter().rev() {
            let Frame {
                sibling_index,
                siblings,
                ..
            } = frame;
            let mut children = match siblings.into_node() {
                Node::Internal(children) => children,
                Node::Leaf(_) => panic!("parent frame cannot be a leaf"),
            };
            let index = (sibling_index as usize).min(children.len());
            let mut right_children = children.split_off(index);
            if !right_children.is_empty() {
                right_children.remove(0);
            }

            match left.take() {
                Some(node) => children.push(Child {
                    metrics: node.metrics(),
                    node: Arc::new(node),
                }),
                None => {}
            }
            match right.take() {
                Some(node) => right_children.insert(
                    0,
                    Child {
                        metrics: node.metrics(),
                        node: Arc::new(node),
                    },
                ),
                None => {}
            }

            left = Node::Internal(children).into_option();
            right = Node::Internal(right_children).into_option();
        }

        (
            left.map(Rope::from_node).unwrap_or_else(Rope::new),
            right.map(Rope::from_node).unwrap_or_else(Rope::new),
        )
    }

    fn insert_direct(&mut self, root: Node<T, M>) {
        let depth = root.depth();

        match depth {
            0 => {
                match root {
                    Node::Leaf(inserted) => match inserted.is_empty() {
                        true => {}
                        false => match self.frame.siblings.make_owned() {
                            Node::Leaf(elements) => {
                                let index = (self.frame.sibling_index as usize).min(elements.len());
                                elements.reserve(inserted.len());
                                elements.splice(index..index, inserted);
                            }
                            Node::Internal(_) => panic!("cannot insert leaf data into branch"),
                        },
                    },
                    Node::Internal(_) => panic!("zero-depth node cannot be internal"),
                }

                let mut child = self.frame.siblings.arc_clone();
                for frame in self.frames.iter_mut().rev() {
                    match frame.siblings.make_owned() {
                        Node::Internal(children) => {
                            let index = (frame.sibling_index as usize).min(children.len());
                            match index < children.len() {
                                true => {
                                    children[index] = Child {
                                        metrics: child.metrics(),
                                        node: Arc::clone(&child),
                                    };
                                }
                                false => {}
                            }
                        }
                        Node::Leaf(_) => panic!("parent frame cannot be a leaf"),
                    }

                    child = frame.siblings.arc_clone();
                }
            }
            _ if depth <= self.frames.len() => {
                let mut inserted = match root {
                    Node::Internal(children) => Some(children),
                    Node::Leaf(_) => panic!("non-zero-depth node cannot be a leaf"),
                };
                let index = self.frame.sibling_index as usize;
                let mut right = match self.frame.siblings.make_owned() {
                    Node::Internal(children) => {
                        let index = index.min(children.len());
                        Node::Internal(children.split_off(index)).into_option()
                    }
                    Node::Leaf(elements) => {
                        let index = index.min(elements.len());
                        let right_elements = elements.split_off(index);
                        Node::Leaf(right_elements).into_option()
                    }
                };
                let mut child = self.frame.siblings.arc_clone();
                let mut level = 1usize;

                for frame in self.frames.iter_mut().rev() {
                    let index = frame.sibling_index as usize;

                    match frame.siblings.make_owned() {
                        Node::Internal(children) => {
                            let index = index.min(children.len());
                            let child_metrics = child.metrics();

                            match index < children.len() {
                                true => {
                                    children[index] = Child {
                                        metrics: child_metrics,
                                        node: Arc::clone(&child),
                                    };
                                }
                                false => {}
                            }

                            match level.cmp(&depth) {
                                std::cmp::Ordering::Less => {
                                    let split_index = index.saturating_add(1).min(children.len());
                                    let mut right_children = children.split_off(split_index);

                                    match right.take() {
                                        Some(right) => {
                                            right_children.insert(
                                                0,
                                                Child {
                                                    metrics: right.metrics(),
                                                    node: Arc::new(right),
                                                },
                                            );
                                        }
                                        None => {}
                                    }

                                    right = Node::Internal(right_children).into_option();
                                }
                                std::cmp::Ordering::Equal => {
                                    let split_index = index.saturating_add(1).min(children.len());
                                    let mut right_children = children.split_off(split_index);

                                    match right.take() {
                                        Some(right) => {
                                            right_children.insert(
                                                0,
                                                Child {
                                                    metrics: right.metrics(),
                                                    node: Arc::new(right),
                                                },
                                            );
                                        }
                                        None => {}
                                    }

                                    let inserted = inserted.take().expect("inserted node");
                                    children.reserve(
                                        inserted.len().saturating_add(right_children.len()),
                                    );
                                    children.extend(inserted);
                                    children.extend(right_children);
                                }
                                std::cmp::Ordering::Greater => {}
                            }
                        }
                        Node::Leaf(_) => panic!("parent frame cannot be a leaf"),
                    }

                    child = frame.siblings.arc_clone();
                    level += 1;
                }
            }
            _ => panic!("inserted rope depth exceeds cursor depth"),
        }
    }

    pub(crate) fn delete(&mut self, mut length: u32) {
        match length {
            0 => {}
            _ => {
                match self.frame.siblings.make_owned() {
                    Node::Internal(_) => panic!("cursor frame cannot be a branch"),
                    Node::Leaf(elements) => {
                        let index = (self.frame.sibling_index as usize).min(elements.len());
                        let deleted = (length as usize).min(elements.len().saturating_sub(index));
                        elements.drain(index..index + deleted);
                        length -= deleted as u32;
                    }
                }

                let mut child = self.frame.siblings.arc_clone();

                for frame in self.frames.iter_mut().rev() {
                    let index = frame.sibling_index as usize;

                    match frame.siblings.make_owned() {
                        Node::Internal(children) => {
                            let index = index.min(children.len());

                            match index < children.len() {
                                true => {
                                    children[index] = Child {
                                        metrics: child.metrics(),
                                        node: Arc::clone(&child),
                                    };
                                }
                                false => {}
                            }

                            let delete_index = index.saturating_add(1).min(children.len());

                            while 0 < length && delete_index < children.len() {
                                match children[delete_index].metrics.length <= length {
                                    true => {
                                        length -= children[delete_index].metrics.length;
                                        children.remove(delete_index);
                                    }
                                    false => {
                                        let deleted =
                                            Arc::make_mut(&mut children[delete_index].node)
                                                .delete_prefix(length);
                                        children[delete_index].metrics =
                                            children[delete_index].node.metrics();
                                        length -= deleted;
                                    }
                                }
                            }
                        }
                        Node::Leaf(_) => panic!("parent frame cannot be a leaf"),
                    }

                    child = frame.siblings.arc_clone();
                }

                if self.is_leaf() {
                    let size = self.frame.siblings.size();
                    if size == 0 {
                        self.frame.sibling_index = 0;
                        self.move_next_leaf();
                    }
                }
            }
        }
    }

    pub(crate) fn rope(mut self) -> Rope<T, M> {
        while self.move_up() {}

        let root = self.frame.siblings.into_node();
        let metrics = root.metrics();

        Rope {
            metrics,
            root: Arc::new(root),
            _measure: PhantomData,
        }
    }
}
