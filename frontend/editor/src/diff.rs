use std::ops::Range;

use operation::{Op, Operation};
use similar::{DiffTag, TextDiff};
use text::Text;

use crate::edit_log::EditLog;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DiffId(pub(crate) u64);

impl DiffId {
    pub(crate) fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[derive(Clone)]
pub struct Diff {
    pub(crate) operation: Operation,
    pub(crate) base_revision: u64,
    pub(crate) markup: crate::markup::MarkupId,
    pub(crate) generation: u64,
}

impl Diff {
    pub fn operation(&self) -> &Operation {
        &self.operation
    }

    pub fn base_revision(&self) -> u64 {
        self.base_revision
    }

    pub fn markup(&self) -> crate::markup::MarkupId {
        self.markup
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn apply_base_edits(&mut self, base_log: &EditLog) -> bool {
        let now = base_log.revision();
        if now < self.base_revision {
            return false;
        }
        if let Some(edits) = base_log.compose_since(self.base_revision) {
            self.operation = edits.invert().splice_compose_into(&self.operation);
        }
        self.base_revision = now;
        true
    }
}

const REFINE_BUDGET: usize = 4 * 1024;

const RETAIN_NOISE: usize = 3;

pub fn diff(left: &Text, right: &Text) -> Operation {
    let left = materialize(left);
    let right = materialize(right);
    let mut ops: Vec<Op> = Vec::new();

    let lines = TextDiff::from_lines(left.as_str(), right.as_str());
    for op in lines.ops() {
        let old = byte_span(&left, lines.old_slices(), op.old_range());
        let new = byte_span(&right, lines.new_slices(), op.new_range());
        match op.tag() {
            DiffTag::Equal => push_retain(&mut ops, old.len()),
            DiffTag::Delete => push_delete(&mut ops, &left[old]),
            DiffTag::Insert => push_insert(&mut ops, &right[new]),
            DiffTag::Replace => refine(&mut ops, &left[old], &right[new]),
        }
    }
    Operation::from_ops(ops)
}

fn refine(ops: &mut Vec<Op>, old: &str, new: &str) {
    if old.len() > REFINE_BUDGET || new.len() > REFINE_BUDGET {
        push_delete(ops, old);
        push_insert(ops, new);
        return;
    }
    let chars = TextDiff::from_chars(old, new);

    let mut refined: Vec<Op> = Vec::new();
    for op in chars.ops() {
        let old_span = char_span(old, chars.old_slices(), op.old_range());
        let new_span = char_span(new, chars.new_slices(), op.new_range());
        match op.tag() {
            DiffTag::Equal => push_retain(&mut refined, old_span.len()),
            DiffTag::Delete => push_delete(&mut refined, &old[old_span]),
            DiffTag::Insert => push_insert(&mut refined, &new[new_span]),
            DiffTag::Replace => {
                push_delete(&mut refined, &old[old_span]);
                push_insert(&mut refined, &new[new_span]);
            }
        }
    }

    let mut old_at = 0usize;
    for (index, op) in refined.iter().enumerate() {
        let len = op.old_len() as usize;
        if let Op::Retain(_) = op {
            let interior = index > 0 && index + 1 < refined.len();
            let retained = &old[old_at..old_at + len];
            if interior && len < RETAIN_NOISE && !retained.contains('\n') {
                push_delete(ops, retained);
                push_insert(ops, retained);
                old_at += len;
                continue;
            }
        }
        match op {
            Op::Retain(len) => push_retain(ops, *len as usize),
            Op::Delete(text) => push_delete(ops, text),
            Op::Insert(text) => push_insert(ops, text),
        }
        old_at += len;
    }
}

fn push_retain(ops: &mut Vec<Op>, len: usize) {
    if len == 0 {
        return;
    }
    if let Some(Op::Retain(last)) = ops.last_mut() {
        *last += len as u32;
        return;
    }
    ops.push(Op::Retain(len as u32));
}

fn push_delete(ops: &mut Vec<Op>, text: &str) {
    if text.is_empty() {
        return;
    }

    if let Some(Op::Insert(_)) = ops.last() {
        let insert = ops.pop().expect("just matched");
        match ops.last_mut() {
            Some(Op::Delete(last)) => last.push_str(text),
            _ => ops.push(Op::Delete(text.to_owned())),
        }
        ops.push(insert);
        return;
    }
    if let Some(Op::Delete(last)) = ops.last_mut() {
        last.push_str(text);
        return;
    }
    ops.push(Op::Delete(text.to_owned()));
}

fn push_insert(ops: &mut Vec<Op>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Op::Insert(last)) = ops.last_mut() {
        last.push_str(text);
        return;
    }
    ops.push(Op::Insert(text.to_owned()));
}

fn materialize(text: &Text) -> String {
    let mut view = text.view();
    let count = view.byte_count();
    view.byte_string(0, count)
}

fn byte_span(source: &str, slices: &[&str], elements: Range<usize>) -> Range<usize> {
    span_of(source, slices, elements)
}

fn char_span(source: &str, slices: &[&str], elements: Range<usize>) -> Range<usize> {
    span_of(source, slices, elements)
}

fn span_of(source: &str, slices: &[&str], elements: Range<usize>) -> Range<usize> {
    let start: usize = slices[..elements.start].iter().map(|s| s.len()).sum();
    let len: usize = slices[elements.clone()].iter().map(|s| s.len()).sum();
    debug_assert!(source.is_char_boundary(start) && source.is_char_boundary(start + len));
    start..start + len
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FragmentKind {
    Added,
    Deleted,
    Modified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fragment {
    pub kind: FragmentKind,
    pub left: Range<u32>,
    pub right: Range<u32>,

    pub words: Vec<(Range<u32>, Range<u32>)>,
}

pub fn fragments_from(op: &Operation, left: &Text, from_left: u32) -> Fragments {
    Fragments {
        iter: op.iter(),
        left_view: left.view(),
        left_at: 0,
        right_at: 0,
        from_left,
        pending: None,
    }
}

pub fn fragments_at(op: &Operation, left: &Text, from_left: u32) -> Fragments {
    let mut view = left.view();
    let old_len = op.old_len();
    if old_len == 0 {
        return Fragments {
            iter: op.iter(),
            left_view: view,
            left_at: 0,
            right_at: 0,
            from_left,
            pending: None,
        };
    }

    let mut probe = from_left.saturating_sub(1).min(old_len - 1);
    loop {
        let mut from = op.ops_from_old(probe);
        let first = from.ops.next();

        let at_origin = from.old_start == 0 && from.new_start == 0;
        let safepoint = match &first {
            None => true,

            Some(Op::Retain(len)) => {
                at_origin || retain_has_newline(&mut view, from.old_start, *len)
            }
            Some(_) => at_origin,
        };
        if safepoint {
            return Fragments {
                iter: from.ops,
                left_view: view,
                left_at: from.old_start,
                right_at: from.new_start,
                from_left,

                pending: first,
            };
        }
        if from.old_start == 0 {
            return Fragments {
                iter: op.iter(),
                left_view: view,
                left_at: 0,
                right_at: 0,
                from_left,
                pending: None,
            };
        }

        probe = from.old_start - 1;
    }
}

fn retain_has_newline(view: &mut text::TextView, start: u32, len: u32) -> bool {
    let count = view.byte_count();
    let start = (start as usize).min(count);
    let end = start.saturating_add(len as usize).min(count);
    view.line_at(end) > view.line_at(start)
}

pub struct Fragments {
    iter: operation::Iter,
    left_view: text::TextView,
    left_at: u32,
    right_at: u32,
    from_left: u32,

    pending: Option<Op>,
}

impl Fragments {
    fn gap_has_newline(&mut self, len: u32) -> bool {
        retain_has_newline(&mut self.left_view, self.left_at, len)
    }
}

impl Iterator for Fragments {
    type Item = Fragment;

    fn next(&mut self) -> Option<Fragment> {
        loop {
            let mut op = match self.pending.take() {
                Some(op) => op,
                None => self.iter.next()?,
            };
            while let Op::Retain(len) = op {
                self.left_at += len;
                self.right_at += len;
                op = self.iter.next()?;
            }

            let left_start = self.left_at;
            let right_start = self.right_at;
            let mut words: Vec<(Range<u32>, Range<u32>)> = Vec::new();
            loop {
                match op {
                    Op::Delete(text) => {
                        let len = text.len() as u32;
                        words.push((self.left_at..self.left_at + len, empty(self.right_at)));
                        self.left_at += len;
                    }
                    Op::Insert(text) => {
                        let len = text.len() as u32;

                        let paired = words.last_mut().filter(|(left, right)| {
                            left.end == self.left_at
                                && right.start == right.end
                                && right.start == self.right_at
                        });
                        match paired {
                            Some((_, right)) => right.end = self.right_at + len,
                            None => words
                                .push((empty(self.left_at), self.right_at..self.right_at + len)),
                        }
                        self.right_at += len;
                    }
                    Op::Retain(len) => {
                        if self.gap_has_newline(len) {
                            self.pending = Some(Op::Retain(len));
                            break;
                        }

                        self.left_at += len;
                        self.right_at += len;
                    }
                }
                op = match self.iter.next() {
                    Some(op) => op,
                    None => break,
                };
            }

            let left = left_start..self.left_at;
            let right = right_start..self.right_at;
            if left.end < self.from_left && left.start < left.end {
                continue;
            }
            let kind = match (left.is_empty(), right.is_empty()) {
                (true, false) => FragmentKind::Added,
                (false, true) => FragmentKind::Deleted,
                _ => FragmentKind::Modified,
            };
            let words = match kind {
                FragmentKind::Modified => words,
                _ => Vec::new(),
            };
            return Some(Fragment {
                kind,
                left,
                right,
                words,
            });
        }
    }
}

fn empty(at: u32) -> Range<u32> {
    at..at
}

#[cfg(test)]
mod tests;
