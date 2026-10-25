// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The LINE pass (IntelliJ's `ByLineRt`, ported): lines with more
//! than a few non-space chars are matched first, so a brace or a
//! blank line never anchors a match on its own; the gaps between
//! those matches are then matched line by line; and a boundary
//! between touching matches slides to an empty or near-empty line,
//! where a human would start or end a hunk.

use std::ops::Range as ByteRange;
use std::time::Instant;

use crate::ranges::{self, Range, Shift, Side};

/// Lines with this many non-space chars or fewer do not anchor a
/// match on the first pass, and attract hunk boundaries.
const UNIMPORTANT_LINE_CHARS: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Line<'a> {
    text: &'a str,
}

impl Line<'_> {
    fn non_space(&self) -> usize {
        self.text
            .bytes()
            .filter(|byte| !matches!(byte, b' ' | b'\t' | b'\n'))
            .count()
    }
}

/// One aligned block of the texts, by BYTES.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Block {
    pub old: ByteRange<usize>,
    pub new: ByteRange<usize>,
    pub equal: bool,
}

fn split_lines(text: &str) -> Vec<Line<'_>> {
    text.split_inclusive('\n')
        .map(|text| Line { text })
        .collect()
}

/// The texts aligned line by line, as byte blocks in order.
pub(crate) fn blocks(text1: &str, text2: &str, deadline: Option<Instant>) -> Vec<Block> {
    let lines1 = split_lines(text1);
    let lines2 = split_lines(text2);
    let matched = ranges::unchanged_two_step(
        &lines1,
        &lines2,
        |line| line.non_space() > UNIMPORTANT_LINE_CHARS,
        deadline,
    );
    let matched = ranges::optimize(&lines1, &lines2, matched, &LineShift);

    let offsets = |lines: &[Line<'_>]| -> Vec<usize> {
        let mut offsets = Vec::with_capacity(lines.len() + 1);
        let mut at = 0;
        offsets.push(at);
        for line in lines {
            at += line.text.len();
            offsets.push(at);
        }
        offsets
    };
    let offsets1 = offsets(&lines1);
    let offsets2 = offsets(&lines2);
    let mut blocks = Vec::new();
    let (mut at1, mut at2) = (0, 0);
    let mut push = |start1: usize, end1: usize, start2: usize, end2: usize, equal: bool| {
        if start1 == end1 && start2 == end2 {
            return;
        }
        blocks.push(Block {
            old: offsets1[start1]..offsets1[end1],
            new: offsets2[start2]..offsets2[end2],
            equal,
        });
    };
    for range in &matched {
        push(at1, range.start1, at2, range.start2, false);
        push(range.start1, range.end1, range.start2, range.end2, true);
        at1 = range.end1;
        at2 = range.end2;
    }
    push(at1, lines1.len(), at2, lines2.len(), false);
    blocks
}

/// Where a boundary between touching line matches slides: to an
/// empty line first, then to a near-empty one — in the unchanged
/// lines (the hunk ends right before or after it) or in the changed
/// ones (the hunk starts or ends with it).
struct LineShift;

impl LineShift {
    fn next_unimportant(
        lines: &[Line<'_>],
        offset: usize,
        count: usize,
        threshold: usize,
    ) -> isize {
        for i in 0..count {
            match lines.get(offset + i) {
                Some(line) if line.non_space() <= threshold => return i as isize,
                Some(_) => {}
                None => return -1,
            }
        }
        -1
    }

    fn prev_unimportant(
        lines: &[Line<'_>],
        offset: usize,
        count: usize,
        threshold: usize,
    ) -> isize {
        for i in 0..count {
            match offset.checked_sub(i).and_then(|at| lines.get(at)) {
                Some(line) if line.non_space() <= threshold => return i as isize,
                Some(_) => {}
                None => return -1,
            }
        }
        -1
    }

    fn pick(forward: isize, backward: isize) -> Option<isize> {
        if forward == -1 && backward == -1 {
            return None;
        }
        if forward == 0 || backward == 0 {
            return Some(0);
        }
        Some(if forward != -1 { forward } else { -backward })
    }

    /// An empty-line boundary in the UNCHANGED lines.
    #[allow(clippy::too_many_arguments)]
    fn unchanged_boundary(
        lines1: &[Line<'_>],
        lines2: &[Line<'_>],
        touch: Side,
        equal_forward: usize,
        equal_backward: usize,
        range2: &Range,
        threshold: usize,
    ) -> Option<isize> {
        let lines = touch.pick(lines1, lines2);
        let touch_start = touch.pick(range2.start1, range2.start2);
        let forward = Self::next_unimportant(lines, touch_start, equal_forward + 1, threshold);
        let backward = match touch_start.checked_sub(1) {
            Some(at) => Self::prev_unimportant(lines, at, equal_backward + 1, threshold),
            None => -1,
        };
        Self::pick(forward, backward)
    }

    /// An empty-line boundary in the CHANGED lines.
    #[allow(clippy::too_many_arguments)]
    fn changed_boundary(
        lines1: &[Line<'_>],
        lines2: &[Line<'_>],
        touch: Side,
        equal_forward: usize,
        equal_backward: usize,
        range1: &Range,
        range2: &Range,
        threshold: usize,
    ) -> Option<isize> {
        let other = touch.other();
        let lines = other.pick(lines1, lines2);
        let change_start = other.pick(range1.end1, range1.end2);
        let change_end = other.pick(range2.start1, range2.start2);
        let forward = Self::next_unimportant(lines, change_start, equal_forward + 1, threshold);
        let backward = match change_end.checked_sub(1) {
            Some(at) => Self::prev_unimportant(lines, at, equal_backward + 1, threshold),
            None => -1,
        };
        Self::pick(forward, backward)
    }
}

impl<'a> Shift<Line<'a>> for LineShift {
    fn shift(
        &self,
        lines1: &[Line<'a>],
        lines2: &[Line<'a>],
        touch: Side,
        equal_forward: usize,
        equal_backward: usize,
        range1: &Range,
        range2: &Range,
    ) -> isize {
        for threshold in [0, UNIMPORTANT_LINE_CHARS] {
            if let Some(shift) = Self::unchanged_boundary(
                lines1,
                lines2,
                touch,
                equal_forward,
                equal_backward,
                range2,
                threshold,
            ) {
                return shift;
            }
            if let Some(shift) = Self::changed_boundary(
                lines1,
                lines2,
                touch,
                equal_forward,
                equal_backward,
                range1,
                range2,
                threshold,
            ) {
                return shift;
            }
        }
        0
    }
}
