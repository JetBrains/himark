// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Go-to reference peek (docs/ui/location-list.md §8): an
//! `InlayMode::Under` master–detail card under the caret's line —
//! the quick random-access interface. The card is a FACE over a
//! store-level feed (`locations::LocationsFeeds`): the master is a
//! tree of locations grouped by file; the detail lazily fetches ONLY
//! the selected location's document and shows it whole in its own
//! scrollable editor. Enter navigates and dismisses; Escape
//! dismisses; the header's promote chip fronts the SAME feed in the
//! Search dock tab — no re-ask.

use std::sync::Arc;

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::AnyEffect,
    event::{Event, EventResult, Key as InputKey},
    store::Store,
    thunk_ext::ThunkExt,
    Layout as _, UiCtx, View,
};
use skia_safe::{Paint, Rect, Size};

use crate::forest::ForestList;
use crate::locations::{
    files_forest, open_feed, AttachFeedStream, DisposeFeed, FeedId, FoundLocation, LocationKey,
    LocationsFeeds,
};
use crate::tree_item::{tree_interaction, TreeListCommand};
use crate::{
    AppRequests, Document, EditorCommand, EditorFocus, EditorView, Inlay, InlayKey, InlayMode,
    LocationsChannel,
};

const PEEK_HEIGHT: f32 = 280.0;

const HEADER_HEIGHT: f32 = 24.0;

const FALLBACK_WIDTH: f32 = 600.0;

pub enum PeekCommand {
    Tree(TreeListCommand),
    Select(isize),
    Fold(bool),
    Pick,
    Close,
    /// The header chip: front this feed in the Search dock tab —
    /// the standing results move, nothing re-asks.
    Promote,
    Refresh,
    Preview(imba::scroll::ScrollCommand<EditorCommand>),
    Fetched {
        index: usize,
        text: Option<String>,
    },
    Built {
        index: usize,
        built: crate::BuiltDocument,
    },
}

#[derive(Clone)]
pub struct PeekView {
    /// The hosting document and the card's own key, baked in by the
    /// push → swap two-step so the card can remove itself.
    host: Option<crate::DocumentId>,
    key: Option<InlayKey>,
    width: f32,
    feed: FeedId,

    tree: ForestList<LocationKey>,
    /// A hit key answers (its index into the feed, the location); a
    /// file key its first occurrence.
    targets: rpds::HashTrieMapSync<LocationKey, (usize, FoundLocation)>,
    shown: u64,
    navigated: bool,

    preview: Option<imba::scroll::ScrollView<EditorView>>,
    preview_for: Option<usize>,
    fetch_token: Option<imba::effect::CancellationToken>,
}

impl PeekView {
    fn new(store: &Store, host: Option<crate::DocumentId>, width: f32, feed: FeedId) -> Self {
        Self {
            host,
            key: None,
            width,
            feed,
            tree: ForestList::new(store),
            targets: rpds::HashTrieMapSync::new_sync(),
            shown: 0,
            navigated: false,
            preview: None,
            preview_for: None,
            fetch_token: None,
        }
    }

    fn keyed(mut self, key: InlayKey) -> Self {
        self.key = Some(key);
        self
    }

    fn row(&self, store: &Store) -> crate::locations::LocationsFeedRow {
        LocationsFeeds::row(store, self.feed).unwrap_or_default()
    }

    /// Rebuild the file-grouped master from the feed; fold state and
    /// the cursor survive by key. The trivial case leaves no card:
    /// exactly one known result navigates directly.
    fn rebuild(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut imba::effect::Effects<'_, PeekCommand>,
    ) {
        let row = self.row(store);
        self.shown = row.generation;
        let locations: Vec<FoundLocation> = row.locations.iter().cloned().collect();

        if row.done && locations.len() == 1 && !self.navigated {
            self.navigated = true;
            self.navigate(store, &locations[0]);
            self.close(store, false);
            return;
        }

        let mut targets = rpds::HashTrieMapSync::new_sync();
        for (index, found) in locations.iter().enumerate() {
            targets.insert_mut(
                LocationKey::Hit(found.location.clone(), found.line, found.column),
                (index, found.clone()),
            );
            let file = LocationKey::Node(found.location.clone());
            if !targets.contains_key(&file) {
                targets.insert_mut(file, (index, found.clone()));
            }
        }
        self.targets = targets;

        let forest = files_forest(store, &locations);
        let cursor = self.tree.list().cursor().cloned();
        self.tree.set(&forest, store, ui);
        if let Some(cursor) = cursor {
            if self.tree.forest.contains(&cursor) {
                self.tree.list_mut().select_only(cursor);
            }
        }
        self.ensure_preview(fx);
    }

    /// Lazily fetch the SELECTED location's document — the one fetch
    /// this surface ever makes before navigation.
    fn ensure_preview(&mut self, fx: &mut imba::effect::Effects<'_, PeekCommand>) {
        let Some(key) = self.tree.list().cursor().cloned() else {
            return;
        };
        let Some((index, found)) = self.targets.get(&key).cloned() else {
            return;
        };
        if self.preview_for == Some(index) {
            return;
        }
        self.preview_for = Some(index);
        self.preview = None;
        let location = found.location;
        fx.relaunch_erased(
            &mut self.fetch_token,
            AnyEffect::new(crate::FetchDocumentEffect { location })
                .map(move |text| PeekCommand::Fetched { index, text }),
        );
    }

    /// The detail pane, VSCode/Fleet style: the WHOLE document in
    /// its own scrollable editor, scrolled to the match, the match
    /// washed `StyleId::Match`.
    fn install_preview(
        &mut self,
        store: &Store,
        built: crate::BuiltDocument,
        index: usize,
        fx: &mut imba::effect::Effects<'_, PeekCommand>,
    ) {
        let row = self.row(store);
        let Some(found) = row.locations.get(index).cloned() else {
            return;
        };
        let fonts = crate::env::Fonts::of(store)();
        let theme = crate::env::Themes::of(store);
        let mut document = built.document;

        let (hit, target) = {
            let mut view = document.text().view();
            let hit = crate::offset_at(&mut view, found.target().start) as u32;
            (hit, hit..hit + found.length.max(1))
        };

        let markup = document.add_markup();
        let mut tints = crate::Markup::new();
        tints.push_styled(target.clone(), crate::theme::StyleId::Match);
        let scoped = |command| PeekCommand::Preview(imba::scroll::ScrollCommand::Content(command));
        fx.scope(scoped, |fx| {
            document.replace_markup(markup, tints, &[target], &fonts, &theme, fx)
        });

        let detail = (self.width * 0.6 - 1.0).max(120.0);
        let editor = fx.scope(scoped, |fx| {
            document.add_editor(
                detail,
                None,
                ::editor::EditorBuild::Complete,
                &[],
                &fonts,
                &theme,
                fx,
            )
        });
        document.show_markup(editor, markup);
        let reveal = document
            .caret_content_rect(editor, hit, &fonts, &theme)
            .map(|(_, y, _, _)| (y - PEEK_HEIGHT / 3.0).max(0.0))
            .unwrap_or(0.0);
        let mut view = EditorView {
            document,
            editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        };
        view.set_caret(hit);
        view.blur();
        let mut preview = imba::scroll::ScrollView::new(view);
        preview.set_scroll_y(reveal);
        self.preview = Some(preview);
    }

    fn navigate(&mut self, store: &mut Store, found: &FoundLocation) {
        AppRequests::push(
            store,
            Arc::new(crate::hisearch::OpenFoundLocation {
                location: found.location.clone(),
                target: found.target(),
            }),
        );
    }

    /// Dismiss the card. The feed dies with it unless it was handed
    /// on (the promote path fronts it in the dock instead).
    fn close(&mut self, store: &mut Store, keep_feed: bool) {
        if !keep_feed {
            AppRequests::push(store, Arc::new(DisposeFeed { feed: self.feed }));
        }
        if let (Some(host), Some(key)) = (self.host, self.key) {
            AppRequests::push(store, Arc::new(RemovePeek { document: host, key }));
        }
    }
}

impl View for PeekView {
    type Command = PeekCommand;

    fn focus_data<'w>(
        &'w self,
        _store: &'w Store,
        _ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, PeekCommand> {
        use imba::focus::FocusData;
        let rows = self.tree.list().len();
        FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Up => EventResult::Command(PeekCommand::Select(-1)),
                InputKey::Down => EventResult::Command(PeekCommand::Select(1)),
                InputKey::Left => EventResult::Command(PeekCommand::Fold(false)),
                InputKey::Right => EventResult::Command(PeekCommand::Fold(true)),
                InputKey::Enter if rows > 0 => EventResult::Command(PeekCommand::Pick),
                InputKey::Escape => EventResult::Command(PeekCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        }
    }

    fn destroy(&mut self, _store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        if let Some(token) = self.fetch_token.take() {
            fx.cancel(token);
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
            PeekCommand::Tree(command) => {
                if let Some((index, toggle)) = tree_interaction(&command) {
                    let Some(key) = self.tree.list().key_at(index).cloned() else {
                        return;
                    };
                    self.tree.list_mut().select_only(key.clone());
                    let file = matches!(&key, LocationKey::Node(_));
                    match toggle || file {
                        true => self.tree.toggle(&key, store, ui),
                        false => {
                            if let Some((_, found)) = self.targets.get(&key).cloned() {
                                self.navigate(store, &found);
                                self.close(store, false);
                                return;
                            }
                        }
                    }
                    self.ensure_preview(fx);
                    return;
                }
                fx.scope(PeekCommand::Tree, |fx| {
                    self.tree.perform(store, ui, command, fx)
                });
            }
            PeekCommand::Select(delta) => {
                self.tree.list_mut().cursor_step(delta);
                self.ensure_preview(fx);
            }
            PeekCommand::Fold(expand) => self.tree.fold_cursor(expand, store, ui),
            PeekCommand::Pick => {
                let Some(key) = self.tree.list().cursor().cloned() else {
                    return;
                };
                match &key {
                    LocationKey::Node(_) => self.tree.toggle(&key, store, ui),
                    LocationKey::Hit(..) => {
                        if let Some((_, found)) = self.targets.get(&key).cloned() {
                            self.navigate(store, &found);
                            self.close(store, false);
                        }
                    }
                }
            }
            PeekCommand::Close => self.close(store, false),
            PeekCommand::Promote => {
                AppRequests::push(
                    store,
                    Arc::new(crate::hisearch::ShowFeedInDock { feed: self.feed }),
                );
                self.close(store, true);
            }
            PeekCommand::Refresh => self.rebuild(store, ui, fx),
            PeekCommand::Preview(command) => {
                if let Some(preview) = &mut self.preview {
                    fx.scope(PeekCommand::Preview, |fx| {
                        View::perform(preview, store, ui, command, fx)
                    });
                }
            }
            PeekCommand::Fetched { index, text } => {
                if self.preview_for != Some(index) {
                    return;
                }
                let Some(text) = text else {
                    return;
                };
                let row = self.row(store);
                let Some(found) = row.locations.get(index) else {
                    return;
                };
                let location = found.location.clone();
                fx.relaunch_erased(
                    &mut self.fetch_token,
                    AnyEffect::new(crate::BuildDocumentEffect { location, text })
                        .map(move |built| PeekCommand::Built { index, built }),
                );
            }
            PeekCommand::Built { index, built } => {
                if self.preview_for != Some(index) {
                    return;
                }
                self.install_preview(store, built, index, fx);
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
            let width = constraints.max.width.min(self.width).max(120.0);
            let size = Size::new(width, PEEK_HEIGHT);
            let chrome = crate::env::Themes::of(store).ui().peeker.clone();
            let fill = chrome.background.0;
            let rule = chrome.rule.0;
            let master = (width * 0.4).max(120.0);

            let mut card = imba::container::container(arena, size);
            let backdrop = imba::leaf::leaf::<PeekCommand>(width, PEEK_HEIGHT).paint_instead(
                move |_arena, canvas, rect| {
                    let mut paint = Paint::default();
                    paint.set_color(fill);
                    canvas.draw_rect(rect, &paint);
                    paint.set_color(rule);
                    for y in [rect.top, rect.top + HEADER_HEIGHT, rect.bottom - 1.0] {
                        canvas.draw_rect(Rect::from_xywh(rect.left, y, rect.width(), 1.0), &paint);
                    }
                    canvas.draw_rect(
                        Rect::from_xywh(
                            rect.left + master,
                            rect.top + HEADER_HEIGHT,
                            1.0,
                            rect.height() - HEADER_HEIGHT,
                        ),
                        &paint,
                    );
                },
            );
            card.place(0.0, 0.0, backdrop);

            // The header: the feed's title and stream state, plus the
            // promote chip fronting the SAME feed in the Search tab.
            let row = self.row(store);
            let hits = row.locations.len();
            let state = match (row.done, row.truncated) {
                (false, _) => format!("{hits} — searching…"),
                (true, true) => format!("{hits} (cut off)"),
                (true, false) => format!("{hits}"),
            };
            let header = crate::ui::ListRow::new(arena, crate::ui::RowStyle::header(store, ui))
                .label(format!("{} · {state}", row.title))
                .action("OPEN IN SEARCH", || PeekCommand::Promote)
                .layout(
                    arena,
                    Constraints {
                        min: Size::new(width, HEADER_HEIGHT),
                        max: Size::new(width, HEADER_HEIGHT),
                    },
                );
            card.place_boxed(0.0, 0.0, header);

            card.place(
                0.0,
                HEADER_HEIGHT + 1.0,
                imba::Layout::layout(
                    self.tree.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(master, PEEK_HEIGHT - HEADER_HEIGHT - 2.0)),
                )
                .map(PeekCommand::Tree),
            );
            if let Some(preview) = &self.preview {
                card.place(
                    master + 1.0,
                    HEADER_HEIGHT + 1.0,
                    imba::Layout::layout(
                        preview.display(arena, store, ui),
                        arena,
                        Constraints {
                            min: Size::default(),
                            max: Size::new(
                                (width - master - 1.0).max(1.0),
                                PEEK_HEIGHT - HEADER_HEIGHT - 2.0,
                            ),
                        },
                    )
                    .map(PeekCommand::Preview),
                );
            }
            let stale = self.shown != row.generation;
            card.wrap_realized(move |card| PeekWidget { card, size, stale })
        })
    }
}

/// The card's widget shell: a paint over a moved feed refreshes the
/// master (the pump is app-level; the face notices on the frame the
/// landing scheduled).
struct PeekWidget<'a> {
    card: imba::container::RealizedContainer<'a, PeekCommand>,
    size: Size,
    stale: bool,
}

impl<'a> imba::Widget<'a, PeekCommand> for PeekWidget<'a> {
    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, PeekCommand>> {
        self.card.overlays()
    }

    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: skia_safe::Rect,
    ) -> EventResult<PeekCommand> {
        match event {
            Event::Paint { .. } if self.stale => EventResult::Command(PeekCommand::Refresh),
            _ => self.card.handle_event(arena, event, viewport),
        }
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, PeekCommand>
    where
        'a: 'w,
    {
        self.card.layout_data(target)
    }
}

struct RemovePeek {
    document: crate::DocumentId,
    key: InlayKey,
}

impl crate::DynamicCommand for RemovePeek {
    fn id(&self) -> &'static str {
        "peek.remove"
    }

    fn name(&self) -> String {
        "Close Reference Peek".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        _window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(mut doc) = crate::OpenDocuments::document(store, self.document) else {
            return;
        };
        let fonts = crate::env::Fonts::of(store)();
        let theme = crate::env::Themes::of(store);
        let document = self.document;
        fx.scope(
            move |command| crate::AppCommand::Entity(document, command),
            |fx| doc.remove_inlay(self.key, &fonts, &theme, fx),
        );
        crate::OpenDocuments::put_document(store, self.document, doc);
    }
}

fn peek_markup() -> crate::MarkupId {
    static ID: std::sync::OnceLock<crate::MarkupId> = std::sync::OnceLock::new();
    *ID.get_or_init(crate::MarkupId::mint)
}

pub struct GoToReference;

impl crate::DynamicEditorCommand for GoToReference {
    fn id(&self) -> &'static str {
        "code.go-to-reference"
    }

    fn name(&self) -> String {
        "Go to Reference".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        document: &mut Document,
        editor: crate::EditorId,
        location: &crate::ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, EditorCommand>,
    ) {
        // Phase two: the channel landed — attach it to the feed the
        // card already fronts.
        if let Some(payload) = payload {
            let Ok(landed) = payload.downcast::<(FeedId, Result<LocationsChannel, String>)>()
            else {
                return;
            };
            let (feed, outcome) = *landed;
            AppRequests::push(store, Arc::new(AttachFeedStream { feed, outcome }));
            return;
        }

        // Phase one: mint the feed, mount the card NOW — the ask's
        // outcome lands into the visible card, never into silence.
        let caret = document.caret_byte(editor);
        let Some(anchor) = caret_anchor(document, caret) else {
            return;
        };
        let position = {
            let mut view = document.text().view();
            crate::line_col_at(&mut view, caret as usize)
        };
        let feed = FeedId::mint();
        open_feed(store, feed, "References".to_owned(), String::new());

        let fonts = crate::env::Fonts::of(store)();
        let theme = crate::env::Themes::of(store);
        let width = match document.layout_width(editor) {
            width if width > 1.0 => width,
            _ => FALLBACK_WIDTH,
        };
        let host = crate::OpenDocuments::by_location(store, location);
        let view = PeekView::new(store, host, width, feed);
        let markup = peek_markup();
        document.ensure_document_markup(markup);
        let key = document.push_inlay(
            markup,
            anchor.clone(),
            Inlay::new(InlayMode::Under, view.clone()),
            &fonts,
            &theme,
            fx,
        );
        document.swap_inlay(key, anchor, Inlay::new(InlayMode::Under, view.keyed(key)));
        document.set_focus(editor, EditorFocus::Inlay(key));

        let _ = fx.push(
            AnyEffect::new(crate::LspLocationsEffect {
                location: location.clone(),
                position,
                kind: crate::LspLocationsKind::References,
            })
            .map(move |outcome| EditorCommand::Dynamic {
                id: "code.go-to-reference",
                payload: Some(Box::new((feed, outcome))),
            }),
        );
    }
}

/// The caret's character as a non-empty anchor span, char-boundary
/// snapped; the character before it at the text's end; `None` on an
/// empty document. An EMPTY anchor renders nothing — an Under
/// inlay's line anchoring needs a real span.
fn caret_anchor(document: &Document, caret: u32) -> Option<std::ops::Range<u32>> {
    let len = document.text().byte_count().min(u32::MAX as usize) as u32;
    if len == 0 {
        return None;
    }
    let mut view = document.text().view();
    if caret < len {
        let head = view.substring(caret..(caret + 4).min(len));
        let step = head.chars().next().map(|ch| ch.len_utf8() as u32).unwrap_or(1);
        Some(caret..(caret + step).min(len))
    } else {
        let tail = view.substring(len.saturating_sub(4)..len);
        let step = tail.chars().last().map(|ch| ch.len_utf8() as u32).unwrap_or(1);
        Some(len.saturating_sub(step)..len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(name: &str, line: u32, context: &str) -> FoundLocation {
        FoundLocation {
            location: crate::ResourceLocation::new(
                crate::ResourceType::document(),
                crate::Authority::new("local"),
                vec!["work".to_owned(), name.to_owned()],
            ),
            line,
            column: 2,
            length: 3,
            context: context.to_owned(),
            context_column_start: 0,
        }
    }

    fn feed(store: &mut Store, rows: &[FoundLocation], done: bool) -> FeedId {
        let feed = FeedId::mint();
        let mut row = crate::locations::LocationsFeedRow {
            title: "References".to_owned(),
            generation: 1,
            done,
            ..Default::default()
        };
        row.locations = rows.iter().cloned().collect();
        LocationsFeeds::put(store, feed, row);
        feed
    }

    #[test]
    fn the_master_groups_by_file_and_navigates_on_pick() {
        let mut store = Store::new();
        let ui = imba::UiCtx::cold();
        let feed = feed(
            &mut store,
            &[
                found("a.rs", 1, "  let x = y;"),
                found("a.rs", 7, "x + 1"),
                found("b.rs", 0, "fn b()"),
            ],
            true,
        );
        let mut view = PeekView::new(&store, None, 600.0, feed);
        let mut batch = imba::effect::Batch::new();
        view.rebuild(&mut store, &ui, &mut batch.effects());

        let rows = view.tree.forest.rows();
        assert_eq!(
            rows.iter()
                .map(|(depth, label, _)| (*depth, label.as_str()))
                .collect::<Vec<_>>(),
            [
                (0, "a.rs"),
                (1, "let x = y;"),
                (1, "x + 1"),
                (0, "b.rs"),
                (1, "fn b()"),
            ],
            "files as groups, occurrences as leaves, contexts trimmed"
        );

        let hit = LocationKey::Hit(found("b.rs", 0, "").location, 0, 2);
        view.tree.list_mut().select_only(hit);
        view.perform(&mut store, &ui, PeekCommand::Pick, &mut batch.effects());
        let requests = store.get::<crate::AppRequests>().expect("requests");
        assert!(!requests.is_empty(), "the pick navigated through the door");
        assert!(
            LocationsFeeds::row(&store, feed).is_some(),
            "disposal rides AppRequests, not the view"
        );
    }

    #[test]
    fn a_single_settled_result_navigates_without_a_card() {
        let mut store = Store::new();
        let ui = imba::UiCtx::cold();
        let feed = feed(&mut store, &[found("a.rs", 3, "only")], true);
        let mut view = PeekView::new(&store, None, 600.0, feed);
        let mut batch = imba::effect::Batch::new();
        view.rebuild(&mut store, &ui, &mut batch.effects());
        assert!(view.navigated, "the trivial case went straight through");
        let requests = store.get::<crate::AppRequests>().expect("requests");
        assert!(!requests.is_empty());
    }

    #[test]
    fn the_peek_anchor_reserves_height() {
        let mut document = ::editor::test_document::plain_document("fn a() {}\nfn b() {}\n");
        let fonts = ::editor::embedded_fonts::source()();
        let theme = crate::theme::Theme::embedded();
        let mut batch = imba::effect::Batch::new();
        let editor = document.add_editor(
            600.0,
            None,
            ::editor::EditorBuild::Complete,
            &[],
            &fonts,
            &theme,
            &mut batch.effects(),
        );
        let bare = document.content_height(editor);

        let markup = crate::MarkupId::mint();
        document.ensure_document_markup(markup);
        // The regression: an empty anchor renders NOTHING (an Under
        // inlay anchors on a line by its span) — the card must mount
        // on the caret_anchor span, never caret..caret.
        for caret in [3u32, document.text().byte_count() as u32] {
            let anchor = caret_anchor(&document, caret).expect("non-empty text anchors");
            assert!(anchor.start < anchor.end, "a real span: {anchor:?}");
            let key = document.push_inlay(
                markup,
                anchor,
                crate::Inlay::new(crate::InlayMode::Under, ProbeCard),
                &fonts,
                &theme,
                &mut batch.effects(),
            );
            let with_card = document.content_height(editor);
            assert!(
                with_card > bare,
                "the anchored card reserves height: {with_card} vs {bare}"
            );
            document.remove_inlay(key, &fonts, &theme, &mut batch.effects());
        }
    }

    #[derive(Clone)]
    struct ProbeCard;

    impl imba::View for ProbeCard {
        type Command = ();
        fn perform(
            &mut self,
            _store: &mut imba::store::Store,
            _ui: &imba::UiCtx,
            _command: Self::Command,
            _fx: &mut imba::effect::Effects<'_, Self::Command>,
        ) {
        }
        fn display<'a>(
            &'a self,
            _arena: &'a imba::arena::Arena,
            _store: &'a imba::store::Store,
            _ui: &'a imba::UiCtx,
        ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
            imba::laid(move |arena: &'a imba::arena::Arena, _constraints| {
                imba::ThunkBox::new(arena, imba::leaf::leaf::<()>(200.0, 111.0))
            })
        }
    }
}
