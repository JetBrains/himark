// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[derive(Clone)]
struct Stub;

impl crate::View for Stub {
    type Command = ();

    fn perform(
        &mut self,
        _store: &mut crate::store::Store,
        _ui: &crate::UiCtx,
        _command: (),
        _fx: &mut crate::effect::Effects<'_, ()>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a crate::arena::Arena,
        _store: &'a crate::store::Store,
        _ui: &'a crate::UiCtx,
    ) -> impl crate::Layout<'a, ()> + crate::LayoutValue + 'a {
        crate::laid(
            move |_arena: &'a crate::arena::Arena,
                  _constraints: crate::constraints::Constraints| {
                crate::leaf::leaf::<()>(10.0, 30.0)
            },
        )
    }
}

fn list(heights: &[f32]) -> ListView<Stub> {
    ListView::from_rope(crate::list::measured(
        heights.iter().map(|height| (Stub, *height)),
    ))
}

#[test]
fn extent_of_reads_metric_spans() {
    let rows = list(&[20.0, 30.0, 10.0]);
    assert_eq!(rows.extent_of(0..1), 20.0);
    assert_eq!(rows.extent_of(1..3), 40.0);
    assert_eq!(rows.extent_of(0..3), 60.0);
    assert_eq!(rows.extent_of(2..3), 10.0);
    assert_eq!(list(&[20.0]).extent_of(0..1), 20.0);
}

#[test]
fn probe_unroll_is_monotone() {
    use crate::store::Store;
    use crate::UiCtx;
    let mut rows = list(&[30.0, 30.0, 30.0]);

    rows.splice_animated(1..2, (0..5).map(|_| (Stub, 30.0)));
    let store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let heights = |rows: &ListView<Stub>| -> Vec<f32> {
        rows.items.iter().map(|element| element.height).collect()
    };
    let mut previous = heights(&rows);
    eprintln!("[probe] t=0 {previous:?}");
    for ms in (0..200).step_by(16) {
        let mut batch = crate::effect::Batch::new();
        rows.perform(
            &mut Store::clone(&store),
            &ui,
            ListCommand::Animate(crate::anim::AnimationClock::from_millis(ms as f64)),
            &mut batch.effects(),
        );
        let current = heights(&rows);
        eprintln!("[probe] t={ms} {current:?}");
        assert_eq!(current[0], 30.0, "the row above never moves");
        assert_eq!(
            *current.last().expect("tail"),
            30.0,
            "the row below never moves"
        );
        for (index, (now, before)) in current.iter().zip(&previous).enumerate() {
            assert!(
                now + 0.01 >= *before,
                "row {index} shrank mid-unroll: {before} -> {now}"
            );
        }
        previous = current;
    }
}

#[test]
fn drags_reach_the_focused_row_in_row_coordinates() {
    use crate::constraints::Constraints;
    use crate::event::{Event, EventResult};
    use crate::store::Store;
    use crate::thunk_ext::ThunkExt;
    use crate::UiCtx;
    use skia_safe::{Point, Rect, Size};

    // A row that echoes the pointer coordinates it was handed.
    #[derive(Clone)]
    struct Echo;
    impl crate::View for Echo {
        type Command = Point;
        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &UiCtx,
            _command: Point,
            _fx: &mut crate::effect::Effects<'_, Point>,
        ) {
        }
        fn display<'a>(
            &'a self,
            _arena: &'a crate::arena::Arena,
            _store: &'a Store,
            _ui: &'a UiCtx,
        ) -> impl crate::Layout<'a, Point> + crate::LayoutValue + 'a {
            crate::laid(
                move |_arena: &'a crate::arena::Arena, _constraints: Constraints| {
                    crate::leaf::leaf::<Point>(200.0, 30.0).event(
                        |_arena, event, _size| match event {
                            Event::MouseDrag { point, .. } => EventResult::Command(*point),
                            _ => EventResult::Ignored,
                        },
                    )
                },
            )
        }
    }

    let mut rows: ListView<Echo> =
        ListView::from_rope(crate::list::measured((0..3).map(|_| (Echo, 30.0))));
    // The press focused the THIRD row (rows at y 0..30, 30..60, 60..90).
    rows.focused = Some(2);

    let arena = crate::arena::Arena::default();
    let store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let viewport = Rect::from_wh(200.0, 90.0);
    let widget = crate::Thunk::realize(
        crate::Layout::layout(
            rows.display(&arena, &store, &ui),
            &arena,
            Constraints::tight(Size::new(200.0, 90.0)),
        ),
        &arena,
        viewport,
    );

    // A drag over the focused row, in LIST coordinates: y = 65 is five
    // pixels into the row. The row must see (5, 5), not (5, 65) — the
    // untranslated point lands past a short row's content and, in a
    // chat cell, selects to the end of the message.
    let result = widget.handle_event(
        &arena,
        &Event::MouseDrag {
            point: Point::new(5.0, 65.0),
            mods: Default::default(),
        },
        viewport,
    );
    match result {
        EventResult::Command(ListCommand::Child(index, point)) => {
            assert_eq!(index, 2, "the focused row answers");
            assert_eq!(
                (point.x, point.y),
                (5.0, 5.0),
                "the drag arrives in row coordinates"
            );
        }
        _ => panic!("the drag routes to the focused row"),
    }
}
