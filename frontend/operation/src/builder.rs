use rope::from_leaves_with_branching;

use crate::measure::{BRANCH_FACTOR, LEAF_CAPACITY};
use crate::op::Op;
use crate::Operation;

#[derive(Clone)]
pub struct OperationBuilder {
    leaves: Vec<Vec<Op>>,
    current_leaf: Vec<Op>,
}

impl OperationBuilder {
    pub fn new() -> Self {
        Self {
            leaves: Vec::new(),
            current_leaf: Vec::new(),
        }
    }

    pub fn push_retain(&mut self, len: u32) {
        match len {
            0 => {}
            _ => match self.current_leaf.last_mut() {
                Some(Op::Retain(existing)) => *existing += len,
                _ => self.push_raw(Op::Retain(len)),
            },
        }
    }

    pub fn push_insert(&mut self, text: String) {
        match text.is_empty() {
            true => {}
            false => match self.current_leaf.last_mut() {
                Some(Op::Insert(existing)) => existing.push_str(&text),
                _ => self.push_raw(Op::Insert(text)),
            },
        }
    }

    pub fn push_delete(&mut self, text: String) {
        match text.is_empty() {
            true => {}
            false => match self.current_leaf.last_mut() {
                Some(Op::Delete(existing)) => existing.push_str(&text),
                _ => self.push_raw(Op::Delete(text)),
            },
        }
    }

    pub fn finish(mut self) -> Operation {
        if !self.current_leaf.is_empty() {
            self.leaves.push(self.current_leaf);
        }

        Operation::from_rope(from_leaves_with_branching(
            self.leaves,
            LEAF_CAPACITY,
            BRANCH_FACTOR,
        ))
    }

    fn push_raw(&mut self, op: Op) {
        self.current_leaf.push(op);
        if self.current_leaf.len() >= LEAF_CAPACITY {
            self.leaves.push(std::mem::take(&mut self.current_leaf));
        }
    }
}
