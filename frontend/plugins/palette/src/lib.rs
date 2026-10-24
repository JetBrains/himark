// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use hikit::list_keyboard::ListKeyCommand;
use hikit::list_keyboard::ListKeyboardController;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;
use himark::app::AppCommand;
use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult, Key},
    layout::{Insets, Layout as _, LayoutExt as _},
    leaf::leaf,
    list::{ListCommand, ListOps, ListSlice, ListView},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    PresentableCommand, View,
};
use skia_safe::Size;

mod sheet;

/// Keys-only controller over the command rows — the palette's own
/// input does the filtering; the table does the movement
/// (docs/ui/list-keyboard.md).
type Rows = ListKeyboardController<ScrollView<ListView<CommandRow, usize>>>;
type RowsCommand = ListKeyCommand<ScrollCommand<ListCommand<std::convert::Infallible>>>;

#[derive(Clone)]
struct Entry {
    id: &'static str,
    name: String,

    shortcut: Option<String>,

    command: std::sync::Arc<std::sync::Mutex<Option<AppCommand>>>,
}

/// One command row: its name with the typed letters marked, its
/// shortcut at the right. The SELECTED row speaks louder — a larger,
/// heavier name — so the row knows whether it is the one.
#[derive(Clone)]
struct CommandRow {
    name: String,
    marks: Vec<std::ops::Range<usize>>,
    shortcut: Option<String>,
    selected: bool,
}

impl View for CommandRow {
    type Command = std::convert::Infallible;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        let themes = ::editor::env::Themes::of(store);
        let peeker = themes.part(::hikit::chrome::PeekerChrome::KEY);
        let chrome = &peeker.palette;
        let ink = themes.part(::editor::theme::InkChrome::KEY);
        let (role, color, shortcut) = match self.selected {
            true => (
                &chrome.selected_row,
                ink.ink_bright.0,
                sheet::mono(ui, chrome.selected_shortcut_size, ink.ink_2.0),
            ),
            false => (
                &chrome.row,
                ink.ink.0,
                sheet::mono(ui, chrome.shortcut_size, ink.ink_3.0),
            ),
        };
        let plain = sheet::styled(ui, role, color);
        let mark = sheet::MatchMark::of(
            ui,
            role,
            chrome.mark_weight,
            ink.ink.0,
            ink.live.0,
            chrome.mark_thickness,
            chrome.mark_offset,
        );
        let mut row = imba::layout::Row::new(arena)
            .child_by_baseline(
                sheet::marked_label(arena, &self.name, &self.marks, &plain, &mark).pad_insets(
                    Insets {
                        left: chrome.pad_x,
                        ..Default::default()
                    },
                ),
            )
            // Main-axis only: a bare `Fill` would stretch to the
            // row's height and drag the baseline group to its top.
            .weighted(1.0, imba::layout::Fill::new().height(0.0));
        if let Some(keys) = &self.shortcut {
            row = row.child_by_baseline(hikit::ui::text(&shortcut, keys.clone()).pad_insets(
                Insets {
                    left: chrome.shortcut_gap,
                    right: chrome.pad_x,
                    ..Default::default()
                },
            ));
        }
        // A press on the row is the LIST's: `Select`, then
        // `Activate(Click)` (docs/ui/list-keyboard.md §2).
        row.align(imba::layout::Alignment::CenterStart)
            .height(chrome.row_height)
            .on_event(
                move |_arena: &Arena, event: &Event<'_>, _size| match event {
                    Event::MouseDown { .. } => EventResult::handled(),
                    _ => EventResult::Ignored,
                },
            )
    }
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

    /// The trimmed query the rows were marked against.
    query: String,

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
        let mut input = editor::editor_view::EditorView::input_styled(
            600.0,
            store,
            ui,
            editor::theme::StyleId::Query,
        );
        input.focus_text();
        let mut palette = Self {
            input,
            entries,
            matches: Vec::new(),
            query: String::new(),
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

    /// The row for the `row`th match, selected or not.
    fn row(&self, row: usize, selected: bool) -> CommandRow {
        let entry = &self.entries[self.matches[row]];
        CommandRow {
            name: entry.name.clone(),
            marks: sheet::match_runs(&entry.name, &self.query),
            shortcut: entry.shortcut.clone(),
            selected,
        }
    }

    fn filter(&mut self, store: &Store, ui: &UiCtx, query: &str) {
        self.query = query.to_owned();
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
        let mut slice: ListSlice<CommandRow, usize> = ListSlice::new();
        for row in 0..self.matches.len() {
            slice.push_keyed(row, self.row(row, row == selected), store, ui);
        }
        let scroll_y = self.list.inner().scroll_y();
        let mut list = ListView::from_slice(slice).with_selection(selection_style(store));
        if !self.matches.is_empty() {
            list.select_only(selected);
        }
        *self.list.inner_mut() = ScrollView::new(list);
        self.list.inner_mut().set_scroll_y(scroll_y);
    }

    /// The selection moved: the row it left speaks quietly again and
    /// the row it reached speaks up — two rows re-laid, in place.
    fn restyle(&mut self, store: &Store, ui: &UiCtx, left: Option<usize>, reached: Option<usize>) {
        for (row, selected) in [(left, false), (reached, true)] {
            let Some(row) = row.filter(|&row| row < self.matches.len()) else {
                continue;
            };
            let mut slice: ListSlice<CommandRow, usize> = ListSlice::new();
            slice.push_keyed(row, self.row(row, selected), store, ui);
            self.list
                .inner_mut()
                .content_mut()
                .splice_slice(row..row + 1, slice);
        }
    }

    #[cfg(test)]
    fn selected(&self) -> usize {
        self.list.cursor_index().unwrap_or(0)
    }
}

/// The selected row's flat wash, across the sheet's whole width.
fn selection_style(store: &Store) -> imba::list::SelectionStyle {
    imba::list::SelectionStyle {
        fill: ::editor::env::Themes::of(store)
            .part(::hikit::chrome::PeekerChrome::KEY)
            .palette
            .selected_fill
            .0,
        ..hikit::rows::selection_style(store)
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
                Key::Enter if empty => EventResult::command(PaletteCommand::Close),
                Key::Escape => EventResult::command(PaletteCommand::Close),
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
                let left = self.list.cursor_index();
                fx.scope(PaletteCommand::Rows, |fx| {
                    imba::View::perform(&mut self.list, store, ui, command, fx)
                });
                let reached = self.list.cursor_index();
                if reached != left {
                    self.restyle(store, ui, left, reached);
                }
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

    /// The sheet's content, top to bottom: the query band (caps
    /// label, the query, the count), a hairline, the ranked rows, a
    /// hairline, the key footer — at the width it is given and its
    /// own height (the rows scroll past the chrome's cap).
    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            use imba::layout::{fixed, Column};
            use imba::Thunk as _;
            let width = constraints.max.width;
            let themes = ::editor::env::Themes::of(store);
            let peeker = themes.part(::hikit::chrome::PeekerChrome::KEY);
            let chrome = &peeker.palette;
            let ink = themes.part(::editor::theme::InkChrome::KEY);
            let loose = Constraints {
                min: Size::default(),
                max: Size::new(width, constraints.max.height),
            };

            let input_width = (width - 2.0 * chrome.pad_x).max(1.0);
            let input_height = self.input.content_height().max(1.0);
            let input = imba::layout::Layout::layout(
                self.input.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(input_width, input_height)),
            )
            .map(PaletteCommand::Input);

            let count = format!("{} of {} commands", self.matches.len(), self.entries.len());
            let head = Column::new(arena)
                .child(hikit::ui::text(&hikit::ui::caps(store, ui), "COMMAND"))
                .child(fixed(input).pad_insets(Insets {
                    top: chrome.label_gap,
                    ..Default::default()
                }))
                .child(hikit::ui::text(&hikit::ui::brief(store, ui), count).pad_insets(
                    Insets {
                        top: chrome.count_gap,
                        ..Default::default()
                    },
                ))
                .pad_insets(Insets {
                    left: chrome.pad_x,
                    right: chrome.pad_x,
                    top: chrome.pad_top,
                    bottom: chrome.pad_bottom,
                })
                .layout(arena, loose);

            let hint = hikit::ui::brief(store, ui).sized(chrome.footer_size);
            let foot = sheet::key_hints(
                arena,
                &[("↑↓", "move"), ("⏎", "run"), ("esc", "close")],
                &sheet::mono(ui, chrome.footer_size, ink.ink_2.0),
                &hint,
                chrome.footer_gap,
            )
            .pad_insets(Insets {
                left: chrome.pad_x,
                right: chrome.pad_x,
                top: chrome.footer_pad_y,
                bottom: chrome.footer_pad_y,
            })
            .layout(arena, loose);

            // The rows take what the bands leave, up to the cap.
            let pads = chrome.list_pad_top + chrome.list_pad_bottom;
            let room = constraints.max.height - head.size().height - foot.size().height - 2.0 - pads;
            let rows: imba::ThunkBox<'a, PaletteCommand> = match self.matches.is_empty() {
                true => imba::layout::Layout::layout(
                    hikit::ui::text(
                        &sheet::styled(ui, &chrome.row, ink.ink_3.0),
                        "no matching commands",
                    )
                    .pad_insets(Insets {
                        left: chrome.pad_x,
                        ..Default::default()
                    })
                    .align(imba::layout::Alignment::CenterStart)
                    .sized(width, chrome.row_height),
                    arena,
                    loose,
                ),
                false => {
                    let total = self.list.inner().content().total_height();
                    let height = total.min(chrome.list_max_height).min(room).max(chrome.row_height);
                    imba::ThunkBox::new(
                        arena,
                        imba::layout::Layout::layout(
                            self.list.display(arena, store, ui),
                            arena,
                            Constraints::tight(Size::new(width, height)),
                        )
                        .map(PaletteCommand::Rows),
                    )
                }
            };

            let column = Column::new(arena)
                .child(fixed(head))
                .child(hikit::ui::rule_h(chrome.rule.0, width))
                .child(fixed(rows).pad_insets(Insets {
                    top: chrome.list_pad_top,
                    bottom: chrome.list_pad_bottom,
                    ..Default::default()
                }))
                .child(hikit::ui::rule_h(chrome.rule.0, width))
                .child(fixed(foot))
                .layout(arena, loose);

            // Enter-with-nothing and Escape close, wherever the
            // keyboard stands inside the sheet.
            let size = column.size();
            let empty = self.matches.is_empty();
            let mut sheet = imba::container::container(arena, size);
            sheet.place_boxed(0.0, 0.0, column);
            sheet.place(
                0.0,
                0.0,
                leaf::<PaletteCommand>(size.width, size.height).event(
                    move |_arena, event, _size| match event {
                        Event::KeyDown {
                            key: Key::Enter, ..
                        } if empty => EventResult::command(PaletteCommand::Close),
                        Event::KeyDown {
                            key: Key::Escape, ..
                        } => EventResult::command(PaletteCommand::Close),
                        _ => EventResult::Ignored,
                    },
                ),
            );
            sheet
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

/// Build the palette overlay: the palette standing on its SHEET,
/// centered under the window's top; it OWNS its input. The command walk runs BEFORE the modal mounts, so
/// the window's own commands are all collected.
pub fn build(
    store: &mut Store,
    ui: &UiCtx,
    window: workbench::window::WindowId,
) -> Box<dyn ModalView> {
    let sheet = sheet::Sheet(::editor::env::Themes::of(store).part(::hikit::chrome::PeekerChrome::KEY).palette.sheet.clone());
    let commands = himark::commands::palette_commands(store, ui, window);
    Box::new(sheet::Sheeted::new(bare(store, ui, commands), sheet, || {
        PaletteCommand::Close
    }))
}

/// The palette UNDRESSED, over commands someone else walked: the
/// mounted road's factory (`Himark::register_palette`). A host
/// that already painted a floor mounts this one inside its glass.
pub fn bare(
    store: &Store,
    ui: &UiCtx,
    commands: Vec<PresentableCommand<AppCommand>>,
) -> PaletteView {
    PaletteView::new(store, ui, commands)
}

/// The palette as a plugin: the toggle command and the mounted
/// road's bare-view factory.
pub struct PalettePlugin;

impl himark::plugin::Plugin for PalettePlugin {
    fn install(&self, app: &mut himark::app::Himark, _cx: &himark::plugin::PluginContext) {
        use himark::app_ext::AppExt;
        app.register_command(std::sync::Arc::new(TogglePalette));
        // The COMMAND PALETTE: one view, both roads. The workbench
        // opens it as a modal over a window; a mounted host mints
        // it as a place, and the engine walks that session's views
        // for the commands. Bare — the host's island glass is the
        // floor it would otherwise paint.
        app.register_palette(std::sync::Arc::new(|store, ui, commands| {
            Box::new(bare(store, ui, commands))
        }));
    }
}

pub struct TogglePalette;

impl himark::commands::HimarkCommand for TogglePalette {
    fn id(&self) -> &'static str {
        "palette.toggle"
    }
    fn name(&self) -> String {
        "Command Palette".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
        let entity =
            workbench::window::Windows::window_ref(store, window).expect("the window entity");
        if entity.has_modal() {
            let mut entity = entity.clone();
            fx.scope(
                move |command| himark::app::content(window, command),
                |fx| entity.dismiss_modal(store, fx),
            );
            workbench::window::Windows::put(store, window, entity);
            return;
        }
        let ui = ui;
        let modal = build(store, &ui, window);
        let mut entity =
            workbench::window::Windows::window(store, window).expect("the window entity");
        fx.scope(
            move |command| himark::app::content(window, command),
            |fx| entity.show_modal(store, modal, fx),
        );
        workbench::window::Windows::put(store, window, entity);
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
