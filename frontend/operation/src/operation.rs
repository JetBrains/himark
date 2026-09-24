// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::cmp::Ordering;
use std::fmt;

use rope::Rope;

use crate::builder::OperationBuilder;
use crate::iter::Iter;
use crate::measure::{OperationMeasure, NEW_LEN, OLD_LEN};
use crate::reader::Reader;
use crate::Op;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bias {
    Left,

    Right,
}

#[derive(Clone)]
pub struct Operation {
    pub(crate) rope: Rope<Op, OperationMeasure>,
}

pub struct OpsFrom {
    pub old_start: u32,
    pub new_start: u32,
    pub ops: Iter,
}

impl Operation {
    pub fn new() -> Self {
        Self { rope: Rope::new() }
    }

    pub fn from_ops(ops: impl IntoIterator<Item = Op>) -> Self {
        let mut builder = OperationBuilder::new();
        for op in ops {
            match op {
                Op::Retain(len) => builder.push_retain(len),
                Op::Insert(text) => builder.push_insert(text),
                Op::Delete(text) => builder.push_delete(text),
            }
        }
        builder.finish()
    }

    pub(crate) fn from_rope(rope: Rope<Op, OperationMeasure>) -> Self {
        Self { rope }
    }

    /// PARTIAL by construction (no tail retain) — for consumers that
    /// complete the coverage themselves against a base they know
    /// (the inlay write-through). Anything headed for a document's
    /// edit door wants [`Operation::insert_in`].
    pub fn insert_at(offset: u32, text: impl Into<String>) -> Self {
        Self::op_at(offset, Op::Insert(text.into()))
    }

    /// PARTIAL by construction — see [`Operation::insert_at`].
    pub fn delete_at(offset: u32, text: impl Into<String>) -> Self {
        Self::op_at(offset, Op::Delete(text.into()))
    }

    /// The EXACT splice over an `old_len`-byte base: retain to
    /// `offset`, insert, retain the rest. Every operation reaching an
    /// edit door must cover its base exactly; these constructors
    /// cannot build anything less.
    pub fn insert_in(old_len: u32, offset: u32, text: impl Into<String>) -> Self {
        Self::op_in(old_len, offset, Op::Insert(text.into()))
    }

    /// The exact deletion over an `old_len`-byte base — see
    /// [`Operation::insert_in`].
    pub fn delete_in(old_len: u32, offset: u32, text: impl Into<String>) -> Self {
        Self::op_in(old_len, offset, Op::Delete(text.into()))
    }

    fn op_at(offset: u32, op: Op) -> Self {
        let mut ops = Vec::with_capacity(2);
        if offset != 0 {
            ops.push(Op::Retain(offset));
        }
        ops.push(op);
        Self::from_ops(ops)
    }

    fn op_in(old_len: u32, offset: u32, op: Op) -> Self {
        let offset = offset.min(old_len);
        let consumed = match &op {
            Op::Delete(text) => offset + text.len().min(u32::MAX as usize) as u32,
            _ => offset,
        };
        assert!(
            consumed <= old_len,
            "the splice must fit its base: {consumed} past {old_len}",
        );
        let mut ops = Vec::with_capacity(3);
        if offset != 0 {
            ops.push(Op::Retain(offset));
        }
        ops.push(op);
        if old_len > consumed {
            ops.push(Op::Retain(old_len - consumed));
        }
        Self::from_ops(ops)
    }

    pub fn transform_offset(&self, offset: u32, bias: Bias) -> u32 {
        self.map_offset(offset, bias, OLD_LEN, NEW_LEN)
    }

    pub fn invert(&self) -> Self {
        let mut builder = OperationBuilder::new();
        for op in self.iter() {
            match op {
                Op::Retain(len) => builder.push_retain(len),
                Op::Insert(text) => builder.push_delete(text),
                Op::Delete(text) => builder.push_insert(text),
            }
        }
        builder.finish()
    }

    pub fn transform_offset_back(&self, offset: u32, bias: Bias) -> u32 {
        self.map_offset(offset, bias, NEW_LEN, OLD_LEN)
    }

    fn map_offset(&self, offset: u32, bias: Bias, source: usize, target: usize) -> u32 {
        if self.rope.is_empty() {
            return offset;
        }

        let totals = self.rope.metrics();
        let source_total = totals.metric_at(rope::MetricId(source));
        if offset > source_total {
            let target_total = totals.metric_at(rope::MetricId(target));
            return offset
                .saturating_sub(source_total)
                .saturating_add(target_total);
        }
        let mut cursor = self.rope.cursor();

        if !cursor.seek(rope::MetricId(source), offset, rope::SeekMode::After) {
            let target_total = totals.metric_at(rope::MetricId(target));
            return offset
                .saturating_sub(source_total)
                .saturating_add(target_total);
        }
        loop {
            let position = cursor.position();
            let source_before = position.metric_at(rope::MetricId(source));
            let target_before = position.metric_at(rope::MetricId(target));
            let element = cursor.element();
            let source_len = match source {
                OLD_LEN => element.old_len(),
                _ => element.new_len(),
            };
            let target_len = match target {
                OLD_LEN => element.old_len(),
                _ => element.new_len(),
            };
            if source_len == 0 {
                if matches!(bias, Bias::Left) {
                    return target_before;
                }
                if !cursor.advance() {
                    return target_before.saturating_add(target_len);
                }
                continue;
            }
            let within = offset - source_before.min(offset);
            if within == source_len && matches!(bias, Bias::Right) {
                if cursor.advance() {
                    continue;
                }
            }
            return match target_len {
                0 => target_before,
                _ => target_before.saturating_add(within.min(target_len)),
            };
        }
    }

    pub fn next_retained_old(&self, offset: u32) -> Option<std::ops::Range<u32>> {
        self.next_retained(offset, OLD_LEN)
    }

    pub fn next_retained_new(&self, offset: u32) -> Option<std::ops::Range<u32>> {
        self.next_retained(offset, NEW_LEN)
    }

    fn next_retained(&self, offset: u32, axis: usize) -> Option<std::ops::Range<u32>> {
        if self.rope.is_empty() {
            return None;
        }
        let total = self.rope.metrics().metric_at(rope::MetricId(axis));
        if offset >= total {
            return None;
        }
        let mut cursor = self.rope.cursor();
        if !cursor.seek(rope::MetricId(axis), offset, rope::SeekMode::After) {
            return None;
        }
        loop {
            let before = cursor.position().metric_at(rope::MetricId(axis));
            let element = cursor.element();
            let len = match axis {
                OLD_LEN => element.old_len(),
                _ => element.new_len(),
            };
            if matches!(element, Op::Retain(_)) && before.saturating_add(len) > offset {
                return Some(before..before.saturating_add(len));
            }
            if !cursor.advance() {
                return None;
            }
        }
    }

    pub fn ops_from_old(&self, offset: u32) -> OpsFrom {
        self.ops_from(offset, OLD_LEN)
    }

    pub fn ops_from_new(&self, offset: u32) -> OpsFrom {
        self.ops_from(offset, NEW_LEN)
    }

    fn ops_from(&self, offset: u32, axis: usize) -> OpsFrom {
        let empty = |old_start: u32, new_start: u32| OpsFrom {
            old_start,
            new_start,
            ops: Iter {
                iter: Rope::new().iter(),
            },
        };
        if self.rope.is_empty() {
            return empty(0, 0);
        }
        let (old_total, new_total) = (self.old_len(), self.new_len());
        let total = match axis {
            OLD_LEN => old_total,
            _ => new_total,
        };
        if offset >= total {
            return empty(old_total, new_total);
        }
        let mut cursor = self.rope.cursor();
        if !cursor.seek(rope::MetricId(axis), offset, rope::SeekMode::After) {
            return empty(old_total, new_total);
        }
        let position = cursor.position();
        OpsFrom {
            old_start: position.metric_at(rope::MetricId(OLD_LEN)),
            new_start: position.metric_at(rope::MetricId(NEW_LEN)),
            ops: Iter {
                iter: cursor.iter(),
            },
        }
    }

    pub fn retain(len: u32) -> Self {
        Self::from_ops([Op::Retain(len)])
    }

    pub fn insert(text: impl Into<String>) -> Self {
        Self::from_ops([Op::Insert(text.into())])
    }

    pub fn delete(text: impl Into<String>) -> Self {
        Self::from_ops([Op::Delete(text.into())])
    }

    pub fn is_empty(&self) -> bool {
        self.old_len() == 0 && self.new_len() == 0
    }

    pub fn size(&self) -> usize {
        self.iter().count()
    }

    pub fn old_len(&self) -> u32 {
        self.rope.metrics().0[OLD_LEN]
    }

    pub fn new_len(&self) -> u32 {
        self.rope.metrics().0[NEW_LEN]
    }

    pub fn iter(&self) -> Iter {
        Iter {
            iter: self.rope.iter(),
        }
    }

    pub fn splice_compose(&self, subsequent: &Self) -> Self {
        self.try_splice_compose(subsequent)
            .unwrap_or_else(|| self.compose(subsequent))
    }

    pub fn splice_compose_into(&self, subsequent: &Self) -> Self {
        self.try_splice_compose_into(subsequent)
            .unwrap_or_else(|| self.compose(subsequent))
    }

    fn try_splice_compose(&self, subsequent: &Self) -> Option<Self> {
        if self.is_empty() || subsequent.is_empty() {
            return None;
        }
        assert_eq!(
            self.new_len(),
            subsequent.old_len(),
            "cannot compose operations with incompatible lengths",
        );
        let margins = Margins::of(subsequent)?;

        let cut = cut_rope(&self.rope, NEW_LEN, margins.start, margins.end_old)?;
        let middle_self = Operation::from_ops(cut.middle.iter().cloned());
        let middle_sub = Operation::from_ops(margins.middle.iter().cloned());
        let composed = middle_self.compose(&middle_sub);
        Some(splice_rope(&self.rope, &cut, composed))
    }

    fn try_splice_compose_into(&self, subsequent: &Self) -> Option<Self> {
        if self.is_empty() || subsequent.is_empty() {
            return None;
        }
        assert_eq!(
            self.new_len(),
            subsequent.old_len(),
            "cannot compose operations with incompatible lengths",
        );
        let margins = Margins::of(self)?;

        let cut = cut_rope(&subsequent.rope, OLD_LEN, margins.start, margins.end_new)?;
        let middle_self = Operation::from_ops(margins.middle.iter().cloned());
        let middle_sub = Operation::from_ops(cut.middle.iter().cloned());
        let composed = middle_self.compose(&middle_sub);
        Some(splice_rope(&subsequent.rope, &cut, composed))
    }

    pub fn compose(&self, subsequent: &Self) -> Self {
        match (self.is_empty(), subsequent.is_empty()) {
            (true, _) => subsequent.clone(),
            (_, true) => self.clone(),
            (false, false) => {
                assert_eq!(
                    self.new_len(),
                    subsequent.old_len(),
                    "cannot compose operations with incompatible lengths",
                );

                let mut left = Reader::new(self);
                let mut right = Reader::new(subsequent);
                let mut out = OperationBuilder::new();

                while !left.is_done() || !right.is_done() {
                    match (
                        !right.is_done() && matches!(right.current(), Op::Insert(_)),
                        !left.is_done() && matches!(left.current(), Op::Delete(_)),
                    ) {
                        (true, _) => match right.take_new_piece(right.remaining_new_len()) {
                            Op::Insert(text) => out.push_insert(text),
                            _ => unreachable!("insert branch must yield an insert"),
                        },
                        (false, true) => match left.take_old_piece(left.remaining_old_len()) {
                            Op::Delete(text) => out.push_delete(text),
                            _ => unreachable!("delete branch must yield a delete"),
                        },
                        (false, false) => match (left.is_done(), right.is_done()) {
                            (true, true) => break,
                            (true, false) => {
                                panic!(
                                    "subsequent operation consumed more input than compose allows"
                                )
                            }
                            (false, true) => {
                                panic!(
                                    "subsequent operation ended before intermediate text was consumed"
                                )
                            }
                            (false, false) => match (left.current(), right.current()) {
                                (Op::Insert(_), Op::Retain(_)) => {
                                    let len =
                                        left.remaining_new_len().min(right.remaining_old_len());
                                    match left.take_new_piece(len) {
                                        Op::Insert(text) => out.push_insert(text),
                                        _ => unreachable!("insert branch must yield an insert"),
                                    }
                                    right.consume_old(len);
                                }
                                (Op::Insert(_), Op::Delete(_)) => {
                                    let len =
                                        left.remaining_new_len().min(right.remaining_old_len());
                                    let deleted = match left.take_new_piece(len) {
                                        Op::Insert(text) => text,
                                        _ => unreachable!("insert branch must yield an insert"),
                                    };
                                    let expected = match right.take_old_piece(len) {
                                        Op::Delete(text) => text,
                                        _ => unreachable!("delete branch must yield a delete"),
                                    };
                                    assert_eq!(
                                        deleted, expected,
                                        "delete text must match inserted text during compose",
                                    );
                                }
                                (Op::Retain(_), Op::Retain(_)) => {
                                    let len =
                                        left.remaining_old_len().min(right.remaining_old_len());
                                    left.consume_old(len);
                                    right.consume_old(len);
                                    out.push_retain(len);
                                }
                                (Op::Retain(_), Op::Delete(_)) => {
                                    let len =
                                        left.remaining_old_len().min(right.remaining_old_len());
                                    left.consume_old(len);
                                    match right.take_old_piece(len) {
                                        Op::Delete(text) => out.push_delete(text),
                                        _ => unreachable!("delete branch must yield a delete"),
                                    }
                                }
                                (Op::Insert(_), Op::Insert(_))
                                | (Op::Retain(_), Op::Insert(_))
                                | (Op::Delete(_), Op::Insert(_))
                                | (Op::Delete(_), Op::Retain(_))
                                | (Op::Delete(_), Op::Delete(_)) => {
                                    panic!("unexpected compose state")
                                }
                            },
                        },
                    }
                }

                out.finish()
            }
        }
    }

    pub fn transform(&self, other: &Self) -> Self {
        match (self.is_empty(), other.is_empty()) {
            (true, _) | (_, true) => self.clone(),
            (false, false) => {
                assert_eq!(
                    self.old_len(),
                    other.old_len(),
                    "cannot transform operations with incompatible source lengths",
                );

                let mut left = Reader::new(self);
                let mut right = Reader::new(other);
                let mut out = OperationBuilder::new();

                while !left.is_done() || !right.is_done() {
                    match (
                        !left.is_done()
                            && !right.is_done()
                            && matches!(left.current(), Op::Insert(_))
                            && matches!(right.current(), Op::Insert(_)),
                        !right.is_done() && matches!(right.current(), Op::Insert(_)),
                        !left.is_done() && matches!(left.current(), Op::Insert(_)),
                    ) {
                        (true, _, _) => {
                            let left_text = left
                                .remaining_insert_text()
                                .expect("insert branch must expose remaining text");
                            let right_text = right
                                .remaining_insert_text()
                                .expect("insert branch must expose remaining text");
                            match left_text.cmp(&right_text) {
                                Ordering::Less | Ordering::Equal => {
                                    let len = left.remaining_new_len();
                                    out.push_insert(left_text);
                                    left.consume_new(len);
                                }
                                Ordering::Greater => {
                                    let len = right.remaining_new_len();
                                    out.push_retain(len);
                                    right.consume_new(len);
                                }
                            }
                        }
                        (false, true, _) => {
                            let len = right.remaining_new_len();
                            right.consume_new(len);
                            out.push_retain(len);
                        }
                        (false, false, true) => match left.take_new_piece(left.remaining_new_len())
                        {
                            Op::Insert(text) => out.push_insert(text),
                            _ => unreachable!("insert branch must yield an insert"),
                        },
                        (false, false, false) => match (left.is_done(), right.is_done()) {
                            (true, true) => break,
                            (true, false) => {
                                panic!(
                                    "other operation consumed more base input than transform allows"
                                )
                            }
                            (false, true) => {
                                panic!("other operation ended before base input was consumed")
                            }
                            (false, false) => {
                                let len = left.remaining_old_len().min(right.remaining_old_len());
                                match (left.current(), right.current()) {
                                    (Op::Retain(_), Op::Retain(_)) => {
                                        left.consume_old(len);
                                        right.consume_old(len);
                                        out.push_retain(len);
                                    }
                                    (Op::Delete(_), Op::Delete(_)) => {
                                        let left_text = match left.take_old_piece(len) {
                                            Op::Delete(text) => text,
                                            _ => unreachable!("delete branch must yield a delete"),
                                        };
                                        let right_text = match right.take_old_piece(len) {
                                            Op::Delete(text) => text,
                                            _ => unreachable!("delete branch must yield a delete"),
                                        };
                                        assert_eq!(
                                            left_text, right_text,
                                            "both deletes must target the same base text",
                                        );
                                    }
                                    (Op::Delete(_), Op::Retain(_)) => {
                                        right.consume_old(len);
                                        match left.take_old_piece(len) {
                                            Op::Delete(text) => out.push_delete(text),
                                            _ => unreachable!("delete branch must yield a delete"),
                                        }
                                    }
                                    (Op::Retain(_), Op::Delete(_)) => {
                                        left.consume_old(len);
                                        right.consume_old(len);
                                    }
                                    (Op::Insert(_), _) | (_, Op::Insert(_)) => {
                                        panic!(
                                            "insert handling must have been consumed before base processing"
                                        )
                                    }
                                }
                            }
                        },
                    }
                }

                out.finish()
            }
        }
    }
}

impl Default for Operation {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for Operation {
    fn eq(&self, other: &Self) -> bool {
        self.size() == other.size() && self.iter().eq(other.iter())
    }
}

impl Eq for Operation {}

impl fmt::Debug for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Operation")
            .field(&self.iter().collect::<Vec<_>>())
            .finish()
    }
}

impl FromIterator<Op> for Operation {
    fn from_iter<T: IntoIterator<Item = Op>>(iter: T) -> Self {
        Self::from_ops(iter)
    }
}

impl IntoIterator for &Operation {
    type Item = Op;
    type IntoIter = Iter;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod ops_from_equivalence {
    use super::*;

    #[test]
    fn seeded_iteration_matches_the_full_walk() {
        let mut rng = 0xD1B54A32D192ED03u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for _ in 0..200 {
            let mut ops = Vec::new();
            for _ in 0..(1 + next() % 10) {
                match next() % 3 {
                    0 => ops.push(Op::Retain(1 + (next() % 9) as u32)),
                    1 => ops.push(Op::Insert("i".repeat(1 + (next() % 5) as usize))),
                    _ => ops.push(Op::Delete("d".repeat(1 + (next() % 5) as usize))),
                }
            }
            let operation = Operation::from_ops(ops);

            let full: Vec<(u32, u32, Op)> = {
                let (mut old, mut new) = (0u32, 0u32);
                operation
                    .iter()
                    .map(|op| {
                        let at = (old, new, op.clone());
                        old += op.old_len();
                        new += op.new_len();
                        at
                    })
                    .collect()
            };
            for axis_new in [false, true] {
                let total = match axis_new {
                    false => operation.old_len(),
                    true => operation.new_len(),
                };
                for offset in 0..=total.saturating_add(2) {
                    let from = match axis_new {
                        false => operation.ops_from_old(offset),
                        true => operation.ops_from_new(offset),
                    };
                    let seeded: Vec<Op> = from.ops.collect();
                    if offset >= total {
                        assert!(seeded.is_empty(), "past-the-end must yield nothing");
                        continue;
                    }

                    let at = full
                        .iter()
                        .position(|(old, new, _)| (*old, *new) == (from.old_start, from.new_start))
                        .unwrap_or_else(|| {
                            panic!(
                                "seed ({}, {}) is not an op start of {:?}",
                                from.old_start,
                                from.new_start,
                                operation.iter().collect::<Vec<_>>()
                            )
                        });
                    let suffix: Vec<Op> = full[at..].iter().map(|(_, _, op)| op.clone()).collect();
                    assert_eq!(seeded, suffix, "suffix mismatch at offset {offset}");

                    let seed = match axis_new {
                        false => from.old_start,
                        true => from.new_start,
                    };
                    assert!(
                        seed <= offset,
                        "seed {seed} past the sought offset {offset}"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod transform_equivalence {
    use super::*;

    fn linear_reference(operation: &Operation, offset: u32, bias: Bias) -> u32 {
        let mut old_offset = 0u32;
        let mut transformed = offset;
        for op in operation.iter() {
            match op {
                Op::Retain(len) => old_offset = old_offset.saturating_add(len),
                Op::Insert(text) => {
                    let shifts = match bias {
                        Bias::Left => old_offset < offset,
                        Bias::Right => old_offset <= offset,
                    };
                    if shifts {
                        transformed =
                            transformed.saturating_add(text.len().min(u32::MAX as usize) as u32);
                    }
                }
                Op::Delete(text) => {
                    let len = text.len().min(u32::MAX as usize) as u32;
                    if old_offset.saturating_add(len) <= offset {
                        transformed = transformed.saturating_sub(len);
                    } else if old_offset < offset {
                        transformed = transformed.saturating_sub(offset - old_offset);
                    }
                    old_offset = old_offset.saturating_add(len);
                }
            }
            let past = match bias {
                Bias::Left => old_offset >= offset,
                Bias::Right => old_offset > offset,
            };
            if past {
                break;
            }
        }
        transformed
    }

    #[test]
    fn seek_mapping_matches_the_linear_reference() {
        let mut rng = 0x9E3779B97F4A7C15u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for _ in 0..400 {
            let mut ops = Vec::new();
            for _ in 0..(1 + next() % 12) {
                match next() % 3 {
                    0 => ops.push(Op::Retain(1 + (next() % 9) as u32)),
                    1 => ops.push(Op::Insert("i".repeat(1 + (next() % 5) as usize))),
                    _ => ops.push(Op::Delete("d".repeat(1 + (next() % 5) as usize))),
                }
            }
            let operation = Operation::from_ops(ops);
            let old_len = operation.old_len();
            for offset in 0..=old_len.saturating_add(3) {
                for bias in [Bias::Left, Bias::Right] {
                    assert_eq!(
                        operation.transform_offset(offset, bias),
                        linear_reference(&operation, offset, bias),
                        "offset {offset} bias {bias:?} over {:?}",
                        operation.iter().collect::<Vec<_>>(),
                    );
                }
            }
        }
    }
}

struct Margins {
    start: u32,

    end_old: u32,

    end_new: u32,
    middle: Vec<Op>,
}

impl Margins {
    fn of(operation: &Operation) -> Option<Margins> {
        let ops: Vec<Op> = operation.iter().collect();
        let mut first = 0usize;
        let mut start = 0u32;
        while let Some(Op::Retain(len)) = ops.get(first) {
            start += len;
            first += 1;
        }
        let mut last = ops.len();
        let mut trailing = 0u32;
        while last > first {
            let Some(Op::Retain(len)) = ops.get(last - 1) else {
                break;
            };
            trailing += len;
            last -= 1;
        }

        if start == 0 && trailing == 0 {
            return None;
        }
        Some(Margins {
            start,
            end_old: operation.old_len() - trailing,
            end_new: operation.new_len() - trailing,
            middle: ops[first..last].to_vec(),
        })
    }
}

struct RopeCut {
    from: u32,

    removed: u32,

    pre: Option<Op>,

    middle: Vec<Op>,

    post: Option<Op>,
}

fn axis_width(op: &Op, axis: usize) -> u32 {
    match axis {
        OLD_LEN => op.old_len(),
        _ => op.new_len(),
    }
}

fn split_op(op: &Op, at: u32, axis: usize) -> Option<(Op, Op)> {
    let split_text = |text: &str| -> Option<(String, String)> {
        let at = at as usize;
        text.is_char_boundary(at)
            .then(|| (text[..at].to_owned(), text[at..].to_owned()))
    };
    match op {
        Op::Retain(len) => Some((Op::Retain(at), Op::Retain(len - at))),
        Op::Insert(text) if axis == NEW_LEN => {
            split_text(text).map(|(a, b)| (Op::Insert(a), Op::Insert(b)))
        }
        Op::Delete(text) if axis == OLD_LEN => {
            split_text(text).map(|(a, b)| (Op::Delete(a), Op::Delete(b)))
        }

        _ => None,
    }
}

fn cut_rope(
    rope: &Rope<Op, OperationMeasure>,
    axis: usize,
    start: u32,
    end: u32,
) -> Option<RopeCut> {
    let total = rope.metrics().0[axis];
    let start = start.min(total);
    let end = end.clamp(start, total);
    let mut cursor = rope.cursor();
    let (from, mut at) = match cursor.seek(rope::MetricId(axis), start, rope::SeekMode::After) {
        true => (cursor.index(), cursor.position().0[axis]),

        false => (rope.len() as u32, total),
    };
    let mut pre = None;
    let mut middle = Vec::new();
    let mut post = None;
    let mut removed = 0u32;
    let mut index = from;
    let len = rope.len() as u32;
    while index < len {
        let op = cursor.peek_element()?.clone();
        let width = axis_width(&op, axis);
        if at >= end {
            break;
        }
        if at < start {
            let (left, right) = split_op(&op, start - at, axis)?;
            pre = Some(left);
            removed += 1;
            index += 1;
            if at + width > end {
                let (mid, tail) = split_op(&right, end - start, axis)?;
                middle.push(mid);
                post = Some(tail);
                break;
            }
            middle.push(right);
            at += width;
            cursor.advance();
            continue;
        }
        if at + width > end {
            let (mid, tail) = split_op(&op, end - at, axis)?;
            middle.push(mid);
            post = Some(tail);
            removed += 1;
            break;
        }
        middle.push(op);
        removed += 1;
        index += 1;
        at += width;
        cursor.advance();
    }
    Some(RopeCut {
        from,
        removed,
        pre,
        middle,
        post,
    })
}

fn splice_rope(rope: &Rope<Op, OperationMeasure>, cut: &RopeCut, composed: Operation) -> Operation {
    let mut replacement: Vec<Op> = Vec::new();
    replacement.extend(cut.pre.iter().cloned());
    replacement.extend(composed.iter());
    replacement.extend(cut.post.iter().cloned());
    let mut cursor = rope.cursor();
    if !cursor.seek_to_index(cut.from) {
        let rope = Rope::from_iter(rope.iter().chain(replacement));
        return Operation::from_rope(rope);
    }
    cursor.delete(cut.removed);
    cursor.insert(Rope::from_iter(replacement));
    Operation::from_rope(cursor.rope())
}
