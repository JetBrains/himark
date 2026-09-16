// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    event::{Event, EventResult, MouseButton},
    lazy::lazy,
    list::{ListCommand, ListView},
    scroll::{ScrollCommand, ScrollView},
    split::{Pane, SplitCommand, SplitView},
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, View, Widget,
};
use skia_safe::{surfaces, Point, Rect, Size};
use std::cell::{Cell, RefCell};

fn perform_into<V: View>(
    view: &mut V,
    store: &mut Store,
    ui: &crate::ui::UiCtx,
    command: V::Command,
) where
    V::Command: Send + 'static,
{
    let mut batch = crate::effect::Batch::new();
    view.perform(store, ui, command, &mut batch.effects());
}

#[derive(Debug, PartialEq)]
enum Command {
    Hit(&'static str, Point),

    Probe(&'static str, bool),
    Fallback,
    Viewport { top: f32, bottom: f32 },
}

struct HitBox {
    name: &'static str,
    size: Size,
}

impl HitBox {
    fn new(name: &'static str, width: f32, height: f32) -> Self {
        Self {
            name,
            size: Size::new(width, height),
        }
    }
}

impl Widget<'_, Command> for HitBox {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<Command> {
        match event {
            Event::MouseDown {
                mods: _,
                point,
                button: MouseButton::Left,
                ..
            } => EventResult::Command(Command::Hit(self.name, *point)),
            Event::HitTest { point, miss } => {
                use skia_safe::Contains;
                let inside = !miss && Rect::from_size(self.size).contains(*point);
                EventResult::Command(Command::Probe(self.name, inside))
            }
            _ => EventResult::Ignored,
        }
    }
}

#[test]
fn container_routes_mouse_to_topmost_child_in_local_coordinates() {
    let arena = Arena::default();
    let mut widget = container(&arena, Size::new(100.0, 100.0));
    widget.place(0.0, 0.0, crate::eager(HitBox::new("bottom", 80.0, 80.0)));
    widget.place(10.0, 12.0, crate::eager(HitBox::new("top", 40.0, 40.0)));
    let widget = widget
        .event(|_, _, _| EventResult::Command(Command::Fallback))
        .realize(&arena, root_viewport());

    let event = Event::MouseDown {
        mods: Default::default(),
        point: Point::new(18.0, 22.0),
        button: MouseButton::Left,
        count: 1,
    };

    let result = widget.handle_event(&arena, &event, root_viewport());

    assert_eq!(
        result_command(result),
        Command::Hit("top", Point::new(8.0, 10.0))
    );
    assert_event_point(event, Point::new(18.0, 22.0));
}

#[test]
fn container_falls_back_when_no_child_handles_event() {
    let arena = Arena::default();
    let mut widget = container(&arena, Size::new(100.0, 100.0));
    widget.place(10.0, 10.0, crate::eager(HitBox::new("child", 20.0, 20.0)));
    let widget = widget
        .event(|_, _, _| EventResult::Command(Command::Fallback))
        .realize(&arena, root_viewport());

    let event = Event::MouseDown {
        mods: Default::default(),
        point: Point::new(90.0, 90.0),
        button: MouseButton::Left,
        count: 1,
    };

    assert_eq!(
        result_command(widget.handle_event(&arena, &event, root_viewport())),
        Command::Fallback
    );
    assert_event_point(event, Point::new(90.0, 90.0));
}

#[test]
fn hit_test_hits_the_topmost_child_and_misses_the_covered_one() {
    let arena = Arena::default();
    let mut widget = container(&arena, Size::new(100.0, 100.0));
    widget.place(0.0, 0.0, crate::eager(HitBox::new("bottom", 80.0, 80.0)));
    widget.place(10.0, 12.0, crate::eager(HitBox::new("top", 40.0, 40.0)));
    let widget = widget.realize(&arena, root_viewport());

    let result = widget.handle_event(
        &arena,
        &Event::HitTest {
            point: Point::new(18.0, 22.0),
            miss: false,
        },
        root_viewport(),
    );
    assert_eq!(
        result_commands(result),
        vec![Command::Probe("bottom", false), Command::Probe("top", true)]
    );

    let result = widget.handle_event(
        &arena,
        &Event::HitTest {
            point: Point::new(18.0, 22.0),
            miss: true,
        },
        root_viewport(),
    );
    assert_eq!(
        result_commands(result),
        vec![
            Command::Probe("bottom", false),
            Command::Probe("top", false)
        ]
    );

    let result = widget.handle_event(
        &arena,
        &Event::HitTest {
            point: Point::new(95.0, 95.0),
            miss: false,
        },
        root_viewport(),
    );
    assert_eq!(
        result_commands(result),
        vec![
            Command::Probe("bottom", false),
            Command::Probe("top", false)
        ]
    );
}

fn result_commands(result: EventResult<Command>) -> Vec<Command> {
    match result {
        EventResult::Commands(commands) => commands,
        EventResult::Command(command) => vec![command],
        EventResult::Ignored => panic!("expected commands, got Ignored"),
        EventResult::Handled => panic!("expected commands, got Handled"),
        EventResult::Reveal(rect) => panic!("expected commands, got Reveal({rect:?})"),
    }
}

#[derive(Debug, PartialEq)]
enum ContentCommand {
    Click(Point),
}

struct FixedContent {
    size: Size,
}

struct FixedWidget {
    size: Size,
}

impl View for FixedContent {
    type Command = ContentCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &crate::ui::UiCtx,
        _command: Self::Command,
        _fx: &mut crate::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        _store: &'a Store,
        _ui: &'a crate::ui::UiCtx,
    ) -> impl crate::Layout<'a, Self::Command> + crate::LayoutValue + 'a {
        crate::laid(move |_arena: &'a Arena, _constraints: Constraints| {
            crate::eager(FixedWidget { size: self.size })
        })
    }
}

impl Widget<'_, ContentCommand> for FixedWidget {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<ContentCommand> {
        match event {
            Event::MouseDown {
                mods: _,
                point,
                button: MouseButton::Left,
                ..
            } => EventResult::Command(ContentCommand::Click(*point)),
            _ => EventResult::Ignored,
        }
    }
}

#[test]
fn scroll_view_clamps_wheel_commands_to_content_bounds() {
    let view = ScrollView::new(FixedContent {
        size: Size::new(100.0, 500.0),
    });
    let arena = Arena::default();
    let store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let widget = crate::Layout::layout(
        view.display(&arena, &store, &ui),
        &arena,
        Constraints::tight(Size::new(100.0, 120.0)),
    )
    .realize(&arena, Rect::from_wh(100.0, 120.0));

    let gesture = crate::event::ScrollGesture::default();

    let event = Event::Scroll {
        delta_x: 0.0,
        point: Point::new(50.0, 60.0),
        delta_y: 1_000.0,
        gesture: &gesture,
    };

    match widget.handle_event(&arena, &event, Rect::from_wh(100.0, 120.0)) {
        EventResult::Command(ScrollCommand::SetScrollY(scroll_y)) => {
            assert_eq!(scroll_y, 380.0);
        }
        _ => panic!("scroll should produce a clamped SetScrollY command"),
    }
}

#[test]
fn a_partially_scrolled_painter_keeps_its_true_box() {
    use std::cell::Cell;
    let seen: std::rc::Rc<Cell<Option<Rect>>> = std::rc::Rc::default();
    let arena = Arena::default();
    let thunk = crate::leaf::leaf::<u32>(100.0, 40.0).paint_instead({
        let seen = std::rc::Rc::clone(&seen);
        move |_arena, _canvas, rect| seen.set(Some(rect))
    });
    let widget = crate::ThunkBox::new(&arena, thunk).realize(&arena, Rect::from_wh(100.0, 40.0));
    let mut surface = skia_safe::surfaces::raster_n32_premul((100, 40)).expect("surface");

    let band = Rect::from_ltrb(0.0, 25.0, 100.0, 40.0);
    let _ = widget.handle_event(
        &arena,
        &Event::Paint {
            canvas: surface.canvas(),
            focused: true,
        },
        band,
    );
    assert_eq!(
        seen.get(),
        Some(Rect::from_wh(100.0, 40.0)),
        "the painter's rect is the widget box, whatever the band"
    );
}

struct PanContent {
    surface: crate::event::ScrollSurfaceId,
}
struct PanWidget {
    surface: crate::event::ScrollSurfaceId,
    height: f32,
}
impl View for PanContent {
    type Command = f32;
    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &crate::ui::UiCtx,
        _command: f32,
        _fx: &mut crate::effect::Effects<'_, f32>,
    ) {
    }
    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        _store: &'a Store,
        _ui: &'a crate::ui::UiCtx,
    ) -> impl crate::Layout<'a, f32> + crate::LayoutValue + 'a {
        crate::laid(move |_arena: &'a Arena, _constraints: Constraints| {
            crate::eager(PanWidget {
                surface: self.surface,
                height: 500.0,
            })
        })
    }
}
impl Widget<'_, f32> for PanWidget {
    fn size(&self) -> Size {
        Size::new(100.0, self.height)
    }
    fn handle_event(&self, _arena: &Arena, event: &Event<'_>, _viewport: Rect) -> EventResult<f32> {
        match event {
            Event::Scroll {
                delta_x,
                delta_y,
                gesture,
                ..
            } => {
                if gesture.owned_by(self.surface) {
                    return EventResult::Command(*delta_x);
                }
                if gesture.owned_by_other(self.surface) || delta_x.abs() <= delta_y.abs() {
                    return EventResult::Ignored;
                }
                let _ = gesture.claims(self.surface);
                EventResult::Command(*delta_x)
            }
            _ => EventResult::Ignored,
        }
    }
}

fn scrolled(
    view: &ScrollView<PanContent>,
    gesture: &crate::event::ScrollGesture,
    delta_x: f32,
    delta_y: f32,
) -> EventResult<ScrollCommand<f32>> {
    let arena = Arena::default();
    let store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let widget = crate::Layout::layout(
        view.display(&arena, &store, &ui),
        &arena,
        Constraints::tight(Size::new(100.0, 120.0)),
    )
    .realize(&arena, Rect::from_wh(100.0, 120.0));
    let event = Event::Scroll {
        point: Point::new(50.0, 60.0),
        delta_x,
        delta_y,
        gesture,
    };
    widget.handle_event(&arena, &event, Rect::from_wh(100.0, 120.0))
}

#[test]
fn an_exhausted_owner_eats_the_gesture() {
    let mut view = ScrollView::new(PanContent {
        surface: crate::event::ScrollSurfaceId::mint(),
    });
    let gesture = crate::event::ScrollGesture::default();
    match scrolled(&view, &gesture, 0.0, 1_000.0) {
        EventResult::Command(ScrollCommand::SetScrollY(y)) => assert_eq!(y, 380.0),
        _ => panic!("the view claims and scrolls"),
    }
    view.set_scroll_y(380.0);

    match scrolled(&view, &gesture, 0.0, 100.0) {
        EventResult::Handled => {}
        _ => panic!("an exhausted owner eats, never hands off"),
    }

    let fresh = crate::event::ScrollGesture::default();
    match scrolled(&view, &fresh, 0.0, 100.0) {
        EventResult::Handled => {}
        _ => panic!("re-captured and eaten"),
    }
}

#[test]
fn a_claiming_content_owns_the_whole_gesture() {
    let view = ScrollView::new(PanContent {
        surface: crate::event::ScrollSurfaceId::mint(),
    });
    let gesture = crate::event::ScrollGesture::default();
    match scrolled(&view, &gesture, 24.0, 4.0) {
        EventResult::Command(ScrollCommand::Content(delta)) => assert_eq!(delta, 24.0),
        _ => panic!("the x-dominant start belongs to the pan content"),
    }

    match scrolled(&view, &gesture, 2.0, 60.0) {
        EventResult::Command(ScrollCommand::Content(_)) => {}
        _ => panic!("the owner keeps the gesture"),
    }

    let fresh = crate::event::ScrollGesture::default();
    match scrolled(&view, &fresh, 0.0, 40.0) {
        EventResult::Command(ScrollCommand::SetScrollY(y)) => assert_eq!(y, 40.0),
        _ => panic!("a declined gesture is the view's"),
    }
}

#[test]
fn a_fitted_view_never_claims() {
    struct Short;
    impl View for Short {
        type Command = f32;
        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &crate::ui::UiCtx,
            _command: f32,
            _fx: &mut crate::effect::Effects<'_, f32>,
        ) {
        }
        fn display<'a>(
            &'a self,
            _arena: &'a Arena,
            _store: &'a Store,
            _ui: &'a crate::ui::UiCtx,
        ) -> impl crate::Layout<'a, f32> + crate::LayoutValue + 'a {
            crate::laid(move |_arena: &'a Arena, _constraints: Constraints| {
                crate::leaf::leaf(100.0, 50.0)
            })
        }
    }
    let view = ScrollView::new(Short);
    let arena = Arena::default();
    let store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let widget = crate::Layout::layout(
        view.display(&arena, &store, &ui),
        &arena,
        Constraints::tight(Size::new(100.0, 120.0)),
    )
    .realize(&arena, Rect::from_wh(100.0, 120.0));
    let gesture = crate::event::ScrollGesture::default();
    let event = Event::Scroll {
        point: Point::new(50.0, 60.0),
        delta_x: 0.0,
        delta_y: 40.0,
        gesture: &gesture,
    };
    match widget.handle_event(&arena, &event, Rect::from_wh(100.0, 120.0)) {
        EventResult::Ignored => {}
        _ => panic!("nothing to scroll, nothing claimed"),
    }
    assert!(
        !gesture.owned_by_other(crate::event::ScrollSurfaceId::mint()),
        "the gesture stays unowned for an ancestor to claim"
    );
}

#[test]
fn scroll_view_translates_mouse_coordinates_into_scrolled_content_space() {
    let mut view = ScrollView::new(FixedContent {
        size: Size::new(100.0, 500.0),
    });
    let mut store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    perform_into(&mut view, &mut store, &ui, ScrollCommand::SetScrollY(75.0));
    let arena = Arena::default();
    let widget = crate::Layout::layout(
        view.display(&arena, &store, &ui),
        &arena,
        Constraints::tight(Size::new(100.0, 120.0)),
    )
    .realize(&arena, Rect::from_wh(100.0, 120.0));

    let event = Event::MouseDown {
        mods: Default::default(),
        point: Point::new(20.0, 10.0),
        button: MouseButton::Left,
        count: 1,
    };

    match widget.handle_event(&arena, &event, Rect::from_wh(100.0, 120.0)) {
        EventResult::Command(ScrollCommand::Content(ContentCommand::Click(point))) => {
            assert_eq!(point, Point::new(20.0, 85.0));
        }
        _ => panic!("mouse down should be delivered to content"),
    }
    assert_event_point(event, Point::new(20.0, 10.0));
}

#[test]
fn paint_modifiers_control_paint_order() {
    let arena = Arena::default();
    let log = RefCell::new(Vec::new());
    let mut surface = surfaces::raster_n32_premul((8, 8)).expect("raster surface");
    let event = Event::Paint {
        focused: true,
        canvas: surface.canvas(),
    };
    let widget = crate::eager(PaintTrace { log: &log })
        .paint_below(|_, _, _| log.borrow_mut().push("below"))
        .paint_above(|_, _, _| log.borrow_mut().push("above"))
        .realize(&arena, Rect::from_wh(8.0, 8.0));

    widget.handle_event(&arena, &event, Rect::from_wh(8.0, 8.0));

    assert_eq!(log.borrow().as_slice(), ["below", "inner", "above"]);
}

#[test]
fn paint_instead_replaces_inner_paint() {
    let arena = Arena::default();
    let log = RefCell::new(Vec::new());
    let mut surface = surfaces::raster_n32_premul((8, 8)).expect("raster surface");
    let event = Event::Paint {
        focused: true,
        canvas: surface.canvas(),
    };
    let widget = crate::eager(PaintTrace { log: &log })
        .paint_instead(|_, _, _| {
            log.borrow_mut().push("instead");
        })
        .realize(&arena, Rect::from_wh(8.0, 8.0));

    widget.handle_event(&arena, &event, Rect::from_wh(8.0, 8.0));

    assert_eq!(log.borrow().as_slice(), ["instead"]);
}

#[test]
fn lazy_defers_layout_until_event_viewport_is_known() {
    let arena = Arena::default();
    let layout_calls = Cell::new(0);
    let widget = lazy(Size::new(40.0, 80.0), |viewport| {
        layout_calls.set(layout_calls.get() + 1);
        crate::eager(ViewportTrace { viewport })
    });

    assert_eq!(Thunk::size(&widget), Size::new(40.0, 80.0));
    assert_eq!(layout_calls.get(), 0);

    let event = Event::MouseDown {
        mods: Default::default(),
        point: Point::new(3.0, 4.0),
        button: MouseButton::Left,
        count: 1,
    };

    let widget = widget.realize(&arena, Rect::from_xywh(0.0, 17.0, 40.0, 25.0));
    assert_eq!(layout_calls.get(), 1);
    let result = widget.handle_event(&arena, &event, Rect::from_xywh(0.0, 17.0, 40.0, 25.0));

    assert_eq!(layout_calls.get(), 1);
    assert_eq!(
        result_command(result),
        Command::Viewport {
            top: 17.0,
            bottom: 42.0,
        }
    );
}

struct PaintTrace<'a> {
    log: &'a RefCell<Vec<&'static str>>,
}

impl Widget<'_, ()> for PaintTrace<'_> {
    fn size(&self) -> Size {
        Size::new(8.0, 8.0)
    }

    fn handle_event(&self, _arena: &Arena, event: &Event<'_>, _viewport: Rect) -> EventResult<()> {
        match event {
            Event::Paint { .. } => {
                self.log.borrow_mut().push("inner");
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

struct ViewportTrace {
    viewport: Rect,
}

impl Widget<'_, Command> for ViewportTrace {
    fn size(&self) -> Size {
        self.viewport.size()
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<Command> {
        match event {
            Event::MouseDown { .. } => EventResult::Command(Command::Viewport {
                top: self.viewport.top,
                bottom: self.viewport.bottom,
            }),
            _ => EventResult::Ignored,
        }
    }
}

fn result_command(result: EventResult<Command>) -> Command {
    match result {
        EventResult::Command(command) => command,
        EventResult::Handled => panic!("expected command, got handled"),
        EventResult::Ignored => panic!("expected command, got ignored"),
        EventResult::Commands(_) => panic!("expected one command, got a batch"),
        EventResult::Reveal(_) => panic!("expected command, got a reveal"),
    }
}

fn assert_event_point(event: Event<'_>, expected: Point) {
    match event {
        Event::MouseDown { point, .. } => assert_eq!(point, expected),
        _ => panic!("expected mouse event"),
    }
}

fn root_viewport() -> Rect {
    Rect::from_wh(100.0, 100.0)
}

#[test]
fn boxed_dyn_view_routes_commands_back_to_the_typed_view() {
    use crate::{leaf::leaf, DynView};
    use std::rc::Rc;

    #[derive(Clone)]
    struct Counter {
        count: Rc<Cell<u32>>,
    }

    enum CounterCommand {
        Add(u32),
    }

    impl View for Counter {
        type Command = CounterCommand;

        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &crate::ui::UiCtx,
            command: Self::Command,
            _fx: &mut crate::effect::Effects<'_, Self::Command>,
        ) {
            match command {
                CounterCommand::Add(amount) => self.count.set(self.count.get() + amount),
            }
        }

        fn display<'a>(
            &'a self,
            _arena: &'a Arena,
            _store: &'a Store,
            _ui: &'a crate::ui::UiCtx,
        ) -> impl crate::Layout<'a, Self::Command> + crate::LayoutValue + 'a {
            crate::laid(move |_arena: &'a Arena, _constraints: Constraints| {
                leaf(10.0, 10.0).event(|_, event, _| match event {
                    Event::MouseDown { .. } => EventResult::Command(CounterCommand::Add(2)),
                    _ => EventResult::Ignored,
                })
            })
        }
    }

    let count = Rc::new(Cell::new(0));
    let mut view: Box<dyn DynView> = Box::new(Counter {
        count: Rc::clone(&count),
    });

    let arena = Arena::default();
    let mut store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let command = {
        let widget = crate::Layout::layout(
            view.display(&arena, &store, &ui),
            &arena,
            Constraints::tight(Size::new(10.0, 10.0)),
        )
        .realize(&arena, Rect::from_wh(10.0, 10.0));
        let event = Event::MouseDown {
            mods: Default::default(),
            point: Point::new(5.0, 5.0),
            button: MouseButton::Left,
            count: 1,
        };
        match widget.handle_event(&arena, &event, Rect::from_wh(10.0, 10.0)) {
            EventResult::Command(command) => command,
            _ => panic!("expected a command"),
        }
    };

    perform_into(&mut view, &mut store, &ui, command);

    assert_eq!(count.get(), 2);
}

#[test]
fn split_view_routes_clicks_to_the_pane_under_the_point() {
    let view = SplitView::row(
        FixedContent {
            size: Size::new(100.0, 100.0),
        },
        FixedContent {
            size: Size::new(100.0, 100.0),
        },
    );
    let arena = Arena::default();
    let store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let widget = crate::Layout::layout(
        view.display(&arena, &store, &ui),
        &arena,
        Constraints::tight(Size::new(200.0, 100.0)),
    )
    .realize(&arena, Rect::from_wh(200.0, 100.0));

    let event = Event::MouseDown {
        mods: Default::default(),
        point: Point::new(30.0, 40.0),
        button: MouseButton::Left,
        count: 1,
    };
    match widget.handle_event(&arena, &event, Rect::from_wh(200.0, 100.0)) {
        EventResult::Command(SplitCommand::Focus(Pane::First, Some(command))) => match *command {
            SplitCommand::First(ContentCommand::Click(point)) => {
                assert_eq!(point, Point::new(30.0, 40.0));
            }
            _ => panic!("the click's command should target the first pane"),
        },
        _ => panic!("a click left of the divider should reach the first pane"),
    }

    let event = Event::MouseDown {
        mods: Default::default(),
        point: Point::new(130.0, 40.0),
        button: MouseButton::Left,
        count: 1,
    };
    match widget.handle_event(&arena, &event, Rect::from_wh(200.0, 100.0)) {
        EventResult::Command(SplitCommand::Focus(Pane::Second, Some(command))) => match *command {
            SplitCommand::Second(ContentCommand::Click(point)) => {
                assert_eq!(
                    point,
                    Point::new(30.0, 40.0),
                    "the second pane sees local coordinates"
                );
            }
            _ => panic!("the click's command should target the second pane"),
        },
        _ => panic!("a click right of the divider should reach the second pane"),
    }
}

#[test]
fn split_view_divides_the_main_axis_by_ratio() {
    let view = SplitView::column(
        FixedContent {
            size: Size::new(100.0, 100.0),
        },
        FixedContent {
            size: Size::new(100.0, 100.0),
        },
    )
    .with_ratio(0.25);
    let arena = Arena::default();
    let store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let widget = crate::Layout::layout(
        view.display(&arena, &store, &ui),
        &arena,
        Constraints::tight(Size::new(100.0, 200.0)),
    )
    .realize(&arena, Rect::from_wh(100.0, 200.0));

    let event = Event::MouseDown {
        mods: Default::default(),
        point: Point::new(10.0, 49.0),
        button: MouseButton::Left,
        count: 1,
    };
    assert!(matches!(
        widget.handle_event(&arena, &event, Rect::from_wh(100.0, 200.0)),
        EventResult::Command(SplitCommand::Focus(Pane::First, Some(_)))
    ));

    let event = Event::MouseDown {
        mods: Default::default(),
        point: Point::new(10.0, 51.0),
        button: MouseButton::Left,
        count: 1,
    };
    match widget.handle_event(&arena, &event, Rect::from_wh(100.0, 200.0)) {
        EventResult::Command(SplitCommand::Focus(Pane::Second, Some(command))) => match *command {
            SplitCommand::Second(ContentCommand::Click(point)) => {
                assert_eq!(point, Point::new(10.0, 1.0));
            }
            _ => panic!("the click's command should target the second pane"),
        },
        _ => panic!("a click below the divider should reach the second pane"),
    }
}

#[test]
fn split_view_perform_routes_to_the_addressed_pane() {
    let mut view = SplitView::row(
        ScrollView::new(FixedContent {
            size: Size::new(100.0, 500.0),
        }),
        ScrollView::new(FixedContent {
            size: Size::new(100.0, 500.0),
        }),
    );
    let mut store = Store::new();
    let ui = crate::ui::UiCtx::cold();

    perform_into(
        &mut view,
        &mut store,
        &ui,
        SplitCommand::Second(ScrollCommand::SetScrollY(75.0)),
    );

    assert_eq!(view.first().scroll_y(), 0.0);
    assert_eq!(view.second().scroll_y(), 75.0);
}

#[test]
fn split_view_scrolls_the_pane_under_the_pointer_and_keys_follow_focus() {
    let mut view = SplitView::row(
        ScrollView::new(FixedContent {
            size: Size::new(100.0, 500.0),
        }),
        ScrollView::new(FixedContent {
            size: Size::new(100.0, 500.0),
        }),
    );
    let mut store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let arena = Arena::default();

    {
        let widget = crate::Layout::layout(
            view.display(&arena, &store, &ui),
            &arena,
            Constraints::tight(Size::new(200.0, 120.0)),
        )
        .realize(&arena, Rect::from_wh(200.0, 120.0));
        let gesture = crate::event::ScrollGesture::default();
        let event = Event::Scroll {
            delta_x: 0.0,
            point: Point::new(150.0, 60.0),
            delta_y: 50.0,
            gesture: &gesture,
        };
        assert!(matches!(
            widget.handle_event(&arena, &event, Rect::from_wh(200.0, 120.0)),
            EventResult::Command(SplitCommand::Second(ScrollCommand::SetScrollY(_)))
        ));
    }

    assert_eq!(view.focused(), Pane::First);

    let command = {
        let widget = crate::Layout::layout(
            view.display(&arena, &store, &ui),
            &arena,
            Constraints::tight(Size::new(200.0, 120.0)),
        )
        .realize(&arena, Rect::from_wh(200.0, 120.0));
        let event = Event::MouseDown {
            mods: Default::default(),
            point: Point::new(150.0, 60.0),
            button: MouseButton::Left,
            count: 1,
        };
        match widget.handle_event(&arena, &event, Rect::from_wh(200.0, 120.0)) {
            EventResult::Command(command) => command,
            _ => panic!("the click should produce a command"),
        }
    };
    assert!(matches!(
        command,
        SplitCommand::Focus(Pane::Second, Some(_))
    ));
    perform_into(&mut view, &mut store, &ui, command);
    assert_eq!(view.focused(), Pane::Second);

    {
        let widget = crate::Layout::layout(
            view.display(&arena, &store, &ui),
            &arena,
            Constraints::tight(Size::new(200.0, 120.0)),
        )
        .realize(&arena, Rect::from_wh(200.0, 120.0));
        let event = Event::TextInput { text: "x" };
        assert!(matches!(
            widget.handle_event(&arena, &event, Rect::from_wh(200.0, 120.0)),
            EventResult::Ignored
        ));
    }
}

#[derive(Debug, PartialEq)]
enum RowCommand {
    Click(Point),
    Text(&'static str),
}

#[derive(Clone)]
struct Row {
    height: f32,
}

struct RowWidget {
    height: f32,
}

impl View for Row {
    type Command = RowCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &crate::ui::UiCtx,
        _command: Self::Command,
        _fx: &mut crate::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        _store: &'a Store,
        _ui: &'a crate::ui::UiCtx,
    ) -> impl crate::Layout<'a, Self::Command> + crate::LayoutValue + 'a {
        crate::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let _ = constraints;
            crate::eager(RowWidget {
                height: self.height,
            })
            .commands(|| {
                vec![crate::PresentableCommand::new(
                    "row.poke",
                    "Poke Row",
                    RowCommand::Text("poked"),
                )]
            })
        })
    }
}

impl Widget<'_, RowCommand> for RowWidget {
    fn size(&self) -> Size {
        Size::new(100.0, self.height)
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<RowCommand> {
        match event {
            Event::MouseDown {
                mods: _,
                point,
                button: MouseButton::Left,
                ..
            } => EventResult::Command(RowCommand::Click(*point)),
            Event::TextInput { .. } => EventResult::Command(RowCommand::Text("typed")),
            _ => EventResult::Ignored,
        }
    }
}

fn row_list() -> ListView<Row> {
    ListView::from_rope(crate::list::measured([
        (Row { height: 10.0 }, 10.0),
        (Row { height: 20.0 }, 20.0),
        (Row { height: 30.0 }, 30.0),
    ]))
}

#[test]
fn an_absurd_row_height_cannot_wrap_the_lists_offsets() {
    // The Fill-under-unbounded class: a row measuring f32::MAX
    // saturates the u32 row metric, and summing two such rows WRAPS
    // the cumulative offsets in release (debug: add-overflow panic)
    // — every later row painting on top of the others. The list's
    // measure clamps instead.
    let list: ListView<Row> = ListView::from_rope(crate::list::measured([
        (Row { height: 10.0 }, 10.0),
        (Row { height: 20.0 }, f32::MAX),
        (Row { height: 30.0 }, 30.0),
        (Row { height: 40.0 }, f32::INFINITY),
    ]));
    let total = list.total_height();
    assert!(
        total.is_finite() && (2_000_040.0..=2_000_100.0).contains(&total),
        "both absurd rows clamp, the real rows still count: {total}"
    );
}

#[test]
fn list_stacks_rows_and_a_click_focuses_the_hit_row() {
    let mut store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let arena = Arena::default();
    let mut list = row_list();

    let command = {
        let widget = crate::Layout::layout(
            list.display(&arena, &store, &ui),
            &arena,
            Constraints::tight(Size::new(100.0, 60.0)),
        )
        .realize(&arena, root_viewport());
        assert_eq!(widget.size(), Size::new(100.0, 60.0), "heights stack");

        let event = Event::MouseDown {
            mods: Default::default(),
            point: Point::new(50.0, 15.0),
            button: MouseButton::Left,
            count: 1,
        };
        match widget.handle_event(&arena, &event, root_viewport()) {
            EventResult::Command(command) => command,
            _ => panic!("the click should produce a command"),
        }
    };

    let ListCommand::Focus(1, Some(inner)) = command else {
        panic!("expected Focus(1, Some(click))");
    };
    let ListCommand::Child(1, RowCommand::Click(point)) = *inner else {
        panic!("expected the click routed to row 1");
    };
    assert_eq!(point, Point::new(50.0, 5.0));

    perform_into(&mut list, &mut store, &ui, ListCommand::Focus(1, None));
    assert_eq!(list.focused(), Some(1));

    assert_eq!(list.index_at_y(5.0), Some(0));
    assert_eq!(list.index_at_y(15.0), Some(1));
    assert_eq!(list.index_at_y(45.0), Some(2));
    assert_eq!(list.total_height(), 60.0);
}

#[test]
fn list_routes_position_less_events_to_the_focused_row() {
    let mut store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let arena = Arena::default();
    let mut list = row_list();

    {
        let widget = crate::Layout::layout(
            list.display(&arena, &store, &ui),
            &arena,
            Constraints::tight(Size::new(100.0, 60.0)),
        )
        .realize(&arena, root_viewport());
        let event = Event::TextInput { text: "x" };
        assert!(matches!(
            widget.handle_event(&arena, &event, root_viewport()),
            EventResult::Ignored
        ));
    }

    perform_into(&mut list, &mut store, &ui, ListCommand::Focus(2, None));

    let widget = crate::Layout::layout(
        list.display(&arena, &store, &ui),
        &arena,
        Constraints::tight(Size::new(100.0, 60.0)),
    )
    .realize(&arena, root_viewport());
    let event = Event::TextInput { text: "x" };
    let result = widget.handle_event(&arena, &event, root_viewport());
    assert!(
        matches!(
            result,
            EventResult::Command(ListCommand::Child(2, RowCommand::Text("typed")))
        ),
        "text goes to the focused row, wrapped with its index"
    );
}

#[test]
fn list_drops_commands_addressed_to_missing_rows() {
    let mut store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let mut list = row_list();
    let mut batch = crate::effect::Batch::new();
    list.perform(
        &mut store,
        &ui,
        ListCommand::Child(9, RowCommand::Text("stale")),
        &mut batch.effects(),
    );
    assert!(batch.is_empty(), "a stale index is dropped, not a panic");
}

struct WidthReporter {
    name: &'static str,
    granted: f32,
    stored: f32,
}

impl Widget<'_, Command> for WidthReporter {
    fn size(&self) -> Size {
        Size::new(self.stored, 10.0)
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<Command> {
        match event {
            Event::Paint { .. } if (self.granted - self.stored).abs() > 1.0 => {
                EventResult::Command(Command::Hit(self.name, Point::new(self.granted, 0.0)))
            }
            _ => EventResult::Ignored,
        }
    }
}

#[test]
fn paint_broadcasts_and_merges_every_mismatch() {
    let mut surface = skia_safe::surfaces::raster_n32_premul((100, 100)).expect("surface");
    let arena = Arena::default();
    let mut widget = container(&arena, Size::new(100.0, 100.0));
    widget.place(
        0.0,
        0.0,
        crate::eager(WidthReporter {
            name: "stale",
            granted: 80.0,
            stored: 50.0,
        }),
    );
    widget.place(
        0.0,
        20.0,
        crate::eager(WidthReporter {
            name: "settled",
            granted: 80.0,
            stored: 80.0,
        }),
    );
    widget.place(
        0.0,
        40.0,
        crate::eager(WidthReporter {
            name: "also stale",
            granted: 80.0,
            stored: 30.0,
        }),
    );

    let canvas = surface.canvas();
    let result = widget.realize(&arena, root_viewport()).handle_event(
        &arena,
        &Event::Paint {
            canvas,
            focused: true,
        },
        root_viewport(),
    );
    let EventResult::Commands(commands) = result else {
        panic!("both mismatches must be reported");
    };
    assert_eq!(commands.len(), 2, "the settled child stays silent");
}

#[test]
fn list_paint_reconciles_stale_row_heights() {
    let mut surface = skia_safe::surfaces::raster_n32_premul((100, 60)).expect("surface");
    let mut store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let arena = Arena::default();

    let mut list: ListView<Row> = ListView::from_rope(crate::list::measured([
        (Row { height: 30.0 }, 10.0),
        (Row { height: 20.0 }, 20.0),
    ]));

    let commands = {
        let widget = crate::Layout::layout(
            list.display(&arena, &store, &ui),
            &arena,
            Constraints::tight(Size::new(100.0, 60.0)),
        )
        .realize(&arena, root_viewport());
        let canvas = surface.canvas();
        match widget.handle_event(
            &arena,
            &Event::Paint {
                canvas,
                focused: true,
            },
            root_viewport(),
        ) {
            EventResult::Commands(commands) => commands,
            EventResult::Command(command) => vec![command],
            _ => panic!("the stale row must be reported"),
        }
    };
    assert_eq!(commands.len(), 1, "only the stale row reports");
    for command in commands {
        perform_into(&mut list, &mut store, &ui, command);
    }
    assert_eq!(list.total_height(), 50.0, "the fresh height is spliced in");

    let widget = crate::Layout::layout(
        list.display(&arena, &store, &ui),
        &arena,
        Constraints::tight(Size::new(100.0, 60.0)),
    )
    .realize(&arena, root_viewport());
    let canvas = surface.canvas();
    assert!(matches!(
        widget.handle_event(
            &arena,
            &Event::Paint {
                canvas,
                focused: true
            },
            root_viewport()
        ),
        EventResult::Ignored | EventResult::Handled
    ));
}

#[test]
fn list_scroll_benchmark() {
    use std::time::Duration;
    const ROWS: usize = 100_000;
    const ROW_HEIGHT: f32 = 24.0;
    const VIEWPORT: Size = Size {
        width: 400.0,
        height: 900.0,
    };

    let mut store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let built_started = std::time::Instant::now();
    let mut view = ScrollView::new(ListView::from_rope(crate::list::measured(
        (0..ROWS).map(|_| (Row { height: ROW_HEIGHT }, ROW_HEIGHT)),
    )));
    let built = built_started.elapsed();

    let mut surface = skia_safe::surfaces::raster_n32_premul((400, 900)).expect("surface");
    let mut arena = Arena::default();
    let viewport = Rect::from_size(VIEWPORT);

    let mut frame = |view: &mut ScrollView<ListView<Row>>, delta: f32| -> Duration {
        arena.reset();
        let arena = &arena;
        let started = std::time::Instant::now();

        let command = {
            let widget = crate::Layout::layout(
                view.display(&arena, &store, &ui),
                &arena,
                Constraints::tight(VIEWPORT),
            )
            .realize(&arena, viewport);
            let gesture = crate::event::ScrollGesture::default();
            let event = Event::Scroll {
                delta_x: 0.0,
                point: Point::new(200.0, 450.0),
                delta_y: delta,
                gesture: &gesture,
            };
            match widget.handle_event(&arena, &event, viewport) {
                EventResult::Command(command) => Some(command),
                _ => None,
            }
        };
        if let Some(command) = command {
            perform_into(view, &mut store, &ui, command);
        }

        {
            let widget = crate::Layout::layout(
                view.display(&arena, &store, &ui),
                &arena,
                Constraints::tight(VIEWPORT),
            )
            .realize(&arena, viewport);
            let canvas = surface.canvas();
            let _ = widget.handle_event(
                &arena,
                &Event::Paint {
                    canvas,
                    focused: true,
                },
                viewport,
            );
        }
        started.elapsed()
    };

    for _ in 0..50 {
        frame(&mut view, 512.0);
    }
    let mut samples: Vec<Duration> = Vec::new();
    for _ in 0..2_000 {
        samples.push(frame(&mut view, 512.0));
    }
    samples.sort();
    let p50 = samples[samples.len() / 2].as_secs_f64() * 1000.0;
    let p95 = samples[samples.len() * 95 / 100].as_secs_f64() * 1000.0;
    let max = samples[samples.len() - 1].as_secs_f64() * 1000.0;
    eprintln!(
        "[bench] list scroll: rows={ROWS} build={:.3}ms frame p50={p50:.4}ms p95={p95:.4}ms max={max:.4}ms",
        built.as_secs_f64() * 1000.0,
    );
    crate::perf::record("list-scroll", "build_ms", built.as_secs_f64() * 1000.0);
    crate::perf::record("list-scroll", "frame_p50_ms", p50);
    crate::perf::record("list-scroll", "frame_p95_ms", p95);
}

fn focus_commands_of<V: View>(
    view: &V,
    store: &Store,
    ui: &crate::ui::UiCtx,
) -> Vec<crate::PresentableCommand<V::Command>> {
    let arena = Arena::default();
    let mut widget = crate::Layout::layout(
        view.display(&arena, store, ui),
        &arena,
        Constraints::tight(Size::new(200.0, 120.0)),
    )
    .realize(&arena, Rect::from_wh(200.0, 120.0));
    let commands = std::mem::take(&mut widget.focus_data().commands);
    drop(widget);
    commands
}

#[test]
fn commands_follow_focus_and_map_up_the_tree() {
    let mut store = Store::new();
    let ui = crate::ui::UiCtx::cold();

    let mut view = ScrollView::new(row_list());
    assert!(focus_commands_of(&view, &store, &ui).is_empty());

    perform_into(
        &mut view,
        &mut store,
        &ui,
        ScrollCommand::Content(ListCommand::Focus(1, None)),
    );
    let commands = focus_commands_of(&view, &store, &ui);
    assert_eq!(commands.len(), 1);
    let presented = &commands[0];
    assert_eq!(presented.id, "row.poke");
    assert_eq!(presented.name, "Poke Row");
    assert!(matches!(
        presented.command,
        ScrollCommand::Content(ListCommand::Child(1, RowCommand::Text("poked")))
    ));

    perform_into(
        &mut view,
        &mut store,
        &ui,
        ScrollCommand::Content(ListCommand::Focus(2, None)),
    );
    assert!(matches!(
        focus_commands_of(&view, &store, &ui)[0].command,
        ScrollCommand::Content(ListCommand::Child(2, _))
    ));
}

#[test]
fn split_and_dyn_views_carry_commands() {
    let mut store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    let mut focused_list = row_list();
    perform_into(
        &mut focused_list,
        &mut store,
        &ui,
        ListCommand::Focus(0, None),
    );
    let mut view = SplitView::row(row_list(), focused_list);

    assert!(focus_commands_of(&view, &store, &ui).is_empty());

    perform_into(
        &mut view,
        &mut store,
        &ui,
        SplitCommand::Focus(Pane::Second, None),
    );
    let commands = focus_commands_of(&view, &store, &ui);
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        commands[0].command,
        SplitCommand::Second(ListCommand::Child(0, RowCommand::Text("poked")))
    ));

    let erased: Box<dyn crate::DynView> = Box::new({
        let mut list = row_list();
        perform_into(&mut list, &mut store, &ui, ListCommand::Focus(1, None));
        list
    });
    let commands = focus_commands_of(&erased, &store, &ui);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].id, "row.poke");
    assert!(commands[0]
        .command
        .downcast_ref::<ListCommand<RowCommand>>()
        .is_some_and(|command| matches!(
            command,
            ListCommand::Child(1, RowCommand::Text("poked"))
        )));
}

mod list_tree {
    use super::*;
    use crate::list::ListSlice;

    fn slice(keys: &[u64]) -> ListSlice<Row, u64> {
        let mut slice = ListSlice::new();
        for key in keys {
            slice.push_keyed_sized(*key, Row { height: 10.0 }, 10.0);
        }
        slice
    }

    fn tree(roots: &[u64]) -> ListView<Row, u64> {
        ListView::from_slice(slice(roots))
    }

    fn expand(list: &mut ListView<Row, u64>, key: u64, children: &[u64]) {
        let range = list.row_range(&key).expect("node exists");
        let mut block = slice(&[key]);
        for child in children {
            block.push_keyed_sized(*child, Row { height: 10.0 }, 10.0);
        }
        block.cover(key, 0..children.len() + 1);
        list.splice_slice(range, block);
    }

    fn collapse(list: &mut ListView<Row, u64>, key: u64) {
        let range = list.row_range(&key).expect("node exists");
        list.splice_slice(range, slice(&[key]));
    }

    fn ranges(list: &ListView<Row, u64>, keys: &[u64]) -> Vec<Option<std::ops::Range<usize>>> {
        keys.iter().map(|key| list.row_range(key)).collect()
    }

    #[test]
    fn expand_and_collapse_keep_every_span_honest() {
        let mut list = tree(&[1, 2, 3]);
        assert_eq!(list.len(), 3);
        assert_eq!(
            ranges(&list, &[1, 2, 3]),
            vec![Some(0..1), Some(1..2), Some(2..3)]
        );

        expand(&mut list, 2, &[21, 22, 23]);
        assert_eq!(list.len(), 6);
        assert_eq!(
            ranges(&list, &[1, 2, 21, 22, 23, 3]),
            vec![
                Some(0..1),
                Some(1..5),
                Some(2..3),
                Some(3..4),
                Some(4..5),
                Some(5..6)
            ]
        );

        expand(&mut list, 22, &[221, 222]);
        assert_eq!(list.len(), 8);
        assert_eq!(
            ranges(&list, &[2, 22, 221, 222, 23, 3]),
            vec![
                Some(1..7),
                Some(3..6),
                Some(4..5),
                Some(5..6),
                Some(6..7),
                Some(7..8)
            ]
        );

        collapse(&mut list, 2);
        assert_eq!(list.len(), 3);
        assert_eq!(
            ranges(&list, &[1, 2, 3, 21, 22, 221]),
            vec![Some(0..1), Some(1..2), Some(2..3), None, None, None]
        );
    }

    #[test]
    fn a_nested_slice_lands_atomically() {
        let mut list = tree(&[1, 2]);

        let mut block = slice(&[1, 11, 111, 112, 12]);
        block.cover(11, 1..4);
        block.cover(1, 0..5);
        let range = list.row_range(&1).unwrap();
        list.splice_slice(range, block);
        assert_eq!(list.len(), 6);
        assert_eq!(list.row_range(&1), Some(0..5));
        assert_eq!(list.row_range(&11), Some(1..4));
        assert_eq!(list.row_range(&112), Some(3..4));

        collapse(&mut list, 11);
        assert_eq!(list.len(), 4);
        assert_eq!(list.row_range(&1), Some(0..3));
        assert_eq!(list.row_range(&111), None);
    }

    #[test]
    fn key_at_resolves_the_innermost_owner() {
        let mut list = tree(&[1, 2]);
        expand(&mut list, 1, &[11, 12]);
        assert_eq!(list.key_at(0), Some(&1), "the header row is the node's own");
        assert_eq!(list.key_at(1), Some(&11));
        assert_eq!(list.key_at(2), Some(&12));
        assert_eq!(list.key_at(3), Some(&2));
        assert_eq!(list.key_at(4), None);
        assert_eq!(list.depth_at(0), 0);
        assert_eq!(list.depth_at(1), 1);
        assert_eq!(list.depth_at(3), 0);
    }

    #[test]
    fn selection_shifts_with_splices_and_repoints_on_folds() {
        let mut list = tree(&[1, 2, 3]).with_selection(Default::default());

        list.select_only(2);
        expand(&mut list, 2, &[21, 22]);
        assert_eq!(list.cursor(), Some(&2));
        assert!(list.is_selected(&2), "the expanded node stays selected");
        let index = list.row_range(&2).unwrap().start;
        assert_eq!(list.key_at(index), Some(&2));

        list.select_only(22);
        assert_eq!(list.cursor(), Some(&22));
        assert!(list.is_selected(&22));

        expand(&mut list, 1, &[11]);
        assert_eq!(list.cursor(), Some(&22));
        let index = list.row_range(&22).unwrap().start;
        assert_eq!(list.key_at(index), Some(&22), "the selection followed");

        collapse(&mut list, 2);
        assert_eq!(list.cursor(), Some(&2), "re-pointed to the fold");
        assert!(list.is_selected(&2));
        assert!(!list.is_selected(&22));
    }

    #[test]
    fn cursor_steps_over_keyed_rows_only() {
        let mut block: ListSlice<Row, u64> = ListSlice::new();
        block.push_keyed_sized(1, Row { height: 10.0 }, 10.0);
        block.push_sized(Row { height: 10.0 }, 10.0);
        block.push_keyed_sized(2, Row { height: 10.0 }, 10.0);
        let mut list = ListView::from_slice(block).with_selection(Default::default());
        list.select_only(1);
        list.cursor_step(1);
        assert_eq!(list.cursor(), Some(&2), "the separator was skipped");
        list.cursor_step(1);
        assert_eq!(list.cursor(), Some(&2), "clamped at the end");
        list.cursor_step(-1);
        assert_eq!(list.cursor(), Some(&1));
    }

    #[test]
    fn a_huge_tree_expands_and_collapses_in_log_time() {
        let count = 200_000u64;
        let built = std::time::Instant::now();
        let mut list = tree(&(1..=count).collect::<Vec<_>>());
        let build_ms = built.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(list.len(), count as usize);

        let mid = count / 2;
        let started = std::time::Instant::now();
        let mut ops = 0u32;
        for round in 0..50 {
            let key = mid + round;
            expand(
                &mut list,
                key,
                &[(key + 1) * 1_000_000, (key + 1) * 1_000_000 + 1],
            );
            collapse(&mut list, key);
            ops += 2;
        }
        let per_op_ms = started.elapsed().as_secs_f64() * 1000.0 / f64::from(ops);
        eprintln!("[probe] tree ops over {count} rows: build={build_ms:.1}ms op={per_op_ms:.3}ms");
        crate::perf::record("tree-ops", "toggle_ms", per_op_ms);
        crate::perf::record("tree-ops", "build_ms", build_ms);
        assert_eq!(list.len(), count as usize, "every toggle round-tripped");
        assert!(
            per_op_ms < 5.0,
            "expand/collapse over a 200k-row tree costs {per_op_ms:.2}ms — not log-time"
        );
    }
}

mod animated_splice {
    use super::*;
    use crate::anim::AnimationClock;

    fn heights(list: &ListView<Row>) -> Vec<f32> {
        list.rows().map(|row| row.height).collect()
    }

    fn cached_total(list: &ListView<Row>) -> f32 {
        list.total_height()
    }

    fn tick(list: &mut ListView<Row>, store: &mut Store, at_ms: f64) {
        perform_into(
            list,
            store,
            &crate::ui::UiCtx::cold(),
            ListCommand::Animate(AnimationClock::from_millis(at_ms)),
        );
    }

    #[test]
    fn spliced_rows_grow_into_place() {
        let mut store = Store::new();
        let mut list = ListView::from_rope(crate::list::measured([(Row { height: 20.0 }, 20.0)]));

        list.splice_animated(0..1, (0..4).map(|_| (Row { height: 20.0 }, 20.0)));
        assert_eq!(list.len(), 4);
        let entering = cached_total(&list);
        assert!(
            (entering - 20.0).abs() < 2.0,
            "the block enters at the old extent: {entering}"
        );

        tick(&mut list, &mut store, 0.0);
        tick(&mut list, &mut store, 80.0);
        let mid = cached_total(&list);
        assert!(
            mid > 24.0 && mid < 79.0,
            "mid-flight the block is between the extents: {mid}"
        );

        tick(&mut list, &mut store, 400.0);
        assert_eq!(heights(&list), vec![20.0; 4]);
        assert!((cached_total(&list) - 80.0).abs() < 1.0);
    }

    #[test]
    fn a_fold_shrinks_into_place() {
        let mut store = Store::new();
        let mut list = ListView::from_rope(crate::list::measured(
            (0..5).map(|_| (Row { height: 20.0 }, 20.0)),
        ));
        list.splice_animated(1..4, [(Row { height: 20.0 }, 20.0)]);
        assert_eq!(list.len(), 3);
        let entering = cached_total(&list);
        assert!(
            (entering - 100.0).abs() < 2.0,
            "the fold enters at the unfolded extent: {entering}"
        );
        tick(&mut list, &mut store, 0.0);
        tick(&mut list, &mut store, 400.0);
        assert_eq!(heights(&list), vec![20.0; 3]);
    }

    #[test]
    fn paint_does_not_snap_animating_rows() {
        let mut store = Store::new();
        let ui = crate::ui::UiCtx::cold();
        let mut list = ListView::from_rope(crate::list::measured([(Row { height: 20.0 }, 20.0)]));
        list.splice_animated(0..1, (0..4).map(|_| (Row { height: 20.0 }, 20.0)));
        let entering = cached_total(&list);

        let arena = Arena::default();
        let mut surface = surfaces::raster_n32_premul((100, 200)).expect("raster surface");
        let result = {
            let widget = crate::Layout::layout(
                list.display(&arena, &store, &ui),
                &arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(100.0, f32::MAX),
                },
            )
            .realize(&arena, Rect::from_wh(100.0, 200.0));
            widget.handle_event(
                &arena,
                &Event::Paint {
                    focused: false,
                    canvas: surface.canvas(),
                },
                Rect::from_wh(100.0, 200.0),
            )
        };

        if let EventResult::Command(command) = result {
            perform_into(&mut list, &mut store, &ui, command);
        }
        assert!(
            (cached_total(&list) - entering).abs() < 1.0,
            "paint left the animated heights alone: entered {entering}, now {}",
            cached_total(&list)
        );

        tick(&mut list, &mut store, 0.0);
        tick(&mut list, &mut store, 400.0);
        assert_eq!(heights(&list), vec![20.0; 4]);
    }

    #[test]
    fn a_new_splice_settles_the_animation_it_lands_on() {
        let mut store = Store::new();
        let mut list = ListView::from_rope(crate::list::measured([(Row { height: 20.0 }, 20.0)]));
        list.splice_animated(0..1, (0..4).map(|_| (Row { height: 20.0 }, 20.0)));
        tick(&mut list, &mut store, 0.0);
        tick(&mut list, &mut store, 40.0);

        list.splice(3..4, [(Row { height: 30.0 }, 30.0)]);
        assert_eq!(heights(&list), vec![20.0, 20.0, 20.0, 30.0]);

        let mut list = ListView::from_rope(crate::list::measured([(Row { height: 20.0 }, 20.0)]));
        list.splice_animated(0..0, (0..2).map(|_| (Row { height: 20.0 }, 20.0)));
        tick(&mut list, &mut store, 0.0);
        list.splice(2..2, [(Row { height: 40.0 }, 40.0)]);
        tick(&mut list, &mut store, 400.0);
        assert_eq!(heights(&list), vec![20.0, 20.0, 40.0, 20.0]);
    }
}

mod reveal {
    use crate::event::{reveal_satisfied, reveal_scroll_target, EventResult, Reveal};
    use skia_safe::Rect;

    #[test]
    fn merge_lets_commands_dominate_a_reveal() {
        let reveal: EventResult<u32> =
            EventResult::Reveal(Reveal::visible(Rect::from_xywh(0.0, 0.0, 1.0, 1.0)));
        match reveal.merge(EventResult::Command(7)) {
            EventResult::Commands(commands) => assert_eq!(commands, vec![7]),
            _ => panic!("commands win the merge"),
        }
        let reveal: EventResult<u32> =
            EventResult::Reveal(Reveal::visible(Rect::from_xywh(1.0, 2.0, 3.0, 4.0)));
        match reveal.merge(EventResult::Ignored) {
            EventResult::Reveal(reveal) => {
                assert_eq!(reveal.rect, Rect::from_xywh(1.0, 2.0, 3.0, 4.0))
            }
            _ => panic!("a lone reveal survives the merge"),
        }
        let first: EventResult<u32> =
            EventResult::Reveal(Reveal::visible(Rect::from_xywh(1.0, 0.0, 1.0, 1.0)));
        let second: EventResult<u32> =
            EventResult::Reveal(Reveal::visible(Rect::from_xywh(2.0, 0.0, 1.0, 1.0)));
        match first.merge(second) {
            EventResult::Reveal(reveal) => {
                assert_eq!(reveal.rect.left, 1.0, "the first reveal wins")
            }
            _ => panic!("one reveal survives"),
        }
    }

    #[test]
    fn scroll_targets_land_at_the_golden_section() {
        let golden = |target: f32, expected: f32| {
            assert!(
                (target - expected).abs() < 0.001,
                "target {target} vs golden {expected}"
            );
        };

        assert_eq!(reveal_scroll_target(100.0, 50.0, 110.0, 140.0), 100.0);

        golden(
            reveal_scroll_target(0.0, 50.0, 80.0, 100.0),
            80.0 - 50.0 * 0.381_966,
        );

        golden(
            reveal_scroll_target(100.0, 50.0, 20.0, 40.0),
            20.0 - 50.0 * 0.381_966,
        );

        golden(
            reveal_scroll_target(0.0, 50.0, 200.0, 400.0),
            200.0 - 50.0 * 0.381_966,
        );

        assert_eq!(reveal_scroll_target(190.0, 50.0, 200.0, 400.0), 190.0);
    }

    #[test]
    fn satisfied_matches_the_target_rule() {
        let viewport = Rect::from_xywh(0.0, 100.0, 200.0, 50.0);
        assert!(reveal_satisfied(
            viewport,
            Rect::from_xywh(10.0, 110.0, 20.0, 30.0)
        ));
        assert!(!reveal_satisfied(
            viewport,
            Rect::from_xywh(10.0, 90.0, 20.0, 30.0)
        ));
        assert!(!reveal_satisfied(
            viewport,
            Rect::from_xywh(10.0, 140.0, 20.0, 30.0)
        ));

        assert!(reveal_satisfied(
            viewport,
            Rect::from_xywh(10.0, 100.0, 20.0, 300.0)
        ));
        assert!(!reveal_satisfied(
            viewport,
            Rect::from_xywh(10.0, 200.0, 20.0, 300.0)
        ));
    }
}

fn scrollbar_style() -> crate::scroll::ScrollbarStyle {
    crate::scroll::ScrollbarStyle {
        color: skia_safe::Color::WHITE,
        width: 4.0,
        margin: 7.0,
        radius: 2.0,
        min_knob: 28.0,
        track_inset: 12.0,
    }
}

// Content 500 tall in a 120 viewport with the style above: max scroll
// 380, track 12..108 (height 96), knob 28 tall, knob travel 68.
fn knob_event(
    view: &ScrollView<FixedContent>,
    event: Event<'_>,
) -> EventResult<ScrollCommand<ContentCommand>> {
    let arena = Arena::default();
    let store = Store::new();
    let ui = crate::ui::UiCtx::cold();
    ui.set(scrollbar_style());
    let widget = crate::Layout::layout(
        view.display(&arena, &store, &ui),
        &arena,
        Constraints::tight(Size::new(100.0, 120.0)),
    )
    .realize(&arena, Rect::from_wh(100.0, 120.0));
    widget.handle_event(&arena, &event, Rect::from_wh(100.0, 120.0))
}

fn knob_perform(view: &mut ScrollView<FixedContent>, command: ScrollCommand<ContentCommand>) {
    let mut batch = crate::effect::Batch::new();
    let ui = crate::ui::UiCtx::cold();
    view.perform(&mut Store::new(), &ui, command, &mut batch.effects());
}

#[test]
fn the_knob_drags_the_scroll_and_releases_back_to_the_content() {
    let mut view = ScrollView::new(FixedContent {
        size: Size::new(100.0, 500.0),
    });

    // A press ON the knob (band x 93..97, knob y 12..40) arms the
    // drag without moving the scroll.
    let press = knob_event(
        &view,
        Event::MouseDown {
            point: Point::new(96.0, 20.0),
            button: MouseButton::Left,
            mods: Default::default(),
            count: 1,
        },
    );
    let (scroll_y, grab) = match press {
        EventResult::Command(ScrollCommand::BeginKnobDrag { scroll_y, grab }) => (scroll_y, grab),
        _ => panic!("the knob press arms the drag"),
    };
    assert_eq!((scroll_y, grab), (0.0, 8.0));
    knob_perform(&mut view, ScrollCommand::BeginKnobDrag { scroll_y, grab });

    // Dragging tracks the pointer proportionally — X free to wander.
    let drag = knob_event(
        &view,
        Event::MouseDrag {
            point: Point::new(30.0, 54.0),
            mods: Default::default(),
        },
    );
    match drag {
        EventResult::Command(ScrollCommand::SetScrollY(next)) => {
            assert!((next - 190.0).abs() < 0.01, "half the travel: {next}");
            knob_perform(&mut view, ScrollCommand::SetScrollY(next));
        }
        _ => panic!("a drag steers the scroll, not the content"),
    }

    // Past the track's end the drag clamps to the bottom.
    let drag = knob_event(
        &view,
        Event::MouseDrag {
            point: Point::new(96.0, 400.0),
            mods: Default::default(),
        },
    );
    match drag {
        EventResult::Command(ScrollCommand::SetScrollY(next)) => assert_eq!(next, 380.0),
        _ => panic!("the clamped drag still lands"),
    }

    // The release disarms; the next drag is the content's again.
    let up = knob_event(
        &view,
        Event::MouseUp {
            point: Point::new(96.0, 400.0),
        },
    );
    match up {
        EventResult::Command(ScrollCommand::EndKnobDrag) => {
            knob_perform(&mut view, ScrollCommand::EndKnobDrag)
        }
        _ => panic!("the release ends the drag"),
    }
    let after = knob_event(
        &view,
        Event::MouseDrag {
            point: Point::new(30.0, 54.0),
            mods: Default::default(),
        },
    );
    assert!(
        matches!(after, EventResult::Ignored),
        "with the drag over, pointer traffic forwards to the content"
    );
}

#[test]
fn a_track_press_jumps_the_knob_under_the_pointer() {
    let view = ScrollView::new(FixedContent {
        size: Size::new(100.0, 500.0),
    });
    // y = 80 is track below the knob: the knob centers there
    // (top 80 - 14 = 66) and the drag arms in the same grip.
    let press = knob_event(
        &view,
        Event::MouseDown {
            point: Point::new(96.0, 80.0),
            button: MouseButton::Left,
            mods: Default::default(),
            count: 1,
        },
    );
    match press {
        EventResult::Command(ScrollCommand::BeginKnobDrag { scroll_y, grab }) => {
            assert_eq!(grab, 14.0);
            let expected = (66.0 - 12.0) / 68.0 * 380.0;
            assert!(
                (scroll_y - expected).abs() < 0.01,
                "{scroll_y} vs {expected}"
            );
        }
        _ => panic!("a track press arms the drag"),
    }

    // A press LEFT of the bar is the content's.
    let press = knob_event(
        &view,
        Event::MouseDown {
            point: Point::new(50.0, 80.0),
            button: MouseButton::Left,
            mods: Default::default(),
            count: 1,
        },
    );
    assert!(
        matches!(
            press,
            EventResult::Command(ScrollCommand::Content(ContentCommand::Click(_)))
        ),
        "content presses still route to the content"
    );
}

mod viewport_preservation {
    use super::*;
    use crate::event::Placement;
    use crate::list::{ListSlice, ListView};
    use crate::scroll::{ScrollCommand, ScrollView};

    fn keyed_rows(rows: &[(u64, f32)]) -> ListView<Row, u64> {
        let mut slice: ListSlice<Row, u64> = ListSlice::new();
        for (key, height) in rows {
            slice.push_keyed_sized(*key, Row { height: *height }, *height);
        }
        ListView::from_slice_at(100.0, slice)
    }

    fn observe_top(list: &mut ListView<Row, u64>, store: &mut Store, top: f32) {
        let ui = crate::ui::UiCtx::cold();
        let mut batch: crate::effect::Batch<ListCommand<RowCommand>> = crate::effect::Batch::new();
        list.perform(
            store,
            &ui,
            ListCommand::ViewportTop(top),
            &mut batch.effects(),
        );
    }

    fn pulse_list(
        list: &ListView<Row, u64>,
        store: &Store,
        ui: &crate::ui::UiCtx,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<ListCommand<RowCommand>> {
        let arena = Arena::default();
        let widget = crate::Layout::layout(
            list.display(&arena, store, ui),
            &arena,
            Constraints {
                min: Size::default(),
                max: Size::new(100.0, f32::MAX),
            },
        )
        .realize(&arena, viewport);
        widget.handle_event(&arena, event, viewport)
    }

    #[test]
    fn a_splice_above_the_viewport_re_aims_the_anchor_exactly() {
        let mut store = Store::new();
        let ui = crate::ui::UiCtx::cold();
        // Ten rows of 20px; the viewport's top edge cuts 5px into the
        // third row (key 3, offset 40). The enclosing scroll pushes
        // the top in through the `scrolled` hook — retained state,
        // mutated where every scroll mutation happens.
        let mut list = keyed_rows(&(1..=10).map(|key| (key, 20.0)).collect::<Vec<_>>());
        observe_top(&mut list, &mut store, 45.0);
        let viewport = Rect::from_xywh(0.0, 45.0, 100.0, 60.0);

        // Three 30px rows land ABOVE the viewport.
        list.splice(0..0, (0..3).map(|_| (Row { height: 30.0 }, 30.0)));

        match pulse_list(&list, &store, &ui, &Event::Settle, viewport) {
            EventResult::Reveal(reveal) => {
                assert_eq!(reveal.placement, Placement::TopLeftAt);
                assert_eq!(
                    reveal.rect.top, 135.0,
                    "the anchored row's dy is preserved to the pixel"
                );
            }
            _ => panic!("the settle pulse re-aims the moved anchor"),
        }

        // The correction lands as a scroll — which supersedes it.
        observe_top(&mut list, &mut store, 135.0);
        assert!(
            matches!(
                pulse_list(
                    &list,
                    &store,
                    &ui,
                    &Event::Settle,
                    Rect::from_xywh(0.0, 135.0, 100.0, 60.0)
                ),
                EventResult::Handled
            ),
            "a landed correction stays landed"
        );
    }

    #[test]
    fn a_splice_below_the_viewport_disturbs_nothing() {
        let mut store = Store::new();
        let ui = crate::ui::UiCtx::cold();
        let mut list = keyed_rows(&(1..=10).map(|key| (key, 20.0)).collect::<Vec<_>>());
        observe_top(&mut list, &mut store, 45.0);
        let viewport = Rect::from_xywh(0.0, 45.0, 100.0, 60.0);

        list.splice(10..10, (0..3).map(|_| (Row { height: 30.0 }, 30.0)));

        assert!(
            matches!(
                pulse_list(&list, &store, &ui, &Event::Settle, viewport),
                EventResult::Handled
            ),
            "content below the viewport does not move the anchor"
        );
    }

    #[test]
    fn the_scroll_view_jumps_to_the_settled_anchor() {
        let mut store = Store::new();
        let ui = crate::ui::UiCtx::cold();
        let mut scroll = ScrollView::new(keyed_rows(
            &(1..=10).map(|key| (key, 20.0)).collect::<Vec<_>>(),
        ));
        scroll.set_scroll_y(45.0);

        let pulse = |scroll: &ScrollView<ListView<Row, u64>>, store: &Store| {
            let arena = Arena::default();
            let result = crate::Layout::layout(
                scroll.display(&arena, store, &ui),
                &arena,
                Constraints::tight(Size::new(100.0, 60.0)),
            )
            .realize(&arena, Rect::from_wh(100.0, 60.0))
            .handle_event(&arena, &Event::Settle, Rect::from_wh(100.0, 60.0));
            result
        };

        // Round one: the widget RE-OBSERVES the moved top — the
        // report is the answer, and its perform refreshes the
        // retained copy before any door can read it.
        let mut batch: crate::effect::Batch<ScrollCommand<ListCommand<RowCommand>>> =
            crate::effect::Batch::new();
        let commands = match pulse(&scroll, &store) {
            EventResult::Command(command) => vec![command],
            EventResult::Commands(commands) => commands,
            _ => panic!("round one reports the top"),
        };
        assert!(commands.iter().any(|command| matches!(
            command,
            ScrollCommand::Content(ListCommand::ViewportTop(top)) if *top == 45.0
        )));
        for command in commands {
            scroll.perform(&mut store, &ui, command, &mut batch.effects());
        }

        scroll
            .content_mut()
            .splice(0..0, (0..3).map(|_| (Row { height: 30.0 }, 30.0)));

        // Round two: the door's correction, exact.
        match pulse(&scroll, &store) {
            EventResult::Command(ScrollCommand::JumpTo(target)) => {
                assert_eq!(target, 135.0, "the corner lands back on the anchored row");
            }
            _ => panic!("the settle pulse becomes an exact jump"),
        }
    }
}

mod row_reveal {
    use super::*;
    use crate::anim::AnimationClock;
    use crate::event::Placement;
    use crate::list::{ListSlice, ListView};

    fn keyed_rows(rows: &[(u64, f32)]) -> ListView<Row, u64> {
        let mut slice: ListSlice<Row, u64> = ListSlice::new();
        for (key, height) in rows {
            slice.push_keyed_sized(*key, Row { height: *height }, *height);
        }
        ListView::from_slice_at(100.0, slice)
    }

    /// `merge` folds lone commands into `Commands` — accept both.
    fn revealed(result: EventResult<ListCommand<RowCommand>>) -> bool {
        match result {
            EventResult::Command(ListCommand::Revealed) => true,
            EventResult::Commands(commands) => commands
                .iter()
                .any(|command| matches!(command, ListCommand::Revealed)),
            _ => false,
        }
    }

    fn clock(
        list: &ListView<Row, u64>,
        store: &Store,
        ui: &crate::ui::UiCtx,
        viewport: Rect,
    ) -> EventResult<ListCommand<RowCommand>> {
        let arena = Arena::default();
        let event = Event::AnimationClock {
            now: AnimationClock::from_millis(16.0),
        };
        let result = crate::Layout::layout(
            list.display(&arena, store, ui),
            &arena,
            Constraints {
                min: Size::default(),
                max: Size::new(100.0, f32::MAX),
            },
        )
        .realize(&arena, viewport)
        .handle_event(&arena, &event, viewport);
        result
    }

    #[test]
    fn a_top_left_reveal_aims_the_row_exactly_and_disarms_on_landing() {
        let store = Store::new();
        let ui = crate::ui::UiCtx::cold();
        let mut list = keyed_rows(&(1..=10).map(|key| (key, 20.0)).collect::<Vec<_>>());
        list.reveal_row(7, Placement::TopLeftAt);

        match clock(&list, &store, &ui, Rect::from_xywh(0.0, 0.0, 100.0, 60.0)) {
            EventResult::Reveal(reveal) => {
                assert_eq!(reveal.placement, Placement::TopLeftAt);
                assert_eq!(reveal.rect.top, 120.0, "key 7 starts at 6 rows of 20px");
            }
            _ => panic!("an armed reveal emits on the clock"),
        }

        // The scroll landed the jump: the next clock answers Revealed,
        // and performing it disarms the slot.
        assert!(
            revealed(clock(
                &list,
                &store,
                &ui,
                Rect::from_xywh(0.0, 120.0, 100.0, 60.0)
            )),
            "a placed reveal reports Revealed"
        );
        let mut store = Store::new();
        let mut batch: crate::effect::Batch<ListCommand<RowCommand>> = crate::effect::Batch::new();
        list.perform(&mut store, &ui, ListCommand::Revealed, &mut batch.effects());
        assert!(
            matches!(
                clock(&list, &store, &ui, Rect::from_xywh(0.0, 120.0, 100.0, 60.0)),
                EventResult::Ignored
            ),
            "a disarmed reveal stays quiet"
        );
    }

    #[test]
    fn a_tail_reveal_settles_at_the_clamp() {
        let store = Store::new();
        let ui = crate::ui::UiCtx::cold();
        // 10 rows x 20px = 200px; a 60px viewport clamps at 140.
        let mut list = keyed_rows(&(1..=10).map(|key| (key, 20.0)).collect::<Vec<_>>());
        list.reveal_row(10, Placement::TopLeftAt);

        assert!(
            matches!(
                clock(&list, &store, &ui, Rect::from_xywh(0.0, 0.0, 100.0, 60.0)),
                EventResult::Reveal(_)
            ),
            "the tail row still asks for its jump"
        );
        // The scroll could only reach the clamp — that IS placed.
        assert!(
            revealed(clock(
                &list,
                &store,
                &ui,
                Rect::from_xywh(0.0, 140.0, 100.0, 60.0)
            )),
            "a clamped tail reveal satisfies at max scroll"
        );
    }

    #[test]
    fn a_reveal_survives_a_splice_between_arming_and_the_clock() {
        let store = Store::new();
        let ui = crate::ui::UiCtx::cold();
        let mut list = keyed_rows(&(1..=10).map(|key| (key, 20.0)).collect::<Vec<_>>());
        list.reveal_row(7, Placement::TopLeftAt);

        // Three 30px rows land above before the clock fires: the
        // reveal resolves against the LIVE rope, not a stale rect.
        list.splice(0..0, (0..3).map(|_| (Row { height: 30.0 }, 30.0)));

        match clock(&list, &store, &ui, Rect::from_xywh(0.0, 0.0, 100.0, 60.0)) {
            EventResult::Reveal(reveal) => {
                assert_eq!(reveal.rect.top, 210.0, "90px of landings shifted the row");
            }
            _ => panic!("the reveal re-aims through the splice"),
        }
    }

    #[test]
    fn a_lost_key_disarms_instead_of_arming_forever() {
        let store = Store::new();
        let ui = crate::ui::UiCtx::cold();
        let mut list = keyed_rows(&(1..=10).map(|key| (key, 20.0)).collect::<Vec<_>>());
        list.reveal_row(7, Placement::TopLeftAt);
        list.splice(6..7, std::iter::empty::<(Row, f32)>());

        assert!(
            revealed(clock(
                &list,
                &store,
                &ui,
                Rect::from_xywh(0.0, 0.0, 100.0, 60.0)
            )),
            "a reveal whose key left the list asks to be disarmed"
        );
    }

    #[test]
    fn an_ensure_visible_reveal_keeps_the_golden_section_road() {
        let store = Store::new();
        let ui = crate::ui::UiCtx::cold();
        let mut list = keyed_rows(&(1..=10).map(|key| (key, 20.0)).collect::<Vec<_>>());
        list.reveal_row(7, Placement::EnsureVisible);

        match clock(&list, &store, &ui, Rect::from_xywh(0.0, 0.0, 100.0, 60.0)) {
            EventResult::Reveal(reveal) => {
                assert_eq!(reveal.placement, Placement::EnsureVisible);
            }
            _ => panic!("an off-screen row asks to be scrolled into view"),
        }
        // Fully visible already → satisfied without moving.
        assert!(
            revealed(clock(
                &list,
                &store,
                &ui,
                Rect::from_xywh(0.0, 110.0, 100.0, 60.0)
            )),
            "a visible row satisfies in place"
        );
    }
}
