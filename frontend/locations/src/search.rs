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

use crate::views::{locations_forest, result_row, ResultRow};
use crate::{open_feed, FeedId, LocationKey, LocationLists, LocationsAsk, LocationsFeedRow};
use editor::{editor_view::EditorCommand, editor_view::EditorView};
use hikit::modal::ModalRequest;
use hikit::modal::RequestSlot;
use hikit::{forest::ForestList, forest::ForestSearcher};
use hikit::{list_keyboard::ListKeyCommand, list_keyboard::ListKeyboardController};
use hikit::{tree_item::tree_toggle, tree_item::TreeListCommand};
use imba::list::{ActivateTrigger, ListOps};

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
    search: ListKeyboardController<
        ForestList<LocationKey, ResultRow>,
        ForestSearcher<LocationKey, ResultRow>,
    >,
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
                ForestList::wrapping(store, result_row(lists)),
                ForestSearcher::default(),
                store,
                ui,
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
                (_, InputKey::Escape) if !searching => EventResult::command(SearchCommand::Dismiss),
                (SearchArea::Input, InputKey::Down) | (SearchArea::Input, InputKey::Tab)
                    if rows > 0 =>
                {
                    EventResult::command(SearchCommand::Focus(SearchArea::Results, None))
                }
                (SearchArea::Input, InputKey::Enter) if rows > 0 => {
                    EventResult::command(SearchCommand::Focus(SearchArea::Results, None))
                }
                (SearchArea::Results, InputKey::Tab) => {
                    EventResult::command(SearchCommand::Focus(SearchArea::Input, None))
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
                type Search = ListKeyboardController<
                    ForestList<LocationKey, ResultRow>,
                    ForestSearcher<LocationKey, ResultRow>,
                >;
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
            let theme = editor::env::Themes::of(store);
            let find = theme.part(crate::chrome::FindChrome::KEY);
            let window = theme.part(::editor::theme::WindowChrome::KEY);
            let ink = theme.part(::editor::theme::InkChrome::KEY);

            let feed = self.feed(store);
            let row = feed.and_then(|feed| LocationLists::row_ref(store, self.lists, feed));
            let (hits, done, truncated, generation) = row
                .map(|row| (row.locations.len(), row.done, row.truncated, row.generation))
                .unwrap_or((0, true, false, 0));
            let running = feed.is_some() && !done;

            // The head, top to bottom (the Find artboard): the caps
            // "FIND IN …", the query over its hairline, the counts.
            let scope = LocationLists::folders(store, self.lists)
                .first()
                .map(|folder| format!("FIND IN {}", folder.name().to_uppercase()))
                .unwrap_or_else(|| "FIND".to_owned());
            let label = hikit::ui::text(&hikit::ui::caps(store, ui), scope);

            // The query: the input editor on its band, a hairline
            // under it — no box, no button; Enter takes the keyboard
            // to the results.
            let editor_h = self.input.content_height().max(1.0).min(find.query_band);
            let focused_input = self.focus == SearchArea::Input;
            let input = imba::layout::laid(move |arena: &'a Arena, constraints: Constraints| {
                imba::layout::Layout::layout(
                    self.input.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(constraints.max.width.max(1.0), editor_h)),
                )
                .map(SearchCommand::Input)
                .focus_scope(focused_input)
            })
            .align(imba::layout::Alignment::CenterStart)
            .height(find.query_band)
            .pad_insets(imba::layout::Insets {
                bottom: find.query_pad_bottom,
                ..Default::default()
            });
            let rule = find.query_rule.0;
            let query = imba::layout::laid(move |arena: &'a Arena, constraints: Constraints| {
                hikit::ui::Hairline::new(rule)
                    .bottom()
                    .wrap(input.layout(arena, constraints))
            });

            // The counts line: the numbers lead in primary ink, the
            // words follow in the secondary; while the stream runs a
            // press on it stops the stream.
            let words = crate::views::role_style(ui, &find.options, ink.ink_2.0);
            let counts = {
                let mut role = find.options;
                role.weight = find.count_weight;
                crate::views::role_style(ui, &role, ink.ink.0)
            };
            let spans: Vec<(bool, String)> = match (feed.is_some(), done, truncated) {
                (false, _, _) => vec![(false, "type to search the session".to_owned())],
                (true, false, _) => vec![
                    (true, hits.to_string()),
                    (false, " results — searching…".to_owned()),
                ],
                (true, true, _) if hits == 0 => vec![(false, "no results".to_owned())],
                (true, true, cut) => {
                    let mut spans = vec![
                        (true, hits.to_string()),
                        (false, " in ".to_owned()),
                        (true, self.files.to_string()),
                        (false, " files".to_owned()),
                    ];
                    if cut {
                        spans.push((false, " (cut off)".to_owned()));
                    }
                    spans
                }
            };
            let mut line = imba::layout::Row::new(arena);
            for (count, span) in spans {
                let style = match count {
                    true => &counts,
                    false => &words,
                };
                line = line.child_by_baseline(hikit::ui::text(style, span));
            }
            let line = line
                .weighted(1.0, imba::layout::Fill::new().height(0.0))
                .on_event(
                    move |_arena: &Arena, event: &Event<'_>, _size| match event {
                        Event::MouseDown { .. } if running => {
                            EventResult::command(SearchCommand::Cancel)
                        }
                        _ => EventResult::Ignored,
                    },
                );

            // The head's bands size to their own text: the column's
            // width, any height.
            let head = Constraints {
                min: Size::new(constraints.min.width, 0.0),
                max: Size::new(constraints.max.width, f32::INFINITY),
            };
            let edges = imba::layout::Insets {
                left: window.side_text_edge,
                top: 0.0,
                right: window.side_trailing,
                bottom: 0.0,
            };
            let field = imba::layout::Column::new(arena)
                .child(label)
                .child(query.pad_insets(imba::layout::Insets {
                    top: find.query_gap,
                    ..Default::default()
                }))
                .pad_insets(imba::layout::Insets {
                    top: find.head_top,
                    ..edges
                })
                .layout(arena, head);
            // Presses above the field's rule are the input's area.
            let input_bottom = imba::Thunk::size(&field).height;
            let band = line
                .pad_insets(imba::layout::Insets {
                    top: find.options_top,
                    ..edges
                })
                .layout(arena, head);
            let tree_top = input_bottom + imba::Thunk::size(&band).height + find.results_top;
            let tree = imba::layout::Layout::layout(
                self.search.display(arena, store, ui),
                arena,
                constraints.reserve(Size::new(0.0, tree_top)),
            )
            .map(SearchCommand::List)
            .focus_scope(self.focus == SearchArea::Results);
            // The panel's width is the one the TREE answered, never the
            // ceiling's — that ceiling can be open on either axis.
            let laid = imba::Thunk::size(&tree);
            let size = constraints.constrain(Size::new(laid.width, tree_top + laid.height));
            let mut panel = imba::container::container(arena, size);

            // Bottom-most: a press anywhere on the head focuses the
            // input (the editor sits on top).
            panel.place(
                0.0,
                0.0,
                imba::leaf::leaf::<SearchCommand>(size.width, input_bottom).event(
                    |_arena, event, _size| match event {
                        Event::MouseDown {
                            button: imba::event::MouseButton::Left,
                            ..
                        } => EventResult::command(SearchCommand::Focus(SearchArea::Input, None)),
                        _ => EventResult::Ignored,
                    },
                ),
            );
            panel.place_boxed(0.0, 0.0, field);
            panel.place_boxed(0.0, input_bottom, band);
            panel.place(0.0, tree_top, tree);
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
                            Event::Paint { .. } => EventResult::command(SearchCommand::Refresh),
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
                    EventResult::Handled(handled) if !handled.commands.is_empty() => {
                        let mut commands = handled.commands.into_iter();
                        let first = commands.next().expect("non-empty");
                        EventResult::commands(
                            std::iter::once(SearchCommand::Focus(area, Some(Box::new(first))))
                                .chain(commands)
                                .collect(),
                        )
                    }
                    _ => EventResult::command(SearchCommand::Focus(area, None)),
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
    markup.push_styled_covering(0..text.len() as u32, editor::theme::StyleId::QuerySmall);
    let document =
        editor::document::Document::new(text::text::Text::from_string_exact(text), markup);
    let mut input = EditorView::of_document(
        document,
        600.0,
        store,
        ui,
        &editor::theme::embedded(),
    );
    input.document.mark_input(input.editor, editor::theme::StyleId::QuerySmall);
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

    /// A MOUSE click on a results row opens it. The widget shell must
    /// pass the list's command BATCH through — a row click is Select +
    /// Activate (`EventResult::Commands`), not the single command a
    /// chevron returns; the shell once kept only single commands and
    /// every row click died in its `_` arm.
    #[test]
    fn a_row_click_opens_through_the_widget_shell() {
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
        let mut batch = imba::effect::Batch::new();

        // Settle the cursor first: entering the results already opens
        // the landing row — consume that so only the CLICK can open.
        view.perform(
            &mut store,
            &ui,
            SearchCommand::Focus(SearchArea::Results, None),
            &mut batch.effects(),
        );
        let _ = hikit::modal::ModalView::take_request(&mut view);

        let mut opened = false;
        let mut y = 4.0;
        while y < 396.0 && !opened {
            let commands = {
                let arena = Arena::default();
                let widget = imba::Thunk::realize(
                    imba::layout::Layout::layout(
                        view.display(&arena, &store, &ui),
                        &arena,
                        imba::constraints::Constraints::tight(Size::new(400.0, 400.0)),
                    ),
                    &arena,
                    skia_safe::Rect::from_wh(400.0, 400.0),
                );
                let event = Event::MouseDown {
                    point: skia_safe::Point::new(200.0, y),
                    button: imba::event::MouseButton::Left,
                    mods: imba::event::Modifiers::default(),
                    count: 1,
                };
                imba::Widget::handle_event(
                    &widget,
                    &arena,
                    &event,
                    skia_safe::Rect::from_wh(400.0, 400.0),
                )
                .into_commands()
            };
            for command in commands {
                view.perform(&mut store, &ui, command, &mut batch.effects());
            }
            if matches!(
                hikit::modal::ModalView::take_request(&mut view),
                Some(ModalRequest::OpenAt { .. })
            ) {
                opened = true;
            }
            y += 8.0;
        }
        assert!(opened, "a click on a results row opens its location");
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
