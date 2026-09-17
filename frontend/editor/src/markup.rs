// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::{ops::Range, sync::Arc};

use imba::{arena::Arena, constraints::Constraints, store::Store, DynCommand, Thunk, UiCtx, View};
use intervals::{Interval, IntervalQuery, Intervals, Order};
use operation::{Op, Operation};
use skia_safe::Size;

pub(crate) fn interval_steps(
    operation: &Operation,
) -> impl Iterator<Item = intervals::EditStep> + '_ {
    operation.iter().map(|op| match op {
        Op::Retain(len) => intervals::EditStep::Retain(len),
        Op::Insert(text) => intervals::EditStep::Insert(text.len() as u32),
        Op::Delete(text) => intervals::EditStep::Delete(text.len() as u32),
    })
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct MarkupId(pub(crate) u64);

impl MarkupId {
    pub fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IntervalId(pub(crate) u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkupScope {
    View,
    Document,
}

#[derive(Clone)]
pub struct Markup {
    /// The BULK lane: plain styled spans — a syntax highlighter puts
    /// tens of thousands here. Nothing structural lives in it.
    styles: Intervals<IntervalId, Decoration>,

    /// The STRUCTURE lane: inlays, hidden/unhide, alignment and the
    /// nested-syntax mounts — low cardinality, and the only tree the
    /// line-shaping and inlay queries ever touch.
    shape: Intervals<IntervalId, Decoration>,
    next_key: u32,

    scope: MarkupScope,

    syntaxes: rpds::HashTrieMapSync<SyntaxId, Syntax>,

    has_inlays: bool,

    has_popups: bool,
}

impl IntervalQuery<IntervalId, Decoration> for Markup {
    type Iter<'a>
        = RecursiveQuery<'a>
    where
        Self: 'a;

    fn query(&self, range: Range<u32>, order: Order) -> Self::Iter<'_> {
        RecursiveQuery::new(self, range, order)
    }
}

pub(crate) struct RecursiveQuery<'a> {
    levels: Vec<RecursiveLevel<'a>>,
    range: Range<u32>,
    order: Order,
}

struct RecursiveLevel<'a> {
    base: u32,
    markup: &'a Markup,

    layer: MarkupLayer,
    iter: intervals::MergedQuery<
        intervals::Query<'a, IntervalId, Decoration>,
        intervals::Query<'a, IntervalId, Decoration>,
    >,

    peeked: Option<intervals::IntervalRef<'a, IntervalId, Decoration>>,
}

impl<'a> RecursiveLevel<'a> {
    fn new(
        markup: &'a Markup,
        base: u32,
        range: &Range<u32>,
        order: Order,
        layer: MarkupLayer,
    ) -> Option<Self> {
        if range.end <= base {
            return None;
        }
        let local = range.start.saturating_sub(base)..range.end - base;
        let mut level = Self {
            base,
            markup,
            layer,
            iter: intervals::MergedQuery::new(
                markup.shape.query(local.clone(), order),
                markup.styles.query(local, order),
                order,
            ),
            peeked: None,
        };
        level.refill();
        Some(level)
    }

    fn refill(&mut self) {
        self.peeked = self.iter.next().map(|mut interval| {
            interval.range = interval.range.start.saturating_add(self.base)
                ..interval.range.end.saturating_add(self.base);
            interval
        });
    }
}

impl<'a> RecursiveQuery<'a> {
    fn new(markup: &'a Markup, range: Range<u32>, order: Order) -> Self {
        Self::over(std::iter::once((MarkupLayer::Syntax, markup)), range, order)
    }

    fn over(
        markups: impl IntoIterator<Item = (MarkupLayer, &'a Markup)>,
        range: Range<u32>,
        order: Order,
    ) -> Self {
        let levels = markups
            .into_iter()
            .filter_map(|(layer, markup)| RecursiveLevel::new(markup, 0, &range, order, layer))
            .collect();
        Self {
            levels,
            range,
            order,
        }
    }

    fn next_keyed(
        &mut self,
    ) -> Option<(
        MarkupLayer,
        intervals::IntervalRef<'a, IntervalId, Decoration>,
    )> {
        let mut best: Option<usize> = None;
        for (index, level) in self.levels.iter().enumerate() {
            let Some(peeked) = &level.peeked else {
                continue;
            };
            best = match best {
                Some(current) => {
                    let start = self.levels[current]
                        .peeked
                        .as_ref()
                        .map(|item| item.range.start);
                    let wins = match self.order {
                        Order::Ascending => Some(peeked.range.start) < start,
                        Order::Descending => Some(peeked.range.start) > start,
                    };
                    match wins {
                        true => Some(index),
                        false => Some(current),
                    }
                }
                None => Some(index),
            };
        }
        let index = best?;
        let item = self.levels[index].peeked.take().expect("peeked");
        let owner = self.levels[index].markup;
        let layer = self.levels[index].layer;
        self.levels[index].refill();
        if self.levels[index].peeked.is_none() {
            self.levels.remove(index);
        }
        if let Decoration::Syntax(id) = item.value {
            if let Some(syntax) = owner.syntaxes.get(id) {
                if let Some(level) = RecursiveLevel::new(
                    &syntax.markup,
                    item.range.start,
                    &self.range,
                    self.order,
                    MarkupLayer::Syntax,
                ) {
                    self.levels.push(level);
                }
            }
        }
        Some((layer, item))
    }
}

impl<'a> Iterator for RecursiveQuery<'a> {
    type Item = intervals::IntervalRef<'a, IntervalId, Decoration>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_keyed().map(|(_, item)| item)
    }
}

pub use crate::theme::{StyleId, TextAlignment, TextAttributes};

#[derive(Clone)]
pub(crate) enum Decoration {
    Styled(StyleId),

    Hidden,

    Unhide,
    Inlay(Inlay),

    Alignment(crate::theme::TextAlignment),

    Syntax(SyntaxId),
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct BlockStyle {
    ids: Vec<StyleId>,

    continues_past: bool,

    alignment: Option<crate::theme::TextAlignment>,
}

impl BlockStyle {
    fn add(&mut self, id: StyleId) {
        if !self.ids.contains(&id) {
            self.ids.push(id);
        }
    }

    pub(crate) fn continues_past(&self) -> bool {
        self.continues_past
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn ids(&self) -> &[StyleId] {
        &self.ids
    }

    pub fn resolved(&self, theme: &crate::theme::Theme) -> TextAttributes {
        let mut merged = theme.base().clone();
        for id in &self.ids {
            match id {
                StyleId::Indent(level) => {
                    let entry = theme.attributes(StyleId::Indent(0));
                    if let Some(inset) = entry.inset {
                        merged.inset =
                            Some(merged.inset.unwrap_or(0.0) + inset * f32::from(*level));
                    }
                }
                id => merged.merge(theme.attributes(*id)),
            }
        }
        if self.alignment.is_some() {
            merged.alignment = self.alignment;
        }
        merged
    }
}

pub struct Syntax {
    pub language: String,

    pub tree: Option<Box<dyn crate::reparse::SyntaxTree>>,

    pub markup: Markup,

    pub folds: Intervals<IntervalId, ()>,

    pub outline: Intervals<IntervalId, OutlineItem>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct OutlineItem {
    pub title: String,
}

pub(crate) fn splice_intervals<V: Clone>(
    tree: &mut Intervals<IntervalId, V>,
    invalidated: &[Range<u32>],
    replacement: impl IntoIterator<Item = (Range<u32>, V)>,
) {
    let mut stale = Vec::new();
    let mut next = 0u32;
    for entry in tree.query(0..u32::MAX, Order::Ascending) {
        next = next.max(entry.key.0.saturating_add(1));
        if invalidated
            .iter()
            .any(|range| entry.range.start < range.end && range.start < entry.range.end)
        {
            stale.push(*entry.key);
        }
    }
    tree.remove(stale.iter());
    tree.insert(replacement.into_iter().filter_map(|(range, value)| {
        (range.start < range.end).then(|| {
            let key = IntervalId(next);
            next = next.wrapping_add(1);
            Interval {
                range,
                greedy_left: false,
                greedy_right: false,
                key,
                value,
            }
        })
    }));
}

impl Syntax {
    pub fn push_outline_item(&mut self, range: Range<u32>, item: OutlineItem) -> IntervalId {
        let next = self
            .outline
            .query(0..u32::MAX, Order::Ascending)
            .map(|entry| entry.key.0.saturating_add(1))
            .max()
            .unwrap_or(0);
        let key = IntervalId(next);
        self.outline.insert([Interval {
            range,
            greedy_left: false,
            greedy_right: false,
            key,
            value: item,
        }]);
        key
    }

    pub fn enclosing_at(&self, byte: u32, extent: Range<u32>) -> (&Syntax, Range<u32>) {
        let mut syntax = self;
        let mut range = extent;
        loop {
            match syntax.markup.child_syntax_at(byte, range.start) {
                Some((child, child_range)) => {
                    syntax = child;
                    range = child_range;
                }
                None => return (syntax, range),
            }
        }
    }

    pub fn new(
        language: impl Into<String>,
        tree: Option<Box<dyn crate::reparse::SyntaxTree>>,
        markup: Markup,
    ) -> Self {
        Self {
            language: language.into(),
            tree,
            markup,
            folds: Intervals::new(),
            outline: Intervals::new(),
        }
    }
}

impl Clone for Syntax {
    fn clone(&self) -> Self {
        Self {
            language: self.language.clone(),
            tree: self.tree.as_ref().map(|tree| tree.clone_tree()),
            markup: self.markup.clone(),
            folds: self.folds.clone(),
            outline: self.outline.clone(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SyntaxId(pub(crate) u64);

impl SyntaxId {
    pub const DOCUMENT: SyntaxId = SyntaxId(u64::MAX);
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextDecorationInterval {
    pub range: Range<u32>,
    pub id: StyleId,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum InlayAlignment {
    Top,

    Middle,

    #[allow(dead_code)]
    Bottom,
}

#[derive(Clone, Copy)]
pub(crate) struct InlayPlaceholder {
    pub(crate) byte: u32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) align: InlayAlignment,
}

/// The host for `Inlay::over` inlays: a pane declares it between its
/// vertical scroll and the gutter/horizontal-scroll container, so a
/// projected strip spans the pane edge to edge and still scrolls with
/// its line. A split declares it ONCE above both panes — the strip is
/// shared.
pub const INLAY_HOST: imba::overlay::OverlayHost = imba::overlay::OverlayHost("editor.inlays");

/// How a projected inlay sits inside its host.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum InlayProjection {
    /// Edge to edge of the host (the fold strip — in a split it runs
    /// across both panes regardless of which pane minted it).
    Span,

    /// At the minting editor's left edge, host-wide to the right (the
    /// deleted-code card, whose own gutter lines up with the host's).
    Aligned,
}

#[derive(Clone)]
pub struct Inlay {
    pub(crate) mode: InlayMode,

    view: Arc<dyn InlayView>,

    /// A PROJECTED inlay keeps reserving its space in the text flow,
    /// but renders through the named overlay host instead of inline —
    /// the fold strip that spans the whole pane (and, hosted above a
    /// split, both panes at once) while still scrolling with its line.
    pub(crate) overlay: Option<(imba::overlay::OverlayHost, InlayProjection)>,
}

pub(crate) trait InlayView: Send + Sync {
    fn clone_view(&self) -> Box<dyn InlayView>;

    fn carry_live(&self) -> bool {
        false
    }

    fn passive_view(&self, _command: &InlayCommand) -> bool {
        false
    }

    /// The inlay's SEMANTIC focus answers — a state walk into the
    /// wrapped view, nothing laid. Geometry (the IME rect) comes from
    /// the realized inlay widget's `layout_data` instead.
    fn focus_view<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, InlayCommand>;

    fn perform_view(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        range: &Range<u32>,
        command: InlayCommand,
        fx: &mut imba::effect::Effects<'_, InlayCommand>,
    ) -> Option<Operation>;
    fn layout_view<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> imba::ThunkBox<'a, InlayCommand>;

    fn as_any(&self) -> &dyn std::any::Any;

    fn adopt_from(
        &mut self,
        previous: &dyn std::any::Any,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    );

    fn destroy_view(
        &mut self,
        _store: &mut Store,
        _fx: &mut imba::effect::Effects<'_, InlayCommand>,
    ) {
    }
}

pub trait InlayEditing {
    fn take_edit(&mut self) -> Option<Operation>;

    fn set_range(&mut self, range: Range<u32>);

    fn adopt_from(
        &mut self,
        previous: &Self,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> bool;

    fn passive(&self, _command: &InlayCommand) -> bool {
        false
    }
}

struct PlainInlayView<V>(V);

impl<V> InlayView for PlainInlayView<V>
where
    V: View + Clone + Send + Sync + 'static,
    V::Command: Send + Sync + 'static,
{
    fn clone_view(&self) -> Box<dyn InlayView> {
        Box::new(PlainInlayView(self.0.clone()))
    }

    fn perform_view(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        _range: &Range<u32>,
        command: InlayCommand,
        fx: &mut imba::effect::Effects<'_, InlayCommand>,
    ) -> Option<Operation> {
        imba::DynView::perform_dyn(&mut self.0, store, ui, command, fx);
        None
    }

    fn layout_view<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> imba::ThunkBox<'a, InlayCommand> {
        imba::DynView::layout_dyn(&self.0, arena, store, ui, constraints)
    }

    fn focus_view<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, InlayCommand> {
        imba::DynView::focus_data_dyn(&self.0, store, ui)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        &self.0
    }

    fn adopt_from(
        &mut self,
        _previous: &dyn std::any::Any,
        _fonts: &skia_safe::textlayout::FontCollection,
        _theme: &crate::theme::Theme,
    ) {
    }

    fn destroy_view(
        &mut self,
        store: &mut Store,
        fx: &mut imba::effect::Effects<'_, InlayCommand>,
    ) {
        imba::DynView::destroy_dyn(&mut self.0, store, fx);
    }
}

struct EditingInlayView<V> {
    view: V,

    carry_live: bool,
}

impl<V> InlayView for EditingInlayView<V>
where
    V: View + InlayEditing + Clone + Send + Sync + 'static,
    V::Command: Send + Sync + 'static,
{
    fn clone_view(&self) -> Box<dyn InlayView> {
        Box::new(EditingInlayView {
            view: self.view.clone(),
            carry_live: self.carry_live,
        })
    }

    fn carry_live(&self) -> bool {
        self.carry_live
    }

    fn perform_view(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        range: &Range<u32>,
        command: InlayCommand,
        fx: &mut imba::effect::Effects<'_, InlayCommand>,
    ) -> Option<Operation> {
        self.view.set_range(range.clone());
        imba::DynView::perform_dyn(&mut self.view, store, ui, command, fx);
        self.view.take_edit()
    }

    fn layout_view<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> imba::ThunkBox<'a, InlayCommand> {
        imba::DynView::layout_dyn(&self.view, arena, store, ui, constraints)
    }

    fn passive_view(&self, command: &InlayCommand) -> bool {
        self.view.passive(command)
    }

    fn focus_view<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, InlayCommand> {
        imba::DynView::focus_data_dyn(&self.view, store, ui)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        &self.view
    }

    fn adopt_from(
        &mut self,
        previous: &dyn std::any::Any,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) {
        if let Some(previous) = previous.downcast_ref::<V>() {
            self.carry_live = InlayEditing::adopt_from(&mut self.view, previous, fonts, theme);
        }
    }

    fn destroy_view(
        &mut self,
        store: &mut Store,
        fx: &mut imba::effect::Effects<'_, InlayCommand>,
    ) {
        imba::DynView::destroy_dyn(&mut self.view, store, fx);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InlayMode {
    Left,
    Right,
    Under,
    Above,

    Instead(InsteadKind),

    Popup(PopupSpec),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PopupSpec {
    pub host: imba::overlay::OverlayHost,
    pub position: imba::overlay::fit::PreferredPosition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsteadKind {
    FullLine,

    Inline,
}

pub type InlayCommand = DynCommand;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MarkupLayer {
    Syntax,

    Markup(MarkupId),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InlayKey {
    pub(crate) layer: MarkupLayer,
    pub(crate) key: IntervalId,
}

impl InlayKey {
    pub fn in_markup(markup: MarkupId, key: u32) -> Self {
        Self {
            layer: MarkupLayer::Markup(markup),
            key: IntervalId(key),
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub(crate) struct InlayMetrics {
    pub(crate) above_height: f32,
    pub(crate) under_height: f32,
    pub(crate) inline_height: f32,
    pub(crate) instead_height: f32,

    pub(crate) instead_covered: bool,
}

pub struct MarkupBuilder {
    next_key: u32,
    intervals: Vec<Interval<IntervalId, Decoration>>,

    pub(crate) folds: Vec<Range<u32>>,
    pub(crate) outline: Vec<(Range<u32>, OutlineItem)>,
}

impl Markup {
    fn mint(&mut self) -> IntervalId {
        let key = IntervalId(self.next_key);
        self.next_key = self.next_key.wrapping_add(1);
        key
    }

    fn is_shape(value: &Decoration) -> bool {
        !matches!(value, Decoration::Styled(_))
    }

    fn insert_split(&mut self, items: impl IntoIterator<Item = Interval<IntervalId, Decoration>>) {
        let (shape, styles): (Vec<_>, Vec<_>) = items
            .into_iter()
            .partition(|interval| Self::is_shape(&interval.value));
        self.shape.insert(shape);
        self.styles.insert(styles);
    }

    /// Both lanes, merged in offset order — for whole-markup reads
    /// (oracles, set diffs, carries). Hot paths use one lane.
    pub(crate) fn merged_query(
        &self,
        range: Range<u32>,
        order: Order,
    ) -> intervals::MergedQuery<
        intervals::Query<'_, IntervalId, Decoration>,
        intervals::Query<'_, IntervalId, Decoration>,
    > {
        intervals::MergedQuery::new(
            self.shape.query(range.clone(), order),
            self.styles.query(range, order),
            order,
        )
    }

    fn find_any(
        &self,
        key: &IntervalId,
    ) -> Option<intervals::IntervalRef<'_, IntervalId, Decoration>> {
        self.shape
            .find_by_id(key)
            .or_else(|| self.styles.find_by_id(key))
    }

    pub fn new() -> Self {
        Self {
            styles: Intervals::new(),
            shape: Intervals::new(),
            next_key: 0,
            scope: MarkupScope::View,
            syntaxes: rpds::HashTrieMapSync::new_sync(),
            has_inlays: false,
            has_popups: false,
        }
    }

    pub(crate) fn scope(&self) -> MarkupScope {
        self.scope
    }

    pub(crate) fn set_scope(&mut self, scope: MarkupScope) {
        self.scope = scope;
    }

    /// Seeds plain styled intervals (zero-length markers kept — the
    /// change map's deletion points ride them).
    pub(crate) fn seed_styled(&mut self, hits: impl IntoIterator<Item = (Range<u32>, StyleId)>) {
        let mut next = self.next_key;
        let seeded: Vec<Interval<IntervalId, Decoration>> = hits
            .into_iter()
            .map(|(range, id)| {
                let key = IntervalId(next);
                next = next.wrapping_add(1);
                Interval {
                    range,
                    greedy_left: false,
                    greedy_right: false,
                    key,
                    value: Decoration::Styled(id),
                }
            })
            .collect();
        self.next_key = next;
        self.styles.insert(seeded);
    }

    pub fn builder() -> MarkupBuilder {
        MarkupBuilder::new()
    }

    pub fn is_empty(&self) -> bool {
        self.shape.is_empty() && self.styles.is_empty()
    }

    pub fn interval_count(&self) -> usize {
        self.shape.len() + self.styles.len()
    }

    pub fn query_count(&self) -> usize {
        self.merged_query(0..u32::MAX, Order::Ascending).count()
    }

    pub(crate) fn has_inlays(&self) -> bool {
        self.has_inlays
    }

    pub(crate) fn has_popups(&self) -> bool {
        self.has_popups
    }
}

impl Decoration {
    pub(crate) fn fingerprint(&self) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        match self {
            Decoration::Styled(id) => {
                0u8.hash(&mut hasher);
                id.hash(&mut hasher);
            }
            Decoration::Hidden => 1u8.hash(&mut hasher),
            Decoration::Unhide => 2u8.hash(&mut hasher),
            Decoration::Alignment(alignment) => {
                3u8.hash(&mut hasher);
                alignment.hash(&mut hasher);
            }
            Decoration::Inlay(_) | Decoration::Syntax(_) => return None,
        }
        Some(hasher.finish())
    }
}

impl Markup {
    pub fn empty() -> &'static Markup {
        static EMPTY: std::sync::OnceLock<Markup> = std::sync::OnceLock::new();
        EMPTY.get_or_init(Markup::new)
    }
}

impl Default for Markup {
    fn default() -> Self {
        Self::new()
    }
}

/// The set difference of two markups as damage ranges — entries
/// present on one side only (position + payload fingerprint; inlays
/// fingerprint by KEY so a carried widget is "unchanged"). The
/// producer-brings-the-change-set primitive: the marks landing and
/// the normalize worker both derive their `changed` through it.
pub fn set_diff(old: Option<&Markup>, new: &Markup) -> Vec<Range<u32>> {
    use intervals::{IntervalQuery, Order};
    let mut counts: std::collections::HashMap<(u32, u32, u64), i32> =
        std::collections::HashMap::new();
    let mut always: Vec<Range<u32>> = Vec::new();
    let empty = Markup::empty();
    for (markup, sign) in [(old.unwrap_or(empty), 1i32), (new, -1i32)] {
        for entry in markup.query(0..u32::MAX, Order::Ascending) {
            let fingerprint = match &entry.value {
                Decoration::Inlay(_) => {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::hash::DefaultHasher::new();
                    4u8.hash(&mut hasher);
                    entry.key.hash(&mut hasher);
                    Some(hasher.finish())
                }
                value => value.fingerprint(),
            };
            match fingerprint {
                Some(fingerprint) => {
                    *counts
                        .entry((entry.range.start, entry.range.end, fingerprint))
                        .or_insert(0) += sign;
                }
                None => always.push(entry.range.clone()),
            }
        }
    }
    let mut changed: Vec<Range<u32>> = counts
        .into_iter()
        .filter(|(_, count)| *count != 0)
        .map(|((start, end, _), _)| start..end)
        .collect();
    changed.extend(always);
    changed
}

#[derive(Clone, Copy)]
pub struct OverlaidMarkup<'e, 'a> {
    document: &'a Markup,

    extras: &'e [(MarkupId, &'a Markup)],
}

impl<'e, 'a> OverlaidMarkup<'e, 'a> {
    pub fn new(document: &'a Markup, extras: &'e [(MarkupId, &'a Markup)]) -> Self {
        Self { document, extras }
    }

    pub fn plain(document: &'a Markup) -> Self {
        Self {
            document,
            extras: &[],
        }
    }

    pub fn document(&self) -> &'a Markup {
        self.document
    }

    pub(crate) fn query(&self, range: Range<u32>, order: Order) -> RecursiveQuery<'a> {
        RecursiveQuery::over(
            std::iter::once((MarkupLayer::Syntax, self.document)).chain(
                self.extras
                    .iter()
                    .map(|(id, markup)| (MarkupLayer::Markup(*id), *markup)),
            ),
            range,
            order,
        )
    }

    pub fn block_marks_in(&self, range: Range<u32>) -> BlockStyle {
        let mut marks = BlockStyle::default();
        for decoration in self.query(range.clone(), Order::Ascending) {
            if intersects(&decoration.range, &range) {
                match decoration.value {
                    Decoration::Styled(id) => marks.add(*id),
                    Decoration::Alignment(alignment) => marks.alignment = Some(*alignment),
                    _ => {}
                }
            }
        }
        marks
    }

    pub fn marks_inline_hidden_in(
        &self,
        range: Range<u32>,
        inline: &mut Vec<TextDecorationInterval>,
        hidden: &mut Vec<Range<u32>>,
    ) -> BlockStyle {
        self.line_marks_in(range, None, inline, hidden).0
    }

    pub(crate) fn line_marks_in(
        &self,
        range: Range<u32>,
        width: Option<f32>,
        inline: &mut Vec<TextDecorationInterval>,
        hidden: &mut Vec<Range<u32>>,
    ) -> (BlockStyle, InlayMetrics) {
        self.line_marks_foldables_in(range, width, inline, hidden)
    }

    pub(crate) fn line_marks_foldables_in(
        &self,
        range: Range<u32>,
        width: Option<f32>,
        inline: &mut Vec<TextDecorationInterval>,
        hidden: &mut Vec<Range<u32>>,
    ) -> (BlockStyle, InlayMetrics) {
        classify_line_marks(
            self.query(range.clone(), Order::Ascending)
                .map(|decoration| (decoration.range, decoration.value)),
            &range,
            width,
            inline,
            hidden,
        )
    }

    pub(crate) fn line_marks_sweep(&self, from: u32, width: Option<f32>) -> LineMarksSweep<'a> {
        let mut iter = self.query(from..u32::MAX, Order::Ascending);
        let peeked = iter.next();
        LineMarksSweep {
            iter,
            peeked,
            active: Vec::new(),
            width,
            pulls: 0,
            active_peak: 0,
        }
    }
}

pub(crate) struct LineMarksSweep<'a> {
    iter: RecursiveQuery<'a>,
    peeked: Option<intervals::IntervalRef<'a, IntervalId, Decoration>>,

    active: Vec<intervals::IntervalRef<'a, IntervalId, Decoration>>,
    width: Option<f32>,

    pub(crate) pulls: usize,
    pub(crate) active_peak: usize,
}

impl<'a> LineMarksSweep<'a> {
    pub(crate) fn line(
        &mut self,
        range: Range<u32>,
        inline: &mut Vec<TextDecorationInterval>,
        hidden: &mut Vec<Range<u32>>,
    ) -> (BlockStyle, InlayMetrics) {
        while let Some(peeked) = &self.peeked {
            if peeked.range.start >= range.end {
                break;
            }
            let entered = self.peeked.take().expect("peeked");
            self.active.push(entered);
            self.peeked = self.iter.next();
            self.pulls += 1;
        }
        self.active_peak = self.active_peak.max(self.active.len());

        self.active.retain(|hit| hit.range.end >= range.start);
        classify_line_marks(
            self.active.iter().map(|hit| (hit.range.clone(), hit.value)),
            &range,
            self.width,
            inline,
            hidden,
        )
    }
}

fn classify_line_marks<'a>(
    hits: impl Iterator<Item = (Range<u32>, &'a Decoration)>,
    range: &Range<u32>,
    width: Option<f32>,
    inline: &mut Vec<TextDecorationInterval>,
    hidden: &mut Vec<Range<u32>>,
) -> (BlockStyle, InlayMetrics) {
    {
        let mut marks = BlockStyle::default();
        let mut metrics = InlayMetrics::default();
        let constraints = width.map(|width| Constraints {
            min: Size::default(),
            max: Size::new(width.max(1.0), f32::MAX),
        });
        inline.clear();
        hidden.clear();
        let mut unhide: Vec<Range<u32>> = Vec::new();
        let mut candidates: Vec<(Range<u32>, Range<u32>)> = Vec::new();
        for (hit_range, value) in hits {
            if !intersects(&hit_range, range) {
                continue;
            }
            let start = hit_range.start.max(range.start) - range.start;
            let end = hit_range.end.min(range.end) - range.start;
            match value {
                Decoration::Styled(id) if !id.is_block() => {
                    if start < end {
                        inline.push(TextDecorationInterval {
                            range: start..end,
                            id: *id,
                        });
                    }
                }
                Decoration::Styled(id) => {
                    marks.add(*id);
                    if hit_range.end > range.end {
                        marks.continues_past = true;
                    }
                }
                Decoration::Hidden => {
                    if start < end {
                        candidates.push((hit_range.clone(), start..end));
                    }
                }
                Decoration::Unhide => unhide.push(hit_range.clone()),
                Decoration::Alignment(alignment) => marks.alignment = Some(*alignment),
                Decoration::Inlay(inlay) => {
                    let Some(constraints) = constraints else {
                        continue;
                    };
                    if !inlay_anchors_line(inlay.mode, &hit_range, range) {
                        if inlay.mode == InlayMode::Instead(InsteadKind::FullLine) {
                            metrics.instead_covered = true;
                        }
                        continue;
                    }
                    let size = inlay.size(instead_constraints(inlay.mode, constraints));
                    match inlay.mode {
                        InlayMode::Left
                        | InlayMode::Right
                        | InlayMode::Instead(InsteadKind::Inline) => {
                            metrics.inline_height = metrics.inline_height.max(size.height.max(0.0));
                        }
                        InlayMode::Under => metrics.under_height += size.height.max(0.0),
                        InlayMode::Above => metrics.above_height += size.height.max(0.0),
                        InlayMode::Instead(InsteadKind::FullLine) => {
                            metrics.instead_height =
                                metrics.instead_height.max(size.height.max(0.0));
                        }

                        InlayMode::Popup(_) => {}
                    }
                }
                Decoration::Syntax(_) => {}
            }
        }
        hidden.extend(candidates.into_iter().filter_map(|(absolute, rebased)| {
            let revealed = unhide
                .iter()
                .any(|unhide| absolute.start < unhide.end && unhide.start < absolute.end);
            (!revealed).then_some(rebased)
        }));
        inline.sort_by_key(|interval| interval.range.start);
        (marks, metrics)
    }
}

impl<'e, 'a> OverlaidMarkup<'e, 'a> {
    pub(crate) fn has_inlays(&self) -> bool {
        self.document.has_inlays() || self.extras.iter().any(|(_, markup)| markup.has_inlays())
    }

    pub(crate) fn has_popups(&self) -> bool {
        self.document.has_popups() || self.extras.iter().any(|(_, markup)| markup.has_popups())
    }

    pub fn all_inlays_in(&self, range: Range<u32>) -> Vec<InlayInterval<'a>> {
        if !self.has_inlays() || range.start >= range.end {
            return Vec::new();
        }
        let mut inlays = Vec::new();
        self.document
            .collect_inlays(0, &range, MarkupLayer::Syntax, &mut inlays);
        for (id, markup) in self.extras {
            markup.collect_inlays(0, &range, MarkupLayer::Markup(*id), &mut inlays);
        }
        inlays.sort_by_key(|interval| interval.range.start);
        inlays
    }

    pub(crate) fn styled_range_at(
        &self,
        byte: u32,
        matches: impl Fn(StyleId) -> bool,
    ) -> Option<Range<u32>> {
        let mut widest: Option<Range<u32>> = None;
        for decoration in self.query(byte..byte.saturating_add(1), Order::Ascending) {
            if let Decoration::Styled(id) = &decoration.value {
                if matches(*id) && decoration.range.start <= byte && decoration.range.end > byte {
                    widest = Some(match widest.take() {
                        Some(range) => {
                            range.start.min(decoration.range.start)
                                ..range.end.max(decoration.range.end)
                        }
                        None => decoration.range.clone(),
                    });
                }
            }
        }
        widest
    }

    pub(crate) fn inlay_metrics_in(&self, range: Range<u32>, width: f32) -> InlayMetrics {
        if !self.has_inlays() || range.start >= range.end {
            return InlayMetrics::default();
        }
        Self::metrics_from(&self.all_inlays_in(range.clone()), &range, width)
    }

    pub(crate) fn metrics_from(
        hits: &[InlayInterval<'_>],
        range: &Range<u32>,
        width: f32,
    ) -> InlayMetrics {
        let range = range.clone();
        let constraints = Constraints {
            min: Size::default(),
            max: Size::new(width.max(1.0), f32::MAX),
        };
        let mut metrics = InlayMetrics::default();
        for interval in hits {
            if !intersects(&interval.range, &range) {
                continue;
            }
            if !inlay_anchors_line(interval.inlay.mode, &interval.range, &range) {
                if interval.inlay.mode == InlayMode::Instead(InsteadKind::FullLine) {
                    metrics.instead_covered = true;
                }
                continue;
            }
            let size = interval
                .inlay
                .size(instead_constraints(interval.inlay.mode, constraints));
            match interval.inlay.mode {
                InlayMode::Left | InlayMode::Right | InlayMode::Instead(InsteadKind::Inline) => {
                    metrics.inline_height = metrics.inline_height.max(size.height.max(0.0));
                }
                InlayMode::Under => metrics.under_height += size.height.max(0.0),
                InlayMode::Above => metrics.above_height += size.height.max(0.0),
                InlayMode::Instead(InsteadKind::FullLine) => {
                    metrics.instead_height = metrics.instead_height.max(size.height.max(0.0));
                }

                InlayMode::Popup(_) => {}
            }
        }
        metrics
    }

    pub(crate) fn inline_placeholders_in(
        &self,
        range: Range<u32>,
        width: f32,
    ) -> Vec<InlayPlaceholder> {
        if !self.has_inlays() || range.start >= range.end {
            return Vec::new();
        }
        let constraints = Constraints {
            min: Size::default(),
            max: Size::new(width.max(1.0), f32::MAX),
        };
        let mut placeholders = Vec::new();
        for interval in self.all_inlays_in(range.clone()) {
            if !matches!(
                interval.inlay.mode,
                InlayMode::Left | InlayMode::Right | InlayMode::Instead(InsteadKind::Inline)
            ) {
                continue;
            }
            let byte = inlay_anchor_byte(interval.inlay.mode, &interval.range);
            if byte < range.start || byte > range.end {
                continue;
            }
            let size = interval.inlay.size(constraints);
            placeholders.push(InlayPlaceholder {
                byte,
                width: size.width.max(0.0),
                height: size.height.max(0.0),
                align: match interval.inlay.mode {
                    InlayMode::Instead(InsteadKind::Inline) => InlayAlignment::Top,
                    _ => InlayAlignment::Middle,
                },
            });
        }
        placeholders.sort_by_key(|placeholder| placeholder.byte);
        placeholders
    }
}

impl Markup {
    pub fn block_marks_in(&self, range: Range<u32>) -> BlockStyle {
        let mut marks = BlockStyle::default();
        for decoration in self.query(range.clone(), Order::Ascending) {
            if intersects(&decoration.range, &range) {
                match decoration.value {
                    Decoration::Styled(id) => marks.add(*id),
                    Decoration::Alignment(alignment) => marks.alignment = Some(*alignment),
                    _ => {}
                }
            }
        }
        marks
    }

    pub fn marks_inline_hidden_in(
        &self,
        range: Range<u32>,
        inline: &mut Vec<TextDecorationInterval>,
        hidden: &mut Vec<Range<u32>>,
    ) -> BlockStyle {
        OverlaidMarkup::plain(self).marks_inline_hidden_in(range, inline, hidden)
    }

    pub fn all_inlays_in(&self, range: Range<u32>) -> Vec<InlayInterval<'_>> {
        let mut inlays = Vec::new();
        self.collect_inlays(0, &range, MarkupLayer::Syntax, &mut inlays);
        inlays.sort_by_key(|interval| interval.range.start);
        inlays
    }

    pub(crate) fn collect_inlays<'a>(
        &'a self,
        base: u32,
        range: &Range<u32>,
        layer: MarkupLayer,
        out: &mut Vec<InlayInterval<'a>>,
    ) {
        if range.end <= base || !self.has_inlays() {
            return;
        }
        let local = range.start.saturating_sub(base)..range.end - base;
        {
            for mut interval in self.inlays_in(local.clone(), layer) {
                interval.range = interval.range.start.saturating_add(base)
                    ..interval.range.end.saturating_add(base);
                out.push(interval);
            }
        }
        if self.syntaxes.is_empty() {
            return;
        }
        for marker in self.shape.query(local, Order::Ascending) {
            if let Decoration::Syntax(id) = marker.value {
                if let Some(syntax) = self.syntaxes.get(id) {
                    syntax.markup.collect_inlays(
                        base.saturating_add(marker.range.start),
                        range,
                        MarkupLayer::Syntax,
                        out,
                    );
                }
            }
        }
    }

    pub(crate) fn inlays_in(
        &self,
        range: Range<u32>,
        layer: MarkupLayer,
    ) -> impl Iterator<Item = InlayInterval<'_>> {
        let has_inlays = self.has_inlays;
        self.shape
            .query(range.clone(), Order::Ascending)
            .filter_map(move |interval| {
                if !has_inlays || !intersects(&interval.range, &range) {
                    return None;
                }

                match interval.value {
                    Decoration::Inlay(inlay) => Some(InlayInterval {
                        key: InlayKey {
                            layer,
                            key: *interval.key,
                        },
                        range: interval.range,
                        inlay,
                    }),
                    _ => None,
                }
            })
    }

    pub(crate) fn push_inlay(&mut self, range: Range<u32>, inlay: Inlay) -> IntervalId {
        let key = self.mint();
        self.replace_inlay(key, range, inlay);
        key
    }

    pub fn push_styled(&mut self, range: Range<u32>, id: StyleId) {
        let _ = self.push_styled_keyed(range, id);
    }

    pub(crate) fn push_styled_keyed(&mut self, range: Range<u32>, id: StyleId) -> IntervalId {
        let key = self.mint();
        self.styles.insert([intervals::Interval {
            range,
            greedy_left: false,
            greedy_right: false,
            key,
            value: Decoration::Styled(id),
        }]);
        key
    }

    pub fn set_unhide(&mut self, range: Range<u32>) -> Option<Range<u32>> {
        let previous = self.take_unhide();
        let key = self.mint();
        self.shape.insert([intervals::Interval {
            range,
            greedy_left: true,
            greedy_right: true,
            key,
            value: Decoration::Unhide,
        }]);
        previous
    }

    pub fn take_unhide(&mut self) -> Option<Range<u32>> {
        let mut stale = None;
        for interval in self.shape.query(0..u32::MAX, Order::Ascending) {
            if matches!(interval.value, Decoration::Unhide) {
                stale = Some((*interval.key, interval.range.clone()));
                break;
            }
        }
        stale.map(|(key, range)| {
            self.shape.remove([key].iter());
            range
        })
    }

    pub fn unhide_range(&self) -> Option<Range<u32>> {
        self.query(0..u32::MAX, Order::Ascending)
            .find(|decoration| matches!(decoration.value, Decoration::Unhide))
            .map(|decoration| decoration.range.clone())
    }

    pub fn push_styled_covering(&mut self, range: Range<u32>, id: StyleId) {
        let key = self.mint();
        self.styles.insert([intervals::Interval {
            range,
            greedy_left: true,
            greedy_right: true,
            key,
            value: Decoration::Styled(id),
        }]);
    }

    pub fn add_syntax(&mut self, range: Range<u32>, syntax: Syntax) -> SyntaxId {
        let key = self.mint();
        let state = SyntaxId(key.0 as u64);
        self.has_inlays = self.has_inlays || syntax.markup.has_inlays;
        self.has_popups = self.has_popups || syntax.markup.has_popups;
        self.syntaxes.insert_mut(state, syntax);
        self.shape.insert([intervals::Interval {
            range,
            greedy_left: false,
            greedy_right: false,
            key,
            value: Decoration::Syntax(state),
        }]);
        state
    }

    pub fn syntax(&self, key: SyntaxId) -> Option<&Syntax> {
        self.syntaxes.get(&key)
    }

    pub(crate) fn child_syntax_at(&self, byte: u32, base: u32) -> Option<(&Syntax, Range<u32>)> {
        let local = byte.saturating_sub(base);
        let probe = local.saturating_sub(1)..local.saturating_add(1);
        for marker in self.shape.query(probe, Order::Ascending) {
            let Decoration::Syntax(id) = marker.value else {
                continue;
            };
            if marker.range.start <= local && local <= marker.range.end {
                if let Some(syntax) = self.syntaxes.get(id) {
                    let range = base.saturating_add(marker.range.start)
                        ..base.saturating_add(marker.range.end);
                    return Some((syntax, range));
                }
            }
        }
        None
    }

    pub fn set_syntax(&mut self, key: SyntaxId, syntax: Syntax) -> bool {
        if !self.syntaxes.contains_key(&key) {
            return false;
        }

        self.has_inlays = self.has_inlays || syntax.markup.has_inlays;
        self.has_popups = self.has_popups || syntax.markup.has_popups;
        self.syntaxes.insert_mut(key, syntax);
        true
    }

    pub fn reconcile_syntaxes(
        &mut self,
        old: &Markup,
        sites: &[crate::reparse::SyntaxSite],
        text: &text::Text,
    ) -> Vec<FreshSyntax> {
        let _ = text;
        self.reconcile_syntaxes_in(old, &[0..u32::MAX], sites)
    }

    pub fn reconcile_syntaxes_in(
        &mut self,
        old: &Markup,
        invalidated: &[Range<u32>],
        sites: &[crate::reparse::SyntaxSite],
    ) -> Vec<FreshSyntax> {
        let mut dirty = Vec::new();
        let mut inherited: std::collections::HashSet<u64> = std::collections::HashSet::new();

        self.next_key = self.next_key.max(old.next_key);
        let relevant = sites.iter().filter(|site| {
            invalidated
                .iter()
                .any(|range| intersects(&site.range, range))
        });
        let mut markers = Vec::new();
        for site in relevant {
            let inherited_payload = old
                .shape
                .query(site.range.clone(), Order::Ascending)
                .find_map(|interval| match interval.value {
                    Decoration::Syntax(key) if interval.range == site.range => {
                        old.syntaxes.get(key).map(|payload| (*key, payload))
                    }
                    _ => None,
                })
                .filter(|(_, payload)| payload.language == site.language);
            let state = match inherited_payload {
                Some((state, payload)) => {
                    inherited.insert(state.0);
                    self.has_inlays = self.has_inlays || payload.markup.has_inlays;
                    self.has_popups = self.has_popups || payload.markup.has_popups;
                    self.syntaxes.insert_mut(state, payload.clone());
                    state
                }
                None => {
                    let state = SyntaxId(self.mint().0 as u64);
                    self.syntaxes.insert_mut(
                        state,
                        Syntax::new(site.language.clone(), None, Markup::new()),
                    );
                    dirty.push(FreshSyntax {
                        key: state,
                        language: site.language.clone(),
                        range: site.range.clone(),
                    });
                    state
                }
            };
            let marker = self.mint();
            markers.push(intervals::Interval {
                range: site.range.clone(),
                greedy_left: false,
                greedy_right: false,
                key: marker,
                value: Decoration::Syntax(state),
            });
        }

        for range in invalidated {
            for interval in old.shape.query(range.clone(), Order::Ascending) {
                if let Decoration::Syntax(key) = interval.value {
                    if intersects(&interval.range, range) && !inherited.contains(&key.0) {
                        self.syntaxes.remove_mut(key);
                    }
                }
            }
        }
        self.shape.insert(markers);
        dirty
    }

    pub fn inlay_at_key(&self, markup: MarkupId, key: InlayKey) -> Option<(Range<u32>, &Inlay)> {
        match key.layer {
            MarkupLayer::Markup(owner) if owner == markup => {}
            _ => return None,
        }
        let interval = self.shape.find_by_id(&key.key)?;
        let Decoration::Inlay(inlay) = interval.value else {
            return None;
        };
        Some((interval.range.clone(), inlay))
    }

    pub(crate) fn inlay_interval(&self, key: IntervalId) -> Option<(Range<u32>, InlayMode)> {
        let interval = self.shape.find_by_id(&key)?;
        let Decoration::Inlay(inlay) = interval.value else {
            return None;
        };
        Some((interval.range.clone(), inlay.mode))
    }

    pub(crate) fn remove_keys(&mut self, keys: impl IntoIterator<Item = IntervalId>) {
        let keys: Vec<IntervalId> = keys.into_iter().collect();
        self.shape.remove(keys.iter());
        self.styles.remove(keys.iter());
    }

    pub(crate) fn replace_inlay(&mut self, key: IntervalId, range: Range<u32>, inlay: Inlay) {
        if range.start >= range.end {
            return;
        }

        self.has_inlays = true;
        self.has_popups = self.has_popups || matches!(inlay.mode, InlayMode::Popup(_));
        self.shape.insert([Interval {
            range,
            greedy_left: false,
            greedy_right: false,
            key,
            value: Decoration::Inlay(inlay),
        }]);
    }

    pub fn replace_inlay_at(&mut self, key: InlayKey, range: Range<u32>, inlay: Inlay) {
        self.replace_inlay(key.key, range, inlay);
    }

    pub fn destroy_inlays_in(&mut self, changed: &[Range<u32>], store: &mut Store) {
        if !self.has_inlays {
            return;
        }
        let mut seen: std::collections::HashSet<IntervalId> = std::collections::HashSet::new();
        let mut victims: Vec<(IntervalId, Inlay)> = Vec::new();
        for range in changed {
            for interval in self.shape.query(range.clone(), Order::Ascending) {
                if !intersects(&interval.range, range) {
                    continue;
                }
                if let Decoration::Inlay(inlay) = interval.value {
                    if seen.insert(*interval.key) {
                        victims.push((*interval.key, inlay.clone()));
                    }
                }
            }
        }
        if victims.is_empty() {
            return;
        }
        let mut batch = imba::effect::Batch::new();
        let mut fx = batch.effects();
        for (_, inlay) in &victims {
            let mut view = inlay.view.clone_view();
            view.destroy_view(store, &mut fx);
        }
    }

    pub(crate) fn inlay_at(&self, key: IntervalId) -> Option<&Inlay> {
        let interval = self.shape.find_by_id(&key)?;
        let Decoration::Inlay(inlay) = interval.value else {
            return None;
        };
        Some(inlay)
    }

    pub(crate) fn inlay_focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
        key: IntervalId,
    ) -> Option<imba::focus::FocusData<'w, InlayCommand>> {
        let interval = self.shape.find_by_id(&key)?;
        let Decoration::Inlay(inlay) = interval.value else {
            return None;
        };
        Some(inlay.view.focus_view(store, ui))
    }

    pub(crate) fn inlay_passive(&self, key: IntervalId, command: &InlayCommand) -> bool {
        let Some(interval) = self.shape.find_by_id(&key) else {
            return false;
        };
        let Decoration::Inlay(inlay) = interval.value else {
            return false;
        };
        inlay.view.passive_view(command)
    }

    pub(crate) fn perform_inlay(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        key: IntervalId,
        command: InlayCommand,
        fx: &mut imba::effect::Effects<'_, InlayCommand>,
    ) -> (Option<(Range<u32>, InlayMode)>, Option<Operation>) {
        let Some(interval) = self.shape.find_by_id(&key) else {
            return (None, None);
        };
        let Decoration::Inlay(inlay) = interval.value else {
            return (None, None);
        };
        let (range, mode, inlay) = (interval.range.clone(), inlay.mode, inlay.clone());
        let (inlay, edit) = inlay.perform(store, ui, &range, command, fx);
        self.replace_inlay(key, range.clone(), inlay);
        (Some((range, mode)), edit)
    }

    pub(crate) fn edit(&mut self, operation: &Operation, view: &mut text::TextView, base: u32) {
        if !self.syntaxes.is_empty() {
            let Some(affected) = affected_span(operation) else {
                self.shape.edit(interval_steps(operation));
                self.styles.edit(interval_steps(operation));
                return;
            };
            let mut touched: Vec<(SyntaxId, Option<(Operation, u32)>)> = Vec::new();
            for interval in self.shape.query(affected, Order::Ascending) {
                if let Decoration::Syntax(key) = interval.value {
                    match rebase_into(
                        operation,
                        &interval.range,
                        interval.greedy_left,
                        interval.greedy_right,
                    ) {
                        Rebase::Outside => {}
                        Rebase::Inside(inner) => {
                            touched.push((*key, Some((inner, interval.range.start))));
                        }
                        Rebase::Boundary => touched.push((*key, None)),
                    }
                }
            }
            for (key, inner) in touched {
                match inner {
                    Some((inner, start)) => {
                        if let Some(entry) = self.syntaxes.get(&key) {
                            let mut entry = entry.clone();
                            let entry_base = base + start;
                            entry.markup.edit(&inner, view, entry_base);

                            entry.folds.edit(interval_steps(&inner));
                            entry.outline.edit(interval_steps(&inner));
                            if let Some(tree) = &mut entry.tree {
                                tree.edit(&inner, view, entry_base);
                            }
                            self.syntaxes.insert_mut(key, entry);
                        }
                    }
                    None => {
                        self.syntaxes.remove_mut(&key);
                    }
                }
            }
        }
        self.shape.edit(interval_steps(operation));
        self.styles.edit(interval_steps(operation));
    }

    pub(crate) fn carry_live_views_in(&mut self, live: &Markup, ranges: &[Range<u32>]) {
        if !self.has_inlays || !live.has_inlays {
            return;
        }
        let carried: Vec<Interval<IntervalId, Decoration>> = ranges
            .iter()
            .flat_map(|range| self.shape.query(range.clone(), Order::Ascending))
            .filter_map(|interval| {
                let Decoration::Inlay(incoming) = interval.value else {
                    return None;
                };

                if !incoming.view.carry_live() {
                    return None;
                }
                let live_interval = live.shape.find_by_id(interval.key)?;
                let Decoration::Inlay(live_inlay) = live_interval.value else {
                    return None;
                };
                if live_inlay.view.as_any().type_id() != incoming.view.as_any().type_id() {
                    return None;
                }
                Some(Interval {
                    range: interval.range.clone(),
                    greedy_left: false,
                    greedy_right: false,
                    key: *interval.key,
                    value: Decoration::Inlay(live_inlay.clone()),
                })
            })
            .collect();
        self.shape.insert(carried);
    }

    pub fn splice(
        &mut self,
        invalidated: &[Range<u32>],
        replacement: MarkupBuilder,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) {
        let mut stale = Vec::new();
        let mut stale_inlays: Vec<(IntervalId, Range<u32>, Inlay)> = Vec::new();
        for range in invalidated {
            for interval in self.merged_query(range.clone(), Order::Ascending) {
                if intersects(&interval.range, range) {
                    stale.push(*interval.key);
                    if let Decoration::Inlay(inlay) = interval.value {
                        stale_inlays.push((*interval.key, interval.range.clone(), inlay.clone()));
                    }
                }
            }
        }
        self.shape.remove(stale.iter());
        self.styles.remove(stale.iter());

        let fresh: Vec<Interval<IntervalId, Decoration>> = replacement
            .intervals
            .into_iter()
            .map(|mut interval| {
                if let Decoration::Inlay(inlay) = &mut interval.value {
                    self.has_inlays = true;
                    self.has_popups = self.has_popups || matches!(inlay.mode, InlayMode::Popup(_));

                    let previous = stale_inlays.iter().find(|(_, range, prior)| {
                        intersects(range, &interval.range)
                            && prior.view.as_any().type_id() == inlay.view.as_any().type_id()
                    });
                    if let Some((key, _, prior)) = previous {
                        let mut view = inlay.view.clone_view();
                        view.adopt_from(prior.view.as_any(), fonts, theme);
                        inlay.view = Arc::from(view);
                        interval.key = *key;
                        return interval;
                    }
                }
                interval.key = IntervalId(self.next_key);
                self.next_key = self.next_key.wrapping_add(1);
                interval
            })
            .collect();
        self.insert_split(fresh);
    }
}

impl InlayMetrics {
    pub(crate) fn has_instead(self) -> bool {
        self.instead_height > 0.0 || self.instead_covered
    }

    pub(crate) fn content_height(self, text_height: f32) -> f32 {
        let text_height = if self.has_instead() { 0.0 } else { text_height };
        text_height.max(self.inline_height).max(self.instead_height)
    }

    pub(crate) fn total_height(self, text_height: f32) -> f32 {
        self.above_height + self.content_height(text_height) + self.under_height
    }

    pub(crate) fn content_height_from_total(self, total_height: f32) -> f32 {
        (total_height - self.above_height - self.under_height).max(0.0)
    }
}

impl TextDecorationInterval {
    pub fn new(range: Range<usize>, id: StyleId) -> Self {
        Self {
            range: range.start.min(u32::MAX as usize) as u32
                ..range.end.min(u32::MAX as usize) as u32,
            id,
        }
    }
}

impl MarkupBuilder {
    fn mint(&mut self) -> IntervalId {
        let key = IntervalId(self.next_key);
        self.next_key = self.next_key.wrapping_add(1);
        key
    }

    pub fn push_styled(&mut self, range: Range<u32>, id: StyleId) {
        self.push(range, Decoration::Styled(id));
    }

    fn new() -> Self {
        Self {
            next_key: 0,
            intervals: Vec::new(),
            folds: Vec::new(),
            outline: Vec::new(),
        }
    }

    pub fn push_block_styles(&mut self, range: Range<u32>, ids: impl IntoIterator<Item = StyleId>) {
        for id in ids {
            self.push(range.clone(), Decoration::Styled(id));
        }
    }

    pub fn push_inline(&mut self, range: Range<u32>, id: StyleId) {
        self.push(range, Decoration::Styled(id));
    }

    pub fn push_alignment(&mut self, range: Range<u32>, alignment: crate::theme::TextAlignment) {
        self.push(range, Decoration::Alignment(alignment));
    }

    pub fn push_inlay(&mut self, range: Range<u32>, inlay: Inlay) {
        if range.start >= range.end {
            return;
        }
        let key = self.mint();
        self.intervals.push(Interval {
            range,
            greedy_left: false,
            greedy_right: false,
            key,
            value: Decoration::Inlay(inlay),
        });
    }

    pub fn push_hidden(&mut self, range: Range<u32>) {
        self.push(range, Decoration::Hidden);
    }

    pub fn push_foldable(&mut self, range: Range<u32>) {
        if range.start < range.end {
            self.folds.push(range);
        }
    }

    pub fn push_outline(&mut self, range: Range<u32>, item: OutlineItem) {
        if range.start < range.end {
            self.outline.push((range, item));
        }
    }

    pub(crate) fn take_channels(&mut self) -> (Vec<Range<u32>>, Vec<(Range<u32>, OutlineItem)>) {
        (
            std::mem::take(&mut self.folds),
            std::mem::take(&mut self.outline),
        )
    }

    fn push(&mut self, range: Range<u32>, value: Decoration) {
        if range.start >= range.end {
            return;
        }

        let key = self.mint();
        self.intervals.push(Interval {
            range,
            greedy_left: false,
            greedy_right: false,
            key,
            value,
        });
    }

    pub fn finish(self) -> Markup {
        let has_inlays = self
            .intervals
            .iter()
            .any(|interval| matches!(interval.value, Decoration::Inlay(_)));
        let has_popups = self.intervals.iter().any(|interval| {
            matches!(&interval.value, Decoration::Inlay(inlay) if matches!(inlay.mode, InlayMode::Popup(_)))
        });
        let mut markup = Markup {
            styles: Intervals::new(),
            shape: Intervals::new(),
            next_key: self.next_key,
            scope: MarkupScope::View,
            has_inlays,
            has_popups,
            syntaxes: rpds::HashTrieMapSync::new_sync(),
        };
        markup.insert_split(self.intervals);
        markup
    }
}

pub struct InlayInterval<'a> {
    pub key: InlayKey,
    pub range: Range<u32>,
    pub inlay: &'a Inlay,
}

pub fn inlay_anchors_line(mode: InlayMode, interval: &Range<u32>, line: &Range<u32>) -> bool {
    if !intersects(interval, line) {
        return false;
    }

    match mode {
        InlayMode::Left | InlayMode::Above | InlayMode::Instead(_) | InlayMode::Popup(_) => {
            line.start <= interval.start && interval.start < line.end
        }
        InlayMode::Right | InlayMode::Under => {
            line.start < interval.end && interval.end <= line.end
        }
    }
}

pub(crate) fn inlay_anchor_byte(mode: InlayMode, interval: &Range<u32>) -> u32 {
    match mode {
        InlayMode::Left | InlayMode::Above | InlayMode::Instead(_) | InlayMode::Popup(_) => {
            interval.start
        }
        InlayMode::Right | InlayMode::Under => interval.end,
    }
}

pub(crate) fn instead_constraints(mode: InlayMode, constraints: Constraints) -> Constraints {
    match mode {
        InlayMode::Instead(InsteadKind::FullLine) => Constraints {
            min: Size::new(constraints.max.width, 0.0),
            max: constraints.max,
        },
        _ => constraints,
    }
}

pub fn inlay_repair_span(mode: InlayMode, interval: &Range<u32>) -> Range<u32> {
    match mode {
        InlayMode::Popup(_) => interval.start..interval.start,
        InlayMode::Left | InlayMode::Above => interval.start..interval.start.saturating_add(1),

        InlayMode::Instead(_) => interval.clone(),
        InlayMode::Right | InlayMode::Under => {
            let anchor = interval.end.saturating_sub(1);
            anchor..anchor.saturating_add(1)
        }
    }
}

impl Inlay {
    pub fn new<V>(mode: InlayMode, view: V) -> Self
    where
        V: View + Clone + Send + Sync + 'static,
        V::Command: Send + Sync + 'static,
    {
        Self {
            mode,
            view: Arc::new(PlainInlayView(view)),
            overlay: None,
        }
    }

    pub fn editing<V>(mode: InlayMode, view: V) -> Self
    where
        V: View + InlayEditing + Clone + Send + Sync + 'static,
        V::Command: Send + Sync + 'static,
    {
        Self {
            mode,
            view: Arc::new(EditingInlayView {
                view,
                carry_live: true,
            }),
            overlay: None,
        }
    }

    /// Render through the given overlay host instead of inline,
    /// spanning it edge to edge; the inlay still reserves its space
    /// in the text flow.
    pub fn over(mut self, host: imba::overlay::OverlayHost) -> Self {
        self.overlay = Some((host, InlayProjection::Span));
        self
    }

    /// Like `over`, but anchored at the minting editor's left edge —
    /// the projected widget lines up with the editor's own columns.
    pub fn over_aligned(mut self, host: imba::overlay::OverlayHost) -> Self {
        self.overlay = Some((host, InlayProjection::Aligned));
        self
    }

    pub fn view_as<V: 'static>(&self) -> Option<&V> {
        self.view.as_any().downcast_ref::<V>()
    }

    pub fn mode(&self) -> InlayMode {
        self.mode
    }

    pub(crate) fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> imba::ThunkBox<'a, InlayCommand> {
        self.view.layout_view(arena, store, ui, constraints)
    }

    pub(crate) fn size(&self, constraints: Constraints) -> Size {
        let arena = Arena::default();
        let store = Store::new();

        let ui = UiCtx::cold();
        let widget = self.layout(&arena, &store, &ui, constraints);
        widget.size()
    }

    pub(crate) fn perform(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        range: &Range<u32>,
        command: InlayCommand,
        fx: &mut imba::effect::Effects<'_, InlayCommand>,
    ) -> (Self, Option<Operation>) {
        let mut view = self.view.clone_view();
        let edit = view.perform_view(store, ui, range, command, fx);
        (
            Self {
                mode: self.mode,
                view: Arc::from(view),
                overlay: self.overlay,
            },
            edit,
        )
    }
}

fn intersects(left: &Range<u32>, right: &Range<u32>) -> bool {
    left.start < right.end && right.start < left.end
}

enum Rebase {
    Outside,

    Inside(Operation),

    Boundary,
}

fn rebase_into(
    operation: &Operation,
    block: &Range<u32>,
    greedy_left: bool,
    greedy_right: bool,
) -> Rebase {
    use operation::Op;
    let mut position: u32 = 0;
    let mut inner: Vec<Op> = Vec::new();
    let mut written: u32 = 0;
    let mut touched = false;
    for op in operation.iter() {
        match &op {
            Op::Retain(len) => position = position.saturating_add(*len),
            Op::Insert(text) => {
                let after_start = match greedy_left {
                    true => position >= block.start,
                    false => position > block.start,
                };
                let before_end = match greedy_right {
                    true => position <= block.end,
                    false => position < block.end,
                };
                if after_start && before_end {
                    let at = position - block.start;
                    if at > written {
                        inner.push(Op::Retain(at - written));
                        written = at;
                    }
                    inner.push(Op::Insert(text.clone()));
                    touched = true;
                }
            }
            Op::Delete(text) => {
                let len = text.len().min(u32::MAX as usize) as u32;
                let end = position.saturating_add(len);
                if end <= block.start || position >= block.end {
                    position = end;
                    continue;
                }
                if position < block.start || end > block.end {
                    return Rebase::Boundary;
                }
                let at = position - block.start;
                if at > written {
                    inner.push(Op::Retain(at - written));
                    written = at;
                }
                inner.push(Op::Delete(text.clone()));
                written = written.saturating_add(len);
                touched = true;
                position = end;
            }
        }
    }
    match touched {
        false => Rebase::Outside,
        true => Rebase::Inside(Operation::from_ops(inner)),
    }
}

#[derive(Clone)]
pub struct FreshSyntax {
    pub key: SyntaxId,
    pub language: String,

    pub range: Range<u32>,
}

impl Markup {
    pub fn foldables_in(&self, span: Range<u32>) -> Vec<Range<u32>> {
        let mut out = Vec::new();
        self.collect_channel_folds(0, &span, &mut out);
        out.sort_by_key(|range| (range.start, range.end));
        out
    }

    pub(crate) fn collect_channel_folds_in(&self, span: &Range<u32>, out: &mut Vec<Range<u32>>) {
        self.collect_channel_folds(0, span, out);
    }

    pub fn has_outline(&self) -> bool {
        for marker in self.shape.query(0..u32::MAX, Order::Ascending) {
            let Decoration::Syntax(id) = marker.value else {
                continue;
            };
            let Some(syntax) = self.syntaxes.get(id) else {
                continue;
            };
            if !syntax.outline.is_empty() || syntax.markup.has_outline() {
                return true;
            }
        }
        false
    }

    pub fn outline_items(&self) -> Vec<(SyntaxId, IntervalId, Range<u32>, OutlineItem)> {
        let mut out = Vec::new();
        self.collect_outline(0, &mut out);
        out.sort_by_key(|(_, _, range, _)| (range.start, std::cmp::Reverse(range.end)));
        out
    }

    pub(crate) fn collect_outline_in(
        &self,
        base: u32,
        out: &mut Vec<(SyntaxId, IntervalId, Range<u32>, OutlineItem)>,
    ) {
        self.collect_outline(base, out);
    }

    fn collect_outline(
        &self,
        base: u32,
        out: &mut Vec<(SyntaxId, IntervalId, Range<u32>, OutlineItem)>,
    ) {
        for marker in self.shape.query(0..u32::MAX, Order::Ascending) {
            let Decoration::Syntax(id) = marker.value else {
                continue;
            };
            let Some(syntax) = self.syntaxes.get(id) else {
                continue;
            };
            let syntax_base = base + marker.range.start;
            for entry in syntax.outline.query(0..u32::MAX, Order::Ascending) {
                out.push((
                    *id,
                    *entry.key,
                    syntax_base + entry.range.start..syntax_base + entry.range.end,
                    entry.value.clone(),
                ));
            }
            syntax.markup.collect_outline(syntax_base, out);
        }
    }

    pub fn resolve_outline(&self, syntax: SyntaxId, key: IntervalId) -> Option<Range<u32>> {
        self.resolve_outline_from(0, syntax, key)
    }

    fn resolve_outline_from(
        &self,
        base: u32,
        syntax: SyntaxId,
        key: IntervalId,
    ) -> Option<Range<u32>> {
        for marker in self.shape.query(0..u32::MAX, Order::Ascending) {
            let Decoration::Syntax(id) = marker.value else {
                continue;
            };
            let Some(entry) = self.syntaxes.get(id) else {
                continue;
            };
            let syntax_base = base + marker.range.start;
            if *id == syntax {
                if let Some(found) = entry.outline.find_by_id(&key) {
                    return Some(syntax_base + found.range.start..syntax_base + found.range.end);
                }
            }
            if let Some(found) = entry.markup.resolve_outline_from(syntax_base, syntax, key) {
                return Some(found);
            }
        }
        None
    }

    pub(crate) fn outline_enclosing_from(&self, base: u32, offset: u32, out: &mut Vec<Range<u32>>) {
        let Some(local) = offset.checked_sub(base) else {
            return;
        };
        for marker in self
            .shape
            .query(local..local.saturating_add(1), Order::Ascending)
        {
            let Decoration::Syntax(id) = marker.value else {
                continue;
            };
            let Some(syntax) = self.syntaxes.get(id) else {
                continue;
            };
            let syntax_base = base + marker.range.start;
            if let Some(rel) = offset.checked_sub(syntax_base) {
                for entry in syntax
                    .outline
                    .query(rel..rel.saturating_add(1), Order::Ascending)
                {
                    out.push(syntax_base + entry.range.start..syntax_base + entry.range.end);
                }
            }
            syntax
                .markup
                .outline_enclosing_from(syntax_base, offset, out);
        }
    }

    fn collect_channel_folds(&self, base: u32, span: &Range<u32>, out: &mut Vec<Range<u32>>) {
        let local = span.start.saturating_sub(base)..span.end.saturating_sub(base).max(1);
        for marker in self.shape.query(local, Order::Ascending) {
            let Decoration::Syntax(id) = marker.value else {
                continue;
            };
            let Some(syntax) = self.syntaxes.get(id) else {
                continue;
            };
            let syntax_base = base + marker.range.start;
            let inner =
                span.start.saturating_sub(syntax_base)..span.end.saturating_sub(syntax_base).max(1);
            for entry in syntax.folds.query(inner, Order::Ascending) {
                let absolute = syntax_base + entry.range.start..syntax_base + entry.range.end;

                if span.start <= absolute.start && absolute.start < span.end {
                    out.push(absolute);
                }
            }
            syntax.markup.collect_channel_folds(syntax_base, span, out);
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn syntax_markers_in(&self, span: Range<u32>) -> Vec<Range<u32>> {
        self.syntax_in(span)
            .into_iter()
            .map(|(_, range)| range)
            .collect()
    }

    pub fn styled_ranges_in(&self, span: Range<u32>) -> Vec<Range<u32>> {
        self.styles
            .query(span, Order::Ascending)
            .filter_map(|interval| match interval.value {
                Decoration::Styled(_) => Some(interval.range.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn syntax_in(&self, span: Range<u32>) -> Vec<(SyntaxId, Range<u32>)> {
        self.shape
            .query(span, Order::Ascending)
            .filter_map(|interval| match interval.value {
                Decoration::Syntax(key) => Some((*key, interval.range.clone())),
                _ => None,
            })
            .collect()
    }
}

fn affected_span(operation: &Operation) -> Option<Range<u32>> {
    use operation::Op;
    let mut position: u32 = 0;
    let mut start: Option<u32> = None;
    let mut end: u32 = 0;
    for op in operation.iter() {
        match &op {
            Op::Retain(len) => position = position.saturating_add(*len),
            Op::Insert(_) => {
                start.get_or_insert(position);
                end = end.max(position);
            }
            Op::Delete(text) => {
                let len = text.len().min(u32::MAX as usize) as u32;
                start.get_or_insert(position);
                position = position.saturating_add(len);
                end = end.max(position);
            }
        }
    }
    start.map(|start| start..end.max(start).saturating_add(1))
}

#[cfg(test)]
mod tests;
