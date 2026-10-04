// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Go-to reference peek (docs/ui/location-list.md §8): an
//! `InlayMode::Under` master–detail card under the caret's line —
//! the quick random-access interface. The card is a FACE over a
//! store-level feed (`locations::LocationLists`): the master is a
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

use crate::views::files_forest;
use crate::{FeedId, FoundLocation, LocationKey, LocationLists, LocationsAsk};
use editor::{Document, EditorCommand, EditorView, InlayKey};
use hikit::ForestList;
use hikit::{tree_toggle, TreeListCommand};
use hikit::{ListKeyCommand, ListKeyboardController};
use imba::list::{ActivateTrigger, ListOps};

const PEEK_HEIGHT: f32 = 280.0;

const HEADER_HEIGHT: f32 = 24.0;

#[derive(Clone)]
pub enum PeekCommand {
    Tree(ListKeyCommand<TreeListCommand>),
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
        built: documents::BuiltDocument,
    },
}

impl std::fmt::Display for PeekCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeekCommand::Tree(command) => command.fmt(out),
            PeekCommand::Preview(command) => command.fmt(out),
            PeekCommand::Close => out.write_str("peek close"),
            PeekCommand::Promote => out.write_str("peek promote"),
            PeekCommand::Refresh => out.write_str("peek refresh"),
            PeekCommand::Fetched { .. } => out.write_str("peek fetched"),
            PeekCommand::Built { .. } => out.write_str("peek built"),
        }
    }
}

#[derive(Clone)]
pub struct PeekView {
    /// The hosting document and the card's own key, baked in by the
    /// push → swap two-step so the card can remove itself.
    host: Option<documents::DocumentId>,
    key: Option<InlayKey>,
    width: f32,
    lists: imba::store::Id<LocationLists>,
    feed: FeedId,

    tree: ListKeyboardController<ForestList<LocationKey>>,
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
    preview_for: Option<editor::ResourceLocation>,
    preview_markup: Option<editor::MarkupId>,
    preview_hit: Option<(u32, u32)>,
    fetch_token: Option<imba::effect::CancellationToken>,

    /// The open-in-pane verb, injected at mount — the shell's window
    /// rides in the closure; the card never holds one.
    open: Arc<
        dyn Fn(&mut Store, editor::ResourceLocation, std::ops::Range<documents::LineCol>)
            + Send
            + Sync,
    >,

    /// The promote-to-dock verb, injected at mount.
    promote: Arc<dyn Fn(&mut Store) + Send + Sync>,
}

impl PeekView {
    pub fn new(
        store: &Store,
        host: Option<documents::DocumentId>,
        width: f32,
        lists: imba::store::Id<LocationLists>,
        feed: FeedId,
        promote: Arc<dyn Fn(&mut Store) + Send + Sync>,
        open: Arc<
            dyn Fn(&mut Store, editor::ResourceLocation, std::ops::Range<documents::LineCol>)
                + Send
                + Sync,
        >,
    ) -> Self {
        Self {
            open,
            promote,
            host,
            key: None,
            width,
            lists,
            feed,
            tree: ListKeyboardController::new(ForestList::new(store)).with_folds(),
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

    pub fn keyed(mut self, key: InlayKey) -> Self {
        self.key = Some(key);
        self
    }

    fn row(&self, store: &Store) -> crate::LocationsFeedRow {
        LocationLists::row(store, self.lists, self.feed).unwrap_or_default()
    }

    fn found(&self, store: &Store, key: &LocationKey) -> Option<FoundLocation> {
        let index = self.targets.get(key).copied()?;
        LocationLists::row_ref(store, self.lists, self.feed)
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
        let cursor = self.tree.inner().list().cursor().cloned();
        self.tree.inner_mut().set(&forest, store, ui);
        if let Some(cursor) = cursor {
            if self.tree.inner().forest.contains(&cursor) {
                self.tree.inner_mut().list_mut().select_only(cursor);
            }
        }
        self.ensure_preview(store, ui, fx);
    }

    /// Ensure the detail shows the SELECTED location. Same file:
    /// re-scroll and re-tint the standing editor — O(log n). New
    /// file: the one fetch this surface makes before navigation;
    /// the build runs off-thread and the mount is BUDGETED (the
    /// mount_editor discipline), never a whole-document layout on
    /// the UI thread.
    fn ensure_preview(
        &mut self,
        store: &Store,
        ui: &imba::UiCtx,
        fx: &mut imba::effect::Effects<'_, PeekCommand>,
    ) {
        let Some(key) = self.tree.inner().list().cursor().cloned() else {
            return;
        };
        let Some(index) = self.targets.get(&key).copied() else {
            return;
        };
        let Some(found) = LocationLists::row_ref(store, self.lists, self.feed)
            .and_then(|row| row.locations.get(index))
            .cloned()
        else {
            return;
        };
        if self.preview_for.as_ref() == Some(&found.location) {
            self.retarget_preview(store, ui, &found, fx);
            return;
        }
        self.preview_for = Some(found.location.clone());
        self.preview = None;
        self.preview_markup = None;
        self.preview_hit = None;
        let location = found.location;
        fx.relaunch_erased(
            &mut self.fetch_token,
            AnyEffect::new(documents::FetchDocumentEffect { location })
                .map(move |text| PeekCommand::Fetched { index, text }),
        );
    }

    /// Move the standing preview onto another hit of the SAME file:
    /// swap the one-range wash (old ∪ new as the change set) and
    /// re-scroll. No fetch, no layout.
    fn retarget_preview(
        &mut self,
        store: &Store,
        ui: &imba::UiCtx,
        found: &FoundLocation,
        fx: &mut imba::effect::Effects<'_, PeekCommand>,
    ) {
        let Some(preview) = &mut self.preview else {
            return;
        };
        let Some(markup) = self.preview_markup else {
            return;
        };
        let fonts = editor::env::Fonts::of(store)();
        let theme = editor::env::Themes::of(store);
        let view = preview.content_mut();
        let (hit, target) = {
            let mut text = view.document.text().view();
            let hit = documents::offset_at(&mut text, found.target().start) as u32;
            (hit, (hit, hit + found.length.max(1)))
        };
        if self.preview_hit == Some(target) {
            return;
        }
        let mut changed: Vec<std::ops::Range<u32>> = vec![target.0..target.1];
        if let Some((start, end)) = self.preview_hit.replace(target) {
            changed.push(start..end);
        }
        let mut tints = editor::Markup::new();
        tints.push_styled(target.0..target.1, editor::theme::StyleId::Match);
        let scoped = |command| PeekCommand::Preview(imba::scroll::ScrollCommand::Content(command));
        fx.scope(scoped, |fx| {
            view.document
                .replace_markup(markup, tints, &changed, store, ui, &fonts, &theme, fx)
        });
        view.set_caret(hit);
        let editor = view.editor;
        fx.scope(scoped, |fx| {
            view.document
                .reveal_at_instant(editor, hit, store, ui, &fonts, &theme, fx)
        });
        let reveal = view
            .document
            .caret_content_rect(editor, hit, store, ui, &fonts, &theme)
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
        ui: &imba::UiCtx,
        built: documents::BuiltDocument,
        index: usize,
        fx: &mut imba::effect::Effects<'_, PeekCommand>,
    ) {
        let Some(found) = LocationLists::row_ref(store, self.lists, self.feed)
            .and_then(|row| row.locations.get(index))
            .cloned()
        else {
            return;
        };
        if self.preview_for.as_ref() != Some(&found.location) {
            return; // the cursor moved on while the build ran
        }
        let fonts = editor::env::Fonts::of(store)();
        let theme = editor::env::Themes::of(store);
        let mut document = built.document;

        let (hit, target) = {
            let mut view = document.text().view();
            let hit = documents::offset_at(&mut view, found.target().start) as u32;
            (hit, hit..hit + found.length.max(1))
        };

        let markup = document.add_markup();
        let mut tints = editor::Markup::new();
        tints.push_styled(target.clone(), editor::theme::StyleId::Match);
        let scoped = |command| PeekCommand::Preview(imba::scroll::ScrollCommand::Content(command));
        fx.scope(scoped, |fx| {
            document.replace_markup(
                markup,
                tints,
                &[target.clone()],
                store,
                ui,
                &fonts,
                &theme,
                fx,
            )
        });

        let detail = (self.width * 0.6 - 1.0).max(120.0);
        let editor = fx.scope(scoped, |fx| {
            document.add_editor(
                detail,
                None,
                ::editor::EditorBuild::Bounded,
                &[],
                store,
                ui,
                &fonts,
                &theme,
                fx,
            )
        });
        document.show_markup(editor, markup);
        fx.scope(scoped, |fx| {
            document.reveal_at_instant(editor, hit, store, ui, &fonts, &theme, fx)
        });
        let reveal = document
            .caret_content_rect(editor, hit, store, ui, &fonts, &theme)
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
        (self.open)(store, found.location.clone(), found.target());
    }

    /// Dismiss the card. The feed dies with it unless it was handed
    /// on (the promote path fronts it in the dock instead).
    fn close(&mut self, store: &mut Store, keep_feed: bool) {
        if !keep_feed {
            LocationLists::ask(store, self.lists, LocationsAsk::Dispose(self.feed));
        }
        if let (Some(host), Some(key)) = (self.host, self.key) {
            imba::command::Requests::push(
                store,
                Arc::new(RemovePeek {
                    lists: self.lists,
                    document: host,
                    key,
                }),
            );
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
        // The key table is the controller's; the card keeps only its
        // own close.
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape => EventResult::Command(PeekCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(self.tree.focus_data(_store, _ui).map(PeekCommand::Tree))
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
                type Tree = ListKeyboardController<ForestList<LocationKey>>;
                match &command {
                    ListKeyCommand::Fold { expand, .. } => {
                        return self.tree.inner_mut().fold_cursor(*expand, store, ui);
                    }
                    ListKeyCommand::Inner(inner) => {
                        if let Some(index) = tree_toggle(inner) {
                            let Some(key) = self.tree.inner().list().key_at(index).cloned() else {
                                return;
                            };
                            self.tree.inner_mut().list_mut().select_only(key.clone());
                            self.tree.inner_mut().toggle(&key, store, ui);
                            return self.ensure_preview(store, ui, fx);
                        }
                    }
                    _ => {}
                }
                if let Some((index, trigger)) = Tree::activated(&command) {
                    let Some(key) = self.tree.inner().list().key_at(index).cloned() else {
                        return;
                    };
                    // Both triggers behave alike here: a file row
                    // toggles its hits, a hit navigates and closes.
                    match (&key, trigger) {
                        (LocationKey::Node(_), ActivateTrigger::Enter | ActivateTrigger::Click) => {
                            self.tree.inner_mut().toggle(&key, store, ui);
                            return self.ensure_preview(store, ui, fx);
                        }
                        (LocationKey::Hit(..), ActivateTrigger::Enter | ActivateTrigger::Click) => {
                            if let Some(found) = self.found(store, &key) {
                                self.navigate(store, &found);
                                self.close(store, false);
                            }
                            return;
                        }
                    }
                }
                let selected = Tree::selected_index(&command).is_some();
                fx.scope(PeekCommand::Tree, |fx| {
                    self.tree.perform(store, ui, command, fx)
                });
                // Moving the selection returns the preview.
                if selected {
                    self.ensure_preview(store, ui, fx);
                }
            }
            PeekCommand::Close => self.close(store, false),
            PeekCommand::Promote => {
                (self.promote)(store);
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
                let Some(location) = LocationLists::row_ref(store, self.lists, self.feed)
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
                    AnyEffect::new(documents::BuildDocumentEffect { location, text })
                        .map(move |built| PeekCommand::Built { index, built }),
                );
            }
            PeekCommand::Built { index, built } => {
                self.install_preview(store, ui, built, index, fx);
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
            let chrome = editor::env::Themes::of(store).ui().peeker.clone();
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
            let row = LocationLists::row_ref(store, self.lists, self.feed);
            let (hits, done, truncated, title) = row
                .map(|row| {
                    (
                        row.locations.len(),
                        row.done,
                        row.truncated,
                        row.title.as_str(),
                    )
                })
                .unwrap_or((0, true, true, ""));
            let state = match (done, truncated) {
                (false, _) => format!("{hits} — searching…"),
                (true, true) => format!("{hits} (cut off)"),
                (true, false) => format!("{hits}"),
            };
            let header = hikit::ui::ListRow::new(arena, hikit::ui::RowStyle::header(store, ui))
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
            let stale = LocationLists::row_ref(store, self.lists, self.feed)
                .is_some_and(|row| row.generation != self.shown);
            if stale {
                card.place(
                    0.0,
                    0.0,
                    imba::leaf::leaf::<PeekCommand>(1.0, 1.0).event(move |_arena, event, _size| {
                        match event {
                            Event::Paint { .. } => EventResult::Command(PeekCommand::Refresh),
                            _ => EventResult::Ignored,
                        }
                    }),
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
    lists: imba::store::Id<LocationLists>,
    document: documents::DocumentId,
    key: InlayKey,
}

impl imba::command::DynamicCommand for RemovePeek {
    fn id(&self) -> &'static str {
        "peek.remove"
    }

    fn name(&self) -> String {
        "Close Reference Peek".to_owned()
    }

    fn perform(&self, store: &mut Store, ui: &imba::UiCtx, fx: &mut imba::command::Fx<'_>) {
        // The collection's wired sibling, not the window's current
        // family — the window may have moved on since the mount.
        let Some(documents) = LocationLists::documents_of(store, self.lists) else {
            return;
        };
        let Some(mut doc) = documents::OpenDocuments::document(store, documents, self.document)
        else {
            return;
        };
        let fonts = editor::env::Fonts::of(store)();
        let theme = editor::env::Themes::of(store);
        let document = self.document;
        fx.scope(
            move |command| {
                imba::command::Verb::at(
                    documents,
                    documents::DocumentsCommand::Editor(document, command),
                )
            },
            |fx| doc.remove_inlay(self.key, store, ui, &fonts, &theme, fx),
        );
        documents::OpenDocuments::put_document(store, documents, self.document, doc);
    }
}

/// The caret's character as a non-empty anchor span, char-boundary
/// snapped; the character before it at the text's end; `None` on an
/// empty document. An EMPTY anchor renders nothing — an Under
/// inlay's line anchoring needs a real span.
pub fn caret_anchor(document: &Document, caret: u32) -> Option<std::ops::Range<u32>> {
    let len = document.text().byte_count().min(u32::MAX as usize) as u32;
    if len == 0 {
        return None;
    }
    let mut view = document.text().view();
    if caret < len {
        let head = view.substring(caret..(caret + 4).min(len));
        let step = head
            .chars()
            .next()
            .map(|ch| ch.len_utf8() as u32)
            .unwrap_or(1);
        Some(caret..(caret + step).min(len))
    } else {
        let tail = view.substring(len.saturating_sub(4)..len);
        let step = tail
            .chars()
            .last()
            .map(|ch| ch.len_utf8() as u32)
            .unwrap_or(1);
        Some(len.saturating_sub(step)..len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(name: &str, line: u32, context: &str) -> FoundLocation {
        FoundLocation {
            location: editor::ResourceLocation::new(
                editor::ResourceType::document(),
                editor::Authority::new("local"),
                vec!["work".to_owned(), name.to_owned()],
            ),
            line,
            column: 2,
            length: 3,
            context: context.to_owned(),
            context_column_start: 0,
        }
    }

    fn feed(
        store: &mut Store,
        rows: &[FoundLocation],
        done: bool,
    ) -> (imba::store::Id<LocationLists>, FeedId) {
        let documents = imba::store::Id::mint();
        store.put_entity(documents, documents::OpenDocuments::default());
        let lists = imba::store::Id::mint();
        store.put_entity(lists, LocationLists::wired(documents));
        let feed = FeedId::mint();
        let mut row = crate::LocationsFeedRow {
            title: "References".to_owned(),
            generation: 1,
            done,
            ..Default::default()
        };
        row.locations = rows.iter().cloned().collect();
        LocationLists::put(store, lists, feed, row);
        (lists, feed)
    }

    #[test]
    fn the_master_groups_by_file_and_navigates_on_pick() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        let (lists, feed) = feed(
            &mut store,
            &[
                found("a.rs", 1, "  let x = y;"),
                found("a.rs", 7, "x + 1"),
                found("b.rs", 0, "fn b()"),
            ],
            true,
        );
        let opened: Arc<std::sync::Mutex<Option<editor::ResourceLocation>>> = Default::default();
        let noted = opened.clone();
        let mut view = PeekView::new(
            &store,
            None,
            600.0,
            lists,
            feed,
            Arc::new(|_: &mut Store| {}),
            Arc::new(move |_: &mut Store, location, _| {
                *noted.lock().unwrap() = Some(location);
            }),
        );
        let mut batch = imba::effect::Batch::new();
        view.rebuild(&mut store, &ui, &mut batch.effects());

        let rows = view.tree.inner().forest.rows();
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
        view.tree.inner_mut().list_mut().select_only(hit);
        let at = view.tree.cursor_index().expect("a cursor row");
        let pick = PeekCommand::Tree(view.tree.activate_command(at, ActivateTrigger::Enter));
        view.perform(&mut store, &ui, pick, &mut batch.effects());
        assert!(
            opened.lock().unwrap().is_some(),
            "the pick navigated through the injected opener"
        );
        assert!(
            LocationLists::row(&store, lists, feed).is_some(),
            "disposal is a NOTE for the lane, never the view's own teardown"
        );
        assert!(
            LocationLists::owes_asks(&store, lists),
            "the close noted the dispose ask"
        );
    }

    #[test]
    fn a_single_settled_result_navigates_without_a_card() {
        let mut store = Store::new();
        let ui = ::editor::test_document::test_ui();
        let (lists, feed) = feed(&mut store, &[found("a.rs", 3, "only")], true);
        let opened: Arc<std::sync::Mutex<Option<editor::ResourceLocation>>> = Default::default();
        let noted = opened.clone();
        let mut view = PeekView::new(
            &store,
            None,
            600.0,
            lists,
            feed,
            Arc::new(|_: &mut Store| {}),
            Arc::new(move |_: &mut Store, location, _| {
                *noted.lock().unwrap() = Some(location);
            }),
        );
        let mut batch = imba::effect::Batch::new();
        view.rebuild(&mut store, &ui, &mut batch.effects());
        assert!(view.navigated, "the trivial case went straight through");
        assert!(opened.lock().unwrap().is_some());
    }

    #[test]
    fn the_peek_anchor_reserves_height() {
        let store = &imba::store::Store::new();
        let ui = ::editor::test_document::test_ui();
        let mut document = ::editor::test_document::plain_document("fn a() {}\nfn b() {}\n");
        let fonts = ::editor::test_document::test_fonts_collection();
        let theme = editor::theme::Theme::embedded();
        let mut batch = imba::effect::Batch::new();
        let editor = document.add_editor(
            600.0,
            None,
            ::editor::EditorBuild::Complete,
            &[],
            store,
            ui,
            &fonts,
            &theme,
            &mut batch.effects(),
        );
        let bare = document.content_height(editor);

        let markup = editor::MarkupId::mint();
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
                editor::Inlay::new(editor::InlayMode::Under, ProbeCard),
                store,
                ui,
                &fonts,
                &theme,
                &mut batch.effects(),
            );
            let with_card = document.content_height(editor);
            assert!(
                with_card > bare,
                "the anchored card reserves height: {with_card} vs {bare}"
            );
            document.remove_inlay(key, store, ui, &fonts, &theme, &mut batch.effects());
        }
    }

    #[derive(Clone)]
    struct ProbeCard;

    impl imba::View for ProbeCard {
        type Command = std::convert::Infallible;
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
                imba::ThunkBox::new(
                    arena,
                    imba::leaf::leaf::<std::convert::Infallible>(200.0, 111.0),
                )
            })
        }
    }
}
