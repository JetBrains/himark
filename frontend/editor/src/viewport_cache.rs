// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The viewport line memo: between two builds whose INPUTS match
//! (revision, markup generation, theme, width, focus, selections,
//! IME and hover state), a visible line's derivation is a pure
//! function of its byte position — so a scroll-moved band re-derives
//! only the newly exposed lines. The walk itself (layout cursor,
//! tops, spacers) still runs every build: it is the cheap part, and
//! it is what decides WHICH lines exist.
//!
//! Known cosmetic staleness: a reused line's fold-chip spin is the
//! frame it was derived on; the toggle that starts the spin changes
//! the markup (folds are hidden ranges) and rebuilds everything, so
//! only the animation's tail can freeze, and only on untouched rows.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;

use crate::viewport::ViewportLine;

#[derive(PartialEq)]
pub(crate) struct BuildStamp {
    pub token: crate::document::DocumentToken,
    pub editor: crate::editor::EditorId,
    pub revision: u64,
    pub markup_generation: u64,
    pub theme: std::sync::Arc<str>,
    pub width_bits: u32,
    pub focused: bool,
    pub gutter: bool,
    pub stripes: Option<crate::diff::DiffId>,
    pub selections: Vec<Range<u32>>,
    pub marked: Option<Range<u32>>,
    pub hovered: Option<Range<u32>>,
}

struct Entry {
    stamp: BuildStamp,
    /// Sorted by `byte_start`; `inline`/`hidden` ranges zeroed — they
    /// index the build-local scratch and die with it.
    lines: Vec<ViewportLine>,
}

thread_local! {
    static CACHE: RefCell<HashMap<(crate::document::DocumentToken, crate::editor::EditorId), Entry>> =
        RefCell::new(HashMap::new());
}

/// The previous build's lines, when every input matches — the entry
/// leaves the cache either way (a fresh one lands after the build).
pub(crate) fn take(stamp: &BuildStamp) -> Option<Vec<ViewportLine>> {
    CACHE.with(|cell| {
        let entry = cell.borrow_mut().remove(&(stamp.token, stamp.editor))?;
        (&entry.stamp == stamp).then_some(entry.lines)
    })
}

/// A build's lines become the next build's memo.
pub(crate) fn store(stamp: BuildStamp, lines: &[ViewportLine]) {
    let lines = lines
        .iter()
        .map(|line| ViewportLine {
            inline: 0..0,
            hidden: 0..0,
            ..line.clone()
        })
        .collect();
    CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        if cache.len() > 32 {
            cache.clear();
        }
        cache.insert((stamp.token, stamp.editor), Entry { stamp, lines });
    });
}

/// The memo's answer for one walked line: the geometry must agree
/// bit-for-bit with what this walk derived — a mismatch is a miss,
/// never a judgement call.
pub(crate) fn hit(
    memo: &Option<Vec<ViewportLine>>,
    byte_start: u32,
    byte_end: u32,
    top: f32,
    height: f32,
    spacer: f32,
) -> Option<ViewportLine> {
    let lines = memo.as_ref()?;
    let at = lines
        .binary_search_by_key(&byte_start, |line| line.byte_start)
        .ok()?;
    let line = &lines[at];
    (line.byte_end == byte_end
        && line.top.to_bits() == top.to_bits()
        && line.height.to_bits() == height.to_bits()
        && line.spacer.to_bits() == spacer.to_bits())
    .then(|| line.clone())
}
