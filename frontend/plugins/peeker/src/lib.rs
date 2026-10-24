// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use ahp_locations::FindEffect;
use documents::entity_view::EditorIdView;
use documents::BuildDocumentEffect;
use documents::FetchDocumentEffect;
use editor::document::Document;
use editor::location::ResourceLocation;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;
use hikit::panel::WidgetOrigin;
use himark::app::Himark;
use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult, Key},
    layout::Layout as _,
    leaf::leaf,
    scroll::ScrollView,
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View,
};
use imba::layout::{Insets, LayoutExt as _};
use skia_safe::Size;
use workbench::workbench_node::EditorPane;

mod sheet;
use workbench::workbench_node::PaneCommand;

#[derive(Clone)]
pub struct Peeker {
    /// The peeker's own query input: the overlay owns its text, typed
    /// directly — no omnibox in between.
    input: editor::editor_view::EditorView,

    /// The documents collection this peeker fronts — its id is wired
    /// at open from the window.s session bundle (docs/entities.md law 3).
    documents: imba::store::Id<documents::OpenDocuments>,

    recents: Vec<ResourceLocation>,

    workspace: Vec<ResourceLocation>,

    found: Vec<ResourceLocation>,

    find_serial: u64,

    find_token: Option<imba::effect::CancellationToken>,

    temp_docs: std::collections::HashMap<ResourceLocation, documents::DocumentId>,

    pending_fetch: std::collections::HashSet<ResourceLocation>,

    widgets: Vec<(WidgetOrigin, Box<dyn hikit::panel::DynPanelView>)>,
    widget_titles: Vec<String>,

    filter_query: String,

    rows: Vec<PeekerRow>,
    labels: Vec<String>,
    /// Each row's directory, relative to its workspace folder (empty
    /// for a widget row).
    dirs: Vec<String>,

    hidden: usize,

    list: Rows,
    preview: Option<PreviewSlot>,

    preview_width: f32,

    /// The windowed modal stands on its own SHEET, centered in the
    /// window; the mounted GLASS road (`unfloored`) rides the host's
    /// island backdrop and the dialog fills the island.
    floored: bool,

    request: hikit::modal::RequestSlot<ModalRequest>,
}

pub(crate) type PeekerEffects<'a> = imba::effect::Effects<'a, PeekerCommand>;

#[derive(Clone)]
pub enum PeekerCommand {
    /// The peeker's OWN input editor — the query lives here, not in
    /// any toolbar.
    Input(editor::editor_view::EditorCommand),

    Preview(PaneCommand),

    Pick(usize),

    Close,

    Found {
        serial: u64,
        locations: Vec<ResourceLocation>,
    },

    FetchedPreview {
        location: ResourceLocation,
        text: Option<String>,
    },

    BuiltPreview {
        location: ResourceLocation,
        document: Document,
    },

    Widget(imba::dyn_view::DynCommand),

    Rows(RowsCommand),
}

impl std::fmt::Display for PeekerCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeekerCommand::Input(command) => command.fmt(out),
            PeekerCommand::Preview(command) => command.fmt(out),
            PeekerCommand::Widget(command) => command.fmt(out),
            PeekerCommand::Rows(command) => command.fmt(out),
            PeekerCommand::Pick(_) => out.write_str("peeker pick"),
            PeekerCommand::Close => out.write_str("peeker close"),
            PeekerCommand::Found { .. } => out.write_str("peeker found"),
            PeekerCommand::FetchedPreview { .. } => out.write_str("peeker fetched preview"),
            PeekerCommand::BuiltPreview { .. } => out.write_str("peeker built preview"),
        }
    }
}

#[derive(Clone)]
struct Preview {
    document: documents::DocumentId,
    width: f32,
    pane: EditorPane,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PeekerRow {
    Recent(usize),

    Widget(usize),

    Found(usize),
}

#[derive(Clone)]
enum PreviewSlot {
    Editor(Preview),

    Widget(usize),
}

const PEEKER_SHOWN: usize = 200;

/// Keys-only controller over the file rows — the peeker's own
/// input filters; the table does the movement, the list's cursor IS
/// the selection (docs/ui/list-keyboard.md).
type Rows = hikit::list_keyboard::ListKeyboardController<
    imba::scroll::ScrollView<imba::list::ListView<FileRow, usize>>,
>;

/// One match: the file's name with the typed letters marked, its
/// directory at the right in grey. The SELECTED row grows — a
/// taller row, a larger and heavier name, a brighter directory — so
/// the row knows whether it is the one. The tail note ("… 12 more")
/// is a dim, unkeyed row.
#[derive(Clone)]
struct FileRow {
    name: String,
    marks: Vec<std::ops::Range<usize>>,
    dir: String,
    selected: bool,
    note: bool,
}

impl View for FileRow {
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
        let chrome = &peeker.jump;
        let ink = themes.part(::editor::theme::InkChrome::KEY);
        let (role, color, dir_color, height) = match (self.selected, self.note) {
            (true, _) => (
                &chrome.selected_name,
                ink.ink_bright.0,
                ink.ink_2.0,
                chrome.selected_row_height,
            ),
            (false, false) => (&chrome.name, chrome.name_color.0, ink.ink_3.0, chrome.row_height),
            (false, true) => (&chrome.name, ink.ink_3.0, ink.ink_3.0, chrome.row_height),
        };
        let plain = sheet::styled(ui, role, color);
        let mark = sheet::MatchMark::of(
            ui,
            role,
            chrome.mark_weight,
            ink.ink_bright.0,
            ink.live.0,
            chrome.mark_thickness,
            chrome.mark_offset,
        );
        // The flexible gap pushes the directory to the right edge
        // (main axis only: a bare `Fill` would drag the baseline
        // group to the row's top).
        let mut row = imba::layout::Row::new(arena)
            .child_by_baseline(
                sheet::marked_label(arena, &self.name, &self.marks, &plain, &mark).pad_insets(
                    Insets {
                        left: chrome.pad_x,
                        ..Default::default()
                    },
                ),
            )
            .weighted(1.0, imba::layout::Fill::new().height(0.0));
        if !self.dir.is_empty() {
            // Measured after the name, against what it leaves: a
            // long directory ellipsizes, the name never does.
            row = row.child_by_baseline(
                hikit::ui::text(&hikit::ui::brief(store, ui).colored(dir_color), self.dir.clone())
                    .pad_insets(Insets {
                        left: chrome.dir_gap,
                        right: chrome.pad_x,
                        ..Default::default()
                    }),
            );
        }
        // A press on the row is the LIST's: `Select`, then
        // `Activate(Click)` (docs/ui/list-keyboard.md §2).
        row.align(imba::layout::Alignment::CenterStart)
            .height(height)
            .on_event(
                move |_arena: &Arena, event: &Event<'_>, _size| match event {
                    Event::MouseDown { .. } => EventResult::handled(),
                    _ => EventResult::Ignored,
                },
            )
    }
}
type RowsCommand = hikit::list_keyboard::ListKeyCommand<
    imba::scroll::ScrollCommand<imba::list::ListCommand<std::convert::Infallible>>,
>;

fn rows_list() -> Rows {
    hikit::list_keyboard::ListKeyboardController::new(imba::scroll::ScrollView::new(
        imba::list::ListView::empty(),
    ))
}

impl Peeker {
    pub fn open(
        store: &mut Store,
        ui: &UiCtx,
        documents: imba::store::Id<documents::OpenDocuments>,
        viewport: Size,
        recents: Vec<ResourceLocation>,
        widgets: Vec<(WidgetOrigin, Box<dyn hikit::panel::DynPanelView>)>,
        folders: Vec<editor::location::ResourceLocation>,
        fx: &mut PeekerEffects<'_>,
    ) -> Self {
        let workspace = folders;
        let widget_titles: Vec<String> = widgets
            .iter()
            .map(|(_, widget)| widget.title(store))
            .collect();
        let chrome = ::editor::env::Themes::of(store).part(::hikit::chrome::PeekerChrome::KEY);
        let mut input =
            editor::editor_view::EditorView::input_styled(
                600.0,
                store,
                ui,
                editor::theme::StyleId::Query,
            );
        input.focus_text();

        let mut peeker = Self {
            input,
            documents,
            recents,
            workspace,
            found: Vec::new(),
            find_serial: 0,
            find_token: None,
            temp_docs: std::collections::HashMap::new(),
            pending_fetch: std::collections::HashSet::new(),
            widgets,
            widget_titles,
            filter_query: String::new(),
            rows: Vec::new(),
            labels: Vec::new(),
            dirs: Vec::new(),
            hidden: 0,
            list: rows_list(),
            preview: None,
            // The windowed sheet's preview: the dialog's width in
            // this viewport, less the matches column.
            preview_width: preview_width(
                sheet::Sheet(chrome.jump.sheet.clone()).width_in(viewport),
                &chrome.jump,
            ),
            floored: true,
            request: Default::default(),
        };
        peeker.filter(store, ui, "");
        peeker.ensure_preview(store, ui, fx);
        peeker
    }

    /// The GLASS face: no sheet of its own — the mounting host
    /// paints the island floor behind the whole peeker.
    pub fn unfloored(mut self) -> Self {
        self.floored = false;
        self
    }

    fn query(&self) -> String {
        let mut view = self.input.document.text().view();
        let byte_count = view.byte_count();
        view.byte_string(0, byte_count)
    }

    fn apply_query(&mut self, store: &mut Store, ui: &UiCtx, fx: &mut PeekerEffects<'_>) {
        let query = self.query();
        let query = query.trim();
        self.filter(store, ui, query);
        self.launch_find(store, ui, query, fx);
        self.ensure_preview(store, ui, fx);
    }

    fn location_at(&self, row: usize) -> Option<&ResourceLocation> {
        match self.rows.get(row) {
            Some(PeekerRow::Recent(index)) => self.recents.get(*index),
            Some(PeekerRow::Found(index)) => self.found.get(*index),
            _ => None,
        }
    }

    fn widget_at(&self, row: usize) -> Option<usize> {
        match self.rows.get(row) {
            Some(PeekerRow::Widget(index)) => Some(*index),
            _ => None,
        }
    }

    fn row_count(&self) -> usize {
        self.rows.len()
    }

    fn refilter(&mut self, store: &Store, ui: &UiCtx) {
        let query = self.filter_query.to_lowercase();
        self.rows.clear();
        self.labels.clear();
        self.hidden = 0;
        let push = |rows: &mut Vec<PeekerRow>,
                    labels: &mut Vec<String>,
                    hidden: &mut usize,
                    row: PeekerRow,
                    label: String| {
            if rows.len() < PEEKER_SHOWN {
                rows.push(row);
                labels.push(label);
            } else {
                *hidden += 1;
            }
        };
        for (index, location) in self.recents.iter().enumerate() {
            let name = location.name().to_owned();
            if subsequence_match(&name.to_lowercase(), &query) {
                push(
                    &mut self.rows,
                    &mut self.labels,
                    &mut self.hidden,
                    PeekerRow::Recent(index),
                    name,
                );
            }
        }
        for (index, title) in self.widget_titles.iter().enumerate() {
            if subsequence_match(&title.to_lowercase(), &query) {
                push(
                    &mut self.rows,
                    &mut self.labels,
                    &mut self.hidden,
                    PeekerRow::Widget(index),
                    title.clone(),
                );
            }
        }
        for (index, location) in self.found.iter().enumerate() {
            push(
                &mut self.rows,
                &mut self.labels,
                &mut self.hidden,
                PeekerRow::Found(index),
                location.name().to_owned(),
            );
        }
        self.dirs = (0..self.rows.len())
            .map(|row| {
                self.location_at(row)
                    .map(|location| self.place_of(location).1)
                    .unwrap_or_default()
            })
            .collect();
        let selected = self.selected().min(self.row_count().saturating_sub(1));
        let mut slice: imba::list::ListSlice<FileRow, usize> = imba::list::ListSlice::new();
        for row in 0..self.rows.len() {
            slice.push_keyed(row, self.file_row(row, row == selected), store, ui);
        }
        if self.hidden > 0 {
            let note = FileRow {
                name: format!("… {} more — narrow the filter", self.hidden),
                marks: Vec::new(),
                dir: String::new(),
                selected: false,
                note: true,
            };
            slice.push(note, store, ui);
        }
        let scroll_y = self.list.inner().scroll_y();
        let mut list = imba::list::ListView::from_slice(slice).with_selection(selection_style(store));
        if !self.labels.is_empty() {
            list.select_only(selected);
        }
        *self.list.inner_mut() = imba::scroll::ScrollView::new(list);
        self.list.inner_mut().set_scroll_y(scroll_y);
    }

    /// The `row`th match as a row, selected or not.
    fn file_row(&self, row: usize, selected: bool) -> FileRow {
        let name = self.labels[row].clone();
        FileRow {
            marks: sheet::match_runs(&name, &self.filter_query),
            name,
            dir: self.dirs.get(row).cloned().unwrap_or_default(),
            selected,
            note: false,
        }
    }

    /// The selection moved: the row it left shrinks back and the row
    /// it reached grows — two rows re-laid, in place.
    fn restyle(&mut self, store: &Store, ui: &UiCtx, left: Option<usize>, reached: Option<usize>) {
        for (row, selected) in [(left, false), (reached, true)] {
            let Some(row) = row.filter(|&row| row < self.rows.len()) else {
                continue;
            };
            let mut slice: imba::list::ListSlice<FileRow, usize> = imba::list::ListSlice::new();
            slice.push_keyed(row, self.file_row(row, selected), store, ui);
            self.list
                .inner_mut()
                .content_mut()
                .splice_slice(row..row + 1, slice);
        }
    }

    /// Where a file sits: its workspace folder shown from home
    /// ("~/Projects/himark-jb/", empty outside every folder) and its
    /// directory under that folder ("tools/grammar-pack/src").
    fn place_of(&self, location: &ResourceLocation) -> (String, String) {
        let path = location.path();
        let parent = &path[..path.len().saturating_sub(1)];
        match self
            .workspace
            .iter()
            .find(|folder| parent.starts_with(folder.path()))
        {
            Some(folder) => (
                format!("{}/", shown_path(folder.path())),
                parent[folder.path().len()..].join("/"),
            ),
            None => (String::new(), parent.join("/")),
        }
    }

    /// The sheet's caps label: what it does, and in which repo.
    fn caption(&self) -> String {
        match self.workspace.first() {
            Some(folder) => format!("JUMP TO FILE · {}", folder.name().to_uppercase()),
            None => "JUMP TO FILE".to_owned(),
        }
    }

    fn selected(&self) -> usize {
        use imba::list::ListOps;
        self.list.cursor_index().unwrap_or(0)
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    #[cfg(test)]
    pub(crate) fn hidden_count(&self) -> usize {
        self.hidden
    }

    /// TEST SUPPORT: no production caller outside this crate.
    #[doc(hidden)]
    pub fn preview_height(&self, store: &Store) -> Option<f32> {
        match self.preview.as_ref()? {
            PreviewSlot::Editor(preview) => {
                Some(preview.pane.content().gathered(store)?.content_height())
            }
            PreviewSlot::Widget(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn previewed_widget_title(&self) -> Option<String> {
        match self.preview.as_ref()? {
            PreviewSlot::Widget(index) => self.widget_titles.get(*index).cloned(),
            PreviewSlot::Editor(_) => None,
        }
    }

    fn filter(&mut self, store: &Store, ui: &UiCtx, query: &str) {
        self.filter_query = query.to_owned();
        self.refilter(store, ui);
    }

    fn launch_find(&mut self, store: &Store, ui: &UiCtx, query: &str, fx: &mut PeekerEffects<'_>) {
        self.find_serial += 1;
        if self.workspace.is_empty() || query.len() < 2 {
            self.found.clear();
            self.refilter(store, ui);
            return;
        }
        let serial = self.find_serial;

        let effect = imba::effect::AnyEffect::new(FindEffect {
            folders: self.workspace.clone(),
            term: query.to_owned(),
        })
        .map(move |locations| PeekerCommand::Found { serial, locations });
        fx.relaunch_erased(&mut self.find_token, effect);
    }

    fn cleanup_temps(
        &mut self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        keep: Option<documents::DocumentId>,
        fx: &mut PeekerEffects<'_>,
    ) {
        let documents = self.documents;
        if let Some(PreviewSlot::Editor(preview)) = &self.preview {
            let entity = *preview.pane.content();
            documents::lifecycle::close_editor(
                store,
                entity.documents(),
                entity.document(),
                entity.editor(),
            );
        }

        for (_, document) in self.temp_docs.drain() {
            if Some(document) != keep {
                documents::OpenDocuments::remove_if_editorless(store, documents, ui, document, fx);
            }
        }
        self.preview = None;
    }

    fn drop_preview(
        &mut self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        fx: &mut PeekerEffects<'_>,
    ) {
        if let Some(PreviewSlot::Editor(preview)) = &self.preview {
            let entity = *preview.pane.content();
            documents::lifecycle::close_editor(
                store,
                entity.documents(),
                entity.document(),
                entity.editor(),
            );

            if self.temp_docs.values().any(|id| *id == entity.document()) {
                documents::OpenDocuments::remove_if_editorless(
                    store,
                    entity.documents(),
                    ui,
                    entity.document(),
                    fx,
                );
                self.temp_docs.retain(|_, id| {
                    documents::OpenDocuments::contains(store, entity.documents(), *id)
                });
            }
        }
        self.preview = None;
    }

    fn ensure_preview(
        &mut self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        fx: &mut PeekerEffects<'_>,
    ) {
        let documents = self.documents;
        let width = EditorIdView::editor_width(
            self.preview_width,
            &::editor::env::Themes::of(store).part(::editor::theme::WindowChrome::KEY),
        );

        if let Some(index) = self.widget_at(self.selected()) {
            self.preview = Some(PreviewSlot::Widget(index));
            return;
        }

        let document_id = match self.location_at(self.selected()).cloned() {
            Some(location) => {
                match documents::OpenDocuments::by_location(store, documents, &location) {
                    Some(id) => Some(id),
                    None => match self.temp_docs.get(&location) {
                        Some(&id) => Some(id),
                        None => {
                            self.drop_preview(store, ui, fx);
                            if self.pending_fetch.insert(location.clone()) {
                                let landing = location.clone();
                                let _ = fx.push(
                                    imba::effect::AnyEffect::new(FetchDocumentEffect { location })
                                        .map(move |text| PeekerCommand::FetchedPreview {
                                            location: landing,
                                            text,
                                        }),
                                );
                            }
                            return;
                        }
                    },
                }
            }
            None => None,
        };
        let Some(document_id) = document_id else {
            self.drop_preview(store, ui, fx);
            return;
        };
        if self.preview.as_ref().is_some_and(|slot| match slot {
            PreviewSlot::Editor(preview) => {
                preview.document == document_id && preview.width == width
            }
            PreviewSlot::Widget(_) => false,
        }) {
            return;
        }
        let Some(mut document) = documents::OpenDocuments::document(store, documents, document_id)
        else {
            self.drop_preview(store, ui, fx);
            return;
        };

        let previous = match &self.preview {
            Some(PreviewSlot::Editor(preview)) => Some(*preview.pane.content()),
            _ => None,
        };
        if let Some(previous) = previous {
            documents::lifecycle::close_editor(
                store,
                previous.documents(),
                previous.document(),
                previous.editor(),
            );
        }
        let editor = fx.scope(
            |command| PeekerCommand::Preview(PaneCommand::Content(command)),
            |fx| documents::lifecycle::mount_editor(store, ui, &mut document, width, None, fx),
        );
        documents::OpenDocuments::put_document(store, documents, document_id, document);
        if let Some(previous) = previous.filter(|previous| previous.document() != document_id) {
            if self.temp_docs.values().any(|id| *id == previous.document()) {
                documents::OpenDocuments::remove_if_editorless(
                    store,
                    previous.documents(),
                    ui,
                    previous.document(),
                    fx,
                );
                self.temp_docs
                    .retain(|_, id| documents::OpenDocuments::contains(store, documents, *id));
            }
        }

        self.preview = Some(PreviewSlot::Editor(Preview {
            document: document_id,
            width,
            pane: ScrollView::new(EditorIdView::new(documents, document_id, editor).blurred()),
        }));
    }
}

impl View for Peeker {
    type Command = PeekerCommand;

    fn focus_data<'w>(
        &'w self,
        _store: &'w Store,
        _ui: &'w imba::ui::UiCtx,
    ) -> imba::focus::FocusData<'w, PeekerCommand> {
        use imba::event::EventResult;
        // Movement and Enter are the controller's table; the peeker
        // keeps its own close (and Enter-with-nothing closes too).
        let empty = self.row_count() == 0;
        let own = imba::focus::FocusData {
            commands: vec![imba::PresentableCommand::new(
                "peeker.close",
                "Close Peeker",
                PeekerCommand::Close,
            )],
            on_key: Some(Box::new(move |key, _mods| match key {
                Key::Enter if empty => EventResult::command(PeekerCommand::Close),
                Key::Escape => EventResult::command(PeekerCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..imba::focus::FocusData::default()
        };
        own.merge_under(self.list.focus_data(_store, _ui).map(PeekerCommand::Rows))
            .merge_under(self.input.focus_data(_store, _ui).map(PeekerCommand::Input))
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        if let Some(token) = self.find_token.take() {
            fx.cancel(token);
        }
        // Teardown-only: `View::destroy` carries no UiCtx.
        let ui = &imba::ui::UiCtx::dont_use_too_slow();
        self.cleanup_temps(store, ui, None, fx);
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut PeekerEffects<'_>,
    ) {
        match command {
            PeekerCommand::Input(command) => {
                fx.scope(PeekerCommand::Input, |fx| {
                    self.input.perform(store, ui, command, fx)
                });
                self.apply_query(store, ui, fx);
            }
            PeekerCommand::Preview(command) => {
                if let Some(PreviewSlot::Editor(preview)) = &mut self.preview {
                    fx.scope(PeekerCommand::Preview, |fx| {
                        preview.pane.perform(store, ui, command, fx)
                    });
                }
            }
            PeekerCommand::Widget(command) => {
                if let Some(PreviewSlot::Widget(index)) = &self.preview {
                    let index = *index;
                    if let Some((_, widget)) = self.widgets.get_mut(index) {
                        fx.scope(PeekerCommand::Widget, |fx| {
                            widget.perform_dyn(store, ui, command, fx)
                        });
                    }
                }
            }
            PeekerCommand::Rows(command) => {
                use imba::list::ListOps;
                if let Some((row, _trigger)) = Rows::activated(&command) {
                    // Enter and click both pick; the note row is
                    // unkeyed and never answers.
                    if self.list.inner().content().key_at(row).is_some() {
                        return self.perform(store, ui, PeekerCommand::Pick(row), fx);
                    }
                }
                let selected = Rows::selected_index(&command).is_some();
                let left = imba::list::ListOps::cursor_index(&self.list);
                fx.scope(PeekerCommand::Rows, |fx| {
                    imba::View::perform(&mut self.list, store, ui, command, fx)
                });
                let reached = imba::list::ListOps::cursor_index(&self.list);
                if reached != left {
                    self.restyle(store, ui, left, reached);
                }
                // Moving the selection returns the preview.
                if selected {
                    self.ensure_preview(store, ui, fx);
                }
            }
            PeekerCommand::Pick(row) => {
                if let Some(index) = self.widget_at(row) {
                    let (_, widget) = self.widgets.remove(index);
                    self.widget_titles.remove(index);
                    self.cleanup_temps(store, ui, None, fx);
                    self.request.file(ModalRequest::SelectWidget(widget));
                    return;
                }
                let request = if let Some(location) = self.location_at(row).cloned() {
                    let documents = self.documents;
                    if let Some(document) =
                        documents::OpenDocuments::by_location(store, documents, &location)
                    {
                        self.cleanup_temps(store, ui, Some(document), fx);
                        ModalRequest::ShowDocument(document)
                    } else {
                        match self.temp_docs.remove(&location) {
                            Some(document) => {
                                self.cleanup_temps(store, ui, Some(document), fx);
                                ModalRequest::ShowDocument(document)
                            }

                            None => {
                                self.cleanup_temps(store, ui, None, fx);
                                ModalRequest::OpenLocations(vec![location])
                            }
                        }
                    }
                } else {
                    self.cleanup_temps(store, ui, None, fx);
                    ModalRequest::Close
                };
                self.request.file(request);
            }
            PeekerCommand::Close => {
                self.cleanup_temps(store, ui, None, fx);
                self.request.file(ModalRequest::Close);
            }
            PeekerCommand::Found { serial, locations } => {
                if serial != self.find_serial {
                    return;
                }

                self.found = locations
                    .into_iter()
                    .filter(|location| !self.recents.contains(location))
                    .collect();
                self.refilter(store, ui);
                self.ensure_preview(store, ui, fx)
            }
            PeekerCommand::FetchedPreview { location, text } => {
                let Some(text) = text else {
                    self.pending_fetch.remove(&location);
                    return;
                };

                let landing = location.clone();
                let _ = fx.push(
                    imba::effect::AnyEffect::new(BuildDocumentEffect { location, text }).map(
                        move |built| PeekerCommand::BuiltPreview {
                            location: landing,
                            document: built.document,
                        },
                    ),
                );
            }
            PeekerCommand::BuiltPreview { location, document } => {
                self.pending_fetch.remove(&location);

                let revision = document.revision();
                let id = {
                    let documents = self.documents;
                    documents::OpenDocuments::register(
                        store,
                        documents,
                        document,
                        Some(location.clone()),
                        location.name().to_owned(),
                        revision,
                    )
                };
                self.temp_docs.insert(location.clone(), id);
                if self.location_at(self.selected()) == Some(&location) {
                    self.ensure_preview(store, ui, fx);
                }
            }
        }
    }

    /// The dialog: the matches column (the query band, the rows, the
    /// key footer) and, past one hairline, the selected file's
    /// preview under its header. Windowed, the dialog stands on its
    /// SHEET centered in the window and a press beside it closes;
    /// on the glass road the host's island is the sheet and the
    /// dialog fills it.
    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            use imba::layout::{fixed, Column};
            use imba::Thunk as _;
            let size = constraints.max;
            let themes = ::editor::env::Themes::of(store);
            let peeker = themes.part(::hikit::chrome::PeekerChrome::KEY);
            let chrome = &peeker.jump;
            let ink = themes.part(::editor::theme::InkChrome::KEY);
            let sheet = sheet::Sheet(chrome.sheet.clone());
            let dialog = match self.floored {
                true => sheet.rect_in(
                    size,
                    Size::new(sheet.width_in(size), sheet.max_height_in(size)),
                ),
                false => skia_safe::Rect::from_size(size),
            };
            let list_w = list_width(dialog.width(), chrome);
            // Live, not the open-time snapshot: the mounted host
            // resizes with its window and the preview fills to it.
            let preview_w = preview_width(dialog.width(), chrome);
            let loose = |width: f32| Constraints {
                min: Size::default(),
                max: Size::new(width, dialog.height()),
            };

            // The matches column.
            let input_width = (list_w - 2.0 * chrome.pad_x).max(1.0);
            let input = imba::layout::Layout::layout(
                self.input.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(input_width, self.input.content_height().max(1.0))),
            )
            .map(PeekerCommand::Input);
            let count = format!("{} matched", self.labels.len() + self.hidden);
            let head = Column::new(arena)
                .child(hikit::ui::text(&hikit::ui::caps(store, ui), self.caption()))
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
                .layout(arena, loose(list_w));
            let foot = sheet::key_hints(
                arena,
                &[("↑↓", "move"), ("⏎", "open"), ("esc", "close")],
                &sheet::mono(ui, chrome.footer_size, ink.ink_2.0),
                &hikit::ui::brief(store, ui).sized(chrome.footer_size),
                chrome.footer_gap,
            )
            .pad_insets(Insets {
                left: chrome.pad_x,
                right: chrome.pad_x,
                top: chrome.footer_pad_y,
                bottom: chrome.footer_pad_y,
            })
            .layout(arena, loose(list_w));
            let rows_h = (dialog.height()
                - head.size().height
                - foot.size().height
                - 2.0
                - 2.0 * chrome.list_pad_y)
                .max(chrome.row_height);
            let rows = imba::layout::Layout::layout(
                self.list.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(list_w, rows_h)),
            )
            .map(PeekerCommand::Rows);
            let matches = Column::new(arena)
                .child(fixed(head))
                .child(hikit::ui::rule_h(ink.rule.0, list_w))
                .child(fixed(rows).pad_insets(Insets {
                    top: chrome.list_pad_y,
                    bottom: chrome.list_pad_y,
                    ..Default::default()
                }))
                .child(hikit::ui::rule_h(ink.rule.0, list_w))
                .child(fixed(foot))
                .sized(list_w, dialog.height())
                .layout(arena, loose(list_w));

            // The preview: its header, then the file itself.
            let preview_head = self.preview_header(arena, store, ui, preview_w);
            let body_top = preview_head.size().height + chrome.preview_top_gap;
            let body_h = (dialog.height() - body_top).max(1.0);
            let body: imba::ThunkBox<'a, PeekerCommand> = match &self.preview {
                Some(PreviewSlot::Editor(preview)) => imba::ThunkBox::new(
                    arena,
                    imba::layout::Layout::layout(
                        preview.pane.display(arena, store, ui),
                        arena,
                        Constraints::tight(Size::new(preview_w, body_h)),
                    )
                    .map(PeekerCommand::Preview)
                    .focus_scope(false),
                ),
                Some(PreviewSlot::Widget(index)) => match self.widgets.get(*index) {
                    Some((_, widget)) => imba::ThunkBox::new(
                        arena,
                        widget
                            .layout_dyn(
                                arena,
                                store,
                                ui,
                                Constraints::tight(Size::new(preview_w, body_h)),
                            )
                            .map(PeekerCommand::Widget)
                            .focus_scope(false),
                    ),
                    None => imba::ThunkBox::new(arena, leaf(preview_w, body_h)),
                },
                None => hikit::ui::text(&hikit::ui::brief(store, ui), "no preview")
                    .pad_insets(Insets {
                        left: chrome.preview_pad_left,
                        ..Default::default()
                    })
                    .layout(arena, loose(preview_w)),
            };

            let mut sheet_box = imba::container::container(arena, dialog.size());
            sheet_box.place(
                0.0,
                0.0,
                hikit::ui::Hairline::new(ink.rule.0).right().wrap(matches),
            );
            sheet_box.place_boxed(list_w, 0.0, preview_head);
            sheet_box.place_boxed(list_w, body_top, body);
            // A press on the sheet's own ground stays on the sheet.
            let sheet_box = sheet_box.event(|_arena, event, _size| match event {
                Event::MouseDown { .. } => EventResult::handled(),
                _ => EventResult::Ignored,
            });

            let mut container = imba::container::container(arena, size);
            // The click-away catcher behind the dialog; windowed, it
            // also casts the sheet's shadow.
            let catcher =
                leaf::<PeekerCommand>(size.width, size.height).event(|_arena, event, _size| {
                    match event {
                        Event::MouseDown { .. } => EventResult::command(PeekerCommand::Close),
                        _ => EventResult::Ignored,
                    }
                });
            match self.floored {
                true => {
                    container.place(0.0, 0.0, catcher.paint_below(sheet.shadow_painter(dialog)));
                    container.place(dialog.left, dialog.top, sheet.wrap(sheet_box));
                }
                false => {
                    container.place(0.0, 0.0, catcher);
                    container.place(dialog.left, dialog.top, sheet_box);
                }
            }

            // Movement and Enter live in the controller's overlay.
            let empty = self.labels.is_empty();
            let keymap = leaf::<PeekerCommand>(size.width, size.height).event(
                move |_arena, event, _size| match event {
                    Event::KeyDown {
                        key: Key::Enter, ..
                    } if empty => EventResult::command(PeekerCommand::Close),
                    Event::KeyDown {
                        key: Key::Escape, ..
                    } => EventResult::command(PeekerCommand::Close),
                    _ => EventResult::Ignored,
                },
            );
            container.place(0.0, 0.0, keymap);

            container
        })
    }
}

impl Peeker {
    /// The preview's header: the file's name, its full path (the
    /// workspace folder quiet, the name bright), and at the far
    /// right how long the file is — one baseline, a hairline under.
    fn preview_header<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        width: f32,
    ) -> imba::ThunkBox<'a, PeekerCommand> {
        let themes = ::editor::env::Themes::of(store);
        let peeker = themes.part(::hikit::chrome::PeekerChrome::KEY);
        let chrome = &peeker.jump;
        let ink = themes.part(::editor::theme::InkChrome::KEY);
        let title = sheet::styled(ui, &chrome.preview_title, ink.ink_bright.0);
        let path = |color| sheet::mono(ui, chrome.path_size, color);
        let selected = self.selected();
        let mut row = imba::layout::Row::new(arena);
        if let Some(location) = self.location_at(selected) {
            let (folder, dir) = self.place_of(location);
            let dir = match dir.is_empty() {
                true => dir,
                false => format!("{dir}/"),
            };
            row = row
                .child_by_baseline(hikit::ui::text(&title, location.name()))
                .child_by_baseline(hikit::ui::text(&path(ink.ink_4.0), folder).pad_insets(
                    Insets {
                        left: chrome.preview_gap,
                        ..Default::default()
                    },
                ))
                .child_by_baseline(hikit::ui::text(&path(ink.ink_3.0), dir))
                .child_by_baseline(hikit::ui::text(&path(ink.ink.0), location.name()));
        } else if let Some(index) = self.widget_at(selected) {
            let name = self.widget_titles.get(index).cloned().unwrap_or_default();
            row = row.child_by_baseline(hikit::ui::text(&title, name));
        }
        row = row.weighted(1.0, imba::layout::Fill::new().height(0.0));
        let lines = match &self.preview {
            Some(PreviewSlot::Editor(preview)) => {
                documents::OpenDocuments::document(store, self.documents, preview.document)
                    .map(|document| document.text().line_count().0)
            }
            _ => None,
        };
        if let Some(lines) = lines {
            let noun = match lines {
                1 => "line",
                _ => "lines",
            };
            row = row.child_by_baseline(
                hikit::ui::text(
                    &hikit::ui::brief(store, ui).sized(chrome.meta_size),
                    format!("{lines} {noun}"),
                )
                .pad_insets(Insets {
                    left: chrome.preview_gap,
                    ..Default::default()
                }),
            );
        }
        let laid = row
            .pad_insets(Insets {
                left: chrome.preview_pad_left,
                right: chrome.preview_pad_right,
                top: chrome.preview_pad_top,
                bottom: chrome.preview_pad_bottom,
            })
            .width(width)
            .layout(
                arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(width, f32::MAX),
                },
            );
        imba::ThunkBox::new(arena, hikit::ui::Hairline::new(ink.rule.0).bottom().wrap(laid))
    }
}

fn subsequence_match(candidate: &str, query: &str) -> bool {
    let mut candidate = candidate.chars();
    query
        .chars()
        .all(|wanted| candidate.by_ref().any(|ch| ch == wanted))
}

/// The selected row's flat wash, across the matches column.
fn selection_style(store: &Store) -> imba::list::SelectionStyle {
    imba::list::SelectionStyle {
        fill: ::editor::env::Themes::of(store)
            .part(::hikit::chrome::PeekerChrome::KEY)
            .jump
            .selected_fill
            .0,
        ..hikit::rows::selection_style(store)
    }
}

/// An absolute path as the user reads it: from home as "~".
fn shown_path(segments: &[String]) -> String {
    let path = format!("/{}", segments.join("/"));
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path,
    }
}

/// The matches column's width in a dialog `width` wide — the
/// chrome's, short of crowding the preview out.
fn list_width(width: f32, chrome: &::hikit::chrome::JumpSheetChrome) -> f32 {
    chrome.list_width.min(width * 0.5).max(1.0)
}

/// The preview's width in a dialog `width` wide: what the matches
/// column and its hairline leave.
fn preview_width(width: f32, chrome: &::hikit::chrome::JumpSheetChrome) -> f32 {
    (width - list_width(width, chrome) - 1.0).max(160.0)
}

impl ModalView for Peeker {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn release_widgets(&mut self) -> Vec<(WidgetOrigin, Box<dyn hikit::panel::DynPanelView>)> {
        if matches!(self.preview, Some(PreviewSlot::Widget(_))) {
            self.preview = None;
        }
        std::mem::take(&mut self.widgets)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Build the goto-file overlay: a plain z-stacked modal layer that
/// OWNS its input — no toolbar session involved.
pub fn build(
    store: &mut Store,
    ui: &UiCtx,
    window: workbench::window::WindowId,
    fx: &mut himark::app::AppFx<'_>,
) -> Box<dyn hikit::modal::ModalView> {
    {
        let mut entity =
            workbench::window::Windows::window(store, window).expect("the window entity");
        let viewport = entity.viewport_size();

        let recents = recents::RecentLocations::list(
            store,
            himark::workspace::entity_state(&entity).recents(),
        );

        let mut widgets = entity.unmount_all_widgets();
        let fronted: Vec<hikit::pane_row::PaneRow> = widgets
            .iter()
            .filter_map(|(_, widget)| widget.pane_row())
            .collect();

        let state = himark::workspace::entity_state(&entity).clone();
        widgets.extend(
            himark::pane_rows::mint_unfronted(store, &state, &fronted)
                .into_iter()
                .map(|widget| (WidgetOrigin::Row, widget)),
        );
        let folders = ahp_session::session::folders::session_folders(
            store,
            &himark::workspace::entity_session(&entity),
        );

        let documents = himark::workspace::entity_state(&entity).documents();
        let peeker = fx.scope(workbench_shell::modal::modal_scope(window), |fx| {
            fx.scope(imba::dyn_view::DynCommand::new::<PeekerCommand>, |fx| {
                Peeker::open(
                    store, ui, documents, viewport, recents, widgets, folders, fx,
                )
            })
        });
        workbench::window::Windows::put(store, window, entity);
        Box::new(peeker)
    }
}

/// The peeker as a plugin: the toggle command and the go-to-file
/// face on the MOUNTED road — the peeker whole (query, results,
/// preview, the FSP search behind them), answered as an external
/// place arm. The peeker crate sits above himark, so himark cannot
/// name it itself.
pub struct PeekerPlugin;

impl himark::plugin::Plugin for PeekerPlugin {
    fn install(&self, app: &mut Himark, _cx: &himark::plugin::PluginContext) {
        use himark::app_ext::AppExt;
        app.register_command(std::sync::Arc::new(TogglePeeker));
        app.register_place_arm(std::sync::Arc::new(|store, ui, id, fx, place| {
            let place = place.place::<himark::places::PeekerPlace>()?;
            let key = place.0.clone();
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let documents = state.documents();
            let recents = recents::RecentLocations::list(store, state.recents());
            let folders = ahp_session::session::folders::session_folders(store, &key);
            let view = fx.scope(
                move |command: PeekerCommand| {
                    himark::app::AppCommand::ToView(id, imba::dyn_view::DynCommand::new(command))
                },
                |fx| {
                    Peeker::open(
                        store,
                        ui,
                        documents,
                        // The desktop go-to sheet's viewport (engine
                        // pixels) — near-full-window glass; display
                        // relays out live, this seeds the preview's
                        // wrap width.
                        skia_safe::Size::new(3_100.0, 1_850.0),
                        recents,
                        Vec::new(),
                        folders,
                        fx,
                    )
                    // The island's glass is the sheet — the peeker
                    // keeps only its hairlines.
                    .unfloored()
                },
            );
            Some(Box::new(view))
        }));
    }
}

pub struct TogglePeeker;

impl himark::commands::HimarkCommand for TogglePeeker {
    fn id(&self) -> &'static str {
        "peeker.toggle"
    }
    fn name(&self) -> String {
        "Go to Document".to_owned()
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
            // Toggle: any standing modal is dismissed; the peeker
            // opens only over a clear window.
            let mut entity = entity.clone();
            fx.scope(
                move |command| himark::app::content(window, command),
                |fx| entity.dismiss_modal(store, fx),
            );
            workbench::window::Windows::put(store, window, entity);
            return;
        }
        let ui = ui;
        let modal = build(store, &ui, window, fx);
        let mut entity =
            workbench::window::Windows::window(store, window).expect("the window entity");
        fx.scope(
            move |command| himark::app::content(window, command),
            |fx| entity.show_modal(store, modal, fx),
        );
        workbench::window::Windows::put(store, window, entity);
    }
}

pub fn labels(app: &Himark) -> Option<Vec<String>> {
    Some(peeker_of(app)?.labels().to_vec())
}

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub fn preview_height(app: &Himark, store: &Store) -> Option<f32> {
    peeker_of(app)?.preview_height(store)
}

#[cfg(test)]
pub(crate) fn previewed_widget(app: &Himark) -> Option<String> {
    peeker_of(app)?.previewed_widget_title()
}

fn peeker_of(app: &Himark) -> Option<&Peeker> {
    app.plugin_modal()?.as_any().downcast_ref::<Peeker>()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod widget_tests;

#[cfg(test)]
mod workspace_tests;

#[cfg(test)]
mod utf8_field_repro;
