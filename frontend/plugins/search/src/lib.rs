// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use himark::{Document, EditorCommand, EditorView};
use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effect,
    event::{Event, EventResult},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    Layout as _, LayoutExt as _, UiCtx, View, Widget,
};
use skia_safe::{Paint, Rect, Size};
use text::Text;

use himark::{
    FindEffect, FindTarget, GroupSpans, LocationList, LocationListCommand, ModalRequest, ModalView,
    PanelView, ResourceLocation,
};

const MAX_HITS_PER_DOCUMENT: usize = 50;
const MAX_HITS_TOTAL: usize = 150;

const MODAL_SHOWN: usize = 100;

const SCAN_WINDOW: usize = 64 * 1024;

const MIN_QUERY: usize = 2;

const MAX_WORKSPACE_DOCS: usize = 50;

const CONTEXT_LINES: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchArea {
    Input,
    Results,
}

pub enum SearchCommand {
    Input(EditorCommand),

    Pin,

    Close,

    Contents(himark::TocCommand),

    List(ScrollCommand<LocationListCommand>),

    Landed(SearchHits),

    Focus(SearchArea, Option<Box<SearchCommand>>),

    FoundLocations {
        serial: u64,
        locations: Vec<ResourceLocation>,
    },

    FetchRest,
}

pub struct SearchHits {
    serial: u64,

    hits: Vec<(himark::DocumentId, GroupSpans, himark::PrebuiltRows)>,
}

pub struct SearchView {
    input: EditorView,

    results: himark::ListId,
    last_query: String,
    serial: u64,

    scan_token: Option<imba::effect::CancellationToken>,
    find_token: Option<imba::effect::CancellationToken>,
    focus: SearchArea,

    workspace: Option<himark::SessionId>,

    docked: bool,

    window: Option<himark::WindowId>,

    contents: Option<himark::TocView>,
    contents_generation: Option<u64>,

    scanned: u64,

    pending_find: Option<(u64, Vec<ResourceLocation>)>,

    unfetched: Vec<ResourceLocation>,

    lift_pending: bool,

    request: himark::RequestSlot<ModalRequest>,

    laid_width: std::sync::atomic::AtomicU32,
}

impl Clone for SearchView {
    fn clone(&self) -> Self {
        Self {
            input: self.input.clone(),
            results: self.results,
            last_query: self.last_query.clone(),
            serial: self.serial,
            scan_token: self.scan_token,
            find_token: self.find_token,
            focus: self.focus,
            workspace: self.workspace.clone(),
            docked: self.docked,
            window: self.window,
            contents: self.contents.clone(),
            contents_generation: self.contents_generation,
            scanned: self.scanned,
            pending_find: self.pending_find.clone(),
            unfetched: self.unfetched.clone(),
            lift_pending: self.lift_pending,
            request: self.request.clone(),
            laid_width: std::sync::atomic::AtomicU32::new(
                self.laid_width.load(std::sync::atomic::Ordering::Relaxed),
            ),
        }
    }
}

fn hold_back_note(shown: usize) -> String {
    format!("showing the first {shown} matches — pin the panel to load the rest")
}

impl SearchView {
    pub fn new() -> Self {
        Self::for_workspace(None)
    }

    pub fn for_workspace(workspace: Option<himark::SessionId>) -> Self {
        Self {
            input: EditorView::input(600.0, himark::fonts::source()),
            results: himark::ListId::mint(),
            last_query: String::new(),
            serial: 0,
            scan_token: None,
            find_token: None,
            focus: SearchArea::Input,
            workspace,
            docked: true,
            window: None,
            contents: None,
            contents_generation: None,
            scanned: 0,
            pending_find: None,
            unfetched: Vec::new(),
            lift_pending: false,
            request: Default::default(),
            laid_width: std::sync::atomic::AtomicU32::new(0),
        }
    }

    pub fn modal(workspace: Option<himark::SessionId>, window: himark::WindowId) -> Self {
        Self {
            docked: false,
            window: Some(window),
            contents: None,
            contents_generation: None,
            scanned: 0,
            pending_find: None,
            unfetched: Vec::new(),
            lift_pending: false,
            focus: SearchArea::Results,
            ..Self::for_workspace(workspace)
        }
    }

    #[doc(hidden)]
    pub fn contents_rows(&self) -> Vec<(u8, String, bool)> {
        self.contents
            .as_ref()
            .map(|contents| contents.rows())
            .unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn group_names(&self, store: &Store) -> Vec<String> {
        self.list_ref(store)
            .map(|list| list.content().group_names())
            .unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn group_heights(&self, store: &Store) -> Vec<f32> {
        self.list_ref(store)
            .map(|list| list.content().group_heights())
            .unwrap_or_default()
    }

    pub fn group_sizes(&self, store: &Store) -> Vec<usize> {
        self.list_ref(store)
            .map(|list| list.content().group_sizes())
            .unwrap_or_default()
    }

    pub fn group_row_editors(
        &self,
        store: &Store,
        group: usize,
    ) -> Vec<(himark::DocumentId, himark::EditorId)> {
        self.list_ref(store)
            .map(|list| list.content().group_row_editors(group))
            .unwrap_or_default()
    }

    fn list_ref<'a>(&self, store: &'a Store) -> Option<&'a ScrollView<LocationList>> {
        himark::LocationLists::entry_ref(store, self.results).map(|entry| &entry.list)
    }

    fn query(&self) -> String {
        let end = self
            .input
            .document
            .text()
            .byte_count()
            .min(u32::MAX as usize) as u32;
        self.input.document.text().view().substring(0..end)
    }

    fn requery(
        &mut self,
        store: &mut Store,
        query: String,
        fx: &mut imba::effect::Effects<'_, SearchCommand>,
    ) {
        if query == self.last_query {
            return;
        }
        self.last_query = query.clone();
        self.serial += 1;

        if query.trim().len() < MIN_QUERY {
            if let Some(previous) = self.find_token.take() {
                fx.cancel(previous);
            }
            fx.relaunch(
                &mut self.scan_token,
                EmptyScan {
                    serial: self.serial,
                },
            );
            return;
        }

        let targets: Vec<(himark::DocumentId, himark::Document)> =
            himark::OpenDocuments::list(store)
                .into_iter()
                .map(|(id, entity)| (id, entity.document().substance()))
                .collect();

        let entry = self.take_list(store);
        let laid = f32::from_bits(self.laid_width.load(std::sync::atomic::Ordering::Relaxed));
        if laid > 1.0 {
            entry.list.content().note_panel_width(store, laid);
        }
        let width = entry.list.content().install_width(store);
        self.put_list(store, entry);
        fx.relaunch(
            &mut self.scan_token,
            SearchEffect {
                serial: self.serial,
                query: query.clone(),
                targets,
                width,
            },
        );

        let folders = match &self.workspace {
            Some(workspace) => himark::higent::session_folders(store, workspace),
            None => Vec::new(),
        };
        if folders.is_empty() {
            if let Some(previous) = self.find_token.take() {
                fx.cancel(previous);
            }
        } else {
            let serial = self.serial;
            fx.relaunch_erased(
                &mut self.find_token,
                imba::effect::AnyEffect::new(FindEffect {
                    folders,
                    term: query,
                    target: FindTarget::Text,
                })
                .map(move |locations| SearchCommand::FoundLocations { serial, locations }),
            );
        }
    }

    #[doc(hidden)]
    pub fn contents_probe(&self, store: &Store) -> Option<Vec<String>> {
        self.contents.as_ref()?;
        Some(
            self.list_ref(store)?
                .content()
                .result_locations()
                .iter()
                .map(|location| location.name().to_owned())
                .collect(),
        )
    }

    fn found_locations(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        mut locations: Vec<ResourceLocation>,
        fx: &mut imba::effect::Effects<'_, SearchCommand>,
    ) {
        himark::sort_locations(&mut locations);
        let mut entry = self.take_list(store);
        entry.list.content_mut().note_locations(locations.clone());
        let mut fetch: Vec<ResourceLocation> = locations
            .into_iter()
            .filter(|location| himark::OpenDocuments::by_location(store, location).is_none())
            .collect();
        self.unfetched = fetch.split_off(fetch.len().min(MAX_WORKSPACE_DOCS));
        fx.scope(
            |command| SearchCommand::List(ScrollCommand::Content(command)),
            |fx| entry.list.content_mut().fetch(fetch, fx),
        );
        self.put_list(store, entry);
        self.refresh_contents(store, ui);
    }

    fn refresh_contents(&mut self, store: &Store, ui: &UiCtx) {
        let Some(window) = self.window else {
            return;
        };
        let Some(list) = self.list_ref(store) else {
            return;
        };
        let generation = list.content().set_generation();
        if self.contents_generation == Some(generation) {
            return;
        }
        let set = list.content().result_locations();
        self.contents = himark::TocView::for_locations(store, ui, window, &set);
        self.contents_generation = Some(generation);
    }

    fn span_source(&self) -> himark::SpanSource {
        let query = self.last_query.clone();
        std::sync::Arc::new(move |_location: &ResourceLocation, document: &Document| {
            let matcher = Matcher::new(&query);
            let (ranges, marks) = scan(&matcher, document.text());
            GroupSpans { ranges, marks }
        })
    }

    pub fn uninstall(&mut self, store: &mut Store) {
        if let Some(mut entry) = himark::LocationLists::take(store, self.results) {
            entry.list.content_mut().uninstall(store);
        }
    }

    fn take_list(&self, store: &mut Store) -> himark::ListEntry {
        himark::LocationLists::take(store, self.results).unwrap_or_else(|| himark::ListEntry {
            title: String::new(),
            list: match self.docked {
                true => ScrollView::new(LocationList::new()),
                false => ScrollView::new(LocationList::with_budget(MODAL_SHOWN, hold_back_note)),
            },
        })
    }

    fn put_list(&self, store: &mut Store, mut entry: himark::ListEntry) {
        entry.title = match self.last_query.trim() {
            "" => "Search".to_owned(),
            query => format!("Search: {query}"),
        };
        himark::LocationLists::put(store, self.results, entry);
    }
}

impl Default for SearchView {
    fn default() -> Self {
        Self::new()
    }
}

impl View for SearchView {
    type Command = SearchCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::UiCtx,
    ) -> imba::focus::FocusData<'w, SearchCommand> {
        use imba::focus::FocusData;
        let modal = !self.docked;
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                imba::event::Key::Escape if modal => {
                    imba::event::EventResult::Command(SearchCommand::Close)
                }
                _ => imba::event::EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        let area = match self.focus {
            SearchArea::Input => self.input.focus_data(store, ui).map(SearchCommand::Input),
            SearchArea::Results => self
                .list_ref(store)
                .map(|list| list.focus_data(store, ui).map(SearchCommand::List))
                .unwrap_or_default(),
        };
        own.merge_under(area)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        if let Some(token) = self.scan_token.take() {
            fx.cancel(token);
        }
        if let Some(token) = self.find_token.take() {
            fx.cancel(token);
        }

        if let Some(mut entry) = himark::LocationLists::take(store, self.results) {
            fx.scope(SearchCommand::List, |fx| entry.list.destroy(store, fx));
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            SearchCommand::Input(command) => {
                fx.scope(SearchCommand::Input, |fx| {
                    imba::View::perform(&mut self.input, store, ui, command, fx)
                });
                let query = self.query();
                self.requery(store, query, fx);
            }
            SearchCommand::List(command) => {
                let mut entry = self.take_list(store);
                fx.scope(SearchCommand::List, |fx| {
                    entry.list.perform(store, ui, command, fx)
                });
                self.put_list(store, entry);
            }
            SearchCommand::Pin => {
                self.docked = true;
                self.input = seeded_input(&self.last_query);
                self.focus = SearchArea::Input;

                self.lift_pending = true;
                let widget = Box::new(self.clone());

                self.results = himark::ListId::mint();
                self.scan_token = None;
                self.find_token = None;
                self.request.file(ModalRequest::SelectWidget(widget));
            }
            SearchCommand::Close => {
                self.uninstall(store);
                self.request.file(ModalRequest::Close);
            }
            SearchCommand::Contents(command) => {
                if let Some(contents) = &mut self.contents {
                    fx.scope(SearchCommand::Contents, |fx| {
                        imba::View::perform(contents, store, ui, command, fx)
                    });
                }
            }
            SearchCommand::Landed(hits) => {
                if hits.serial == self.serial {
                    let fonts = himark::env::ui_collection(store, ui);
                    let groups = hits
                        .hits
                        .into_iter()
                        .map(|(document, spans, prebuilt)| {
                            himark::InstallGroup::prebuilt(document, spans, prebuilt)
                        })
                        .collect();
                    let spans = self.span_source();
                    let mut entry = self.take_list(store);
                    fx.scope(
                        |command| SearchCommand::List(ScrollCommand::Content(command)),
                        |fx| {
                            entry.list.content_mut().install(
                                store,
                                ui,
                                &fonts,
                                groups,
                                Some(spans),
                                fx,
                            )
                        },
                    );

                    entry.list.set_scroll_y(0.0);
                    self.put_list(store, entry);

                    self.refresh_contents(store, ui);

                    self.scanned = hits.serial;
                    if let Some((serial, locations)) = self.pending_find.take() {
                        if serial == self.serial {
                            self.found_locations(store, ui, locations, fx);
                        }
                    }
                }
            }
            SearchCommand::Focus(area, then) => {
                self.focus = area;
                match area {
                    SearchArea::Input => self.input.focus_text(),
                    SearchArea::Results => self.input.blur(),
                }
                if let Some(command) = then {
                    self.perform(store, ui, *command, fx);
                }
            }
            SearchCommand::FetchRest => {
                let mut entry = self.take_list(store);
                if self.lift_pending {
                    self.lift_pending = false;
                    let fonts = himark::env::ui_collection(store, ui);
                    fx.scope(
                        |command| SearchCommand::List(ScrollCommand::Content(command)),
                        |fx| entry.list.content_mut().lift_budget(store, ui, &fonts, fx),
                    );
                }
                let rest = std::mem::take(&mut self.unfetched);
                fx.scope(
                    |command| SearchCommand::List(ScrollCommand::Content(command)),
                    |fx| entry.list.content_mut().fetch(rest, fx),
                );
                self.put_list(store, entry);
            }
            SearchCommand::FoundLocations { serial, locations } => {
                if serial != self.serial {
                    return;
                }
                if self.scanned == serial {
                    self.found_locations(store, ui, locations, fx);
                } else {
                    self.pending_find = Some((serial, locations));
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
            let thunk: imba::ThunkBox<'a, SearchCommand> = match self.docked {
                true => {
                    imba::ThunkBox::new(arena, self.layout_docked(arena, store, ui, constraints))
                }
                false => {
                    imba::ThunkBox::new(arena, self.layout_modal(arena, store, ui, constraints))
                }
            };
            thunk
        })
    }
}

impl SearchView {
    fn layout_docked<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl imba::Thunk<'a, SearchCommand> + 'a {
        let size = constraints.max;
        let chrome = himark::env::Themes::of(store).ui().search.clone();
        let pad = chrome.pad;
        let input_height = chrome.input_height;
        let input_bottom = pad + input_height;
        let mut panel = imba::container::container(arena, size);
        self.laid_width.store(
            size.width.max(1.0).to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
        let inner_height = (input_height - chrome.input_pad_y * 2.0).max(1.0);
        panel.place(
            pad + chrome.input_pad_x,
            pad + chrome.input_pad_y,
            imba::Layout::layout(
                self.input.display(arena, store, ui),
                arena,
                Constraints {
                    min: Size::new(0.0, inner_height),
                    max: Size::new(
                        (size.width - (pad + chrome.input_pad_x) * 2.0).max(1.0),
                        inner_height,
                    ),
                },
            )
            .map(SearchCommand::Input)
            .focus_scope(self.focus == SearchArea::Input),
        );
        match self.list_ref(store) {
            Some(list) => panel.place(
                0.0,
                input_bottom + pad,
                imba::Layout::layout(
                    list.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(
                        size.width,
                        (size.height - input_bottom - pad).max(1.0),
                    )),
                )
                .map(SearchCommand::List)
                .focus_scope(self.focus == SearchArea::Results),
            ),

            None => panel.place(
                0.0,
                input_bottom + pad,
                imba::leaf::leaf::<SearchCommand>(
                    size.width,
                    (size.height - input_bottom - pad).max(1.0),
                ),
            ),
        }
        let focus = self.focus;
        panel.wrap_realized(move |panel| SearchWidget {
            fetch_rest: (self.lift_pending || !self.unfetched.is_empty()) && self.docked,
            panel,
            size,
            input_bottom: input_bottom + pad,
            focus,
            chrome,
            input_index: 0,
            results_index: 1,
            modal: false,
        })
    }

    fn layout_modal<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl imba::Thunk<'a, SearchCommand> + 'a {
        use imba::leaf::leaf;
        let size = constraints.max;
        let themes = himark::env::Themes::of(store);
        let chrome = themes.ui().search.clone();
        let surface = themes.ui().peeker.clone();

        let pin_height = surface.row_height;
        let pin_width = pin_height * 1.9;
        let pin_radius = chrome.input_outset.max(6.0);
        let results_top = surface.margin + pin_height + surface.list_top_gap;
        let results_height = (size.height - results_top).max(1.0);

        let mut panel = imba::container::container(arena, size);

        let backdrop_surface = surface.clone();
        let backdrop = leaf::<SearchCommand>(size.width, size.height)
            .paint_instead(move |_arena, canvas, rect| {
                let mut paint = Paint::default();
                paint.set_color(backdrop_surface.background.0);
                canvas.draw_rect(rect, &paint);
            })
            .event(|_arena, event, _size| match event {
                Event::MouseDown { .. } => EventResult::Command(SearchCommand::Close),
                _ => EventResult::Ignored,
            });
        panel.place(0.0, 0.0, backdrop);

        // The PIN chip's label is an `imba::text` at exact parity:
        // the text self-measures the same advance the old painter
        // centered by (top alignment centers x, pads down so the
        // baseline lands on the old height * 0.5 + 6.0 line); the
        // round-rect stroke stays a backdrop painter, and the press
        // is `.on_click`, minting Pin like the old event closure.
        let pin_font = himark::fonts::ui_font(ui, surface.hint_size);
        let pin_surface = surface.clone();
        let pin_ascent = -pin_font.metrics().1.ascent;
        let pin = imba::text("PIN", pin_font, pin_surface.dim_text.0)
            .pad_insets(imba::Insets {
                left: 0.0,
                top: (pin_height * 0.5 + 6.0 - pin_ascent).max(0.0),
                right: 0.0,
                bottom: 0.0,
            })
            .align(imba::Alignment::TopCenter)
            .sized(pin_width, pin_height)
            .backdrop(
                move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: Rect| {
                    let mut paint = Paint::default();
                    paint.set_anti_alias(true);
                    paint.set_stroke(true);
                    paint.set_stroke_width(1.0);
                    paint.set_color(pin_surface.rule.0);
                    canvas.draw_round_rect(
                        rect.with_inset((0.5, 0.5)),
                        pin_radius,
                        pin_radius,
                        &paint,
                    );
                },
            )
            .on_click(|| SearchCommand::Pin);
        panel.place_boxed(
            (size.width - surface.margin - pin_width).max(0.0),
            surface.margin,
            pin.layout(arena, Constraints::tight(Size::new(pin_width, pin_height))),
        );

        if let Some(contents) = &self.contents {
            let tree = imba::Layout::layout(
                contents.display(arena, store, ui),
                arena,
                Constraints::tight(Size::new(himark::DRAWER_WIDTH, size.height)),
            )
            .map(SearchCommand::Contents);
            panel.place(0.0, 0.0, MouseOnly(tree));
        }

        let results_x = match self.contents {
            Some(_) => himark::DRAWER_WIDTH - surface.margin * 4.0 / 3.0,
            None => 0.0,
        };
        self.laid_width.store(
            (size.width - results_x).max(160.0).to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
        match self.list_ref(store) {
            Some(list) => panel.place(
                results_x,
                results_top,
                imba::Layout::layout(
                    list.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(
                        (size.width - results_x).max(160.0),
                        results_height,
                    )),
                )
                .map(SearchCommand::List)
                .focus_scope(true),
            ),

            None => panel.place(
                results_x,
                results_top,
                imba::leaf::leaf::<SearchCommand>(
                    (size.width - results_x).max(160.0),
                    results_height,
                ),
            ),
        }

        let results_index = match self.contents {
            Some(_) => 3,
            None => 2,
        };
        panel.wrap_realized(move |panel| SearchWidget {
            fetch_rest: (self.lift_pending || !self.unfetched.is_empty()) && self.docked,
            panel,
            size,

            input_bottom: 0.0,
            focus: SearchArea::Results,
            chrome,
            input_index: results_index,
            results_index,
            modal: true,
        })
    }
}

struct SearchWidget<'a> {
    fetch_rest: bool,

    panel: imba::container::RealizedContainer<'a, SearchCommand>,
    size: Size,

    input_bottom: f32,
    focus: SearchArea,
    chrome: himark::theme::SearchChrome,
    input_index: usize,
    results_index: usize,

    modal: bool,
}

impl<'a> Widget<'a, SearchCommand> for SearchWidget<'a> {
    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, SearchCommand>> {
        self.panel.overlays()
    }

    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<SearchCommand> {
        match event {
            Event::Paint { canvas, .. } => {
                if self.fetch_rest {
                    return EventResult::Command(SearchCommand::FetchRest)
                        .merge(self.panel.handle_event(arena, event, viewport));
                }
                if !self.modal {
                    let chrome = &self.chrome;
                    let box_edge = chrome.pad - chrome.input_outset;
                    let mut paint = Paint::default();
                    paint.set_anti_alias(true);
                    paint.set_color(chrome.input_fill.0);
                    canvas.draw_rect(
                        Rect::from_xywh(
                            box_edge,
                            box_edge,
                            self.size.width - box_edge * 2.0,
                            chrome.input_height + chrome.input_outset * 2.0,
                        ),
                        &paint,
                    );
                }
                self.panel.handle_event(arena, event, viewport)
            }

            Event::MouseDown { point, .. } => {
                let area = if point.y < self.input_bottom {
                    SearchArea::Input
                } else {
                    SearchArea::Results
                };
                match self.panel.handle_event(arena, event, viewport) {
                    EventResult::Command(command) => {
                        EventResult::Command(SearchCommand::Focus(area, Some(Box::new(command))))
                    }
                    _ => EventResult::Command(SearchCommand::Focus(area, None)),
                }
            }
            // Settle is a BROADCAST like paint: every scroll host
            // re-observes its viewport top on the pulse
            // (docs/editor/viewport-preservation.md §3.1) — routing it to
            // the focused area only would leave the other list
            // reporting drift on every later paint.
            Event::Scroll { .. }
            | Event::AnimationClock { .. }
            | Event::ThemeChanged
            | Event::Settle => self.panel.handle_event(arena, event, viewport),

            _ => {
                let index = match self.focus {
                    SearchArea::Input => self.input_index,
                    SearchArea::Results => self.results_index,
                };
                self.panel.route_to(index, arena, event, viewport)
            }
        }
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, SearchCommand>
    where
        'a: 'w,
    {
        self.panel.layout_data(target)
    }
}

struct SearchEffect {
    serial: u64,
    query: String,
    targets: Vec<(himark::DocumentId, himark::Document)>,

    width: f32,
}

struct EmptyScan {
    serial: u64,
}

impl EmptyScan {
    fn execute(self) -> SearchCommand {
        SearchCommand::Landed(SearchHits {
            serial: self.serial,
            hits: Vec::new(),
        })
    }
}

impl Effect for EmptyScan {
    type Result = SearchCommand;
}

enum Matcher {
    Regex(regex::Regex),
    Literal(String),
}

impl Matcher {
    fn new(query: &str) -> Self {
        let build = |pattern: &str| {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
        };
        match build(query).or_else(|_| build(&regex::escape(query))) {
            Ok(regex) => Self::Regex(regex),
            Err(_) => Self::Literal(query.to_owned()),
        }
    }

    fn find_in<'h>(&self, haystack: &'h str) -> Vec<Range<usize>> {
        match self {
            Self::Regex(regex) => regex
                .find_iter(haystack)
                .filter(|found| !found.is_empty())
                .map(|found| found.range())
                .collect(),
            Self::Literal(needle) => haystack
                .match_indices(needle.as_str())
                .map(|(at, _)| at..at + needle.len())
                .collect(),
        }
    }
}

impl SearchEffect {
    fn execute(self, workshop: &himark::Workshop) -> SearchCommand {
        let matcher = Matcher::new(&self.query);
        let fonts = workshop.fonts();
        let theme = workshop.theme();
        let mut hits = Vec::new();
        let mut total = 0usize;
        for (document_id, document) in &self.targets {
            if total >= MAX_HITS_TOTAL {
                break;
            }
            let (mut lines, matches) = scan(&matcher, document.text());
            lines.truncate(MAX_HITS_TOTAL - total);
            total += lines.len();
            if lines.is_empty() {
                continue;
            }
            let spans = GroupSpans {
                ranges: lines,
                marks: matches,
            };
            let prebuilt = himark::prebuild_group(document, &spans, self.width, &fonts, &theme);
            hits.push((*document_id, spans, prebuilt));
        }
        SearchCommand::Landed(SearchHits {
            serial: self.serial,
            hits,
        })
    }
}

impl Effect for SearchEffect {
    type Result = SearchCommand;
}

pub struct SearchHandler(pub std::sync::Arc<himark::Workshop>);

impl himark::EffectHandler<SearchEffect> for SearchHandler {
    async fn handle(&self, effect: SearchEffect) -> SearchCommand {
        effect.execute(&self.0)
    }
}

impl himark::EffectHandler<EmptyScan> for EmptyScanHandler {
    async fn handle(&self, effect: EmptyScan) -> SearchCommand {
        effect.execute()
    }
}

pub fn register_handlers(app: &mut himark::Application) {
    let workshop = std::sync::Arc::clone(app.workshop());
    app.register_handler::<SearchEffect>(SearchHandler(workshop));
    app.register_handler::<EmptyScan>(EmptyScanHandler);
}

pub struct EmptyScanHandler;

fn scan(matcher: &Matcher, text: &Text) -> (Vec<Range<u32>>, Vec<Range<u32>>) {
    let count = text.byte_count();
    let mut view = text.view();
    let mut lines: Vec<Range<u32>> = Vec::new();
    let mut matches: Vec<Range<u32>> = Vec::new();
    let mut start = 0usize;
    while start < count && lines.len() < MAX_HITS_PER_DOCUMENT {
        let mut end = (start + SCAN_WINDOW).min(count);
        while end < count {
            let probe_end = (end + 4096).min(count);
            let mut probe = Vec::new();
            view.byte_range_into(end, probe_end, &mut probe);
            if let Some(at) = probe.iter().position(|byte| *byte == b'\n') {
                end += at + 1;
                break;
            }
            end = probe_end;
        }
        let window = view.byte_string(start, end);
        for found in matcher.find_in(&window) {
            matches.push((start + found.start) as u32..(start + found.end) as u32);

            let mut line_start = window[..found.start].rfind('\n').map_or(0, |at| at + 1);
            for _ in 0..CONTEXT_LINES {
                match window[..line_start.saturating_sub(1)].rfind('\n') {
                    Some(at) => line_start = at + 1,
                    None => {
                        line_start = 0;
                        break;
                    }
                }
            }
            let mut line_end = window[found.end..]
                .find('\n')
                .map_or(window.len(), |at| found.end + at + 1);
            for _ in 0..CONTEXT_LINES {
                match window[line_end..].find('\n') {
                    Some(at) => line_end += at + 1,
                    None => {
                        line_end = window.len();
                        break;
                    }
                }
            }
            let absolute = (start + line_start) as u32..(start + line_end) as u32;

            match lines.last_mut() {
                Some(last) if absolute.start <= last.end => last.end = last.end.max(absolute.end),
                _ => lines.push(absolute),
            }
            if lines.len() >= MAX_HITS_PER_DOCUMENT {
                break;
            }
        }
        start = end.max(start + 1);
    }
    (lines, matches)
}

fn seeded_input(text: &str) -> EditorView {
    let mut markup = himark::Markup::new();
    markup.push_styled_covering(0..text.len() as u32, himark::theme::StyleId::Input);
    let document = himark::Document::new(himark::Text::from_string_exact(text), markup);
    let fonts = himark::fonts::source();
    let mut input =
        EditorView::of_document(document, 600.0, &fonts(), &himark::theme::Theme::embedded());
    input.set_caret(text.len() as u32);
    input.focus_text();
    input
}

pub fn overlay_surface() -> himark::OverlaySurface {
    himark::OverlaySurface {
        prefix: Some('%'),
        open: std::sync::Arc::new(|store, _ui, window, _fx| {
            let workspace = himark::Windows::window_ref(store, window)
                .expect("the window entity")
                .current_session();
            Box::new(SearchView::modal(Some(workspace), window))
        }),
    }
}

pub struct OpenSearch;

impl himark::DynamicCommand for OpenSearch {
    fn id(&self) -> &'static str {
        "search.open"
    }
    fn name(&self) -> String {
        "Open Search".to_owned()
    }
    fn perform(
        &self,
        app: &mut himark::Application,
        store: &mut Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        let ui = app.ui_handle();
        himark::toggle_toolbar_session(store, &ui, window, &overlay_surface(), "%", fx);
    }
}

impl ModalView for SearchView {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        if let Some(contents) = &mut self.contents {
            if let Some(request) = himark::ModalView::take_request(contents) {
                return Some(request);
            }
        }
        self.request.take()
    }

    fn set_query(
        &mut self,
        store: &mut Store,
        _ui: &UiCtx,
        query: &str,
        fx: &mut imba::effect::Effects<'_, imba::DynCommand>,
    ) {
        fx.scope(
            |command: SearchCommand| Box::new(command) as imba::DynCommand,
            |fx| self.requery(store, query.to_owned(), fx),
        )
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl PanelView for SearchView {
    type Place = himark::NoPlace;

    fn title(&self, _store: &Store) -> String {
        "Search".to_owned()
    }

    fn dismantle(&mut self, store: &mut Store) {
        self.uninstall(store);
    }

    fn family_row(&self) -> Option<himark::FamilyRow> {
        Some(himark::FamilyRow::List(self.results))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn drawer_view(
        &self,
        store: &Store,
        ui: &UiCtx,
        window: himark::WindowId,
    ) -> Option<Box<dyn himark::ModalView>> {
        if let Some(contents) = &self.contents {
            return Some(Box::new(contents.clone()));
        }
        let locations = self
            .list_ref(store)
            .map(|list| list.content().result_locations())
            .unwrap_or_default();
        himark::TocView::for_locations(store, ui, window, &locations)
            .map(|view| Box::new(view) as Box<dyn himark::ModalView>)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod landing_bench;

#[cfg(test)]
mod steady_scroll;

#[cfg(test)]
mod scroll_probe;

#[cfg(test)]
mod scan_boundaries;

#[cfg(test)]
mod utf8_field_repro;

struct MouseOnly<Inner>(Inner);

impl<'a, Inner: imba::Thunk<'a, SearchCommand> + 'a> imba::Thunk<'a, SearchCommand>
    for MouseOnly<Inner>
{
    fn size(&self) -> Size {
        self.0.size()
    }

    fn realize(
        self,
        arena: &'a Arena,
        viewport: skia_safe::Rect,
    ) -> imba::WidgetBox<'a, SearchCommand> {
        imba::WidgetBox::new(arena, MouseOnly(self.0.realize(arena, viewport)))
    }
}

impl<'a, Inner: imba::Widget<'a, SearchCommand>> imba::Widget<'a, SearchCommand>
    for MouseOnly<Inner>
{
    fn size(&self) -> Size {
        self.0.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, SearchCommand>> {
        self.0.overlays()
    }

    fn handle_event(
        &self,
        arena: &imba::arena::Arena,
        event: &Event<'_>,
        viewport: skia_safe::Rect,
    ) -> EventResult<SearchCommand> {
        match event {
            Event::MouseDrag { .. } | Event::MouseUp { .. } => EventResult::Ignored,
            _ => self.0.handle_event(arena, event, viewport),
        }
    }
}
