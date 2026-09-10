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

struct ListElement<T> {
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

struct ListMeasure;

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
        Metrics([element.height.ceil() as u32])
    }
}

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

    pub fn push(&mut self, view: T, height: f32) {
        self.pending.push(ListElement { view, height });
    }

    pub fn push_keyed(&mut self, key: K, view: T, height: f32) {
        let index = self.len() as u32;
        self.push(view, height);
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

    animations: Vec<SpliceAnimation>,

    generation: u64,
}

pub enum ListCommand<C> {
    Child(usize, C),

    Focus(usize, Option<Box<ListCommand<C>>>),

    SetHeight(usize, f32),

    Animate(AnimationClock),

    Revealed,
}

impl<T: Clone, K: Clone + Eq + Hash> ListView<T, K> {
    pub fn from_measured(items: impl IntoIterator<Item = (T, f32)>) -> Self {
        Self::from_measured_at(f32::NAN, items)
    }

    pub fn from_measured_at(width: f32, items: impl IntoIterator<Item = (T, f32)>) -> Self {
        Self {
            items: Rope::from_iter(
                items
                    .into_iter()
                    .map(|(view, height)| ListElement { view, height }),
            ),
            structure: Intervals::new(),
            selection: None,
            matches: Intervals::new(),
            focused: None,
            separators: None,
            laid_width: std::sync::atomic::AtomicU32::new(width.to_bits()),
            animations: Vec::new(),
            generation: 0,
        }
    }

    pub fn from_slice_at(width: f32, slice: ListSlice<T, K>) -> Self {
        let mut list = Self::from_slice(slice);
        list.laid_width = std::sync::atomic::AtomicU32::new(width.to_bits());
        list
    }

    pub fn from_slice(slice: ListSlice<T, K>) -> Self {
        let slice = slice.sealed();
        let mut list = Self::from_measured([]);
        list.items = slice.items;

        list.structure.insert(slice.spans);
        list
    }

    pub fn with_separators(mut self, style: SeparatorStyle) -> Self {
        self.separators = Some(style);
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

    fn splice_impl(&mut self, range: std::ops::Range<usize>, slice: ListSlice<T, K>) {
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

            animations: self.animations.clone(),
            generation: self.generation,
        }
    }
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
                    let live = element
                        .view
                        .layout(
                            &Arena::default(),
                            store,
                            ui,
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
            ListCommand::Revealed => {
                if let Some(selection) = &mut self.selection {
                    selection.reveal = false;
                }
            }
            ListCommand::SetHeight(index, height) => {
                if self.items.is_empty() {
                    return;
                }
                let mut cursor = self.items.cursor();
                if !cursor.seek_to_index(index as u32) {
                    return;
                }
                let mut element = cursor.element().clone();
                element.height = height;
                cursor.delete(1u32);
                cursor.insert(Rope::from_iter([element]));
                self.items = cursor.rope();
            }
        }
    }

    fn layout<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let width = constraints.max.width;
        self.laid_width
            .store(width.to_bits(), std::sync::atomic::Ordering::Relaxed);

        let reveal = self.selection.as_ref().and_then(|selection| {
            if !selection.reveal {
                return None;
            }
            let index = self.own_row_index(selection.cursor.as_ref()?)?;
            let mut cursor = self.items.cursor();
            if !cursor.seek_to_index(index as u32) {
                return None;
            }
            let top = cursor.position().metric_at(ROW_PX) as f32;
            let height = cursor.element_metrics().metric_at(ROW_PX) as f32;
            Some(Rect::from_xywh(0.0, top, width.max(1.0), height))
        });
        ListWidget {
            items: &self.items,
            selection: self.selection.as_ref(),
            matches: &self.matches,
            reveal,
            animations: &self.animations,
            separators: self.separators,
            store,
            ui,
            child_constraints: Constraints {
                min: Size::default(),
                max: Size::new(width, f32::MAX),
            },
            size: Size::new(width, self.items.metrics().metric_at(ROW_PX) as f32),
            focused: self.focused,
        }
    }
}

struct ListWidget<'a, T: Clone, K: Clone + Eq + Hash> {
    items: &'a Rope<ListElement<T>, ListMeasure>,
    selection: Option<&'a SelectionState<K>>,
    matches: &'a Intervals<K, ()>,

    reveal: Option<Rect>,
    animations: &'a [SpliceAnimation],
    separators: Option<SeparatorStyle>,
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
        cursor
            .element()
            .view
            .layout(arena, self.store, self.ui, self.child_constraints)
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

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<ListCommand<T::Command>> {
        self.list.handle_event(arena, event, viewport)
    }

    fn focus_data<'w>(&'w mut self) -> crate::focus::FocusData<'w, ListCommand<T::Command>>
    where
        'a: 'w,
    {
        use crate::event::EventResult;
        use crate::focus::FocusData;
        let Some(index) = self.list.focused else {
            return FocusData::default();
        };
        if self.list.items.is_empty() {
            return FocusData::default();
        }
        let list = &self.list;
        let arena = self.arena;
        let viewport = self.viewport;
        let commands = with_focused_row(list, arena, viewport, index, |widget| {
            std::mem::take(&mut widget.focus_data().commands)
        })
        .unwrap_or_default()
        .into_iter()
        .map(|presentable| presentable.map(|command| ListCommand::Child(index, command)))
        .collect();
        FocusData {
            commands,
            on_key: Some(Box::new(move |key, mods| {
                with_focused_row(list, arena, viewport, index, |widget| {
                    widget
                        .focus_data()
                        .key(key, mods)
                        .map(|command| ListCommand::Child(index, command))
                })
                .unwrap_or(EventResult::Ignored)
            })),
            on_text: Some(Box::new(move |text| {
                with_focused_row(list, arena, viewport, index, |widget| {
                    widget
                        .focus_data()
                        .text(text)
                        .map(|command| ListCommand::Child(index, command))
                })
                .unwrap_or(EventResult::Ignored)
            })),
            ime: Some(crate::focus::ImeSeat {
                origin: skia_safe::Point::default(),
                clip: None,
                ask: Box::new(move |origin, clip, visit| {
                    with_focused_row_at(list, arena, viewport, index, |widget, rect| {
                        match widget.focus_data().ime.take() {
                            Some(mut seat) => {
                                let at = skia_safe::Point::new(
                                    origin.x + rect.left + seat.origin.x,
                                    origin.y + rect.top + seat.origin.y,
                                );
                                (seat.ask)(at, clip, visit)
                                    .map(|command| ListCommand::Child(index, command))
                            }
                            None => EventResult::Ignored,
                        }
                    })
                    .unwrap_or(EventResult::Ignored)
                }),
            }),
            clipboard: Some(Box::new(move |visit| {
                with_focused_row(list, arena, viewport, index, |widget| {
                    match widget.focus_data().clipboard.as_mut() {
                        Some(seat) => seat(visit).map(|command| ListCommand::Child(index, command)),
                        None => EventResult::Ignored,
                    }
                })
                .unwrap_or(EventResult::Ignored)
            })),
            location: with_focused_row(list, arena, viewport, index, |widget| {
                widget.focus_data().location.take()
            })
            .flatten(),
        }
    }
}

fn with_focused_row<'a, T, K, R>(
    list: &ListWidget<'a, T, K>,
    arena: &'a Arena,
    viewport: Rect,
    index: usize,
    f: impl FnOnce(&mut crate::WidgetBox<'_, T::Command>) -> R,
) -> Option<R>
where
    T: View + Clone,
    K: Clone + Eq + Hash,
{
    with_focused_row_at(list, arena, viewport, index, |widget, _rect| f(widget))
}

fn with_focused_row_at<'a, T, K, R>(
    list: &ListWidget<'a, T, K>,
    arena: &'a Arena,
    viewport: Rect,
    index: usize,
    f: impl FnOnce(&mut crate::WidgetBox<'_, T::Command>, Rect) -> R,
) -> Option<R>
where
    T: View + Clone,
    K: Clone + Eq + Hash,
{
    let mut cursor = list.items.cursor();
    if !cursor.seek_to_index(index as u32) {
        return None;
    }
    let rect = list.row_rect(&cursor);
    let child_viewport = viewport_for_child(viewport, rect).unwrap_or_default();
    let mut widget = cursor
        .element()
        .view
        .layout(arena, list.store, list.ui, list.child_constraints)
        .realize(arena, child_viewport);
    Some(f(&mut widget, rect))
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
                    let widget = cursor.element().view.layout(
                        arena,
                        self.store,
                        self.ui,
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
                match merged {
                    EventResult::Ignored => EventResult::Handled,
                    merged => merged,
                }
            }

            Event::AnimationClock { .. } | Event::ThemeChanged => {
                let mut merged = EventResult::Ignored;
                self.for_visible(viewport, |cursor, rect| {
                    let child_viewport = viewport_for_child(viewport, rect).unwrap_or_default();
                    let result = self.route(cursor, arena, event, child_viewport);
                    merged = std::mem::replace(&mut merged, EventResult::Ignored).merge(result);
                });
                if let Event::AnimationClock { now } = event {
                    if !self.animations.is_empty() {
                        merged = std::mem::replace(&mut merged, EventResult::Ignored)
                            .merge(EventResult::Command(ListCommand::Animate(*now)));
                    }

                    if let Some(rect) = self.reveal {
                        let mine = match crate::event::reveal_satisfied(viewport, rect) {
                            true => EventResult::Command(ListCommand::Revealed),
                            false => EventResult::Reveal(rect),
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
                self.route(&cursor, arena, event, child_viewport)
            }
        }
    }
}

#[cfg(test)]
mod tests;
