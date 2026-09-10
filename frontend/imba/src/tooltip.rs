use std::convert::Infallible;
use std::sync::Arc;

use skia_safe::Contains;
use skia_safe::{Point, Rect, Size};

use crate::arena::Arena;
use crate::constraints::Constraints;
use crate::event::{Event, EventResult};
use crate::store::Store;
use crate::thunk_ext::ThunkExt;
use crate::{overlay, Thunk, ThunkBox, View, Widget, WidgetBox};

const HOVER_DELAY_MS: f32 = 450.0;

const TIP_GAP: f32 = 8.0;

pub enum TooltipCommand<C> {
    Host(C),

    Moved(Point),
    Left,

    Tick(crate::anim::AnimationClock),
}

#[derive(Clone)]
enum Hover<T> {
    Idle,

    Arming {
        anchor: Rect,
        tip: T,
        rested: f32,
        last: Option<crate::anim::AnimationClock>,
    },
    Shown { anchor: Rect, tip: T },
}

impl<V: Clone, T: Clone> Clone for TooltipView<V, T> {
    fn clone(&self) -> Self {
        Self {
            view: self.view.clone(),
            provide: Arc::clone(&self.provide),
            hover: self.hover.clone(),
        }
    }
}

pub struct TooltipView<V, T> {
    view: V,
    provide: Arc<dyn Fn(&V, &Store, Point) -> Option<(Rect, T)> + Send + Sync>,
    hover: Hover<T>,
}

impl<V, T> TooltipView<V, T>
where
    V: View,
    T: View<Command = Infallible>,
{
    pub fn new(
        view: V,
        provide: impl Fn(&V, &Store, Point) -> Option<(Rect, T)> + Send + Sync + 'static,
    ) -> Self {
        Self {
            view,
            provide: Arc::new(provide),
            hover: Hover::Idle,
        }
    }

    pub fn view(&self) -> &V {
        &self.view
    }

    pub fn view_mut(&mut self) -> &mut V {
        &mut self.view
    }

    pub fn showing(&self) -> bool {
        matches!(self.hover, Hover::Shown { .. })
    }
}

impl<V, T> View for TooltipView<V, T>
where
    V: View,
    V::Command: 'static,
    T: View<Command = Infallible>,
{
    type Command = TooltipCommand<V::Command>;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &crate::ui::UiCtx,
        command: Self::Command,
        fx: &mut crate::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            TooltipCommand::Host(command) => {
                fx.scope(TooltipCommand::Host, |fx| {
                    self.view.perform(store, ui, command, fx)
                });
            }
            TooltipCommand::Moved(point) => match (self.provide)(&self.view, store, point) {
                None => self.hover = Hover::Idle,
                Some((anchor, tip)) => {
                    self.hover = match std::mem::replace(&mut self.hover, Hover::Idle) {
                        Hover::Arming {
                            anchor: standing,
                            tip: cached,
                            rested,
                            last,
                        } if standing == anchor => Hover::Arming {
                            anchor,
                            tip: cached,
                            rested,
                            last,
                        },
                        Hover::Shown {
                            anchor: standing,
                            tip: cached,
                        } if standing == anchor => Hover::Shown {
                            anchor,
                            tip: cached,
                        },

                        Hover::Shown { .. } => Hover::Shown { anchor, tip },
                        _ => Hover::Arming {
                            anchor,
                            tip,
                            rested: 0.0,
                            last: None,
                        },
                    };
                }
            },
            TooltipCommand::Left => self.hover = Hover::Idle,
            TooltipCommand::Tick(now) => {
                if let Hover::Arming {
                    anchor,
                    tip,
                    rested,
                    last,
                } = std::mem::replace(&mut self.hover, Hover::Idle)
                {
                    let rested = rested
                        + last
                            .map(|last| now.millis_since(last) as f32)
                            .unwrap_or(0.0);
                    self.hover = match rested >= HOVER_DELAY_MS {
                        true => Hover::Shown { anchor, tip },
                        false => Hover::Arming {
                            anchor,
                            tip,
                            rested,
                            last: Some(now),
                        },
                    };
                }
            }
        }
    }

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a crate::ui::UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let inner = ThunkBox::new(
            arena,
            self.view.layout(arena, store, ui, constraints).map(TooltipCommand::Host),
        );
        let tip = match &self.hover {
            Hover::Shown { anchor, tip } => Some((
                *anchor,
                ThunkBox::new(
                    arena,
                    tip.layout(arena, store, ui, Constraints::tight(constraints.max).loosen())
                        .map(|never: Infallible| match never {}),
                ),
            )),
            _ => None,
        };
        TooltipThunk {
            inner,
            tip,
            armed: matches!(self.hover, Hover::Arming { .. }),
            active: !matches!(self.hover, Hover::Idle),
        }
    }
}

struct TooltipThunk<'a, C> {
    inner: ThunkBox<'a, TooltipCommand<C>>,
    tip: Option<(Rect, ThunkBox<'a, TooltipCommand<C>>)>,
    armed: bool,
    active: bool,
}

impl<'a, C: 'a> Thunk<'a, TooltipCommand<C>> for TooltipThunk<'a, C> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn realize(
        self,
        arena: &'a Arena,
        viewport: Rect,
    ) -> WidgetBox<'a, TooltipCommand<C>> {
        let size = self.inner.size();
        WidgetBox::new(
            arena,
            TooltipWidget {
                inner: self.inner.realize(arena, viewport),
                tip: self.tip,
                armed: self.armed,
                active: self.active,
                size,
            },
        )
    }
}

struct TooltipWidget<'a, C> {
    inner: WidgetBox<'a, TooltipCommand<C>>,
    tip: Option<(Rect, ThunkBox<'a, TooltipCommand<C>>)>,
    armed: bool,
    active: bool,
    size: Size,
}

impl<'a, C: 'a> Widget<'a, TooltipCommand<C>> for TooltipWidget<'a, C> {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<TooltipCommand<C>> {
        let inner = self.inner.handle_event(arena, event, viewport);
        let mine = match event {
            Event::HitTest { point, miss } => {
                let inside = !miss && Rect::from_size(self.size).contains(*point);
                match (inside, self.active) {
                    (true, _) => EventResult::Command(TooltipCommand::Moved(*point)),
                    (false, true) => EventResult::Command(TooltipCommand::Left),

                    (false, false) => EventResult::Ignored,
                }
            }
            Event::AnimationClock { now } if self.armed => {
                EventResult::Command(TooltipCommand::Tick(*now))
            }
            _ => EventResult::Ignored,
        };
        inner.merge(mine)
    }

    fn blocks_pointer(&self, point: Point) -> bool {
        self.inner.blocks_pointer(point)
    }

    fn overlays(&mut self) -> Vec<overlay::Overlay<'a, TooltipCommand<C>>> {
        let mut overlays = self.inner.overlays();
        if let Some((anchor, tip)) = self.tip.take() {
            overlays.push(overlay::Overlay {
                host: overlay::WINDOW,
                anchor,
                content: Box::new(move |host_size: Size, anchor: Rect| {
                    let size = tip.size();

                    let mut x = anchor.right + TIP_GAP;
                    if x + size.width > host_size.width {
                        x = (anchor.left - TIP_GAP - size.width).max(0.0);
                    }
                    let y = anchor
                        .top
                        .min(host_size.height - size.height)
                        .max(0.0);
                    vec![(
                        Point::new(x, y),
                        tip,
                    )]
                }),
            });
        }
        overlays
    }

    fn focus_data<'w>(&'w mut self) -> crate::focus::FocusData<'w, TooltipCommand<C>>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::AnimationClock;
    use crate::leaf::leaf;

    struct Body;
    impl View for Body {
        type Command = u32;
        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &crate::ui::UiCtx,
            _command: u32,
            _fx: &mut crate::effect::Effects<'_, u32>,
        ) {
        }
        fn layout<'a>(
            &'a self,
            _arena: &'a Arena,
            _store: &'a Store,
            _ui: &'a crate::ui::UiCtx,
            _constraints: Constraints,
        ) -> impl Thunk<'a, u32> + 'a {
            leaf(200.0, 40.0)
        }
    }

    struct Tip;
    impl View for Tip {
        type Command = Infallible;
        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &crate::ui::UiCtx,
            command: Infallible,
            _fx: &mut crate::effect::Effects<'_, Infallible>,
        ) {
            match command {}
        }
        fn layout<'a>(
            &'a self,
            _arena: &'a Arena,
            _store: &'a Store,
            _ui: &'a crate::ui::UiCtx,
            _constraints: Constraints,
        ) -> impl Thunk<'a, Infallible> + 'a {
            leaf(60.0, 24.0)
        }
    }

    fn rows(_view: &Body, _store: &Store, point: Point) -> Option<(Rect, Tip)> {
        match point.y {
            y if (0.0..20.0).contains(&y) => Some((Rect::from_xywh(0.0, 0.0, 200.0, 20.0), Tip)),
            y if (20.0..40.0).contains(&y) => Some((Rect::from_xywh(0.0, 20.0, 200.0, 20.0), Tip)),
            _ => None,
        }
    }

    fn drive(view: &mut TooltipView<Body, Tip>, event: Event<'_>) {
        let ui = crate::ui::UiCtx::new();
        let commands = {
            let arena = Arena::default();
            let store = Store::new();
            let widget = view
                .layout(&arena, &store, &ui, Constraints::tight(Size::new(200.0, 40.0)))
                .realize(&arena, Rect::from_wh(200.0, 40.0));
            match widget.handle_event(&arena, &event, Rect::from_wh(200.0, 40.0)) {
                EventResult::Command(command) => vec![command],
                EventResult::Commands(commands) => commands,
                _ => Vec::new(),
            }
        };
        let mut store = Store::new();
        let mut batch = crate::effect::Batch::new();
        for command in commands {
            view.perform(&mut store, &ui, command, &mut batch.effects());
        }
    }

    fn overlay_count(view: &TooltipView<Body, Tip>) -> usize {
        let arena = Arena::default();
        let store = Store::new();
        let ui = crate::ui::UiCtx::new();
        let mut widget = view
            .layout(&arena, &store, &ui, Constraints::tight(Size::new(200.0, 40.0)))
            .realize(&arena, Rect::from_wh(200.0, 40.0));
        let count = widget.overlays().len();
        drop(widget);
        count
    }

    #[test]
    fn a_rested_hover_shows_walks_and_hides() {
        let mut view = TooltipView::new(Body, rows);
        let inside = Point::new(50.0, 10.0);
        drive(&mut view, Event::HitTest { point: inside, miss: false });
        assert!(!view.showing(), "arming, not shown");

        drive(
            &mut view,
            Event::AnimationClock {
                now: AnimationClock::from_millis(0.0),
            },
        );
        drive(
            &mut view,
            Event::AnimationClock {
                now: AnimationClock::from_millis(500.0),
            },
        );
        assert!(view.showing(), "the rest showed the tip");
        assert_eq!(overlay_count(&view), 1, "the tip is an overlay");

        drive(&mut view, Event::HitTest { point: Point::new(50.0, 30.0), miss: false });
        assert!(view.showing(), "the tip walks across rows");

        drive(&mut view, Event::HitTest { point: Point::new(500.0, 300.0), miss: false });
        assert!(!view.showing(), "leaving hides");
        assert_eq!(overlay_count(&view), 0);
    }

    #[test]
    fn bare_spots_and_outside_moves_stay_silent() {
        let mut view = TooltipView::new(Body, rows);

        let arena = Arena::default();
        let store = Store::new();
        let ui = crate::ui::UiCtx::new();
        let widget = view
            .layout(&arena, &store, &ui, Constraints::tight(Size::new(200.0, 40.0)))
            .realize(&arena, Rect::from_wh(200.0, 40.0));
        match widget.handle_event(
            &arena,
            &Event::HitTest {
                point: Point::new(500.0, 300.0),
                miss: false,
            },
            Rect::from_wh(200.0, 40.0),
        ) {
            EventResult::Ignored => {}
            _ => panic!("an idle tooltip stays silent on outside moves"),
        }
        drop(widget);

        drive(&mut view, Event::HitTest { point: Point::new(50.0, 39.9), miss: false });
        drive(&mut view, Event::HitTest { point: Point::new(50.0, 45.0), miss: false });
        assert!(!view.showing());
        drive(
            &mut view,
            Event::AnimationClock {
                now: AnimationClock::from_millis(1_000.0),
            },
        );
        assert!(!view.showing(), "nothing armed, nothing shows");
    }
}
