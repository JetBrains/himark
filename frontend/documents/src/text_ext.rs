// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The line/column coordinate bridge — the wire and the trees speak
//! `LineCol`, the editor speaks byte offsets.
//!
//! This file once carried a `ChangeObserver` road that recomputed
//! LSP-shaped text changes on every edit and notified an effect —
//! the pre-channel precursor to document sync. Edits now stream as
//! OPERATIONS over the ahp document channel (`hiahp::docsync`'s
//! `SyncSink`, the one installed `ChangeSink`); mirroring them into
//! an LSP server is the HOST's job, against its own mirror of the
//! text. Nothing consumed the observer's effect anymore, so the road
//! is gone.

use text::{LineNumber, TextView};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

pub fn line_col_at(view: &mut TextView, offset: usize) -> LineCol {
    let line = view.line_at(offset);
    let start = view.line_start_offset(line);
    LineCol {
        line: line.0 as u32,
        col: (offset - start) as u32,
    }
}

pub fn offset_at(view: &mut TextView, position: LineCol) -> usize {
    let last = view.line_count().0 - 1;
    let line = LineNumber((position.line as usize).min(last));
    let start = view.line_start_offset(line);
    let end = view.line_end_offset(line);
    let content_end = match line.0 < last {
        true => end - 1,
        false => end,
    };
    (start + position.col as usize).min(content_end)
}
