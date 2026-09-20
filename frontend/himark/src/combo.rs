// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult, Key, MouseButton},
    leaf::leaf,
    list::{ListCommand, ListSlice, ListView},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View,
};
use skia_safe::{Font, Paint, PathBuilder, Point, Rect, Size};

use crate::speedsearch::{ItemSource, Searcher, SpeedSearchCommand, SpeedSearchView};
use ::editor::theme::ComboChrome;

const MENU_MAX_ROWS: usize = 9;

pub trait ComboItem: View + Clone + Send + Sync + 'static {
    fn id(&self) -> &str;

    fn cell_label(&self) -> String;

    fn search_label(&self) -> String {
        self.cell_label()
    }

    fn selectable(&self) -> bool {
        true
    }
}

fn measured<T: ComboItem>(item: &T, store: &Store, ui: &UiCtx) -> Size
where
    T::Command: Send + 'static,
{
    let arena = imba::arena::Arena::default();
    let size = imba::Layout::layout(
        item.display(&arena, store, ui),
        &arena,
        Constraints {
            min: Size::default(),
            max: Size::new(f32::INFINITY, f32::INFINITY),
        },
    )
    .size();
    size
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComboOption {
    pub id: String,
    pub label: String,

    pub trail: Option<String>,
}

impl ComboOption {
    pub fn plain(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            trail: None,
        }
    }
}

impl View for ComboOption {
    type Command = std::convert::Infallible;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        command: Self::Command,
        _fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {}
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        let mut row = crate::ui::ListRow::new(arena, crate::ui::RowStyle::standard(store, ui))
            .label(self.label.clone());
        if let Some(trail) = &self.trail {
            row = row.trail(trail.clone());
        }
        row
    }
}

impl ComboItem for ComboOption {
    fn id(&self) -> &str {
        &self.id
    }

    fn cell_label(&self) -> String {
        self.label.clone()
    }
}

type OptionList<T> = ScrollView<ListView<T, String>>;

pub struct OptionSearcher<T>(std::marker::PhantomData<fn() -> T>);

impl<T> Clone for OptionSearcher<T> {
    fn clone(&self) -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T> Default for OptionSearcher<T> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T> Searcher for OptionSearcher<T>
where
    T: ComboItem,
    T::Command: Send + 'static,
{
    type View = OptionList<T>;
    type Key = String;

    fn capture(&self, view: &Self::View) -> ItemSource<String> {
        let list = view.content().clone();
        Box::new(move || {
            let mut items = Vec::new();
            for (index, row) in list.rows().enumerate() {
                if let Some(key) = list.key_at(index) {
                    items.push((row.search_label(), key.clone()));
                }
            }
            items
        })
    }

    fn generation(&self, view: &Self::View) -> u64 {
        view.content().generation()
    }
}

#[derive(Clone)]
pub struct Combo<T: ComboItem = ComboOption>
where
    T::Command: Send + 'static,
{
    pub label: &'static str,

    pub picked: usize,
    pub open: bool,
    menu: SpeedSearchView<OptionList<T>, OptionSearcher<T>>,
}

pub enum ComboCommand<C = std::convert::Infallible> {
    Open,
    Close,

    Select(isize),
    Pick(usize),

    PickCursor,

    Menu(Box<SpeedSearchCommand<ScrollCommand<ListCommand<C>>>>),
}

impl<C> ComboCommand<C> {
    pub fn picks(&self) -> bool {
        match self {
            ComboCommand::Pick(_) | ComboCommand::PickCursor => true,
            ComboCommand::Menu(command) => matches!(
                command.as_ref(),
                SpeedSearchCommand::Inner(ScrollCommand::Content(ListCommand::Focus(_, _)))
            ),
            _ => false,
        }
    }
}

impl<T: ComboItem> Combo<T>
where
    T::Command: Send + 'static,
{
    pub fn new(store: &imba::store::Store, ui: &imba::UiCtx, label: &'static str) -> Self {
        Self {
            label,
            picked: 0,
            open: false,
            menu: SpeedSearchView::new(
                ScrollView::new(
                    ListView::empty().with_selection(imba::list::SelectionStyle::default()),
                ),
                OptionSearcher::default(),
                store,
                ui,
                crate::embedded_fonts::source(),
            ),
        }
    }

    fn list(&self) -> &ListView<T, String> {
        self.menu.inner().content()
    }

    fn list_mut(&mut self) -> &mut ListView<T, String> {
        self.menu.inner_mut().content_mut()
    }

    pub fn set_options(&mut self, store: &Store, ui: &UiCtx, options: Vec<T>) {
        let kept = self.value().map(|old| old.id().to_owned());
        let mut slice = ListSlice::new();
        for option in options {
            let height = measured(&option, store, ui).height;
            let id = option.id().to_owned();
            if option.selectable() {
                slice.push_keyed_sized(id, option, height);
            } else {
                slice.push_sized(option, height);
            }
        }
        let len = self.list().len();
        self.list_mut().splice_slice(0..len, slice);
        self.list_mut()
            .set_selection_style(crate::rows::selection_style(store));
        let position = kept
            .and_then(|id| self.position_of(&id))
            .or_else(|| self.list().rows().position(|option| option.selectable()));
        self.picked = position
            .unwrap_or(0)
            .min(self.list().len().saturating_sub(1));
    }

    fn position_of(&self, id: &str) -> Option<usize> {
        self.list()
            .rows()
            .position(|option| option.selectable() && option.id() == id)
    }

    pub fn pick_id(&mut self, id: &str) {
        if let Some(index) = self.position_of(id) {
            self.picked = index;
        }
    }

    pub fn value(&self) -> Option<T> {
        self.list()
            .rows_from(self.picked)
            .next()
            .filter(|option| option.selectable())
    }

    pub fn options(&self) -> Vec<T> {
        self.list().rows().collect()
    }

    pub fn len(&self) -> usize {
        self.list().len()
    }

    pub fn is_empty(&self) -> bool {
        self.list().is_empty()
    }

    fn cursor_index(&self) -> Option<usize> {
        let key = self.list().cursor()?.clone();
        Some(self.list().row_range(&key)?.start)
    }

    pub fn cell_width(&self, ui: &UiCtx, chrome: &ComboChrome) -> f32 {
        let label_font = crate::fonts::ui_font(ui, chrome.label_size);
        let value_font = crate::fonts::ui_text_font(ui, chrome.value_size);
        let label = tracked_width(&label_font, self.label);
        let value = self
            .value()
            .map(|option| measured_plain(&value_font, &option.cell_label()))
            .unwrap_or(0.0);
        chrome.pad + label + chrome.gap + value + chrome.gap + chrome.chevron + chrome.pad
    }

    pub fn cell<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        height: f32,
    ) -> impl Thunk<'a, ComboCommand<T::Command>> + 'a {
        let themes = crate::env::Themes::of(store);
        let theme = themes.ui();
        let chrome = theme.combo.clone();

        let rule = chrome.menu_border.0;
        let width = self.cell_width(ui, &chrome);
        let label_font = crate::fonts::ui_font(ui, chrome.label_size);
        let value_font = crate::fonts::ui_text_font(ui, chrome.value_size);
        let label = self.label;
        let value = self.value().map(|option| option.cell_label());
        let open = self.open;

        let cell = leaf::<ComboCommand<T::Command>>(width, height)
            .paint_below({
                let chrome = chrome.clone();
                let label_font = label_font.clone();
                let value_font = value_font.clone();
                let value = value.clone();
                move |_arena, canvas, rect| {
                    let mut paint = Paint::default();
                    paint.set_anti_alias(false);
                    paint.set_color(rule);
                    canvas.draw_rect(
                        Rect::from_xywh(rect.right - 1.0, rect.top, 1.0, rect.height()),
                        &paint,
                    );
                    paint.set_anti_alias(true);

                    let mut x = rect.left + chrome.pad;
                    let mid = rect.top + rect.height() * 0.5;
                    paint.set_color(chrome.label_color.0);
                    x = draw_tracked(
                        canvas,
                        &label_font,
                        &paint,
                        label,
                        x,
                        mid + chrome.label_size * 0.35,
                    );
                    x += chrome.gap;
                    if let Some(value) = &value {
                        paint.set_color(chrome.value_color.0);
                        canvas.draw_str(
                            value.as_str(),
                            (x, mid + chrome.value_size * 0.35),
                            &value_font,
                            &paint,
                        );
                        x += value_font.measure_str(value.as_str(), None).0;
                    }
                    x += chrome.gap;
                    let mut chevron = Paint::default();
                    chevron.set_anti_alias(true);
                    chevron.set_stroke(true);
                    chevron.set_stroke_width(2.0);
                    chevron.set_color(chrome.chevron_color.0);
                    let half = chrome.chevron * 0.5;
                    let mut path = PathBuilder::new();
                    path.move_to((x, mid - half * 0.35));
                    path.line_to((x + half, mid + half * 0.4));
                    path.line_to((x + chrome.chevron, mid - half * 0.35));
                    canvas.draw_path(&path.detach(), &chevron);
                }
            })
            .event(|_arena, event, _size| match event {
                Event::MouseDown {
                    button: MouseButton::Left,
                    ..
                } => EventResult::Command(ComboCommand::Open),
                _ => EventResult::Ignored,
            });

        let thunk: imba::ThunkBox<'a, ComboCommand<T::Command>> = if open {
            let widest = self
                .list()
                .rows()
                .map(|option| measured(&option, store, ui).width)
                .fold(0.0f32, f32::max);
            let seed = MenuSeed {
                menu: &self.menu,
                store,
                ui,
                chrome,
                widest,
                rows: self.list().len(),
                searching: self.menu.searching(),
            };
            imba::ThunkBox::new(
                arena,
                cell.overlay(
                    imba::overlay::WINDOW,
                    move |host_size: Size, anchor: Rect| seed.layout(arena, host_size, anchor),
                ),
            )
        } else {
            imba::ThunkBox::new(arena, cell)
        };
        thunk
    }
}

impl<T: ComboItem> View for Combo<T>
where
    T::Command: Send + 'static,
{
    type Command = ComboCommand<T::Command>;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        use imba::focus::FocusData;
        let searching = self.menu.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                Key::Up if !searching => EventResult::Command(ComboCommand::Select(-1)),
                Key::Down if !searching => EventResult::Command(ComboCommand::Select(1)),
                Key::Enter if searching => EventResult::Commands(vec![
                    ComboCommand::PickCursor,
                    ComboCommand::Menu(Box::new(SpeedSearchCommand::Clear)),
                ]),
                Key::Enter => EventResult::Command(ComboCommand::PickCursor),
                Key::Escape if !searching => EventResult::Command(ComboCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(
            self.menu
                .focus_data(store, ui)
                .map(|command| ComboCommand::Menu(Box::new(command))),
        )
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            ComboCommand::Open => {
                self.list_mut()
                    .set_selection_style(crate::rows::selection_style(store));
                self.open = !self.is_empty();
                if !self.open {
                    return;
                }

                fx.scope(
                    |command| ComboCommand::Menu(Box::new(command)),
                    |fx| self.menu.clear(store, ui, fx),
                );
                if let Some(key) = self.list().key_at(self.picked).cloned() {
                    self.list_mut().select_only(key);
                    self.list_mut().cursor_step(0);
                }
            }
            ComboCommand::Close => {
                self.open = false;
            }
            ComboCommand::Select(delta) => {
                self.list_mut().cursor_step(delta);
            }
            ComboCommand::Pick(index) => {
                let selectable = index < self.len()
                    && self
                        .list()
                        .rows_from(index)
                        .next()
                        .is_some_and(|option| option.selectable());
                if selectable {
                    self.open = false;
                    self.picked = index;
                }
            }
            ComboCommand::PickCursor => {
                self.open = false;
                if let Some(index) = self.cursor_index() {
                    self.picked = index;
                }
            }
            ComboCommand::Menu(command) => {
                if let SpeedSearchCommand::Inner(ScrollCommand::Content(ListCommand::Focus(
                    index,
                    _,
                ))) = command.as_ref()
                {
                    let index = *index;
                    self.perform(store, ui, ComboCommand::Pick(index), fx);
                    return;
                }
                fx.scope(
                    |command| ComboCommand::Menu(Box::new(command)),
                    |fx| self.menu.perform(store, ui, *command, fx),
                );
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a imba::arena::Arena, constraints: Constraints| {
                self.cell(arena, store, ui, constraints.max.height)
            },
        )
    }
}

fn measured_tracked(font: &Font, text: &str) -> (f32, std::rc::Rc<[f32]>) {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static MEMO: RefCell<HashMap<u32, HashMap<String, (f32, std::rc::Rc<[f32]>)>>> =
            RefCell::new(HashMap::new());
    }
    MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        let by_text = memo.entry(font.size().to_bits()).or_default();
        if let Some(hit) = by_text.get(text) {
            return hit.clone();
        }
        let mut advances = Vec::new();
        let mut total = 0.0f32;
        for (at, ch) in text.char_indices() {
            let advance = font.measure_str(&text[at..at + ch.len_utf8()], None).0 + 1.5;
            advances.push(advance);
            total += advance;
        }
        let entry = (total, std::rc::Rc::from(advances));
        by_text.insert(text.to_owned(), entry.clone());
        entry
    })
}

pub(crate) fn tracked_width(font: &Font, text: &str) -> f32 {
    measured_tracked(font, text).0
}

fn measured_plain(font: &Font, text: &str) -> f32 {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static MEMO: RefCell<HashMap<u32, HashMap<String, f32>>> =
            RefCell::new(HashMap::new());
    }
    MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        let by_text = memo.entry(font.size().to_bits()).or_default();
        if let Some(hit) = by_text.get(text) {
            return *hit;
        }
        let width = font.measure_str(text, None).0;
        by_text.insert(text.to_owned(), width);
        width
    })
}

pub(crate) fn draw_tracked(
    canvas: &skia_safe::Canvas,
    font: &Font,
    paint: &Paint,
    text: &str,
    x: f32,
    baseline: f32,
) -> f32 {
    let (_, advances) = measured_tracked(font, text);
    let mut x = x;
    for ((at, ch), advance) in text.char_indices().zip(advances.iter()) {
        canvas.draw_str(&text[at..at + ch.len_utf8()], (x, baseline), font, paint);
        x += advance;
    }
    x
}

struct MenuSeed<'a, T: ComboItem>
where
    T::Command: Send + 'static,
{
    menu: &'a SpeedSearchView<OptionList<T>, OptionSearcher<T>>,
    store: &'a Store,
    ui: &'a UiCtx,
    chrome: ComboChrome,
    widest: f32,
    rows: usize,
    searching: bool,
}

impl<'a, T: ComboItem> MenuSeed<'a, T>
where
    T::Command: Send + 'static,
{
    fn layout(
        self,
        arena: &'a imba::arena::Arena,
        host_size: Size,
        anchor: Rect,
    ) -> Vec<(Point, imba::ThunkBox<'a, ComboCommand<T::Command>>)> {
        let chrome = self.chrome;

        let width = self
            .widest
            .max(anchor.width())
            .max(chrome.menu_min_width)
            .min(host_size.width);
        let visible = self.rows.min(MENU_MAX_ROWS);
        let desired = visible as f32 * chrome.menu_row_height + 2.0;
        let x = anchor.left.min(host_size.width - width).max(0.0);

        let above = anchor.top.max(0.0);
        let below = (host_size.height - anchor.bottom).max(0.0);
        let (height, y) = if above >= desired {
            (desired, anchor.top - desired)
        } else if below >= desired {
            (desired, anchor.bottom)
        } else if above >= below {
            let height = desired.min(above).max(chrome.menu_row_height + 2.0);
            (height, (anchor.top - height).max(0.0))
        } else {
            let height = desired.min(below).max(chrome.menu_row_height + 2.0);
            (
                height,
                anchor.bottom.min(host_size.height - height).max(0.0),
            )
        };
        let viewport_height = height - 2.0;

        let mut menu = imba::container::Container::new(arena, Size::new(width, height));

        let fill = chrome.menu_fill.0;
        let border = chrome.menu_border.0;
        menu.place(
            0.0,
            0.0,
            leaf::<ComboCommand<T::Command>>(width, height).paint_instead(
                move |_arena, canvas, rect| {
                    let mut paint = Paint::default();
                    paint.set_color(fill);
                    canvas.draw_rect(rect, &paint);
                    paint.set_stroke(true);
                    paint.set_stroke_width(1.0);
                    paint.set_color(border);
                    canvas.draw_rect(rect.with_inset((0.5, 0.5)), &paint);
                },
            ),
        );

        let rows = imba::Layout::layout(
            self.menu.display(arena, self.store, self.ui),
            arena,
            Constraints::tight(Size::new(width - 2.0, viewport_height)),
        )
        .map(|command| ComboCommand::Menu(Box::new(command)));
        menu.place(1.0, 1.0, rows);

        let searching = self.searching;
        menu.place(
            0.0,
            0.0,
            leaf::<ComboCommand<T::Command>>(width, height).event(move |_arena, event, _size| {
                match event {
                    Event::KeyDown { key: Key::Up, .. } if !searching => {
                        EventResult::Command(ComboCommand::Select(-1))
                    }
                    Event::KeyDown { key: Key::Down, .. } if !searching => {
                        EventResult::Command(ComboCommand::Select(1))
                    }
                    Event::KeyDown {
                        key: Key::Enter, ..
                    } if searching => EventResult::Commands(vec![
                        ComboCommand::PickCursor,
                        ComboCommand::Menu(Box::new(SpeedSearchCommand::Clear)),
                    ]),
                    Event::KeyDown {
                        key: Key::Enter, ..
                    } => EventResult::Command(ComboCommand::PickCursor),
                    Event::KeyDown {
                        key: Key::Escape, ..
                    } if !searching => EventResult::Command(ComboCommand::Close),
                    _ => EventResult::Ignored,
                }
            }),
        );

        let backdrop = leaf::<ComboCommand<T::Command>>(host_size.width, host_size.height).event(
            |_arena, event, _size| match event {
                Event::MouseDown { .. } => EventResult::Command(ComboCommand::Close),
                _ => EventResult::Ignored,
            },
        );
        vec![
            (Point::new(0.0, 0.0), imba::ThunkBox::new(arena, backdrop)),
            (Point::new(x, y), imba::ThunkBox::new(arena, menu)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ui() -> UiCtx {
        let ui = UiCtx::dont_use_too_slow();
        ui.set(::editor::env::UiFonts(crate::embedded_fonts::source()()));
        ui
    }

    fn stacked(store: &Store, ui: &UiCtx, count: usize) -> Combo {
        let mut combo = Combo::new(store, ui, "DIR");
        combo.set_options(
            store,
            ui,
            (0..count)
                .map(|index| ComboOption::plain(format!("id-{index}"), format!("Folder {index}")))
                .collect(),
        );
        combo
    }

    fn drive(combo: &mut Combo, store: &mut Store, ui: &UiCtx, command: ComboCommand) {
        let mut batch = imba::effect::Batch::new();
        combo.perform(store, ui, command, &mut batch.effects());
    }

    #[test]
    fn picks_ride_ids_and_the_cursor() {
        let mut store = Store::new();
        let ui = test_ui();
        let mut combo = stacked(&store, &ui, 30);
        combo.pick_id("id-7");
        assert_eq!(combo.picked, 7);

        combo.set_options(
            &store,
            &ui,
            (5..30)
                .map(|index| ComboOption::plain(format!("id-{index}"), format!("Folder {index}")))
                .collect(),
        );
        assert_eq!(combo.value().expect("picked").id, "id-7");

        drive(&mut combo, &mut store, &ui, ComboCommand::Open);
        assert!(combo.open);
        drive(&mut combo, &mut store, &ui, ComboCommand::Select(2));
        drive(&mut combo, &mut store, &ui, ComboCommand::PickCursor);
        assert!(!combo.open);
        assert_eq!(combo.value().expect("picked").id, "id-9");
    }

    #[test]
    fn opening_recolors_the_selected_row_for_the_current_theme() {
        let mut store = Store::new();
        let ui = test_ui();
        crate::env::Themes::set(&mut store, crate::Theme::embedded());
        let mut combo = stacked(&store, &ui, 3);
        let dark = crate::rows::selection_style(&store);
        assert_eq!(combo.list().selection_style().unwrap().fill, dark.fill);

        crate::env::Themes::set(&mut store, crate::Theme::light());
        let light = crate::rows::selection_style(&store);
        assert_ne!(dark.fill, light.fill);
        drive(&mut combo, &mut store, &ui, ComboCommand::Open);

        assert_eq!(
            combo.list().selection_style().unwrap().fill,
            light.fill,
            "opening under light mode replaces the cached dark selection"
        );
    }

    #[test]
    fn the_searcher_projects_the_options() {
        let store = Store::new();
        let ui = test_ui();
        let combo = stacked(&store, &ui, 3);
        let source = OptionSearcher::default().capture(combo.menu.inner());
        let items = source();
        assert_eq!(
            items,
            vec![
                ("Folder 0".to_owned(), "id-0".to_owned()),
                ("Folder 1".to_owned(), "id-1".to_owned()),
                ("Folder 2".to_owned(), "id-2".to_owned()),
            ]
        );
    }
}
