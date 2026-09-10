use std::ops::Range;

use operation::{Bias, Op, Operation};

use crate::document_layout::DocumentLayout;

pub(crate) fn sync_spacers(
    left: &mut DocumentLayout,
    right: &mut DocumentLayout,
    diff: &Operation,
    region_left: Range<u32>,
    hunt_limit: usize,
    visits: &mut u64,
) -> Option<Range<u32>> {
    if left.is_unlaid() || right.is_unlaid() {
        return None;
    }

    let left_end = left.byte_size();
    let right_end = diff.transform_offset(region_left.end.min(left_end), Bias::Right);
    let right_size = right.byte_size();
    let right_start = diff.transform_offset(region_left.start, Bias::Left);
    let Some(mut boundary) = first_boundary_at_or_after(right, right_start) else {
        return None;
    };
    let mut hunt = 0usize;
    loop {
        *visits += 1;

        if boundary >= right_end.min(right_size) {
            hunt += 1;
            if hunt > hunt_limit {
                return Some(region_left);
            }
        }
        let mut paired = false;

        if boundary < right_size {
            let mapped = diff.transform_offset_back(boundary, Bias::Right);
            paired = diff.transform_offset(mapped, Bias::Right) == boundary
                && element_starting_at(left, mapped).is_some();
            if !paired {
                reset_spacer(right, boundary);
            }
        }
        if boundary >= right_end.min(right_size) && (paired || boundary >= right_size) {
            break;
        }

        let next = match boundary >= right_end.min(right_size) && !paired {
            true => diff.next_retained_new(boundary).and_then(|run| {
                first_boundary_at_or_after(right, run.start.max(boundary.saturating_add(1)))
            }),
            false => next_boundary_after(right, boundary),
        };
        match next {
            Some(next) => boundary = next,
            None => break,
        }
    }

    let trace = crate::env_flags::trace_diff();
    let Some(mut boundary) = first_boundary_at_or_after(left, region_left.start) else {
        return None;
    };

    let region_end_left = region_left.end.min(left_end);
    let right_target = diff.transform_offset(region_end_left, Bias::Right);
    let mut hunt = 0usize;
    loop {
        *visits += 1;

        if boundary >= region_left.end.min(left_end) {
            hunt += 1;
            if hunt > hunt_limit {
                let from = region_left.end.saturating_sub(1).max(region_left.start);
                return Some(from..region_left.end);
            }
        }
        let mut cleared = false;
        let mut paired = false;
        if boundary < left_end {
            match align_boundary(left, right, diff, boundary) {
                Some(mapped) => {
                    paired = true;

                    cleared = mapped >= right_target
                        && (boundary > region_end_left || mapped > right_target);
                    if trace {
                        eprintln!(
                            "[alignwalk] boundary={boundary} mapped={mapped} \
                             right_target={right_target} cleared={cleared}"
                        );
                    }
                }
                None => {
                    if trace {
                        eprintln!("[alignwalk] boundary={boundary} UNPAIRED (reset left)");
                    }
                    reset_spacer(left, boundary);
                }
            }
        }

        if boundary >= region_left.end.min(left_end) && (cleared || boundary >= left_end) {
            break;
        }

        let next = match boundary >= region_left.end.min(left_end) && !paired {
            true => diff.next_retained_old(boundary).and_then(|run| {
                first_boundary_at_or_after(left, run.start.max(boundary.saturating_add(1)))
            }),
            false => next_boundary_after(left, boundary),
        };
        match next {
            Some(next) => boundary = next,
            None => break,
        }
    }
    None
}

fn first_boundary_at_or_after(layout: &DocumentLayout, byte: u32) -> Option<u32> {
    let probe = byte.min(layout.byte_size().saturating_sub(1));
    let (start, element) = layout.spans_from(probe).next()?;
    Some(match start >= byte {
        true => start,
        false => start.saturating_add(element.byte_size),
    })
}

fn next_boundary_after(layout: &DocumentLayout, boundary: u32) -> Option<u32> {
    let probe = boundary.min(layout.byte_size().saturating_sub(1));
    layout
        .spans_from(probe)
        .map(|(start, element)| start.saturating_add(element.byte_size))
        .find(|end| *end > boundary)
}

fn element_starting_at(layout: &DocumentLayout, byte: u32) -> Option<f32> {
    if byte >= layout.byte_size() {
        return None;
    }
    let (start, element) = layout.spans_from(byte).next()?;
    (start == byte).then_some(element.spacer_above)
}

fn reset_spacer(layout: &mut DocumentLayout, byte: u32) {
    if let Some(spacer) = element_starting_at(layout, byte) {
        if spacer > 0.5 {
            layout.set_spacer(byte, 0.0);
        }
    }
}

fn align_boundary(
    left: &mut DocumentLayout,
    right: &mut DocumentLayout,
    diff: &Operation,
    boundary: u32,
) -> Option<u32> {
    let mapped = diff.transform_offset(boundary, Bias::Right);
    if diff.transform_offset_back(mapped, Bias::Right) != boundary {
        return None;
    }
    let left_spacer_now = element_starting_at(left, boundary)?;
    let right_spacer_now = element_starting_at(right, mapped)?;

    let left_before = left.height_before(boundary) as i64 + decoration_height_at(left, boundary);
    let right_before = right.height_before(mapped) as i64 + decoration_height_at(right, mapped);
    let delta = left_before - right_before;
    let (left_spacer, right_spacer) = match delta {
        0 => (0.0, 0.0),
        d if d > 0 => (0.0, d as f32),
        d => ((-d) as f32, 0.0),
    };
    if (left_spacer_now - left_spacer).abs() > 0.5 {
        left.set_spacer(boundary, left_spacer);
    }
    if (right_spacer_now - right_spacer).abs() > 0.5 {
        right.set_spacer(mapped, right_spacer);
    }
    Some(mapped)
}

fn decoration_height_at(layout: &DocumentLayout, boundary: u32) -> i64 {
    if boundary >= layout.byte_size() {
        return 0;
    }
    let mut first = true;
    let mut sum = 0i64;
    for (start, element) in layout.spans_from(boundary) {
        if start != boundary || element.byte_size != 0 {
            break;
        }
        let spacer = match first {
            true => 0.0,
            false => element.spacer_above,
        };
        sum += (element.height + spacer).ceil().max(0.0) as i64;
        first = false;
    }
    sum
}

pub(crate) fn disagreement(old: &Operation, new: &Operation) -> Option<Range<u32>> {
    let runs = |diff: &Operation| -> Vec<(u32, u32, u32)> {
        let mut runs = Vec::new();
        let (mut l, mut r) = (0u32, 0u32);
        for op in diff.iter() {
            match op {
                Op::Retain(len) => {
                    runs.push((l, r, len));
                    l += len;
                    r += len;
                }
                Op::Delete(text) => l += text.len() as u32,
                Op::Insert(text) => r += text.len() as u32,
            }
        }
        runs
    };
    let old_runs = runs(old);
    let new_runs = runs(new);
    let left_len: u32 = old.iter().map(|op| op.old_len()).sum();

    let mut region: Option<Range<u32>> = None;
    let mut widen = |range: Range<u32>| {
        if range.start >= range.end {
            return;
        }
        region = Some(match region.take() {
            Some(current) => current.start.min(range.start)..current.end.max(range.end),
            None => range,
        });
    };

    let (mut i, mut j) = (0usize, 0usize);
    let mut pos = 0u32;
    while pos < left_len {
        while i < old_runs.len() && old_runs[i].0 + old_runs[i].2 <= pos {
            i += 1;
        }
        while j < new_runs.len() && new_runs[j].0 + new_runs[j].2 <= pos {
            j += 1;
        }
        let state = |runs: &[(u32, u32, u32)], at: usize| -> (Option<i64>, u32) {
            match runs.get(at) {
                Some(&(l, r, len)) if pos >= l => {
                    (Some(i64::from(r) - i64::from(l)), l.saturating_add(len))
                }
                Some(&(l, ..)) => (None, l),
                None => (None, left_len),
            }
        };
        let (old_offset, old_next) = state(&old_runs, i);
        let (new_offset, new_next) = state(&new_runs, j);
        let next = old_next
            .min(new_next)
            .max(pos.saturating_add(1))
            .min(left_len);
        if old_offset != new_offset {
            widen(pos..next);
        }
        pos = next;
    }
    region
}
