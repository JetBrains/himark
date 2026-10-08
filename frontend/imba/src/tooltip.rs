// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! A rested hover shows a tip beside a view. The view is the anchor
//! (a list row, a chip): the tip is its own overlay request and
//! rides to the window like any other, with nothing to measure.

use std::convert::Infallible;
use std::sync::Arc;

use skia_safe::Contains;
use skia_safe::{Rect, Size};

use crate::arena::Arena;
use crate::constraints::Constraints;
use crate::event::{Event, EventResult};
use crate::store::Store;
use crate::thunk_ext::ThunkExt;
use crate::with_overlay::Placement;
use crate::{Thunk, ThunkBox, View};

const HOVER_DELAY_MS: f32 = 450.0;

impl<C: std::fmt::Display> std::fmt::Display for TooltipCommand<C> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TooltipCommand::Host(command) => command.fmt(out),
            TooltipCommand::Entered => out.write_str("tooltip entered"),
            TooltipCommand::Left => out.write_str("tooltip left"),
            TooltipCommand::Tick(_) => out.write_str("tooltip tick"),
        }
    }
}

#[derive(Clone)]
pub enum TooltipCommand<C> {
    Host(C),

    Entered,
    Left,

    Tick(crate::anim::AnimationClock),
}

#[derive(Clone)]
enum Hover<T> {
    Idle,

    Arming {
        tip: T,
        rested: f32,
        last: Option<crate::anim::AnimationClock>,
    },
    Shown {
        tip: T,
    },
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
    provide: Arc<dyn Fn(&V, &Store) -> Option<T> + Send + Sync>,
    hover: Hover<T>,
}

impl<V, T> TooltipView<V, T>
where
    V: View,
    T: View<Command = Infallible>,
{
    /// `provide` answers the tip when the pointer settles on the
    /// view — `None` for a view with nothing to say.
    pub fn new(view: V, provide: impl Fn(&V, &Store) -> Option<T> + Send + Sync + 'static) -> Self {
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
            TooltipCommand::Entered => {
                if let (Hover::Idle, Some(tip)) = (&self.hover, (self.provide)(&self.view, store)) {
                    self.hover = Hover::Arming {
                        tip,
                        rested: 0.0,
                        last: None,
                    };
                }
            }
            TooltipCommand::Left => self.hover = Hover::Idle,
            TooltipCommand::Tick(now) => {
                if let Hover::Arming { tip, rested, last } =
                    std::mem::replace(&mut self.hover, Hover::Idle)
                {
                    let rested = rested
                        + last
                            .map(|last| now.millis_since(last) as f32)
                            .unwrap_or(0.0);
                    self.hover = match rested >= HOVER_DELAY_MS {
                        true => Hover::Shown { tip },
                        false => Hover::Arming {
                            tip,
                            rested,
                            last: Some(now),
                        },
                    };
                }
            }
        }
    }

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w crate::ui::UiCtx,
    ) -> crate::focus::FocusData<'w, Self::Command> {
        self.view.focus_data(store, ui).map(TooltipCommand::Host)
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a crate::ui::UiCtx,
    ) -> impl crate::layout::Layout<'a, Self::Command> + crate::layout::LayoutValue + 'a {
        crate::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let active = !matches!(self.hover, Hover::Idle);
            let armed = matches!(self.hover, Hover::Arming { .. });
            let inner = crate::layout::Layout::layout(
                self.view.display(arena, store, ui),
                arena,
                constraints,
            )
            .map(TooltipCommand::Host)
            .event(move |_arena, event, size| match event {
                Event::HitTest { point, miss, .. } => {
                    let inside = !miss && Rect::from_size(size).contains(*point);
                    match (inside, active) {
                        (true, false) => EventResult::Command(TooltipCommand::Entered),
                        (false, true) => EventResult::Command(TooltipCommand::Left),
                        _ => EventResult::Ignored,
                    }
                }
                Event::AnimationClock { now } if armed => {
                    EventResult::Command(TooltipCommand::Tick(*now))
                }
                _ => EventResult::Ignored,
            });
            let Hover::Shown { tip } = &self.hover else {
                return ThunkBox::new(arena, inner);
            };
            ThunkBox::new(
                arena,
                inner.overlay(crate::overlay::WINDOW, move |host: Size, anchor: Rect| {
                    let content = ThunkBox::new(
                        arena,
                        crate::layout::Layout::layout(
                            tip.display(arena, store, ui),
                            arena,
                            Constraints::tight(host).loosen(),
                        )
                        .map(|never: Infallible| match never {}),
                    );
                    let origin = Placement::Beside.origin(host, anchor, content.size());
                    vec![(origin, content)]
                }),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::AnimationClock;
    use crate::leaf::leaf;
    use crate::Widget;

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
        fn display<'a>(
            &'a self,
            _arena: &'a Arena,
            _store: &'a Store,
            _ui: &'a crate::ui::UiCtx,
        ) -> impl crate::layout::Layout<'a, u32> + crate::layout::LayoutValue + 'a {
            crate::layout::laid(move |_arena: &'a Arena, _constraints: Constraints| {
                leaf(200.0, 40.0)
            })
        }
    }

    #[derive(Clone)]
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
        fn display<'a>(
            &'a self,
            _arena: &'a Arena,
            _store: &'a Store,
            _ui: &'a crate::ui::UiCtx,
        ) -> impl crate::layout::Layout<'a, Infallible> + crate::layout::LayoutValue + 'a {
            crate::layout::laid(move |_arena: &'a Arena, _constraints: Constraints| {
                leaf(60.0, 24.0)
            })
        }
    }

    fn drive(view: &mut TooltipView<Body, Tip>, event: Event<'_>) {
        let ui = crate::ui::UiCtx::dont_use_too_slow();
        let commands = {
            let arena = Arena::default();
            let store = Store::new();
            let widget = crate::layout::Layout::layout(
                view.display(&arena, &store, &ui),
                &arena,
                Constraints::tight(Size::new(200.0, 40.0)),
            )
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
        let ui = crate::ui::UiCtx::dont_use_too_slow();
        let mut widget = crate::layout::Layout::layout(
            view.display(&arena, &store, &ui),
            &arena,
            Constraints::tight(Size::new(200.0, 40.0)),
        )
        .realize(&arena, Rect::from_wh(200.0, 40.0));
        let count = widget.overlays().len();
        drop(widget);
        count
    }

    fn hit(x: f32, y: f32) -> Event<'static> {
        Event::HitTest {
            point: skia_safe::Point::new(x, y),
            miss: false,
            mods: Default::default(),
        }
    }

    fn rest(view: &mut TooltipView<Body, Tip>) {
        for ms in [0.0, 500.0] {
            drive(
                view,
                Event::AnimationClock {
                    now: AnimationClock::from_millis(ms),
                },
            );
        }
    }

    #[test]
    fn a_rested_hover_shows_and_leaving_hides() {
        let mut view = TooltipView::new(Body, |_: &Body, _: &Store| Some(Tip));
        drive(&mut view, hit(50.0, 10.0));
        assert!(!view.showing(), "arming, not shown");
        assert_eq!(overlay_count(&view), 0);
        rest(&mut view);
        assert!(view.showing(), "the rest showed the tip");
        assert_eq!(overlay_count(&view), 1, "the tip is an overlay on the view");

        drive(&mut view, hit(500.0, 300.0));
        assert!(!view.showing(), "leaving hides");
        assert_eq!(overlay_count(&view), 0);
    }

    #[test]
    fn a_mouse_leaving_the_window_hides_the_tip() {
        let mut view = TooltipView::new(Body, |_: &Body, _: &Store| Some(Tip));
        drive(&mut view, hit(50.0, 10.0));
        rest(&mut view);
        assert!(view.showing());
        // The shells' cursor-left road: a HitTest beyond any
        // component's reach.
        drive(&mut view, Event::window_left());
        assert!(!view.showing(), "the off-window miss hid the tip");
    }

    #[test]
    fn a_view_with_nothing_to_say_never_arms() {
        let mut view = TooltipView::new(Body, |_: &Body, _: &Store| None);
        drive(&mut view, hit(50.0, 10.0));
        rest(&mut view);
        assert!(!view.showing());
        assert_eq!(overlay_count(&view), 0);
    }
}
