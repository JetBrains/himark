// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use operation::{Op, Operation};
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

/// The user's standing fold reveals: intervals over the BASE (left)
/// text the fold derivation must never fold again. Pair-level state —
/// no view owns it — living on the `Diff` entry with the operation and
/// maintained at the same explicit points: born empty at `track`,
/// rolled by `apply_base_edits` with the same composed base operation
/// that rolls the diff, untouched by target-side edits (base
/// coordinates) and by normalize landings, gone with the entry.
pub type FoldBans = intervals::Intervals<crate::markup::IntervalId, ()>;

/// Rewrite the banned set WITHIN `extent` to `extent \ keep`, leaving
/// bans outside `extent` alone. `keep` empty bans the whole extent (a
/// full Remove); `keep == extent` un-bans it (a full Hide). The set
/// stays canonical: sorted, disjoint, merged, empty-free.
pub(crate) fn rewrite_bans(bans: &mut FoldBans, extent: Range<u32>, keep: Range<u32>) {
    use intervals::{IntervalQuery, Order};
    let mut fresh: Vec<Range<u32>> = Vec::new();
    for standing in bans.query(0..u32::MAX, Order::Ascending) {
        // The parts of a standing ban OUTSIDE the extent survive.
        if standing.range.start < extent.start {
            fresh.push(standing.range.start..standing.range.end.min(extent.start));
        }
        if standing.range.end > extent.end {
            fresh.push(standing.range.start.max(extent.end)..standing.range.end);
        }
    }
    let keep = keep.start.clamp(extent.start, extent.end)..keep.end.clamp(extent.start, extent.end);
    if keep.start >= keep.end {
        fresh.push(extent.clone());
    } else {
        fresh.push(extent.start..keep.start);
        fresh.push(keep.end..extent.end);
    }
    fresh.retain(|range| range.start < range.end);
    fresh.sort_by_key(|range| range.start);
    // Merge touching neighbours and rebuild — the set is user-action
    // sized, and a canonical rebuild is what keeps it disjoint.
    let mut merged: Vec<Range<u32>> = Vec::with_capacity(fresh.len());
    for range in fresh {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    let mut canonical = FoldBans::new();
    canonical.insert(
        merged
            .into_iter()
            .enumerate()
            .map(|(n, range)| intervals::Interval {
                range,
                greedy_left: false,
                greedy_right: false,
                key: crate::markup::IntervalId(n as u32),
                value: (),
            }),
    );
    *bans = canonical;
}

#[derive(Clone)]
pub struct Diff {
    pub(crate) operation: Operation,
    pub(crate) base_revision: u64,
    /// THE diff markup (docs/editor/scroll-stripe.md §7): one styled interval
    /// per hunk over the whole document, target coordinates — the
    /// split pane's washes, the gutter's classification and the
    /// scroll track's marks all read this one entry. Seeded at birth,
    /// refreshed by every normalize landing, shifted at the edit door
    /// in between.
    pub(crate) markup: crate::markup::MarkupId,
    pub(crate) generation: u64,
    pub(crate) fold_bans: FoldBans,
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

    pub fn fold_bans(&self) -> &FoldBans {
        &self.fold_bans
    }

    pub(crate) fn ban_fold(&mut self, extent: Range<u32>, keep: Range<u32>) {
        rewrite_bans(&mut self.fold_bans, extent, keep);
    }

    pub fn apply_base_edits(&mut self, base_log: &EditLog) -> bool {
        let now = base_log.revision();
        if now < self.base_revision {
            return false;
        }
        if let Some(edits) = base_log.compose_since(self.base_revision) {
            self.operation = edits.invert().splice_compose_into(&self.operation);
            // The fold bans are intervals over the same base text —
            // the one composed operation rolls them too (an emptied
            // ban banned text that is gone, and drops itself).
            self.fold_bans.edit(crate::markup::interval_steps(&edits));
        }
        self.base_revision = now;
        true
    }
}

/// One side's tree for a policy diff. `fresh` means the tree matches
/// the text byte-for-byte (a completed reparse). A STALE tree is still
/// EDIT-ADJUSTED — the edit door moves its byte offsets with every
/// edit — which is exactly what tree-sitter's incremental parse takes
/// as `old`: a policy catches it up for pennies instead of parsing the
/// whole file cold. Never hand a stale tree to alignment directly.
pub struct DiffTree<'a> {
    pub tree: &'a dyn crate::reparse::SyntaxTree,
    pub fresh: bool,
}

/// Optional syntax context for a policy diff: the language name and
/// whichever side trees the caller already has. A policy may parse or
/// catch up a side itself, or ignore trees entirely.
pub struct DiffSyntax<'a> {
    pub language: &'a str,
    pub base: Option<DiffTree<'a>>,
    pub target: Option<DiffTree<'a>>,
}

/// How an `Operation` is derived from two texts. The editor and the
/// documents registry depend only on this trait; the concrete engine
/// (Myers via the myersdiff crate, difftastic via structdiff) is
/// chosen at the edge and installed in the store as [`crate::env::Differ`]
/// (docs/editor/structural-diff.md). The contract: the returned operation
/// must be EXACT — applying it to `base` yields `target`, always;
/// policies may differ only in alignment quality and cost.
pub trait DiffPolicy: Send + Sync {
    fn diff(&self, base: &Text, target: &Text, syntax: Option<&DiffSyntax<'_>>) -> Operation;
}

/// Exact but content-blind: one delete of the whole base, one insert
/// of the whole target. The last-resort default when no policy was
/// installed — every himark application edge installs a real one at
/// construction, so hitting this outside a bare-store unit test means
/// the wiring regressed.
pub struct ReplaceAll;

impl DiffPolicy for ReplaceAll {
    fn diff(&self, base: &Text, target: &Text, _syntax: Option<&DiffSyntax<'_>>) -> Operation {
        let mut base = base.view();
        let mut target = target.view();
        let deleted = base.byte_string(0, base.byte_count());
        let inserted = target.byte_string(0, target.byte_count());
        Operation::from_ops([Op::Delete(deleted), Op::Insert(inserted)])
    }
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

/// Derives THE diff markup FROM an operation — deliberately a
/// separate stage from `diff()`: the operation is the diff's truth,
/// the markup its presentation, and presentation-level preferences
/// (whitespace handling, word granularity) parameterize HERE when
/// they arrive, without touching the diff itself. One hunk per
/// maximal non-retain run, grouped like the fragment walk (a retain
/// without a newline stays inside its hunk — probed on the TARGET
/// text, where retained content is identical to the base's), in
/// target coordinates: `DiffAdded`/`DiffModified` spans plus
/// zero-length `DiffDeleted` markers at pure deletions. O(ops +
/// probes) — the normalize worker's job, and a track's birth beside
/// the synchronous first diff.
pub fn hunk_markup(operation: &Operation, right: &Text) -> crate::markup::Markup {
    use crate::theme::StyleId;
    let mut view = right.view();
    let mut hunks: Vec<(Range<u32>, StyleId)> = Vec::new();
    let mut right_at = 0u32;
    // An open hunk: (target start, committed target end, base bytes).
    let mut run: Option<(u32, u32, u32)> = None;
    let mut flush = |run: &mut Option<(u32, u32, u32)>| {
        if let Some((start, end, left_len)) = run.take() {
            let style = match (left_len > 0, end > start) {
                (false, true) => StyleId::DiffAdded,
                (true, false) => StyleId::DiffDeleted,
                _ => StyleId::DiffModified,
            };
            hunks.push((start..end, style));
        }
    };
    for op in operation.iter() {
        match op {
            Op::Retain(len) => {
                if run.is_none() || retain_has_newline(&mut view, right_at, len) {
                    flush(&mut run);
                }
                right_at += len;
            }
            Op::Delete(text) => {
                let (_, end, left_len) = run.get_or_insert((right_at, right_at, 0));
                *end = right_at;
                *left_len += text.len() as u32;
            }
            Op::Insert(text) => {
                right_at += text.len() as u32;
                let (_, end, _) = run.get_or_insert((right_at - text.len() as u32, right_at, 0));
                *end = right_at;
            }
        }
    }
    flush(&mut run);
    let mut markup = crate::markup::Markup::new();
    markup.seed_styled(hunks);
    markup
}

#[cfg(test)]
mod tests;
