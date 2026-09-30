// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult, Key},
    leaf::leaf,
    list::{ListCommand, ListOps, ListSlice, ListView},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View,
};
use skia_safe::{Paint, Point, Rect, Size};

use crate::combo::{measured, ComboItem, ComboOption};
use crate::list_keyboard::{ListKeyCommand, ListKeyboardController};

type MenuList = ScrollView<ListView<ComboOption, String>>;
type Controller = ListKeyboardController<MenuList>;

pub enum MenuCommand {
    Close,

    Rows(Box<ListKeyCommand<ScrollCommand<ListCommand<std::convert::Infallible>>>>),
}

/// The menu itself: rows, arrow keys, Enter and click picks. Where
/// it appears is the wrapper's concern (`PopupMenuView`).
#[derive(Clone)]
pub struct MenuView {
    rows: Controller,
}

impl MenuView {
    pub fn new(store: &Store, ui: &UiCtx, items: Vec<ComboOption>) -> Self {
        let mut slice = ListSlice::new();
        for item in items {
            let height = measured(&item, store, ui).height;
            let id = item.id().to_owned();
            slice.push_keyed_sized(id, item, height);
        }
        let mut list = ListView::empty().with_selection(crate::rows::selection_style(store));
        let len = slice.len();
        let first = (len > 0).then(|| 0usize);
        list.splice_slice(0..0, slice);
        if let Some(key) = first.and_then(|index| list.key_at(index).cloned()) {
            list.select_only(key);
        }
        Self {
            rows: ListKeyboardController::new(ScrollView::new(list)),
        }
    }

    fn list(&self) -> &ListView<ComboOption, String> {
        self.rows.inner().content()
    }

    pub fn len(&self) -> usize {
        self.list().len()
    }

    pub fn is_empty(&self) -> bool {
        self.list().is_empty()
    }

    pub fn widest(&self, store: &Store, ui: &UiCtx) -> f32 {
        self.list()
            .rows()
            .map(|item| measured(&item, store, ui).width)
            .fold(0.0f32, f32::max)
    }

    /// The picked item's id, when this command is the pick.
    pub fn picked(&self, command: &MenuCommand) -> Option<String> {
        let MenuCommand::Rows(rows) = command else {
            return None;
        };
        let (index, _trigger) = Controller::activated(rows.as_ref())?;
        self.list().key_at(index).cloned()
    }

    pub fn closes(command: &MenuCommand) -> bool {
        matches!(command, MenuCommand::Close)
    }
}

impl View for MenuView {
    type Command = MenuCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        use imba::focus::FocusData;
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                Key::Escape => EventResult::Command(MenuCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(
            self.rows
                .focus_data(store, ui)
                .map(|command| MenuCommand::Rows(Box::new(command))),
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
            // Close is the OWNER's signal — the menu holds no `open`.
            MenuCommand::Close => {}
            MenuCommand::Rows(command) => fx.scope(
                |command| MenuCommand::Rows(Box::new(command)),
                |fx| self.rows.perform(store, ui, *command, fx),
            ),
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        use imba::LayoutExt as _;
        self.rows
            .display(arena, store, ui)
            .map_layout(|command| MenuCommand::Rows(Box::new(command)))
    }
}

/// The context-menu wrapper: an anchored window overlay with the
/// combo dropdown's chrome and a full-host backdrop that closes on
/// any outside press.
#[derive(Clone)]
pub struct PopupMenuView {
    pub menu: MenuView,
}

impl PopupMenuView {
    pub fn new(store: &Store, ui: &UiCtx, items: Vec<ComboOption>) -> Self {
        Self {
            menu: MenuView::new(store, ui, items),
        }
    }

    pub fn picked(&self, command: &MenuCommand) -> Option<String> {
        self.menu.picked(command)
    }

    /// A zero-sized thunk the owner places where the menu should
    /// anchor — its translated rect reaches the window host as the
    /// anchor, so container and scroll offsets apply on the way up.
    pub fn overlay_at<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> imba::ThunkBox<'a, MenuCommand> {
        let themes = crate::env::Themes::of(store);
        let chrome = themes.ui().combo.clone();
        let seed = PopupSeed {
            menu: &self.menu,
            store,
            ui,
            widest: self.menu.widest(store, ui),
            rows: self.menu.len(),
            chrome,
        };
        imba::ThunkBox::new(
            arena,
            leaf::<MenuCommand>(1.0, 1.0).overlay(
                imba::overlay::WINDOW,
                move |host_size: Size, anchor: Rect| seed.layout(arena, host_size, anchor),
            ),
        )
    }
}

impl View for PopupMenuView {
    type Command = MenuCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        self.menu.focus_data(store, ui)
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        self.menu.perform(store, ui, command, fx)
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a imba::arena::Arena, _constraints: Constraints| {
                self.overlay_at(arena, store, ui)
            },
        )
    }
}

struct PopupSeed<'a> {
    menu: &'a MenuView,
    store: &'a Store,
    ui: &'a UiCtx,
    widest: f32,
    rows: usize,
    chrome: ::editor::theme::ComboChrome,
}

impl<'a> PopupSeed<'a> {
    fn layout(
        self,
        arena: &'a imba::arena::Arena,
        host_size: Size,
        anchor: Rect,
    ) -> Vec<(Point, imba::ThunkBox<'a, MenuCommand>)> {
        let chrome = self.chrome;

        let width = self
            .widest
            .max(chrome.menu_min_width)
            .min(host_size.width.max(1.0));
        let desired = self.rows as f32 * chrome.menu_row_height + 2.0;
        let x = anchor.left.min(host_size.width - width).max(0.0);

        // A context menu drops BELOW the press; above is the
        // fallback, shrinking only when neither side has the room.
        let above = anchor.top.max(0.0);
        let below = (host_size.height - anchor.bottom).max(0.0);
        let (height, y) = if below >= desired {
            (desired, anchor.bottom)
        } else if above >= desired {
            (desired, anchor.top - desired)
        } else if below >= above {
            let height = desired.min(below).max(chrome.menu_row_height + 2.0);
            (
                height,
                anchor.bottom.min(host_size.height - height).max(0.0),
            )
        } else {
            let height = desired.min(above).max(chrome.menu_row_height + 2.0);
            (height, (anchor.top - height).max(0.0))
        };

        let mut menu = imba::container::Container::new(arena, Size::new(width, height));
        let fill = chrome.menu_fill.0;
        let border = chrome.menu_border.0;
        menu.place(
            0.0,
            0.0,
            leaf::<MenuCommand>(width, height).paint_instead(move |_arena, canvas, rect| {
                let mut paint = Paint::default();
                paint.set_color(fill);
                canvas.draw_rect(rect, &paint);
                paint.set_stroke(true);
                paint.set_stroke_width(1.0);
                paint.set_color(border);
                canvas.draw_rect(rect.with_inset((0.5, 0.5)), &paint);
            }),
        );
        let rows = imba::Layout::layout(
            self.menu.display(arena, self.store, self.ui),
            arena,
            Constraints::tight(Size::new(width - 2.0, height - 2.0)),
        );
        menu.place(1.0, 1.0, rows);

        let backdrop =
            leaf::<MenuCommand>(host_size.width, host_size.height).event(|_arena, event, _size| {
                match event {
                    Event::MouseDown { .. } => EventResult::Command(MenuCommand::Close),
                    _ => EventResult::Ignored,
                }
            });
        vec![
            (Point::new(0.0, 0.0), imba::ThunkBox::new(arena, backdrop)),
            (Point::new(x, y), imba::ThunkBox::new(arena, menu)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imba::list::ActivateTrigger;

    fn test_ui() -> UiCtx {
        let ui = UiCtx::dont_use_too_slow();
        ui.set(::editor::env::UiFonts(
            ::editor::test_document::test_fonts_collection().clone(),
        ));
        ui
    }

    fn stacked(store: &Store, ui: &UiCtx) -> MenuView {
        MenuView::new(
            store,
            ui,
            vec![
                ComboOption::plain("rename", "Rename"),
                ComboOption::plain("delete", "Delete"),
                ComboOption::plain("create", "New File"),
            ],
        )
    }

    #[test]
    fn keys_walk_and_enter_picks_the_id() {
        let mut store = Store::new();
        let ui = test_ui();
        let mut menu = stacked(&store, &ui);
        assert_eq!(menu.len(), 3);

        let step = menu.rows.step_index(1).expect("a stepped row");
        let select = MenuCommand::Rows(Box::new(menu.rows.select_command(step)));
        assert!(menu.picked(&select).is_none());
        let mut batch = imba::effect::Batch::new();
        menu.perform(&mut store, &ui, select, &mut batch.effects());

        let at = menu.rows.cursor_index().expect("a cursor row");
        let pick = MenuCommand::Rows(Box::new(
            menu.rows.activate_command(at, ActivateTrigger::Enter),
        ));
        assert_eq!(menu.picked(&pick).as_deref(), Some("delete"));
    }

    #[test]
    fn the_popup_seed_prefers_below_and_flips_above() {
        let store = Store::new();
        let ui = test_ui();
        let popup = PopupMenuView::new(&store, &ui, vec![ComboOption::plain("one", "One")]);
        let arena = imba::arena::Arena::default();
        let host = Size::new(800.0, 600.0);

        let seed = |anchor: Rect| {
            let chrome = crate::env::Themes::of(&store).ui().combo.clone();
            PopupSeed {
                menu: &popup.menu,
                store: &store,
                ui: &ui,
                widest: popup.menu.widest(&store, &ui),
                rows: popup.menu.len(),
                chrome,
            }
            .layout(&arena, host, anchor)
        };

        let below = seed(Rect::from_xywh(100.0, 100.0, 1.0, 1.0));
        assert_eq!(below.len(), 2, "backdrop and menu");
        assert!(below[1].0.y >= 101.0, "drops below the anchor");

        let flipped = seed(Rect::from_xywh(100.0, 595.0, 1.0, 1.0));
        assert!(flipped[1].0.y < 595.0, "flips above at the bottom edge");
    }
}
