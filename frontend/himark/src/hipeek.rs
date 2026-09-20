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
    /// A hit key answers its INDEX into the feed (a file key its
    /// first occurrence) — indices, never copies of the locations:
    /// the feed row is the one holder of the contexts.
    targets: rpds::HashTrieMapSync<LocationKey, usize>,
    shown: u64,
    navigated: bool,

    /// The detail, keyed by FILE: moving between hits of one file
    /// re-scrolls and re-tints the standing editor — no refetch, no
    /// relayout.
    preview: Option<imba::scroll::ScrollView<EditorView>>,
    preview_for: Option<crate::ResourceLocation>,
    preview_markup: Option<crate::MarkupId>,
    preview_hit: Option<(u32, u32)>,
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
            preview_markup: None,
            preview_hit: None,
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

    fn found(&self, store: &Store, key: &LocationKey) -> Option<FoundLocation> {
        let index = self.targets.get(key).copied()?;
        LocationsFeeds::row_ref(store, self.feed)
            .and_then(|row| row.locations.get(index))
            .cloned()
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

        if row.done && row.locations.len() == 1 && !self.navigated {
            self.navigated = true;
            if let Some(found) = row.locations.get(0).cloned() {
                self.navigate(store, &found);
            }
            self.close(store, false);
            return;
        }

        // Indices only — location keys ride Arc'd paths, O(1)
        // clones; the contexts stay in the feed row.
        let mut targets = rpds::HashTrieMapSync::new_sync();
        for (index, found) in row.locations.iter().enumerate() {
            targets.insert_mut(
                LocationKey::Hit(found.location.clone(), found.line, found.column),
                index,
            );
            let file = LocationKey::Node(found.location.clone());
            if !targets.contains_key(&file) {
                targets.insert_mut(file, index);
            }
        }
        self.targets = targets;

        let forest = files_forest(store, row.locations.iter());
        let cursor = self.tree.list().cursor().cloned();
        self.tree.set(&forest, store, ui);
        if let Some(cursor) = cursor {
            if self.tree.forest.contains(&cursor) {
                self.tree.list_mut().select_only(cursor);
            }
        }
        self.ensure_preview(store, fx);
    }

    /// Ensure the detail shows the SELECTED location. Same file:
    /// re-scroll and re-tint the standing editor — O(log n). New
    /// file: the one fetch this surface makes before navigation;
    /// the build runs off-thread and the mount is BUDGETED (the
    /// mount_editor discipline), never a whole-document layout on
    /// the UI thread.
    fn ensure_preview(&mut self, store: &Store, fx: &mut imba::effect::Effects<'_, PeekCommand>) {
        let Some(key) = self.tree.list().cursor().cloned() else {
            return;
        };
        let Some(index) = self.targets.get(&key).copied() else {
            return;
        };
        let Some(found) = LocationsFeeds::row_ref(store, self.feed)
            .and_then(|row| row.locations.get(index))
            .cloned()
        else {
            return;
        };
        if self.preview_for.as_ref() == Some(&found.location) {
            self.retarget_preview(store, &found, fx);
            return;
        }
        self.preview_for = Some(found.location.clone());
        self.preview = None;
        self.preview_markup = None;
        self.preview_hit = None;
        let location = found.location;
        fx.relaunch_erased(
            &mut self.fetch_token,
            AnyEffect::new(crate::FetchDocumentEffect { location })
                .map(move |text| PeekCommand::Fetched { index, text }),
        );
    }

    /// Move the standing preview onto another hit of the SAME file:
    /// swap the one-range wash (old ∪ new as the change set) and
    /// re-scroll. No fetch, no layout.
    fn retarget_preview(
        &mut self,
        store: &Store,
        found: &FoundLocation,
        fx: &mut imba::effect::Effects<'_, PeekCommand>,
    ) {
        let Some(preview) = &mut self.preview else {
            return;
        };
        let Some(markup) = self.preview_markup else {
            return;
        };
        let fonts = crate::env::Fonts::of(store)();
        let theme = crate::env::Themes::of(store);
        let view = preview.content_mut();
        let (hit, target) = {
            let mut text = view.document.text().view();
            let hit = crate::offset_at(&mut text, found.target().start) as u32;
            (hit, (hit, hit + found.length.max(1)))
        };
        if self.preview_hit == Some(target) {
            return;
        }
        let mut changed: Vec<std::ops::Range<u32>> = vec![target.0..target.1];
        if let Some((start, end)) = self.preview_hit.replace(target) {
            changed.push(start..end);
        }
        let mut tints = crate::Markup::new();
        tints.push_styled(target.0..target.1, crate::theme::StyleId::Match);
        let scoped = |command| PeekCommand::Preview(imba::scroll::ScrollCommand::Content(command));
        fx.scope(scoped, |fx| {
            view.document
                .replace_markup(markup, tints, &changed, &fonts, &theme, fx)
        });
        view.set_caret(hit);
        let editor = view.editor;
        fx.scope(scoped, |fx| {
            view.document
                .reveal_at_instant(editor, hit, &fonts, &theme, fx)
        });
        let reveal = view
            .document
            .caret_content_rect(editor, hit, &fonts, &theme)
            .map(|(_, y, _, _)| (y - PEEK_HEIGHT / 3.0).max(0.0));
        if let Some(reveal) = reveal {
            preview.set_scroll_y(reveal);
        }
    }

    /// The detail pane, VSCode/Fleet style: the WHOLE document in
    /// its own scrollable editor, scrolled to the match, the match
    /// washed `StyleId::Match`. The mount is BUDGETED (Bounded build
    /// + reveal, the mount_editor discipline) — the tail repairs in
    /// the background through the Preview scope; the UI thread never
    /// lays a whole document.
    fn install_preview(
        &mut self,
        store: &Store,
        built: crate::BuiltDocument,
        index: usize,
        fx: &mut imba::effect::Effects<'_, PeekCommand>,
    ) {
        let Some(found) = LocationsFeeds::row_ref(store, self.feed)
            .and_then(|row| row.locations.get(index))
            .cloned()
        else {
            return;
        };
        if self.preview_for.as_ref() != Some(&found.location) {
            return; // the cursor moved on while the build ran
        }
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
            document.replace_markup(markup, tints, &[target.clone()], &fonts, &theme, fx)
        });

        let detail = (self.width * 0.6 - 1.0).max(120.0);
        let editor = fx.scope(scoped, |fx| {
            document.add_editor(
                detail,
                None,
                ::editor::EditorBuild::Bounded,
                &[],
                &fonts,
                &theme,
                fx,
            )
        });
        document.show_markup(editor, markup);
        fx.scope(scoped, |fx| {
            document.reveal_at_instant(editor, hit, &fonts, &theme, fx)
        });
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
        self.preview_markup = Some(markup);
        self.preview_hit = Some((target.start, target.end));
    }

    fn navigate(&mut self, store: &mut Store, found: &FoundLocation) {
        AppRequests::push(
            store,
            Arc::new(crate::hisearch::OpenFoundLocation {
                location: found.location.clone(),
                target: found.target(),
                feed: None,
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
                            if let Some(found) = self.found(store, &key) {
                                self.navigate(store, &found);
                                self.close(store, false);
                                return;
                            }
                        }
                    }
                    self.ensure_preview(store, fx);
                    return;
                }
                fx.scope(PeekCommand::Tree, |fx| {
                    self.tree.perform(store, ui, command, fx)
                });
            }
            PeekCommand::Select(delta) => {
                self.tree.list_mut().cursor_step(delta);
                self.ensure_preview(store, fx);
            }
            PeekCommand::Fold(expand) => self.tree.fold_cursor(expand, store, ui),
            PeekCommand::Pick => {
                let Some(key) = self.tree.list().cursor().cloned() else {
                    return;
                };
                match &key {
                    LocationKey::Node(_) => self.tree.toggle(&key, store, ui),
                    LocationKey::Hit(..) => {
                        if let Some(found) = self.found(store, &key) {
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
                let Some(text) = text else {
                    return;
                };
                let Some(location) = LocationsFeeds::row_ref(store, self.feed)
                    .and_then(|row| row.locations.get(index))
                    .map(|found| found.location.clone())
                else {
                    return;
                };
                if self.preview_for.as_ref() != Some(&location) {
                    return; // the cursor moved on while the fetch ran
                }
                fx.relaunch_erased(
                    &mut self.fetch_token,
                    AnyEffect::new(crate::BuildDocumentEffect { location, text })
                        .map(move |built| PeekCommand::Built { index, built }),
                );
            }
            PeekCommand::Built { index, built } => {
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
            let row = LocationsFeeds::row_ref(store, self.feed);
            let (hits, done, truncated, title) = row
                .map(|row| (row.locations.len(), row.done, row.truncated, row.title.as_str()))
                .unwrap_or((0, true, true, ""));
            let state = match (done, truncated) {
                (false, _) => format!("{hits} — searching…"),
                (true, true) => format!("{hits} (cut off)"),
                (true, false) => format!("{hits}"),
            };
            let header = crate::ui::ListRow::new(arena, crate::ui::RowStyle::header(store, ui))
                .label(format!("{title} · {state}"))
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
            // The refresh probe: a 1px leaf whose own paint files
            // Refresh when the feed moved — the CARD keeps painting;
            // hijacking the card's Paint blanked a frame per batch.
            let stale = LocationsFeeds::row_ref(store, self.feed)
                .is_some_and(|row| row.generation != self.shown);
            if stale {
                card.place(
                    0.0,
                    0.0,
                    imba::leaf::leaf::<PeekCommand>(1.0, 1.0).event(
                        move |_arena, event, _size| match event {
                            Event::Paint { .. } => EventResult::Command(PeekCommand::Refresh),
                            _ => EventResult::Ignored,
                        },
                    ),
                );
            }
            card.wrap_realized(move |card| PeekWidget { card, size })
        })
    }
}

/// The card's widget shell: a paint over a moved feed refreshes the
/// master (the pump is app-level; the face notices on the frame the
/// landing scheduled).
struct PeekWidget<'a> {
    card: imba::container::RealizedContainer<'a, PeekCommand>,
    size: Size,
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
        self.card.handle_event(arena, event, viewport)
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
        let ui = imba::UiCtx::dont_use_too_slow();
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
        let ui = imba::UiCtx::dont_use_too_slow();
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
