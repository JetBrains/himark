// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use operation::{Bias, Op, Operation};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EditIdentity(u64);

impl EditIdentity {
    pub fn mint() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static MINT: AtomicU64 = AtomicU64::new(1);
        Self(MINT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone)]
pub struct EditLog {
    operations: rpds::VectorSync<Operation>,

    identities: rpds::VectorSync<EditIdentity>,
}

impl std::fmt::Debug for EditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "EditLog(revision {}, head {:?})",
            self.revision(),
            self.head()
        )
    }
}

impl Default for EditLog {
    fn default() -> Self {
        Self::new()
    }
}

impl EditLog {
    pub fn new() -> Self {
        Self {
            operations: rpds::VectorSync::new_sync(),
            identities: rpds::VectorSync::new_sync(),
        }
    }

    pub fn revision(&self) -> u64 {
        self.operations.len() as u64
    }

    pub fn record(&mut self, operation: &Operation, old_len: u32) -> EditIdentity {
        self.record_as(EditIdentity::mint(), operation, old_len)
    }

    pub fn record_as(
        &mut self,
        identity: EditIdentity,
        operation: &Operation,
        old_len: u32,
    ) -> EditIdentity {
        // The edit door admits only exact-coverage operations — a
        // mismatch here is a bug upstream, never padded over.
        debug_assert_eq!(operation.old_len(), old_len);
        self.operations.push_back_mut(operation.clone());
        self.identities.push_back_mut(identity);
        identity
    }

    pub fn identity_at(&self, index: usize) -> Option<EditIdentity> {
        self.identities.get(index).copied()
    }

    pub fn head(&self) -> Option<EditIdentity> {
        self.identity_at(self.operations.len().checked_sub(1)?)
    }

    pub fn as_of(&self, revision: u64) -> Self {
        let mut log = self.clone();
        while log.operations.len() > revision as usize {
            log.operations.drop_last_mut();
            log.identities.drop_last_mut();
        }
        log
    }

    fn composed(&self, from: usize) -> Option<Operation> {
        let mut composed: Option<Operation> = None;
        for index in from..self.operations.len() {
            let Some(operation) = self.operations.get(index) else {
                continue;
            };
            composed = Some(match composed {
                Some(before) => before.compose(operation),
                None => operation.clone(),
            });
        }
        composed
    }

    pub fn since(&self, revision: u64) -> impl Iterator<Item = &Operation> {
        let from = revision.min(self.revision()) as usize;
        (from..self.operations.len()).filter_map(|index| self.operations.get(index))
    }

    pub fn entries_since(&self, revision: u64) -> impl Iterator<Item = (EditIdentity, &Operation)> {
        let from = revision.min(self.revision()) as usize;
        (from..self.operations.len())
            .filter_map(|index| Some((*self.identities.get(index)?, self.operations.get(index)?)))
    }

    pub fn compose_since(&self, revision: u64) -> Option<Operation> {
        let mut composed: Option<Operation> = None;
        for operation in self.since(revision) {
            composed = Some(match composed {
                Some(composed) => composed.compose(operation),
                None => operation.clone(),
            });
        }
        composed
    }

    pub fn ranges_since(&self, revision: u64) -> Vec<Range<u32>> {
        let mut ranges: Vec<Range<u32>> = Vec::new();
        for operation in self.since(revision) {
            for range in &mut ranges {
                *range = Self::transform_range(range.clone(), operation);
            }
            push_operation_ranges(&mut ranges, operation);
        }
        coalesce_ranges(&mut ranges);
        ranges
    }

    pub fn transform_range(range: Range<u32>, operation: &Operation) -> Range<u32> {
        operation.transform_offset(range.start, Bias::Left)
            ..operation.transform_offset(range.end, Bias::Right)
    }

    pub fn coalesce(ranges: &mut Vec<Range<u32>>) {
        coalesce_ranges(ranges);
    }
}

fn push_operation_ranges(edited: &mut Vec<Range<u32>>, operation: &Operation) {
    let mut offset = 0u32;
    for op in operation.iter() {
        match op {
            Op::Retain(len) => offset = offset.saturating_add(len),
            Op::Insert(text) => {
                let len = text.len().min(u32::MAX as usize) as u32;
                edited.push(offset..offset.saturating_add(len));
                offset = offset.saturating_add(len);
            }
            Op::Delete(_) => edited.push(offset..offset),
        }
    }
}

fn coalesce_ranges(ranges: &mut Vec<Range<u32>>) {
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<u32>> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    *ranges = merged;
}

pub fn common_base(before: &EditLog, after: &EditLog) -> Option<(usize, usize)> {
    let mut left = before.revision() as isize - 1;
    let mut right = after.revision() as isize - 1;
    while left >= 0 && right >= 0 {
        if before.identity_at(left as usize) == after.identity_at(right as usize) {
            return Some((left as usize, right as usize));
        }

        if left < right && right > 0 {
            right -= 1;
        } else {
            left -= 1;
        }
    }
    None
}

pub fn bridge(from: &EditLog, to: &EditLog) -> Option<Operation> {
    let (from_at, to_at) = match common_base(from, to) {
        Some((left, right)) => (left + 1, right + 1),
        None => (0, 0),
    };
    match (from.composed(from_at), to.composed(to_at)) {
        (None, None) => Some(Operation::default()),
        (None, Some(after)) => Some(after),
        (Some(before), None) => Some(before.invert()),
        (Some(before), Some(after)) => {
            let undo = before.invert();
            (undo.new_len() == after.old_len()).then(|| undo.compose(&after))
        }
    }
}

#[cfg(test)]
mod tests;
