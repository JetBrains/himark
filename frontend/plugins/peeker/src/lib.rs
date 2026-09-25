// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use himark::{
    Application, BuildDocumentEffect, Document, EditorIdView, EditorPane, FetchDocumentEffect,
    FindEffect, ModalRequest, ModalView, PaneCommand, ResourceLocation, WidgetOrigin,
};
use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult, Key},
    leaf::leaf,
    scroll::ScrollView,
    store::Store,
    thunk_ext::ThunkExt,
    Layout as _, UiCtx, View,
};
use skia_safe::{Paint, Rect, Size};

#[derive(Clone)]
pub struct Peeker {
    recents: Vec<ResourceLocation>,

    workspace: Vec<ResourceLocation>,

    found: Vec<ResourceLocation>,

    find_serial: u64,

    find_token: Option<imba::effect::CancellationToken>,

    temp_docs: std::collections::HashMap<ResourceLocation, himark::DocumentId>,

    pending_fetch: std::collections::HashSet<ResourceLocation>,

    widgets: Vec<(WidgetOrigin, Box<dyn himark::DynPanelView>)>,
    widget_titles: Vec<String>,

    filter_query: String,

    rows: Vec<PeekerRow>,
    labels: Vec<String>,

    hidden: usize,

    list: Rows,
    preview: Option<PreviewSlot>,

    preview_width: f32,

    chrome: himark::theme::PeekerChrome,

    request: himark::RequestSlot<ModalRequest>,
}

pub type PeekerEffects<'a> = imba::effect::Effects<'a, PeekerCommand>;

pub enum PeekerCommand {
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

    Widget(imba::DynCommand),

    Rows(RowsCommand),
}

#[derive(Clone)]
struct Preview {
    document: himark::DocumentId,
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

/// Keys-only controller over the raw label list — the peeker's own
/// input filters; the table does the movement, the list's cursor IS
/// the selection (docs/ui/list-keyboard.md).
type Rows = himark::ListKeyboardController<
    imba::scroll::ScrollView<imba::list::ListView<himark::LabelRow, usize>>,
>;
type RowsCommand = himark::ListKeyCommand<
    imba::scroll::ScrollCommand<imba::list::ListCommand<std::convert::Infallible>>,
>;

fn rows_list() -> Rows {
    himark::ListKeyboardController::new(imba::scroll::ScrollView::new(
        imba::list::ListView::empty(),
    ))
}

impl Peeker {
    pub fn open(
        store: &mut Store,
        ui: &UiCtx,
        viewport: Size,
        recents: Vec<ResourceLocation>,
        widgets: Vec<(WidgetOrigin, Box<dyn himark::DynPanelView>)>,
        folders: Vec<himark::ResourceLocation>,
        fx: &mut PeekerEffects<'_>,
    ) -> Self {
        let workspace = folders;
        let widget_titles: Vec<String> = widgets
            .iter()
            .map(|(_, widget)| widget.title(store))
            .collect();
        let chrome = himark::env::Themes::of(store).ui().peeker.clone();

        let mut peeker = Self {
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
            hidden: 0,
            list: rows_list(),
            preview: None,
            preview_width: preview_width(viewport, &chrome),
            chrome,
            request: Default::default(),
        };
        peeker.filter(store, ui, "");
        peeker.ensure_preview(store, ui, fx);
        peeker
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
        let selected = self.selected().min(self.row_count().saturating_sub(1));
        let note = (self.hidden > 0).then(|| format!("… {} more — narrow the filter", self.hidden));
        let scroll_y = self.list.inner().scroll_y();
        let mut list = imba::list::ListView::from_slice(himark::label_slice(
            store,
            ui,
            &self.labels,
            &[],
            note,
        ))
        .with_selection(himark::selection_style(store));
        if !self.labels.is_empty() {
            list.select_only(selected);
        }
        *self.list.inner_mut() = imba::scroll::ScrollView::new(list);
        self.list.inner_mut().set_scroll_y(scroll_y);
    }

    fn selected(&self) -> usize {
        use imba::list::ListOps;
        self.list.cursor_index().unwrap_or(0)
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    pub fn hidden_count(&self) -> usize {
        self.hidden
    }

    pub fn preview_height(&self, store: &Store) -> Option<f32> {
        match self.preview.as_ref()? {
            PreviewSlot::Editor(preview) => {
                Some(preview.pane.content().gathered(store)?.content_height())
            }
            PreviewSlot::Widget(_) => None,
        }
    }

    pub fn previewed_widget_title(&self) -> Option<String> {
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
        store: &mut Store,
        ui: &imba::UiCtx,
        keep: Option<himark::DocumentId>,
        fx: &mut PeekerEffects<'_>,
    ) {
        if let Some(PreviewSlot::Editor(preview)) = &self.preview {
            let entity = *preview.pane.content();
            himark::close_editor(store, entity.document(), entity.editor());
        }

        for (_, document) in self.temp_docs.drain() {
            if Some(document) != keep {
                himark::OpenDocuments::remove_if_editorless(store, ui, document, fx);
            }
        }
        self.preview = None;
    }

    fn drop_preview(&mut self, store: &mut Store, ui: &imba::UiCtx, fx: &mut PeekerEffects<'_>) {
        if let Some(PreviewSlot::Editor(preview)) = &self.preview {
            let entity = *preview.pane.content();
            himark::close_editor(store, entity.document(), entity.editor());

            if self.temp_docs.values().any(|id| *id == entity.document()) {
                himark::OpenDocuments::remove_if_editorless(store, ui, entity.document(), fx);
                self.temp_docs
                    .retain(|_, id| himark::OpenDocuments::contains(store, *id));
            }
        }
        self.preview = None;
    }

    fn ensure_preview(&mut self, store: &mut Store, ui: &imba::UiCtx, fx: &mut PeekerEffects<'_>) {
        let width = EditorIdView::editor_width(
            self.preview_width,
            &himark::env::Themes::of(store).ui().window,
        );

        if let Some(index) = self.widget_at(self.selected()) {
            self.preview = Some(PreviewSlot::Widget(index));
            return;
        }

        let document_id = match self.location_at(self.selected()).cloned() {
            Some(location) => match himark::OpenDocuments::by_location(store, &location) {
                Some(id) => Some(id),
                None => match self.temp_docs.get(&location) {
                    Some(&id) => Some(id),
                    None => {
                        self.drop_preview(store, ui, fx);
                        if self.pending_fetch.insert(location.clone()) {
                            let landing = location.clone();
                            let _ = fx.push(
                                imba::effect::AnyEffect::new(FetchDocumentEffect { location }).map(
                                    move |text| PeekerCommand::FetchedPreview {
                                        location: landing,
                                        text,
                                    },
                                ),
                            );
                        }
                        return;
                    }
                },
            },
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
        let Some(mut document) = himark::OpenDocuments::document(store, document_id) else {
            self.drop_preview(store, ui, fx);
            return;
        };

        let previous = match &self.preview {
            Some(PreviewSlot::Editor(preview)) => Some(*preview.pane.content()),
            _ => None,
        };
        if let Some(previous) = previous {
            himark::close_editor(store, previous.document(), previous.editor());
        }
        let editor = fx.scope(
            |command| PeekerCommand::Preview(PaneCommand::Content(command)),
            |fx| himark::mount_editor(store, ui, &mut document, width, None, fx),
        );
        himark::OpenDocuments::put_document(store, document_id, document);
        if let Some(previous) = previous.filter(|previous| previous.document() != document_id) {
            if self.temp_docs.values().any(|id| *id == previous.document()) {
                himark::OpenDocuments::remove_if_editorless(store, ui, previous.document(), fx);
                self.temp_docs
                    .retain(|_, id| himark::OpenDocuments::contains(store, *id));
            }
        }

        self.preview = Some(PreviewSlot::Editor(Preview {
            document: document_id,
            width,
            pane: ScrollView::new(EditorIdView::new(document_id, editor).blurred()),
        }));
    }
}

impl View for Peeker {
    type Command = PeekerCommand;

    fn focus_data<'w>(
        &'w self,
        _store: &'w Store,
        _ui: &'w imba::UiCtx,
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
                Key::Enter if empty => EventResult::Command(PeekerCommand::Close),
                Key::Escape => EventResult::Command(PeekerCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..imba::focus::FocusData::default()
        };
        own.merge_under(self.list.focus_data(_store, _ui).map(PeekerCommand::Rows))
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        if let Some(token) = self.find_token.take() {
            fx.cancel(token);
        }
        // Teardown-only: `View::destroy` carries no UiCtx.
        let ui = &imba::UiCtx::dont_use_too_slow();
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
                fx.scope(PeekerCommand::Rows, |fx| {
                    imba::View::perform(&mut self.list, store, ui, command, fx)
                });
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
                    if let Some(document) = himark::OpenDocuments::by_location(store, &location) {
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
                let id = himark::OpenDocuments::register(
                    store,
                    document,
                    Some(location.clone()),
                    location.name().to_owned(),
                    revision,
                );
                self.temp_docs.insert(location.clone(), id);
                if self.location_at(self.selected()) == Some(&location) {
                    self.ensure_preview(store, ui, fx);
                }
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
            let chrome = &self.chrome;
            let row_font = himark::fonts::ui_font(ui, chrome.row_size);
            let hint_font = himark::fonts::ui_font(ui, chrome.hint_size);
            let margin = chrome.margin;
            let row_height = chrome.row_height;
            let list_width = list_width(size, chrome);
            let preview_x = margin + list_width + chrome.list_preview_gap;
            let preview_width = self.preview_width;

            let inset = margin * 4.0 / 3.0;
            let list_x = inset + 1.0;
            let list_top = inset + 8.0;
            let list_height = (size.height - inset - row_height - 2.0 - list_top).max(row_height);

            let sheet_rule = himark::env::Themes::of(store).ui().toolbar.rule.0;
            let match_count = self.labels.len();
            let hidden = self.hidden;
            let has_preview = self.preview.is_some();

            let mut container = imba::container::container(arena, size);

            let backdrop = leaf::<PeekerCommand>(size.width, size.height)
                .paint_instead(move |_arena, canvas, rect| {
                    let chrome = &self.chrome;
                    let mut surface = Paint::default();
                    surface.set_color(chrome.background.0);
                    canvas.draw_rect(rect, &surface);

                    let panel = Rect::from_xywh(
                        inset,
                        inset,
                        list_width + 2.0,
                        (size.height - inset * 2.0).max(1.0),
                    );

                    let echo = inset * 0.5;
                    let sheet = panel.with_offset((echo, echo));
                    let mut rule = Paint::default();
                    surface.set_color(chrome.background.0);
                    canvas.draw_rect(sheet, &surface);
                    rule.set_color(sheet_rule);
                    for edge in [
                        Rect::from_xywh(sheet.left, sheet.bottom - 1.0, sheet.width(), 1.0),
                        Rect::from_xywh(sheet.right - 1.0, sheet.top, 1.0, sheet.height()),
                    ] {
                        canvas.draw_rect(edge, &rule);
                    }
                    surface.set_color(chrome.background.0);
                    canvas.draw_rect(panel, &surface);
                    rule.set_color(chrome.rule.0);
                    for edge in [
                        Rect::from_xywh(panel.left, panel.top, panel.width(), 1.0),
                        Rect::from_xywh(panel.left, panel.bottom - 1.0, panel.width(), 1.0),
                        Rect::from_xywh(panel.left, panel.top, 1.0, panel.height()),
                        Rect::from_xywh(panel.right - 1.0, panel.top, 1.0, panel.height()),
                    ] {
                        canvas.draw_rect(edge, &rule);
                    }
                    rule.set_color(chrome.rule.0);
                    canvas.draw_rect(
                        Rect::from_xywh(
                            panel.left,
                            panel.bottom - chrome.row_height,
                            panel.width(),
                            1.0,
                        ),
                        &rule,
                    );
                })
                .event(|_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::Command(PeekerCommand::Close),
                    _ => EventResult::Ignored,
                });
            container.place(0.0, 0.0, backdrop);

            // The chrome labels as `imba::text`, centered in the
            // bottom row band (the design-system row rule); the texts
            // ignore presses, so the backdrop's close-on-click still
            // answers underneath them.
            let panel_bottom = inset + (size.height - inset * 2.0).max(1.0);
            let hint_metrics = hint_font.metrics().1;
            let hint_height = (-hint_metrics.ascent + hint_metrics.descent)
                .ceil()
                .max(1.0);
            container.place_boxed(
                inset + chrome.row_text_x,
                panel_bottom - chrome.row_height
                    + ((chrome.row_height - hint_height) * 0.5).max(0.0),
                imba::text(
                    ui,
                    format!(
                        "{} matched   enter open   esc dismiss",
                        match_count + hidden
                    ),
                    hint_font.clone(),
                    chrome.dim_text.0,
                )
                .layout(arena, Constraints::tight(size).loosen()),
            );
            if !has_preview {
                let row_ascent = -row_font.metrics().1.ascent;
                container.place_boxed(
                    preview_x,
                    list_top + chrome.no_preview_offset - row_ascent,
                    imba::text(ui, "no preview", row_font.clone(), chrome.dim_text.0)
                        .layout(arena, Constraints::tight(size).loosen()),
                );
            }

            if let Some(slot) = &self.preview {
                let preview_height = (size.height - list_top - margin).max(1.0);
                match slot {
                    PreviewSlot::Editor(preview) => {
                        let pane = imba::Layout::layout(
                            preview.pane.display(arena, store, ui),
                            arena,
                            Constraints::tight(Size::new(preview_width, preview_height)),
                        )
                        .map(PeekerCommand::Preview)
                        .focus_scope(false);
                        container.place(preview_x, list_top, pane);
                    }
                    PreviewSlot::Widget(index) => {
                        if let Some((_, widget)) = self.widgets.get(*index) {
                            let mounted = widget
                                .layout_dyn(
                                    arena,
                                    store,
                                    ui,
                                    Constraints::tight(Size::new(preview_width, preview_height)),
                                )
                                .map(PeekerCommand::Widget)
                                .focus_scope(false);
                            container.place(preview_x, list_top, mounted);
                        }
                    }
                }

                let shield = leaf::<PeekerCommand>(preview_width, preview_height).event(
                    |_arena, event, _size| match event {
                        Event::MouseDown { .. } => EventResult::Handled,
                        _ => EventResult::Ignored,
                    },
                );
                container.place(preview_x, list_top, shield);
            }

            container.place(
                list_x,
                list_top,
                imba::Layout::layout(
                    self.list.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(list_width, list_height)),
                )
                .map(PeekerCommand::Rows),
            );

            // Movement and Enter live in the controller's overlay.
            let empty = match_count == 0;
            let keymap = leaf::<PeekerCommand>(size.width, size.height).event(
                move |_arena, event, _size| match event {
                    Event::KeyDown {
                        key: Key::Enter, ..
                    } if empty => EventResult::Command(PeekerCommand::Close),
                    Event::KeyDown {
                        key: Key::Escape, ..
                    } => EventResult::Command(PeekerCommand::Close),
                    _ => EventResult::Ignored,
                },
            );
            container.place(0.0, 0.0, keymap);

            container
        })
    }
}

fn subsequence_match(candidate: &str, query: &str) -> bool {
    let mut candidate = candidate.chars();
    query
        .chars()
        .all(|wanted| candidate.by_ref().any(|ch| ch == wanted))
}

fn list_width(size: Size, chrome: &himark::theme::PeekerChrome) -> f32 {
    (size.width * chrome.list_ratio).clamp(chrome.list_min, chrome.list_max)
}

fn preview_width(size: Size, chrome: &himark::theme::PeekerChrome) -> f32 {
    (size.width - list_width(size, chrome) - chrome.margin * 2.0 - chrome.preview_margin).max(160.0)
}

impl ModalView for Peeker {
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
        fx: &mut imba::effect::Effects<'_, imba::DynCommand>,
    ) {
        fx.scope(
            |command: PeekerCommand| Box::new(command) as imba::DynCommand,
            |fx| {
                let query = query.trim();
                self.filter(store, ui, query);
                self.launch_find(store, ui, query, fx);
                self.ensure_preview(store, ui, fx);
            },
        )
    }

    fn release_widgets(&mut self) -> Vec<(WidgetOrigin, Box<dyn himark::DynPanelView>)> {
        if matches!(self.preview, Some(PreviewSlot::Widget(_))) {
            self.preview = None;
        }
        std::mem::take(&mut self.widgets)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub fn overlay_surface() -> himark::OverlaySurface {
    himark::OverlaySurface {
        prefix: None,
        open: std::sync::Arc::new(|store, ui, window, fx| {
            let mut entity = himark::Windows::window(store, window).expect("the window entity");
            let viewport = entity.viewport_size();

            let recents = himark::RecentLocations::list(store);

            let mut widgets = entity.unmount_all_widgets();
            let fronted: Vec<himark::FamilyRow> = widgets
                .iter()
                .filter_map(|(_, widget)| widget.family_row())
                .collect();

            widgets.extend(
                himark::mint_unfronted(store, &fronted)
                    .into_iter()
                    .map(|widget| (WidgetOrigin::Family, widget)),
            );
            let folders = himark::higent::session_folders(store, &entity.current_session());

            let peeker = fx.scope(himark::modal_scope(window), |fx| {
                fx.scope(
                    |command: PeekerCommand| Box::new(command) as imba::DynCommand,
                    |fx| Peeker::open(store, ui, viewport, recents, widgets, folders, fx),
                )
            });
            himark::Windows::put(store, window, entity);
            Box::new(peeker)
        }),
    }
}

pub struct TogglePeeker;

impl himark::DynamicCommand for TogglePeeker {
    fn id(&self) -> &'static str {
        "peeker.toggle"
    }
    fn name(&self) -> String {
        "Go to Document".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        let entity = himark::Windows::window_ref(store, window).expect("the window entity");
        if entity.has_modal()
            && entity
                .plugin_modal()
                .is_none_or(|m| !m.as_any().is::<Peeker>())
        {
            let mut entity = entity.clone();
            fx.scope(
                move |command| himark::AppCommand::Content(window, command),
                |fx| entity.dismiss_modal(store, fx),
            );
            himark::Windows::put(store, window, entity);
            return;
        }
        let ui = app.ui_handle();
        himark::toggle_toolbar_session(store, &ui, window, &overlay_surface(), "", fx);
    }
}

pub fn labels(app: &Application) -> Option<Vec<String>> {
    Some(peeker_of(app)?.labels().to_vec())
}

pub fn preview_height(app: &Application, store: &Store) -> Option<f32> {
    peeker_of(app)?.preview_height(store)
}

pub fn previewed_widget(app: &Application) -> Option<String> {
    peeker_of(app)?.previewed_widget_title()
}

fn peeker_of(app: &Application) -> Option<&Peeker> {
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
