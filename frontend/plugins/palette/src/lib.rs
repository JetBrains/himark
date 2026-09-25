// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use himark::{AppCommand, Application, ListKeyCommand, ListKeyboardController, ModalRequest, ModalView};
use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult, Key},
    leaf::leaf,
    list::{ListCommand, ListOps, ListView},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    Layout as _, PresentableCommand, UiCtx, View,
};
use skia_safe::{Paint, Size};

/// Keys-only controller over the raw label list — the palette's own
/// input does the filtering; the table does the movement
/// (docs/ui/list-keyboard.md).
type Rows =
    ListKeyboardController<ScrollView<ListView<himark::LabelRow, usize>>>;
type RowsCommand = ListKeyCommand<ScrollCommand<ListCommand<std::convert::Infallible>>>;

#[derive(Clone)]
struct Entry {
    id: &'static str,
    name: String,

    shortcut: Option<String>,

    command: std::sync::Arc<std::sync::Mutex<Option<AppCommand>>>,
}

pub enum PaletteCommand {
    Rows(RowsCommand),

    Pick(usize),

    Close,
}

const PALETTE_SHOWN: usize = 200;

#[derive(Clone)]
pub struct PaletteView {
    entries: Vec<Entry>,

    matches: Vec<usize>,

    list: Rows,

    request: himark::RequestSlot<ModalRequest>,
}

impl PaletteView {
    pub fn new(store: &Store, ui: &UiCtx, commands: Vec<PresentableCommand<AppCommand>>) -> Self {
        let shortcuts = himark::Keymaps::of(store).shortcuts_by_id();
        let entries = commands
            .into_iter()
            .map(|presentable| Entry {
                shortcut: shortcuts.get(presentable.id).cloned(),
                id: presentable.id,
                name: presentable.name,
                command: std::sync::Arc::new(std::sync::Mutex::new(Some(presentable.command))),
            })
            .collect();
        let mut palette = Self {
            entries,
            matches: Vec::new(),
            list: ListKeyboardController::new(ScrollView::new(ListView::empty())),
            request: Default::default(),
        };
        palette.filter(store, ui, "");
        palette
    }

    pub fn labels(&self) -> Vec<String> {
        self.matches
            .iter()
            .map(|&index| self.entries[index].name.clone())
            .collect()
    }

    fn filter(&mut self, store: &Store, ui: &UiCtx, query: &str) {
        let query = query.to_lowercase();
        self.matches.clear();
        for (index, entry) in self.entries.iter().enumerate() {
            if self.matches.len() >= PALETTE_SHOWN {
                break;
            }
            if subsequence_match(&entry.name.to_lowercase(), &query)
                || subsequence_match(entry.id, &query)
            {
                self.matches.push(index);
            }
        }
        let selected = self
            .list
            .cursor_index()
            .unwrap_or(0)
            .min(self.matches.len().saturating_sub(1));
        let labels = self.labels();

        let trails: Vec<Option<String>> = self
            .matches
            .iter()
            .map(|&index| self.entries[index].shortcut.clone())
            .collect();
        let scroll_y = self.list.inner().scroll_y();
        let mut list = ListView::from_slice(himark::label_slice(store, ui, &labels, &trails, None))
            .with_selection(himark::selection_style(store));
        if !labels.is_empty() {
            list.select_only(selected);
        }
        *self.list.inner_mut() = ScrollView::new(list);
        self.list.inner_mut().set_scroll_y(scroll_y);
    }

    fn selected(&self) -> usize {
        self.list.cursor_index().unwrap_or(0)
    }
}

impl View for PaletteView {
    type Command = PaletteCommand;

    fn focus_data<'w>(
        &'w self,
        _store: &'w Store,
        _ui: &'w imba::UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        use imba::event::EventResult;
        // Movement and Enter are the controller's table; the palette
        // keeps its own close (and Enter-with-nothing closes too).
        let empty = self.matches.is_empty();
        let own = imba::focus::FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                Key::Enter if empty => EventResult::Command(PaletteCommand::Close),
                Key::Escape => EventResult::Command(PaletteCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..imba::focus::FocusData::default()
        };
        own.merge_under(self.list.focus_data(_store, _ui).map(PaletteCommand::Rows))
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            PaletteCommand::Rows(command) => {
                if let Some((row, _trigger)) = Rows::activated(&command) {
                    // Enter and click both run the command; the note
                    // row is unkeyed and never answers.
                    if self.list.inner().content().key_at(row).is_some() {
                        return self.perform(store, ui, PaletteCommand::Pick(row), fx);
                    }
                }
                fx.scope(PaletteCommand::Rows, |fx| {
                    imba::View::perform(&mut self.list, store, ui, command, fx)
                });
            }
            PaletteCommand::Pick(row) => {
                let picked = self
                    .matches
                    .get(row)
                    .and_then(|&index| self.entries[index].command.lock().unwrap().take())
                    .map(ModalRequest::Perform)
                    .unwrap_or(ModalRequest::Close);
                self.request.file(picked);
            }
            PaletteCommand::Close => {
                self.request.file(ModalRequest::Close);
            }
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

            let chrome = ::himark::env::Themes::of(store).ui().peeker.clone();
            let row_height = chrome.row_height;

            let list_width = size.width;
            let list_x = 0.0;
            let list_top = chrome.margin;

            let list_height =
                (size.height - list_top - chrome.hint_bottom - chrome.row_height).max(row_height);

            let match_count = self.matches.len();
            let total = self.entries.len();
            let row_font = himark::fonts::ui_font(ui, chrome.row_size);
            let hint_font = himark::fonts::ui_font(ui, chrome.hint_size);

            let mut container = imba::container::container(arena, size);

            let backdrop = leaf::<PaletteCommand>(size.width, size.height)
                .paint_instead({
                    let background = chrome.background.0;
                    move |_arena, canvas, rect| {
                        let mut surface = Paint::default();
                        surface.set_color(background);
                        canvas.draw_rect(rect, &surface);
                    }
                })
                .event(|_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::Command(PaletteCommand::Close),
                    _ => EventResult::Ignored,
                });
            container.place(0.0, 0.0, backdrop);

            // The chrome labels as `imba::text`, centered in their
            // row band (the design-system row rule); the texts ignore
            // presses, so the backdrop's close-on-click still answers
            // underneath them.
            if match_count == 0 {
                let metrics = row_font.metrics().1;
                let text_height = (-metrics.ascent + metrics.descent).ceil().max(1.0);
                container.place_boxed(
                    list_x + chrome.row_text_x,
                    list_top + ((row_height - text_height) * 0.5).max(0.0),
                    imba::text(
                        ui,
                        "no matching commands",
                        row_font.clone(),
                        chrome.dim_text.0,
                    )
                    .layout(arena, Constraints::tight(size).loosen()),
                );
            }
            let hint_ascent = -hint_font.metrics().1.ascent;
            container.place_boxed(
                list_x,
                size.height - chrome.hint_bottom - hint_ascent,
                imba::text(
                    ui,
                    format!("{match_count} of {total} commands   ↑↓ select   ⏎ run   esc dismiss"),
                    hint_font.clone(),
                    chrome.dim_text.0,
                )
                .layout(arena, Constraints::tight(size).loosen()),
            );

            container.place(
                list_x,
                list_top,
                imba::Layout::layout(
                    self.list.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(list_width, list_height)),
                )
                .map(PaletteCommand::Rows),
            );

            // Movement and Enter live in the controller's overlay.
            let empty = match_count == 0;
            let keymap = leaf::<PaletteCommand>(size.width, size.height).event(
                move |_arena, event, _size| match event {
                    Event::KeyDown {
                        key: Key::Enter, ..
                    } if empty => EventResult::Command(PaletteCommand::Close),
                    Event::KeyDown {
                        key: Key::Escape, ..
                    } => EventResult::Command(PaletteCommand::Close),
                    _ => EventResult::Ignored,
                },
            );
            container.place(0.0, 0.0, keymap);

            container
        })
    }
}

impl ModalView for PaletteView {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn set_query(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        query: &str,
        _fx: &mut imba::effect::Effects<'_, imba::DynCommand>,
    ) {
        self.filter(store, ui, query.trim());
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub fn overlay_surface() -> himark::OverlaySurface {
    himark::OverlaySurface {
        prefix: Some('>'),
        open: std::sync::Arc::new(|store, ui, window, _fx| {
            let commands = himark::palette_commands(store, ui, window);
            Box::new(PaletteView::new(store, ui, commands))
        }),
    }
}

pub struct TogglePalette;

impl himark::DynamicCommand for TogglePalette {
    fn id(&self) -> &'static str {
        "palette.toggle"
    }
    fn name(&self) -> String {
        "Command Palette".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        let ui = app.ui_handle();
        himark::toggle_toolbar_session(store, &ui, window, &overlay_surface(), ">", fx);
    }
}

fn subsequence_match(candidate: &str, query: &str) -> bool {
    let mut candidate = candidate.chars();
    query
        .chars()
        .all(|wanted| candidate.by_ref().any(|ch| ch == wanted))
}

#[cfg(test)]
mod tests;
