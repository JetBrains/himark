// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! THE keyboard layer for list and tree surfaces
//! (docs/ui/list-keyboard.md): the arrow/Enter/Escape/fold table
//! declared once, speed-search folded in as an OPTION (`Searcher`
//! supplied). Every selection edit it initiates descends as
//! `ListCommand::Select(index)`; activation as
//! `ListCommand::Activate(index, trigger)` — the surface reacts by
//! inspecting the command stream it already routes.

use std::any::Any;
use std::hash::Hash;

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::{AnyEffect, CancellationToken, Effect, EffectHandler, Effects},
    event::{Event, EventResult, Key as InputKey},
    list::{Edge, ListOps},
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View,
};
use skia_safe::{Paint, Rect, Size};

use editor::EditorView;

pub use imba::list::ActivateTrigger;

pub type ItemSource<K> = Box<dyn FnOnce() -> Vec<(String, K)> + Send + Sync>;

pub trait Searcher: Clone + Send + Sync + 'static {
    type View: View;
    type Key: Clone + Eq + Hash + Send + Sync + 'static;

    fn capture(&self, view: &Self::View) -> ItemSource<Self::Key>;

    fn generation(&self, view: &Self::View) -> u64;
}

/// The keys-only default: a controller without a search lane never
/// captures or launches anything; this type exists so `S` has a
/// well-formed stand-in (`fn() -> T` keeps it Send+Sync regardless
/// of the view).
pub struct NoSearcher<T>(std::marker::PhantomData<fn() -> T>);

impl<T> Clone for NoSearcher<T> {
    fn clone(&self) -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T> Searcher for NoSearcher<T>
where
    T: ListOps + 'static,
{
    type View = T;
    type Key = T::Key;

    fn capture(&self, _view: &T) -> ItemSource<T::Key> {
        Box::new(Vec::new)
    }

    fn generation(&self, _view: &T) -> u64 {
        0
    }
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

/// The landing's selection ride: a `perform` cannot hand a command
/// back up the tree, so the first-match `Select` (and nothing else)
/// round-trips through this always-ready effect and descends from
/// the root — the surface sees it like every other selection edit.
pub struct AnnounceSelect;

impl Effect for AnnounceSelect {
    type Result = ();
}

pub struct AnnounceSelectHandler;

impl EffectHandler<AnnounceSelect> for AnnounceSelectHandler {
    async fn handle(&self, _effect: AnnounceSelect) {}
}

pub enum ListKeyCommand<C> {
    Inner(C),

    /// Left/Right on the cursor row. Fold stays a SURFACE-handled
    /// command, uniformly — the controller carries no fold policy
    /// (the eager trees answer with `Forest::fold_cursor`, the lazy
    /// ones with their own states).
    Fold { index: usize, expand: bool },

    // — the search lane, only reachable with a Searcher: —
    Input(::editor::EditorCommand),

    Landed(SpeedSearchMatches),

    Clear,

    Refresh,
}

struct SearchLane<S> {
    searcher: S,
    /// The SHADOW input: a real one-line input editor (value host,
    /// own document) — real because typing must be REAL typing: IME
    /// composition, dead keys, backspace, clipboard all come free.
    /// Renders as a compact query pill only while non-empty.
    input: EditorView,
    /// THE filter lane: one in-flight run, a newer query supersedes
    /// (fx.relaunch — ask-answer never conflates).
    lane: Option<CancellationToken>,
    /// The launch guard stamp: (query, items generation).
    launched: Option<(String, u64)>,
}

impl<S: Clone> Clone for SearchLane<S> {
    fn clone(&self) -> Self {
        Self {
            searcher: self.searcher.clone(),
            input: self.input.clone(),
            lane: self.lane,
            launched: self.launched.clone(),
        }
    }
}

pub struct ListKeyboardController<T, S = NoSearcher<T>>
where
    T: ListOps,
    S: Searcher<View = T, Key = T::Key>,
{
    inner: T,
    /// Left/Right emit `Fold` only for surfaces that opted in — on a
    /// flat list the keys stay unconsumed (a combo's input keeps its
    /// caret motion).
    folds: bool,
    search: Option<SearchLane<S>>,
}

impl<T, S> Clone for ListKeyboardController<T, S>
where
    T: ListOps + Clone,
    S: Searcher<View = T, Key = T::Key>,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            folds: self.folds,
            search: self.search.clone(),
        }
    }
}

impl<T> ListKeyboardController<T, NoSearcher<T>>
where
    T: ListOps + 'static,
{
    /// Keys only: the table without a pill, a lane or a stamp.
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            folds: false,
            search: None,
        }
    }
}

impl<T, S> ListKeyboardController<T, S>
where
    T: ListOps,
    S: Searcher<View = T, Key = T::Key>,
{
    /// Keys plus the speed-search lane.
    pub fn searchable(
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
            folds: false,
            search: Some(SearchLane {
                searcher,
                input,
                lane: None,
                launched: None,
            }),
        }
    }

    /// Trees opt into Left/Right emitting `Fold`.
    pub fn with_folds(mut self) -> Self {
        self.folds = true;
        self
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

    pub fn query(&self) -> String {
        let Some(lane) = &self.search else {
            return String::new();
        };
        let mut view = lane.input.document.text().view();
        let byte_count = view.byte_count();
        view.byte_string(0, byte_count)
    }

    pub fn clear(
        &mut self,
        store: &imba::store::Store,
        ui: &imba::UiCtx,
        fx: &mut Effects<'_, ListKeyCommand<T::Command>>,
    ) where
        T::Command: Send + 'static,
    {
        let Some(lane) = &mut self.search else {
            return;
        };
        lane.input =
            EditorView::input(PILL_INPUT_WIDTH, store, ui, crate::embedded_fonts::source());
        lane.input.focus_text();
        lane.launched = None;
        if let Some(token) = lane.lane.take() {
            fx.cancel(token);
        }
        self.inner.clear_matches();
    }

    fn relaunch(&mut self, fx: &mut Effects<'_, ListKeyCommand<T::Command>>)
    where
        T::Command: Send + 'static,
    {
        let query = self.query();
        let Some(lane) = &mut self.search else {
            return;
        };
        if query.is_empty() {
            lane.launched = None;
            if let Some(token) = lane.lane.take() {
                fx.cancel(token);
            }
            self.inner.clear_matches();
            return;
        }
        let generation = lane.searcher.generation(&self.inner);
        let stamp = (query.clone(), generation);
        if lane.launched.as_ref() == Some(&stamp) {
            return;
        }
        lane.launched = Some(stamp.clone());
        let source = lane.searcher.capture(&self.inner);
        let effect = SpeedSearchEffect {
            run: Box::new(move || {
                let term = query.to_lowercase();
                let matched: Vec<T::Key> = source()
                    .into_iter()
                    .filter(|(label, _)| subsequence_match(label, &term))
                    .map(|(_, key)| key)
                    .collect();
                Box::new(matched)
            }),
            stamp,
        };
        fx.relaunch_erased(
            &mut lane.lane,
            AnyEffect::new(effect).map(ListKeyCommand::Landed),
        );
    }

    /// THE key table — one declaration, served to both `focus_data`
    /// and the keymap leaf the controller's `display` plants.
    fn table_key(&self, key: InputKey) -> EventResult<ListKeyCommand<T::Command>> {
        let searching = self.searching();
        let select = |index: Option<usize>| match index {
            Some(index) => {
                EventResult::Command(ListKeyCommand::Inner(self.inner.select_command(index)))
            }
            // The panel owns its movement keys even with nothing to
            // move to — they never leak to what sits behind it.
            None => EventResult::Handled,
        };
        match key {
            InputKey::Up if searching => select(self.inner.matched_step_index(-1)),
            InputKey::Down if searching => select(self.inner.matched_step_index(1)),
            InputKey::Up => select(self.inner.step_index(-1)),
            InputKey::Down => select(self.inner.step_index(1)),
            InputKey::Home if searching => select(self.inner.matched_edge_index(Edge::First)),
            InputKey::End if searching => select(self.inner.matched_edge_index(Edge::Last)),
            InputKey::Home => select(self.inner.edge_index(Edge::First)),
            InputKey::End => select(self.inner.edge_index(Edge::Last)),
            InputKey::PageUp => select(self.inner.page_index(-1)),
            InputKey::PageDown => select(self.inner.page_index(1)),
            InputKey::Enter => match self.inner.cursor_index() {
                Some(index) => EventResult::Command(ListKeyCommand::Inner(
                    self.inner
                        .activate_command(index, ActivateTrigger::Enter),
                )),
                None => EventResult::Handled,
            },
            InputKey::Escape if searching => EventResult::Command(ListKeyCommand::Clear),
            InputKey::Left | InputKey::Right if self.folds => match self.inner.cursor_index() {
                Some(index) => EventResult::Command(ListKeyCommand::Fold {
                    index,
                    expand: key == InputKey::Right,
                }),
                None => EventResult::Handled,
            },
            _ => EventResult::Ignored,
        }
    }
}

const PILL_INPUT_WIDTH: f32 = 220.0;

/// One more wrapper layer, one more line each: the controller
/// forwards the ops and peels its own `Inner` on inspection, so a
/// surface asks the CONTROLLER type about the commands it routes.
impl<T, S> ListOps for ListKeyboardController<T, S>
where
    T: ListOps + Clone,
    T::Command: Send + 'static,
    S: Searcher<View = T, Key = T::Key>,
{
    type Key = T::Key;

    fn set_matches(&mut self, keys: &[Self::Key]) {
        self.inner.set_matches(keys);
    }
    fn clear_matches(&mut self) {
        self.inner.clear_matches();
    }
    fn match_count(&self) -> usize {
        self.inner.match_count()
    }
    fn cursor_index(&self) -> Option<usize> {
        self.inner.cursor_index()
    }
    fn step_index(&self, delta: isize) -> Option<usize> {
        self.inner.step_index(delta)
    }
    fn matched_step_index(&self, delta: isize) -> Option<usize> {
        self.inner.matched_step_index(delta)
    }
    fn edge_index(&self, edge: Edge) -> Option<usize> {
        self.inner.edge_index(edge)
    }
    fn matched_edge_index(&self, edge: Edge) -> Option<usize> {
        self.inner.matched_edge_index(edge)
    }
    fn page_index(&self, direction: isize) -> Option<usize> {
        self.inner.page_index(direction)
    }
    fn select_command(&self, index: usize) -> Self::Command {
        ListKeyCommand::Inner(self.inner.select_command(index))
    }
    fn activate_command(&self, index: usize, trigger: ActivateTrigger) -> Self::Command {
        ListKeyCommand::Inner(self.inner.activate_command(index, trigger))
    }
    fn selected_index(command: &Self::Command) -> Option<usize> {
        match command {
            ListKeyCommand::Inner(inner) => T::selected_index(inner),
            _ => None,
        }
    }
    fn activated(command: &Self::Command) -> Option<(usize, ActivateTrigger)> {
        match command {
            ListKeyCommand::Inner(inner) => T::activated(inner),
            _ => None,
        }
    }
}

impl<T, S> View for ListKeyboardController<T, S>
where
    T: ListOps + Clone,
    T::Command: Send + 'static,
    S: Searcher<View = T, Key = T::Key>,
{
    type Command = ListKeyCommand<T::Command>;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        use imba::focus::FocusData;
        let inner = self.inner.focus_data(store, ui).map(ListKeyCommand::Inner);
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| self.table_key(key))),
            ..FocusData::default()
        };
        let Some(lane) = &self.search else {
            return own.merge_under(inner);
        };
        // The INPUT is always seated — typing is what STARTS a
        // search; only a live query keeps its commands in the
        // palette.
        let searching = self.searching();
        let mut input = lane.input.focus_data(store, ui).map(ListKeyCommand::Input);
        if !searching {
            input.commands = Vec::new();
        }
        own.merge_under(input).merge_under(inner)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        // Teardown-only: `View::destroy` carries no UiCtx.
        let ui = &imba::UiCtx::dont_use_too_slow();
        self.clear(store, ui, fx);
        fx.scope(ListKeyCommand::Inner, |fx| self.inner.destroy(store, fx));
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            ListKeyCommand::Inner(command) => fx.scope(ListKeyCommand::Inner, |fx| {
                self.inner.perform(store, ui, command, fx)
            }),
            // Fold is the surface's — nothing to do at this level.
            ListKeyCommand::Fold { .. } => {}
            ListKeyCommand::Input(command) => {
                if let Some(lane) = &mut self.search {
                    fx.scope(ListKeyCommand::Input, |fx| {
                        lane.input.perform(store, ui, command, fx)
                    });
                    self.relaunch(fx);
                }
            }
            ListKeyCommand::Landed(landing) => {
                let Some(lane) = &mut self.search else {
                    return;
                };
                if lane.launched.as_ref() != Some(&landing.stamp) {
                    return;
                }
                lane.lane = None;
                let Ok(matched) = landing.matched.downcast::<Vec<T::Key>>() else {
                    return;
                };
                self.inner.set_matches(&matched);
                // The first-match jump descends from the root like
                // every other Select — the surface sees it.
                if let Some(index) = self.inner.matched_step_index(0) {
                    let command = ListKeyCommand::Inner(self.inner.select_command(index));
                    fx.push(AnyEffect::new(AnnounceSelect).map(move |_| command));
                }
            }
            ListKeyCommand::Clear => self.clear(store, ui, fx),
            ListKeyCommand::Refresh => self.relaunch(fx),
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
                    .map(ListKeyCommand::Inner);
            root.place(0.0, 0.0, inner);

            let searching = self.searching();

            if let Some(lane) = &self.search {
                let chrome = crate::env::Themes::of(store).ui().peeker.clone();
                // The pill wraps the input editor's TRUE line height —
                // the chrome never clips the text it hosts.
                let input_height = lane.input.content_height().max(1.0);
                let pill_height = (input_height + 6.0).max(chrome.row_height * 0.75);

                let input = imba::Layout::layout(
                    lane.input.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(PILL_INPUT_WIDTH, input_height)),
                )
                .map(ListKeyCommand::Input)
                .wrap(move |inner| ChainGate {
                    inner,
                    open: searching,
                });
                let pill_width = PILL_INPUT_WIDTH + 16.0;
                let mut pill =
                    imba::container::container(arena, Size::new(pill_width, pill_height));
                if searching {
                    let fill = chrome.background.0;
                    let rule = chrome.rule.0;
                    let backdrop =
                        imba::leaf::leaf::<Self::Command>(pill_width, pill_height).paint_instead(
                            move |_arena, canvas, rect| {
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
                            },
                        );
                    pill.place(0.0, 0.0, backdrop);
                }
                pill.place(8.0, ((pill_height - input_height) * 0.5).max(0.0), input);
                root.place((size.width - pill_width - 8.0).max(0.0), 4.0, pill);
            }

            let stale = searching
                && self.search.as_ref().is_some_and(|lane| {
                    lane.launched.as_ref().is_some_and(|(query, generation)| {
                        *generation != lane.searcher.generation(&self.inner)
                            || *query != self.query()
                    })
                });
            // The ONE keymap overlay — the same table `focus_data`
            // answers, for surfaces whose keys arrive down the widget
            // tree; the per-surface duplicates are gone.
            let keymap = imba::leaf::leaf::<Self::Command>(size.width, size.height).event(
                move |_arena, event, _size| match event {
                    Event::Paint { .. } if stale => {
                        EventResult::Command(ListKeyCommand::Refresh)
                    }
                    Event::KeyDown { key, .. } => self.table_key(*key),
                    _ => EventResult::Ignored,
                },
            );
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
