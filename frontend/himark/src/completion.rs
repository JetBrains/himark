// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::effect::{CancellationToken, Effects};
use imba::event::{Event, EventResult, Key};
use imba::store::Store;
use imba::thunk_ext::ThunkExt;
use imba::{UiCtx, View};

use crate::rows::{RowList, RowListCommand};
use crate::{FindEffect, LineCol, ResourceLocation};

const SHOWN: usize = 128;

const POPUP_WIDTH: f32 = 560.0;
const VISIBLE_ROWS: usize = 9;

pub enum CompletionCommand {
    Select(isize),

    PickCursor,

    Rows(RowListCommand),

    Close,
}

#[derive(Clone)]
pub enum CompletionFound {
    Path {
        serial: u64,
        locations: Vec<ResourceLocation>,
    },
    Lsp {
        serial: u64,
        answer: Option<LspAnswer>,
    },
}

#[derive(Clone, Debug)]
pub struct LspAnswer {
    pub items: Vec<LspItem>,

    pub incomplete: bool,
}

#[derive(Clone, Debug)]
pub struct LspItem {
    pub label: String,
    pub detail: Option<String>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,

    pub edit: Option<(std::ops::Range<LineCol>, String)>,
    pub insert_text: Option<String>,
}

pub struct LspCompletionEffect {
    pub location: ResourceLocation,
    pub position: LineCol,
}

impl imba::effect::Effect for LspCompletionEffect {
    type Result = Option<LspAnswer>;
}

#[derive(Clone)]
pub struct PickedFile {
    pub label: String,

    pub rel: String,
    pub location: ResourceLocation,
}

#[derive(Clone)]
enum SourceState {
    Path {
        folders: Arc<Vec<ResourceLocation>>,
        recents: Arc<Vec<ResourceLocation>>,
        found: Arc<Vec<ResourceLocation>>,

        rows: Arc<Vec<ResourceLocation>>,
    },
    Lsp {
        items: Arc<Vec<LspItem>>,
        incomplete: bool,

        rows: Arc<Vec<usize>>,
    },
}

impl SourceState {
    fn row_count(&self) -> usize {
        match self {
            SourceState::Path { rows, .. } => rows.len(),
            SourceState::Lsp { rows, .. } => rows.len(),
        }
    }
}

#[derive(Clone)]
pub struct CompletionPopupView {
    list: RowList,
    rows: usize,
}

impl View for CompletionPopupView {
    type Command = CompletionCommand;

    fn focus_data<'w>(
        &'w self,
        _store: &'w imba::store::Store,
        _ui: &'w imba::UiCtx,
    ) -> imba::focus::FocusData<'w, CompletionCommand> {
        use imba::event::EventResult;
        let armed = self.list.len() > 0;
        imba::focus::FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                Key::Up if armed => EventResult::Command(CompletionCommand::Select(-1)),
                Key::Down if armed => EventResult::Command(CompletionCommand::Select(1)),
                Key::Enter | Key::Tab if armed => {
                    EventResult::Command(CompletionCommand::PickCursor)
                }
                Key::Escape => EventResult::Command(CompletionCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..imba::focus::FocusData::default()
        }
    }

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: Self::Command,
        _fx: &mut Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a imba::arena::Arena, constraints: imba::constraints::Constraints| {
                let theme = crate::env::Themes::of(store);
                let chrome = theme.ui().peeker.clone();
                let row_height = chrome.row_height.max(1.0);

                let visible = self.rows.min(VISIBLE_ROWS).max(1);
                let natural = skia_safe::Size::new(POPUP_WIDTH, visible as f32 * row_height + 2.0);
                let width = match constraints.max.width.is_finite() {
                    true => constraints.max.width,
                    false => natural.width,
                };
                let height = match constraints.max.height.is_finite() {
                    true => constraints.max.height,
                    false => natural.height,
                };
                let mut popup =
                    imba::container::Container::new(arena, skia_safe::Size::new(width, height));
                let fill = chrome.background.0;
                let border = chrome.rule.0;
                popup.place(
                    0.0,
                    0.0,
                    imba::leaf::leaf::<CompletionCommand>(width, height).paint_instead(
                        move |_arena, canvas, rect| {
                            let mut paint = skia_safe::Paint::default();
                            paint.set_color(fill);
                            canvas.draw_rect(rect, &paint);
                            paint.set_stroke(true);
                            paint.set_stroke_width(1.0);
                            paint.set_color(border);
                            canvas.draw_rect(rect.with_inset((0.5, 0.5)), &paint);
                        },
                    ),
                );
                let rows = imba::Layout::layout(
                    self.list.display(arena, store, ui),
                    arena,
                    imba::constraints::Constraints::tight(skia_safe::Size::new(
                        width - 2.0,
                        height - 2.0,
                    )),
                )
                .map(CompletionCommand::Rows);
                popup.place(1.0, 1.0, rows);

                let armed = self.list.len() > 0;
                popup.place(
                    0.0,
                    0.0,
                    imba::leaf::leaf::<CompletionCommand>(width, height).event(
                        move |_arena, event, _size| match event {
                            Event::KeyDown { key: Key::Up, .. } if armed => {
                                EventResult::Command(CompletionCommand::Select(-1))
                            }
                            Event::KeyDown { key: Key::Down, .. } if armed => {
                                EventResult::Command(CompletionCommand::Select(1))
                            }

                            Event::KeyDown {
                                key: Key::Enter, ..
                            } if armed => EventResult::Command(CompletionCommand::PickCursor),
                            Event::KeyDown { key: Key::Tab, .. } if armed => {
                                EventResult::Command(CompletionCommand::PickCursor)
                            }
                            Event::KeyDown {
                                key: Key::Escape, ..
                            } => EventResult::Command(CompletionCommand::Close),
                            _ => EventResult::Ignored,
                        },
                    ),
                );
                popup
            },
        )
    }
}

#[derive(Clone)]
pub struct Completion {
    key: Option<crate::InlayKey>,
    markup: Option<crate::MarkupId>,

    installed: Option<(crate::DocumentId, ::editor::EditorId)>,
    query: String,

    anchor_offset: u32,

    serial: u64,

    lane: Option<CancellationToken>,
    list: RowList,
    source: SourceState,
}

impl Completion {
    pub fn new() -> Self {
        Self {
            key: None,
            markup: None,
            installed: None,
            query: String::new(),
            anchor_offset: 0,
            serial: 0,
            lane: None,
            list: RowList::new(),
            source: SourceState::Path {
                folders: Arc::new(Vec::new()),
                recents: Arc::new(Vec::new()),
                found: Arc::new(Vec::new()),
                rows: Arc::new(Vec::new()),
            },
        }
    }

    pub fn open(&self) -> bool {
        self.key.is_some()
    }

    pub fn inlay_key(&self) -> Option<crate::InlayKey> {
        self.key
    }

    pub fn installed(&self) -> Option<(crate::DocumentId, ::editor::EditorId)> {
        self.installed
    }

    pub fn serial(&self) -> u64 {
        self.serial
    }

    pub fn selected(&self) -> usize {
        self.list.selected()
    }

    #[doc(hidden)]
    pub fn row_labels(&self) -> Vec<String> {
        match &self.source {
            SourceState::Path { rows, .. } => rows
                .iter()
                .map(|location| location.name().to_owned())
                .collect(),
            SourceState::Lsp { items, rows, .. } => rows
                .iter()
                .filter_map(|index| items.get(*index))
                .map(|item| item.label.clone())
                .collect(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sync_path<C, W, E>(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        document: &mut crate::Document,
        editor: ::editor::EditorId,
        typed_at: Option<u32>,
        session: &crate::SessionId,
        installed: Option<(crate::DocumentId, ::editor::EditorId)>,
        fx: &mut Effects<'_, C>,
        wrap: W,
        to_editor: E,
    ) where
        C: 'static,
        W: Fn(CompletionFound) -> C + Send + Sync + Clone + 'static,
        E: Fn(crate::EditorCommand) -> C + Send + Sync + Clone + 'static,
    {
        if self.key.is_some() {
            let caret = document.caret_byte(editor);
            let Some(at) = self.marker_start(document, editor) else {
                self.drop_state(document, store, ui, fx, to_editor);
                return;
            };
            let mut view = document.text().view();
            let end = view.byte_count() as u32;
            let still_at = at < end && view.substring(at..(at + 1).min(end)) == "@";
            let query = (still_at && caret > at)
                .then(|| view.substring(at + 1..caret.min(end)))
                .filter(|query| !query.chars().any(|c| c.is_whitespace() || c == '@'));
            let Some(query) = query else {
                self.drop_state(document, store, ui, fx, to_editor);
                return;
            };
            if query != self.query {
                self.query = query;
                self.launch_path(fx, wrap);
                self.refresh(store, ui, document, editor);
            }
            return;
        }
        let Some(at) = typed_at else { return };

        let mut view = document.text().view();
        let boundary = at == 0 || {
            let before = view.substring(at.saturating_sub(1)..at);
            before.chars().all(char::is_whitespace)
        };
        if !boundary {
            return;
        }
        self.source = SourceState::Path {
            folders: Arc::new(crate::higent::session_folders(store, session)),
            recents: Arc::new(crate::RecentLocations::list(store)),
            found: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
        };
        self.query = String::new();
        self.open_marker(store, ui, document, editor, at, 1, installed, fx, to_editor);
    }

    fn launch_path<C, W>(&mut self, fx: &mut Effects<'_, C>, wrap: W)
    where
        C: 'static,
        W: Fn(CompletionFound) -> C + Send + Sync + Clone + 'static,
    {
        self.serial += 1;
        let SourceState::Path { folders, found, .. } = &mut self.source else {
            return;
        };
        if folders.is_empty() || self.query.len() < 2 {
            *found = Arc::new(Vec::new());
            return;
        }
        let serial = self.serial;
        let effect = imba::effect::AnyEffect::new(FindEffect {
            folders: folders.as_ref().clone(),
            term: self.query.clone(),
        })
        .map(move |locations| wrap(CompletionFound::Path { serial, locations }));
        fx.relaunch_erased(&mut self.lane, effect);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sync_lsp<C, W, E>(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        document: &mut crate::Document,
        editor: ::editor::EditorId,
        typed: Option<&str>,
        explicit: bool,
        location: &ResourceLocation,
        installed: Option<(crate::DocumentId, ::editor::EditorId)>,
        fx: &mut Effects<'_, C>,
        wrap: W,
        to_editor: E,
    ) where
        C: 'static,
        W: Fn(CompletionFound) -> C + Send + Sync + Clone + 'static,
        E: Fn(crate::EditorCommand) -> C + Send + Sync + Clone + 'static,
    {
        let caret = document.caret_byte(editor);
        if self.key.is_some() {
            let Some(anchor) = self.query_anchor(document, editor) else {
                self.drop_state(document, store, ui, fx, to_editor);
                return;
            };
            let mut view = document.text().view();
            let end = view.byte_count() as u32;
            let query = (caret >= anchor)
                .then(|| view.substring(anchor..caret.min(end)))
                .filter(|query| query.chars().all(identifier_char));
            let Some(query) = query else {
                self.drop_state(document, store, ui, fx, to_editor);
                return;
            };
            if query != self.query {
                let grew = query.len() > self.query.len() && query.starts_with(&self.query);
                self.query = query;
                let incomplete = matches!(
                    &self.source,
                    SourceState::Lsp {
                        incomplete: true,
                        ..
                    }
                );
                if grew && !incomplete {
                    self.refresh(store, ui, document, editor);
                } else {
                    self.launch_lsp(document, editor, location, fx, wrap);
                    self.refresh(store, ui, document, editor);
                }
            }
            return;
        }

        let opener = if explicit {
            let mut view = document.text().view();
            let start = word_start(&mut view, caret);
            if start < caret {
                Some((start, 0))
            } else if caret > 0 {
                Some((caret - 1, 1))
            } else {
                None
            }
        } else {
            match typed {
                Some(text) if text.chars().count() == 1 => {
                    let typed_char = text.chars().next().expect("one char");
                    if trigger_char(location, typed_char) {
                        Some((caret - typed_char.len_utf8() as u32, 1))
                    } else if identifier_char(typed_char) {
                        let mut view = document.text().view();
                        let start = word_start(&mut view, caret);

                        (start == caret - typed_char.len_utf8() as u32).then_some((start, 0))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        };
        let Some((cover, offset)) = opener else {
            return;
        };
        self.source = SourceState::Lsp {
            items: Arc::new(Vec::new()),
            incomplete: false,
            rows: Arc::new(Vec::new()),
        };
        let mut view = document.text().view();
        self.query = view.substring(cover + offset..caret);
        self.open_marker(
            store, ui, document, editor, cover, offset, installed, fx, to_editor,
        );
        self.launch_lsp(document, editor, location, fx, wrap);
    }

    fn launch_lsp<C, W>(
        &mut self,
        document: &crate::Document,
        editor: ::editor::EditorId,
        location: &ResourceLocation,
        fx: &mut Effects<'_, C>,
        wrap: W,
    ) where
        C: 'static,
        W: Fn(CompletionFound) -> C + Send + Sync + Clone + 'static,
    {
        self.serial += 1;
        let serial = self.serial;
        let caret = document.caret_byte(editor) as usize;
        let mut view = document.text().view();
        let position = crate::line_col_at(&mut view, caret);
        let effect = imba::effect::AnyEffect::new(LspCompletionEffect {
            location: location.clone(),
            position,
        })
        .map(move |answer| wrap(CompletionFound::Lsp { serial, answer }));
        fx.relaunch_erased(&mut self.lane, effect);
    }

    fn marker_start(&self, document: &crate::Document, editor: ::editor::EditorId) -> Option<u32> {
        let key = self.key?;
        document
            .popups_in(editor, 0..u32::MAX)
            .into_iter()
            .find(|(standing, _, _, _)| *standing == key)
            .map(|(_, range, _, _)| range.start)
    }

    fn query_anchor(&self, document: &crate::Document, editor: ::editor::EditorId) -> Option<u32> {
        Some(self.marker_start(document, editor)? + self.anchor_offset)
    }

    #[allow(clippy::too_many_arguments)]
    fn open_marker<C, E>(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        document: &mut crate::Document,
        editor: ::editor::EditorId,
        cover: u32,
        anchor_offset: u32,
        installed: Option<(crate::DocumentId, ::editor::EditorId)>,
        fx: &mut Effects<'_, C>,
        to_editor: E,
    ) where
        C: 'static,
        E: Fn(crate::EditorCommand) -> C + Send + Sync + Clone + 'static,
    {
        let markup = document.add_markup();
        document.show_markup(editor, markup);
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        self.markup = Some(markup);
        self.installed = installed;
        self.anchor_offset = anchor_offset;
        self.refresh_rows(store, ui);
        let view = self.view();

        let range = cover..cover + 1;
        let mut key = None;
        fx.scope(to_editor, |fx| {
            key = Some(document.push_inlay(
                markup,
                range,
                crate::Inlay::new(
                    crate::InlayMode::Popup(crate::PopupSpec {
                        host: imba::overlay::WINDOW,
                        position: imba::overlay::fit::PreferredPosition::At {
                            x: imba::overlay::fit::RangeEnd::Begin,
                            side: imba::overlay::fit::Side::Bottom,
                            align: imba::overlay::fit::Align::Left,
                        },
                    }),
                    view,
                ),
                &fonts,
                &theme,
                fx,
            ));
        });
        self.key = key;
    }

    pub fn land(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        document: &mut crate::Document,
        editor: ::editor::EditorId,
        found: CompletionFound,
    ) {
        if !self.open() {
            return;
        }
        match (found, &mut self.source) {
            (
                CompletionFound::Path { serial, locations },
                SourceState::Path { recents, found, .. },
            ) if serial == self.serial => {
                *found = Arc::new(
                    locations
                        .into_iter()
                        .filter(|location| !recents.contains(location))
                        .collect(),
                );
            }
            (
                CompletionFound::Lsp { serial, answer },
                SourceState::Lsp {
                    items, incomplete, ..
                },
            ) if serial == self.serial => {
                let answer = answer.unwrap_or(LspAnswer {
                    items: Vec::new(),
                    incomplete: false,
                });
                let mut fresh = answer.items;
                fresh.sort_by(|a, b| {
                    a.sort_text
                        .as_deref()
                        .unwrap_or(&a.label)
                        .cmp(b.sort_text.as_deref().unwrap_or(&b.label))
                });
                *items = Arc::new(fresh);
                *incomplete = answer.incomplete;
            }
            _ => return,
        }
        self.refresh(store, ui, document, editor);
    }

    pub fn select(
        &mut self,
        _store: &Store,
        document: &mut crate::Document,
        editor: ::editor::EditorId,
        delta: isize,
    ) {
        let count = self.source.row_count();
        if count == 0 {
            return;
        }
        let last = count as isize - 1;
        let next = (self.list.selected() as isize + delta).clamp(0, last) as usize;
        self.list.select(next);
        self.swap_view(document, editor);
    }

    pub fn rows_command(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        document: &mut crate::Document,
        editor: ::editor::EditorId,
        command: RowListCommand,
    ) -> Option<usize> {
        if let Some(row) = self.list.picked(&command) {
            return Some(row);
        }

        let mut discarded = imba::effect::Batch::new();
        self.list
            .perform(store, ui, command, &mut discarded.effects());
        self.swap_view(document, editor);
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_pick<C, E>(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        document: &mut crate::Document,
        editor: ::editor::EditorId,
        row: usize,
        fx: &mut Effects<'_, C>,
        to_editor: E,
    ) -> Option<PickedFile>
    where
        C: 'static,
        E: Fn(crate::EditorCommand) -> C + Send + Sync + Clone + 'static,
    {
        let start = self.marker_start(document, editor)?;
        let anchor = start + self.anchor_offset;
        let caret = document.caret_byte(editor);
        let picked = match &self.source {
            SourceState::Path { folders, rows, .. } => {
                let location = rows.get(row).cloned()?;
                let rel = rel_path(folders, &location);
                let inserted = format!("@{rel} ");
                let end = caret.max(start + 1);
                self.write_replace(
                    store,
                    ui,
                    document,
                    editor,
                    start..end,
                    &inserted,
                    fx,
                    &to_editor,
                );
                Some(PickedFile {
                    label: location.name().to_owned(),
                    rel,
                    location,
                })
            }
            SourceState::Lsp { items, rows, .. } => {
                let item = rows.get(row).and_then(|index| items.get(*index))?.clone();
                let inserted = item
                    .edit
                    .as_ref()
                    .map(|(_, text)| text.clone())
                    .or(item.insert_text.clone())
                    .unwrap_or_else(|| item.label.clone());
                let inserted = strip_snippets(&inserted);

                let range = item
                    .edit
                    .as_ref()
                    .and_then(|(range, _)| {
                        let mut view = document.text().view();
                        let start = crate::offset_at(&mut view, range.start) as u32;
                        let end = crate::offset_at(&mut view, range.end) as u32;
                        (start <= anchor && end >= caret.min(end.max(caret)) && start <= end)
                            .then_some(start..end.max(caret))
                    })
                    .unwrap_or(anchor..caret.max(anchor));
                self.write_replace(
                    store, ui, document, editor, range, &inserted, fx, &to_editor,
                );
                None
            }
        };
        self.drop_state(document, store, ui, fx, to_editor);
        picked
    }

    #[allow(clippy::too_many_arguments)]
    fn write_replace<C, E>(
        &self,
        store: &Store,
        ui: &UiCtx,
        document: &mut crate::Document,
        editor: ::editor::EditorId,
        range: std::ops::Range<u32>,
        inserted: &str,
        fx: &mut Effects<'_, C>,
        to_editor: &E,
    ) where
        C: 'static,
        E: Fn(crate::EditorCommand) -> C + Send + Sync + Clone + 'static,
    {
        let mut view = document.text().view();
        let total = view.byte_count() as u32;
        let range = range.start.min(total)..range.end.min(total);
        let removed = view.substring(range.clone());
        let mut builder = operation::OperationBuilder::new();
        builder.push_retain(range.start);
        builder.push_delete(removed);
        builder.push_insert(inserted.to_owned());
        builder.push_retain(total - range.end);
        let operation = builder.finish();
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        fx.scope(to_editor.clone(), |fx| {
            document.edit(&operation, &fonts, &theme, fx);
        });
        document.set_caret(editor, range.start + inserted.len() as u32);
    }

    pub fn drop_state<C, E>(
        &mut self,
        document: &mut crate::Document,
        store: &Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, C>,
        to_editor: E,
    ) where
        C: 'static,
        E: Fn(crate::EditorCommand) -> C + Send + Sync + Clone + 'static,
    {
        if let Some(token) = self.lane.take() {
            fx.cancel(token);
        }
        if let Some(markup) = self.markup.take() {
            let fonts = crate::env::ui_collection(store, ui);
            let theme = crate::env::Themes::of(store);
            fx.scope(to_editor, |fx| {
                document.remove_markup(markup, &[], &fonts, &theme, fx);
            });
        }
        self.clear();
    }

    pub fn clear(&mut self) {
        self.key = None;
        self.markup = None;
        self.installed = None;
        self.query.clear();
        self.anchor_offset = 0;
        self.source = SourceState::Path {
            folders: Arc::new(Vec::new()),
            recents: Arc::new(Vec::new()),
            found: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
        };
    }

    pub fn refresh(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        document: &mut crate::Document,
        editor: ::editor::EditorId,
    ) {
        self.refresh_rows(store, ui);
        self.swap_view(document, editor);
    }

    fn refresh_rows(&mut self, store: &Store, ui: &UiCtx) {
        let query = self.query.to_lowercase();
        let mut labels = Vec::new();
        let mut trails = Vec::new();
        let mut hidden = 0usize;
        match &mut self.source {
            SourceState::Path {
                folders,
                recents,
                found,
                rows,
            } => {
                let mut fresh = Vec::new();
                let mut push = |location: &ResourceLocation| {
                    if fresh.len() >= SHOWN {
                        hidden += 1;
                        return;
                    }
                    labels.push(location.name().to_owned());
                    trails.push(Some(rel_path(folders, location)));
                    fresh.push(location.clone());
                };

                for location in recents.iter() {
                    if crate::speedsearch::subsequence_match(
                        &location.name().to_lowercase(),
                        &query,
                    ) {
                        push(location);
                    }
                }
                for location in found.iter() {
                    push(location);
                }
                *rows = Arc::new(fresh);
            }
            SourceState::Lsp { items, rows, .. } => {
                let mut fresh = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    if fresh.len() >= SHOWN {
                        hidden += 1;
                        continue;
                    }
                    let haystack = item.filter_text.as_deref().unwrap_or(&item.label);
                    if !crate::speedsearch::subsequence_match(&haystack.to_lowercase(), &query) {
                        continue;
                    }
                    labels.push(item.label.clone());
                    trails.push(item.detail.clone());
                    fresh.push(index);
                }
                *rows = Arc::new(fresh);
            }
        }
        let note = (hidden > 0).then(|| format!("… {hidden} more — narrow the filter"));
        let selected = self
            .list
            .selected()
            .min(self.source.row_count().saturating_sub(1));
        self.list
            .set_with_trails(store, ui, &labels, &trails, note, selected);
    }

    fn view(&self) -> CompletionPopupView {
        CompletionPopupView {
            list: self.list.clone(),
            rows: self.source.row_count(),
        }
    }

    fn swap_view(&mut self, document: &mut crate::Document, editor: ::editor::EditorId) {
        let Some(key) = self.key else { return };
        let Some((_, range, _, spec)) = document
            .popups_in(editor, 0..u32::MAX)
            .into_iter()
            .find(|(standing, _, _, _)| *standing == key)
        else {
            return;
        };
        document.swap_inlay(
            key,
            range,
            crate::Inlay::new(crate::InlayMode::Popup(spec), self.view()),
        );
    }
}

fn identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_start(view: &mut ::text::TextView, caret: u32) -> u32 {
    let line = view.line_at(caret as usize);
    let line_start = view.line_start_offset(line) as u32;
    let prefix = view.substring(line_start..caret);
    let tail = prefix
        .chars()
        .rev()
        .take_while(|c| identifier_char(*c))
        .map(char::len_utf8)
        .sum::<usize>() as u32;
    caret - tail
}

fn trigger_char(location: &ResourceLocation, c: char) -> bool {
    match location.extension().as_str() {
        "rs" => matches!(c, '.' | ':'),
        _ => c == '.',
    }
}

fn strip_snippets(text: &str) -> String {
    if !text.contains('$') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('{') => {
                chars.next();

                let mut body = String::new();
                let mut depth = 1;
                for inner in chars.by_ref() {
                    match inner {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    body.push(inner);
                }
                if let Some((_, placeholder)) = body.split_once(':') {
                    out.push_str(placeholder);
                }
            }

            Some('0') => {
                chars.next();
            }
            _ => out.push('$'),
        }
    }
    out
}

fn rel_path(folders: &[ResourceLocation], location: &ResourceLocation) -> String {
    for folder in folders {
        let base = folder.path();
        if location.path().len() > base.len() && location.path()[..base.len()] == base[..] {
            return location.path()[base.len()..].join("/");
        }
    }
    location.path().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_strip_to_plain_text() {
        assert_eq!(strip_snippets("push($0)"), "push()");
        assert_eq!(
            strip_snippets("insert(${1:key}, ${2:value})"),
            "insert(key, value)"
        );
        assert_eq!(strip_snippets("${1}"), "");
        assert_eq!(strip_snippets("cost: $5"), "cost: $5");
        assert_eq!(strip_snippets("a $0 b"), "a  b");
        assert_eq!(strip_snippets("plain"), "plain");
    }
}
