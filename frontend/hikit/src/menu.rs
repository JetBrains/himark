// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    constraints::Constraints,
    effect::Effects,
    leaf::leaf,
    list::{ListCommand, ListOps, ListSlice, ListView},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View,
};
use skia_safe::{Paint, Size};

use crate::combo::{measured, ComboItem, ComboOption};
use crate::list_keyboard::{ListKeyCommand, ListKeyboardController};

type MenuList = ScrollView<ListView<ComboOption, String>>;
type Controller = ListKeyboardController<MenuList>;

#[derive(Clone)]
pub enum MenuCommand {
    Rows(Box<ListKeyCommand<ScrollCommand<ListCommand<std::convert::Infallible>>>>),
}

impl std::fmt::Display for MenuCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MenuCommand::Rows(command) => command.fmt(out),
        }
    }
}

/// A context menu: rows in the combo dropdown's chrome, arrow keys,
/// Enter and click picks. Where it stands is the owner's concern —
/// a row shows it as its overlay (`imba::with_overlay::WithOverlay`).
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
        let MenuCommand::Rows(rows) = command;
        let (index, _trigger) = Controller::activated(rows.as_ref())?;
        self.list().key_at(index).cloned()
    }
}

impl View for MenuView {
    type Command = MenuCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        self.rows
            .focus_data(store, ui)
            .map(|command| MenuCommand::Rows(Box::new(command)))
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            MenuCommand::Rows(command) => fx.scope(
                |command| MenuCommand::Rows(Box::new(command)),
                |fx| self.rows.perform(store, ui, *command, fx),
            ),
        }
    }

    /// The menu sizes itself: its widest row (never under the
    /// chrome's minimum), its rows stacked, both cut to what the host
    /// offers.
    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        use imba::layout::LayoutExt as _;
        imba::layout::laid(
            move |_arena: &'a imba::arena::Arena, constraints: Constraints| {
                let chrome = editor::env::Themes::of(store).ui().combo.clone();
                let width = self
                    .widest(store, ui)
                    .max(chrome.menu_min_width)
                    .min(constraints.max.width.max(1.0));
                let height = (self.len() as f32 * chrome.menu_row_height + 2.0)
                    .min(constraints.max.height.max(chrome.menu_row_height + 2.0));
                let mut menu = imba::container::Container::new(arena, Size::new(width, height));
                let fill = chrome.menu_fill.0;
                let border = chrome.menu_border.0;
                menu.place(
                    0.0,
                    0.0,
                    leaf::<MenuCommand>(width, height).paint_instead(
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
                let rows = imba::layout::Layout::layout(
                    self.rows
                        .display(arena, store, ui)
                        .map_layout(|command| MenuCommand::Rows(Box::new(command))),
                    arena,
                    Constraints::tight(Size::new(width - 2.0, height - 2.0)),
                );
                menu.place(1.0, 1.0, rows);
                menu
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imba::list::ActivateTrigger;

    fn test_ui() -> UiCtx {
        let ui = UiCtx::dont_use_too_slow();
        ui.set(imba::ui::UiFonts(
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
}
