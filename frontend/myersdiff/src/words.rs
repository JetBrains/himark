// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The WORD pass inside a changed block (IntelliJ's `ByWordRt`,
//! ported): the texts are cut into word chunks and newlines, the
//! LCS runs over those — a word matches whole or changes whole —
//! then the punctuation between matched words is matched on its
//! own, and equal whitespace on a change's edges is given back.
//! Letters never match across words, which is what keeps `color`
//! against `colour` one changed word and not two changed letters.

use std::time::Instant;

use crate::ranges::{self, Range, Shift, Side};

/// One chunk of a text: a word (a run of letters, digits and `_`,
/// or one codepoint of a continuous script) or a newline. Spaces
/// and punctuation are no chunk at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Chunk {
    pub start: usize,
    pub end: usize,
    pub newline: bool,
}

/// What the LCS compares: a word by its text, every newline alike.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Token<'a> {
    Word(&'a str),
    Newline,
}

fn is_whitespace(byte: u8) -> bool {
    byte == b' ' || byte == b'\t' || byte == b'\n'
}

/// ASCII punctuation, `_` excluded — a word character.
pub(crate) fn is_punctuation(c: char) -> bool {
    let b = c as u32;
    if b == 95 {
        return false;
    }
    (33..=47).contains(&b)
        || (58..=64).contains(&b)
        || (91..=96).contains(&b)
        || (123..=126).contains(&b)
}

fn is_alpha(c: char) -> bool {
    if c.is_ascii() && is_whitespace(c as u8) {
        return false;
    }
    !is_punctuation(c)
}

/// A codepoint of a script without word breaks (CJK, kana, Thai,
/// Javanese), or anything outside the alphabet: one chunk each.
fn is_continuous_script(c: char) -> bool {
    let code = c as u32;
    if code < 128 || c.is_ascii_digit() {
        return false;
    }
    if code > 0xFFFF {
        return true;
    }
    let ideographic = matches!(
        code,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3006..=0x3007 | 0x3021..=0x3029 | 0x3038..=0x303A
    );
    if ideographic || !c.is_alphabetic() {
        return true;
    }
    matches!(
        code,
        0x3040..=0x309F | 0x30A0..=0x30FF | 0x31F0..=0x31FF | 0x0E00..=0x0E7F | 0xA980..=0xA9DF
    )
}

pub(crate) fn chunks(text: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut word_start: Option<usize> = None;
    for (offset, c) in text.char_indices() {
        let alpha = is_alpha(c);
        let word_part = alpha && !is_continuous_script(c);
        if word_part {
            word_start.get_or_insert(offset);
            continue;
        }
        if let Some(start) = word_start.take() {
            chunks.push(Chunk {
                start,
                end: offset,
                newline: false,
            });
        }
        if alpha {
            chunks.push(Chunk {
                start: offset,
                end: offset + c.len_utf8(),
                newline: false,
            });
        } else if c == '\n' {
            chunks.push(Chunk {
                start: offset,
                end: offset + 1,
                newline: true,
            });
        }
    }
    if let Some(start) = word_start {
        chunks.push(Chunk {
            start,
            end: text.len(),
            newline: false,
        });
    }
    chunks
}

fn tokens<'a>(text: &'a str, chunks: &[Chunk]) -> Vec<Token<'a>> {
    chunks
        .iter()
        .map(|chunk| match chunk.newline {
            true => Token::Newline,
            false => Token::Word(&text[chunk.start..chunk.end]),
        })
        .collect()
}

/// The changed byte ranges between `text1` and `text2`, in order.
pub(crate) fn changes(text1: &str, text2: &str, deadline: Option<Instant>) -> Vec<Range> {
    let chunks1 = chunks(text1);
    let chunks2 = chunks(text2);
    let tokens1 = tokens(text1, &chunks1);
    let tokens2 = tokens(text2, &chunks2);
    // Words first, so a newline never anchors a match a word could
    // have taken ("\ni" against "i\n" keeps the word).
    let matched = ranges::unchanged_two_step(
        &tokens1,
        &tokens2,
        |token| matches!(token, Token::Word(_)),
        deadline,
    );
    let matched = ranges::optimize(
        &tokens1,
        &tokens2,
        matched,
        &WordShift {
            text1,
            text2,
            chunks1: &chunks1,
            chunks2: &chunks2,
        },
    );
    let equal = PunctuationMatcher {
        text1,
        text2,
        chunks1: &chunks1,
        chunks2: &chunks2,
        matched: &matched,
        equal: EqualBuilder::default(),
        last: None,
    }
    .build();
    let changes = ranges::changes_between(&equal, text1.len(), text2.len());
    give_back_whitespace(text1, text2, changes)
}

/// Where a boundary between touching word matches slides: never
/// inside a run of words with no whitespace between them (a
/// "sentence"), so `1.0.123 1.0.155` against `1.0.123 1.0.134
/// 1.0.155` marks one whole version.
struct WordShift<'a> {
    text1: &'a str,
    text2: &'a str,
    chunks1: &'a [Chunk],
    chunks2: &'a [Chunk],
}

impl<'a> WordShift<'a> {
    fn separated(text: &str, first: &Chunk, second: &Chunk) -> bool {
        if first.newline || second.newline {
            return true;
        }
        text.as_bytes()[first.end..second.start]
            .iter()
            .any(|byte| is_whitespace(*byte))
    }

    /// The first boundary within `count` chunks from `offset` that
    /// whitespace separates, walking one way; -1 when none.
    fn sequence_edge(
        text: &str,
        chunks: &[Chunk],
        offset: usize,
        count: usize,
        forward: bool,
    ) -> isize {
        for i in 0..count {
            let (first, second) = match forward {
                true => (chunks.get(offset + i), chunks.get(offset + i + 1)),
                false => (
                    offset.checked_sub(i + 1).and_then(|at| chunks.get(at)),
                    offset.checked_sub(i).and_then(|at| chunks.get(at)),
                ),
            };
            let (Some(first), Some(second)) = (first, second) else {
                return -1;
            };
            if Self::separated(text, first, second) {
                return i as isize + 1;
            }
        }
        -1
    }
}

impl<'a, T> Shift<T> for WordShift<'a> {
    fn shift(
        &self,
        _data1: &[T],
        _data2: &[T],
        touch: Side,
        equal_forward: usize,
        equal_backward: usize,
        _range1: &Range,
        range2: &Range,
    ) -> isize {
        let chunks = touch.pick(self.chunks1, self.chunks2);
        let text = touch.pick(self.text1, self.text2);
        let touch_start = touch.pick(range2.start1, range2.start2);
        if touch_start == 0 || touch_start >= chunks.len() {
            return 0;
        }
        // Already separated by whitespace: a fine place for a boundary.
        if Self::separated(text, &chunks[touch_start - 1], &chunks[touch_start]) {
            return 0;
        }
        // Slide left: [X]A Y[A ZA] -> [XA] YA [ZA].
        let left = Self::sequence_edge(text, chunks, touch_start, equal_forward, true);
        if left > 0 {
            return left;
        }
        // Slide right: [AX A]Y A[Z] -> [AX] AY [AZ].
        let right = Self::sequence_edge(text, chunks, touch_start - 1, equal_backward, false);
        if right > 0 {
            return -right;
        }
        0
    }
}

/// Equal byte ranges, marked in order; touching marks merge.
#[derive(Default)]
struct EqualBuilder {
    ranges: Vec<Range>,
}

impl EqualBuilder {
    fn mark(&mut self, start1: usize, start2: usize, end1: usize, end2: usize) {
        debug_assert_eq!(
            end1 - start1,
            end2 - start2,
            "an equal range is as long on both sides"
        );
        if let Some(last) = self.ranges.last() {
            debug_assert!(
                start1 >= last.end1 && start2 >= last.end2,
                "equal ranges are marked in order"
            );
        }
        ranges::push_unchanged(&mut self.ranges, Range::new(start1, end1, start2, end2));
    }
}

/// Between matched words, match the PUNCTUATION on its own
/// (IntelliJ's `AdjustmentPunctuationMatcher`): the gaps around each
/// matched pair are compared punctuation-char by punctuation-char,
/// and where one side has unmatched words between two matches the
/// other side's single gap is compared against both of its gaps
/// as one ("[ X { A ! B } Y ]" vs "( X ... Y )").
struct PunctuationMatcher<'a> {
    text1: &'a str,
    text2: &'a str,
    chunks1: &'a [Chunk],
    chunks2: &'a [Chunk],
    matched: &'a [Range],
    equal: EqualBuilder,
    /// The gap AFTER the last matched pair, waiting for the next
    /// pair to say how it is matched.
    last: Option<Range>,
}

impl<'a> PunctuationMatcher<'a> {
    fn build(mut self) -> Vec<Range> {
        self.forward_gap(None, None);
        for range in self.matched {
            let count = range.end1 - range.start1;
            for i in 0..count {
                let index1 = range.start1 + i;
                let index2 = range.start2 + i;
                let chunk1 = self.chunks1[index1];
                let chunk2 = self.chunks2[index2];
                self.backward_gap(index1, index2);
                self.equal
                    .mark(chunk1.start, chunk2.start, chunk1.end, chunk2.end);
                self.forward_gap(Some(index1), Some(index2));
            }
        }
        self.backward_gap(self.chunks1.len(), self.chunks2.len());
        self.equal.ranges
    }

    fn gap_after(&self, index1: Option<usize>, index2: Option<usize>) -> Range {
        let start1 = index1.map_or(0, |index| self.chunks1[index].end);
        let start2 = index2.map_or(0, |index| self.chunks2[index].end);
        let next1 = index1.map_or(0, |index| index + 1);
        let next2 = index2.map_or(0, |index| index + 1);
        let end1 = self
            .chunks1
            .get(next1)
            .map_or(self.text1.len(), |chunk| chunk.start);
        let end2 = self
            .chunks2
            .get(next2)
            .map_or(self.text2.len(), |chunk| chunk.start);
        Range::new(start1, end1, start2, end2)
    }

    fn gap_before(&self, index1: usize, index2: usize) -> Range {
        let start1 = match index1 {
            0 => 0,
            index => self.chunks1[index - 1].end,
        };
        let start2 = match index2 {
            0 => 0,
            index => self.chunks2[index - 1].end,
        };
        let end1 = self
            .chunks1
            .get(index1)
            .map_or(self.text1.len(), |chunk| chunk.start);
        let end2 = self
            .chunks2
            .get(index2)
            .map_or(self.text2.len(), |chunk| chunk.start);
        Range::new(start1, end1, start2, end2)
    }

    fn forward_gap(&mut self, index1: Option<usize>, index2: Option<usize>) {
        debug_assert!(self.last.is_none());
        self.last = Some(self.gap_after(index1, index2));
    }

    fn backward_gap(&mut self, index1: usize, index2: usize) {
        let gap = self.gap_before(index1, index2);
        let Some(last) = self.last.take() else {
            return;
        };
        if last.start1 == gap.start1 && last.start2 == gap.start2 {
            // Adjacent matched words: ONE gap on both sides ("A B" - "A B").
            self.match_gap(gap);
        } else if last.start1 < gap.start1 && last.start2 < gap.start2 {
            // Unmatched words between on both sides ("A X B" - "A Y B").
            self.match_gap(last);
            self.match_gap(gap);
        } else if last.start1 == gap.start1 && last.end1 == gap.end1 {
            // Unmatched words between on the second side only
            // ("A B" - "A Y B"): the first side's one gap against both.
            self.match_split(
                Side::First,
                gap.start1,
                gap.end1,
                last.start2,
                last.end2,
                gap.start2,
                gap.end2,
            );
        } else if last.start2 == gap.start2 && last.end2 == gap.end2 {
            self.match_split(
                Side::Second,
                gap.start2,
                gap.end2,
                last.start1,
                last.end1,
                gap.start1,
                gap.end1,
            );
        }
    }

    fn match_gap(&mut self, gap: Range) {
        if gap.is_empty() {
            return;
        }
        let seq1 = &self.text1[gap.start1..gap.end1];
        let seq2 = &self.text2[gap.start2..gap.end2];
        for equal in punctuation_matches(seq1, seq2) {
            self.equal.mark(
                gap.start1 + equal.start1,
                gap.start2 + equal.start2,
                gap.start1 + equal.end1,
                gap.start2 + equal.end2,
            );
        }
    }

    /// One gap on `whole`'s side against two gaps on the other,
    /// compared as if the two were one sequence, then split back.
    #[allow(clippy::too_many_arguments)]
    fn match_split(
        &mut self,
        whole: Side,
        start: usize,
        end: usize,
        first_start: usize,
        first_end: usize,
        second_start: usize,
        second_end: usize,
    ) {
        let (text_whole, text_split) =
            whole.pick((self.text1, self.text2), (self.text2, self.text1));
        let seq = &text_whole[start..end];
        let first = &text_split[first_start..first_end];
        let second = &text_split[second_start..second_end];
        let mut joined = String::with_capacity(first.len() + second.len());
        joined.push_str(first);
        joined.push_str(second);
        let offset = first.len();
        for equal in punctuation_matches(seq, &joined) {
            // A match straddling the seam splits at it.
            let pieces = if equal.end2 <= offset {
                vec![(equal, first_start, 0)]
            } else if equal.start2 >= offset {
                vec![(equal, second_start, offset)]
            } else {
                let cut = offset - equal.start2;
                vec![
                    (
                        Range::new(equal.start1, equal.start1 + cut, equal.start2, offset),
                        first_start,
                        0,
                    ),
                    (
                        Range::new(equal.start1 + cut, equal.end1, offset, equal.end2),
                        second_start,
                        offset,
                    ),
                ]
            };
            for (piece, split_base, split_offset) in pieces {
                let whole_start = start + piece.start1;
                let whole_end = start + piece.end1;
                let split_start = split_base + piece.start2 - split_offset;
                let split_end = split_base + piece.end2 - split_offset;
                match whole {
                    Side::First => self
                        .equal
                        .mark(whole_start, split_start, whole_end, split_end),
                    Side::Second => self
                        .equal
                        .mark(split_start, whole_start, split_end, whole_end),
                }
            }
        }
    }
}

/// The punctuation chars of two sequences matched by LCS, every
/// other char left unmatched; as equal byte ranges of the sequences.
fn punctuation_matches(seq1: &str, seq2: &str) -> Vec<Range> {
    let collect = |seq: &str| -> (Vec<char>, Vec<usize>) {
        seq.char_indices()
            .filter(|(_, c)| is_punctuation(*c))
            .map(|(offset, c)| (c, offset))
            .unzip()
    };
    let (chars1, offsets1) = collect(seq1);
    let (chars2, offsets2) = collect(seq2);
    if chars1.is_empty() || chars2.is_empty() {
        return Vec::new();
    }
    let mut equal = Vec::new();
    for range in ranges::unchanged(&chars1, &chars2, None) {
        for i in 0..range.end1 - range.start1 {
            // Punctuation is ASCII: one byte each.
            let offset1 = offsets1[range.start1 + i];
            let offset2 = offsets2[range.start2 + i];
            ranges::push_unchanged(
                &mut equal,
                Range::new(offset1, offset1 + 1, offset2, offset2 + 1),
            );
        }
    }
    equal
}

/// Equal whitespace on a change's edges is not a change
/// (IntelliJ's `DefaultCorrector`): the word LCS leaves the spaces
/// between words unmatched, so each change gives back the equal
/// whitespace it starts and ends with.
fn give_back_whitespace(text1: &str, text2: &str, changes: Vec<Range>) -> Vec<Range> {
    let bytes1 = text1.as_bytes();
    let bytes2 = text2.as_bytes();
    let mut kept = Vec::with_capacity(changes.len());
    for change in changes {
        let mut end_cut = 0;
        while change.end1 > change.start1 + end_cut
            && change.end2 > change.start2 + end_cut
            && bytes1[change.end1 - 1 - end_cut] == bytes2[change.end2 - 1 - end_cut]
            && is_whitespace(bytes1[change.end1 - 1 - end_cut])
        {
            end_cut += 1;
        }
        let (end1, end2) = (change.end1 - end_cut, change.end2 - end_cut);
        let mut start_cut = 0;
        while change.start1 + start_cut < end1
            && change.start2 + start_cut < end2
            && bytes1[change.start1 + start_cut] == bytes2[change.start2 + start_cut]
            && is_whitespace(bytes1[change.start1 + start_cut])
        {
            start_cut += 1;
        }
        let trimmed = Range::new(
            change.start1 + start_cut,
            end1,
            change.start2 + start_cut,
            end2,
        );
        if !trimmed.is_empty() {
            kept.push(trimmed);
        }
    }
    kept
}
