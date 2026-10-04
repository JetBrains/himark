// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use himark::app::AppCommand;
use himark::app::Application;
use hikit::list_keyboard::ListKeyCommand;
use hikit::list_keyboard::ListKeyboardController;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;
use imba::{arena::Arena, constraints::Constraints, event::{Event, EventResult, Key}, leaf::leaf, list::{ListCommand, ListOps, ListView}, scroll::{ScrollCommand, ScrollView}, store::Store, thunk_ext::ThunkExt, layout::Layout as _, PresentableCommand, ui::UiCtx, View};
use skia_safe::{Paint, Size};

/// Keys-only controller over the raw label list — the palette's own
/// input does the filtering; the table does the movement
/// (docs/ui/list-keyboard.md).
type Rows = ListKeyboardController<ScrollView<ListView<hikit::rows::LabelRow, usize>>>;
type RowsCommand = ListKeyCommand<ScrollCommand<ListCommand<std::convert::Infallible>>>;

#[derive(Clone)]
struct Entry {
    id: &'static str,
    name: String,

    shortcut: Option<String>,

    command: std::sync::Arc<std::sync::Mutex<Option<AppCommand>>>,
}

#[derive(Clone)]
pub enum PaletteCommand {
    /// The palette's OWN input editor — the query lives here.
    Input(editor::editor_view::EditorCommand),

    Rows(RowsCommand),

    Pick(usize),

    Close,
}

impl std::fmt::Display for PaletteCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PaletteCommand::Input(command) => command.fmt(out),
            PaletteCommand::Rows(command) => command.fmt(out),
            PaletteCommand::Pick(_) => out.write_str("palette pick"),
            PaletteCommand::Close => out.write_str("palette close"),
        }
    }
}

const PALETTE_SHOWN: usize = 200;

#[derive(Clone)]
pub struct PaletteView {
    /// The palette's own query input: the overlay owns its text.
    input: editor::editor_view::EditorView,

    entries: Vec<Entry>,

    matches: Vec<usize>,

    list: Rows,

    request: hikit::modal::RequestSlot<ModalRequest>,
}

impl PaletteView {
    pub fn new(store: &Store, ui: &UiCtx, commands: Vec<PresentableCommand<AppCommand>>) -> Self {
        let shortcuts = himark::keymap::Keymaps::of(store).shortcuts_by_id();
        let entries = commands
            .into_iter()
            .map(|presentable| Entry {
                shortcut: shortcuts.get(presentable.id).cloned(),
                id: presentable.id,
                name: presentable.name,
                command: std::sync::Arc::new(std::sync::Mutex::new(Some(presentable.command))),
            })
            .collect();
        let mut input = editor::editor_view::EditorView::input(600.0, store, ui, hikit::fonts::source());
        input.focus_text();
        let mut palette = Self {
            input,
            entries,
            matches: Vec::new(),
            list: ListKeyboardController::new(ScrollView::new(ListView::empty())),
            request: Default::default(),
        };
        palette.filter(store, ui, "");
        palette
    }

    fn query(&self) -> String {
        let mut view = self.input.document.text().view();
        let byte_count = view.byte_count();
        view.byte_string(0, byte_count)
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
        let mut list = ListView::from_slice(hikit::rows::label_slice(store, ui, &labels, &trails, None))
            .with_selection(hikit::rows::selection_style(store));
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
        _ui: &'w imba::ui::UiCtx,
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
            .merge_under(
                self.input
                    .focus_data(_store, _ui)
                    .map(PaletteCommand::Input),
            )
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            PaletteCommand::Input(command) => {
                fx.scope(PaletteCommand::Input, |fx| {
                    self.input.perform(store, ui, command, fx)
                });
                let query = self.query();
                self.filter(store, ui, query.trim());
            }
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
                    .map(|command| ModalRequest::Perform(himark::app::shell_verb(command)))
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
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;

            let chrome = ::editor::env::Themes::of(store).ui().peeker.clone();
            let row_height = chrome.row_height;

            let list_width = size.width;
            let list_x = 0.0;
            let input_height = chrome.input_height;
            let list_top = chrome.margin + input_height;

            let list_height =
                (size.height - list_top - chrome.hint_bottom - chrome.row_height).max(row_height);

            let match_count = self.matches.len();
            let total = self.entries.len();
            let row_font = hikit::fonts::ui_font(ui, chrome.row_size);
            let hint_font = hikit::fonts::ui_font(ui, chrome.hint_size);

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

            let input_w = (list_width - chrome.input_inset_x * 2.0).max(chrome.input_min_width);
            let input_h = (input_height - chrome.input_inset_y * 2.0).max(1.0);
            container.place(
                chrome.input_inset_x,
                chrome.margin * 0.5 + chrome.input_inset_y,
                imba::layout::Layout::layout(
                    self.input.display(arena, store, ui),
                    arena,
                    Constraints {
                        min: Size::new(input_w, input_h),
                        max: Size::new(input_w, input_h),
                    },
                )
                .map(PaletteCommand::Input),
            );

            // The chrome labels as `imba::layout::text`, centered in their
            // row band (the design-system row rule); the texts ignore
            // presses, so the backdrop's close-on-click still answers
            // underneath them.
            if match_count == 0 {
                let metrics = row_font.metrics().1;
                let text_height = (-metrics.ascent + metrics.descent).ceil().max(1.0);
                container.place_boxed(
                    list_x + chrome.row_text_x,
                    list_top + ((row_height - text_height) * 0.5).max(0.0),
                    imba::layout::text(
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
                imba::layout::text(
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
                imba::layout::Layout::layout(
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
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Build the palette overlay: a plain z-stacked modal layer that
/// OWNS its input. The command walk runs BEFORE the modal mounts, so
/// the window's own commands are all collected.
pub fn build(store: &mut Store, ui: &UiCtx, window: himark::window::WindowId) -> Box<dyn ModalView> {
    let commands = himark::commands::palette_commands(store, ui, window);
    Box::new(PaletteView::new(store, ui, commands))
}

pub struct TogglePalette;

impl himark::commands::DynamicCommand for TogglePalette {
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
        window: himark::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let entity = himark::window::Windows::window_ref(store, window).expect("the window entity");
        if entity.has_modal() {
            let mut entity = entity.clone();
            fx.scope(
                move |command| himark::app::AppCommand::Content(window, command),
                |fx| entity.dismiss_modal(store, fx),
            );
            himark::window::Windows::put(store, window, entity);
            return;
        }
        let ui = app.ui_handle();
        let modal = build(store, &ui, window);
        let mut entity = himark::window::Windows::window(store, window).expect("the window entity");
        fx.scope(
            move |command| himark::app::AppCommand::Content(window, command),
            |fx| entity.show_modal(store, modal, fx),
        );
        himark::window::Windows::put(store, window, entity);
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
