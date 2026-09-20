// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::any::Any;
use std::hash::Hash;

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::{AnyEffect, CancellationToken, Effect, EffectHandler, Effects},
    event::{Event, EventResult, Key as InputKey},
    list::SearchableList,
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View,
};
use skia_safe::{Paint, Rect, Size};

use editor::EditorView;

pub type ItemSource<K> = Box<dyn FnOnce() -> Vec<(String, K)> + Send + Sync>;

pub trait Searcher: Clone + Send + Sync + 'static {
    type View: View;
    type Key: Clone + Eq + Hash + Send + Sync + 'static;

    fn capture(&self, view: &Self::View) -> ItemSource<Self::Key>;

    fn generation(&self, view: &Self::View) -> u64;
}

pub fn subsequence_match(haystack: &str, term: &str) -> bool {
    let mut chars = term.chars();
    let mut wanted = chars.next();
    for present in haystack.chars() {
        match wanted {
            Some(next) if next.eq_ignore_ascii_case(&present) => wanted = chars.next(),
            Some(_) => {}
            None => break,
        }
    }
    wanted.is_none()
}

pub struct SpeedSearchEffect {
    run: Box<dyn FnOnce() -> Box<dyn Any + Send + Sync> + Send + Sync>,
    stamp: (String, u64),
}

pub struct SpeedSearchMatches {
    pub(crate) matched: Box<dyn Any + Send + Sync>,
    pub(crate) stamp: (String, u64),
}

impl Effect for SpeedSearchEffect {
    type Result = SpeedSearchMatches;
}

pub struct SpeedSearchHandler;

impl EffectHandler<SpeedSearchEffect> for SpeedSearchHandler {
    async fn handle(&self, effect: SpeedSearchEffect) -> SpeedSearchMatches {
        SpeedSearchMatches {
            matched: (effect.run)(),
            stamp: effect.stamp,
        }
    }
}

pub enum SpeedSearchCommand<C> {
    Inner(C),

    Input(::editor::EditorCommand),

    Landed(SpeedSearchMatches),

    Step(isize),

    Clear,

    Refresh,
}

pub struct SpeedSearchView<T, S>
where
    T: View + SearchableList<S::Key>,
    S: Searcher<View = T>,
{
    inner: T,
    searcher: S,
    input: EditorView,
    lane: Option<CancellationToken>,

    launched: Option<(String, u64)>,
}

impl<T, S> Clone for SpeedSearchView<T, S>
where
    T: View + SearchableList<S::Key> + Clone,
    S: Searcher<View = T>,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            searcher: self.searcher.clone(),
            input: self.input.clone(),
            lane: self.lane,
            launched: self.launched.clone(),
        }
    }
}

impl<T, S> SpeedSearchView<T, S>
where
    T: View + SearchableList<S::Key>,
    S: Searcher<View = T>,
{
    pub fn new(
        inner: T,
        searcher: S,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fonts: crate::FontSource,
    ) -> Self {
        let mut input = EditorView::input(PILL_INPUT_WIDTH, store, ui, fonts);
        input.focus_text();
        Self {
            inner,
            searcher,
            input,
            lane: None,
            launched: None,
        }
    }

    pub fn inner(&self) -> &T {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    pub fn searching(&self) -> bool {
        !self.query().is_empty()
    }

    pub fn clear(
        &mut self,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fx: &mut Effects<'_, SpeedSearchCommand<T::Command>>,
    ) where
        T::Command: Send + 'static,
    {
        self.input = EditorView::input(PILL_INPUT_WIDTH, store, ui, crate::embedded_fonts::source());
        self.input.focus_text();
        self.launched = None;
        if let Some(token) = self.lane.take() {
            fx.cancel(token);
        }
        self.inner.clear_matches();
    }

    pub fn query(&self) -> String {
        let mut view = self.input.document.text().view();
        let byte_count = view.byte_count();
        view.byte_string(0, byte_count)
    }

    fn relaunch(&mut self, fx: &mut Effects<'_, SpeedSearchCommand<T::Command>>)
    where
        T::Command: Send + 'static,
    {
        let query = self.query();
        if query.is_empty() {
            self.launched = None;
            if let Some(token) = self.lane.take() {
                fx.cancel(token);
            }
            self.inner.clear_matches();
            return;
        }
        let generation = self.searcher.generation(&self.inner);
        let stamp = (query.clone(), generation);
        if self.launched.as_ref() == Some(&stamp) {
            return;
        }
        self.launched = Some(stamp.clone());
        let source = self.searcher.capture(&self.inner);
        let effect = SpeedSearchEffect {
            run: Box::new(move || {
                let term = query.to_lowercase();
                let matched: Vec<S::Key> = source()
                    .into_iter()
                    .filter(|(label, _)| subsequence_match(label, &term))
                    .map(|(_, key)| key)
                    .collect();
                Box::new(matched)
            }),
            stamp,
        };
        fx.relaunch_erased(
            &mut self.lane,
            AnyEffect::new(effect).map(SpeedSearchCommand::Landed),
        );
    }
}

const PILL_INPUT_WIDTH: f32 = 220.0;

impl<T, S> View for SpeedSearchView<T, S>
where
    T: View + SearchableList<S::Key> + Clone,
    T::Command: Send + 'static,
    S: Searcher<View = T>,
{
    type Command = SpeedSearchCommand<T::Command>;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        use imba::event::EventResult;
        use imba::focus::FocusData;
        let inner = self
            .inner
            .focus_data(store, ui)
            .map(SpeedSearchCommand::Inner);
        // The INPUT is always seated — typing is what STARTS a
        // search; only the stepping keys wait for a live query. A
        // closed search keeps its input commands out of the palette.
        let searching = self.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                imba::event::Key::Up if searching => {
                    EventResult::Command(SpeedSearchCommand::Step(-1))
                }
                imba::event::Key::Down if searching => {
                    EventResult::Command(SpeedSearchCommand::Step(1))
                }
                imba::event::Key::Escape if searching => {
                    EventResult::Command(SpeedSearchCommand::Clear)
                }
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        let mut input = self
            .input
            .focus_data(store, ui)
            .map(SpeedSearchCommand::Input);
        if !searching {
            input.commands = Vec::new();
        }
        own.merge_under(input).merge_under(inner)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        // Teardown-only: `View::destroy` carries no UiCtx.
        let ui = &imba::UiCtx::dont_use_too_slow();
        self.clear(store, ui, fx);
        fx.scope(SpeedSearchCommand::Inner, |fx| {
            self.inner.destroy(store, fx)
        });
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            SpeedSearchCommand::Inner(command) => fx.scope(SpeedSearchCommand::Inner, |fx| {
                self.inner.perform(store, ui, command, fx)
            }),
            SpeedSearchCommand::Input(command) => {
                fx.scope(SpeedSearchCommand::Input, |fx| {
                    self.input.perform(store, ui, command, fx)
                });
                self.relaunch(fx);
            }
            SpeedSearchCommand::Landed(landing) => {
                if self.launched.as_ref() != Some(&landing.stamp) {
                    return;
                }
                self.lane = None;
                let Ok(matched) = landing.matched.downcast::<Vec<S::Key>>() else {
                    return;
                };
                self.inner.set_matches(&matched);

                self.inner.step_matched(0);
            }
            SpeedSearchCommand::Step(delta) => self.inner.step_matched(delta),
            SpeedSearchCommand::Clear => self.clear(store, ui, fx),
            SpeedSearchCommand::Refresh => self.relaunch(fx),
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let mut root = imba::container::container(arena, size);
            let inner =
                imba::Layout::layout(self.inner.display(arena, store, ui), arena, constraints)
                    .map(SpeedSearchCommand::Inner);
            root.place(0.0, 0.0, inner);

            let searching = self.searching();
            let chrome = crate::env::Themes::of(store).ui().peeker.clone();
            // The pill wraps the input editor's TRUE line height — the
            // chrome never clips the text it hosts.
            let input_height = self.input.content_height().max(1.0);
            let pill_height = (input_height + 6.0).max(chrome.row_height * 0.75);

            let input = imba::Layout::layout(
                self.input.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(PILL_INPUT_WIDTH, input_height)),
            )
            .map(SpeedSearchCommand::Input)
            .wrap(move |inner| ChainGate {
                inner,
                open: searching,
            });
            let pill_width = PILL_INPUT_WIDTH + 16.0;
            let mut pill = imba::container::container(arena, Size::new(pill_width, pill_height));
            if searching {
                let fill = chrome.background.0;
                let rule = chrome.rule.0;
                let backdrop =
                    imba::leaf::leaf::<SpeedSearchCommand<T::Command>>(pill_width, pill_height)
                        .paint_instead(move |_arena, canvas, rect| {
                            let mut paint = Paint::default();
                            paint.set_anti_alias(true);
                            paint.set_color(fill);
                            canvas.draw_round_rect(rect, 6.0, 6.0, &paint);
                            let mut edge = Paint::default();
                            edge.set_anti_alias(true);
                            edge.set_color(rule);
                            edge.set_style(skia_safe::paint::Style::Stroke);
                            canvas.draw_round_rect(
                                Rect::from_xywh(
                                    rect.left + 0.5,
                                    rect.top + 0.5,
                                    rect.width() - 1.0,
                                    rect.height() - 1.0,
                                ),
                                6.0,
                                6.0,
                                &edge,
                            );
                        });
                pill.place(0.0, 0.0, backdrop);
            }
            pill.place(8.0, ((pill_height - input_height) * 0.5).max(0.0), input);
            root.place((size.width - pill_width - 8.0).max(0.0), 4.0, pill);

            let stale = searching
                && self.launched.as_ref().is_some_and(|(query, generation)| {
                    *generation != self.searcher.generation(&self.inner) || *query != self.query()
                });
            let armed = searching;
            let keymap = imba::leaf::leaf::<SpeedSearchCommand<T::Command>>(
                size.width,
                size.height,
            )
            .event(move |_arena, event, _size| match event {
                Event::Paint { .. } if stale => EventResult::Command(SpeedSearchCommand::Refresh),
                Event::KeyDown {
                    key: InputKey::Up, ..
                } if armed => EventResult::Command(SpeedSearchCommand::Step(-1)),
                Event::KeyDown {
                    key: InputKey::Down,
                    ..
                } if armed => EventResult::Command(SpeedSearchCommand::Step(1)),
                Event::KeyDown {
                    key: InputKey::Escape,
                    ..
                } if armed => EventResult::Command(SpeedSearchCommand::Clear),
                _ => EventResult::Ignored,
            });
            root.place(0.0, 0.0, keymap);
            root
        })
    }
}

struct ChainGate<Inner> {
    inner: Inner,
    open: bool,
}

impl<'a, Inner, Command: 'a> imba::Widget<'a, Command> for ChainGate<Inner>
where
    Inner: imba::Widget<'a, Command>,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, Command>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        match event {
            Event::Paint { canvas, focused } => self.inner.handle_event(
                arena,
                &Event::Paint {
                    canvas,
                    focused: *focused && self.open,
                },
                viewport,
            ),
            _ => self.inner.handle_event(arena, event, viewport),
        }
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, Command>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    struct PaintFocusProbe<'a>(&'a Cell<Option<bool>>);

    impl<'a> imba::Widget<'a, ()> for PaintFocusProbe<'_> {
        fn size(&self) -> Size {
            Size::default()
        }

        fn handle_event(
            &self,
            _arena: &Arena,
            event: &Event<'_>,
            _viewport: Rect,
        ) -> EventResult<()> {
            if let Event::Paint { focused, .. } = event {
                self.0.set(Some(*focused));
            }
            EventResult::Handled
        }
    }

    #[test]
    fn speed_search_input_paints_focused_only_while_a_query_stands() {
        let arena = Arena::default();
        let mut surface = skia_safe::surfaces::raster_n32_premul((1, 1)).expect("surface");
        let observed = Cell::new(None);
        let idle = ChainGate {
            inner: PaintFocusProbe(&observed),
            open: false,
        };

        let _ = imba::Widget::handle_event(
            &idle,
            &arena,
            &Event::Paint {
                canvas: surface.canvas(),
                focused: true,
            },
            Rect::from_wh(1.0, 1.0),
        );

        assert_eq!(observed.get(), Some(false));

        observed.set(None);
        let searching = ChainGate {
            inner: PaintFocusProbe(&observed),
            open: true,
        };
        let _ = imba::Widget::handle_event(
            &searching,
            &arena,
            &Event::Paint {
                canvas: surface.canvas(),
                focused: true,
            },
            Rect::from_wh(1.0, 1.0),
        );

        assert_eq!(observed.get(), Some(true));
    }
}
