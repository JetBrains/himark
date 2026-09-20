// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::cell::RefCell;

use skia_safe::{Point, Rect, Size};

use crate::{
    arena::Arena,
    container::Container,
    event::{Event, EventResult, MouseButton},
    leaf::leaf,
    thunk_ext::ThunkExt,
    Thunk, ThunkBox, Widget,
};

use super::OverlayHost;

const HOST: OverlayHost = OverlayHost("test-host");
const OUTER: OverlayHost = OverlayHost("outer-host");

#[derive(Debug, PartialEq)]
enum Cmd {
    Hit(&'static str, Point),
    Text(&'static str),
}

struct HitBox {
    name: &'static str,
    size: Size,
    takes_text: bool,
}

impl HitBox {
    fn new(name: &'static str, width: f32, height: f32) -> Self {
        Self {
            name,
            size: Size::new(width, height),
            takes_text: false,
        }
    }

    fn texty(mut self) -> Self {
        self.takes_text = true;
        self
    }
}

impl Widget<'_, Cmd> for HitBox {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(&self, _arena: &Arena, event: &Event<'_>, _viewport: Rect) -> EventResult<Cmd> {
        match event {
            Event::MouseDown {
                point,
                button: MouseButton::Left,
                ..
            } => EventResult::Command(Cmd::Hit(self.name, *point)),
            Event::TextInput { .. } if self.takes_text => {
                EventResult::Command(Cmd::Text(self.name))
            }
            _ => EventResult::Ignored,
        }
    }
}

fn press(x: f32, y: f32) -> Event<'static> {
    Event::MouseDown {
        mods: Default::default(),
        point: Point::new(x, y),
        button: MouseButton::Left,
        count: 1,
    }
}

fn command<C>(result: EventResult<C>) -> C {
    match result {
        EventResult::Command(command) => command,
        EventResult::Commands(_) => panic!("expected one command, got a batch"),
        EventResult::Handled => panic!("expected a command, got handled"),
        EventResult::Ignored => panic!("expected a command, got ignored"),
        EventResult::Reveal(_) => panic!("expected a command, got a reveal"),
    }
}

fn viewport() -> Rect {
    Rect::from_wh(200.0, 200.0)
}

fn one<'a, C: 'a>(
    arena: &'a Arena,
    x: f32,
    y: f32,
    thunk: impl Thunk<'a, C> + 'a,
) -> Vec<(Point, ThunkBox<'a, C>)> {
    vec![(Point::new(x, y), ThunkBox::new(arena, thunk))]
}

#[test]
fn a_plain_widget_answers_no_overlays_and_allocates_nothing() {
    let arena = Arena::default();
    let mut widget = leaf::<Cmd>(10.0, 10.0).realize(&arena, viewport());
    let overlays = widget.overlays();
    assert!(overlays.is_empty());
    assert_eq!(overlays.capacity(), 0);
}

#[test]
fn a_request_is_minted_at_realization_self_relative_and_drained_once() {
    let arena = Arena::default();
    let recorded = RefCell::new(Vec::new());
    let mut widget = leaf::<Cmd>(7.0, 9.0)
        .overlay(HOST, |host_size, anchor: Rect| {
            recorded.borrow_mut().push((host_size, anchor));
            Vec::new()
        })
        .realize(&arena, viewport());

    let mut overlays = widget.overlays();
    assert_eq!(overlays.len(), 1);
    let overlay = overlays.pop().unwrap();
    assert_eq!(overlay.host, HOST);
    assert_eq!(overlay.anchor, Rect::from_xywh(0.0, 0.0, 7.0, 9.0));

    assert!(widget.overlays().is_empty());

    overlay
        .content
        .layout(&arena, Size::new(100.0, 50.0), overlay.anchor);
    assert_eq!(
        *recorded.borrow(),
        vec![(Size::new(100.0, 50.0), Rect::from_xywh(0.0, 0.0, 7.0, 9.0))]
    );
}

#[test]
fn placements_offset_anchors_through_nested_containers() {
    let arena = Arena::default();
    let mut inner: Container<'_, Cmd> = Container::new(&arena, Size::new(50.0, 50.0));
    inner.place(
        5.0,
        6.0,
        leaf::<Cmd>(7.0, 9.0).overlay(HOST, |_, _: Rect| Vec::new()),
    );

    let mut root: Container<'_, Cmd> = Container::new(&arena, Size::new(100.0, 100.0));
    root.place(10.0, 20.0, inner);

    let overlays = root.realize(&arena, viewport()).overlays();
    assert_eq!(overlays.len(), 1);

    assert_eq!(overlays[0].anchor, Rect::from_xywh(15.0, 26.0, 7.0, 9.0));
}

#[test]
fn unclaimed_requests_keep_bubbling_out_of_a_host() {
    let arena = Arena::default();
    let base = leaf::<Cmd>(40.0, 40.0)
        .overlay(OUTER, |_, _: Rect| Vec::new())
        .overlay_host(HOST);

    let mut root: Container<'_, Cmd> = Container::new(&arena, Size::new(100.0, 100.0));
    root.place(30.0, 40.0, base);

    let overlays = root.realize(&arena, viewport()).overlays();
    assert_eq!(overlays.len(), 1);
    assert_eq!(overlays[0].host, OUTER);
    assert_eq!(overlays[0].anchor, Rect::from_xywh(30.0, 40.0, 40.0, 40.0));
}

#[test]
fn a_host_with_no_requests_is_transparent() {
    let arena = Arena::default();
    let host = leaf::<Cmd>(80.0, 60.0)
        .event(|_, _, _| EventResult::Command(Cmd::Text("base")))
        .overlay_host(HOST);
    assert_eq!(Thunk::size(&host), Size::new(80.0, 60.0));
    let mut host = host.realize(&arena, viewport());
    assert_eq!(Widget::size(&host), Size::new(80.0, 60.0));
    assert!(host.overlays().is_empty());
    assert_eq!(
        command(host.handle_event(&arena, &Event::TextInput { text: "x" }, viewport())),
        Cmd::Text("base")
    );
}

#[test]
fn popups_stack_over_the_base_for_positional_events() {
    let arena = Arena::default();
    let base = {
        let mut root: Container<'_, Cmd> = Container::new(&arena, Size::new(100.0, 100.0));
        root.place(0.0, 0.0, crate::eager(HitBox::new("base", 100.0, 100.0)));
        root.place(
            10.0,
            10.0,
            leaf::<Cmd>(5.0, 5.0).overlay(HOST, |_, anchor: Rect| {
                one(
                    &arena,
                    anchor.left,
                    anchor.bottom,
                    crate::eager(HitBox::new("popup", 30.0, 30.0)),
                )
            }),
        );
        root
    };
    let host = base.overlay_host(HOST).realize(&arena, viewport());

    assert_eq!(
        command(host.handle_event(&arena, &press(25.0, 25.0), viewport())),
        Cmd::Hit("popup", Point::new(15.0, 10.0))
    );

    assert_eq!(
        command(host.handle_event(&arena, &press(80.0, 80.0), viewport())),
        Cmd::Hit("base", Point::new(80.0, 80.0))
    );
}

#[test]
fn focus_routed_events_try_popups_first_and_fall_through_on_ignored() {
    let arena = Arena::default();

    let host = crate::eager(HitBox::new("base", 100.0, 100.0).texty())
        .overlay(HOST, |_, _: Rect| {
            one(
                &arena,
                0.0,
                0.0,
                crate::eager(HitBox::new("popup", 10.0, 10.0).texty()),
            )
        })
        .overlay_host(HOST)
        .realize(&arena, viewport());
    assert_eq!(
        command(host.handle_event(&arena, &Event::TextInput { text: "x" }, viewport())),
        Cmd::Text("popup")
    );

    let host = crate::eager(HitBox::new("base", 100.0, 100.0).texty())
        .overlay(HOST, |_, _: Rect| {
            one(
                &arena,
                0.0,
                0.0,
                crate::eager(HitBox::new("hover-card", 10.0, 10.0)),
            )
        })
        .overlay_host(HOST)
        .realize(&arena, viewport());
    assert_eq!(
        command(host.handle_event(&arena, &Event::TextInput { text: "x" }, viewport())),
        Cmd::Text("base")
    );
}

#[test]
fn one_request_answers_a_batch_of_thunks() {
    let arena = Arena::default();

    let host = crate::eager(HitBox::new("base", 100.0, 100.0))
        .overlay(HOST, |_, _: Rect| {
            vec![
                (
                    Point::new(0.0, 0.0),
                    ThunkBox::new(&arena, crate::eager(HitBox::new("a", 10.0, 10.0))),
                ),
                (
                    Point::new(0.0, 40.0),
                    ThunkBox::new(&arena, crate::eager(HitBox::new("b", 10.0, 10.0))),
                ),
                (
                    Point::new(0.0, 80.0),
                    ThunkBox::new(&arena, crate::eager(HitBox::new("c", 10.0, 10.0))),
                ),
            ]
        })
        .overlay_host(HOST)
        .realize(&arena, viewport());

    for (name, y) in [("a", 5.0), ("b", 45.0), ("c", 85.0)] {
        assert_eq!(
            command(host.handle_event(&arena, &press(5.0, y), viewport())),
            Cmd::Hit(name, Point::new(5.0, 5.0))
        );
    }

    assert_eq!(
        command(host.handle_event(&arena, &press(5.0, 25.0), viewport())),
        Cmd::Hit("base", Point::new(5.0, 25.0))
    );
}

#[test]
fn same_key_recursion_chains_submenus_with_anchors_at_each_placement() {
    let arena = Arena::default();
    let arena = &arena;
    let recorded = RefCell::new(Vec::new());
    let rec = &recorded;

    let host = crate::eager(HitBox::new("base", 100.0, 100.0))
        .overlay(HOST, move |_, anchor: Rect| {
            rec.borrow_mut().push(("menu", anchor));
            one(
                &arena,
                20.0,
                0.0,
                crate::eager(HitBox::new("menu", 10.0, 10.0)).overlay(
                    HOST,
                    move |_, anchor: Rect| {
                        rec.borrow_mut().push(("submenu", anchor));
                        one(
                            &arena,
                            anchor.right,
                            anchor.top,
                            crate::eager(HitBox::new("submenu", 10.0, 10.0)).overlay(
                                HOST,
                                move |_, anchor: Rect| {
                                    rec.borrow_mut().push(("subsubmenu", anchor));
                                    one(
                                        &arena,
                                        anchor.right,
                                        anchor.top,
                                        crate::eager(HitBox::new("subsubmenu", 10.0, 10.0)),
                                    )
                                },
                            ),
                        )
                    },
                ),
            )
        })
        .overlay_host(HOST)
        .realize(&arena, viewport());

    assert_eq!(
        *recorded.borrow(),
        vec![
            ("menu", Rect::from_xywh(0.0, 0.0, 100.0, 100.0)),
            ("submenu", Rect::from_xywh(20.0, 0.0, 10.0, 10.0)),
            ("subsubmenu", Rect::from_xywh(30.0, 0.0, 10.0, 10.0)),
        ]
    );

    assert_eq!(
        command(host.handle_event(&arena, &press(45.0, 5.0), viewport())),
        Cmd::Hit("subsubmenu", Point::new(5.0, 5.0))
    );
}

#[test]
fn a_popup_can_ask_an_outer_host_for_more_room() {
    let arena = Arena::default();
    let arena = &arena;
    let recorded = RefCell::new(Vec::new());
    let rec = &recorded;

    let inner = crate::eager(HitBox::new("inner-base", 40.0, 40.0))
        .overlay(HOST, move |_, _: Rect| {
            one(
                &arena,
                8.0,
                4.0,
                leaf::<Cmd>(10.0, 10.0).overlay(OUTER, move |host_size, anchor: Rect| {
                    rec.borrow_mut().push((host_size, anchor));
                    Vec::new()
                }),
            )
        })
        .overlay_host(HOST);

    let mut root: Container<'_, Cmd> = Container::new(&arena, Size::new(200.0, 200.0));
    root.place(100.0, 50.0, inner);
    let _outer = root.overlay_host(OUTER).realize(&arena, viewport());

    assert_eq!(
        *recorded.borrow(),
        vec![(
            Size::new(200.0, 200.0),
            Rect::from_xywh(108.0, 54.0, 10.0, 10.0)
        )]
    );
}

#[test]
fn recursion_crosses_map_boundaries_with_the_origin_scope() {
    #[derive(Debug, PartialEq)]
    enum Outer {
        Wrapped(Cmd),
    }

    let arena = Arena::default();

    let host = crate::eager(HitBox::new("base", 100.0, 100.0))
        .overlay(HOST, |_, _: Rect| {
            one(
                &arena,
                10.0,
                10.0,
                crate::eager(HitBox::new("popup", 20.0, 20.0)).overlay(HOST, |_, anchor: Rect| {
                    one(
                        &arena,
                        anchor.right,
                        anchor.top,
                        crate::eager(HitBox::new("nested", 20.0, 20.0)),
                    )
                }),
            )
        })
        .map(Outer::Wrapped)
        .overlay_host(HOST)
        .realize(&arena, viewport());

    assert_eq!(
        command(host.handle_event(&arena, &press(15.0, 15.0), viewport())),
        Outer::Wrapped(Cmd::Hit("popup", Point::new(5.0, 5.0)))
    );
    assert_eq!(
        command(host.handle_event(&arena, &press(35.0, 15.0), viewport())),
        Outer::Wrapped(Cmd::Hit("nested", Point::new(5.0, 5.0)))
    );
    assert_eq!(
        command(host.handle_event(&arena, &press(80.0, 80.0), viewport())),
        Outer::Wrapped(Cmd::Hit("base", Point::new(80.0, 80.0)))
    );
}

#[test]
fn a_lazy_subtree_mints_requests_only_for_what_realizes() {
    struct LazyProducer;
    impl<'a> Thunk<'a, Cmd> for LazyProducer {
        fn size(&self) -> Size {
            Size::new(100.0, 1000.0)
        }

        fn realize(self, arena: &'a Arena, viewport: Rect) -> crate::WidgetBox<'a, Cmd> {
            let mut content: Container<'_, Cmd> = Container::new(arena, Size::new(100.0, 1000.0));
            let first = (viewport.top / 100.0).floor() as i32;
            let last = (viewport.bottom / 100.0).ceil() as i32;
            for row in first.max(0)..last.min(10) {
                content.place(
                    0.0,
                    row as f32 * 100.0,
                    leaf::<Cmd>(10.0, 10.0).overlay(HOST, |_, _: Rect| Vec::new()),
                );
            }
            content.realize(arena, viewport)
        }
    }

    let arena = Arena::default();

    let mut widget = LazyProducer.realize(&arena, Rect::from_xywh(0.0, 200.0, 100.0, 200.0));
    let overlays = widget.overlays();
    assert_eq!(overlays.len(), 2);
    assert_eq!(overlays[0].anchor, Rect::from_xywh(0.0, 200.0, 10.0, 10.0));
    assert_eq!(overlays[1].anchor, Rect::from_xywh(0.0, 300.0, 10.0, 10.0));

    let mut widget = LazyProducer.realize(&arena, Rect::default());
    assert!(widget.overlays().is_empty());
}

#[test]
fn scroll_carries_anchors_by_the_scroll_offset() {
    use crate::{
        constraints::Constraints, scroll::ScrollCommand, scroll::ScrollView, store::Store,
        ui::UiCtx, View,
    };

    struct Tall;
    impl View for Tall {
        type Command = Cmd;
        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &UiCtx,
            _command: Cmd,
            _fx: &mut crate::effect::Effects<'_, Cmd>,
        ) {
        }
        fn display<'a>(
            &'a self,
            arena: &'a Arena,
            _store: &'a Store,
            _ui: &'a UiCtx,
        ) -> impl crate::Layout<'a, Cmd> + crate::LayoutValue + 'a {
            crate::laid(move |_arena: &'a Arena, constraints: Constraints| {
                let mut root: Container<'_, Cmd> =
                    Container::new(arena, Size::new(constraints.max.width, 500.0));
                root.place(
                    0.0,
                    300.0,
                    leaf::<Cmd>(10.0, 10.0).overlay(HOST, |_, _: Rect| Vec::new()),
                );
                root
            })
        }
    }

    let arena = Arena::default();
    let store = Store::default();
    let ui = UiCtx::dont_use_too_slow();
    let mut view = ScrollView::new(Tall);
    {
        let mut batch = crate::effect::Batch::new();
        view.perform(
            &mut store.clone(),
            &ui,
            ScrollCommand::SetScrollY(120.0),
            &mut batch.effects(),
        );
    }
    let mut widget = crate::Layout::layout(
        view.display(&arena, &store, &ui),
        &arena,
        Constraints {
            min: Size::default(),
            max: Size::new(100.0, 200.0),
        },
    )
    .realize(&arena, Rect::from_wh(100.0, 200.0));

    let overlays = widget.overlays();
    assert_eq!(overlays.len(), 1);

    assert_eq!(overlays[0].anchor, Rect::from_xywh(0.0, 180.0, 10.0, 10.0));
}
