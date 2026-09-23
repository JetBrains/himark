// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Search dock tab (docs/ui/location-list.md §6): a query input
//! over the locations tree. The tab is a FACE over a store-level
//! feed (`locations::LocationsFeeds`, addressed by id): the feed and
//! its pump outlive the face, results keep landing while the dock is
//! closed, and a peek promotes its feed here without asking again.

use std::sync::Arc;

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::AnyEffect,
    event::{Event, EventResult, Key as InputKey},
    store::Store,
    thunk_ext::ThunkExt,
    Layout as _, LayoutExt as _, UiCtx, View,
};
use skia_safe::Size;

use crate::forest::{ForestList, ForestSearcher};
use crate::locations::{
    locations_forest, open_feed, AttachFeedStream, DisposeFeed, FeedId, LocationKey,
    LocationsFeedRow, LocationsFeeds, SessionSearchFeeds, StopFeed,
};
use crate::modal::RequestSlot;
use crate::speedsearch::{SpeedSearchCommand, SpeedSearchView};
use crate::tree_item::{tree_interaction, TreeListCommand};
use crate::{AppRequests, EditorCommand, EditorView, ModalRequest, SessionId, WindowId};

/// The dock owner id — the toggle command's, shared by everything
/// that lands content into this tab.
pub const OWNER: &str = "search.view";

const MIN_QUERY: usize = 2;

/// The stream's total-location cap per query; the host caps harder.
const QUERY_LIMIT: usize = 2048;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchArea {
    Input,
    Results,
}

pub enum SearchCommand {
    Input(EditorCommand),
    List(SpeedSearchCommand<TreeListCommand>),
    Select(isize),
    Fold(bool),
    Pick,
    /// A click landed: move the keyboard to the clicked area, then
    /// forward the click itself.
    Focus(SearchArea, Option<Box<SearchCommand>>),
    /// The stop affordance: cancel the running stream, keep what
    /// landed.
    Cancel,
    Dismiss,
    /// The face noticed the feed moved (paint-driven).
    Refresh,
    Asked {
        feed: FeedId,
        outcome: Result<crate::LocationsChannel, String>,
    },
}

pub struct SearchView {
    window: WindowId,
    session: SessionId,
    input: EditorView,
    search: SpeedSearchView<ForestList<LocationKey>, ForestSearcher<LocationKey>>,
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
            window: self.window,
            session: self.session.clone(),
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
    pub fn open(store: &Store, ui: &UiCtx, window: WindowId, session: SessionId) -> Self {
        let row = SessionSearchFeeds::feed(store, &session)
            .and_then(|feed| LocationsFeeds::row(store, feed))
            .unwrap_or_default();
        let mut view = Self {
            window,
            session,
            input: seeded_input(store, ui, &row.query),
            search: SpeedSearchView::new(
                ForestList::new(store),
                ForestSearcher::default(),
                store,
                ui,
                crate::env::Fonts::of(store),
            ),
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
        SessionSearchFeeds::feed(store, &self.session)
    }

    fn row(&self, store: &Store) -> LocationsFeedRow {
        self.feed(store)
            .and_then(|feed| LocationsFeeds::row(store, feed))
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

        if let Some(previous) = self.feed(store) {
            AppRequests::push(store, Arc::new(DisposeFeed { feed: previous }));
        }
        let feed = FeedId::mint();
        open_feed(store, feed, format!("Search: {query}"), query.clone());
        SessionSearchFeeds::put(store, self.session.clone(), feed);

        let folders = crate::higent::session_folders(store, &self.session);
        let launches = query.trim().len() >= MIN_QUERY && !folders.is_empty();
        if !launches {
            let mut row = LocationsFeeds::row(store, feed).unwrap_or_default();
            row.done = true;
            LocationsFeeds::put(store, feed, row);
            self.rebuild(store, ui);
            return;
        }
        self.rebuild(store, ui);
        let _ = fx.push(
            AnyEffect::new(crate::SearchLocationsEffect {
                folders,
                query,
                regex: false,
                case_sensitive: false,
                limit: QUERY_LIMIT,
            })
            .map(move |outcome| SearchCommand::Asked { feed, outcome }),
        );
    }

    fn pick(&mut self, store: &Store, key: LocationKey, focus: bool) {
        let Some(found) = self
            .targets
            .get(&key)
            .copied()
            .and_then(|index| {
                self.feed(store)
                    .and_then(|feed| LocationsFeeds::row_ref(store, feed))
                    .and_then(|row| row.locations.get(index))
            })
            .cloned()
        else {
            return;
        };
        self.navigated = Some(key);
        let target = found.target();
        self.request
            .file(ModalRequest::Perform(crate::AppCommand::Dynamic(
                self.window,
                Arc::new(OpenFoundLocation {
                    location: found.location,
                    target,
                    feed: self.feed(store),
                    focus,
                }),
            )));
    }

    /// Selection IS navigation: the keyboard cursor landing on a file
    /// or a hit opens it through the same door a click uses — the
    /// keyboard stays in the dock (nothing moves the layer focus).
    /// Directories only fold; standing still is deduped.
    fn navigate_selection(&mut self, store: &Store) {
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

pub(crate) struct OpenFoundLocation {
    pub(crate) location: crate::ResourceLocation,
    pub(crate) target: std::ops::Range<crate::LineCol>,
    /// Set for search-view picks: the opened editor gets the feed's
    /// find-results wash — every occurrence in the file highlighted.
    pub(crate) feed: Option<FeedId>,
    /// A deliberate jump (click, Enter) moves the keyboard to the
    /// editor; a selection move browsing results just shows it.
    pub(crate) focus: bool,
}

impl crate::DynamicCommand for OpenFoundLocation {
    fn id(&self) -> &'static str {
        "search.open-location"
    }

    fn name(&self) -> String {
        "Open Search Result".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        if let Some(feed) = self.feed {
            match crate::OpenDocuments::by_location(store, &self.location) {
                Some(document) => crate::AppRequests::push(
                    store,
                    Arc::new(crate::locations::WashDocument { feed, document }),
                ),
                None => crate::locations::PendingWashes::note(store, self.location.clone(), feed),
            }
        }
        let _ = fx.push(crate::open_by_location_effect(
            window,
            self.location.clone(),
            true,
            self.focus,
            Some(self.target.clone()),
        ));
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
                (SearchArea::Results, InputKey::Up) if !searching => {
                    EventResult::Command(SearchCommand::Select(-1))
                }
                (SearchArea::Results, InputKey::Down) if !searching => {
                    EventResult::Command(SearchCommand::Select(1))
                }
                (SearchArea::Results, InputKey::Left) if !searching => {
                    EventResult::Command(SearchCommand::Fold(false))
                }
                (SearchArea::Results, InputKey::Right) if !searching => {
                    EventResult::Command(SearchCommand::Fold(true))
                }
                (SearchArea::Results, InputKey::Enter) if rows > 0 => {
                    EventResult::Command(SearchCommand::Pick)
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
                if let SpeedSearchCommand::Inner(inner) = &command {
                    if let Some((index, toggle)) = tree_interaction(inner) {
                        let Some(key) = self.search.inner().list().key_at(index).cloned() else {
                            return;
                        };
                        self.search.inner_mut().list_mut().select_only(key.clone());
                        let branch = matches!(&key, LocationKey::Node(location)
                            if location.kind().is_directory());
                        match toggle || branch {
                            true => self.search.inner_mut().toggle(&key, store, ui),
                            // A click browses too — the keyboard stays
                            // with the tree; only Enter is the jump.
                            false => self.pick(store, key, false),
                        }
                        return;
                    }
                }
                let before = self.search.inner().list().cursor().cloned();
                fx.scope(SearchCommand::List, |fx| {
                    self.search.perform(store, ui, command, fx)
                });
                if self.search.inner().list().cursor().cloned() != before {
                    self.navigate_selection(store);
                }
            }
            SearchCommand::Select(delta) => {
                self.search.inner_mut().list_mut().cursor_step(delta);
                self.navigate_selection(store);
            }
            SearchCommand::Fold(expand) => {
                self.search.inner_mut().fold_cursor(expand, store, ui);
                self.navigate_selection(store);
            }
            SearchCommand::Pick => {
                if let Some(key) = self.search.inner().list().cursor().cloned() {
                    self.pick(store, key, true);
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
                    AppRequests::push(store, Arc::new(StopFeed { feed }));
                }
            }
            SearchCommand::Dismiss => self.request.file(ModalRequest::Close),
            SearchCommand::Refresh => self.rebuild(store, ui),
            SearchCommand::Asked { feed, outcome } => {
                AppRequests::push(store, Arc::new(AttachFeedStream { feed, outcome }));
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
            let theme = crate::env::Themes::of(store);
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
            let row = feed.and_then(|feed| LocationsFeeds::row_ref(store, feed));
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
                imba::Layout::layout(
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
                let caps_font = crate::fonts::ui_font(ui, combo.label_size * 1.1);
                let key_font = crate::fonts::ui_text_font(ui, ui_theme.peeker.hint_size * 0.95);
                let label = if running { "STOP" } else { "SEARCH" };
                let cell_width = label
                    .chars()
                    .map(|ch| caps_font.measure_str(ch.to_string(), None).0 + 1.5)
                    .sum::<f32>()
                    + if running {
                        0.0
                    } else {
                        imba::text_advance(ui, &key_font, "⏎") + combo.gap
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
                let mut cell = imba::Row::new(arena).gap(combo.gap).child(
                    imba::text(ui, label, caps_font.clone(), on_accent)
                        .tracking(1.5)
                        .pad_insets(imba::Insets {
                            left: 0.0,
                            top: (mid + caps_font.size() * 0.35 - caps_ascent).max(0.0),
                            right: 0.0,
                            bottom: 0.0,
                        }),
                );
                if !running {
                    cell = cell.child(
                        imba::text(ui, "⏎", key_font.clone(), accent_soft).pad_insets(
                            imba::Insets {
                                left: 0.0,
                                top: (mid + key_font.size() * 0.35 - key_ascent).max(0.0),
                                right: 0.0,
                                bottom: 0.0,
                            },
                        ),
                    );
                }
                let cell = cell
                    .pad_insets(imba::Insets {
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
            let band = crate::ui::ListRow::new(arena, crate::ui::RowStyle::header(store, ui))
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
                imba::Layout::layout(
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
                match self.panel.handle_event(arena, event, viewport) {
                    EventResult::Command(command) => {
                        EventResult::Command(SearchCommand::Focus(area, Some(Box::new(command))))
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

impl crate::ModalView for SearchView {
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
        self.input = seeded_input(store, ui, query);
        self.focus = SearchArea::Input;
        fx.scope(
            |command: SearchCommand| Box::new(command) as imba::DynCommand,
            |fx| self.requery(store, ui, query.to_owned(), fx),
        );
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn clone_modal(&self) -> Box<dyn crate::ModalView> {
        Box::new(self.clone())
    }
}

/// Front a feed in the Search dock tab — the one door every producer
/// uses: a references ask opening the tab at ASK time, and the
/// peek's promote button, which reuses the standing feed instead of
/// asking again.
pub struct ShowFeedInDock {
    pub feed: FeedId,
}

impl crate::DynamicCommand for ShowFeedInDock {
    fn id(&self) -> &'static str {
        "search.show-feed"
    }

    fn name(&self) -> String {
        "Show Locations in Search".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };
        let session = entity.current_session();
        if let Some(previous) = SessionSearchFeeds::feed(store, &session) {
            if previous != self.feed {
                AppRequests::push(store, Arc::new(DisposeFeed { feed: previous }));
            }
        }
        SessionSearchFeeds::put(store, session.clone(), self.feed);

        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let panel = SearchView::open(store, &app.ui_ctx(), window, session);
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.show_dock(store, Box::new(panel), OWNER, fx),
        );
        crate::Windows::put(store, window, entity);
    }
}

pub struct ToggleSearchView;

impl crate::DynamicCommand for ToggleSearchView {
    fn id(&self) -> &'static str {
        OWNER
    }

    fn name(&self) -> String {
        "Search".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            crate::Windows::put(store, window, entity);
            return;
        }

        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );

        let session = entity.current_session();
        let panel = SearchView::open(store, &app.ui_ctx(), window, session);
        let owner = self.id();
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.show_dock(store, Box::new(panel), owner, fx),
        );
        crate::Windows::put(store, window, entity);
    }
}

/// The KEYBINDING's road (cmd-shift-f, `search.focus`): open the
/// dock if it is away, and FOCUS the query input if it already
/// fronts — never a toggle-away; the toolbar button (`search.view`,
/// [`ToggleSearchView`]) keeps the toggle every dock button has.
pub struct FocusSearchView;

impl crate::DynamicCommand for FocusSearchView {
    fn id(&self) -> &'static str {
        "search.focus"
    }

    fn name(&self) -> String {
        "Focus Search".to_owned()
    }

    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(OWNER) {
            entity.focus_dock();
            if let Some(panel) = entity.dock_panel_mut() {
                let ui = app.ui_ctx();
                fx.scope(
                    move |command| {
                        crate::AppCommand::Content(
                            window,
                            crate::WindowCommand::Dock(Box::new(
                                crate::dock::DockCommand::Content(command),
                            )),
                        )
                    },
                    |fx| {
                        imba::DynView::perform_dyn(
                            panel.as_mut(),
                            store,
                            &ui,
                            Box::new(SearchCommand::Focus(SearchArea::Input, None)),
                            fx,
                        )
                    },
                );
            }
            crate::Windows::put(store, window, entity);
            return;
        }
        crate::Windows::put(store, window, entity);
        ToggleSearchView.perform(app, store, window, fx);
    }
}

pub fn toolbar_button() -> crate::ToolbarButton {
    crate::ToolbarButton {
        command: OWNER,
        order: 0.5,
        side: crate::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let radius = rect.width() * 0.32;
            let center = skia_safe::Point::new(
                rect.left + rect.width() * 0.42,
                rect.top + rect.height() * 0.42,
            );
            canvas.draw_circle(center, radius, &paint);
            let start = skia_safe::Point::new(
                center.x + radius * std::f32::consts::FRAC_1_SQRT_2,
                center.y + radius * std::f32::consts::FRAC_1_SQRT_2,
            );
            canvas.draw_line(
                start,
                skia_safe::Point::new(
                    rect.right - rect.width() * 0.08,
                    rect.bottom - rect.height() * 0.08,
                ),
                &paint,
            );
        }),
    }
}

fn seeded_input(store: &imba::store::Store, ui: &imba::UiCtx, text: &str) -> EditorView {
    let mut markup = crate::Markup::new();
    markup.push_styled_covering(0..text.len() as u32, crate::theme::StyleId::Input);
    let document = crate::Document::new(crate::Text::from_string_exact(text), markup);
    let fonts = crate::fonts::source();
    let mut input = EditorView::of_document(
        document,
        600.0,
        store,
        ui,
        &fonts(),
        &crate::theme::Theme::embedded(),
    );
    input.set_caret(text.len() as u32);
    input.focus_text();
    input
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locations::FoundLocation;

    fn found(path: &[&str], line: u32, column: u32, context: &str) -> FoundLocation {
        FoundLocation {
            location: crate::ResourceLocation::new(
                crate::ResourceType::document(),
                crate::Authority::new("local"),
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

    fn session() -> SessionId {
        SessionId {
            host: crate::higent::HostId::LOCAL,
            session: "test".to_owned(),
        }
    }

    fn feed(store: &mut Store, rows: &[FoundLocation], done: bool) -> FeedId {
        let feed = SessionSearchFeeds::feed(store, &session()).unwrap_or_else(|| {
            let minted = FeedId::mint();
            SessionSearchFeeds::put(store, session(), minted);
            minted
        });
        let mut row = LocationsFeeds::row(store, feed).unwrap_or_default();
        row.generation += 1;
        row.locations = rows.iter().cloned().collect();
        row.done = done;
        LocationsFeeds::put(store, feed, row);
        feed
    }

    fn view(store: &mut Store, ui: &UiCtx) -> SearchView {
        let window = crate::WindowId::from_raw(77);
        SearchView::open(store, ui, window, session())
    }

    #[test]
    fn the_feed_renders_and_survives_rebuilds() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        feed(
            &mut store,
            &[
                found(&["work", "a.rs"], 0, 0, "alpha"),
                found(&["work", "b.rs"], 2, 1, "beta"),
            ],
            false,
        );
        let mut view = view(&mut store, &ui);
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
        assert!(crate::OpenDocuments::list(&store).is_empty());
    }

    #[test]
    fn picking_routes_through_the_navigation_door() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        feed(&mut store, &[found(&["work", "a.rs"], 3, 2, "alpha")], true);
        let mut view = view(&mut store, &ui);

        let hit = LocationKey::Hit(found(&["work", "a.rs"], 3, 2, "").location, 3, 2);
        view.pick(&store, hit, true);
        let request = crate::ModalView::take_request(&mut view);
        assert!(
            matches!(
                request,
                Some(ModalRequest::Perform(crate::AppCommand::Dynamic(_, _)))
            ),
            "a hit pick performs the located open"
        );

        // A file pick answers its first occurrence.
        let file = LocationKey::Node(found(&["work", "a.rs"], 0, 0, "").location);
        view.pick(&store, file, true);
        assert!(crate::ModalView::take_request(&mut view).is_some());

        // A directory key has no target: no request.
        let dir = LocationKey::Node(crate::ResourceLocation::new(
            crate::ResourceType::directory(),
            crate::Authority::new("local"),
            vec!["work".to_owned()],
        ));
        view.pick(&store, dir, true);
        assert!(crate::ModalView::take_request(&mut view).is_none());
    }

    /// Selection IS navigation: keyboard cursor moves open the row
    /// they land on; standing still (and feed rebuilds re-asserting
    /// the cursor) never re-open; directories only fold.
    #[test]
    fn moving_the_selection_navigates_and_dedups() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        feed(
            &mut store,
            &[
                found(&["work", "a.rs"], 0, 0, "alpha"),
                found(&["work", "b.rs"], 2, 1, "beta"),
            ],
            true,
        );
        let mut view = view(&mut store, &ui);
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
            crate::ModalView::take_request(&mut view).is_some(),
            "entering the results opens the row the cursor lands on"
        );

        // Down onto the hit under a.rs: a fresh key, a fresh open.
        view.perform(
            &mut store,
            &ui,
            SearchCommand::Select(1),
            &mut batch.effects(),
        );
        assert!(
            crate::ModalView::take_request(&mut view).is_some(),
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
            crate::ModalView::take_request(&mut view).is_none(),
            "standing still never re-navigates"
        );

        // Down again onto b.rs: navigates too.
        view.perform(
            &mut store,
            &ui,
            SearchCommand::Select(1),
            &mut batch.effects(),
        );
        assert!(
            crate::ModalView::take_request(&mut view).is_some(),
            "the next row navigates too"
        );

        // Enter on the same row still opens (the deliberate, focusing
        // jump — dedup never swallows an explicit pick).
        view.perform(&mut store, &ui, SearchCommand::Pick, &mut batch.effects());
        assert!(
            crate::ModalView::take_request(&mut view).is_some(),
            "an explicit pick always opens"
        );
    }
}
