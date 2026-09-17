// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The LAYOUT stage (docs/UI.md, revision 3): the structure of a
//! subtree with the state already read — every store-derived fact
//! captured, no geometry yet. `View::display` answers one; sizing it
//! (`layout(constraints)`) answers the thunk. Placement arithmetic
//! lives HERE, as reusable values (`Column`, `Row`, … — stage 2),
//! instead of being hand-rolled inside every composite view.

use crate::arena::{self, Arena};
use crate::constraints::Constraints;
use crate::{Thunk, ThunkBox};

/// A structured, unsized subtree: state read, geometry pending.
/// Consumed by sizing (single-shot, like every frame artifact). The
/// erased return keeps one method surface for static and boxed
/// children alike — the measured default (docs/UI.md: the perf gate
/// decides whether the RPITIT form replaces it).
pub trait Layout<'a, Command> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command>
    where
        Self: Sized;
}

/// Type erasure — the same arena-box story as `ThunkBox`/`WidgetBox`:
/// a bump pointer, destructors riding the frame arena.
pub struct LayoutBox<'a, Command>(arena::ArenaBox<'a, dyn DynLayout<'a, Command> + 'a>);

impl<'a, Command: 'a> LayoutBox<'a, Command> {
    pub fn new<L: Layout<'a, Command> + 'a>(arena: &'a Arena, layout: L) -> Self {
        let slot = arena.boxed(Slot(Some(layout)));
        let raw: *mut Slot<L> = arena::ArenaBox::into_raw(slot);

        Self(unsafe { arena::ArenaBox::from_raw(raw as *mut (dyn DynLayout<'a, Command> + 'a)) })
    }
}

impl<'a, Command: 'a> Layout<'a, Command> for LayoutBox<'a, Command> {
    fn layout(mut self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        self.0.layout_dyn(arena, constraints)
    }
}

trait DynLayout<'a, Command> {
    fn layout_dyn(&mut self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command>;
}

struct Slot<L>(Option<L>);

impl<'a, Command: 'a, L: Layout<'a, Command>> DynLayout<'a, Command> for Slot<L> {
    fn layout_dyn(&mut self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        self.0
            .take()
            .expect("laid out twice")
            .layout(arena, constraints)
    }
}

/// The migration shim (docs/UI.md): lifts a sizing closure — an old
/// `View::layout` body, verbatim — into a `Layout`. Stage-2 views
/// return stock layouts instead; a `laid` at a call site marks
/// placement arithmetic not yet extracted.
pub fn laid<F>(sizing: F) -> Laid<F> {
    Laid(sizing)
}

pub struct Laid<F>(F);

impl<'a, Command: 'a, T, F> Layout<'a, Command> for Laid<F>
where
    T: Thunk<'a, Command> + 'a,
    F: FnOnce(&'a Arena, Constraints) -> T,
{
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        ThunkBox::new(arena, (self.0)(arena, constraints))
    }
}

// ---- primitives (docs/UI.md stage 2) --------------------------------
//
// Jetpack-Compose-shaped, deliberately: `Column`/`Row` with
// `Arrangement`-style gaps, cross-axis `CrossAlign`, per-child
// `weight`; `Pad`/`Align`/`SizedBox` as modifier structs behind
// `LayoutExt`. Every layout is a REIFIED struct — no closures to
// squint at — so a view's `display` reads as the structure it names.

use crate::container::container;
use crate::thunk_ext::ThunkExt;
use skia_safe::Size;

/// Treat imba's conventional f32::MAX-ish bounds as "unbounded".
fn bounded(extent: f32) -> Option<f32> {
    (extent < f32::MAX / 2.0).then_some(extent)
}

/// Cross-axis placement of a stack's children (Compose's
/// `horizontalAlignment` / `verticalAlignment`).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum CrossAlign {
    #[default]
    Start,
    Center,
    End,
}

impl CrossAlign {
    fn offset(self, room: f32, child: f32) -> f32 {
        match self {
            CrossAlign::Start => 0.0,
            CrossAlign::Center => ((room - child) * 0.5).max(0.0),
            CrossAlign::End => (room - child).max(0.0),
        }
    }
}

/// Per-child cross-axis behavior — Compose's `RowScope`/`ColumnScope`
/// modifiers: inherit the stack's `align_items`, override it
/// (`Modifier.align`), or ride the baseline group
/// (`Modifier.alignByBaseline`, rows only).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ChildCross {
    #[default]
    Inherit,
    Align(CrossAlign),
    Baseline,
}

struct FlexChild<'a, Command> {
    layout: LayoutBox<'a, Command>,
    weight: Option<f32>,
    cross: ChildCross,
}

/// The one FLEX algorithm, parameterized by axis (Column =
/// vertical). (Not to be confused with `imba::stack::Stack`, the
/// base+modal VIEW compositor.)
/// Unweighted children measure first, in order, against the space
/// still free on the main axis; weighted children then split the
/// leftover proportionally with TIGHT main-axis constraints —
/// weights need a bounded main axis to mean anything (an unbounded
/// stack gives them zero, like Compose forbids). The cross extent is
/// the widest child, clamped into the incoming constraints.
struct Flex<'a, Command> {
    arena: &'a Arena,
    children: Vec<FlexChild<'a, Command>>,
    gap: f32,
    cross: CrossAlign,
    horizontal: bool,
}

impl<'a, Command: 'a> Flex<'a, Command> {
    fn lay(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        let (main_max, cross_max) = match self.horizontal {
            true => (constraints.max.width, constraints.max.height),
            false => (constraints.max.height, constraints.max.width),
        };
        let (main_min, cross_min) = match self.horizontal {
            true => (constraints.min.width, constraints.min.height),
            false => (constraints.min.height, constraints.min.width),
        };
        let gaps = self.gap * self.children.len().saturating_sub(1) as f32;
        let total_weight: f32 = self.children.iter().filter_map(|child| child.weight).sum();

        let horizontal = self.horizontal;
        let child_constraints = |main: Option<f32>, tight: bool| -> Constraints {
            let main = main.unwrap_or(f32::MAX);
            let (width, height) = match horizontal {
                true => (main, cross_max),
                false => (cross_max, main),
            };
            let min = match tight {
                true => match horizontal {
                    true => Size::new(main, 0.0),
                    false => Size::new(0.0, main),
                },
                false => Size::default(),
            };
            Constraints {
                min,
                max: Size::new(width, height),
            }
        };

        let mut thunks: Vec<Option<ThunkBox<'a, Command>>> =
            self.children.iter().map(|_| None).collect();
        // Measure passes need ownership of the children's layouts;
        // both passes run over a drained vec keyed by index.
        let mut children = self.children;
        let mut order: Vec<(usize, Option<f32>)> = children
            .iter()
            .enumerate()
            .map(|(index, child)| (index, child.weight))
            .collect();
        // unweighted first (in order), then weighted (in order)
        order.sort_by_key(|(index, weight)| (weight.is_some(), *index));
        let mut used = 0.0f32;
        let mut main_sizes = vec![0.0f32; children.len()];
        let mut cross_sizes = vec![0.0f32; children.len()];
        let mut baselines: Vec<Option<f32>> = vec![None; children.len()];
        let leftover_at = |used: f32| bounded(main_max).map(|max| (max - gaps - used).max(0.0));
        let mut leftover_for_weights = 0.0f32;
        let mut weighted_started = false;
        for (index, weight) in order {
            let slot = std::mem::replace(&mut children[index].layout, LayoutBox::tombstone(arena));
            let thunk = match weight {
                None => slot.layout(arena, child_constraints(leftover_at(used), false)),
                Some(weight) => {
                    if !weighted_started {
                        leftover_for_weights = leftover_at(used).unwrap_or(0.0);
                        weighted_started = true;
                    }
                    let share = match total_weight > 0.0 {
                        true => leftover_for_weights * weight / total_weight,
                        false => 0.0,
                    };
                    slot.layout(arena, child_constraints(Some(share), true))
                }
            };
            let size = thunk.size();
            let (main, cross) = match self.horizontal {
                true => (size.width, size.height),
                false => (size.height, size.width),
            };
            if weight.is_none() {
                used += main;
            }
            main_sizes[index] = main;
            cross_sizes[index] = cross;
            baselines[index] = thunk.first_baseline();
            thunks[index] = Some(thunk);
        }

        // The baseline group (rows only): children aligned by their
        // first baseline share one line — the deepest baseline among
        // them — and the group's extent is that line plus the
        // deepest descent below it (Compose's alignByBaseline).
        let by_baseline = |index: usize| {
            self.horizontal
                && children[index].cross == ChildCross::Baseline
                && baselines[index].is_some()
        };
        let mut line = 0.0f32;
        let mut below = 0.0f32;
        for index in 0..children.len() {
            if by_baseline(index) {
                let baseline = baselines[index].expect("guarded");
                line = line.max(baseline);
                below = below.max(cross_sizes[index] - baseline);
            }
        }

        let content_main: f32 = main_sizes.iter().sum::<f32>() + gaps;
        let main_extent = match total_weight > 0.0 {
            true => bounded(main_max).unwrap_or(content_main).max(main_min),
            false => content_main.max(main_min).min(main_max),
        };
        let cross_extent = cross_sizes
            .iter()
            .enumerate()
            .map(|(index, cross)| match by_baseline(index) {
                true => line + below,
                false => *cross,
            })
            .fold(0.0f32, |widest, cross| widest.max(cross))
            .max(cross_min)
            .min(cross_max);

        let size = match self.horizontal {
            true => Size::new(main_extent, cross_extent),
            false => Size::new(cross_extent, main_extent),
        };
        let mut frame = container(self.arena, size);
        let mut at = 0.0f32;
        for (index, thunk) in thunks.into_iter().enumerate() {
            let Some(thunk) = thunk else { continue };
            let along = match children[index].cross {
                ChildCross::Baseline if by_baseline(index) => {
                    line - baselines[index].expect("guarded")
                }
                ChildCross::Align(cross) => cross.offset(cross_extent, cross_sizes[index]),
                _ => self.cross.offset(cross_extent, cross_sizes[index]),
            };
            let (x, y) = match self.horizontal {
                true => (at, along),
                false => (along, at),
            };
            at += main_sizes[index] + self.gap;
            frame.place_boxed(x, y, thunk);
        }
        ThunkBox::new(arena, frame)
    }
}

impl<'a, Command: 'a> LayoutBox<'a, Command> {
    /// A zero-size placeholder for slots being drained during a
    /// measure pass — never laid out.
    fn tombstone(arena: &'a Arena) -> Self {
        LayoutBox::new(arena, Fixed(crate::leaf::leaf::<Command>(0.0, 0.0)))
    }
}

/// Compose's `Column`: children stack top-to-bottom; `gap` is
/// `Arrangement.spacedBy`; `weight` children split the leftover
/// height; `align_items` is `horizontalAlignment`.
pub struct Column<'a, Command> {
    flex: Flex<'a, Command>,
}

impl<'a, Command: 'a> Column<'a, Command> {
    pub fn new(arena: &'a Arena) -> Self {
        Self {
            flex: Flex {
                arena,
                children: Vec::new(),
                gap: 0.0,
                cross: CrossAlign::Start,
                horizontal: false,
            },
        }
    }

    pub fn gap(mut self, gap: f32) -> Self {
        self.flex.gap = gap;
        self
    }

    pub fn align_items(mut self, cross: CrossAlign) -> Self {
        self.flex.cross = cross;
        self
    }

    pub fn child(mut self, child: impl Layout<'a, Command> + 'a) -> Self {
        self.flex.children.push(FlexChild {
            layout: LayoutBox::new(self.flex.arena, child),
            weight: None,
            cross: ChildCross::Inherit,
        });
        self
    }

    pub fn weighted(mut self, weight: f32, child: impl Layout<'a, Command> + 'a) -> Self {
        self.flex.children.push(FlexChild {
            layout: LayoutBox::new(self.flex.arena, child),
            weight: Some(weight.max(0.0)),
            cross: ChildCross::Inherit,
        });
        self
    }
}

impl<'a, Command: 'a> Layout<'a, Command> for Column<'a, Command> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        self.flex.lay(arena, constraints)
    }
}

/// Compose's `Row`: left-to-right; `align_items` is
/// `verticalAlignment`.
pub struct Row<'a, Command> {
    flex: Flex<'a, Command>,
}

impl<'a, Command: 'a> Row<'a, Command> {
    pub fn new(arena: &'a Arena) -> Self {
        Self {
            flex: Flex {
                arena,
                children: Vec::new(),
                gap: 0.0,
                cross: CrossAlign::Start,
                horizontal: true,
            },
        }
    }

    pub fn gap(mut self, gap: f32) -> Self {
        self.flex.gap = gap;
        self
    }

    pub fn align_items(mut self, cross: CrossAlign) -> Self {
        self.flex.cross = cross;
        self
    }

    pub fn child(mut self, child: impl Layout<'a, Command> + 'a) -> Self {
        self.flex.children.push(FlexChild {
            layout: LayoutBox::new(self.flex.arena, child),
            weight: None,
            cross: ChildCross::Inherit,
        });
        self
    }

    pub fn weighted(mut self, weight: f32, child: impl Layout<'a, Command> + 'a) -> Self {
        self.flex.children.push(FlexChild {
            layout: LayoutBox::new(self.flex.arena, child),
            weight: Some(weight.max(0.0)),
            cross: ChildCross::Inherit,
        });
        self
    }
}

impl<'a, Command: 'a> Row<'a, Command> {
    /// Compose's `Modifier.align` on one child: overrides
    /// `align_items` for it.
    pub fn child_aligned(
        mut self,
        cross: CrossAlign,
        child: impl Layout<'a, Command> + 'a,
    ) -> Self {
        self.flex.children.push(FlexChild {
            layout: LayoutBox::new(self.flex.arena, child),
            weight: None,
            cross: ChildCross::Align(cross),
        });
        self
    }

    /// Compose's `Modifier.alignByBaseline`: the child joins the
    /// row's baseline group (falls back to `align_items` when its
    /// thunk answers no line).
    pub fn child_by_baseline(mut self, child: impl Layout<'a, Command> + 'a) -> Self {
        self.flex.children.push(FlexChild {
            layout: LayoutBox::new(self.flex.arena, child),
            weight: None,
            cross: ChildCross::Baseline,
        });
        self
    }
}

impl<'a, Command: 'a> Layout<'a, Command> for Row<'a, Command> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        self.flex.lay(arena, constraints)
    }
}

/// Compose's `Spacer`/`fillMaxSize`: an empty box taking the whole
/// incoming bound — the blank panel, the flexible gap.
pub struct Fill<Command>(std::marker::PhantomData<fn() -> Command>);

impl<Command> Fill<Command> {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<'a, Command: 'a> Layout<'a, Command> for Fill<Command> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        // Compose's `fillMaxSize`: an UNBOUNDED axis is not filled —
        // it falls back to the minimum. Filling to f32::MAX would
        // poison any measured parent (a list row's height metric
        // saturates and overflows the sumtree sums).
        let width = bounded(constraints.max.width).unwrap_or(constraints.min.width);
        let height = bounded(constraints.max.height).unwrap_or(constraints.min.height);
        ThunkBox::new(arena, crate::leaf::leaf::<Command>(width, height))
    }
}

/// Lifts a THUNK into a layout that ignores the incoming constraints
/// — the adapter for content whose size is its own fact (a `leaf`, a
/// measured widget).
pub struct Fixed<T>(pub T);

pub fn fixed<T>(thunk: T) -> Fixed<T> {
    Fixed(thunk)
}

impl<'a, Command: 'a, T: Thunk<'a, Command> + 'a> Layout<'a, Command> for Fixed<T> {
    fn layout(self, arena: &'a Arena, _constraints: Constraints) -> ThunkBox<'a, Command> {
        ThunkBox::new(arena, self.0)
    }
}

#[derive(Clone, Copy, Default)]
pub struct Insets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Insets {
    pub fn all(value: f32) -> Self {
        Self {
            left: value,
            top: value,
            right: value,
            bottom: value,
        }
    }

    pub fn xy(x: f32, y: f32) -> Self {
        Self {
            left: x,
            top: y,
            right: x,
            bottom: y,
        }
    }
}

/// Compose's `Modifier.padding`: deflates the constraints, offsets
/// the child.
pub struct Pad<L> {
    inner: L,
    insets: Insets,
}

impl<'a, Command: 'a, L: Layout<'a, Command> + 'a> Layout<'a, Command> for Pad<L> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        let x = self.insets.left + self.insets.right;
        let y = self.insets.top + self.insets.bottom;
        let deflated = Constraints {
            min: Size::new(
                (constraints.min.width - x).max(0.0),
                (constraints.min.height - y).max(0.0),
            ),
            max: Size::new(
                (constraints.max.width - x).max(0.0),
                (constraints.max.height - y).max(0.0),
            ),
        };
        let child = self.inner.layout(arena, deflated);
        let size = child.size();
        let mut frame = container(arena, Size::new(size.width + x, size.height + y));
        frame.place_boxed(self.insets.left, self.insets.top, child);
        ThunkBox::new(arena, frame)
    }
}

/// Compose's 9-point `Alignment` for a child inside the incoming max
/// bounds (a one-child `Box(contentAlignment = …)`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    TopStart,
    TopCenter,
    TopEnd,
    CenterStart,
    Center,
    CenterEnd,
    BottomStart,
    BottomCenter,
    BottomEnd,
}

impl Alignment {
    fn place(self, room: Size, child: Size) -> (f32, f32) {
        let x = match self {
            Alignment::TopStart | Alignment::CenterStart | Alignment::BottomStart => 0.0,
            Alignment::TopCenter | Alignment::Center | Alignment::BottomCenter => {
                ((room.width - child.width) * 0.5).max(0.0)
            }
            Alignment::TopEnd | Alignment::CenterEnd | Alignment::BottomEnd => {
                (room.width - child.width).max(0.0)
            }
        };
        let y = match self {
            Alignment::TopStart | Alignment::TopCenter | Alignment::TopEnd => 0.0,
            Alignment::CenterStart | Alignment::Center | Alignment::CenterEnd => {
                ((room.height - child.height) * 0.5).max(0.0)
            }
            Alignment::BottomStart | Alignment::BottomCenter | Alignment::BottomEnd => {
                (room.height - child.height).max(0.0)
            }
        };
        (x, y)
    }
}

pub struct Align<L> {
    inner: L,
    alignment: Alignment,
}

impl<'a, Command: 'a, L: Layout<'a, Command> + 'a> Layout<'a, Command> for Align<L> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        let child = self.inner.layout(arena, constraints.loosen());
        let size = child.size();
        let room = Size::new(
            match bounded(constraints.max.width) {
                Some(width) => width,
                None => size.width.max(constraints.min.width),
            },
            match bounded(constraints.max.height) {
                Some(height) => height,
                None => size.height.max(constraints.min.height),
            },
        );
        let (x, y) = self.alignment.place(room, size);
        let mut frame = container(arena, room);
        frame.place_boxed(x, y, child);
        ThunkBox::new(arena, frame)
    }
}

/// Compose's `Modifier.size`/`width`/`height`: tightens the given
/// axes; `f32::NAN` leaves an axis as it came.
pub struct SizedBox<L> {
    inner: L,
    width: f32,
    height: f32,
}

impl<'a, Command: 'a, L: Layout<'a, Command> + 'a> Layout<'a, Command> for SizedBox<L> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        let tightened = Constraints {
            min: Size::new(
                if self.width.is_nan() {
                    constraints.min.width
                } else {
                    self.width
                },
                if self.height.is_nan() {
                    constraints.min.height
                } else {
                    self.height
                },
            ),
            max: Size::new(
                if self.width.is_nan() {
                    constraints.max.width
                } else {
                    self.width
                },
                if self.height.is_nan() {
                    constraints.max.height
                } else {
                    self.height
                },
            ),
        };
        self.inner.layout(arena, tightened)
    }
}

/// The command boundary, one stage above `ThunkExt::map`.
pub struct MapLayout<L, F, Child> {
    inner: L,
    wrap: F,
    _child: std::marker::PhantomData<fn() -> Child>,
}

impl<'a, Child: 'a, Parent: 'a, L, F> Layout<'a, Parent> for MapLayout<L, F, Child>
where
    L: Layout<'a, Child> + 'a,
    F: Fn(Child) -> Parent + Clone + 'a,
{
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Parent> {
        ThunkBox::new(arena, self.inner.layout(arena, constraints).map(self.wrap))
    }
}

/// The modifier surface (Compose's `Modifier`, curried onto the
/// layout value itself). Unconditioned on purpose: the modifiers
/// only WRAP — every bound lives on the wrapper's own `Layout`
/// impl, so `Text` (a layout for every command type) modifies
/// without inference ceremony.
/// The Command-free marker that admits a type to the modifier
/// surface — every layout struct declares it (one line), which keeps
/// `LayoutExt` off unrelated types' method namespaces while leaving
/// the modifiers free of command-type inference.
pub trait LayoutValue {}

impl<F> LayoutValue for Laid<F> {}
impl<'a, Command> LayoutValue for LayoutBox<'a, Command> {}
impl<'a, Command> LayoutValue for Column<'a, Command> {}
impl<'a, Command> LayoutValue for Row<'a, Command> {}
impl<L> LayoutValue for Pad<L> {}
impl<L> LayoutValue for Align<L> {}
impl<L> LayoutValue for SizedBox<L> {}
impl<L, F, Child> LayoutValue for MapLayout<L, F, Child> {}
impl<L, H> LayoutValue for OnEvent<L, H> {}
impl<T> LayoutValue for Fixed<T> {}
impl<Command> LayoutValue for Fill<Command> {}
impl<'a, Command, F> LayoutValue for Button<'a, Command, F> {}
impl LayoutValue for Text {}

pub trait LayoutExt: LayoutValue + Sized {
    fn pad(self, all: f32) -> Pad<Self> {
        self.pad_insets(Insets::all(all))
    }

    fn pad_xy(self, x: f32, y: f32) -> Pad<Self> {
        self.pad_insets(Insets::xy(x, y))
    }

    fn pad_insets(self, insets: Insets) -> Pad<Self> {
        Pad {
            inner: self,
            insets,
        }
    }

    fn align(self, alignment: Alignment) -> Align<Self> {
        Align {
            inner: self,
            alignment,
        }
    }

    fn width(self, width: f32) -> SizedBox<Self> {
        SizedBox {
            inner: self,
            width,
            height: f32::NAN,
        }
    }

    fn height(self, height: f32) -> SizedBox<Self> {
        SizedBox {
            inner: self,
            width: f32::NAN,
            height,
        }
    }

    fn sized(self, width: f32, height: f32) -> SizedBox<Self> {
        SizedBox {
            inner: self,
            width,
            height,
        }
    }

    /// `Modifier.clickable`: any layout becomes a press target; the
    /// lambda mints the command per press, like Compose's `onClick`.
    fn on_click<F>(self, mint: F) -> OnEvent<Self, OnClick<F>> {
        OnEvent {
            inner: self,
            handler: OnClick(mint),
        }
    }

    /// `Modifier.background`, painter-shaped.
    fn backdrop<F>(self, painter: F) -> Backdrop<Self, F> {
        Backdrop {
            inner: self,
            painter,
        }
    }

    /// Presses stop here instead of falling through.
    fn shield(self) -> OnEvent<Self, Shield> {
        OnEvent {
            inner: self,
            handler: Shield,
        }
    }

    fn on_event<H>(self, handler: H) -> OnEvent<Self, H> {
        OnEvent {
            inner: self,
            handler,
        }
    }

    fn map_layout<Child, F>(self, wrap: F) -> MapLayout<Self, F, Child> {
        MapLayout {
            inner: self,
            wrap,
            _child: std::marker::PhantomData,
        }
    }
}

impl<T: LayoutValue + Sized> LayoutExt for T {}

/// A thunk carrying its `FirstBaseline` line — the provider side of
/// baseline alignment. `Text` wraps itself in one; any custom thunk
/// with a known baseline can too.
pub struct WithBaseline<T> {
    pub thunk: T,
    pub baseline: f32,
}

impl<'a, Command: 'a, T: Thunk<'a, Command> + 'a> Thunk<'a, Command> for WithBaseline<T> {
    fn size(&self) -> Size {
        self.thunk.size()
    }

    fn first_baseline(&self) -> Option<f32> {
        Some(self.baseline)
    }

    fn realize(self, arena: &'a Arena, viewport: skia_safe::Rect) -> crate::WidgetBox<'a, Command> {
        self.thunk.realize(arena, viewport)
    }
}

/// Compose's `Text`, single-line: measures itself from the font's
/// metrics and paints its own glyphs — labels stop being hand-rolled
/// `draw_str` closures inside leaves. Wider text than the incoming
/// bound clips (the container's clip); position it with `.align()` /
/// `Pad` like any layout. (Wrapping and ellipsis are later verses —
/// paragraphs belong to the editor.)
pub struct Text {
    text: String,
    font: skia_safe::Font,
    color: skia_safe::Color,
    tracking: f32,
}

pub fn text(content: impl Into<String>, font: skia_safe::Font, color: skia_safe::Color) -> Text {
    Text {
        text: content.into(),
        font,
        color,
        tracking: 0.0,
    }
}

impl Text {
    /// Extra per-glyph advance — the caps-label look several chrome
    /// labels hand-roll today.
    pub fn tracking(mut self, tracking: f32) -> Self {
        self.tracking = tracking;
        self
    }
}

impl<'a, Command: 'a> Layout<'a, Command> for Text {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        let (_, metrics) = self.font.metrics();
        let ascent = -metrics.ascent;
        let height = (ascent + metrics.descent).ceil().max(1.0);
        let advance = match self.tracking == 0.0 {
            true => self.font.measure_str(&self.text, None).0,
            false => self
                .text
                .chars()
                .map(|ch| self.font.measure_str(ch.to_string(), None).0 + self.tracking)
                .sum(),
        };
        let width = advance
            .min(constraints.max.width)
            .max(constraints.min.width);
        let Text {
            text,
            font,
            color,
            tracking,
        } = self;
        let label = crate::leaf::leaf::<Command>(width, height).paint_instead(
            move |_arena, canvas, rect| {
                let mut paint = skia_safe::Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(color);
                let baseline = rect.top + ascent;
                if tracking == 0.0 {
                    canvas.draw_str(&text, (rect.left, baseline), &font, &paint);
                } else {
                    let mut x = rect.left;
                    for ch in text.chars() {
                        let glyph = ch.to_string();
                        canvas.draw_str(&glyph, (x, baseline), &font, &paint);
                        x += font.measure_str(&glyph, None).0 + tracking;
                    }
                }
            },
        );
        ThunkBox::new(
            arena,
            WithBaseline {
                thunk: label,
                baseline: ascent,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::leaf::leaf;

    fn sized(width: f32, height: f32) -> impl for<'a> Layout<'a, ()> + LayoutValue {
        SizedProbe { width, height }
    }

    struct SizedProbe {
        width: f32,
        height: f32,
    }

    impl LayoutValue for SizedProbe {}

    impl<'a> Layout<'a, ()> for SizedProbe {
        fn layout(self, arena: &'a Arena, _constraints: Constraints) -> ThunkBox<'a, ()> {
            ThunkBox::new(arena, leaf::<()>(self.width, self.height))
        }
    }

    fn arena() -> Arena {
        Arena::default()
    }

    #[test]
    fn a_column_stacks_gaps_and_reports_the_widest_child() {
        let arena = arena();
        let thunk = Column::new(&arena)
            .gap(4.0)
            .child(sized(30.0, 10.0))
            .child(sized(50.0, 20.0))
            .child(sized(20.0, 5.0))
            .layout(
                &arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(400.0, 400.0),
                },
            );
        let size = thunk.size();
        assert_eq!(
            (size.width, size.height),
            (50.0, 10.0 + 4.0 + 20.0 + 4.0 + 5.0)
        );
    }

    #[test]
    fn fill_leaves_an_unbounded_axis_at_the_minimum() {
        let arena = arena();
        let thunk = Fill::<()>::new().layout(
            &arena,
            Constraints {
                min: Size::default(),
                max: Size::new(100.0, f32::MAX),
            },
        );
        let size = thunk.size();
        assert_eq!((size.width, size.height), (100.0, 0.0));
    }

    #[test]
    fn a_weighted_fill_row_stays_finite_under_a_list_measure() {
        // A list measures rows under unbounded height; a header Row
        // with a weighted Fill must not balloon the row's extent.
        let arena = arena();
        let thunk = Row::new(&arena)
            .child(sized(30.0, 20.0))
            .weighted(1.0, Fill::new())
            .layout(
                &arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(200.0, f32::MAX),
                },
            );
        let size = thunk.size();
        assert_eq!((size.width, size.height), (200.0, 20.0));
    }

    #[test]
    fn weighted_children_split_the_leftover_and_fill_the_axis() {
        use std::cell::RefCell;
        use std::rc::Rc;

        struct Probe {
            heights: Rc<RefCell<Vec<f32>>>,
        }
        impl<'a> Layout<'a, ()> for Probe {
            fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, ()> {
                self.heights.borrow_mut().push(constraints.min.height);
                ThunkBox::new(arena, leaf::<()>(10.0, constraints.min.height))
            }
        }

        let arena = arena();
        let heights = Rc::new(RefCell::new(Vec::new()));
        let thunk = Column::new(&arena)
            .child(sized(10.0, 40.0))
            .weighted(
                1.0,
                Probe {
                    heights: Rc::clone(&heights),
                },
            )
            .weighted(
                3.0,
                Probe {
                    heights: Rc::clone(&heights),
                },
            )
            .layout(
                &arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(100.0, 240.0),
                },
            );
        // 200 leftover after the 40px child: weights 1:3 → 50 and 150.
        assert_eq!(*heights.borrow(), vec![50.0, 150.0]);
        assert_eq!(
            thunk.size().height,
            240.0,
            "a weighted column fills its axis"
        );
    }

    #[test]
    fn a_row_mirrors_the_axes() {
        let arena = arena();
        let thunk = Row::new(&arena)
            .gap(2.0)
            .child(sized(10.0, 30.0))
            .child(sized(20.0, 10.0))
            .layout(
                &arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(400.0, 400.0),
                },
            );
        let size = thunk.size();
        assert_eq!((size.width, size.height), (32.0, 30.0));
    }

    #[test]
    fn pad_inflates_the_child_and_align_centers_it() {
        let arena = arena();
        let padded = sized(10.0, 10.0).pad_xy(6.0, 2.0).layout(
            &arena,
            Constraints {
                min: Size::default(),
                max: Size::new(100.0, 100.0),
            },
        );
        assert_eq!((padded.size().width, padded.size().height), (22.0, 14.0));

        let aligned = sized(10.0, 10.0).align(Alignment::Center).layout(
            &arena,
            Constraints {
                min: Size::default(),
                max: Size::new(100.0, 50.0),
            },
        );
        assert_eq!((aligned.size().width, aligned.size().height), (100.0, 50.0));
    }

    #[test]
    fn sized_box_tightens_one_axis() {
        let arena = arena();
        let thunk = Column::new(&arena)
            .weighted(1.0, sized(10.0, 0.0))
            .height(80.0)
            .layout(
                &arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(100.0, f32::MAX),
                },
            );
        assert_eq!(thunk.size().height, 80.0);
    }
}

#[cfg(test)]
mod baseline_tests {
    use super::*;
    use crate::leaf::leaf;

    struct Lined {
        width: f32,
        height: f32,
        baseline: f32,
    }

    impl LayoutValue for Lined {}

    impl<'a> Layout<'a, ()> for Lined {
        fn layout(self, arena: &'a Arena, _constraints: Constraints) -> ThunkBox<'a, ()> {
            ThunkBox::new(
                arena,
                WithBaseline {
                    thunk: leaf::<()>(self.width, self.height),
                    baseline: self.baseline,
                },
            )
        }
    }

    fn bounds() -> Constraints {
        Constraints {
            min: Size::default(),
            max: Size::new(400.0, 400.0),
        }
    }

    #[test]
    fn a_baseline_group_shares_the_deepest_line() {
        let arena = Arena::default();
        // Lines at 15 and 10; descents 5 and 20. The shared line sits
        // at 15, the group extends 15 + 20 = 35.
        let thunk = Row::new(&arena)
            .child_by_baseline(Lined {
                width: 10.0,
                height: 20.0,
                baseline: 15.0,
            })
            .child_by_baseline(Lined {
                width: 10.0,
                height: 30.0,
                baseline: 10.0,
            })
            .layout(&arena, bounds());
        assert_eq!(thunk.size().height, 35.0);
        // The row's OWN first baseline is the group's shared line —
        // containers propagate the topmost placed line.
        assert_eq!(thunk.first_baseline(), Some(15.0));
    }

    #[test]
    fn a_taller_plain_neighbor_still_wins_the_extent() {
        let arena = Arena::default();
        let thunk = Row::new(&arena)
            .child_by_baseline(Lined {
                width: 10.0,
                height: 20.0,
                baseline: 15.0,
            })
            .child(Lined {
                width: 10.0,
                height: 60.0,
                baseline: 5.0,
            })
            .layout(&arena, bounds());
        assert_eq!(thunk.size().height, 60.0);
    }

    #[test]
    fn pad_offsets_the_propagated_line() {
        let arena = Arena::default();
        let thunk = Lined {
            width: 10.0,
            height: 20.0,
            baseline: 12.0,
        }
        .pad_insets(Insets {
            left: 0.0,
            top: 7.0,
            right: 0.0,
            bottom: 0.0,
        })
        .layout(&arena, bounds());
        assert_eq!(thunk.first_baseline(), Some(19.0));
    }

    #[test]
    fn text_answers_its_font_ascent_as_the_line() {
        let arena = Arena::default();
        let typeface = skia_safe::FontMgr::new()
            .legacy_make_typeface(None, skia_safe::FontStyle::normal())
            .expect("a system typeface");
        let font = skia_safe::Font::new(typeface, 24.0);
        let thunk: ThunkBox<'_, ()> =
            text("hello", font.clone(), skia_safe::Color::WHITE).layout(&arena, bounds());
        let (_, metrics) = font.metrics();
        assert_eq!(thunk.first_baseline(), Some(-metrics.ascent));
        assert!(thunk.size().width > 0.0, "a real face measures");
    }
}

/// Compose's `Modifier.clickable` / the event escape hatch: attaches
/// a raw event handler to whatever the layout produces. The handler
/// is an EVENT closure, not layout logic — the structure stays
/// reified.
pub struct OnEvent<L, H> {
    inner: L,
    handler: H,
}

/// What `OnEvent` runs — closures via the blanket impl, plus the
/// reified handlers (`OnClick`).
pub trait EventHandler<Command> {
    fn handle(
        &self,
        arena: &Arena,
        event: &crate::event::Event<'_>,
        size: Size,
    ) -> crate::event::EventResult<Command>;
}

impl<Command, F> EventHandler<Command> for F
where
    F: for<'event> Fn(
        &Arena,
        &crate::event::Event<'event>,
        Size,
    ) -> crate::event::EventResult<Command>,
{
    fn handle(
        &self,
        arena: &Arena,
        event: &crate::event::Event<'_>,
        size: Size,
    ) -> crate::event::EventResult<Command> {
        self(arena, event, size)
    }
}

/// The reified press handler behind `LayoutExt::on_click`: a
/// `MouseDown` mints the command (a lambda, like Compose's
/// `onClick`), everything else passes.
#[derive(Clone)]
pub struct OnClick<F>(pub F);

impl<Command, F: Fn() -> Command> EventHandler<Command> for OnClick<F> {
    fn handle(
        &self,
        _arena: &Arena,
        event: &crate::event::Event<'_>,
        _size: Size,
    ) -> crate::event::EventResult<Command> {
        match event {
            crate::event::Event::MouseDown { .. } => crate::event::EventResult::Command((self.0)()),
            _ => crate::event::EventResult::Ignored,
        }
    }
}

impl<'a, Command: 'a, L, H> Layout<'a, Command> for OnEvent<L, H>
where
    L: Layout<'a, Command> + 'a,
    H: EventHandler<Command> + 'a,
{
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        let handler = self.handler;
        ThunkBox::new(
            arena,
            self.inner
                .layout(arena, constraints)
                .event(move |arena, event, size| handler.handle(arena, event, size)),
        )
    }
}

/// Compose's `Button(onClick) { content }`: a click surface with a
/// CONTENT SLOT — fill, stroke, radius and padding are the button's
/// chrome; the content is any layout (a `Text`, a `Row` of texts),
/// centered in the padded surface. Replaces the ad-hoc
/// paint-a-round-rect-then-draw_str-then-event chips.
pub struct Button<'a, Command, F> {
    content: LayoutBox<'a, Command>,
    on_click: F,
    insets: Insets,
    fill: Option<skia_safe::Color>,
    stroke: Option<skia_safe::Color>,
    radius: f32,
    min_width: f32,
    enabled: bool,
}

impl<'a, Command: 'a, F: Fn() -> Command + 'a> Button<'a, Command, F> {
    pub fn new(arena: &'a Arena, content: impl Layout<'a, Command> + 'a, on_click: F) -> Self {
        Self {
            content: LayoutBox::new(arena, content),
            on_click,
            insets: Insets::xy(10.0, 4.0),
            fill: None,
            stroke: None,
            radius: 4.0,
            min_width: 0.0,
            enabled: true,
        }
    }

    pub fn fill(mut self, color: skia_safe::Color) -> Self {
        self.fill = Some(color);
        self
    }

    pub fn stroke(mut self, color: skia_safe::Color) -> Self {
        self.stroke = Some(color);
        self
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    pub fn pad_content(mut self, insets: Insets) -> Self {
        self.insets = insets;
        self
    }

    pub fn min_width(mut self, min_width: f32) -> Self {
        self.min_width = min_width;
        self
    }

    /// A disabled button swallows its presses (the caller dims its
    /// own colors).
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

impl<'a, Command: 'a, F: Fn() -> Command + 'a> Layout<'a, Command> for Button<'a, Command, F> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        let x = self.insets.left + self.insets.right;
        let y = self.insets.top + self.insets.bottom;
        let inner = Constraints {
            min: Size::default(),
            max: Size::new(
                (constraints.max.width - x).max(0.0),
                (constraints.max.height - y).max(0.0),
            ),
        };
        let content = self.content.layout(arena, inner);
        let label = content.size();
        let size = Size::new(
            (label.width + x)
                .max(self.min_width)
                .min(constraints.max.width),
            (label.height + y).min(constraints.max.height),
        );
        let mut surface = container(arena, size);
        surface.place_boxed(
            ((size.width - label.width) * 0.5).max(0.0),
            ((size.height - label.height) * 0.5).max(0.0),
            content,
        );
        let Button {
            fill,
            stroke,
            radius,
            on_click,
            enabled,
            ..
        } = self;
        let chrome = surface.paint_below(move |_arena, canvas, rect| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            if let Some(fill) = fill {
                paint.set_color(fill);
                canvas.draw_round_rect(rect, radius, radius, &paint);
            }
            if let Some(stroke) = stroke {
                paint.set_stroke(true);
                paint.set_stroke_width(1.0);
                paint.set_color(stroke);
                canvas.draw_round_rect(rect.with_inset((0.5, 0.5)), radius, radius, &paint);
            }
        });
        let armed = chrome.event(move |_arena, event, _size| match event {
            crate::event::Event::MouseDown { .. } if enabled => {
                crate::event::EventResult::Command(on_click())
            }
            crate::event::Event::MouseDown { .. } => crate::event::EventResult::Handled,
            _ => crate::event::EventResult::Ignored,
        });
        ThunkBox::new(arena, armed)
    }
}

#[cfg(test)]
mod button_tests {
    use super::*;
    use crate::event::{Event, EventResult, MouseButton};
    use crate::leaf::leaf;
    use crate::Widget as _;

    #[test]
    fn a_button_wraps_its_content_and_a_press_mints_the_command() {
        let arena = Arena::default();
        let thunk = Button::new(&arena, fixed(leaf::<u32>(40.0, 10.0)), || 7u32)
            .pad_content(Insets::xy(10.0, 4.0))
            .min_width(0.0)
            .layout(
                &arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(400.0, 400.0),
                },
            );
        let size = thunk.size();
        assert_eq!((size.width, size.height), (60.0, 18.0));

        let viewport = skia_safe::Rect::from_wh(60.0, 18.0);
        let widget = thunk.realize(&arena, viewport);
        let result = widget.handle_event(
            &arena,
            &Event::MouseDown {
                point: skia_safe::Point::new(5.0, 5.0),
                button: MouseButton::Left,
                mods: Default::default(),
                count: 1,
            },
            viewport,
        );
        assert!(matches!(result, EventResult::Command(7)));
    }

    #[test]
    fn a_disabled_button_swallows_the_press() {
        let arena = Arena::default();
        let thunk = Button::new(&arena, fixed(leaf::<u32>(40.0, 10.0)), || 7u32)
            .enabled(false)
            .layout(
                &arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(400.0, 400.0),
                },
            );
        let viewport = skia_safe::Rect::from_wh(60.0, 18.0);
        let widget = thunk.realize(&arena, viewport);
        let result = widget.handle_event(
            &arena,
            &Event::MouseDown {
                point: skia_safe::Point::new(5.0, 5.0),
                button: MouseButton::Left,
                mods: Default::default(),
                count: 1,
            },
            viewport,
        );
        assert!(matches!(result, EventResult::Handled));
    }
}

/// Compose's `Box`, named `ZBox` (std owns `Box`): children stack in
/// Z — later = on top — each placed by an alignment; the box takes
/// its largest child, clamped into the incoming constraints. Purely
/// VISUAL stacking; the base+modal VIEW with routing semantics
/// remains `imba::stack::Stack`.
enum ZChild {
    Aligned(Alignment),
    /// Compose's `Modifier.matchParentSize`: measured AFTER the
    /// aligned children decide the box, with the box's size tight —
    /// the backdrop bar, the full-card shield.
    MatchParent,
}

pub struct ZBox<'a, Command> {
    arena: &'a Arena,
    children: Vec<(ZChild, LayoutBox<'a, Command>)>,
    alignment: Alignment,
}

impl<'a, Command: 'a> ZBox<'a, Command> {
    pub fn new(arena: &'a Arena) -> Self {
        Self {
            arena,
            children: Vec::new(),
            alignment: Alignment::TopStart,
        }
    }

    /// The default placement (`contentAlignment`).
    pub fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    pub fn child(mut self, child: impl Layout<'a, Command> + 'a) -> Self {
        let alignment = self.alignment;
        self.children.push((
            ZChild::Aligned(alignment),
            LayoutBox::new(self.arena, child),
        ));
        self
    }

    /// Compose's per-child `Modifier.align(…)` inside a Box.
    pub fn child_aligned(
        mut self,
        alignment: Alignment,
        child: impl Layout<'a, Command> + 'a,
    ) -> Self {
        self.children.push((
            ZChild::Aligned(alignment),
            LayoutBox::new(self.arena, child),
        ));
        self
    }

    /// Compose's `Modifier.matchParentSize`: laid with the box's
    /// decided size, tight; does not influence the box's extent.
    pub fn child_match_parent(mut self, child: impl Layout<'a, Command> + 'a) -> Self {
        self.children
            .push((ZChild::MatchParent, LayoutBox::new(self.arena, child)));
        self
    }
}

impl<'a, Command> LayoutValue for ZBox<'a, Command> {}

impl<'a, Command: 'a> Layout<'a, Command> for ZBox<'a, Command> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        let loose = constraints.loosen();
        let mut slots: Vec<Option<(Alignment, ThunkBox<'a, Command>)>> =
            self.children.iter().map(|_| None).collect();
        let mut matchers: Vec<(usize, LayoutBox<'a, Command>)> = Vec::new();
        let mut extent = Size::new(constraints.min.width, constraints.min.height);
        for (index, (kind, child)) in self.children.into_iter().enumerate() {
            match kind {
                ZChild::Aligned(alignment) => {
                    let thunk = child.layout(arena, loose);
                    let size = thunk.size();
                    extent.width = extent.width.max(size.width);
                    extent.height = extent.height.max(size.height);
                    slots[index] = Some((alignment, thunk));
                }
                ZChild::MatchParent => matchers.push((index, child)),
            }
        }
        extent.width = extent.width.min(constraints.max.width);
        extent.height = extent.height.min(constraints.max.height);
        for (index, child) in matchers {
            slots[index] = Some((
                Alignment::TopStart,
                child.layout(arena, Constraints::tight(extent)),
            ));
        }
        let mut frame = container(self.arena, extent);
        for slot in slots {
            let Some((alignment, thunk)) = slot else {
                continue;
            };
            let (x, y) = alignment.place(extent, thunk.size());
            frame.place_boxed(x, y, thunk);
        }
        ThunkBox::new(arena, frame)
    }
}

/// A fixed-size empty box — Compose's `Spacer(Modifier.size(…))`.
pub fn spacer<'a, Command: 'a>(
    width: f32,
    height: f32,
) -> Fixed<crate::Eager<crate::leaf::Leaf<'a, Command>>> {
    fixed(crate::leaf::leaf(width, height))
}

/// A painter running UNDER the layout's own pixels — Compose's
/// `Modifier.background`, generalized to a paint closure (painting
/// is imperative by nature; the STRUCTURE stays reified).
pub struct Backdrop<L, F> {
    inner: L,
    painter: F,
}

impl<L, F> LayoutValue for Backdrop<L, F> {}

impl<'a, Command: 'a, L, F> Layout<'a, Command> for Backdrop<L, F>
where
    L: Layout<'a, Command> + 'a,
    F: Fn(&Arena, &skia_safe::Canvas, skia_safe::Rect) + 'a,
{
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        ThunkBox::new(
            arena,
            self.inner
                .layout(arena, constraints)
                .paint_below(self.painter),
        )
    }
}

/// The reified press shield: consumes presses so they stop falling
/// through to whatever sits underneath (panel chrome over content).
#[derive(Clone)]
pub struct Shield;

impl<Command> EventHandler<Command> for Shield {
    fn handle(
        &self,
        _arena: &Arena,
        event: &crate::event::Event<'_>,
        _size: Size,
    ) -> crate::event::EventResult<Command> {
        match event {
            crate::event::Event::MouseDown { .. } => crate::event::EventResult::Handled,
            _ => crate::event::EventResult::Ignored,
        }
    }
}
