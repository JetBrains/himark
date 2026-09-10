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
    Thunk, UiCtx, View,
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
    pub fn new(inner: T, searcher: S, fonts: crate::FontSource) -> Self {
        let mut input = EditorView::input(PILL_INPUT_WIDTH, fonts);
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

    pub fn clear(&mut self, fx: &mut Effects<'_, SpeedSearchCommand<T::Command>>)
    where
        T::Command: Send + 'static,
    {
        self.input = EditorView::input(PILL_INPUT_WIDTH, crate::embedded_fonts::source());
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

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        self.clear(fx);
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
            SpeedSearchCommand::Clear => self.clear(fx),
            SpeedSearchCommand::Refresh => self.relaunch(fx),
        }
    }

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let size = constraints.max;
        let mut root = imba::container::container(arena, size);
        let inner = self
            .inner
            .layout(arena, store, ui, constraints)
            .map(SpeedSearchCommand::Inner);
        root.place(0.0, 0.0, inner);

        let searching = self.searching();
        let chrome = crate::env::Themes::of(store).ui().peeker.clone();
        let pill_height = chrome.row_height * 0.75;

        let input = self
            .input
            .layout(
                arena,
                store,
                ui,
                Constraints::tight(Size::new(PILL_INPUT_WIDTH, pill_height)),
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
        pill.place(8.0, 0.0, input);
        root.place((size.width - pill_width - 8.0).max(0.0), 4.0, pill);

        let stale = searching
            && self.launched.as_ref().is_some_and(|(query, generation)| {
                *generation != self.searcher.generation(&self.inner) || *query != self.query()
            });
        let armed = searching;
        let keymap = imba::leaf::leaf::<SpeedSearchCommand<T::Command>>(size.width, size.height)
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

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, Command>
    where
        'a: 'w,
    {
        let mut data = self.inner.focus_data();
        if !self.open {
            data.commands = Vec::new();
        }
        data
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
