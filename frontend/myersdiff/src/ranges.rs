// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The shared currency of the passes: a matched or changed RANGE
//! across two sequences, lists of them, and the chunk OPTIMIZER
//! that slides a boundary between two touching matches to where a
//! human would put it (IntelliJ's `ChunkOptimizer`, ported).

use similar::{Algorithm, DiffOp};
use std::hash::Hash;

/// `[start1, end1)` on the first sequence aligned with `[start2,
/// end2)` on the second — an unchanged pair, or a change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Range {
    pub start1: usize,
    pub end1: usize,
    pub start2: usize,
    pub end2: usize,
}

impl Range {
    pub fn new(start1: usize, end1: usize, start2: usize, end2: usize) -> Self {
        Self {
            start1,
            end1,
            start2,
            end2,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.start1 == self.end1 && self.start2 == self.end2
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Side {
    First,
    Second,
}

impl Side {
    pub fn pick<T>(self, first: T, second: T) -> T {
        match self {
            Side::First => first,
            Side::Second => second,
        }
    }

    pub fn other(self) -> Side {
        match self {
            Side::First => Side::Second,
            Side::Second => Side::First,
        }
    }
}

/// The LCS of two sequences as the UNCHANGED ranges, in order.
pub(crate) fn unchanged<T: Eq + Hash>(
    old: &[T],
    new: &[T],
    deadline: Option<std::time::Instant>,
) -> Vec<Range> {
    let ops = similar::capture_diff_slices_deadline(Algorithm::Myers, old, new, deadline);
    let mut ranges = Vec::new();
    for op in ops {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            push_unchanged(
                &mut ranges,
                Range::new(old_index, old_index + len, new_index, new_index + len),
            );
        }
    }
    ranges
}

/// The LCS in two steps: over the IMPORTANT elements first, so an
/// unimportant one (a brace line, a newline) never anchors a match
/// on its own, then every gap between those matches over all its
/// elements (IntelliJ's `compareSmart` + `SmartLineChangeCorrector`).
pub(crate) fn unchanged_two_step<T: Eq + Hash>(
    data1: &[T],
    data2: &[T],
    important: impl Fn(&T) -> bool,
    deadline: Option<std::time::Instant>,
) -> Vec<Range> {
    fn pick<'d, T>(data: &'d [T], important: &impl Fn(&T) -> bool) -> (Vec<&'d T>, Vec<usize>) {
        data.iter()
            .enumerate()
            .filter(|(_, item)| important(item))
            .map(|(index, item)| (item, index))
            .unzip()
    }
    let (big1, indexes1) = pick(data1, &important);
    let (big2, indexes2) = pick(data2, &important);
    let big_matched = unchanged(&big1, &big2, deadline);

    let mut equal = Vec::new();
    let (mut last1, mut last2) = (0, 0);
    let match_gap =
        |equal: &mut Vec<Range>, start1: usize, end1: usize, start2: usize, end2: usize| {
            let forward = expand_forward(data1, data2, start1, start2, end1, end2);
            let backward =
                expand_backward(data1, data2, start1 + forward, start2 + forward, end1, end2);
            let inner1 = start1 + forward..end1 - backward;
            let inner2 = start2 + forward..end2 - backward;
            push_unchanged(
                equal,
                Range::new(start1, inner1.start, start2, inner2.start),
            );
            for range in unchanged(&data1[inner1.clone()], &data2[inner2.clone()], deadline) {
                push_unchanged(
                    equal,
                    Range::new(
                        inner1.start + range.start1,
                        inner1.start + range.end1,
                        inner2.start + range.start2,
                        inner2.start + range.end2,
                    ),
                );
            }
            push_unchanged(equal, Range::new(inner1.end, end1, inner2.end, end2));
        };
    for range in big_matched {
        for i in 0..range.end1 - range.start1 {
            let index1 = indexes1[range.start1 + i];
            let index2 = indexes2[range.start2 + i];
            match_gap(&mut equal, last1, index1, last2, index2);
            push_unchanged(
                &mut equal,
                Range::new(index1, index1 + 1, index2, index2 + 1),
            );
            last1 = index1 + 1;
            last2 = index2 + 1;
        }
    }
    match_gap(&mut equal, last1, data1.len(), last2, data2.len());
    equal
}

/// Append an unchanged range, merging it into the last when they
/// touch on both sides.
pub(crate) fn push_unchanged(ranges: &mut Vec<Range>, range: Range) {
    if range.end1 == range.start1 {
        return;
    }
    if let Some(last) = ranges.last_mut() {
        if last.end1 == range.start1 && last.end2 == range.start2 {
            last.end1 = range.end1;
            last.end2 = range.end2;
            return;
        }
    }
    ranges.push(range);
}

/// The CHANGES between unchanged ranges: every gap, the ends
/// included, empty gaps left out.
pub(crate) fn changes_between(unchanged: &[Range], len1: usize, len2: usize) -> Vec<Range> {
    let mut changes = Vec::new();
    let (mut at1, mut at2) = (0, 0);
    for range in unchanged {
        let gap = Range::new(at1, range.start1, at2, range.start2);
        if !gap.is_empty() {
            changes.push(gap);
        }
        at1 = range.end1;
        at2 = range.end2;
    }
    let gap = Range::new(at1, len1, at2, len2);
    if !gap.is_empty() {
        changes.push(gap);
    }
    changes
}

/// Equal elements from `start`s on, up to the `end`s.
pub(crate) fn expand_forward<T: PartialEq>(
    data1: &[T],
    data2: &[T],
    start1: usize,
    start2: usize,
    end1: usize,
    end2: usize,
) -> usize {
    let mut count = 0;
    while start1 + count < end1.min(data1.len())
        && start2 + count < end2.min(data2.len())
        && data1[start1 + count] == data2[start2 + count]
    {
        count += 1;
    }
    count
}

/// Equal elements from the `end`s back, down to the `start`s.
pub(crate) fn expand_backward<T: PartialEq>(
    data1: &[T],
    data2: &[T],
    start1: usize,
    start2: usize,
    end1: usize,
    end2: usize,
) -> usize {
    let mut count = 0;
    while end1 > start1 + count
        && end2 > start2 + count
        && data1[end1 - 1 - count] == data2[end2 - 1 - count]
    {
        count += 1;
    }
    count
}

/// Where a boundary between two touching matches may slide: a policy
/// answers 0 (stay), >0 (forward) or <0 (backward), given how many
/// elements are equal past the boundary each way.
pub(crate) trait Shift<T> {
    fn shift(
        &self,
        data1: &[T],
        data2: &[T],
        touch: Side,
        equal_forward: usize,
        equal_backward: usize,
        range1: &Range,
        range2: &Range,
    ) -> isize;
}

/// Slide boundaries between touching unchanged ranges: merge the two
/// when one fully repeats into the other (fewer chunks), otherwise
/// ask the policy where the split should land.
pub(crate) fn optimize<T: PartialEq>(
    data1: &[T],
    data2: &[T],
    unchanged: Vec<Range>,
    policy: &impl Shift<T>,
) -> Vec<Range> {
    let mut ranges: Vec<Range> = Vec::with_capacity(unchanged.len());
    for range in unchanged {
        ranges.push(range);
        process_last(&mut ranges, data1, data2, policy);
    }
    ranges
}

fn process_last<T: PartialEq>(
    ranges: &mut Vec<Range>,
    data1: &[T],
    data2: &[T],
    policy: &impl Shift<T>,
) {
    let n = ranges.len();
    if n < 2 {
        return;
    }
    let range1 = ranges[n - 2];
    let range2 = ranges[n - 1];
    if range1.end1 != range2.start1 && range1.end2 != range2.start2 {
        // The matches do not touch: nothing to slide.
        return;
    }
    let count1 = range1.end1 - range1.start1;
    let count2 = range2.end1 - range2.start1;

    let equal_forward = expand_forward(
        data1,
        data2,
        range1.end1,
        range1.end2,
        range1.end1 + count2,
        range1.end2 + count2,
    );
    let equal_backward = expand_backward(
        data1,
        data2,
        range2.start1.saturating_sub(count1),
        range2.start2.saturating_sub(count1),
        range2.start1,
        range2.start2,
    );
    if equal_forward == 0 && equal_backward == 0 {
        return;
    }

    // Merge left: [A]B[B] -> [AB]B.
    if equal_forward == count2 {
        ranges.truncate(n - 2);
        ranges.push(Range::new(
            range1.start1,
            range1.end1 + count2,
            range1.start2,
            range1.end2 + count2,
        ));
        process_last(ranges, data1, data2, policy);
        return;
    }
    // Merge right: [A]A[B] -> A[AB].
    if equal_backward == count1 {
        ranges.truncate(n - 2);
        ranges.push(Range::new(
            range2.start1 - count1,
            range2.end1,
            range2.start2 - count1,
            range2.end2,
        ));
        process_last(ranges, data1, data2, policy);
        return;
    }

    let touch = match range1.end1 == range2.start1 {
        true => Side::First,
        false => Side::Second,
    };
    let shift = policy.shift(
        data1,
        data2,
        touch,
        equal_forward,
        equal_backward,
        &range1,
        &range2,
    );
    if shift != 0 {
        let shifted = |at: usize| (at as isize + shift) as usize;
        ranges.truncate(n - 2);
        ranges.push(Range::new(
            range1.start1,
            shifted(range1.end1),
            range1.start2,
            shifted(range1.end2),
        ));
        ranges.push(Range::new(
            shifted(range2.start1),
            range2.end1,
            shifted(range2.start2),
            range2.end2,
        ));
    }
}
