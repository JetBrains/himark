// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Completion: the editor's own popup at the caret, filled by whoever
//! answers its asks — a language server (`CompletionEffect`) or the
//! workspace's files behind an `@` (`PathCompletionEffect`, with the
//! context a host installs as `Mentions`). The anchor is a styled
//! interval in a markup of the editor's, so it follows the text; the
//! popup is the editor's overlay, placed under that anchor.

use std::sync::Arc;

use imba::effect::{CancellationToken, Effects};
use imba::list::{ListCommand, ListOps, ListSlice, ListView, SelectionStyle};
use imba::scroll::{ScrollCommand, ScrollView};
use imba::store::Store;
use imba::thunk_ext::ThunkExt;
use imba::{ui::UiCtx, View};

use crate::document::Document;
use crate::editor::EditorId;
use crate::editor_view::EditorCommand;
use crate::linecol::LineCol;
use crate::location::ResourceLocation;
use crate::markup::{IntervalId, MarkupId};

/// A server's completion at a position: its items, and whether a
/// longer query should ask again rather than filter these.
#[derive(Clone, Debug)]
pub struct Completion {
    pub items: Vec<CompletionItem>,

    pub incomplete: bool,
}

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,

    pub edit: Option<(std::ops::Range<LineCol>, String)>,
    pub insert_text: Option<String>,
}

pub struct CompletionEffect {
    pub location: ResourceLocation,
    pub position: LineCol,
}

impl std::fmt::Display for CompletionEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "completion /{}", self.location.path().join("/"))
    }
}

impl imba::effect::Effect for CompletionEffect {
    type Result = Option<Completion>;
}

/// The files under `folders` whose names carry `term`.
pub struct PathCompletionEffect {
    pub folders: Vec<ResourceLocation>,
    pub term: String,
}

impl std::fmt::Display for PathCompletionEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "path completion {:?}", self.term)
    }
}

impl imba::effect::Effect for PathCompletionEffect {
    type Result = Vec<ResourceLocation>;
}

/// What an `@` completes against: the folders to search and the
/// recent files offered first.
#[derive(Clone)]
pub struct MentionContext {
    pub folders: Arc<Vec<ResourceLocation>>,
    pub recents: Vec<ResourceLocation>,
}

/// The host's answer to "what does `@` mean in this document" — a
/// markdown note's workspace, say. An editor whose owner set a
/// context of its own (a chat composer) never asks.
#[derive(Clone)]
pub struct Mentions(Arc<dyn Fn(&Store, &ResourceLocation) -> Option<MentionContext> + Send + Sync>);

impl Mentions {
    pub fn install(
        store: &mut Store,
        resolve: impl Fn(&Store, &ResourceLocation) -> Option<MentionContext> + Send + Sync + 'static,
    ) {
        store.put(Mentions(Arc::new(resolve)));
    }

    fn resolve(store: &Store, location: &ResourceLocation) -> Option<MentionContext> {
        let resolver = store.get::<Mentions>()?.0.clone();
        resolver(store, location)
    }
}

type PopupList = ScrollView<ListView<CompletionRow, usize>>;
pub type PopupRowsCommand = ScrollCommand<ListCommand<std::convert::Infallible>>;

const SHOWN: usize = 128;
const POPUP_WIDTH: f32 = 560.0;
const VISIBLE_ROWS: usize = 9;

#[derive(Clone)]
pub enum CompletionCommand {
    /// The explicit ask at the caret (ctrl-space).
    Trigger,

    Select(isize),

    PickCursor,

    Rows(PopupRowsCommand),

    Close,

    Found(CompletionFound),
}

impl std::fmt::Display for CompletionCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompletionCommand::Trigger => out.write_str("completion trigger"),
            CompletionCommand::Rows(command) => command.fmt(out),
            CompletionCommand::Select(_) => out.write_str("completion select"),
            CompletionCommand::PickCursor => out.write_str("completion pick"),
            CompletionCommand::Close => out.write_str("completion close"),
            CompletionCommand::Found(_) => out.write_str("completion found"),
        }
    }
}

#[derive(Clone)]
pub enum CompletionFound {
    Path {
        serial: u64,
        locations: Vec<ResourceLocation>,
    },
    Items {
        serial: u64,
        answer: Option<Completion>,
    },
}

/// A file picked from an `@` completion — the owner's signal
/// (`EditorCommand::MentionPicked`).
#[derive(Clone, Debug)]
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
    Items {
        items: Arc<Vec<CompletionItem>>,
        incomplete: bool,

        rows: Arc<Vec<usize>>,
    },
}

impl SourceState {
    fn row_count(&self) -> usize {
        match self {
            SourceState::Path { rows, .. } => rows.len(),
            SourceState::Items { rows, .. } => rows.len(),
        }
    }

    fn empty_path() -> Self {
        SourceState::Path {
            folders: Arc::new(Vec::new()),
            recents: Arc::new(Vec::new()),
            found: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
        }
    }
}

/// One row of the popup.
#[derive(Clone)]
struct CompletionRow {
    label: String,
    trail: Option<String>,
    dim: bool,
}

impl View for CompletionRow {
    type Command = std::convert::Infallible;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        command: Self::Command,
        _fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {}
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(
            move |_arena: &'a imba::arena::Arena, constraints: imba::constraints::Constraints| {
                let theme = crate::env::Themes::of(store);
                let chrome = theme.ui().peeker.clone();
                let height = chrome.row_height.max(1.0);
                let width = constraints.max.width.max(1.0);
                let font = popup_font(ui, chrome.row_size);
                let shaper = imba::layout::TextShaper::of(ui);
                let pad = 12.0;
                let text = match self.dim {
                    true => chrome.dim_text.0,
                    false => chrome.text.0,
                };
                let dim = chrome.dim_text.0;
                imba::leaf::leaf::<Self::Command>(width, height).paint_instead(
                    move |_arena, canvas, rect| {
                        let baseline = rect.top + rect.height() * 0.5 + chrome.row_size * 0.36;
                        let label_width = shaper.draw(
                            canvas,
                            &font,
                            &self.label,
                            text,
                            0.0,
                            rect.left + pad,
                            baseline,
                        );
                        if let Some(trail) = &self.trail {
                            let room = rect.width() - pad * 2.0 - label_width - 16.0;
                            if room > 40.0 {
                                let advance = shaper.advance(&font, trail);
                                let x = rect.right - pad - advance.min(room);
                                canvas.save();
                                canvas.clip_rect(
                                    skia_safe::Rect::from_xywh(x, rect.top, room, rect.height()),
                                    None,
                                    true,
                                );
                                shaper.draw(canvas, &font, trail, dim, 0.0, x, baseline);
                                canvas.restore();
                            }
                        }
                    },
                )
            },
        )
    }
}

fn popup_font(ui: &UiCtx, size: f32) -> skia_safe::Font {
    struct PopupTypeface(skia_safe::Typeface);
    let typeface = ui.env(|| {
        PopupTypeface(
            crate::env::ui_typeface(ui, &[] as &[&str], skia_safe::FontStyle::normal())
                .expect("a ui typeface"),
        )
    });
    let mut font = skia_safe::Font::from_typeface(typeface.0.clone(), size);
    font.set_edging(skia_safe::font::Edging::AntiAlias);
    font
}

fn selection_style(theme: &crate::theme::Theme) -> SelectionStyle {
    let ui = theme.ui();
    SelectionStyle {
        fill: ui.tree.highlight.0,
        accent: ui.peeker.accent.0,
        accent_width: ui.peeker.accent_width,
        accent_inset: 0.0,
        ..SelectionStyle::default()
    }
}

/// The popup: the rows in the peeker's chrome. Keys are the editor's
/// (`focus_data`), clicks the rows' own.
#[derive(Clone)]
pub struct CompletionPopupView {
    list: PopupList,
    rows: usize,
}

impl View for CompletionPopupView {
    type Command = CompletionCommand;

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
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(
            move |_arena: &'a imba::arena::Arena, constraints: imba::constraints::Constraints| {
                let theme = crate::env::Themes::of(store);
                let chrome = theme.ui().peeker.clone();
                let row_height = chrome.row_height.max(1.0);
                let visible = self.rows.min(VISIBLE_ROWS).max(1);
                let width = POPUP_WIDTH.min(constraints.max.width.max(1.0));
                let height =
                    (visible as f32 * row_height + 2.0).min(constraints.max.height.max(row_height));
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
                let rows = imba::layout::Layout::layout(
                    self.list.display(arena, store, ui),
                    arena,
                    imba::constraints::Constraints::tight(skia_safe::Size::new(
                        width - 2.0,
                        height - 2.0,
                    )),
                )
                .map(CompletionCommand::Rows);
                popup.place(1.0, 1.0, rows);
                popup
            },
        )
    }
}

/// One editor's completion: the standing anchor, the query behind
/// it, the source feeding the rows, and the popup's list.
#[derive(Clone)]
pub struct Completer {
    /// The anchor: a styled interval in a markup of the editor's,
    /// moving with the text.
    cover: Option<(MarkupId, IntervalId)>,
    query: String,

    anchor_offset: u32,

    serial: u64,

    lane: Option<CancellationToken>,
    list: PopupList,
    source: SourceState,
}

impl Default for Completer {
    fn default() -> Self {
        Self {
            cover: None,
            query: String::new(),
            anchor_offset: 0,
            serial: 0,
            lane: None,
            list: ScrollView::new(ListView::empty()),
            source: SourceState::empty_path(),
        }
    }
}

impl Completer {
    pub fn open(&self) -> bool {
        self.cover.is_some()
    }

    pub fn selected(&self) -> usize {
        self.list.content().cursor().copied().unwrap_or(0)
    }

    #[doc(hidden)]
    pub fn row_labels(&self) -> Vec<String> {
        match &self.source {
            SourceState::Path { rows, .. } => rows
                .iter()
                .map(|location| location.name().to_owned())
                .collect(),
            SourceState::Items { items, rows, .. } => rows
                .iter()
                .filter_map(|index| items.get(*index))
                .map(|item| item.label.clone())
                .collect(),
        }
    }

    pub(crate) fn view(&self) -> CompletionPopupView {
        CompletionPopupView {
            list: self.list.clone(),
            rows: self.source.row_count(),
        }
    }

    /// Where the anchor stands now.
    pub(crate) fn anchor_start(&self, document: &Document) -> Option<u32> {
        let (markup, key) = self.cover?;
        document
            .feature_markup(markup)?
            .styled_range_of(key)
            .map(|range| range.start)
    }

    fn query_anchor(&self, document: &Document) -> Option<u32> {
        Some(self.anchor_start(document)? + self.anchor_offset)
    }

    /// The `@` road: a fresh `@` at a word boundary opens, a standing
    /// popup follows the query behind its `@`.
    fn sync_path(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        document: &mut Document,
        editor: EditorId,
        typed_at: Option<u32>,
        context: impl FnOnce() -> Option<MentionContext>,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        if self.open() {
            let caret = document.caret_byte(editor);
            let Some(at) = self.anchor_start(document) else {
                self.drop_state(document, store, ui, fx);
                return;
            };
            let mut view = document.text().view();
            let end = view.byte_count() as u32;
            let still_at = at < end && view.substring(at..(at + 1).min(end)) == "@";
            let query = (still_at && caret > at)
                .then(|| view.substring(at + 1..caret.min(end)))
                .filter(|query| !query.chars().any(|c| c.is_whitespace() || c == '@'));
            let Some(query) = query else {
                self.drop_state(document, store, ui, fx);
                return;
            };
            if query != self.query {
                self.query = query;
                self.launch_path(fx);
                self.refresh(store, ui);
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
        let Some(context) = context() else {
            return;
        };
        self.source = SourceState::Path {
            folders: context.folders,
            recents: Arc::new(context.recents),
            found: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
        };
        self.query = String::new();
        self.open_cover(store, ui, document, editor, at, 1, fx);
    }

    fn launch_path(&mut self, fx: &mut Effects<'_, EditorCommand>) {
        self.serial += 1;
        let SourceState::Path { folders, found, .. } = &mut self.source else {
            return;
        };
        if folders.is_empty() || self.query.len() < 2 {
            *found = Arc::new(Vec::new());
            return;
        }
        let serial = self.serial;
        let effect = imba::effect::AnyEffect::new(PathCompletionEffect {
            folders: folders.as_ref().clone(),
            term: self.query.clone(),
        })
        .map(move |locations| {
            EditorCommand::Completion(CompletionCommand::Found(CompletionFound::Path {
                serial,
                locations,
            }))
        });
        fx.relaunch_erased(&mut self.lane, effect);
    }

    /// The server road: a trigger character or an identifier's first
    /// letter opens, the explicit ask opens anywhere, a standing
    /// popup follows the word behind the caret.
    #[allow(clippy::too_many_arguments)]
    fn sync_items(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        document: &mut Document,
        editor: EditorId,
        typed: Option<&str>,
        explicit: bool,
        location: &ResourceLocation,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let caret = document.caret_byte(editor);
        if self.open() {
            let Some(anchor) = self.query_anchor(document) else {
                self.drop_state(document, store, ui, fx);
                return;
            };
            let mut view = document.text().view();
            let end = view.byte_count() as u32;
            let query = (caret >= anchor)
                .then(|| view.substring(anchor..caret.min(end)))
                .filter(|query| query.chars().all(identifier_char));
            let Some(query) = query else {
                self.drop_state(document, store, ui, fx);
                return;
            };
            if query != self.query {
                let grew = query.len() > self.query.len() && query.starts_with(&self.query);
                self.query = query;
                let incomplete = matches!(
                    &self.source,
                    SourceState::Items {
                        incomplete: true,
                        ..
                    }
                );
                if !(grew && !incomplete) {
                    self.launch_items(document, editor, location, fx);
                }
                self.refresh(store, ui);
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
        self.source = SourceState::Items {
            items: Arc::new(Vec::new()),
            incomplete: false,
            rows: Arc::new(Vec::new()),
        };
        let mut view = document.text().view();
        self.query = view.substring(cover + offset..caret);
        self.open_cover(store, ui, document, editor, cover, offset, fx);
        self.launch_items(document, editor, location, fx);
    }

    fn launch_items(
        &mut self,
        document: &Document,
        editor: EditorId,
        location: &ResourceLocation,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        self.serial += 1;
        let serial = self.serial;
        let caret = document.caret_byte(editor) as usize;
        let mut view = document.text().view();
        let position = crate::linecol::line_col_at(&mut view, caret);
        if std::env::var_os("HIMARK_TRACE_LSP").is_some() {
            eprintln!("[lsp] completion ask #{serial} at {location:?} {position:?}");
        }
        let effect = imba::effect::AnyEffect::new(CompletionEffect {
            location: location.clone(),
            position,
        })
        .map(move |answer| {
            EditorCommand::Completion(CompletionCommand::Found(CompletionFound::Items {
                serial,
                answer,
            }))
        });
        fx.relaunch_erased(&mut self.lane, effect);
    }

    #[allow(clippy::too_many_arguments)]
    fn open_cover(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        document: &mut Document,
        editor: EditorId,
        cover: u32,
        anchor_offset: u32,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let markup = document.add_markup();
        document.show_markup(editor, markup);
        let fonts = crate::env::ui_collection(store, ui);
        let theme = crate::env::Themes::of(store);
        let mut tints = crate::markup::Markup::new();
        let key = tints.push_styled_keyed(cover..cover + 1, crate::theme::StyleId::Match);
        document.replace_markup(
            markup,
            tints,
            &[cover..cover + 1],
            store,
            ui,
            &fonts,
            &theme,
            fx,
        );
        self.cover = Some((markup, key));
        self.anchor_offset = anchor_offset;
        self.refresh(store, ui);
    }

    fn land(&mut self, store: &Store, ui: &UiCtx, found: CompletionFound) {
        if std::env::var_os("HIMARK_TRACE_LSP").is_some() {
            let (serial, count) = match &found {
                CompletionFound::Path { serial, locations } => (*serial, locations.len()),
                CompletionFound::Items { serial, answer } => (
                    *serial,
                    answer.as_ref().map_or(0, |answer| answer.items.len()),
                ),
            };
            eprintln!(
                "[lsp] completion landed #{serial} ({count} items) open={} standing=#{}",
                self.open(),
                self.serial
            );
        }
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
                CompletionFound::Items { serial, answer },
                SourceState::Items {
                    items, incomplete, ..
                },
            ) if serial == self.serial => {
                let answer = answer.unwrap_or(Completion {
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
        self.refresh(store, ui);
    }

    fn select(&mut self, delta: isize) {
        let count = self.source.row_count();
        if count == 0 {
            return;
        }
        let last = count as isize - 1;
        let next = (self.selected() as isize + delta).clamp(0, last) as usize;
        self.list.content_mut().select_only(next);
    }

    /// A rows command: a row body click picks (the answer), anything
    /// else moves the list.
    fn rows_command(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: PopupRowsCommand,
    ) -> Option<usize> {
        if let Some((row, _trigger)) = PopupList::activated(&command) {
            if self.list.content().key_at(row).is_some() {
                return Some(row);
            }
        }
        if matches!(command, ScrollCommand::SetScrollY(_)) {
            self.list.content_mut().cancel_reveal();
        }
        let mut discarded = imba::effect::Batch::new();
        self.list
            .perform(store, ui, command, &mut discarded.effects());
        None
    }

    fn apply_pick(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        document: &mut Document,
        editor: EditorId,
        row: usize,
        fx: &mut Effects<'_, EditorCommand>,
    ) -> Option<PickedFile> {
        let start = self.anchor_start(document)?;
        let anchor = start + self.anchor_offset;
        let caret = document.caret_byte(editor);
        let picked = match &self.source {
            SourceState::Path { folders, rows, .. } => {
                let location = rows.get(row).cloned()?;
                let rel = rel_path(folders, &location);
                let inserted = format!("@{rel} ");
                let end = caret.max(start + 1);
                write_replace(store, ui, document, editor, start..end, &inserted, fx);
                Some(PickedFile {
                    label: location.name().to_owned(),
                    rel,
                    location,
                })
            }
            SourceState::Items { items, rows, .. } => {
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
                        let start = crate::linecol::offset_at(&mut view, range.start) as u32;
                        let end = crate::linecol::offset_at(&mut view, range.end) as u32;
                        (start <= anchor && end >= caret.min(end.max(caret)) && start <= end)
                            .then_some(start..end.max(caret))
                    })
                    .unwrap_or(anchor..caret.max(anchor));
                write_replace(store, ui, document, editor, range, &inserted, fx);
                None
            }
        };
        self.drop_state(document, store, ui, fx);
        picked
    }

    fn drop_state(
        &mut self,
        document: &mut Document,
        store: &Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        if let Some(token) = self.lane.take() {
            fx.cancel(token);
        }
        if let Some((markup, _)) = self.cover.take() {
            let fonts = crate::env::ui_collection(store, ui);
            let theme = crate::env::Themes::of(store);
            document.remove_markup(markup, &[], store, ui, &fonts, &theme, fx);
        }
        *self = Self::default();
    }

    fn refresh(&mut self, store: &Store, ui: &UiCtx) {
        let _ = ui;
        let query = self.query.to_lowercase();
        let mut rows_out: Vec<CompletionRow> = Vec::new();
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
                    rows_out.push(CompletionRow {
                        label: location.name().to_owned(),
                        trail: Some(rel_path(folders, location)),
                        dim: false,
                    });
                    fresh.push(location.clone());
                };
                for location in recents.iter() {
                    if imba::list::subsequence_match(&location.name().to_lowercase(), &query) {
                        push(location);
                    }
                }
                for location in found.iter() {
                    push(location);
                }
                *rows = Arc::new(fresh);
            }
            SourceState::Items { items, rows, .. } => {
                let mut fresh = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    if fresh.len() >= SHOWN {
                        hidden += 1;
                        continue;
                    }
                    let haystack = item.filter_text.as_deref().unwrap_or(&item.label);
                    if !imba::list::subsequence_match(&haystack.to_lowercase(), &query) {
                        continue;
                    }
                    rows_out.push(CompletionRow {
                        label: item.label.clone(),
                        trail: item.detail.clone(),
                        dim: false,
                    });
                    fresh.push(index);
                }
                *rows = Arc::new(fresh);
            }
        }
        let theme = crate::env::Themes::of(store);
        let row_height = theme.ui().peeker.row_height.max(1.0);
        let selected = self
            .selected()
            .min(self.source.row_count().saturating_sub(1));
        let scroll_y = self.list.scroll_y();
        let mut slice = ListSlice::new();
        let count = rows_out.len();
        for (index, row) in rows_out.into_iter().enumerate() {
            slice.push_keyed_sized(index, row, row_height);
        }
        if hidden > 0 {
            slice.push_sized(
                CompletionRow {
                    label: format!("… {hidden} more — narrow the filter"),
                    trail: None,
                    dim: true,
                },
                row_height,
            );
        }
        let mut list = ListView::from_slice(slice).with_selection(selection_style(&theme));
        if count > 0 {
            list.select_only(selected);
        }
        self.list = ScrollView::new(list);
        self.list.set_scroll_y(scroll_y);
    }
}

#[allow(clippy::too_many_arguments)]
fn write_replace(
    store: &Store,
    ui: &UiCtx,
    document: &mut Document,
    editor: EditorId,
    range: std::ops::Range<u32>,
    inserted: &str,
    fx: &mut Effects<'_, EditorCommand>,
) {
    let mut view = document.text().view();
    let total = view.byte_count() as u32;
    let range = range.start.min(total)..range.end.min(total);
    let removed = view.substring(range.clone());
    let mut builder = operation::builder::OperationBuilder::new();
    builder.push_retain(range.start);
    builder.push_delete(removed);
    builder.push_insert(inserted.to_owned());
    builder.push_retain(total - range.end);
    let operation = builder.finish();
    let fonts = crate::env::ui_collection(store, ui);
    let theme = crate::env::Themes::of(store);
    document.edit(&operation, store, ui, &fonts, &theme, fx);
    document.set_caret(editor, range.start + inserted.len() as u32);
}

impl Document {
    pub fn completion(&self, editor: EditorId) -> &Completer {
        &self.editor(editor).completion
    }

    /// An `@` context of this editor's own (a chat composer): `@`
    /// completes against it without asking the host.
    pub fn set_mentions(&mut self, editor: EditorId, context: Option<MentionContext>) {
        self.editor_mut(editor).mentions = context.map(Arc::new);
    }

    /// After a command: a standing popup follows the caret, a typed
    /// character may open one.
    pub(crate) fn completion_sync(
        &mut self,
        editor: EditorId,
        location: Option<&ResourceLocation>,
        typed: Option<&str>,
        explicit: bool,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        let mut completer = std::mem::take(&mut self.editor_mut(editor).completion);
        if !completer.open() && typed.is_none() && !explicit {
            self.editor_mut(editor).completion = completer;
            return;
        }
        let own = self.editor(editor).mentions.clone();
        let markdown = self.syntax().map(|syntax| syntax.language.as_str()) == Some("markdown");
        let mentions = own.is_some() || markdown;
        match location {
            _ if mentions && !explicit => {
                let typed_at =
                    (typed == Some("@")).then(|| self.caret_byte(editor).saturating_sub(1));
                let context = || match (own, location) {
                    (Some(context), _) => Some(context.as_ref().clone()),
                    (None, Some(location)) => Mentions::resolve(store, location),
                    (None, None) => None,
                };
                completer.sync_path(store, ui, self, editor, typed_at, context, fx);
            }
            Some(location) if !location.is_synthetic() && !markdown => {
                completer.sync_items(store, ui, self, editor, typed, explicit, location, fx);
            }
            _ if completer.open() => completer.drop_state(self, store, ui, fx),
            _ => {}
        }
        self.editor_mut(editor).completion = completer;
    }

    pub(crate) fn completion_perform(
        &mut self,
        editor: EditorId,
        location: Option<&ResourceLocation>,
        command: CompletionCommand,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut Effects<'_, EditorCommand>,
    ) {
        if let CompletionCommand::Trigger = command {
            return self.completion_sync(editor, location, None, true, store, ui, fx);
        }
        let mut completer = std::mem::take(&mut self.editor_mut(editor).completion);
        if std::env::var_os("HIMARK_TRACE_LSP").is_some() {
            eprintln!(
                "[lsp] completion {command} on {editor:?} open={}",
                completer.open()
            );
        }
        match command {
            CompletionCommand::Trigger => unreachable!("handled above"),
            CompletionCommand::Select(delta) => completer.select(delta),
            CompletionCommand::PickCursor => {
                let row = completer.selected();
                if let Some(pick) = completer.apply_pick(store, ui, self, editor, row, fx) {
                    fx.follow_up(EditorCommand::MentionPicked(pick));
                }
            }
            CompletionCommand::Rows(rows) => {
                if let Some(row) = completer.rows_command(store, ui, rows) {
                    if let Some(pick) = completer.apply_pick(store, ui, self, editor, row, fx) {
                        fx.follow_up(EditorCommand::MentionPicked(pick));
                    }
                }
            }
            CompletionCommand::Close => completer.drop_state(self, store, ui, fx),
            CompletionCommand::Found(found) => completer.land(store, ui, found),
        }
        self.editor_mut(editor).completion = completer;
    }
}

fn identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_start(view: &mut ::text::text_view::TextView, caret: u32) -> u32 {
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
