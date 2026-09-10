use std::ops::Range;

use operation::{Op, Operation};
use sumtree::{Bias, Dimension, Item, Splice, SumTree, Summary};

#[derive(Clone, Debug)]
pub(crate) struct WidthSpan {
    pub(crate) bytes: u32,
    pub(crate) width: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct WidthSummary {
    bytes: u64,
    max: f32,
}

impl Summary for WidthSummary {
    fn empty() -> Self {
        Self { bytes: 0, max: 0.0 }
    }

    fn add(&mut self, other: &Self) {
        self.bytes += other.bytes;
        self.max = self.max.max(other.max);
    }
}

impl Item for WidthSpan {
    type Summary = WidthSummary;

    fn summary(&self) -> WidthSummary {
        WidthSummary {
            bytes: u64::from(self.bytes),
            max: self.width,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
struct Bytes(u64);

impl Dimension<WidthSummary> for Bytes {
    fn from_summary(summary: &WidthSummary) -> Self {
        Self(summary.bytes)
    }

    fn add(&mut self, other: Self) {
        self.0 += other.0;
    }
}

#[derive(Clone)]
pub(crate) struct WidthTree {
    tree: SumTree<WidthSpan>,
}

impl WidthTree {
    pub(crate) fn new() -> Self {
        Self {
            tree: SumTree::new(),
        }
    }

    pub(crate) fn byte_size(&self) -> u64 {
        self.tree.summary().bytes
    }

    pub(crate) fn max(&self) -> f32 {
        self.tree.summary().max
    }

    pub(crate) fn max_in(&self, range: Range<u32>) -> f32 {
        self.tree
            .summary_between(Bytes(u64::from(range.start))..Bytes(u64::from(range.end)))
            .max
    }

    pub(crate) fn replace_bytes(&mut self, range: Range<u64>, spans: Vec<WidthSpan>) {
        let total = self.byte_size();
        let range = range.start.min(total)..range.end.min(total).max(range.start.min(total));

        let start_seek = self.tree.find(Bytes(range.start), Bias::Right);
        let mut insert: Vec<WidthSpan> = Vec::with_capacity(spans.len() + 2);
        let mut start_index = start_seek.index;
        if start_seek.start.0 < range.start {
            let head = self
                .tree
                .get(start_seek.index)
                .expect("a span holds the cut byte");
            insert.push(WidthSpan {
                bytes: (range.start - start_seek.start.0).min(u64::from(head.bytes)) as u32,
                width: head.width,
            });
        } else {
        }

        let end_seek = self.tree.find(Bytes(range.end), Bias::Left);
        let end_index = if end_seek.index == self.tree.len() {
            self.tree.len()
        } else {
            end_seek.index + 1
        };
        let end_index = end_index.max(start_index);
        insert.extend(spans);
        if end_index > start_index {
            if let Some(tail) = self.tree.get(end_index - 1) {
                let tail_start = self.tree.offset_of::<Bytes>(end_index - 1).0;
                let tail_end = tail_start + u64::from(tail.bytes);
                if tail_end > range.end {
                    insert.push(WidthSpan {
                        bytes: (tail_end - range.end) as u32,
                        width: tail.width,
                    });
                }
            }
        }

        if start_seek.start.0 < range.start {
            start_index = start_seek.index;
        }
        self.tree = self.tree.splice([Splice {
            range: start_index..end_index,
            insert,
        }]);
    }

    pub(crate) fn edit(&mut self, operation: &Operation) {
        let mut offset = 0u64;
        for op in operation.iter() {
            match op {
                Op::Retain(len) => offset += u64::from(len),
                Op::Insert(text) => {
                    let len = text.len() as u64;
                    self.insert_bytes(offset, len);
                    offset += len;
                }
                Op::Delete(text) => {
                    let len = text.len() as u64;
                    self.replace_bytes(offset..offset + len, Vec::new());
                }
            }
        }
    }

    fn insert_bytes(&mut self, offset: u64, len: u64) {
        if len == 0 {
            return;
        }
        if self.tree.is_empty() {
            self.tree = self.tree.splice([Splice {
                range: 0..0,
                insert: vec![WidthSpan {
                    bytes: len.min(u64::from(u32::MAX)) as u32,
                    width: 0.0,
                }],
            }]);
            return;
        }

        let seek = self
            .tree
            .find(Bytes(offset.min(self.byte_size())), Bias::Left);
        let index = seek.index.min(self.tree.len() - 1);
        let span = self.tree.get(index).expect("a span holds the offset");
        self.tree = self.tree.set(
            index,
            WidthSpan {
                bytes: span
                    .bytes
                    .saturating_add(len.min(u64::from(u32::MAX)) as u32),
                width: span.width,
            },
        );
    }

    pub(crate) fn clear(&mut self) {
        self.tree = SumTree::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn below(&mut self, bound: usize) -> usize {
            match bound {
                0 => 0,
                _ => (self.next() % bound as u64) as usize,
            }
        }
    }

    fn model_max_in(model: &[f32], range: Range<usize>) -> f32 {
        model[range.start.min(model.len())..range.end.min(model.len())]
            .iter()
            .fold(0.0f32, |max, width| max.max(*width))
    }

    #[test]
    fn replace_and_edit_match_the_byte_model() {
        let mut rng = Rng(101);
        for _ in 0..40 {
            let mut model: Vec<f32> = Vec::new();
            let mut tree = WidthTree::new();
            for _ in 0..60 {
                match rng.below(3) {
                    0 => {
                        let a = rng.below(model.len() + 1);
                        let b = rng.below(model.len() + 1);
                        let (a, b) = (a.min(b), a.max(b));
                        let mut spans = Vec::new();
                        let mut bytes = Vec::new();
                        for _ in 0..rng.below(4) {
                            let len = 1 + rng.below(30);
                            let width = rng.below(5_000) as f32;
                            spans.push(WidthSpan {
                                bytes: len as u32,
                                width,
                            });
                            bytes.extend(std::iter::repeat(width).take(len));
                        }
                        tree.replace_bytes(a as u64..b as u64, spans);
                        model.splice(a..b, bytes);
                    }
                    1 => {
                        let at = rng.below(model.len() + 1);
                        let len = 1 + rng.below(20);
                        let op = Operation::from_ops([
                            Op::Retain(at as u32),
                            Op::Insert("x".repeat(len)),
                            Op::Retain((model.len() - at) as u32),
                        ]);
                        tree.edit(&op);

                        let width = match at {
                            0 => model.first().copied().unwrap_or(0.0),
                            _ => model[at - 1],
                        };
                        model.splice(at..at, std::iter::repeat(width).take(len));
                    }
                    _ => {
                        let a = rng.below(model.len() + 1);
                        let b = rng.below(model.len() + 1);
                        let (a, b) = (a.min(b), a.max(b));
                        let op = Operation::from_ops([
                            Op::Retain(a as u32),
                            Op::Delete("x".repeat(b - a)),
                            Op::Retain((model.len() - b) as u32),
                        ]);
                        tree.edit(&op);
                        model.splice(a..b, std::iter::empty());
                    }
                }
                assert_eq!(tree.byte_size(), model.len() as u64, "byte coverage");
                assert_eq!(
                    tree.max(),
                    model_max_in(&model, 0..model.len()),
                    "whole max"
                );
                for _ in 0..8 {
                    let a = rng.below(model.len() + 1);
                    let b = rng.below(model.len() + 1);
                    let (a, b) = (a.min(b), a.max(b));
                    assert_eq!(
                        tree.max_in(a as u32..b as u32),
                        model_max_in(&model, a..b),
                        "range {a}..{b} over {} bytes",
                        model.len()
                    );
                }
            }
        }
    }
}
