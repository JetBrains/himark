// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::{Effect, Effects},
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View,
};
use operation::{Bias, Op, Operation};
use skia_safe::Size;
use text::Text;

use crate::diff::FragmentKind;
use crate::editor_view::{EditorCommand, EditorView};
use crate::theme::StyleId;

const CENTER_GAP: f32 = 2.0;

const MARK_FRAGMENT_CAP: usize = 512;

const MARK_SLACK_PX: f32 = 2_000.0;

#[derive(Clone)]
pub struct DiffState {
    id: crate::diff::DiffId,

    diff: Operation,

    seen_generation: u64,
    left_revision: u64,
    right_revision: u64,

    left_marks: crate::markup::MarkupId,
    right_marks: crate::markup::MarkupId,

    /// THE diff markup (`Diff.markup`) — read-only from the pane's
    /// side; the diff machinery maintains it.
    hunks: crate::markup::MarkupId,

    unified_layout: crate::unified_diff::DiffLayout,
    inline_editor: Option<crate::editor::EditorId>,

    align_pending: Option<Range<u32>>,

    #[cfg(any(test, feature = "test-support"))]
    pub ui_synced_boundaries: u64,

    pair_repair_token: Option<imba::effect::CancellationToken>,

    pair_seq: u64,

    fold_phase: fold::FoldPhase,

    marks_dirty: bool,

    marks_window: Option<Range<u32>>,
}

impl DiffState {
    pub fn attach(
        id: crate::diff::DiffId,
        left: &crate::Document,
        right: &crate::Document,
        left_marks: crate::markup::MarkupId,
        right_marks: crate::markup::MarkupId,
        seeded: Option<Range<u32>>,
    ) -> Option<Self> {
        let mut entry = right.diff(id)?.clone();
        let operation = match entry.apply_base_edits(left.log()) {
            true => entry.operation().clone(),

            false => crate::diff::diff(left.text(), right.text()),
        };
        Some(Self {
            id,
            diff: operation,
            seen_generation: entry.generation(),
            left_revision: left.revision(),
            right_revision: right.revision(),
            left_marks,
            right_marks,
            hunks: entry.markup(),

            align_pending: Some(0..left.text().byte_count() as u32),
            unified_layout: crate::unified_diff::DiffLayout::Split,
            inline_editor: None,
            #[cfg(any(test, feature = "test-support"))]
            ui_synced_boundaries: 0,
            pair_repair_token: None,
            pair_seq: 0,
            fold_phase: match fold::FOLDS_ENABLED {
                true if seeded.is_some() => fold::FoldPhase::Done,

                true if entry.generation() > 0 => fold::FoldPhase::Owed,
                true => fold::FoldPhase::Waiting,
                false => fold::FoldPhase::Done,
            },
            marks_dirty: seeded.is_none(),
            marks_window: seeded,
        })
    }

    pub fn unified_layout(&self) -> crate::unified_diff::DiffLayout {
        self.unified_layout
    }

    pub fn inline_editor(&self) -> Option<crate::editor::EditorId> {
        self.inline_editor
    }

    pub(crate) fn set_unified(
        &mut self,
        layout: crate::unified_diff::DiffLayout,
        inline_editor: Option<crate::editor::EditorId>,
    ) {
        self.unified_layout = layout;
        self.inline_editor = inline_editor;
    }

    pub(crate) fn right_marks(&self) -> crate::markup::MarkupId {
        self.right_marks
    }

    /// THE diff markup on the target document (docs/scroll-stripe.md
    /// §7) — the hunk washes the halves show; maintained by the diff
    /// machinery, never by this pane.
    pub(crate) fn hunk_markup(&self) -> crate::markup::MarkupId {
        self.hunks
    }

    #[doc(hidden)]
    pub fn right_marks_oracle(&self) -> crate::markup::MarkupId {
        self.right_marks
    }

    #[doc(hidden)]
    pub fn hunk_markup_oracle(&self) -> crate::markup::MarkupId {
        self.hunks
    }

    pub fn diff_id(&self) -> crate::diff::DiffId {
        self.id
    }

    pub fn diff(&self) -> &Operation {
        &self.diff
    }

    pub fn mark_markups(&self) -> (crate::markup::MarkupId, crate::markup::MarkupId) {
        (self.left_marks, self.right_marks)
    }
}

#[derive(Clone)]
pub struct SplitDiffView {
    pub left: EditorView,
    pub right: EditorView,
    pub state: DiffState,
}

pub enum SplitDiffCommand {
    Left(EditorCommand),
    Right(EditorCommand),

    PairRepaired {
        left: crate::repair::RepairedLayout,
        right: crate::repair::RepairedLayout,

        align: Option<Range<u32>>,

        marks: Option<MarksLanding>,

        seq: u64,
    },

    Resync,
}

pub type SplitDiffEffects<'a> = Effects<'a, SplitDiffCommand>;

pub(crate) mod align;
pub(crate) mod fold;

fn trace_diff(message: impl FnOnce() -> String) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("HIMARK_TRACE_DIFF").is_some()) {
        eprintln!("[diffsync] {}", message());
    }
}

impl SplitDiffView {
    pub fn new(left: EditorView, right: EditorView, state: DiffState) -> Self {
        Self { left, right, state }
    }

    fn roll_forward(&mut self) -> Option<(Option<Range<u32>>, bool)> {
        let mut rolled_any = false;
        let mut region: Option<Range<u32>> = None;
        let mut widen = |range: Range<u32>| {
            region = Some(match region.take() {
                Some(current) => current.start.min(range.start)..current.end.max(range.end),
                None => range,
            });
        };

        if self.left.document.revision() != self.state.left_revision {
            let a = self
                .left
                .document
                .log()
                .compose_since(self.state.left_revision)?;
            if let Some(span) = affected_span(&a) {
                widen(span);
            }
            self.state.diff = a.invert().splice_compose_into(&self.state.diff);
            self.state.left_revision = self.left.document.revision();
            rolled_any = true;
        }
        if self.right.document.revision() != self.state.right_revision {
            let b = self
                .right
                .document
                .log()
                .compose_since(self.state.right_revision)?;
            if let Some(span) = affected_span(&b) {
                let start = self
                    .state
                    .diff
                    .transform_offset_back(span.start, Bias::Left);
                let end = self.state.diff.transform_offset_back(span.end, Bias::Right);
                widen(start..end.max(start));
            }
            self.state.diff = self.state.diff.splice_compose(&b);
            self.state.right_revision = self.right.document.revision();
            rolled_any = true;
        }
        Some((region, rolled_any))
    }

    fn settle(&mut self, left_hint: Option<Range<u32>>, right_hint: Option<Range<u32>>) {
        let (mut region, text_moved) = match self.roll_forward() {
            Some((rolled, moved)) => (rolled, moved),
            None => {
                self.state.diff =
                    crate::diff::diff(self.left.document.text(), self.right.document.text());
                self.state.left_revision = self.left.document.revision();
                self.state.right_revision = self.right.document.revision();
                self.state.marks_dirty = true;
                self.state.marks_window = None;
                self.state.align_pending = Some(0..self.left.document.text().byte_count() as u32);
                (None, false)
            }
        };
        let mut widen = |range: Range<u32>| {
            region = Some(match region.take() {
                Some(current) => current.start.min(range.start)..current.end.max(range.end),
                None => range,
            });
        };

        if let Some(hint) = left_hint {
            widen(hint);
        }
        if let Some(hint) = right_hint {
            let start = self
                .state
                .diff
                .transform_offset_back(hint.start, Bias::Left);
            let end = self.state.diff.transform_offset_back(hint.end, Bias::Right);
            widen(start..end.max(start));
        }

        if let Some(region) = region {
            const UI_BAND_CAP: u32 = 256 * 1024;
            if region.end.saturating_sub(region.start) > UI_BAND_CAP {
                trace_diff(|| format!("settle defers oversized {region:?}"));
                self.sync_visible_owe_rest(region);
            } else {
                trace_diff(|| format!("settle syncs {region:?}"));
                self.sync_region(region);
            }
        }

        if text_moved {
            self.state.marks_dirty = true;
        }

        self.adopt_normalized();
    }

    fn adopt_normalized(&mut self) {
        let id = self.state.id;
        if !self
            .right
            .document
            .apply_diff_base_edits(id, self.left.document.log())
        {
            return;
        }
        let Some(entry) = self.right.document.diff(id) else {
            return;
        };
        if entry.generation() == self.state.seen_generation {
            return;
        }
        let generation = entry.generation();
        let fresh = entry.operation().clone();
        let owed = align::disagreement(&self.state.diff, &fresh);
        trace_diff(|| format!("adopt generation {generation} disagreement={owed:?}"));
        self.state.diff = fresh;
        self.state.left_revision = self.left.document.revision();
        self.state.right_revision = self.right.document.revision();
        self.state.seen_generation = generation;
        self.state.marks_dirty = true;

        if self.state.fold_phase == fold::FoldPhase::Waiting {
            self.state.fold_phase = fold::FoldPhase::Owed;
        }
        if let Some(owed) = owed {
            self.sync_visible_owe_rest(owed);
        }
    }

    fn viewport_bytes_left(&self) -> Option<Range<u32>> {
        let viewport = self.left.document.viewport(self.left.editor)?;
        let layout = &self.left.document.editors.get(&self.left.editor)?.layout;
        let start = layout.byte_at_y((viewport.start - MARK_SLACK_PX).max(0.0));
        let end = layout.byte_at_y(viewport.end + MARK_SLACK_PX);

        Some(start..end.saturating_add(4 * 1024))
    }

    fn viewport_bytes_right(&self) -> Option<Range<u32>> {
        let viewport = self.right.document.viewport(self.right.editor)?;
        let layout = &self.right.document.editors.get(&self.right.editor)?.layout;
        let start = layout.byte_at_y((viewport.start - MARK_SLACK_PX).max(0.0));
        let end = layout
            .byte_at_y(viewport.end + MARK_SLACK_PX)
            .saturating_add(4 * 1024);
        let start = self.state.diff.transform_offset_back(start, Bias::Left);
        let end = self.state.diff.transform_offset_back(end, Bias::Right);
        Some(start..end.max(start))
    }

    fn widen_align_pending(&mut self, range: Range<u32>) {
        if range.start >= range.end {
            return;
        }
        self.state.align_pending = Some(match self.state.align_pending.take() {
            Some(current) => current.start.min(range.start)..current.end.max(range.end),
            None => range,
        });
    }

    fn capture_pair_repair(&mut self) -> RepairDiffEffect {
        let side = |view: &EditorView| {
            let document = &view.document;
            let state = &document.editors[&view.editor];
            match crate::repair::RepairEffect::collect(
                document,
                std::iter::once((&view.editor, state)),
            ) {
                Some(effect) => PairSide::Repair(effect),
                None => PairSide::Layout(crate::repair::RepairedLayout {
                    editor: view.editor,
                    revision: document.revision(),
                    markup_generation: document.markup_generation(),
                    width: state.layout.layout_width(),
                    layout: state.layout.clone(),
                }),
            }
        };

        let widths_match = self.widths_match();
        let align = match widths_match {
            true => self.state.align_pending.take(),
            false => None,
        };
        let window = self.marks_window_now();
        trace_diff(|| {
            let width = |view: &EditorView| {
                view.document
                    .document_layout(view.editor)
                    .map_or(0.0, |layout| layout.layout_width())
            };
            format!(
                "capture widths={:.1}/{:.1} marks_dirty={} window={window:?} standing={:?}",
                width(&self.left),
                width(&self.right),
                self.state.marks_dirty,
                self.state.marks_window
            )
        });
        let marks = (widths_match
            && (self.state.marks_dirty || self.state.marks_window.as_ref() != Some(&window)))
        .then(|| MarksJob {
            left_text: self.left.document.text().clone(),
            left_current: self
                .left
                .document
                .feature_markup(self.state.left_marks)
                .cloned(),
            right_current: self
                .right
                .document
                .feature_markup(self.state.right_marks)
                .cloned(),
            derive_folds: self.state.fold_phase == fold::FoldPhase::Owed,
            window,
        });
        RepairDiffEffect {
            left: side(&self.left),
            right: side(&self.right),
            diff: self.state.diff.clone(),
            align,
            marks,
            seq: self.state.pair_seq,
        }
    }

    pub(crate) fn pair_lane(&mut self, fx: &mut SplitDiffEffects<'_>) {
        let pending = |view: &EditorView| {
            view.document
                .document_layout(view.editor)
                .is_some_and(|layout| layout.repair_pending().is_some())
        };
        let window = self.marks_window_now();
        let marks_owed =
            self.state.marks_dirty || self.state.marks_window.as_ref() != Some(&window);

        let widths_match = self.widths_match();
        if pending(&self.left)
            || pending(&self.right)
            || (widths_match && (self.state.align_pending.is_some() || marks_owed))
        {
            let capture = self.capture_pair_repair();
            fx.relaunch(&mut self.state.pair_repair_token, capture);
        }
    }

    fn widths_match(&self) -> bool {
        let width = |view: &EditorView| {
            view.document
                .document_layout(view.editor)
                .map_or(0.0, |layout| layout.layout_width())
        };
        (width(&self.left) - width(&self.right)).abs() <= 1.0
    }

    fn sync_visible_owe_rest(&mut self, owed: Range<u32>) {
        const UI_OWE_SYNC_CAP: u32 = 256 * 1024;
        if owed.start >= owed.end {
            return;
        }
        let window = match (self.viewport_bytes_left(), self.viewport_bytes_right()) {
            (Some(left), Some(right)) => Some(left.start.min(right.start)..left.end.max(right.end)),
            (window, None) | (None, window) => window,
        };
        trace_diff(|| format!("owe_rest owed={owed:?} window={window:?}"));
        let visible = match window {
            Some(window) => owed.start.max(window.start)..owed.end.min(window.end),

            None => owed.clone(),
        };
        let visible = visible.start
            ..visible
                .end
                .min(visible.start.saturating_add(UI_OWE_SYNC_CAP));
        if visible.start < visible.end {
            self.sync_region(visible);
        }
        self.widen_align_pending(owed);
    }

    pub(crate) fn adjust_fold(
        &mut self,
        key: crate::markup::InlayKey,
        command: fold::FoldCommand,
        store: &Store,
        ui: &UiCtx,
        fx: &mut SplitDiffEffects<'_>,
    ) {
        let strip = key.key;
        let left_key = crate::markup::InlayKey {
            layer: crate::markup::MarkupLayer::Markup(self.state.left_marks),
            key: strip,
        };
        let right_key = crate::markup::InlayKey {
            layer: crate::markup::MarkupLayer::Markup(self.state.right_marks),
            key: strip,
        };
        let Some((left_range, _)) = self
            .left
            .document
            .feature_markup(self.state.left_marks)
            .and_then(|markup| markup.inlay_interval(strip))
        else {
            return;
        };
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);

        if matches!(command, fold::FoldCommand::Remove) {
            let left = &mut self.left;
            Self::half_scope(fx, SplitDiffCommand::Left, |fx| {
                left.document.remove_inlay(left_key, &fonts, &theme, fx)
            });
            let right = &mut self.right;
            Self::half_scope(fx, SplitDiffCommand::Right, |fx| {
                right.document.remove_inlay(right_key, &fonts, &theme, fx)
            });
            self.settle_after(Some(left_range), None);
            return self.pair_lane(fx);
        }

        let Some(spec) = fold::derive_folds(
            &self.state.diff,
            self.left.document.text(),
            left_range.clone(),
            fold::FOLD_CONTEXT,
        )
        .into_iter()
        .find(|spec| spec.left.start < left_range.end && left_range.start < spec.left.end) else {
            return;
        };

        let mut scan = fold::LineScan::new(self.left.document.text());
        let len = self
            .left
            .document
            .text()
            .view()
            .byte_count()
            .min(u32::MAX as usize) as u32;
        let mut start = left_range.start;
        let mut end = left_range.end;
        match command {
            fold::FoldCommand::RevealTop => {
                for _ in 0..fold::FOLD_STEP {
                    match scan.next_line_start(start, end) {
                        Some(next) => start = next,
                        None => {
                            start = end;
                            break;
                        }
                    }
                }
            }
            fold::FoldCommand::HideTop => {
                for _ in 0..fold::FOLD_STEP {
                    match scan.previous_line_start(start) {
                        Some(previous) if previous >= spec.left.start => start = previous,
                        _ => break,
                    }
                }
            }
            fold::FoldCommand::RevealBottom => {
                for _ in 0..fold::FOLD_STEP {
                    match scan.previous_line_start(end) {
                        Some(previous) if previous > start => end = previous,
                        _ => {
                            end = start;
                            break;
                        }
                    }
                }
            }
            fold::FoldCommand::HideBottom => {
                for _ in 0..fold::FOLD_STEP {
                    match scan.next_line_start(end, spec.left.end.saturating_add(1)) {
                        Some(next) if next <= spec.left.end => end = next,
                        _ => break,
                    }
                }
                end = end.min(spec.left.end).min(len);
            }
            fold::FoldCommand::Remove => unreachable!("handled above"),
        }

        if end > start {
            let lines = scan.count_lines(start, end);
            let spacer = crate::markup::Inlay::new(
                crate::markup::InlayMode::Instead(crate::markup::InsteadKind::FullLine),
                fold::FoldStrip::spacer(lines),
            );
            let strip = crate::markup::Inlay::new(
                crate::markup::InlayMode::Instead(crate::markup::InsteadKind::FullLine),
                fold::FoldStrip::new(lines),
            )
            .over(crate::markup::INLAY_HOST);
            let offset = start - spec.left.start;
            let right_start = spec.right.start + offset;
            let right_range = right_start..right_start + (end - start);
            let left = &mut self.left;
            Self::half_scope(fx, SplitDiffCommand::Left, |fx| {
                left.document
                    .replace_inlay(left_key, start..end, spacer, &fonts, &theme, fx)
            });
            let right = &mut self.right;
            Self::half_scope(fx, SplitDiffCommand::Right, |fx| {
                right
                    .document
                    .replace_inlay(right_key, right_range, strip, &fonts, &theme, fx)
            });
        } else {
            let left = &mut self.left;
            Self::half_scope(fx, SplitDiffCommand::Left, |fx| {
                left.document.remove_inlay(left_key, &fonts, &theme, fx)
            });
            let right = &mut self.right;
            Self::half_scope(fx, SplitDiffCommand::Right, |fx| {
                right.document.remove_inlay(right_key, &fonts, &theme, fx)
            });
        }

        let touched = left_range.start.min(spec.left.start)..left_range.end.max(spec.left.end);
        self.settle_after(Some(touched), None);
        self.pair_lane(fx)
    }

    fn half_scope<T>(
        fx: &mut SplitDiffEffects<'_>,
        wrap: impl Fn(EditorCommand) -> SplitDiffCommand + Send + Clone + 'static,
        f: impl FnOnce(&mut Effects<'_, EditorCommand>) -> T,
    ) -> T {
        fx.scope_filtered(
            wrap,
            |effect| !effect.is::<crate::repair::RepairEffect>(),
            f,
        )
    }

    pub(crate) fn settle_after(
        &mut self,
        extra_left: Option<Range<u32>>,
        extra_right: Option<Range<u32>>,
    ) {
        let union = |a: Option<Range<u32>>, b: Option<Range<u32>>| match (a, b) {
            (Some(a), Some(b)) => Some(a.start.min(b.start)..a.end.max(b.end)),
            (a, None) | (None, a) => a,
        };
        let left_editor = self.left.editor;
        let right_editor = self.right.editor;
        let left_hint = union(extra_left, self.left.document.take_healed(left_editor));
        let right_hint = union(extra_right, self.right.document.take_healed(right_editor));
        trace_diff(|| format!("settle_after left_hint={left_hint:?} right_hint={right_hint:?}"));
        self.settle(left_hint, right_hint)
    }

    fn sync_region(&mut self, region: Range<u32>) {
        let left_editor = self.left.editor;
        let right_editor = self.right.editor;
        let Some(left_state) = self.left.document.editors.get_mut(&left_editor) else {
            #[cfg(debug_assertions)]
            eprintln!("[diff] sync skipped: left editor missing");
            return;
        };
        let left_layout = &mut left_state.layout;
        let Some(right_state) = self.right.document.editors.get_mut(&right_editor) else {
            #[cfg(debug_assertions)]
            eprintln!("[diff] sync skipped: right editor missing");
            return;
        };
        let right_layout = &mut right_state.layout;
        if (left_layout.layout_width() - right_layout.layout_width()).abs() > 1.0 {
            #[cfg(debug_assertions)]
            eprintln!(
                "[diff] sync skipped: widths differ {} vs {}",
                left_layout.layout_width(),
                right_layout.layout_width()
            );
            return;
        }

        const UI_HUNT_LIMIT: usize = 32;
        let mut visits = 0u64;
        let leftover = align::sync_spacers(
            left_layout,
            right_layout,
            &self.state.diff,
            region.clone(),
            UI_HUNT_LIMIT,
            &mut visits,
        );
        #[cfg(any(test, feature = "test-support"))]
        {
            self.state.ui_synced_boundaries += visits;
        }
        #[cfg(not(any(test, feature = "test-support")))]
        let _ = visits;
        trace_diff(|| format!("sync_region {region:?} leftover={leftover:?}"));
        if let Some(leftover) = leftover {
            self.widen_align_pending(leftover);
        }
    }

    fn marks_window_now(&self) -> Range<u32> {
        let exact = match (self.viewport_bytes_left(), self.viewport_bytes_right()) {
            (Some(a), Some(b)) => a.start.min(b.start)..a.end.max(b.end),
            (Some(a), None) | (None, Some(a)) => a,

            (None, None) => self.state.marks_window.clone().unwrap_or(0..64 * 1024),
        };
        const BLOCK: u32 = 4 * 1024;
        (exact.start / BLOCK * BLOCK)..exact.end.div_ceil(BLOCK).saturating_mul(BLOCK)
    }
}

fn derive_wash_markups(
    diff: &Operation,
    left_text: &Text,
    window: &Range<u32>,
) -> (crate::markup::Markup, crate::markup::Markup) {
    let mut left_markup = crate::markup::Markup::new();
    let mut right_markup = crate::markup::Markup::new();

    // The RIGHT half's line washes are THE diff markup's now
    // (docs/scroll-stripe.md §7 — maintained by the diff machinery,
    // whole-document); this pane derives only what stays its own:
    // the base side's washes and the windowed word tints.
    for fragment in crate::diff::fragments_at(diff, left_text, window.start).take(MARK_FRAGMENT_CAP)
    {
        if fragment.left.start > window.end {
            break;
        }
        match fragment.kind {
            FragmentKind::Added => {}
            FragmentKind::Deleted => {
                left_markup.push_styled(fragment.left.clone(), StyleId::DiffDeleted);
            }
            FragmentKind::Modified => {
                left_markup.push_styled(fragment.left.clone(), StyleId::DiffDeleted);
                for (left, right) in &fragment.words {
                    if left.start < left.end {
                        left_markup.push_styled(left.clone(), StyleId::DiffDeletedWord);
                    }
                    if right.start < right.end {
                        right_markup.push_styled(right.clone(), StyleId::DiffAddedWord);
                    }
                }
            }
        }
    }
    (left_markup, right_markup)
}

fn mint_fold_strips(
    diff: &Operation,
    left_text: &Text,
    left_markup: &mut crate::markup::Markup,
    right_markup: &mut crate::markup::Markup,
) {
    let len = left_text.byte_count().min(u32::MAX as usize) as u32;
    let specs = fold::derive_folds(diff, left_text, 0..len, fold::FOLD_CONTEXT);
    for (n, spec) in specs.iter().enumerate() {
        let key = crate::markup::IntervalId(u32::MAX - n as u32);
        // The left pane carries a silent spacer for aligned heights;
        // the right pane's strip is the shared, interactive face,
        // projected onto the pane-wide (split-wide) overlay host.
        let spacer = crate::markup::Inlay::new(
            crate::markup::InlayMode::Instead(crate::markup::InsteadKind::FullLine),
            fold::FoldStrip::spacer(spec.lines),
        );
        let strip = crate::markup::Inlay::new(
            crate::markup::InlayMode::Instead(crate::markup::InsteadKind::FullLine),
            fold::FoldStrip::new(spec.lines),
        )
        .over(crate::markup::INLAY_HOST);
        left_markup.replace_inlay(key, spec.left.clone(), spacer);
        right_markup.replace_inlay(key, spec.right.clone(), strip);
    }
}

#[derive(Clone)]
pub struct PreparedMarks {
    pub left: crate::markup::Markup,
    pub right: crate::markup::Markup,

    pub window: Range<u32>,
}

pub fn prepare_marks(diff: &Operation, left_text: &Text) -> PreparedMarks {
    let len = left_text.byte_count().min(u32::MAX as usize) as u32;
    const BLOCK: u32 = 4 * 1024;
    let window = 0..len.div_ceil(BLOCK).saturating_mul(BLOCK);
    let (mut left, mut right) = derive_wash_markups(diff, left_text, &window);
    if fold::FOLDS_ENABLED {
        mint_fold_strips(diff, left_text, &mut left, &mut right);
    }
    PreparedMarks {
        left,
        right,
        window,
    }
}

impl View for SplitDiffView {
    type Command = SplitDiffCommand;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: SplitDiffCommand,
        fx: &mut SplitDiffEffects<'_>,
    ) {
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        match command {
            SplitDiffCommand::Left(command) => {
                if let EditorCommand::Inlay { key, command } = &command {
                    if let Some(fold_command) = command.downcast_ref::<fold::FoldCommand>() {
                        return self.adjust_fold(*key, *fold_command, store, ui, fx);
                    }
                }

                if matches!(command, EditorCommand::Click { .. }) {
                    self.right.blur();
                }
                let left = &mut self.left;
                Self::half_scope(fx, SplitDiffCommand::Left, |fx| {
                    left.perform(store, ui, command, fx)
                });

                self.settle_after(None, None);
                self.pair_lane(fx)
            }
            SplitDiffCommand::Right(command) => {
                if let EditorCommand::Inlay { key, command } = &command {
                    if let Some(fold_command) = command.downcast_ref::<fold::FoldCommand>() {
                        return self.adjust_fold(*key, *fold_command, store, ui, fx);
                    }
                }
                if matches!(command, EditorCommand::Click { .. }) {
                    self.left.blur();
                }
                let right = &mut self.right;
                Self::half_scope(fx, SplitDiffCommand::Right, |fx| {
                    right.perform(store, ui, command, fx)
                });
                self.settle_after(None, None);
                self.pair_lane(fx)
            }
            SplitDiffCommand::PairRepaired {
                left,
                right,
                align,
                marks,
                seq,
            } => {
                let landable = seq == self.state.pair_seq
                    && self.left.document.repair_landable(&left)
                    && self.right.document.repair_landable(&right);
                trace_diff(|| {
                    format!(
                        "pair landing seq={seq}/{} landable={landable} marks={}",
                        self.state.pair_seq,
                        marks.is_some()
                    )
                });
                match landable {
                    true => {
                        self.state.pair_seq += 1;
                        let moved = self.left.document.apply_repair_anchored(left)
                            | self.right.document.apply_repair_anchored(right);
                        if moved {
                            fx.settle();
                        }

                        if let Some(fold) = self.left.document.take_swap_fold(self.left.editor) {
                            self.widen_align_pending(fold);
                        }
                        if let Some(fold) = self.right.document.take_swap_fold(self.right.editor) {
                            let start = self
                                .state
                                .diff
                                .transform_offset_back(fold.start, Bias::Left);
                            let end = self.state.diff.transform_offset_back(fold.end, Bias::Right);
                            self.widen_align_pending(start..end.max(start));
                        }

                        if let Some(marks) = marks {
                            let left_marks = self.state.left_marks;
                            let left = &mut self.left;
                            Self::half_scope(fx, SplitDiffCommand::Left, |fx| {
                                left.document.replace_markup(
                                    left_marks,
                                    marks.left_markup,
                                    &marks.left_changed,
                                    &fonts,
                                    &theme,
                                    fx,
                                )
                            });
                            let right_marks = self.state.right_marks;
                            let right = &mut self.right;
                            Self::half_scope(fx, SplitDiffCommand::Right, |fx| {
                                right.document.replace_markup(
                                    right_marks,
                                    marks.right_markup,
                                    &marks.right_changed,
                                    &fonts,
                                    &theme,
                                    fx,
                                )
                            });
                            self.state.marks_window = Some(marks.window);
                            self.state.marks_dirty = false;
                            if marks.derived_folds {
                                self.state.fold_phase = fold::FoldPhase::Done;
                            }
                        }

                        let left_editor = self.left.editor;
                        let right_editor = self.right.editor;
                        if let Some(healed) = self.left.document.take_healed(left_editor) {
                            self.sync_visible_owe_rest(healed);
                        }
                        if let Some(healed) = self.right.document.take_healed(right_editor) {
                            let start = self
                                .state
                                .diff
                                .transform_offset_back(healed.start, Bias::Left);
                            let end = self
                                .state
                                .diff
                                .transform_offset_back(healed.end, Bias::Right);
                            self.sync_visible_owe_rest(start..end.max(start));
                        }
                    }
                    false => {
                        if let Some(owed) = align {
                            self.widen_align_pending(owed);
                        }
                        if marks.is_some() {
                            self.state.marks_dirty = true;
                        }
                    }
                }
                self.settle_after(None, None);
                self.pair_lane(fx)
            }
            SplitDiffCommand::Resync => {
                self.settle_after(None, None);
                self.pair_lane(fx)
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let width = constraints.max.width;
            let half = ((width - CENTER_GAP) * 0.5).max(1.0);
            let half_constraints = Constraints {
                min: Size::new(half, 0.0),
                max: Size::new(half, f32::MAX),
            };
            let left =
                imba::Layout::layout(self.left.display(arena, store, ui), arena, half_constraints)
                    .map(SplitDiffCommand::Left);
            let right = imba::Layout::layout(
                self.right.display(arena, store, ui),
                arena,
                half_constraints,
            )
            .map(SplitDiffCommand::Right);
            let height = left.size().height.max(right.size().height);
            let divider = crate::env::Themes::of(store).ui().window.divider.0;
            let mut pair = container(arena, Size::new(width, height));
            pair.place(0.0, 0.0, left);
            pair.place(half + CENTER_GAP, 0.0, right);

            let focused = if self.left.focus() != crate::editor_view::EditorFocus::None {
                Some(0)
            } else if self.right.focus() != crate::editor_view::EditorFocus::None {
                Some(1)
            } else {
                None
            };
            let pair = pair.wrap_realized(move |pair| PairChain { pair, focused });

            pair.paint_above(move |_arena, canvas, rect| {
                let mut paint = skia_safe::Paint::default();
                paint.set_color(divider);
                canvas.draw_rect(
                    skia_safe::Rect::from_xywh(
                        rect.left + half + CENTER_GAP * 0.5 - 0.5,
                        rect.top,
                        1.0,
                        rect.height(),
                    ),
                    &paint,
                );
            })
            // The pair hosts the projected inlays ONCE, above both
            // panes: the right pane's fold strip lands here and spans
            // the whole split, gutters included.
            .overlay_host(crate::markup::INLAY_HOST)
        })
    }
}

struct PairChain<'a> {
    pair: imba::container::RealizedContainer<'a, SplitDiffCommand>,
    focused: Option<usize>,
}

impl<'a> imba::Widget<'a, SplitDiffCommand> for PairChain<'a> {
    fn size(&self) -> Size {
        imba::Widget::size(&self.pair)
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &imba::event::Event<'_>,
        viewport: skia_safe::Rect,
    ) -> imba::event::EventResult<SplitDiffCommand> {
        self.pair.handle_event(arena, event, viewport)
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, SplitDiffCommand>> {
        self.pair.overlays()
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, SplitDiffCommand>
    where
        'a: 'w,
    {
        match self.focused {
            Some(index) => self.pair.focus_data_of(index),
            None => imba::focus::FocusData::default(),
        }
    }
}

struct MarksJob {
    window: Range<u32>,
    left_text: Text,

    left_current: Option<crate::markup::Markup>,
    right_current: Option<crate::markup::Markup>,

    derive_folds: bool,
}

pub struct MarksLanding {
    window: Range<u32>,
    left_markup: crate::markup::Markup,
    right_markup: crate::markup::Markup,
    left_changed: Vec<Range<u32>>,
    right_changed: Vec<Range<u32>>,

    derived_folds: bool,
}

enum PairSide {
    Repair(crate::repair::RepairEffect),
    Layout(crate::repair::RepairedLayout),
}

pub struct RepairDiffEffect {
    left: PairSide,
    right: PairSide,
    diff: Operation,

    align: Option<Range<u32>>,

    marks: Option<MarksJob>,

    seq: u64,
}

pub struct RepairDiffHandler(pub std::sync::Arc<crate::env::Workshop>);

impl imba::effect::EffectHandler<RepairDiffEffect> for RepairDiffHandler {
    async fn handle(&self, effect: RepairDiffEffect) -> SplitDiffCommand {
        let repairs = crate::repair::RepairHandler(std::sync::Arc::clone(&self.0));
        let resolve = |side: PairSide| match side {
            PairSide::Repair(inner) => repairs
                .repairs(inner)
                .pop()
                .expect("the capture holds exactly one editor"),
            PairSide::Layout(layout) => layout,
        };
        let mut left = resolve(effect.left);
        let mut right = resolve(effect.right);

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
        if let Some(owed) = effect.align.clone() {
            widen(owed);
        }
        if let Some(rebuilt) = left.layout.take_healed() {
            widen(rebuilt);
        }
        if let Some(rebuilt) = right.layout.take_healed() {
            let start = effect.diff.transform_offset_back(rebuilt.start, Bias::Left);
            let end = effect.diff.transform_offset_back(rebuilt.end, Bias::Right);
            widen(start..end.max(start));
        }
        if let Some(region) = region {
            let _ = align::sync_spacers(
                &mut left.layout,
                &mut right.layout,
                &effect.diff,
                region,
                usize::MAX,
                &mut 0,
            );
        }

        let _ = left.layout.take_stale_for_swap();
        let _ = right.layout.take_stale_for_swap();
        let marks = effect.marks.map(|job| {
            let (mut left_markup, mut right_markup) =
                derive_wash_markups(&effect.diff, &job.left_text, &job.window);
            if job.derive_folds {
                mint_fold_strips(
                    &effect.diff,
                    &job.left_text,
                    &mut left_markup,
                    &mut right_markup,
                );
            } else {
                let carry = |from: Option<&crate::markup::Markup>,
                             into: &mut crate::markup::Markup| {
                    let Some(from) = from else { return };
                    for hit in from.all_inlays_in(0..u32::MAX) {
                        into.replace_inlay(hit.key.key, hit.range.clone(), hit.inlay.clone());
                    }
                };
                carry(job.left_current.as_ref(), &mut left_markup);
                carry(job.right_current.as_ref(), &mut right_markup);
            }
            trace_diff(|| {
                use intervals::IntervalQuery;
                format!(
                    "marks derived window={:?} folds={} left={} right={} changed=({},{})",
                    job.window,
                    job.derive_folds,
                    left_markup
                        .query(0..u32::MAX, intervals::Order::Ascending)
                        .count(),
                    right_markup
                        .query(0..u32::MAX, intervals::Order::Ascending)
                        .count(),
                    crate::markup::set_diff(job.left_current.as_ref(), &left_markup).len(),
                    crate::markup::set_diff(job.right_current.as_ref(), &right_markup).len(),
                )
            });
            MarksLanding {
                left_changed: crate::markup::set_diff(job.left_current.as_ref(), &left_markup),
                right_changed: crate::markup::set_diff(job.right_current.as_ref(), &right_markup),
                window: job.window,
                left_markup,
                right_markup,
                derived_folds: job.derive_folds,
            }
        });
        SplitDiffCommand::PairRepaired {
            left,
            right,
            align: effect.align,
            marks,
            seq: effect.seq,
        }
    }
}

impl Effect for RepairDiffEffect {
    type Result = SplitDiffCommand;
}

fn affected_span(operation: &Operation) -> Option<Range<u32>> {
    let mut at = 0u32;
    let mut start = None;
    let mut end = 0u32;
    for op in operation.iter() {
        match op {
            Op::Retain(len) => at += len,
            other => {
                let len = other.old_len();
                start.get_or_insert(at);
                end = at + len;
                at += len;
            }
        }
    }
    start.map(|start| start..end.max(start))
}

#[cfg(test)]
mod tests;
