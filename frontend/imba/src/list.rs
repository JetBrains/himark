// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::hash::Hash;

use crate::anim::{Animation, AnimationClock, Easing, Motion};
use crate::ui::UiCtx;
use intervals::{Interval, IntervalQuery, Intervals, Order};
use rope::{Cursor, Measure, MetricId, Metrics, Rope, SeekMode};
use skia_safe::{Color, Paint, Rect, Size};

use crate::{
    arena::Arena,
    constraints::Constraints,
    container::viewport_for_child,
    event::{Event, EventResult},
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, View, Widget,
};

const ROW_PX: MetricId = MetricId(0);

/// The dedicated overlay host for a list's sticky lines — the pane
/// that owns the scrolling list declares it (`.overlay_host`), the
/// list plants into it.
pub const STICKY_HOST: crate::overlay::OverlayHost =
    crate::overlay::OverlayHost("imba.list-sticky");

const MAX_STICKY: usize = 4;

/// How the planted band paints around the rows it re-realizes: the
/// band must be OPAQUE (it floats over the scrolled content) and
/// close with a divider. Colors are the caller's — imba stays
/// theme-free.
#[derive(Clone, Copy)]
pub struct StickyStyle {
    pub background: Color,
    pub divider: Color,
    pub divider_width: f32,
}

/// Resolved at display time, so the band re-themes live.
pub type StickySource = std::sync::Arc<dyn Fn(&Store) -> StickyStyle + Send + Sync>;

pub struct ListElement<T> {
    view: T,
    height: f32,
}

impl<T: Clone> Clone for ListElement<T> {
    fn clone(&self) -> Self {
        Self {
            view: self.view.clone(),
            height: self.height,
        }
    }
}

pub struct ListMeasure;

impl<T: Clone> Measure<ListElement<T>> for ListMeasure {
    type Metrics = Metrics<1>;

    fn zero() -> Self::Metrics {
        Metrics::zero()
    }

    fn metric_at(metrics: &Self::Metrics, id: MetricId) -> u32 {
        metrics.metric_at(id)
    }

    fn add_assign(metrics: &mut Self::Metrics, other: Self::Metrics) {
        metrics.add_assign(other);
    }

    fn sub_assign(metrics: &mut Self::Metrics, other: Self::Metrics) {
        metrics.sub_assign(other);
    }

    fn measure(element: &ListElement<T>) -> Self::Metrics {
        // A row's height is a pixel count. An unbounded layout that
        // leaks f32::MAX through here would saturate to u32::MAX and
        // WRAP the sumtree's cumulative offsets (debug: add-overflow
        // panic; release: every later row painted on top of the
        // others). Clamp at the sink — the row is still absurd, but
        // the list stays a list.
        Metrics([element.height.ceil().clamp(0.0, MAX_ROW_PX) as u32])
    }
}

/// The tallest a single measured row may claim. Generous for real
/// content (a million pixels), small enough that thousands of
/// clamped rows still sum far below `u32::MAX`.
const MAX_ROW_PX: f32 = 1_000_000.0;

#[derive(Clone, Copy, Default)]
pub struct SeparatorStyle {
    pub color: Color,

    pub inset: f32,
    pub thickness: f32,
}

const SPLICE_MS: f64 = 160.0;

const UNROLL_CAP_PX: f32 = 2048.0;

struct SpliceAnimation {
    start: usize,
    kind: AnimationKind,
}

enum AnimationKind {
    Scale {
        targets: Vec<f32>,
        scale: Animation<f32>,
    },

    Unroll {
        targets: Vec<f32>,
        edge: Animation<f32>,
        written: usize,
    },
}

impl SpliceAnimation {
    fn len(&self) -> usize {
        match &self.kind {
            AnimationKind::Scale { targets, .. } => targets.len(),
            AnimationKind::Unroll { targets, .. } => targets.len(),
        }
    }

    fn targets(&self) -> &[f32] {
        match &self.kind {
            AnimationKind::Scale { targets, .. } => targets,
            AnimationKind::Unroll { targets, .. } => targets,
        }
    }
}

impl Clone for SpliceAnimation {
    fn clone(&self) -> Self {
        Self {
            start: self.start,
            kind: match &self.kind {
                AnimationKind::Scale { targets, scale } => AnimationKind::Scale {
                    targets: targets.clone(),
                    scale: *scale,
                },
                AnimationKind::Unroll {
                    targets,
                    edge,
                    written,
                } => AnimationKind::Unroll {
                    targets: targets.clone(),
                    edge: *edge,
                    written: *written,
                },
            },
        }
    }
}

fn measured_slice<T: Clone, K: Clone + Eq + Hash>(
    rows: impl IntoIterator<Item = (T, f32)>,
) -> ListSlice<T, K> {
    let mut slice = ListSlice::new();
    slice.pending = rows
        .into_iter()
        .map(|(view, height)| ListElement { view, height })
        .collect();
    slice
}

#[derive(Clone, Copy, Default)]
pub struct SelectionStyle {
    pub fill: Color,

    pub accent: Color,
    pub accent_width: f32,

    pub accent_inset: f32,
    pub radius: f32,
}

struct SelectionState<K> {
    intervals: Intervals<K, ()>,

    cursor: Option<K>,

    reveal: bool,
    style: SelectionStyle,
}

impl<K: Clone + Eq + std::hash::Hash> Clone for SelectionState<K> {
    fn clone(&self) -> Self {
        Self {
            intervals: self.intervals.clone(),
            cursor: self.cursor.clone(),
            reveal: self.reveal,
            style: self.style,
        }
    }
}

pub struct ListSlice<T: Clone, K = ()> {
    pending: Vec<ListElement<T>>,
    items: Rope<ListElement<T>, ListMeasure>,

    spans: Vec<Interval<K, ()>>,
}

impl<T: Clone, K: Clone + Eq + Hash> Default for ListSlice<T, K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone, K: Clone + Eq + Hash> ListSlice<T, K> {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            items: Rope::from_iter([]),
            spans: Vec::new(),
        }
    }

    /// The view is the one that knows how to lay itself out, so it is
    /// laid RIGHT HERE and enters the slice already measured — the
    /// rope's invariant (every element carries its real height) holds
    /// at the door. Only for rows whose height ignores the width.
    pub fn push(&mut self, view: T, store: &Store, ui: &UiCtx)
    where
        T: View,
    {
        let height = laid_row_height(&view, store, ui);
        self.push_sized(view, height);
    }

    pub fn push_keyed(&mut self, key: K, view: T, store: &Store, ui: &UiCtx)
    where
        T: View,
    {
        let height = laid_row_height(&view, store, ui);
        self.push_keyed_sized(key, view, height);
    }

    /// A row whose height the caller already knows — measured at a
    /// real width (width-dependent rows) or carried over from an
    /// existing rope element.
    pub fn push_sized(&mut self, view: T, height: f32) {
        self.pending.push(ListElement { view, height });
    }

    pub fn push_keyed_sized(&mut self, key: K, view: T, height: f32) {
        let index = self.len() as u32;
        self.push_sized(view, height);
        self.spans.push(Interval {
            range: index..index + 1,
            greedy_left: true,
            greedy_right: false,
            key,
            value: (),
        });
    }

    pub fn cover(&mut self, key: K, range: std::ops::Range<usize>) {
        self.spans.push(Interval {
            range: range.start as u32..range.end as u32,
            greedy_left: true,
            greedy_right: false,
            key,
            value: (),
        });
    }

    pub fn sealed(mut self) -> Self {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.items = Rope::from_iter(self.items.iter().chain(pending));
        }
        self
    }

    pub fn len(&self) -> usize {
        self.items.len() + self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.pending.is_empty()
    }

    pub fn total_height(&self) -> f32 {
        self.items.metrics().metric_at(ROW_PX) as f32
            + self
                .pending
                .iter()
                .map(|element| element.height)
                .sum::<f32>()
    }
}

pub struct ListView<T: Clone, K: Clone + Eq + Hash = ()> {
    items: Rope<ListElement<T>, ListMeasure>,

    structure: Intervals<K, ()>,

    selection: Option<SelectionState<K>>,

    matches: Intervals<K, ()>,

    focused: Option<usize>,
    separators: Option<SeparatorStyle>,

    laid_width: std::sync::atomic::AtomicU32,

    /// The viewport's top edge — the enclosing scroll's own
    /// `scroll_y`, pushed in via `View::scrolled` from the scroll's
    /// perform (docs/viewport-preservation.md §3.1). Height mutations
    /// read it to anchor what the user was looking at.
    viewport_top: f32,

    /// Where the anchored content sits AFTER a height mutation above
    /// the viewport — set by the mutation door, read by the settle
    /// pulse, cleared when the next Viewport report lands. Absolute,
    /// so repeated pulses converge instead of compounding.
    settle_to: Option<f32>,

    /// An armed keyed reveal, independent of selection: `reveal_row`
    /// scrolls a row into view without touching what is selected.
    /// Resolved against the LIVE rope on every clock, so splices
    /// between arming and landing re-aim it for free; cleared by the
    /// `Revealed` round trip, or when the key leaves the list.
    row_reveal: Option<(K, crate::event::Placement)>,

    animations: Vec<SpliceAnimation>,

    generation: u64,

    /// Sticky parent lines (see `with_sticky`).
    sticky: Option<StickySource>,
}

pub enum ListCommand<C> {
    Child(usize, C),

    Focus(usize, Option<Box<ListCommand<C>>>),

    /// The widget re-observed its viewport top on a traversal
    /// (docs/viewport-preservation.md §3.1): the retained copy
    /// refreshes, and any pending correction is superseded — the
    /// scroll that moved the top knows better than the door did.
    ViewportTop(f32),

    SetHeight(usize, f32),

    Animate(AnimationClock),

    Revealed,
}

/// The list's row rope — the PREBUILT form a `ListView` mounts O(1).
/// `measured` is the one linear builder: workers build bulk ropes
/// and hand them over; a UI-thread call site composing
/// `from_rope(measured(..))` states the linear cost exactly where it
/// is paid, instead of hiding it inside a constructor.
pub type ListRope<T> = Rope<ListElement<T>, ListMeasure>;

pub fn measured<T: Clone>(items: impl IntoIterator<Item = (T, f32)>) -> ListRope<T> {
    Rope::from_iter(
        items
            .into_iter()
            .map(|(view, height)| ListElement { view, height }),
    )
}

impl<T: Clone, K: Clone + Eq + Hash> ListView<T, K> {
    /// The mount state — rows arrive later (splices, landings).
    pub fn empty() -> Self {
        Self::from_rope(Rope::from_iter([]))
    }

    pub fn empty_at(width: f32) -> Self {
        Self::from_rope_at(width, Rope::from_iter([]))
    }

    pub fn from_rope(items: ListRope<T>) -> Self {
        Self::from_rope_at(f32::NAN, items)
    }

    pub fn from_rope_at(width: f32, items: ListRope<T>) -> Self {
        Self {
            items,
            structure: Intervals::new(),
            selection: None,
            matches: Intervals::new(),
            focused: None,
            separators: None,
            laid_width: std::sync::atomic::AtomicU32::new(width.to_bits()),
            viewport_top: 0.0,
            settle_to: None,
            row_reveal: None,
            animations: Vec::new(),
            generation: 0,
            sticky: None,
        }
    }

    pub fn from_slice_at(width: f32, slice: ListSlice<T, K>) -> Self {
        let mut list = Self::from_slice(slice);
        list.laid_width = std::sync::atomic::AtomicU32::new(width.to_bits());
        list
    }

    pub fn from_slice(slice: ListSlice<T, K>) -> Self {
        let slice = slice.sealed();
        let mut list = Self::empty();
        list.items = slice.items;

        list.structure.insert(slice.spans);
        list
    }

    pub fn with_separators(mut self, style: SeparatorStyle) -> Self {
        self.separators = Some(style);
        self
    }

    /// Sticky parent lines: while a node's subtree fills the top of
    /// the viewport, the node's OWN row is planted at the top of the
    /// list (into `STICKY_HOST`). Parenthood is the list's own
    /// structure — any node whose span covers more rows than its own
    /// qualifies; chains nest up to a small depth. The planted line
    /// is the live row view RE-REALIZED, so everything the row offers
    /// (buttons, chevrons, presses) keeps working in the plant.
    pub fn with_sticky(mut self, style: StickySource) -> Self {
        self.sticky = Some(style);
        self
    }

    pub fn with_selection(mut self, style: SelectionStyle) -> Self {
        self.selection = Some(SelectionState {
            intervals: Intervals::new(),
            cursor: None,
            reveal: false,
            style,
        });
        self
    }

    pub fn set_selection_style(&mut self, style: SelectionStyle) {
        if let Some(selection) = &mut self.selection {
            selection.style = style;
        }
    }

    #[doc(hidden)]
    pub fn selection_style(&self) -> Option<SelectionStyle> {
        self.selection.as_ref().map(|selection| selection.style)
    }

    pub fn structure_keys(&self) -> Intervals<K, ()> {
        self.structure.clone()
    }

    pub fn row_range(&self, key: &K) -> Option<std::ops::Range<usize>> {
        self.structure
            .find_by_id(key)
            .map(|interval| interval.range.start as usize..interval.range.end as usize)
    }

    pub fn spans_within(&self, range: std::ops::Range<usize>) -> Vec<(K, std::ops::Range<usize>)> {
        let (start, end) = (range.start as u32, range.end as u32);
        self.structure
            .query(start..end.max(start.saturating_add(1)), Order::Ascending)
            .filter(|interval| {
                interval.range.start >= start
                    && interval.range.end <= end
                    && interval.range.end - interval.range.start > 1
            })
            .map(|interval| {
                (
                    interval.key.clone(),
                    interval.range.start as usize..interval.range.end as usize,
                )
            })
            .collect()
    }

    /// The row's measured height, straight from the rope — carriers
    /// re-splicing existing rows bring these along instead of
    /// re-measuring.
    pub fn height_at(&self, index: usize) -> Option<f32> {
        let mut cursor = self.items.cursor();
        cursor
            .seek_to_index(index as u32)
            .then(|| cursor.element().height)
    }

    /// A row view CLONE — the test oracle's read; ropes only hand
    /// out borrows through live cursors.
    #[doc(hidden)]
    pub fn view_at(&self, index: usize) -> Option<T> {
        if self.items.is_empty() {
            return None;
        }
        let mut cursor = self.items.cursor();
        if !cursor.seek_to_index(index as u32) {
            return None;
        }
        Some(cursor.element().view.clone())
    }

    pub fn key_at(&self, index: usize) -> Option<&K> {
        let index = index as u32;
        self.structure
            .query(index..index.saturating_add(1), Order::Ascending)
            .filter(|interval| interval.range.contains(&index))
            .min_by_key(|interval| interval.range.end - interval.range.start)
            .map(|interval| interval.key)
    }

    pub fn depth_at(&self, index: usize) -> usize {
        let index = index as u32;
        self.structure
            .query(index..index.saturating_add(1), Order::Ascending)
            .filter(|interval| interval.range.contains(&index))
            .count()
            .saturating_sub(1)
    }

    pub fn cursor(&self) -> Option<&K> {
        self.selection.as_ref()?.cursor.as_ref()
    }

    pub fn selected(&self) -> Vec<K> {
        let Some(selection) = &self.selection else {
            return Vec::new();
        };
        selection
            .intervals
            .query(0..u32::MAX, Order::Ascending)
            .map(|interval| interval.key.clone())
            .collect()
    }

    pub fn is_selected(&self, key: &K) -> bool {
        self.selection
            .as_ref()
            .is_some_and(|selection| selection.intervals.find_by_id(key).is_some())
    }

    pub fn select_only(&mut self, key: K) {
        let Some(range) = self.own_row(&key) else {
            return;
        };
        let Some(selection) = &mut self.selection else {
            return;
        };
        selection.intervals = Intervals::new();
        selection.intervals.insert([Interval {
            range,
            greedy_left: false,
            greedy_right: false,
            key: key.clone(),
            value: (),
        }]);
        selection.cursor = Some(key);
        selection.reveal = true;
    }

    pub fn toggle_selected(&mut self, key: K) {
        let Some(range) = self.own_row(&key) else {
            return;
        };
        let Some(selection) = &mut self.selection else {
            return;
        };
        if selection.intervals.find_by_id(&key).is_some() {
            selection.intervals.remove([&key]);
        } else {
            selection.intervals.insert([Interval {
                range,
                greedy_left: false,
                greedy_right: false,
                key: key.clone(),
                value: (),
            }]);
        }
        selection.cursor = Some(key);
        selection.reveal = true;
    }

    pub fn cancel_reveal(&mut self) {
        if let Some(selection) = &mut self.selection {
            selection.reveal = false;
        }
        self.row_reveal = None;
    }

    /// Arm a keyed reveal WITHOUT selecting: `TopLeftAt` pins the
    /// row's top edge to the viewport's (clamped at the end of the
    /// list), `EnsureVisible` is the golden-section scroll-into-view.
    /// Works on keys whose content has not landed yet — the reveal
    /// aims at the row's reserved extent and survives its swap.
    pub fn reveal_row(&mut self, key: K, placement: crate::event::Placement) {
        self.row_reveal = Some((key, placement));
    }

    pub fn clear_selection(&mut self) {
        if let Some(selection) = &mut self.selection {
            selection.intervals = Intervals::new();
            selection.cursor = None;
            selection.reveal = false;
        }
    }

    pub fn cursor_step(&mut self, delta: isize) {
        if self.selection.is_none() || self.items.is_empty() {
            return;
        }
        let at = self
            .cursor()
            .and_then(|key| self.own_row_index(key))
            .unwrap_or(0);
        let len = self.items.len();
        let step = if delta >= 0 { 1isize } else { -1 };
        let mut index = at as isize;
        let mut remaining = delta.abs();
        let mut landed: Option<K> = self.key_at(at).cloned();
        while remaining > 0 {
            index += step;
            if index < 0 || index >= len as isize {
                break;
            }
            if let Some(key) = self.key_at(index as usize) {
                landed = Some(key.clone());
                remaining -= 1;
            }
        }
        if let Some(key) = landed {
            self.select_only(key);
        }
    }

    fn own_row(&self, key: &K) -> Option<std::ops::Range<u32>> {
        let start = self.structure.find_by_id(key)?.range.start;
        Some(start..start + 1)
    }

    fn own_row_index(&self, key: &K) -> Option<usize> {
        self.own_row(key).map(|range| range.start as usize)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn focused(&self) -> Option<usize> {
        self.focused
    }

    pub fn rows(&self) -> impl Iterator<Item = T> + '_ {
        self.items.iter().map(|element| element.view)
    }

    pub fn rows_from(&self, index: usize) -> impl Iterator<Item = T> + '_ {
        let mut cursor = self.items.cursor();
        let seeked = cursor.seek_to_index(index as u32);
        seeked
            .then(|| cursor.iter().map(|element| element.view))
            .into_iter()
            .flatten()
    }

    pub fn total_height(&self) -> f32 {
        self.items.metrics().metric_at(ROW_PX) as f32
    }

    pub fn laid_width(&self) -> f32 {
        f32::from_bits(self.laid_width.load(std::sync::atomic::Ordering::Relaxed))
    }

    pub fn row_span(&self, index: usize) -> Option<(f32, f32)> {
        let mut cursor = self.items.cursor();
        if !cursor.seek_to_index(index as u32) {
            return None;
        }
        let top = cursor.position().metric_at(ROW_PX) as f32;
        let height = cursor.element_metrics().metric_at(ROW_PX) as f32;
        Some((top, height))
    }

    pub fn index_at_y(&self, y: f32) -> Option<usize> {
        if self.items.is_empty() || y < 0.0 {
            return None;
        }
        let mut cursor = self.items.cursor();
        if !cursor.seek(ROW_PX, y as u32, SeekMode::After) {
            return None;
        }
        Some(cursor.index() as usize)
    }

    pub fn splice(
        &mut self,
        range: std::ops::Range<usize>,
        rows: impl IntoIterator<Item = (T, f32)>,
    ) {
        self.splice_slice(range, measured_slice(rows));
    }

    pub fn splice_slice(&mut self, range: std::ops::Range<usize>, slice: ListSlice<T, K>) {
        self.splice_impl(range, slice.sealed());
    }

    pub fn splice_animated(
        &mut self,
        range: std::ops::Range<usize>,
        rows: impl IntoIterator<Item = (T, f32)>,
    ) {
        self.splice_slice_animated(range, measured_slice(rows));
    }

    pub fn splice_slice_animated(&mut self, range: std::ops::Range<usize>, slice: ListSlice<T, K>) {
        let slice = slice.sealed();
        let len = self.items.len();
        let start = range.start.min(len);
        let old_extent = self.extent_of(start..range.end.clamp(start, len));
        let new_extent = slice.total_height();
        if new_extent <= f32::EPSILON {
            return self.splice_impl(range, slice);
        }
        if new_extent <= old_extent {
            let targets: Vec<f32> = slice.items.iter().map(|element| element.height).collect();
            let from_scale = (old_extent / new_extent).max(0.0);
            let mut scale = Animation::done(
                from_scale,
                Motion::Ease {
                    duration_ms: SPLICE_MS,
                    easing: Easing::EaseOut,
                },
            );
            scale.set(1.0);
            let mut scaled = slice;
            scaled.items = Rope::from_iter(scaled.items.iter().map(|element| ListElement {
                view: element.view.clone(),
                height: element.height * from_scale,
            }));
            self.splice_impl(range, scaled);
            self.animations.push(SpliceAnimation {
                start,
                kind: AnimationKind::Scale { targets, scale },
            });
            return;
        }

        let mut unrolled: Vec<f32> = Vec::new();
        let mut extent = 0.0f32;
        for element in slice.items.iter() {
            if extent >= UNROLL_CAP_PX {
                break;
            }
            unrolled.push(element.height);
            extent += element.height;
        }
        let prefix = unrolled.len();
        let mut rezeroed = slice;
        rezeroed.items =
            Rope::from_iter(rezeroed.items.iter().enumerate().map(|(index, element)| {
                ListElement {
                    view: element.view.clone(),
                    height: if index < prefix { 0.0 } else { element.height },
                }
            }));
        self.splice_impl(range, rezeroed);

        let from = old_extent.min(extent);
        let mut edge = Animation::done(
            from,
            Motion::Ease {
                duration_ms: SPLICE_MS,
                easing: Easing::EaseOut,
            },
        );
        edge.set(extent);
        let written = self.apply_unroll_frame(start, &unrolled, from, 0, false);
        self.animations.push(SpliceAnimation {
            start,
            kind: AnimationKind::Unroll {
                targets: unrolled,
                edge,
                written,
            },
        });
    }

    fn apply_unroll_frame(
        &mut self,
        start: usize,
        targets: &[f32],
        value: f32,
        written: usize,
        settle: bool,
    ) -> usize {
        let mut cum = 0.0f32;
        let mut crossed = 0usize;
        for target in targets {
            if cum + target <= value {
                cum += target;
                crossed += 1;
            } else {
                break;
            }
        }
        let from = written.min(crossed);
        let mut frame: Vec<f32> = targets[from..crossed].to_vec();
        if !settle && crossed < targets.len() {
            frame.push((value - cum).max(0.0));
        }
        if !frame.is_empty() {
            self.set_heights(start + from, frame.into_iter());
        }
        if settle {
            let tail: Vec<f32> = targets[crossed..].to_vec();
            if !tail.is_empty() {
                self.set_heights(start + crossed, tail.into_iter());
            }
        }
        crossed
    }

    fn extent_of(&self, range: std::ops::Range<usize>) -> f32 {
        if range.is_empty() || self.items.is_empty() {
            return 0.0;
        }
        let mut cursor = self.items.cursor();
        if !cursor.seek_to_index(range.start as u32) {
            return 0.0;
        }
        let top = cursor.position().metric_at(ROW_PX) as f32;

        let bottom = match range.end < self.items.len() {
            true if cursor.seek_to_index(range.end as u32) => {
                cursor.position().metric_at(ROW_PX) as f32
            }
            _ => self.total_height(),
        };
        bottom - top
    }

    fn set_heights(&mut self, start: usize, heights: impl ExactSizeIterator<Item = f32>) {
        if heights.len() == 0 || self.items.is_empty() {
            return;
        }
        let door = self.door_anchor();
        let mut cursor = self.items.cursor();
        if !cursor.seek_to_index(start as u32) {
            return;
        }
        let mut rebuilt = Vec::with_capacity(heights.len());
        for height in heights {
            let mut element = cursor.element().clone();
            element.height = height;
            rebuilt.push(element);
            if !cursor.advance() {
                break;
            }
        }
        let count = rebuilt.len() as u32;
        let mut cursor = self.items.cursor();
        if !cursor.seek_to_index(start as u32) {
            return;
        }
        cursor.delete(count);
        cursor.insert(Rope::from_iter(rebuilt));
        self.items = cursor.rope();
        self.door_resolve(door);
    }

    fn reconcile_animations(&mut self, range: std::ops::Range<usize>, inserted: usize) {
        let animations = std::mem::take(&mut self.animations);
        let mut settled: Vec<SpliceAnimation> = Vec::new();
        let mut kept = Vec::new();
        for mut animation in animations {
            let end = animation.start + animation.len();
            if end <= range.start {
                kept.push(animation);
            } else if animation.start >= range.end {
                animation.start = animation.start - (range.end - range.start) + inserted;
                kept.push(animation);
            } else {
                settled.push(animation);
            }
        }
        self.animations = kept;
        for animation in settled {
            let targets: Vec<f32> = animation.targets().to_vec();
            self.set_heights(animation.start, targets.into_iter());
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The viewport anchor, captured BEFORE a height mutation: the
    /// keyed row under the reported viewport top, the offset into it
    /// and the top itself (docs/viewport-preservation.md §3).
    fn door_anchor(&self) -> Option<(K, f32, f32)> {
        let top = self.viewport_top;
        if top <= 0.5 || self.items.is_empty() {
            return None;
        }
        let mut cursor = self.items.cursor();
        if !cursor.seek(ROW_PX, top as u32, SeekMode::After) {
            return None;
        }
        let key = self.key_at(cursor.index() as usize)?.clone();
        let row_top = cursor.position().metric_at(ROW_PX) as f32;
        Some((key, top - row_top, top))
    }

    /// Resolve the captured anchor against the mutated rope; if the
    /// anchored row moved, note where the viewport must re-aim. A
    /// deleted anchor row keeps plain pixels — the fallback.
    fn door_resolve(&mut self, anchor: Option<(K, f32, f32)>) {
        let Some((key, dy, was)) = anchor else {
            return;
        };
        let Some(interval) = self.structure.find_by_id(&key) else {
            return;
        };
        let mut cursor = self.items.cursor();
        if !cursor.seek_to_index(interval.range.start) {
            return;
        }
        let fresh = cursor.position().metric_at(ROW_PX) as f32 + dy;
        if (fresh - was).abs() > 0.5 {
            self.settle_to = Some(fresh);
        }
    }

    fn splice_impl(&mut self, range: std::ops::Range<usize>, slice: ListSlice<T, K>) {
        let door = self.door_anchor();
        self.generation += 1;
        let len = self.items.len();
        let start = range.start.min(len);
        let end = range.end.clamp(start, len);
        let inserted = slice.items;
        let inserted_len = inserted.len();
        self.reconcile_animations(start..end, inserted_len);

        if start == len {
            if inserted_len > 0 {
                if len == 0 {
                    self.items = inserted;
                } else {
                    let mut cursor = self.items.cursor();
                    if !cursor.seek_to_index(len as u32 - 1) {
                        return;
                    }
                    let last = cursor.element().clone();
                    cursor.delete(1u32);
                    cursor.insert(Rope::from_iter(
                        std::iter::once(last).chain(inserted.iter()),
                    ));
                    self.items = cursor.rope();
                }
            }
        } else {
            let mut cursor = self.items.cursor();
            if !cursor.seek_to_index(start as u32) {
                return;
            }
            cursor.delete((end - start) as u32);
            cursor.insert(inserted);
            self.items = cursor.rope();
        }

        let (start_u, end_u) = (start as u32, end as u32);
        let covered = |intervals: &Intervals<K, ()>| -> Vec<K> {
            intervals
                .query(
                    start_u..end_u.max(start_u.saturating_add(1)),
                    Order::Ascending,
                )
                .filter(|interval| start_u <= interval.range.start && interval.range.end <= end_u)
                .map(|interval| interval.key.clone())
                .collect()
        };

        let mut steps = Vec::with_capacity(3);
        if start > 0 {
            steps.push(intervals::EditStep::Retain(start_u));
        }
        if inserted_len > 0 {
            steps.push(intervals::EditStep::Insert(inserted_len as u32));
        }
        if end > start {
            steps.push(intervals::EditStep::Delete((end - start) as u32));
        }
        let stale = covered(&self.structure);
        self.structure.remove(stale.iter());
        self.structure.edit(steps.iter().copied());

        let shifted = slice.spans.into_iter().map(|mut interval| {
            interval.range.start = interval.range.start.saturating_add(start_u);
            interval.range.end = interval.range.end.saturating_add(start_u);
            interval
        });
        self.structure.insert(shifted.collect::<Vec<_>>());

        let displaced_selection: Vec<K> = match &mut self.selection {
            Some(selection) => {
                let stale = covered(&selection.intervals);
                selection.intervals.remove(stale.iter());
                selection.intervals.edit(steps.iter().copied());
                stale
            }
            None => Vec::new(),
        };
        let displaced_matches: Vec<K> = if self.matches.is_empty() {
            Vec::new()
        } else {
            let stale = covered(&self.matches);
            self.matches.remove(stale.iter());
            self.matches.edit(steps.iter().copied());
            stale
        };

        for key in displaced_selection {
            if let (Some(range), Some(selection)) = (self.own_row(&key), &mut self.selection) {
                selection.intervals.insert([Interval {
                    range,
                    greedy_left: false,
                    greedy_right: false,
                    key,
                    value: (),
                }]);
            }
        }
        for key in displaced_matches {
            if let Some(range) = self.own_row(&key) {
                self.matches.insert([Interval {
                    range,
                    greedy_left: false,
                    greedy_right: false,
                    key,
                    value: (),
                }]);
            }
        }
        self.door_resolve(door);

        self.focused = self.focused.and_then(|focused| {
            if focused < start {
                Some(focused)
            } else if focused < end {
                None
            } else {
                Some(focused - (end - start) + inserted_len)
            }
        });

        let repoint = self
            .selection
            .as_ref()
            .and_then(|selection| selection.cursor.clone())
            .is_some_and(|cursor| self.structure.find_by_id(&cursor).is_none());
        if repoint {
            let landing = self
                .key_at(start.min(self.items.len().saturating_sub(1)))
                .cloned();
            if let Some(selection) = &mut self.selection {
                selection.intervals = Intervals::new();
                selection.cursor = None;
                selection.reveal = false;
            }
            if let Some(key) = landing {
                if let (Some(range), Some(selection)) = (self.own_row(&key), &mut self.selection) {
                    selection.intervals.insert([Interval {
                        range,
                        greedy_left: false,
                        greedy_right: false,
                        key: key.clone(),
                        value: (),
                    }]);
                    selection.cursor = Some(key);
                }
            }
        }
    }

    pub fn update_heights(&mut self, mut measure: impl FnMut(&T) -> Option<f32>) -> bool {
        let mut changed = false;
        let remeasured: Vec<ListElement<T>> = self
            .items
            .iter()
            .map(|element| {
                let height = match measure(&element.view) {
                    Some(height) if (height - element.height).abs() > 0.5 => {
                        changed = true;
                        height
                    }
                    _ => element.height,
                };
                ListElement {
                    view: element.view,
                    height,
                }
            })
            .collect();
        if changed {
            self.items = Rope::from_iter(remeasured);
        }
        changed
    }
}

pub trait SearchableList<K> {
    fn set_matches(&mut self, keys: &[K]);
    fn clear_matches(&mut self);

    fn step_matched(&mut self, delta: isize);
    fn match_count(&self) -> usize;
}

impl<T: Clone, K: Clone + Eq + Hash> SearchableList<K> for ListView<T, K> {
    fn set_matches(&mut self, keys: &[K]) {
        let mut matches = Intervals::new();
        matches.insert(
            keys.iter()
                .filter_map(|key| {
                    let range = self.own_row(key)?;
                    Some(Interval {
                        range,
                        greedy_left: false,
                        greedy_right: false,
                        key: key.clone(),
                        value: (),
                    })
                })
                .collect::<Vec<_>>(),
        );
        self.matches = matches;
    }

    fn clear_matches(&mut self) {
        self.matches = Intervals::new();
    }

    fn step_matched(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let at = self
            .cursor()
            .and_then(|key| self.own_row_index(key))
            .unwrap_or(0) as u32;

        let first = |order: Order, range: std::ops::Range<u32>| -> Option<K> {
            self.matches
                .query(range.clone(), order)
                .find(|interval| {
                    interval.range.end > range.start && interval.range.start < range.end
                })
                .map(|interval| interval.key.clone())
        };
        let len = self.items.len() as u32;
        let landed = if delta == 0 {
            first(Order::Ascending, at..len).or_else(|| first(Order::Ascending, 0..len))
        } else if delta > 0 {
            first(Order::Ascending, at.saturating_add(1)..len)
                .or_else(|| first(Order::Ascending, 0..len))
        } else {
            first(Order::Descending, 0..at).or_else(|| first(Order::Descending, 0..len))
        };
        if let Some(key) = landed {
            self.select_only(key);
        }
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }
}

impl<V, K> SearchableList<K> for crate::scroll::ScrollView<V>
where
    V: SearchableList<K> + crate::View,
{
    fn set_matches(&mut self, keys: &[K]) {
        self.content_mut().set_matches(keys);
    }
    fn clear_matches(&mut self) {
        self.content_mut().clear_matches();
    }
    fn step_matched(&mut self, delta: isize) {
        self.content_mut().step_matched(delta);
    }
    fn match_count(&self) -> usize {
        self.content().match_count()
    }
}

impl<T: Clone, K: Clone + Eq + Hash> Clone for ListView<T, K> {
    fn clone(&self) -> Self {
        Self {
            items: self.items.clone(),
            structure: self.structure.clone(),
            selection: self.selection.clone(),
            matches: self.matches.clone(),
            focused: self.focused,
            separators: self.separators,
            laid_width: std::sync::atomic::AtomicU32::new(
                self.laid_width.load(std::sync::atomic::Ordering::Relaxed),
            ),
            viewport_top: self.viewport_top,
            settle_to: self.settle_to,
            row_reveal: self.row_reveal.clone(),

            animations: self.animations.clone(),
            generation: self.generation,
            sticky: self.sticky.clone(),
        }
    }
}

/// Lays a row view once to learn its height — the store carries the
/// theme; fonts resolve identically under a fresh `UiCtx`.
/// Lays a row view once to learn its height — with the CALLER's
/// `UiCtx`, so font caches are the app's own, warm ones: a
/// measurement costs a text measure, never a font-manager build.
fn laid_row_height<T: View>(view: &T, store: &Store, ui: &UiCtx) -> f32 {
    let arena = Arena::default();
    let thunk = crate::Layout::layout(
        view.display(&arena, store, ui),
        &arena,
        Constraints {
            min: Size::default(),
            max: Size::new(f32::MAX, f32::MAX),
        },
    );
    let height = thunk.size().height;
    drop(thunk);
    height
}

impl<T, K> View for ListView<T, K>
where
    T: View + Clone,
    T::Command: Send + 'static,
    K: Clone + Eq + Hash + Send + Sync + 'static,
{
    type Command = ListCommand<T::Command>;

    fn destroy(&mut self, store: &mut Store, fx: &mut crate::effect::Effects<'_, Self::Command>) {
        for (index, element) in self.items.iter().enumerate() {
            let mut view = element.view.clone();
            fx.scope(
                move |command| ListCommand::Child(index, command),
                |fx| view.destroy(store, fx),
            );
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut crate::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            ListCommand::Child(index, command) => {
                if self.items.is_empty() {
                    return;
                }
                let mut cursor = self.items.cursor();
                if !cursor.seek_to_index(index as u32) {
                    return;
                }
                let mut element = cursor.element().clone();
                fx.scope(
                    move |command| ListCommand::Child(index, command),
                    |fx| element.view.perform(store, ui, command, fx),
                );

                let laid_width =
                    f32::from_bits(self.laid_width.load(std::sync::atomic::Ordering::Relaxed));
                if laid_width.is_finite() {
                    let frame = Arena::default();
                    let live = crate::Layout::layout(
                        element.view.display(&frame, store, ui),
                        &frame,
                        Constraints {
                            min: Size::default(),
                            max: Size::new(laid_width, f32::MAX),
                        },
                    )
                    .size()
                    .height;
                    if (live - element.height).abs() > 0.5 {
                        element.height = live;
                    }
                }
                cursor.delete(1u32);
                cursor.insert(Rope::from_iter([element]));
                self.items = cursor.rope();
            }
            ListCommand::Focus(index, then) => {
                self.focused = Some(index.min(self.items.len().saturating_sub(1)));
                if let Some(command) = then {
                    self.perform(store, ui, *command, fx);
                }
            }
            ListCommand::Animate(now) => {
                let mut animations = std::mem::take(&mut self.animations);
                let mut index = 0;
                while index < animations.len() {
                    let animation = &mut animations[index];
                    let start = animation.start;
                    let done = match &mut animation.kind {
                        AnimationKind::Scale { targets, scale } => {
                            scale.advance(now);
                            let value = scale.value();
                            if scale.running() {
                                let frame: Vec<f32> =
                                    targets.iter().map(|target| target * value).collect();
                                self.set_heights(start, frame.into_iter());
                                false
                            } else {
                                let targets = targets.clone();
                                self.set_heights(start, targets.into_iter());
                                true
                            }
                        }
                        AnimationKind::Unroll {
                            targets,
                            edge,
                            written,
                        } => {
                            edge.advance(now);
                            let value = edge.value();
                            let running = edge.running();
                            let targets = targets.clone();
                            let was = *written;
                            let crossed =
                                self.apply_unroll_frame(start, &targets, value, was, !running);
                            match &mut animations[index].kind {
                                AnimationKind::Unroll { written, .. } => *written = crossed,
                                _ => unreachable!(),
                            }
                            !running
                        }
                    };
                    if done {
                        animations.swap_remove(index);
                    } else {
                        index += 1;
                    }
                }
                self.animations = animations;
            }
            ListCommand::ViewportTop(top) => {
                self.viewport_top = top;
                self.settle_to = None;
            }
            ListCommand::Revealed => {
                self.row_reveal = None;
                if let Some(selection) = &mut self.selection {
                    selection.reveal = false;
                }
            }
            ListCommand::SetHeight(index, height) => {
                if self.items.is_empty() {
                    return;
                }
                let door = self.door_anchor();
                let mut cursor = self.items.cursor();
                if !cursor.seek_to_index(index as u32) {
                    return;
                }
                let mut element = cursor.element().clone();
                element.height = height;
                cursor.delete(1u32);
                cursor.insert(Rope::from_iter([element]));
                self.items = cursor.rope();
                self.door_resolve(door);
                if self.settle_to.is_some() {
                    fx.settle();
                }
            }
        }
    }

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> crate::focus::FocusData<'w, Self::Command> {
        use crate::event::EventResult;
        use crate::focus::FocusData;
        let Some(index) = self.focused else {
            return FocusData::default();
        };
        if self.items.is_empty() {
            return FocusData::default();
        }
        // Ropes only hand out borrows through live cursors, so the
        // owned answers (commands, location) are drained through one
        // cursor here and every handler re-seeks on the call — a
        // state walk per ask, nothing laid.
        let (commands, location, seat) = {
            let mut cursor = self.items.cursor();
            match cursor.seek_to_index(index as u32) {
                true => {
                    let mut data = cursor.element().view.focus_data(store, ui);
                    (
                        std::mem::take(&mut data.commands),
                        data.location.take(),
                        data.seat.take(),
                    )
                }
                false => return FocusData::default(),
            }
        };
        let with_row =
            move |f: &mut dyn FnMut(FocusData<'_, T::Command>) -> EventResult<T::Command>| {
                let mut cursor = self.items.cursor();
                match cursor.seek_to_index(index as u32) {
                    true => f(cursor.element().view.focus_data(store, ui))
                        .map(|command| ListCommand::Child(index, command)),
                    false => EventResult::Ignored,
                }
            };
        FocusData {
            commands: commands
                .into_iter()
                .map(|presentable| presentable.map(|command| ListCommand::Child(index, command)))
                .collect(),
            on_key: Some(Box::new(move |key, mods| {
                with_row(&mut |mut data| data.key(key, mods))
            })),
            on_text: Some(Box::new(move |text| {
                with_row(&mut |mut data| data.text(text))
            })),
            clipboard: Some(Box::new(move |visit| {
                with_row(&mut |mut data| match data.clipboard.as_mut() {
                    Some(seat) => seat(visit),
                    None => EventResult::Ignored,
                })
            })),
            location,
            seat,
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl crate::Layout<'a, Self::Command> + crate::LayoutValue + 'a {
        crate::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let width = constraints.max.width;
            self.laid_width
                .store(width.to_bits(), std::sync::atomic::Ordering::Relaxed);

            let rect_of = |key: &K| -> Option<Rect> {
                let index = self.own_row_index(key)?;
                let mut cursor = self.items.cursor();
                if !cursor.seek_to_index(index as u32) {
                    return None;
                }
                let top = cursor.position().metric_at(ROW_PX) as f32;
                let height = cursor.element_metrics().metric_at(ROW_PX) as f32;
                Some(Rect::from_xywh(0.0, top, width.max(1.0), height))
            };
            // The armed keyed reveal outranks the selection's: it is
            // explicit navigation. A key that left the list resolves
            // to a `Revealed` round trip that disarms it.
            let (reveal, reveal_lost) = match &self.row_reveal {
                Some((key, placement)) => match rect_of(key) {
                    Some(rect) => (Some((rect, *placement)), false),
                    None => (None, true),
                },
                None => (
                    self.selection
                        .as_ref()
                        .filter(|selection| selection.reveal)
                        .and_then(|selection| rect_of(selection.cursor.as_ref()?))
                        .map(|rect| (rect, crate::event::Placement::EnsureVisible)),
                    false,
                ),
            };
            ListWidget {
                items: &self.items,
                structure: &self.structure,
                selection: self.selection.as_ref(),
                matches: &self.matches,
                settle_to: self.settle_to,
                viewport_top: self.viewport_top,
                reveal,
                reveal_lost,
                animations: &self.animations,
                separators: self.separators,
                sticky: self.sticky.as_ref().map(|source| source(store)),
                store,
                ui,
                child_constraints: Constraints {
                    min: Size::default(),
                    max: Size::new(width, f32::MAX),
                },
                size: Size::new(width, self.items.metrics().metric_at(ROW_PX) as f32),
                focused: self.focused,
            }
        })
    }
}

struct ListWidget<'a, T: Clone, K: Clone + Eq + Hash> {
    items: &'a Rope<ListElement<T>, ListMeasure>,
    structure: &'a Intervals<K, ()>,
    selection: Option<&'a SelectionState<K>>,
    matches: &'a Intervals<K, ()>,
    settle_to: Option<f32>,
    viewport_top: f32,

    reveal: Option<(Rect, crate::event::Placement)>,
    reveal_lost: bool,
    animations: &'a [SpliceAnimation],
    separators: Option<SeparatorStyle>,
    sticky: Option<StickyStyle>,
    store: &'a Store,
    ui: &'a UiCtx,
    child_constraints: Constraints,
    size: Size,
    focused: Option<usize>,
}

impl<T, K> ListWidget<'_, T, K>
where
    T: View + Clone,
    K: Clone + Eq + Hash,
{
    fn row_rect(&self, cursor: &Cursor<ListElement<T>, ListMeasure>) -> Rect {
        let top = cursor.position().metric_at(ROW_PX) as f32;
        let height = cursor.element_metrics().metric_at(ROW_PX) as f32;
        Rect::from_xywh(0.0, top, self.size.width, height)
    }

    fn route(
        &self,
        cursor: &Cursor<ListElement<T>, ListMeasure>,
        arena: &Arena,
        event: &Event<'_>,
        child_viewport: Rect,
    ) -> EventResult<ListCommand<T::Command>> {
        let index = cursor.index() as usize;
        let rect = self.row_rect(cursor);
        crate::Layout::layout(
            cursor.element().view.display(arena, self.store, self.ui),
            arena,
            self.child_constraints,
        )
        .focus_scope(self.focused == Some(index))
        .realize(arena, child_viewport)
        .handle_event(arena, event, child_viewport)
        .map(move |command| ListCommand::Child(index, command))
        .reveal_translated(rect.left, rect.top)
    }

    fn cursor_at_y(&self, y: f32) -> Option<Cursor<ListElement<T>, ListMeasure>> {
        if self.items.is_empty() {
            return None;
        }
        let mut cursor = self.items.cursor();
        cursor
            .seek(ROW_PX, y.max(0.0) as u32, SeekMode::After)
            .then_some(cursor)
    }

    fn for_visible(
        &self,
        viewport: Rect,
        mut visit: impl FnMut(&Cursor<ListElement<T>, ListMeasure>, Rect),
    ) {
        let Some(mut cursor) = self.cursor_at_y(viewport.top) else {
            return;
        };
        loop {
            let rect = self.row_rect(&cursor);
            if rect.top >= viewport.bottom {
                break;
            }
            visit(&cursor, rect);
            if !cursor.advance() {
                break;
            }
        }
    }

    /// The parent chain to PLANT for this viewport: for the row under
    /// the (stack-consumed) top edge, the enclosing nodes — outermost
    /// first — whose own rows have scrolled behind the stack. A node
    /// encloses when its structure span covers more rows than its
    /// own; at a boundary the incoming node's real row IS at the top,
    /// so nothing plants and the handoff is seamless.
    fn sticky_chain(&self, viewport: Rect) -> Vec<StickyLine<T>> {
        let mut lines: Vec<StickyLine<T>> = Vec::new();
        let mut consumed = 0.0f32;
        while lines.len() < MAX_STICKY {
            let probe_y = viewport.top + consumed;
            if probe_y >= viewport.bottom {
                break;
            }
            let Some(cursor) = self.cursor_at_y(probe_y) else {
                break;
            };
            let probe = cursor.index();
            let mut parents: Vec<std::ops::Range<u32>> = self
                .structure
                .query(probe..probe.saturating_add(1), Order::Ascending)
                .filter(|interval| {
                    interval.range.start <= probe
                        && probe < interval.range.end
                        && interval.range.end - interval.range.start > 1
                })
                .map(|interval| interval.range.clone())
                .collect();
            parents.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
            parents.dedup_by(|a, b| a.start == b.start);
            if parents.len() <= lines.len() {
                break;
            }
            if !lines
                .iter()
                .zip(parents.iter())
                .all(|(line, span)| line.index == span.start as usize)
            {
                break;
            }
            let span = parents[lines.len()].clone();
            let mut row = self.items.cursor();
            if !row.seek_to_index(span.start) {
                break;
            }
            let top = row.position().metric_at(ROW_PX) as f32;
            let height = row.element_metrics().metric_at(ROW_PX) as f32;
            if top >= probe_y || height <= 0.0 {
                break;
            }
            lines.push(StickyLine {
                index: span.start as usize,
                view: row.element().view.clone(),
                height,
            });
            consumed += height;
        }
        lines
    }
}

struct StickyLine<T> {
    index: usize,
    view: T,
    height: f32,
}

/// The planted band: an overlay over the scrolled list re-realizing
/// the parent rows LIVE — commands route as the rows' own
/// (`Child(index, …)`), so a planted row's buttons work exactly like
/// the real one's. Opaque background, closing divider; presses stop
/// here (the band shadows what it covers).
struct StickyLines<'a, T: Clone> {
    rows: Vec<StickyLine<T>>,
    style: StickyStyle,
    store: &'a Store,
    ui: &'a UiCtx,
    child_constraints: Constraints,

    /// Where the list's content starts inside the pane-wide band.
    content_left: f32,
    size: Size,
}

impl<'a, T: Clone> StickyLines<'a, T> {
    fn line_at(&self, y: f32) -> Option<(&StickyLine<T>, f32)> {
        let mut top = 0.0;
        for line in &self.rows {
            if y < top + line.height {
                return Some((line, top));
            }
            top += line.height;
        }
        None
    }
}

impl<'a, T> Widget<'a, ListCommand<T::Command>> for StickyLines<'a, T>
where
    T: View + Clone,
    T::Command: 'a,
{
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<ListCommand<T::Command>> {
        let route =
            |line: &StickyLine<T>, event: &Event<'_>| -> EventResult<ListCommand<T::Command>> {
                let index = line.index;
                let child_viewport = Rect::from_xywh(
                    0.0,
                    0.0,
                    self.child_constraints.max.width.max(1.0),
                    line.height,
                );
                crate::Layout::layout(
                    line.view.display(arena, self.store, self.ui),
                    arena,
                    self.child_constraints,
                )
                .focus_scope(false)
                .realize(arena, child_viewport)
                .handle_event(arena, event, child_viewport)
                .map(move |command| ListCommand::Child(index, command))
            };
        match event {
            Event::Paint { canvas, .. } => {
                let bounds = Rect::from_size(self.size);
                let mut paint = Paint::default();
                paint.set_anti_alias(false);
                paint.set_color(self.style.background);
                canvas.draw_rect(bounds, &paint);

                let mut merged: EventResult<ListCommand<T::Command>> = EventResult::Ignored;
                let mut top = 0.0;
                for line in &self.rows {
                    canvas.save();
                    canvas.translate((self.content_left, top));
                    canvas.clip_rect(
                        Rect::from_xywh(
                            0.0,
                            0.0,
                            (bounds.width() - self.content_left).max(0.0),
                            line.height,
                        ),
                        None,
                        false,
                    );
                    let result = route(line, event);
                    merged = std::mem::replace(&mut merged, EventResult::Ignored).merge(result);
                    canvas.restore();
                    top += line.height;
                }

                paint.set_color(self.style.divider);
                canvas.draw_rect(
                    Rect::from_xywh(
                        0.0,
                        bounds.bottom - self.style.divider_width,
                        bounds.width(),
                        self.style.divider_width,
                    ),
                    &paint,
                );
                merged
            }
            // Presses mirror the list's own: the planted row FOCUSES,
            // exactly as its real twin would.
            Event::MouseDown { point, .. } => match self.line_at(point.y) {
                Some((line, top)) => {
                    let index = line.index;
                    match route(line, &event.translated(-self.content_left, -top)) {
                        EventResult::Command(command) => {
                            EventResult::Command(ListCommand::Focus(index, Some(Box::new(command))))
                        }
                        _ => EventResult::Command(ListCommand::Focus(index, None)),
                    }
                }
                None => EventResult::Handled,
            },
            _ => EventResult::Ignored,
        }
    }
}

impl<'a, T, K> Thunk<'a, ListCommand<T::Command>> for ListWidget<'a, T, K>
where
    T: View + Clone,
    T::Command: 'a,
    K: Clone + Eq + Hash,
{
    fn size(&self) -> Size {
        self.size
    }

    fn realize(
        self,
        arena: &'a Arena,
        viewport: Rect,
    ) -> crate::WidgetBox<'a, ListCommand<T::Command>> {
        crate::WidgetBox::new(
            arena,
            RealizedList {
                list: self,
                viewport,
                arena,
            },
        )
    }
}

struct RealizedList<'a, T: Clone, K: Clone + Eq + Hash> {
    list: ListWidget<'a, T, K>,

    #[allow(dead_code)]
    viewport: Rect,

    arena: &'a Arena,
}
impl<'a, T, K> Widget<'a, ListCommand<T::Command>> for RealizedList<'a, T, K>
where
    T: View + Clone,
    T::Command: 'a,
    K: Clone + Eq + Hash,
{
    fn size(&self) -> Size {
        self.list.size
    }

    fn overlays(&mut self) -> Vec<crate::overlay::Overlay<'a, ListCommand<T::Command>>> {
        let Some(style) = self.list.sticky else {
            return Vec::new();
        };
        let viewport = self.viewport;
        if viewport.top <= 0.0 || viewport.height() <= 0.0 {
            return Vec::new();
        }
        let rows = self.list.sticky_chain(viewport);
        if rows.is_empty() {
            return Vec::new();
        }
        let height: f32 = rows.iter().map(|line| line.height).sum();
        let width = self.list.size.width;
        let store = self.list.store;
        let ui = self.list.ui;
        let child_constraints = self.list.child_constraints;
        let arena = self.arena;
        vec![crate::overlay::Overlay {
            host: STICKY_HOST,
            anchor: Rect::from_xywh(0.0, viewport.top, width.max(1.0), height),
            content: Box::new(move |host_size: Size, anchor: Rect| {
                let widget = StickyLines {
                    rows,
                    style,
                    store,
                    ui,
                    child_constraints,
                    content_left: anchor.left.max(0.0),
                    size: Size::new(host_size.width.max(1.0), height),
                };
                vec![(
                    skia_safe::Point::new(0.0, anchor.top),
                    crate::ThunkBox::new(arena, crate::eager(widget)),
                )]
            }),
        }]
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<ListCommand<T::Command>> {
        self.list.handle_event(arena, event, viewport)
    }

    fn layout_data<'w>(
        &'w mut self,
        target: crate::focus::SeatKey,
    ) -> crate::focus::LayoutData<'w, ListCommand<T::Command>>
    where
        'a: 'w,
    {
        use crate::focus::LayoutData;
        // The list realizes rows per traversal, so there is nothing
        // standing to fold. The FOCUSED row (the list's own state, a
        // hint — not a second copy of anyone else's routing) is
        // realized here into the frame arena, and the target key
        // decides by RECOGNITION whether the seat is really inside.
        let Some(index) = self.list.focused else {
            return LayoutData::default();
        };
        if self.list.items.is_empty() {
            return LayoutData::default();
        }
        let mut cursor = self.list.items.cursor();
        if !cursor.seek_to_index(index as u32) {
            return LayoutData::default();
        }
        let rect = self.list.row_rect(&cursor);
        let child_viewport = viewport_for_child(self.viewport, rect).unwrap_or_default();
        // The cursor moves into the arena so the row view's borrow
        // reaches the frame lifetime; the realized row rides along.
        let cursor: &'a _ = crate::arena::ArenaBox::leak(self.arena.boxed(cursor));
        let widget = crate::Layout::layout(
            cursor
                .element()
                .view
                .display(self.arena, self.list.store, self.list.ui),
            self.arena,
            self.list.child_constraints,
        )
        .realize(self.arena, child_viewport);
        let widget: &'w mut crate::WidgetBox<'a, T::Command> =
            crate::arena::ArenaBox::leak(self.arena.boxed(widget));
        widget
            .layout_data(target)
            .translated(rect.left, rect.top)
            .map(move |command| ListCommand::Child(index, command))
    }
}

impl<'a, T, K> Widget<'a, ListCommand<T::Command>> for ListWidget<'a, T, K>
where
    T: View + Clone,
    K: Clone + Eq + Hash,
{
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<ListCommand<T::Command>> {
        match event {
            Event::Paint { .. } => {
                let canvas = match event {
                    Event::Paint { canvas, .. } => *canvas,
                    _ => unreachable!(),
                };
                let mut merged = EventResult::Ignored;
                self.for_visible(viewport, |cursor, rect| {
                    let Some(child_viewport) = viewport_for_child(viewport, rect) else {
                        return;
                    };

                    let entering = {
                        let index = cursor.index() as usize;
                        self.animations.iter().any(|animation| {
                            index >= animation.start
                                && index < animation.start + animation.len()
                                && cursor.element().height + 0.01
                                    < animation.targets()[index - animation.start]
                        })
                    };
                    if entering {
                        return;
                    }

                    if let Some(selection) = self.selection {
                        let index = cursor.index();

                        if !self.matches.is_empty() {
                            let matched = self
                                .matches
                                .query(index..index.saturating_add(1), Order::Ascending)
                                .any(|interval| interval.range.contains(&index));
                            if matched {
                                let fill = selection.style.fill;
                                let mut paint = Paint::default();
                                paint.set_color(Color::from_argb(
                                    fill.a() / 3,
                                    fill.r(),
                                    fill.g(),
                                    fill.b(),
                                ));
                                canvas.draw_rect(rect, &paint);
                            }
                        }
                        let covered = selection
                            .intervals
                            .query(index..index.saturating_add(1), Order::Ascending)
                            .any(|interval| interval.range.contains(&index));
                        if covered {
                            let style = selection.style;
                            let mut paint = Paint::default();
                            paint.set_anti_alias(true);
                            paint.set_color(style.fill);
                            canvas.draw_round_rect(rect, style.radius, style.radius, &paint);
                            if style.accent_width > 0.0 {
                                let mut accent = Paint::default();
                                accent.set_color(style.accent);
                                canvas.draw_rect(
                                    Rect::from_xywh(
                                        rect.left,
                                        rect.top + style.accent_inset,
                                        style.accent_width,
                                        (rect.height() - style.accent_inset * 2.0).max(0.0),
                                    ),
                                    &accent,
                                );
                            }
                        }
                    }
                    canvas.save();
                    canvas.translate((rect.left, rect.top));
                    canvas.clip_rect(Rect::from_size(rect.size()), None, true);

                    let index = cursor.index() as usize;
                    let widget = crate::Layout::layout(
                        cursor.element().view.display(arena, self.store, self.ui),
                        arena,
                        self.child_constraints,
                    );
                    let live = widget.size().height;
                    let result = widget
                        .focus_scope(self.focused == Some(index))
                        .realize(arena, child_viewport)
                        .handle_event(arena, event, child_viewport)
                        .map(move |command| ListCommand::Child(index, command));
                    merged = std::mem::replace(&mut merged, EventResult::Ignored).merge(result);
                    canvas.restore();

                    let animating = self.animations.iter().any(|animation| {
                        (animation.start..animation.start + animation.len()).contains(&index)
                    });
                    if !animating && (live - cursor.element().height).abs() > 0.5 {
                        merged = std::mem::replace(&mut merged, EventResult::Ignored).merge(
                            EventResult::Command(ListCommand::SetHeight(
                                cursor.index() as usize,
                                live,
                            )),
                        );
                    }

                    if let Some(style) = self.separators {
                        if cursor.index() > 0 {
                            let mut paint = Paint::default();
                            paint.set_anti_alias(true);
                            paint.set_color(style.color);
                            canvas.draw_rect(
                                Rect::from_xywh(
                                    rect.left + style.inset,
                                    rect.top - style.thickness * 0.5,
                                    (rect.width() - style.inset * 2.0).max(0.0),
                                    style.thickness,
                                ),
                                &paint,
                            );
                        }
                    }
                });
                // Re-observe the viewport top from paint too — the
                // belt for programmatic `set_scroll_y` placements,
                // which raise no pulse (docs §3.1).
                if (viewport.top - self.viewport_top).abs() > 0.5 {
                    merged = std::mem::replace(&mut merged, EventResult::Ignored)
                        .merge(EventResult::Command(ListCommand::ViewportTop(viewport.top)));
                }
                match merged {
                    EventResult::Ignored => EventResult::Handled,
                    merged => merged,
                }
            }

            Event::AnimationClock { .. } | Event::Settle | Event::ThemeChanged => {
                let mut merged = EventResult::Ignored;
                self.for_visible(viewport, |cursor, rect| {
                    let child_viewport = viewport_for_child(viewport, rect).unwrap_or_default();
                    let result = self.route(cursor, arena, event, child_viewport);
                    merged = std::mem::replace(&mut merged, EventResult::Ignored).merge(result);
                });
                if let Event::Settle = event {
                    // The pulse delivers the honest viewport: a
                    // drifted retained top means the scroll moved
                    // since the last observation — refresh it FIRST.
                    // Its perform also drops any pending correction
                    // (the move supersedes the door), so no reveal
                    // rides this round; the next round, if any,
                    // speaks from fresh state.
                    if (viewport.top - self.viewport_top).abs() > 0.5 {
                        let mine = EventResult::Command(ListCommand::ViewportTop(viewport.top));
                        merged = std::mem::replace(&mut merged, EventResult::Ignored).merge(mine);
                        return merged;
                    }
                    // The deepest anchor speaks: a row's inner editor
                    // that already answered owns the corner; the
                    // list's own note only fills silence. The target
                    // is absolute, so re-emitting until the next
                    // Viewport report clears it converges at the
                    // scroll instead of compounding.
                    if !matches!(merged, EventResult::Reveal(_)) {
                        if let Some(fresh) = self.settle_to {
                            let mine = EventResult::Reveal(crate::event::Reveal::top_left_at(
                                Rect::from_xywh(0.0, fresh, self.size.width, viewport.height()),
                            ));
                            merged =
                                std::mem::replace(&mut merged, EventResult::Ignored).merge(mine);
                        }
                    }
                    return match merged {
                        EventResult::Ignored => EventResult::Handled,
                        merged => merged,
                    };
                }
                if let Event::AnimationClock { now } = event {
                    if !self.animations.is_empty() {
                        merged = std::mem::replace(&mut merged, EventResult::Ignored)
                            .merge(EventResult::Command(ListCommand::Animate(*now)));
                    }

                    if self.reveal_lost {
                        merged = std::mem::replace(&mut merged, EventResult::Ignored)
                            .merge(EventResult::Command(ListCommand::Revealed));
                    }
                    if let Some((rect, placement)) = self.reveal {
                        let mine = match placement {
                            crate::event::Placement::EnsureVisible => {
                                match crate::event::reveal_satisfied(viewport, rect) {
                                    true => EventResult::Command(ListCommand::Revealed),
                                    false => {
                                        EventResult::Reveal(crate::event::Reveal::visible(rect))
                                    }
                                }
                            }
                            crate::event::Placement::TopLeftAt => {
                                // The scroll clamps at the end of the
                                // list; satisfaction must measure
                                // against the clamped target or a
                                // tail row re-emits forever.
                                let best = rect
                                    .top
                                    .min((self.size.height - viewport.height()).max(0.0));
                                match (viewport.top - best).abs() < 0.5 {
                                    true => EventResult::Command(ListCommand::Revealed),
                                    false => {
                                        EventResult::Reveal(crate::event::Reveal::top_left_at(rect))
                                    }
                                }
                            }
                        };
                        merged = std::mem::replace(&mut merged, EventResult::Ignored).merge(mine);
                    }
                }
                merged
            }

            Event::MouseDown { point, .. }
            | Event::Scroll { point, .. }
            | Event::MouseMove { point }
            | Event::HitTest { point, .. } => {
                let Some(cursor) = self.cursor_at_y(point.y) else {
                    return EventResult::Ignored;
                };
                let rect = self.row_rect(&cursor);
                if point.x < rect.left || point.x >= rect.right || point.y < rect.top {
                    return EventResult::Ignored;
                }
                let child_viewport = viewport_for_child(viewport, rect).unwrap_or_default();
                let local = event.translated(-rect.left, -rect.top);
                let result = self.route(&cursor, arena, &local, child_viewport);
                match event {
                    Event::MouseDown { .. } => {
                        let index = cursor.index() as usize;
                        match result {
                            EventResult::Command(command) => EventResult::Command(
                                ListCommand::Focus(index, Some(Box::new(command))),
                            ),
                            _ => EventResult::Command(ListCommand::Focus(index, None)),
                        }
                    }
                    _ => result,
                }
            }

            _ => {
                let Some(focused) = self.focused else {
                    return EventResult::Ignored;
                };
                if self.items.is_empty() {
                    return EventResult::Ignored;
                }
                let mut cursor = self.items.cursor();
                if !cursor.seek_to_index(focused as u32) {
                    return EventResult::Ignored;
                }
                let rect = self.row_rect(&cursor);
                let child_viewport = viewport_for_child(viewport, rect).unwrap_or_default();
                // Into ROW coordinates, like the hit-tested arm above —
                // a drag routed to the focused row with list-level
                // points lands past the row's content (selects to the
                // end of a chat cell); point-less events pass through
                // `translated` untouched.
                let local = event.translated(-rect.left, -rect.top);
                self.route(&cursor, arena, &local, child_viewport)
            }
        }
    }
}

#[cfg(test)]
mod tests;
