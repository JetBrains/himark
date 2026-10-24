// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Contents drawer's named layouts: the side COLUMN it stands
//! in, and the typographic pieces of a table-of-contents line — the
//! oblique kind word, the dotted LEADER and the leader LINE that
//! runs a name to its number.

use imba::{
    arena::Arena,
    constraints::Constraints,
    layout::{Layout, LayoutExt as _, LayoutValue},
    thunk_ext::ThunkExt as _,
    Thunk as _,
};
use skia_safe::Size;

fn bounded(extent: f32) -> Option<f32> {
    (extent < f32::MAX / 2.0).then_some(extent)
}

/// The drawer's side COLUMN (docs/Design.md, "a column, not a
/// sheet"): the window's side width on the ground, a hairline on its
/// open edge, the content under the band's own words (the band says
/// Contents; the column has no header of its own, like the Sessions
/// drawer). A full-size key surface lies underneath — it answers the
/// drawer's own keys and the paint-time probes wherever the pointer
/// is.
pub(crate) struct ContentsColumn<'a, Command, Keys> {
    pub content: imba::layout::LayoutBox<'a, Command>,
    pub width: f32,
    pub top_gap: f32,
    pub ground: skia_safe::Color,
    pub rule: skia_safe::Color,
    pub keys: Keys,
}

impl<Command, Keys> LayoutValue for ContentsColumn<'_, Command, Keys> {}

impl<'a, Command: 'a, Keys> Layout<'a, Command> for ContentsColumn<'a, Command, Keys>
where
    Keys: imba::layout::EventHandler<Command> + 'a,
{
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, Command> {
        let ContentsColumn {
            content,
            width,
            top_gap,
            ground,
            rule,
            keys,
        } = self;
        let height = constraints.max.height;
        let column = imba::layout::Column::new(arena)
            .weighted(
                1.0,
                content.pad_insets(imba::layout::Insets {
                    top: top_gap,
                    ..Default::default()
                }),
            )
            .shield()
            .layout(arena, Constraints::tight(Size::new(width, height)));
        // The rule rides the column's open edge — its LEFT: the
        // column stands over the side column, from the right, like
        // the dock whose rule it mirrors. The ground goes under both.
        let column = hikit::ui::Hairline::new(rule)
            .left()
            .wrap(column)
            .paint_below(hikit::ui::ground(ground));
        imba::layout::ZBox::new(arena)
            .child(imba::layout::Fill::new().on_event(keys))
            .child(imba::layout::fixed(column))
            .layout(arena, constraints)
    }
}

/// A word set OBLIQUE: the chrome face has no italic cut, so the
/// word leans by a skew about its own baseline (the mock's italic
/// kind words).
pub(crate) struct Oblique<L> {
    pub inner: L,
    pub slant: f32,
}

impl<L> LayoutValue for Oblique<L> {}

impl<'a, Command: 'a, L: Layout<'a, Command> + 'a> Layout<'a, Command> for Oblique<L> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, Command> {
        let word = self.inner.layout(arena, constraints);
        let baseline = word.first_baseline().unwrap_or(word.size().height);
        // The lean carries the word's top past its upright box: the
        // box grows by the lean so no glyph is clipped.
        let lean = (self.slant * baseline).ceil();
        let mut leaning = imba::container::container(
            arena,
            Size::new(word.size().width + lean, word.size().height),
        );
        leaning.place_boxed(0.0, 0.0, word);
        let word = leaning;
        let slant = self.slant;
        imba::ThunkBox::new(
            arena,
            word.paint_below(move |_arena, canvas, rect| {
                let line = rect.top + baseline;
                canvas.save();
                canvas.translate((0.0, line));
                canvas.skew((-slant, 0.0));
                canvas.translate((0.0, -line));
            })
            .paint_above(|_arena, canvas, _rect| {
                canvas.restore();
            }),
        )
    }
}

/// The dotted LEADER a table of contents runs from a name to its
/// number: a row of dots raised `rise` off the baseline it joins.
pub(crate) struct DottedLeader {
    pub width: f32,
    pub color: skia_safe::Color,
    pub dot: f32,
    pub rise: f32,
}

impl LayoutValue for DottedLeader {}

impl<'a, Command: 'a> Layout<'a, Command> for DottedLeader {
    fn layout(self, arena: &'a Arena, _constraints: Constraints) -> imba::ThunkBox<'a, Command> {
        let DottedLeader {
            width,
            color,
            dot,
            rise,
        } = self;
        let height = dot + rise;
        let dots = imba::leaf::leaf::<Command>(width, height).paint_instead(
            move |_arena, canvas, rect| {
                let mut paint = skia_safe::Paint::default();
                paint.set_color(color);
                let mut x = rect.left;
                while x + dot <= rect.right {
                    canvas.draw_rect(skia_safe::Rect::from_xywh(x, rect.top, dot, dot), &paint);
                    x += dot * 2.0;
                }
            },
        );
        // The line the row aligns on is the dots' foot plus the rise.
        imba::ThunkBox::new(
            arena,
            imba::layout::WithBaseline {
                thunk: dots,
                baseline: height,
            },
        )
    }
}

/// A leader LINE: the name, the dotted leader taking whatever room
/// is left, the number right-aligned in its slot — one baseline.
/// The number and the leader's minimum are reserved FIRST, so a
/// long name ellipsizes instead of pushing the number off the row.
pub(crate) struct LeaderLine<'a, Command> {
    pub name: imba::layout::LayoutBox<'a, Command>,
    pub number: imba::layout::Text,
    pub number_min_width: f32,
    pub color: skia_safe::Color,
    pub min: f32,
    pub dot: f32,
    pub rise: f32,
    pub gap: f32,
}

impl<Command> LayoutValue for LeaderLine<'_, Command> {}

impl<'a, Command: 'a> Layout<'a, Command> for LeaderLine<'a, Command> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, Command> {
        let LeaderLine {
            name,
            number,
            number_min_width,
            color,
            min,
            dot,
            rise,
            gap,
        } = self;
        let loose = Constraints {
            min: Size::default(),
            max: constraints.max,
        };
        let number = Layout::<Command>::layout(number, arena, loose);
        let number_width = number.size().width;
        let slot = number_width.max(number_min_width);
        let reserved = slot + min + gap * 2.0;
        let name = name.layout(
            arena,
            Constraints {
                min: Size::default(),
                max: Size::new(
                    bounded(constraints.max.width)
                        .map_or(f32::MAX, |width| (width - reserved).max(0.0)),
                    constraints.max.height,
                ),
            },
        );
        let width = bounded(constraints.max.width)
            .unwrap_or(name.size().width + reserved)
            .max(constraints.min.width);
        let leader = (width - name.size().width - slot - gap * 2.0).max(min);
        imba::layout::Row::new(arena)
            .gap(gap)
            .child_by_baseline(imba::layout::fixed(name))
            .child_by_baseline(DottedLeader {
                width: leader,
                color,
                dot,
                rise,
            })
            .child_by_baseline(imba::layout::fixed(number).pad_insets(imba::layout::Insets {
                left: slot - number_width,
                ..Default::default()
            }))
            .layout(arena, loose)
    }
}
