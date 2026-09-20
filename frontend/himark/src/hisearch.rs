// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Search dock tab (docs/ui/location-list.md §6): a query input
//! over the locations tree, streaming from an `ahp-locations:/…`
//! channel. The session's feed row is the substance — results
//! survive the surface; the live channel rides the view and dies
//! with it (closing the surface IS the cancel).

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
    locations_forest, resolve_batch, FoundLocation, LocationKey, LocationsFeedRow, LocationsFeeds,
};
use crate::modal::RequestSlot;
use crate::speedsearch::{SpeedSearchCommand, SpeedSearchView};
use crate::tree_item::{tree_interaction, TreeListCommand};
use crate::{EditorCommand, EditorView, LocationsChannel, ModalRequest, SessionId, WindowId};

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
    Focus(SearchArea),
    /// The stop affordance: cancel the running stream, keep what
    /// landed.
    Cancel,
    Dismiss,
    Nothing,
    Asked {
        generation: u64,
        outcome: Result<LocationsChannel, String>,
    },
    Snapshot {
        generation: u64,
        outcome: Result<himark_ahp_ext_types::LocationList, String>,
    },
    Polled {
        generation: u64,
        batches: Vec<himark_ahp_ext_types::LocationList>,
    },
}

pub struct SearchView {
    window: WindowId,
    session: SessionId,
    input: EditorView,
    search: SpeedSearchView<ForestList<LocationKey>, ForestSearcher<LocationKey>>,
    focus: SearchArea,
    last_query: String,

    /// The live stream — the view's own; dies with it.
    channel: Option<LocationsChannel>,
    ask_token: Option<imba::effect::CancellationToken>,
    poll_token: Option<imba::effect::CancellationToken>,

    /// Pick lookup: a hit key answers its own location; a file key
    /// its first occurrence.
    targets: rpds::HashTrieMapSync<LocationKey, FoundLocation>,
    files: usize,

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
            channel: self.channel.clone(),
            ask_token: self.ask_token,
            poll_token: self.poll_token,
            targets: self.targets.clone(),
            files: self.files,
            request: RequestSlot::default(),
        }
    }
}

impl SearchView {
    /// The face over the session's standing feed: reopening seeds
    /// the input with the last query and shows what stood.
    pub fn open(store: &Store, ui: &UiCtx, window: WindowId, session: SessionId) -> Self {
        let row = LocationsFeeds::row(store, &session).unwrap_or_default();
        let mut view = Self {
            window,
            session,
            input: seeded_input(&row.query),
            search: SpeedSearchView::new(
                ForestList::new(store),
                ForestSearcher::default(),
                crate::env::Fonts::of(store),
            ),
            focus: SearchArea::Input,
            last_query: row.query.clone(),
            channel: None,
            ask_token: None,
            poll_token: None,
            targets: rpds::HashTrieMapSync::new_sync(),
            files: 0,
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

    fn row(&self, store: &Store) -> LocationsFeedRow {
        LocationsFeeds::row(store, &self.session).unwrap_or_default()
    }

    /// Rebuild the tree and the pick table from the feed row —
    /// cursor and fold state survive by key.
    fn rebuild(&mut self, store: &Store, ui: &UiCtx) {
        let row = self.row(store);
        let locations: Vec<FoundLocation> = row.locations.iter().cloned().collect();
        let forest = locations_forest(store, &locations);

        let mut targets = rpds::HashTrieMapSync::new_sync();
        let mut files = 0usize;
        for found in &locations {
            let hit = LocationKey::Hit(found.location.clone(), found.line, found.column);
            targets.insert_mut(hit, found.clone());
            let file = LocationKey::Node(found.location.clone());
            if !targets.contains_key(&file) {
                files += 1;
                targets.insert_mut(file, found.clone());
            }
        }
        self.targets = targets;
        self.files = files;

        let cursor = self.search.inner().list().cursor().cloned();
        self.search.inner_mut().set(&forest, store, ui);
        if let Some(cursor) = cursor {
            if self.search.inner().forest.contains(&cursor) {
                self.search.inner_mut().list_mut().select_only(cursor);
            }
        }
    }

    /// Drop the live stream: unsubscribe (the host-side cancel) and
    /// forget the in-flight landings.
    fn drop_stream(&mut self, fx: &mut imba::effect::Effects<'_, SearchCommand>) {
        if let Some(token) = self.ask_token.take() {
            fx.cancel(token);
        }
        if let Some(token) = self.poll_token.take() {
            fx.cancel(token);
        }
        if let Some(channel) = self.channel.take() {
            let _ = fx.push(
                AnyEffect::new(crate::higent::UnsubscribeLocationsEffect {
                    seat: channel.seat,
                    channel: channel.channel,
                })
                .map(|()| SearchCommand::Nothing),
            );
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
        self.drop_stream(fx);

        let mut row = self.row(store);
        row.generation += 1;
        row.query = query.clone();
        row.title = format!("Search: {query}");
        row.locations = rpds::VectorSync::new_sync();
        row.truncated = false;
        let generation = row.generation;

        let folders = crate::higent::session_folders(store, &self.session);
        let launches = query.trim().len() >= MIN_QUERY && !folders.is_empty();
        row.done = !launches;
        LocationsFeeds::put(store, self.session.clone(), row);
        self.rebuild(store, ui);
        if !launches {
            return;
        }
        fx.relaunch_erased(
            &mut self.ask_token,
            AnyEffect::new(crate::SearchLocationsEffect {
                folders,
                query,
                regex: false,
                case_sensitive: false,
                limit: QUERY_LIMIT,
            })
            .map(move |outcome| SearchCommand::Asked {
                generation,
                outcome,
            }),
        );
    }

    /// Fold a landed wire batch into the feed row; answers whether
    /// the stream still runs.
    fn land(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        generation: u64,
        batches: Vec<himark_ahp_ext_types::LocationList>,
    ) -> bool {
        let mut row = self.row(store);
        if generation != row.generation {
            return false;
        }
        let Some(channel) = &self.channel else {
            return false;
        };
        for batch in batches {
            row.done |= batch.done;
            row.truncated |= batch.truncated;
            for found in resolve_batch(channel, batch) {
                row.locations.push_back_mut(found);
            }
        }
        let running = !row.done;
        LocationsFeeds::put(store, self.session.clone(), row);
        self.rebuild(store, ui);
        running
    }

    fn relaunch_poll(
        &mut self,
        generation: u64,
        fx: &mut imba::effect::Effects<'_, SearchCommand>,
    ) {
        let Some(channel) = &self.channel else {
            return;
        };
        fx.relaunch_erased(
            &mut self.poll_token,
            AnyEffect::new(crate::higent::PollLocationsEffect {
                seat: Arc::clone(&channel.seat),
                channel: channel.channel.clone(),
            })
            .map(move |batches| SearchCommand::Polled {
                generation,
                batches,
            }),
        );
    }

    /// Resolve the channel's story locally after an error or a stop:
    /// what landed stays, marked cut off.
    fn resolve_cut(&mut self, store: &mut Store, ui: &UiCtx, generation: u64) {
        let mut row = self.row(store);
        if generation != row.generation || row.done {
            return;
        }
        row.done = true;
        row.truncated = true;
        LocationsFeeds::put(store, self.session.clone(), row);
        self.rebuild(store, ui);
    }

    fn pick(&mut self, key: LocationKey) {
        let Some(found) = self.targets.get(&key).cloned() else {
            return;
        };
        let target = found.target();
        self.request.file(ModalRequest::Perform(crate::AppCommand::Dynamic(
            self.window,
            Arc::new(OpenFoundLocation {
                location: found.location,
                target,
            }),
        )));
    }
}

struct OpenFoundLocation {
    location: crate::ResourceLocation,
    target: std::ops::Range<crate::LineCol>,
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
        _store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let _ = fx.push(crate::open_by_location_effect(
            window,
            self.location.clone(),
            true,
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
                (_, InputKey::Escape) if !searching => {
                    EventResult::Command(SearchCommand::Dismiss)
                }
                (SearchArea::Input, InputKey::Down) | (SearchArea::Input, InputKey::Tab)
                    if rows > 0 =>
                {
                    EventResult::Command(SearchCommand::Focus(SearchArea::Results))
                }
                (SearchArea::Input, InputKey::Enter) if rows > 0 => {
                    EventResult::Command(SearchCommand::Focus(SearchArea::Results))
                }
                (SearchArea::Results, InputKey::Tab) => {
                    EventResult::Command(SearchCommand::Focus(SearchArea::Input))
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

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        self.drop_stream(fx);
        // The stream ends with the surface; what landed stays in the
        // feed row for the next open, honestly marked cut off.
        let mut row = self.row(store);
        if !row.done {
            row.done = true;
            row.truncated = true;
            LocationsFeeds::put(store, self.session.clone(), row);
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
                        self.focus = SearchArea::Results;
                        self.search.inner_mut().list_mut().select_only(key.clone());
                        let branch = matches!(&key, LocationKey::Node(location)
                            if location.kind().is_directory());
                        match toggle || branch {
                            true => self.search.inner_mut().toggle(&key, store, ui),
                            false => self.pick(key),
                        }
                        return;
                    }
                }
                fx.scope(SearchCommand::List, |fx| {
                    self.search.perform(store, ui, command, fx)
                });
            }
            SearchCommand::Select(delta) => {
                self.search.inner_mut().list_mut().cursor_step(delta)
            }
            SearchCommand::Fold(expand) => self.search.inner_mut().fold_cursor(expand, store, ui),
            SearchCommand::Pick => {
                if let Some(key) = self.search.inner().list().cursor().cloned() {
                    self.pick(key);
                }
            }
            SearchCommand::Focus(area) => self.focus = area,
            SearchCommand::Cancel => {
                let generation = self.row(store).generation;
                self.drop_stream(fx);
                self.resolve_cut(store, ui, generation);
            }
            SearchCommand::Dismiss => self.request.file(ModalRequest::Close),
            SearchCommand::Nothing => {}
            SearchCommand::Asked {
                generation,
                outcome,
            } => {
                if generation != self.row(store).generation {
                    return;
                }
                match outcome {
                    Ok(channel) => {
                        self.channel = Some(channel.clone());
                        fx.relaunch_erased(
                            &mut self.poll_token,
                            AnyEffect::new(crate::higent::SubscribeLocationsEffect {
                                seat: channel.seat,
                                channel: channel.channel,
                            })
                            .map(move |outcome| SearchCommand::Snapshot {
                                generation,
                                outcome,
                            }),
                        );
                    }
                    Err(_) => self.resolve_cut(store, ui, generation),
                }
            }
            SearchCommand::Snapshot {
                generation,
                outcome,
            } => match outcome {
                Ok(snapshot) => {
                    if self.land(store, ui, generation, vec![snapshot]) {
                        self.relaunch_poll(generation, fx);
                    }
                }
                Err(_) => self.resolve_cut(store, ui, generation),
            },
            SearchCommand::Polled {
                generation,
                batches,
            } => {
                if self.land(store, ui, generation, batches) {
                    self.relaunch_poll(generation, fx);
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
            let chrome = crate::env::Themes::of(store).ui().search.clone();
            let pad = chrome.pad;
            let input_height = chrome.input_height;
            let mut panel = imba::container::container(arena, size);

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

            // The status band: counts while streaming and after; a
            // click while running stops the stream.
            let row = self.row(store);
            let hits = row.locations.len();
            let status = match (row.done, row.truncated) {
                (false, _) => format!("{hits} results — searching… (click stops)"),
                (true, false) => match hits {
                    0 => "no results".to_owned(),
                    _ => format!("{hits} results in {} files", self.files),
                },
                (true, true) => format!("{hits} results (cut off)"),
            };
            let running = !row.done;
            let band = crate::ui::ListRow::new(arena, crate::ui::RowStyle::header(store, ui))
                .label(status)
                .on_event(move |_arena: &Arena, event: &Event<'_>, _size| match event {
                    Event::MouseDown { .. } if running => {
                        EventResult::Command(SearchCommand::Cancel)
                    }
                    _ => EventResult::Ignored,
                });
            let band = band.layout(
                arena,
                Constraints {
                    min: Size::new(size.width, 0.0),
                    max: Size::new(size.width, f32::MAX),
                },
            );
            let band_height = imba::Thunk::size(&band).height;
            let input_bottom = pad + input_height + pad;
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
            panel
        })
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
        self.input = seeded_input(query);
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

pub struct ToggleSearchView;

impl crate::DynamicCommand for ToggleSearchView {
    fn id(&self) -> &'static str {
        "search.view"
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

pub fn toolbar_button() -> crate::ToolbarButton {
    crate::ToolbarButton {
        command: "search.view",
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
                skia_safe::Point::new(rect.right - rect.width() * 0.08, rect.bottom - rect.height() * 0.08),
                &paint,
            );
        }),
    }
}

fn seeded_input(text: &str) -> EditorView {
    let mut markup = crate::Markup::new();
    markup.push_styled_covering(0..text.len() as u32, crate::theme::StyleId::Input);
    let document = crate::Document::new(crate::Text::from_string_exact(text), markup);
    let fonts = crate::fonts::source();
    let mut input =
        EditorView::of_document(document, 600.0, &fonts(), &crate::theme::Theme::embedded());
    input.set_caret(text.len() as u32);
    input.focus_text();
    input
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn feed(store: &mut Store, rows: &[FoundLocation], done: bool) {
        let mut row = LocationsFeeds::row(store, &session()).unwrap_or_default();
        row.generation += 1;
        row.locations = rows.iter().cloned().collect();
        row.done = done;
        LocationsFeeds::put(store, session(), row);
    }

    fn view(store: &mut Store, ui: &UiCtx) -> SearchView {
        let window = crate::WindowId::from_raw(77);
        SearchView::open(store, ui, window, session())
    }

    #[test]
    fn the_feed_row_renders_and_survives_rebuilds() {
        let mut store = Store::new();
        let ui = imba::UiCtx::cold();
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
        // survive the rebuild by key.
        let a = LocationKey::Node(found(&["work", "a.rs"], 0, 0, "").location);
        view.search.inner_mut().toggle(&a, &store, &ui);
        view.search.inner_mut().list_mut().select_only(a.clone());
        feed(
            &mut store,
            &[
                found(&["work", "a.rs"], 0, 0, "alpha"),
                found(&["work", "b.rs"], 2, 1, "beta"),
                found(&["work", "b.rs"], 4, 0, "gamma"),
            ],
            true,
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
        let ui = imba::UiCtx::cold();
        feed(&mut store, &[found(&["work", "a.rs"], 3, 2, "alpha")], true);
        let mut view = view(&mut store, &ui);

        let hit = LocationKey::Hit(found(&["work", "a.rs"], 3, 2, "").location, 3, 2);
        view.pick(hit);
        let request = crate::ModalView::take_request(&mut view);
        assert!(
            matches!(request, Some(ModalRequest::Perform(crate::AppCommand::Dynamic(_, _)))),
            "a hit pick performs the located open"
        );

        // A file pick answers its first occurrence.
        let file = LocationKey::Node(found(&["work", "a.rs"], 0, 0, "").location);
        view.pick(file);
        assert!(crate::ModalView::take_request(&mut view).is_some());

        // A directory key has no target: no request.
        let dir = LocationKey::Node(crate::ResourceLocation::new(
            crate::ResourceType::directory(),
            crate::Authority::new("local"),
            vec!["work".to_owned()],
        ));
        view.pick(dir);
        assert!(crate::ModalView::take_request(&mut view).is_none());
    }
}
