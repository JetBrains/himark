// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[derive(Clone)]
struct Stub;

impl crate::View for Stub {
    type Command = std::convert::Infallible;

    fn perform(
        &mut self,
        _store: &mut crate::store::Store,
        _ui: &crate::ui::UiCtx,
        _command: std::convert::Infallible,
        _fx: &mut crate::effect::Effects<'_, std::convert::Infallible>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a crate::arena::Arena,
        _store: &'a crate::store::Store,
        _ui: &'a crate::ui::UiCtx,
    ) -> impl crate::layout::Layout<'a, std::convert::Infallible> + crate::layout::LayoutValue + 'a
    {
        crate::layout::laid(
            move |_arena: &'a crate::arena::Arena,
                  _constraints: crate::constraints::Constraints| {
                crate::leaf::leaf::<std::convert::Infallible>(10.0, 30.0)
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
    use crate::ui::UiCtx;
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
    use crate::ui::UiCtx;
    use skia_safe::{Point, Rect, Size};

    // A row that echoes the pointer coordinates it was handed.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct EchoedPoint(Point);

    impl std::fmt::Display for EchoedPoint {
        fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(out, "{self:?}")
        }
    }

    #[derive(Clone)]
    struct Echo;
    impl crate::View for Echo {
        type Command = EchoedPoint;
        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &UiCtx,
            _command: EchoedPoint,
            _fx: &mut crate::effect::Effects<'_, EchoedPoint>,
        ) {
        }
        fn display<'a>(
            &'a self,
            _arena: &'a crate::arena::Arena,
            _store: &'a Store,
            _ui: &'a UiCtx,
        ) -> impl crate::layout::Layout<'a, EchoedPoint> + crate::layout::LayoutValue + 'a {
            crate::layout::laid(
                move |_arena: &'a crate::arena::Arena, _constraints: Constraints| {
                    crate::leaf::leaf::<EchoedPoint>(200.0, 30.0).event(|_arena, event, _size| {
                        match event {
                            Event::MouseDrag { point, .. } => {
                                EventResult::command(EchoedPoint(*point))
                            }
                            _ => EventResult::Ignored,
                        }
                    })
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
        crate::layout::Layout::layout(
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
    match result.into_command() {
        Some(ListCommand::Child(index, point)) => {
            assert_eq!(index, 2, "the focused row answers");
            assert_eq!(
                (point.0.x, point.0.y),
                (5.0, 5.0),
                "the drag arrives in row coordinates"
            );
        }
        _ => panic!("the drag routes to the focused row"),
    }
}

/// A keyed pure INSERT in the middle of the list (empty delete range):
/// the rows must land at the index and every standing key must shift.
#[test]
fn a_mid_list_keyed_insert_lands_and_shifts_keys() {
    let mut view: ListView<Stub, &'static str> = ListView::empty();
    let mut slice: ListSlice<Stub, &'static str> = ListSlice::new();
    slice.push_keyed_sized("banner", Stub, 20.0);
    slice.push_keyed_sized("b.header", Stub, 30.0);
    slice.push_keyed_sized("b.diff", Stub, 100.0);
    slice.cover("b", 1..3);
    view.splice_slice(0..0, slice);
    assert_eq!(view.len(), 3);
    assert_eq!(view.row_range(&"b"), Some(1..3), "the cover stands");

    // The pure insert at index 1 — between the banner and b's rows.
    let mut fresh: ListSlice<Stub, &'static str> = ListSlice::new();
    fresh.push_keyed_sized("a.header", Stub, 30.0);
    fresh.push_keyed_sized("a.diff", Stub, 100.0);
    fresh.cover("a", 0..2);
    view.splice_slice(1..1, fresh);

    assert_eq!(view.len(), 5, "the two rows landed");
    assert_eq!(view.row_range(&"a"), Some(1..3), "a covers its pair");
    assert_eq!(view.row_range(&"b"), Some(3..5), "b shifted right");
    assert_eq!(view.key_at(1), Some(&"a.header"));
    assert_eq!(view.key_at(3), Some(&"b.header"));
}

/// A row whose widget emits an overlay (a face's completion popup).
#[derive(Clone)]
struct Emitter {
    emits: bool,
}

const ROW_HOST: crate::overlay::OverlayHost = crate::overlay::OverlayHost("row-host");

struct EmitterWidget {
    emits: bool,
}

impl<'a> crate::Widget<'a, std::convert::Infallible> for EmitterWidget {
    fn size(&self) -> skia_safe::Size {
        skia_safe::Size::new(10.0, 30.0)
    }

    fn handle_event(
        &self,
        _arena: &crate::arena::Arena,
        _event: &crate::event::Event<'_>,
        _viewport: skia_safe::Rect,
    ) -> crate::event::EventResult<std::convert::Infallible> {
        crate::event::EventResult::Ignored
    }

    fn overlays(&mut self) -> Vec<crate::overlay::Overlay<'a, std::convert::Infallible>> {
        if !self.emits {
            return Vec::new();
        }
        vec![crate::overlay::Overlay {
            host: ROW_HOST,
            attachment: crate::overlay::Attachment::Flank,
            anchor: skia_safe::Rect::from_xywh(2.0, 4.0, 6.0, 8.0),
            content: Box::new(|_host: skia_safe::Size, _anchor: skia_safe::Rect| Vec::new()),
        }]
    }
}

impl crate::View for Emitter {
    type Command = std::convert::Infallible;

    fn perform(
        &mut self,
        _store: &mut crate::store::Store,
        _ui: &crate::ui::UiCtx,
        _command: std::convert::Infallible,
        _fx: &mut crate::effect::Effects<'_, std::convert::Infallible>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a crate::arena::Arena,
        _store: &'a crate::store::Store,
        _ui: &'a crate::ui::UiCtx,
    ) -> impl crate::layout::Layout<'a, std::convert::Infallible> + crate::layout::LayoutValue + 'a
    {
        let emits = self.emits;
        crate::layout::laid(
            move |_arena: &'a crate::arena::Arena,
                  _constraints: crate::constraints::Constraints| {
                crate::eager(EmitterWidget { emits })
            },
        )
    }
}

/// The list realizes rows per traversal, so a row's popup used to
/// die with the traversal that minted it: the window never saw a
/// canvas face's completion popup. The visible rows' overlays now
/// surface through the list, anchored in list coordinates.
#[test]
fn visible_rows_surface_their_overlays_through_the_list() {
    use crate::store::Store;
    use crate::ui::UiCtx;
    let rows: ListView<Emitter> = ListView::from_rope(crate::list::measured(
        [false, true, false, true]
            .into_iter()
            .map(|emits| (Emitter { emits }, 30.0)),
    ));
    let store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let arena = crate::arena::Arena::default();
    let viewport = skia_safe::Rect::from_xywh(0.0, 0.0, 100.0, 75.0);
    let mut widget = crate::layout::Layout::layout(
        crate::View::display(&rows, &arena, &store, &ui),
        &arena,
        crate::constraints::Constraints::tight(skia_safe::Size::new(100.0, 120.0)),
    )
    .realize(&arena, viewport);
    let overlays = crate::Widget::overlays(&mut widget);
    let anchors: Vec<(crate::overlay::OverlayHost, skia_safe::Rect)> = overlays
        .iter()
        .map(|overlay| (overlay.host, overlay.anchor))
        .collect();
    assert_eq!(
        anchors,
        vec![
            // Row 1 (top 30) emits and is visible; row 3 (top 90)
            // lies past the viewport and is never realized.
            (ROW_HOST, skia_safe::Rect::from_xywh(2.0, 34.0, 6.0, 8.0)),
        ],
        "the emitting visible row's anchor rides into list coordinates"
    );
}

/// The transcript's follow mode (docs/mounted-parity.md): the tail
/// ask aims at the LIVE bottom edge, re-aims as content grows,
/// disarms once the tail is visible — and is SUPERSEDED the moment
/// the viewport moves up. The reveal road never fights the hand.
#[test]
fn the_tail_ask_follows_growth_and_yields_to_the_hand() {
    use crate::event::{Event, Motion, Placement};
    use crate::store::Store;
    use crate::ui::UiCtx;
    let mut rows = list(&[30.0; 10]); // total 300
    let store = Store::new();
    let ui = UiCtx::dont_use_too_slow();

    // One clock tick seen from a viewport at `top` — what the list
    // broadcasts there (and performs back into itself, as a shell's
    // drain would).
    let clock = |rows: &mut ListView<Stub>, top: f32| -> Option<crate::event::Reveal> {
        let viewport = skia_safe::Rect::from_xywh(0.0, top, 100.0, 100.0);
        let (reveal, commands) = {
            let arena = crate::arena::Arena::default();
            let widget = crate::layout::Layout::layout(
                crate::View::display(&*rows, &arena, &store, &ui),
                &arena,
                crate::constraints::Constraints::tight(skia_safe::Size::new(
                    100.0,
                    rows.total_height(),
                )),
            )
            .realize(&arena, viewport);
            let result = crate::Widget::handle_event(
                &widget,
                &arena,
                &Event::AnimationClock {
                    now: crate::anim::AnimationClock::from_millis(16.0),
                },
                viewport,
            );
            match result.into_handled() {
                None => (None, Vec::new()),
                Some(handled) => (handled.reveal, handled.commands),
            }
        };
        for command in commands {
            let mut batch = crate::effect::Batch::new();
            rows.perform(
                &mut Store::clone(&store),
                &ui,
                command,
                &mut batch.effects(),
            );
        }
        reveal
    };

    rows.reveal_tail();
    let ask = clock(&mut rows, 0.0).expect("the armed tail asks from the top");
    assert_eq!(ask.placement, Placement::EnsureVisible);
    assert_eq!(ask.motion, Motion::Jump, "a fresh landing never glides");
    assert!(
        ask.rect.bottom > 297.0 && ask.rect.bottom <= 300.0,
        "the ask aims at the bottom edge: {:?}",
        ask.rect
    );

    // The scroller landed at the tail: the ask satisfies — QUIET,
    // but the mode stands.
    assert_eq!(clock(&mut rows, 200.0), None, "satisfied: quiet");
    assert_eq!(clock(&mut rows, 200.0), None, "and stays quiet");

    // The content grows — a stream append, or a landing storm
    // re-heighting history. The STANDING mode re-asks at the live
    // total on its own, with no re-arm in sight.
    let len = rows.len();
    rows.splice(len..len, (0..5).map(|_| (Stub, 30.0)));
    let ask = clock(&mut rows, 200.0).expect("growth re-asks unprompted");
    assert!(
        ask.rect.bottom > 447.0 && ask.rect.bottom <= 450.0,
        "the ask re-aims at the grown tail: {:?}",
        ask.rect
    );

    // A PRESS into the rows is the hand too: expanding a tool call
    // must not get yanked to the grown bottom.
    let mut batch = crate::effect::Batch::new();
    rows.perform(
        &mut Store::clone(&store),
        &ui,
        ListCommand::Focus(3, None),
        &mut batch.effects(),
    );
    assert_eq!(clock(&mut rows, 200.0), None, "a press releases the follow");

    // The hand proper: re-armed, then a viewport that moved UP and
    // LEFT the tail supersedes the ask — no re-emission yanks the
    // user back down. (A clamp that stays on the bottom edge — a
    // shrinking row — is exempt.)
    rows.reveal_tail();
    rows.perform(
        &mut Store::clone(&store),
        &ui,
        ListCommand::ViewportTop(350.0, 100.0),
        &mut batch.effects(),
    );
    rows.perform(
        &mut Store::clone(&store),
        &ui,
        ListCommand::ViewportTop(150.0, 100.0),
        &mut batch.effects(),
    );
    assert_eq!(
        clock(&mut rows, 150.0),
        None,
        "the hand outranks the follow"
    );
}

/// The rope holds its rows behind `Arc`s: a `Child` or `SetHeight`
/// splice clones the touched leaf — up to BRANCHING_FACTOR elements —
/// and must copy refcounts, never rows. A chat row carries editors
/// (documents, syntax trees); copying 32 of them per command was the
/// Oct 30 2026 beach ball's amplifier.
#[test]
fn a_splice_copies_refcounts_not_rows() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CLONES: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Copy, Debug)]
    struct Nudge;
    impl std::fmt::Display for Nudge {
        fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            out.write_str("nudge")
        }
    }

    struct Counted;
    impl Clone for Counted {
        fn clone(&self) -> Self {
            CLONES.fetch_add(1, Ordering::Relaxed);
            Counted
        }
    }
    impl crate::View for Counted {
        type Command = Nudge;

        fn perform(
            &mut self,
            _store: &mut crate::store::Store,
            _ui: &crate::ui::UiCtx,
            _command: Nudge,
            _fx: &mut crate::effect::Effects<'_, Nudge>,
        ) {
        }

        fn display<'a>(
            &'a self,
            _arena: &'a crate::arena::Arena,
            _store: &'a crate::store::Store,
            _ui: &'a crate::ui::UiCtx,
        ) -> impl crate::layout::Layout<'a, Nudge> + crate::layout::LayoutValue + 'a {
            crate::layout::laid(
                move |_arena: &'a crate::arena::Arena,
                      _constraints: crate::constraints::Constraints| {
                    crate::leaf::leaf::<Nudge>(10.0, 30.0)
                },
            )
        }
    }

    let mut rows: ListView<Counted> =
        ListView::from_rope(crate::list::measured((0..40).map(|_| (Counted, 30.0))));
    let store = crate::store::Store::new();
    let ui = crate::ui::UiCtx::dont_use_too_slow();

    CLONES.store(0, Ordering::Relaxed);
    let mut batch = crate::effect::Batch::new();
    rows.perform(
        &mut crate::store::Store::clone(&store),
        &ui,
        ListCommand::Child(5, Nudge),
        &mut batch.effects(),
    );
    let child = CLONES.load(Ordering::Relaxed);

    CLONES.store(0, Ordering::Relaxed);
    let mut batch = crate::effect::Batch::new();
    rows.perform(
        &mut crate::store::Store::clone(&store),
        &ui,
        ListCommand::SetHeight(7, 12.0),
        &mut batch.effects(),
    );
    let set_height = CLONES.load(Ordering::Relaxed);

    // A snapshot elsewhere (a cloned view, a standing frame) shares
    // the rows: the performed row copies ONCE, its neighbours never.
    let snapshot = rows.clone();
    CLONES.store(0, Ordering::Relaxed);
    let mut batch = crate::effect::Batch::new();
    rows.perform(
        &mut crate::store::Store::clone(&store),
        &ui,
        ListCommand::Child(9, Nudge),
        &mut batch.effects(),
    );
    let shared = CLONES.load(Ordering::Relaxed);
    drop(snapshot);

    eprintln!("[clones] child={child} set_height={set_height} shared={shared}");
    assert_eq!(child, 0, "a uniquely held row performs in place");
    assert_eq!(set_height, 0, "a height splice copies refcounts");
    assert_eq!(shared, 1, "a shared row copies once — never the leaf");
    assert_eq!(rows.len(), 40);
}
