// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Search dock tab (docs/ui/location-list.md §6): a query input
//! over the locations tree. The tab is a FACE over a store-level
//! feed (`locations::LocationLists`, addressed by id): the feed and
//! its pump outlive the face, results keep landing while the dock is
//! closed, and a peek promotes its feed here without asking again.

use std::sync::Arc;

use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult, Key as InputKey},
    layout::Layout as _,
    layout::LayoutExt as _,
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    View,
};
use skia_safe::Size;

use crate::views::locations_forest;
use crate::{open_feed, FeedId, LocationKey, LocationLists, LocationsAsk, LocationsFeedRow};
use editor::{editor_view::EditorCommand, editor_view::EditorView};
use hikit::modal::ModalRequest;
use hikit::modal::RequestSlot;
use hikit::{forest::ForestList, forest::ForestSearcher};
use hikit::{list_keyboard::ListKeyCommand, list_keyboard::ListKeyboardController};
use hikit::{tree_item::tree_toggle, tree_item::TreeListCommand};
use imba::list::{ActivateTrigger, ListOps};

const MIN_QUERY: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchArea {
    Input,
    Results,
}

#[derive(Clone)]
pub enum SearchCommand {
    Input(EditorCommand),
    List(ListKeyCommand<TreeListCommand>),
    /// A click landed: move the keyboard to the clicked area, then
    /// forward the click itself.
    Focus(SearchArea, Option<Box<SearchCommand>>),
    /// The stop affordance: cancel the running stream, keep what
    /// landed.
    Cancel,
    Dismiss,
    /// The face noticed the feed moved (paint-driven).
    Refresh,
}

impl std::fmt::Display for SearchCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchCommand::Input(command) => command.fmt(out),
            SearchCommand::List(command) => command.fmt(out),
            SearchCommand::Focus(_, Some(command)) => command.fmt(out),
            SearchCommand::Focus(_, None) => out.write_str("search focus"),
            SearchCommand::Cancel => out.write_str("search cancel"),
            SearchCommand::Dismiss => out.write_str("search dismiss"),
            SearchCommand::Refresh => out.write_str("search refresh"),
        }
    }
}

pub struct SearchView {
    /// The session's lists collection — stamped at open from the
    /// window's session (docs/entities.md law 3): the face reads the
    /// model and NOTES asks; the wire lane does the rest.
    lists: imba::store::Id<LocationLists>,
    input: EditorView,
    search: ListKeyboardController<ForestList<LocationKey>, ForestSearcher<LocationKey>>,
    focus: SearchArea,
    last_query: String,

    /// Pick lookup: a hit key answers its INDEX into the feed (a
    /// file key its first occurrence) — indices, never copies; the
    /// feed row is the one holder of the contexts.
    targets: rpds::HashTrieMapSync<LocationKey, usize>,
    files: usize,
    shown: Option<(FeedId, u64)>,

    /// The last key SELECTION navigated to — moving the keyboard
    /// cursor over results opens them (the click door), and this is
    /// the dedup: feed rebuilds re-assert the cursor without
    /// re-opening, and standing still never re-navigates.
    navigated: Option<LocationKey>,

    request: RequestSlot<ModalRequest>,
}

impl Clone for SearchView {
    fn clone(&self) -> Self {
        Self {
            lists: self.lists,
            input: self.input.clone(),
            search: self.search.clone(),
            focus: self.focus,
            last_query: self.last_query.clone(),
            targets: self.targets.clone(),
            files: self.files,
            shown: self.shown,
            navigated: self.navigated.clone(),
            request: RequestSlot::default(),
        }
    }
}

impl SearchView {
    /// The face over the session's fronting feed: reopening seeds
    /// the input with the feed's query and shows what stands.
    pub fn open(store: &Store, ui: &UiCtx, lists: imba::store::Id<LocationLists>) -> Self {
        let row = LocationLists::search(store, lists)
            .and_then(|feed| LocationLists::row(store, lists, feed))
            .unwrap_or_default();
        let mut view = Self {
            lists,
            input: seeded_input(store, ui, &row.query),
            search: ListKeyboardController::searchable(
                ForestList::new(store),
                ForestSearcher::default(),
                store,
                ui,
                editor::env::Fonts::of(store),
            )
            .with_folds(),
            focus: SearchArea::Input,
            last_query: row.query.clone(),
            targets: rpds::HashTrieMapSync::new_sync(),
            files: 0,
            shown: None,
            navigated: None,
            request: RequestSlot::default(),
        };
        view.rebuild(store, ui);
        view
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

    fn feed(&self, store: &Store) -> Option<FeedId> {
        LocationLists::search(store, self.lists)
    }

    fn row(&self, store: &Store) -> LocationsFeedRow {
        self.feed(store)
            .and_then(|feed| LocationLists::row(store, self.lists, feed))
            .unwrap_or_default()
    }

    /// Rebuild the tree and the pick table from the feed — cursor
    /// and fold state survive by key. Location keys ride Arc'd
    /// paths (O(1) clones); the contexts are never copied here.
    fn rebuild(&mut self, store: &Store, ui: &UiCtx) {
        let feed = self.feed(store);
        let row = self.row(store);
        let forest = locations_forest(store, row.locations.iter());

        let mut targets = rpds::HashTrieMapSync::new_sync();
        let mut files = 0usize;
        for (index, found) in row.locations.iter().enumerate() {
            let hit = LocationKey::Hit(found.location.clone(), found.line, found.column);
            targets.insert_mut(hit, index);
            let file = LocationKey::Node(found.location.clone());
            if !targets.contains_key(&file) {
                files += 1;
                targets.insert_mut(file, index);
            }
        }
        self.targets = targets;
        self.files = files;
        self.shown = feed.map(|feed| (feed, row.generation));

        let cursor = self.search.inner().list().cursor().cloned();
        self.search.inner_mut().set(&forest, store, ui);
        if let Some(cursor) = cursor {
            if self.search.inner().forest.contains(&cursor) {
                self.search.inner_mut().list_mut().select_only(cursor);
            }
        }
    }

    fn requery(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        query: String,
        fx: &mut imba::effect::Effects<'_, SearchCommand>,
    ) {
        if query == self.last_query {
            return;
        }
        self.last_query = query.clone();

        let _ = fx;
        if let Some(previous) = self.feed(store) {
            LocationLists::ask(store, self.lists, LocationsAsk::Dispose(previous));
        }
        let feed = FeedId::mint();
        open_feed(
            store,
            self.lists,
            feed,
            format!("Search: {query}"),
            query.clone(),
        );
        LocationLists::set_search(store, self.lists, feed);
        // The ASK is a note; the wire lane launches it with the
        // model's own folders (an unlaunchable query resolves the
        // row cut-off there).
        LocationLists::ask(store, self.lists, LocationsAsk::Search { feed, query });
        self.rebuild(store, ui);
    }

    fn pick(&mut self, store: &mut Store, key: LocationKey, focus: bool) {
        let Some(found) = self
            .targets
            .get(&key)
            .copied()
            .and_then(|index| {
                self.feed(store)
                    .and_then(|feed| LocationLists::row_ref(store, self.lists, feed))
                    .and_then(|row| row.locations.get(index))
            })
            .cloned()
        else {
            return;
        };
        self.navigated = Some(key);
        let target = found.target();
        // The WASH rides the pick: an open document washes now; a
        // closed one notes, and the registration hook converts when
        // the open lands. The open itself is the shell's — the
        // request carries location, caret and the focus intent.
        if let Some(feed) = self.feed(store) {
            let documents = LocationLists::documents_of(store, self.lists);
            match documents.and_then(|docs| {
                documents::OpenDocuments::by_location(store, docs, &found.location)
            }) {
                Some(document) => imba::command::Requests::push(
                    store,
                    Arc::new(crate::views::WashDocument {
                        lists: self.lists,
                        feed,
                        document,
                    }),
                ),
                None => {
                    LocationLists::note_wash(store, self.lists, found.location.clone(), feed);
                }
            }
        }
        self.request.file(ModalRequest::OpenAt {
            location: found.location,
            target: Some(target),
            focus,
        });
    }

    /// Selection IS navigation: the keyboard cursor landing on a file
    /// or a hit opens it through the same door a click uses — the
    /// keyboard stays in the dock (nothing moves the layer focus).
    /// Directories only fold; standing still is deduped.
    fn navigate_selection(&mut self, store: &mut Store) {
        let Some(key) = self.search.inner().list().cursor().cloned() else {
            return;
        };
        if matches!(&key, LocationKey::Node(location) if location.kind().is_directory()) {
            return;
        }
        if self.navigated.as_ref() == Some(&key) {
            return;
        }
        self.pick(store, key, false);
    }
}

impl View for SearchView {
    type Command = SearchCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, SearchCommand> {
        use imba::focus::FocusData;
        let focus = self.focus;
        let searching = self.focus == SearchArea::Results && self.search.searching();
        let rows = self.search.inner().list().len();
        // Area moves are the surface's; the movement keys inside the
        // results area are the controller's table.
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match (focus, key) {
                (_, InputKey::Escape) if !searching => EventResult::Command(SearchCommand::Dismiss),
                (SearchArea::Input, InputKey::Down) | (SearchArea::Input, InputKey::Tab)
                    if rows > 0 =>
                {
                    EventResult::Command(SearchCommand::Focus(SearchArea::Results, None))
                }
                (SearchArea::Input, InputKey::Enter) if rows > 0 => {
                    EventResult::Command(SearchCommand::Focus(SearchArea::Results, None))
                }
                (SearchArea::Results, InputKey::Tab) => {
                    EventResult::Command(SearchCommand::Focus(SearchArea::Input, None))
                }
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        let area = match self.focus {
            SearchArea::Input => self.input.focus_data(store, ui).map(SearchCommand::Input),
            SearchArea::Results => self.search.focus_data(store, ui).map(SearchCommand::List),
        };
        own.merge_under(area)
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
                    View::perform(&mut self.input, store, ui, command, fx)
                });
                let query = self.query();
                self.requery(store, ui, query, fx);
            }
            SearchCommand::List(command) => {
                type Search =
                    ListKeyboardController<ForestList<LocationKey>, ForestSearcher<LocationKey>>;
                match &command {
                    ListKeyCommand::Fold { expand, .. } => {
                        self.search.inner_mut().fold_cursor(*expand, store, ui);
                        return self.navigate_selection(store);
                    }
                    ListKeyCommand::Inner(inner) => {
                        if let Some(index) = tree_toggle(inner) {
                            let Some(key) = self.search.inner().list().key_at(index).cloned()
                            else {
                                return;
                            };
                            self.search.inner_mut().list_mut().select_only(key.clone());
                            return self.search.inner_mut().toggle(&key, store, ui);
                        }
                    }
                    _ => {}
                }
                if let Some((index, trigger)) = Search::activated(&command) {
                    if let Some(key) = self.search.inner().list().key_at(index).cloned() {
                        let branch = matches!(&key, LocationKey::Node(location)
                            if location.kind().is_directory());
                        match (trigger, branch) {
                            (_, true) => self.search.inner_mut().toggle(&key, store, ui),
                            // Enter is the deliberate jump — the
                            // keyboard moves to the editor; a click
                            // browses, the keyboard stays here.
                            (ActivateTrigger::Enter, false) => self.pick(store, key, true),
                            (ActivateTrigger::Click, false) => self.pick(store, key, false),
                        }
                        return;
                    }
                }
                let selected = Search::selected_index(&command).is_some();
                fx.scope(SearchCommand::List, |fx| {
                    self.search.perform(store, ui, command, fx)
                });
                // Selection IS navigation (docs/ui/location-list.md):
                // any selection edit — keyboard, click or search step
                // — shows what the cursor stands on.
                if selected {
                    self.navigate_selection(store);
                }
            }
            SearchCommand::Focus(area, then) => {
                self.focus = area;
                match area {
                    SearchArea::Input => self.input.focus_text(),
                    SearchArea::Results => self.input.blur(),
                }
                match then {
                    Some(command) => self.perform(store, ui, *command, fx),
                    // A pure keyboard entry (Down/Enter from the
                    // input): the cursor's row is now the selection —
                    // open it like any other selection move.
                    None if area == SearchArea::Results => self.navigate_selection(store),
                    None => {}
                }
            }
            SearchCommand::Cancel => {
                if let Some(feed) = self.feed(store) {
                    LocationLists::ask(store, self.lists, LocationsAsk::Stop(feed));
                }
            }
            SearchCommand::Dismiss => self.request.file(ModalRequest::Close),
            SearchCommand::Refresh => self.rebuild(store, ui),
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
            let theme = editor::env::Themes::of(store);
            let ui_theme = theme.ui();
            // The CHAT COMPOSER's dress, the commit box's copy of it:
            // the bare input band, a hairline, and the toolbar row
            // with the accent cell flush right — the box reads as an
            // input even when the caret is elsewhere.
            let chrome = ui_theme.chat.clone();
            let pad = chrome.pad;
            let box_pad = pad * 0.75;
            let editor_h = chrome.title_size * 1.6;
            let toolbar_h = ui_theme.toolbar.height;
            let input_band = editor_h + box_pad * 2.0;
            let mut panel = imba::container::container(arena, size);

            let feed = self.feed(store);
            let row = feed.and_then(|feed| LocationLists::row_ref(store, self.lists, feed));
            let (hits, done, truncated, generation) = row
                .map(|row| (row.locations.len(), row.done, row.truncated, row.generation))
                .unwrap_or((0, true, false, 0));
            let running = feed.is_some() && !done;

            // Bottom-most: a press anywhere on the input band focuses
            // the box (the editor sits on top).
            panel.place(
                0.0,
                0.0,
                imba::leaf::leaf::<SearchCommand>(size.width, input_band).event(
                    |_arena, event, _size| match event {
                        Event::MouseDown {
                            button: imba::event::MouseButton::Left,
                            ..
                        } => EventResult::Command(SearchCommand::Focus(SearchArea::Input, None)),
                        _ => EventResult::Ignored,
                    },
                ),
            );
            let editor_w = (size.width - pad * 2.0).max(120.0);
            panel.place(
                pad,
                box_pad,
                imba::layout::Layout::layout(
                    self.input.display(arena, store, ui),
                    arena,
                    Constraints {
                        min: Size::new(editor_w, editor_h),
                        max: Size::new(editor_w, editor_h),
                    },
                )
                .map(SearchCommand::Input)
                .focus_scope(self.focus == SearchArea::Input),
            );

            // The toolbar row: ruled off above, the SEARCH cell flush
            // right — STOP while the stream runs, in the stop color.
            let rule = ui_theme.toolbar.rule.0;
            panel.place(
                0.0,
                input_band,
                imba::leaf::leaf::<SearchCommand>(size.width, 1.0).paint_instead(
                    move |_arena, canvas, rect| {
                        let mut paint = skia_safe::Paint::default();
                        paint.set_anti_alias(false);
                        paint.set_color(rule);
                        canvas.draw_rect(rect, &paint);
                    },
                ),
            );
            {
                let combo = ui_theme.combo.clone();
                let caps_font = hikit::fonts::ui_font(ui, combo.label_size * 1.1);
                let key_font = hikit::fonts::ui_text_font(ui, ui_theme.peeker.hint_size * 0.95);
                let label = if running { "STOP" } else { "SEARCH" };
                let cell_width = label
                    .chars()
                    .map(|ch| caps_font.measure_str(ch.to_string(), None).0 + 1.5)
                    .sum::<f32>()
                    + if running {
                        0.0
                    } else {
                        imba::layout::text_advance(ui, &key_font, "⏎") + combo.gap
                    }
                    + combo.pad * 2.0;
                let sendable = self.query().trim().len() >= MIN_QUERY;
                let accent = if running {
                    chrome.stop_color.0
                } else {
                    chrome.accent.0
                };
                let on_accent = chrome.on_accent.0;
                let accent_soft = ui_theme.peeker.dim_text.0;
                let cell_h = toolbar_h - 1.0;
                let mid = cell_h * 0.5;
                let caps_ascent = -caps_font.metrics().1.ascent;
                let key_ascent = -key_font.metrics().1.ascent;
                let mut cell = imba::layout::Row::new(arena).gap(combo.gap).child(
                    imba::layout::text(ui, label, caps_font.clone(), on_accent)
                        .tracking(1.5)
                        .pad_insets(imba::layout::Insets {
                            left: 0.0,
                            top: (mid + caps_font.size() * 0.35 - caps_ascent).max(0.0),
                            right: 0.0,
                            bottom: 0.0,
                        }),
                );
                if !running {
                    cell = cell.child(
                        imba::layout::text(ui, "⏎", key_font.clone(), accent_soft).pad_insets(
                            imba::layout::Insets {
                                left: 0.0,
                                top: (mid + key_font.size() * 0.35 - key_ascent).max(0.0),
                                right: 0.0,
                                bottom: 0.0,
                            },
                        ),
                    );
                }
                let cell = cell
                    .pad_insets(imba::layout::Insets {
                        left: combo.pad,
                        top: 0.0,
                        right: 0.0,
                        bottom: 0.0,
                    })
                    .sized(cell_width, cell_h)
                    .backdrop(
                        move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: skia_safe::Rect| {
                            let mut paint = skia_safe::Paint::default();
                            let mut fill = accent;
                            if !sendable && !running {
                                fill = fill.with_a(0x50);
                            }
                            paint.set_color(fill.with_a(fill.a() / 3));
                            canvas.draw_rect(rect, &paint);
                            paint.set_anti_alias(false);
                            paint.set_color(fill);
                            canvas.draw_rect(
                                skia_safe::Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                                &paint,
                            );
                        },
                    )
                    .on_click(move || match running {
                        true => SearchCommand::Cancel,
                        false => SearchCommand::Focus(SearchArea::Results, None),
                    });
                panel.place_boxed(
                    size.width - cell_width,
                    input_band + 1.0,
                    cell.layout(arena, Constraints::tight(Size::new(cell_width, cell_h))),
                );
            }

            // The status band: counts while streaming and after; a
            // click while running stops the stream. Per-frame reads
            // go through row_ref — no row clone per paint.
            let status = match (feed.is_some(), done, truncated) {
                (false, _, _) => "type to search the session".to_owned(),
                (true, false, _) => format!("{hits} results — searching… (click stops)"),
                (true, true, false) => match hits {
                    0 => "no results".to_owned(),
                    _ => format!("{hits} results in {} files", self.files),
                },
                (true, true, true) => format!("{hits} results (cut off)"),
            };
            let band = hikit::ui::ListRow::new(arena, hikit::ui::RowStyle::header(store, ui))
                .label(status)
                .on_event(
                    move |_arena: &Arena, event: &Event<'_>, _size| match event {
                        Event::MouseDown { .. } if running => {
                            EventResult::Command(SearchCommand::Cancel)
                        }
                        _ => EventResult::Ignored,
                    },
                );
            let band = band.layout(
                arena,
                Constraints {
                    min: Size::new(size.width, 0.0),
                    max: Size::new(size.width, f32::MAX),
                },
            );
            let band_height = imba::Thunk::size(&band).height;
            let input_bottom = input_band + toolbar_h;
            panel.place_boxed(0.0, input_bottom, band);

            let tree_top = input_bottom + band_height;
            panel.place(
                0.0,
                tree_top,
                imba::layout::Layout::layout(
                    self.search.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(size.width, (size.height - tree_top).max(1.0))),
                )
                .map(SearchCommand::List)
                .focus_scope(self.focus == SearchArea::Results),
            );
            // The refresh probe: a 1px leaf whose own paint files
            // Refresh when the feed moved — the PANEL keeps painting;
            // hijacking its Paint blanked a frame per landed batch
            // (the input caret blinked on every one).
            let stale = self.shown != feed.map(|feed| (feed, generation));
            if stale {
                panel.place(
                    0.0,
                    0.0,
                    imba::leaf::leaf::<SearchCommand>(1.0, 1.0).event(
                        move |_arena, event, _size| match event {
                            Event::Paint { .. } => EventResult::Command(SearchCommand::Refresh),
                            _ => EventResult::Ignored,
                        },
                    ),
                );
            }
            panel.wrap_realized(move |panel| SearchPanelWidget {
                panel,
                size,
                input_bottom,
            })
        })
    }
}

/// The face's widget shell: clicks move the keyboard to the clicked
/// area before landing (the input is focusable by click again), and
/// a paint over a moved feed refreshes the tree.
struct SearchPanelWidget<'a> {
    panel: imba::container::RealizedContainer<'a, SearchCommand>,
    size: Size,
    input_bottom: f32,
}

impl<'a> imba::Widget<'a, SearchCommand> for SearchPanelWidget<'a> {
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
        viewport: skia_safe::Rect,
    ) -> EventResult<SearchCommand> {
        match event {
            Event::MouseDown { point, .. } => {
                let area = match point.y < self.input_bottom {
                    true => SearchArea::Input,
                    false => SearchArea::Results,
                };
                // A row click on a selectable list is a BATCH (Select +
                // Activate) — fold the first into the Focus so it lands
                // after the area switch, and keep the rest.
                match self.panel.handle_event(arena, event, viewport) {
                    EventResult::Command(command) => {
                        EventResult::Command(SearchCommand::Focus(area, Some(Box::new(command))))
                    }
                    EventResult::Commands(commands) => {
                        let mut commands = commands.into_iter();
                        match commands.next() {
                            Some(first) => EventResult::Commands(
                                std::iter::once(SearchCommand::Focus(
                                    area,
                                    Some(Box::new(first)),
                                ))
                                .chain(commands)
                                .collect(),
                            ),
                            None => EventResult::Command(SearchCommand::Focus(area, None)),
                        }
                    }
                    _ => EventResult::Command(SearchCommand::Focus(area, None)),
                }
            }
            _ => self.panel.handle_event(arena, event, viewport),
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

impl hikit::modal::ModalView for SearchView {
    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn set_query(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        query: &str,
        fx: &mut imba::effect::Effects<'_, imba::dyn_view::DynCommand>,
    ) {
        self.input = seeded_input(store, ui, query);
        self.focus = SearchArea::Input;
        fx.scope(imba::dyn_view::DynCommand::new::<SearchCommand>, |fx| {
            self.requery(store, ui, query.to_owned(), fx)
        });
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn clone_modal(&self) -> Box<dyn hikit::modal::ModalView> {
        Box::new(self.clone())
    }
}

fn seeded_input(store: &imba::store::Store, ui: &imba::ui::UiCtx, text: &str) -> EditorView {
    let mut markup = editor::markup::Markup::new();
    markup.push_styled_covering(0..text.len() as u32, editor::theme::StyleId::Input);
    let document =
        editor::document::Document::new(text::text::Text::from_string_exact(text), markup);
    let fonts = hikit::fonts::source();
    let mut input = EditorView::of_document(
        document,
        600.0,
        store,
        ui,
        &fonts(),
        &editor::theme::Theme::embedded(),
    );
    input.set_caret(text.len() as u32);
    input.focus_text();
    input
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FoundLocation;

    fn found(path: &[&str], line: u32, column: u32, context: &str) -> FoundLocation {
        FoundLocation {
            location: editor::location::ResourceLocation::new(
                editor::location::ResourceType::document(),
                editor::location::Authority::new("local"),
                path.iter()
                    .map(|segment| segment.to_string())
                    .collect::<Vec<String>>(),
            ),
            line,
            column,
            length: 4,
            context: context.to_owned(),
            context_column_start: 0,
        }
    }

    /// One wired collection per store — the tests' stand-in for the
    /// session ceremony.
    fn lists(store: &mut Store) -> imba::store::Id<LocationLists> {
        let documents = imba::store::Id::mint();
        store.put_entity(documents, documents::OpenDocuments::default());
        let lists = imba::store::Id::mint();
        store.put_entity(lists, LocationLists::wired(documents));
        lists
    }

    fn feed(
        store: &mut Store,
        lists: imba::store::Id<LocationLists>,
        rows: &[FoundLocation],
        done: bool,
    ) -> FeedId {
        let feed = LocationLists::search(store, lists).unwrap_or_else(|| {
            let minted = FeedId::mint();
            LocationLists::set_search(store, lists, minted);
            minted
        });
        let mut row = LocationLists::row(store, lists, feed).unwrap_or_default();
        row.generation += 1;
        row.locations = rows.iter().cloned().collect();
        row.done = done;
        LocationLists::put(store, lists, feed, row);
        feed
    }

    fn view(store: &mut Store, ui: &UiCtx, lists: imba::store::Id<LocationLists>) -> SearchView {
        SearchView::open(store, ui, lists)
    }

    #[test]
    fn the_feed_renders_and_survives_rebuilds() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        let lists = lists(&mut store);
        feed(
            &mut store,
            lists,
            &[
                found(&["work", "a.rs"], 0, 0, "alpha"),
                found(&["work", "b.rs"], 2, 1, "beta"),
            ],
            false,
        );
        let mut view = view(&mut store, &ui, lists);
        let rows = view.search.inner().forest.rows();
        assert_eq!(
            rows.iter()
                .map(|(depth, label, _)| (*depth, label.as_str()))
                .collect::<Vec<_>>(),
            [
                (0, "work"),
                (1, "a.rs"),
                (2, "alpha"),
                (1, "b.rs"),
                (2, "beta"),
            ]
        );

        // Fold a file, land more results: the fold and the cursor
        // survive the rebuild by key — the Refresh the paint-driven
        // shell files when the feed's generation moves.
        let a = LocationKey::Node(found(&["work", "a.rs"], 0, 0, "").location);
        view.search.inner_mut().toggle(&a, &store, &ui);
        view.search.inner_mut().list_mut().select_only(a.clone());
        let shown = view.shown;
        feed(
            &mut store,
            lists,
            &[
                found(&["work", "a.rs"], 0, 0, "alpha"),
                found(&["work", "b.rs"], 2, 1, "beta"),
                found(&["work", "b.rs"], 4, 0, "gamma"),
            ],
            true,
        );
        assert_ne!(
            shown,
            view.feed(&store)
                .map(|feed| (feed, view.row(&store).generation)),
            "the shell would see the staleness"
        );
        view.rebuild(&store, &ui);
        assert!(view.search.inner().forest.is_collapsed(&a), "fold kept");
        assert_eq!(view.search.inner().list().cursor(), Some(&a), "cursor kept");
        assert_eq!(view.files, 2);

        // Zero fetches anywhere: nothing above registered a document.
        assert!(documents::OpenDocuments::list(&store, imba::store::Id::mint()).is_empty());
    }

    #[test]
    fn picking_routes_through_the_navigation_door() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        let lists = lists(&mut store);
        feed(
            &mut store,
            lists,
            &[found(&["work", "a.rs"], 3, 2, "alpha")],
            true,
        );
        let mut view = view(&mut store, &ui, lists);

        let hit = LocationKey::Hit(found(&["work", "a.rs"], 3, 2, "").location, 3, 2);
        view.pick(&mut store, hit, true);
        let request = hikit::modal::ModalView::take_request(&mut view);
        assert!(
            matches!(request, Some(ModalRequest::OpenAt { .. })),
            "a hit pick performs the located open"
        );

        // A file pick answers its first occurrence.
        let file = LocationKey::Node(found(&["work", "a.rs"], 0, 0, "").location);
        view.pick(&mut store, file, true);
        assert!(hikit::modal::ModalView::take_request(&mut view).is_some());

        // A directory key has no target: no request.
        let dir = LocationKey::Node(editor::location::ResourceLocation::new(
            editor::location::ResourceType::directory(),
            editor::location::Authority::new("local"),
            vec!["work".to_owned()],
        ));
        view.pick(&mut store, dir, true);
        assert!(hikit::modal::ModalView::take_request(&mut view).is_none());
    }

    /// Selection IS navigation: keyboard cursor moves open the row
    /// they land on; standing still (and feed rebuilds re-asserting
    /// the cursor) never re-open; directories only fold.
    #[test]
    fn moving_the_selection_navigates_and_dedups() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        let lists = lists(&mut store);
        feed(
            &mut store,
            lists,
            &[
                found(&["work", "a.rs"], 0, 0, "alpha"),
                found(&["work", "b.rs"], 2, 1, "beta"),
            ],
            true,
        );
        let mut view = view(&mut store, &ui, lists);
        let mut batch = imba::effect::Batch::new();

        // Entering the results lands the cursor on the first FILE row
        // (the list skips the branch): the landing already navigates.
        view.perform(
            &mut store,
            &ui,
            SearchCommand::Focus(SearchArea::Results, None),
            &mut batch.effects(),
        );
        assert!(
            hikit::modal::ModalView::take_request(&mut view).is_some(),
            "entering the results opens the row the cursor lands on"
        );

        // Down onto the hit under a.rs: a fresh key, a fresh open —
        // through the same Select command the key table emits.
        let step = view.search.step_index(1).expect("a next row");
        let select = SearchCommand::List(view.search.select_command(step));
        view.perform(&mut store, &ui, select, &mut batch.effects());
        assert!(
            hikit::modal::ModalView::take_request(&mut view).is_some(),
            "the selection move navigated"
        );

        // A rebuild re-asserts the cursor: no re-open.
        view.rebuild(&store, &ui);
        view.perform(
            &mut store,
            &ui,
            SearchCommand::Refresh,
            &mut batch.effects(),
        );
        assert!(
            hikit::modal::ModalView::take_request(&mut view).is_none(),
            "standing still never re-navigates"
        );

        // Down again onto b.rs: navigates too.
        let step = view.search.step_index(1).expect("a next row");
        let select = SearchCommand::List(view.search.select_command(step));
        view.perform(&mut store, &ui, select, &mut batch.effects());
        assert!(
            hikit::modal::ModalView::take_request(&mut view).is_some(),
            "the next row navigates too"
        );

        // Enter on the same row still opens (the deliberate, focusing
        // jump — dedup never swallows an explicit pick).
        let at = view.search.cursor_index().expect("a cursor row");
        let pick = SearchCommand::List(view.search.activate_command(at, ActivateTrigger::Enter));
        view.perform(&mut store, &ui, pick, &mut batch.effects());
        assert!(
            hikit::modal::ModalView::take_request(&mut view).is_some(),
            "an explicit pick always opens"
        );
    }
}
