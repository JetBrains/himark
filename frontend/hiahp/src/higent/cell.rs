// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::higent::tool_group::{
    ToolCallSpec, ToolGroup, ToolRowCommand, ToolRowKey, ToolRowsCommand, ToolUpdate,
};
use editor::{env, EditorCommand, EditorView};
use hikit::TreeItemCommand;
use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult},
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View, Widget,
};
use skia_safe::{Paint, Rect, Size};

type ChatChrome = editor::theme::ChatChrome;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CellKind {
    User,
    Agent,
    Reasoning,
    Tool,
    Notice,
    Error,
}

// Per thread: a cell is mounted where it is displayed, and a process-wide
// counter would mix one window's (or one test's) work into another's.
thread_local! {
    static MOUNTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn note_mount() {
    MOUNTED.with(|mounted| mounted.set(mounted.get() + 1));
}

#[derive(Clone)]
pub enum CellCommand {
    Editor(EditorCommand),

    Rewrite(editor::Text),

    Rewrap(f32),

    ResolveDiff(Result<crate::higent::BuiltFileEdit, String>),

    /// The off-thread markdown build landed, for the cell whose
    /// `pending_nonce` this is.
    ResolveText {
        nonce: u64,
        built: documents::BuiltDocument,
    },

    Diff(editor::UnifiedDiffCommand),

    Tool(ToolUpdate),

    ToolRows(ToolRowsCommand),

    ToolRow {
        key: ToolRowKey,
        command: TreeItemCommand<ToolRowCommand>,
    },

    Append(String),

    /// The diff header's OPEN: navigate to the WORKING COPY of the
    /// edited file. The uri is the wire's (`file:///…`); the panel
    /// resolves it against the session's client and opens the location.
    OpenFile(ahp_types::common::Uri),
}

impl std::fmt::Display for CellCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CellCommand::Editor(command) => command.fmt(out),
            CellCommand::Diff(command) => command.fmt(out),
            CellCommand::ToolRows(command) => command.fmt(out),
            CellCommand::ToolRow { command, .. } => command.fmt(out),
            CellCommand::Rewrite(_) => out.write_str("cell rewrite"),
            CellCommand::Rewrap(_) => out.write_str("cell rewrap"),
            CellCommand::ResolveDiff(_) => out.write_str("resolve diff"),
            CellCommand::ResolveText { .. } => out.write_str("resolve text"),
            CellCommand::Tool(_) => out.write_str("tool update"),
            CellCommand::Append(_) => out.write_str("cell append"),
            CellCommand::OpenFile(_) => out.write_str("open file"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct DiffHeader {
    pub title: String,
    pub added: Option<i64>,
    pub removed: Option<i64>,
    /// The edited file's own uri — the working copy the OPEN action
    /// navigates to.
    pub uri: Option<ahp_types::common::Uri>,
}

#[derive(Clone)]
enum CellBody {
    Markdown(EditorView),

    /// A markdown cell whose DOCUMENT is being built off the UI thread
    /// (`BuildDocumentEffect` — the text and the parse in the handler,
    /// the editor mounted at the landing). Streamed deltas land in the
    /// buffer meanwhile; `grown` says how the landing catches up.
    PendingText {
        markdown: String,
        width: f32,
        /// The band's height while the build runs: what the row was
        /// sized at, so the first paint does not collapse it.
        estimate: f32,
        /// Which build this cell waits for. A re-laid cell under the
        /// same key must not mount an older build's document.
        nonce: u64,
        grown: Grown,
    },

    PendingDiff {
        header: DiffHeader,

        width: f32,
    },

    /// A proper INLINE diff face over the edit — the editor crate's
    /// unified view: hunk washes, word tints, deleted lines as
    /// before-inlays and unchanged context collapsed behind FOLD
    /// strips. Seeded settled (`prepare_marks` over the whole texts —
    /// chat edits are small), so no marks lane is owed at birth.
    Diff {
        header: DiffHeader,
        view: editor::UnifiedDiffView,
    },

    Tools(ToolGroup),
}

/// What streamed into a pending cell while its build ran.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Grown {
    Nothing,
    /// Deltas appended past the text the build was launched with — the
    /// landing appends the suffix, an edit the size of the deltas.
    Appended {
        built: usize,
    },
    /// The text was replaced outright (a re-dressed part).
    Rewritten,
}

#[derive(Clone)]
pub struct Cell {
    kind: CellKind,
    body: CellBody,
}

fn mint_nonce() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn markdown_document(text: editor::Text) -> editor::Document {
    editor::Document::new(text, editor::Markup::new()).with_syntax(
        editor::Syntax::new("markdown", None, editor::Markup::new()),
        &[],
    )
}

/// One side of an edit as a document with its syntax. The parsers are
/// HANDED IN: this runs in the workshop's bare build store (off the UI
/// thread), which knows no languages.
pub(crate) fn side_document(
    text: editor::Text,
    extension: &str,
    parsers: &std::sync::Arc<editor::SyntaxLanguages>,
    store: &Store,
    ui: &imba::UiCtx,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &editor::Theme,
) -> editor::Document {
    let language = if !extension.is_empty() && parsers.knows(extension) {
        extension
    } else {
        "markdown"
    };
    // A registry that knows no markdown either (a bare unit-test store)
    // still gets a document — plain, with markdown's own markup.
    if !parsers.knows(language) {
        return markdown_document(text);
    }
    editor::Document::from_language(text, language, parsers, store, ui, fonts, theme)
}

pub(crate) fn document_text(document: &editor::Document) -> String {
    let end = document.text().byte_count().min(u32::MAX as usize) as u32;
    document.text().view().substring(0..end)
}

fn cell_surface(kind: CellKind, chrome: &ChatChrome) -> Option<skia_safe::Color> {
    match kind {
        CellKind::User => None,
        CellKind::Reasoning => Some(chrome.thought_surface.0),
        CellKind::Tool => Some(chrome.tool_surface.0),
        CellKind::Error => Some(chrome.error_surface.0),
        CellKind::Agent | CellKind::Notice => None,
    }
}

impl Cell {
    pub(crate) fn build(
        store: &Store,
        ui: &UiCtx,
        kind: CellKind,
        markdown: &str,
        content_width: f32,
        fx: &mut Effects<'_, EditorCommand>,
    ) -> (Self, f32) {
        Self::build_text(
            store,
            ui,
            kind,
            editor::Text::from_string_exact(markdown),
            content_width,
            fx,
        )
    }

    /// How many cells this thread has MOUNTED editors for — a markdown
    /// cell's one, a diff cell's pair — whether the document came built
    /// off-thread (`resolve_text`, `resolve_diff`) or was built in place
    /// (`build_text`). The perf contract reads it: a streamed turn
    /// mounts a cell per PART, never per delta and never a whole turn
    /// again, and walking back to a chat mounts nothing at all.
    #[doc(hidden)]
    pub fn documents_mounted() -> u64 {
        MOUNTED.with(std::cell::Cell::get)
    }

    /// A markdown cell at BIRTH: no document, no parse, no layout — a
    /// band at an estimated height while the build runs off-thread.
    pub(crate) fn pending_text(
        store: &Store,
        kind: CellKind,
        markdown: &str,
        content_width: f32,
    ) -> (Self, f32) {
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let width = Self::editor_width(kind, &chrome, content_width);
        let lines = markdown.lines().count().clamp(1, 40) as f32;
        let estimate = (lines * chrome.title_size * 1.5).max(chrome.min_cell_height);
        (
            Self {
                kind,
                body: CellBody::PendingText {
                    markdown: markdown.to_owned(),
                    width,
                    estimate,
                    nonce: mint_nonce(),
                    grown: Grown::Nothing,
                },
            },
            estimate + chrome.pad * 2.0 + chrome.gap,
        )
    }

    /// The build a pending cell waits for — the landing carries it back.
    pub(crate) fn pending_nonce(&self) -> Option<u64> {
        match &self.body {
            CellBody::PendingText { nonce, .. } => Some(*nonce),
            _ => None,
        }
    }

    pub(crate) fn build_text(
        store: &Store,
        ui: &UiCtx,
        kind: CellKind,
        text: editor::Text,
        content_width: f32,
        fx: &mut Effects<'_, EditorCommand>,
    ) -> (Self, f32) {
        note_mount();
        let fonts = env::ui_collection(store, ui);
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let editor_width = Self::editor_width(kind, &chrome, content_width);
        let mut document = markdown_document(text);
        let editor = document.add_editor(
            editor_width,
            None,
            ::editor::EditorBuild::Bounded,
            &[],
            store,
            ui,
            &fonts,
            &theme,
            fx,
        );
        if let Some(parsers) = env::Parsers::of(store) {
            document.launch_reparse(parsers, fx);
        }
        let height = document.content_height(editor).max(chrome.min_cell_height)
            + chrome.pad * 2.0
            + chrome.gap;
        (
            Self {
                kind,
                body: CellBody::Markdown(EditorView {
                    document,
                    editor,
                    reports_geometry: false,
                    location: None,
                    gutter_width: 0.0,
                    base: None,
                }),
            },
            height,
        )
    }

    pub(crate) fn pending_diff(
        store: &Store,
        header: DiffHeader,
        content_width: f32,
    ) -> (Self, f32) {
        let chrome = env::Themes::of(store).ui().chat.clone();
        let height = Self::header_band(&chrome) + chrome.pad * 2.0 + chrome.gap;
        (
            Self {
                kind: CellKind::Tool,
                body: CellBody::PendingDiff {
                    header,
                    width: content_width,
                },
            },
            height,
        )
    }

    pub(crate) fn tools(
        store: &Store,
        ui: &UiCtx,
        specs: Vec<ToolCallSpec>,
        content_width: f32,
    ) -> (Self, f32) {
        let chrome = env::Themes::of(store).ui().chat.clone();
        let (group, rows_height) = ToolGroup::new(
            store,
            ui,
            specs,
            (content_width - chrome.pad * 2.0).max(120.0),
        );
        let height = rows_height + chrome.pad * 2.0 + chrome.gap;
        (
            Self {
                kind: CellKind::Tool,
                body: CellBody::Tools(group),
            },
            height,
        )
    }

    fn header_band(chrome: &ChatChrome) -> f32 {
        chrome.title_size * 1.8
    }

    /// The built document lands: mount its editor at this cell's width
    /// and replay whatever streamed in while the build ran.
    fn resolve_text(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        nonce: u64,
        built: documents::BuiltDocument,
        fx: &mut Effects<'_, CellCommand>,
    ) {
        let CellBody::PendingText {
            markdown,
            width,
            nonce: waited,
            grown,
            ..
        } = &mut self.body
        else {
            return;
        };
        if *waited != nonce {
            return;
        }
        let (markdown, width, grown) = (std::mem::take(markdown), *width, *grown);
        note_mount();
        let fonts = env::ui_collection(store, ui);
        let theme = env::Themes::of(store);
        let mut document = built.document;
        let editor = fx.scope(CellCommand::Editor, |fx| {
            document.add_editor(
                width,
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
        self.body = CellBody::Markdown(EditorView {
            document,
            editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        });
        // The stream did not wait for the build: catch the cell up.
        match grown {
            Grown::Nothing => {}
            Grown::Appended { built } => {
                let suffix = markdown[built..].to_owned();
                self.perform(store, ui, CellCommand::Append(suffix), fx);
            }
            Grown::Rewritten => {
                let text = editor::Text::from_string_exact(&markdown);
                self.perform(store, ui, CellCommand::Rewrite(text), fx);
            }
        }
    }

    /// Mount a pair that was BUILT off-thread: two documents with their
    /// syntax, the diff installed, the marks prepared. All that is left
    /// here is layout at this cell's width — no Text, no parse, no diff.
    fn resolve_diff(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        result: Result<crate::higent::BuiltFileEdit, String>,
        fx: &mut Effects<'_, CellCommand>,
    ) {
        let CellBody::PendingDiff { header, width } = self.body.clone() else {
            return;
        };
        let fonts = env::ui_collection(store, ui);
        let theme = env::Themes::of(store);
        match result {
            Err(error) => {
                let markdown = format!("**{}** — contents unavailable: {error}", header.title);
                let (cell, _) = fx.scope(CellCommand::Editor, |fx| {
                    Cell::build(store, ui, CellKind::Error, &markdown, width, fx)
                });
                *self = cell;
            }
            Ok(built) => {
                note_mount();
                let crate::higent::BuiltFileEdit {
                    before: mut before_doc,
                    after: mut after_doc,
                    diff: diff_id,
                    hunks,
                    left_marks,
                    prepared,
                } = built;

                let gutter = theme.ui().editor_gutter.width;
                // Diff bodies run edge-to-edge — the fold strips and
                // washes bump the chat's edges exactly.
                let editor_width = (width - gutter).max(120.0);
                let mut throwaway = imba::effect::Batch::new();
                let quiet = &mut throwaway.effects();

                let left_editor = before_doc.add_editor(
                    editor_width,
                    None,
                    ::editor::EditorBuild::Bounded,
                    &[left_marks],
                    store,
                    ui,
                    &fonts,
                    &theme,
                    quiet,
                );
                before_doc.manage_repairs_in_pair(left_editor);

                let right_editor = after_doc.add_editor(
                    editor_width,
                    None,
                    ::editor::EditorBuild::Bounded,
                    &[hunks],
                    store,
                    ui,
                    &fonts,
                    &theme,
                    quiet,
                );
                after_doc.manage_repairs_in_pair(right_editor);
                let right_extras = after_doc.add_owned_markup(right_editor);
                after_doc.replace_markup(
                    right_extras,
                    prepared.right.clone(),
                    &[],
                    store,
                    ui,
                    &fonts,
                    &theme,
                    quiet,
                );

                let state = editor::DiffViewState::attach(
                    diff_id,
                    &before_doc,
                    &after_doc,
                    left_marks,
                    right_extras,
                    Some(prepared.window),
                )
                .expect("the entry was just installed");
                let left = EditorView {
                    document: before_doc,
                    editor: left_editor,
                    reports_geometry: false,
                    location: None,
                    gutter_width: 0.0,
                    base: None,
                };
                let right = EditorView {
                    document: after_doc,
                    editor: right_editor,
                    reports_geometry: false,
                    location: None,
                    gutter_width: 0.0,
                    base: None,
                };
                let mut view =
                    editor::UnifiedDiffView::new(editor::SplitDiffView::new(left, right, state));
                fx.scope(CellCommand::Diff, |fx| {
                    view.perform(
                        store,
                        ui,
                        editor::UnifiedDiffCommand::SetLayout(editor::DiffLayout::Inline),
                        fx,
                    )
                });
                self.body = CellBody::Diff { header, view };
            }
        }
    }

    pub(crate) fn oracle(&self) -> (String, String) {
        let kind = format!("{:?}", self.kind);
        match &self.body {
            CellBody::Markdown(editor) => (kind, document_text(&editor.document)),
            CellBody::PendingText { markdown, .. } => (kind, markdown.clone()),
            CellBody::PendingDiff { header, .. } => {
                (kind, format!("[diff {} pending]", header.title))
            }
            CellBody::Diff { header, view } => (
                kind,
                format!(
                    "[diff {} +{} -{}]\n{}",
                    header.title,
                    header.added.unwrap_or(0),
                    header.removed.unwrap_or(0),
                    document_text(&view.split.right.document)
                ),
            ),
            CellBody::Tools(group) => (kind, group.oracle().join("\n")),
        }
    }

    fn editor_view(&self) -> Option<&EditorView> {
        match &self.body {
            CellBody::Markdown(view) => Some(view),
            CellBody::Tools(_)
            | CellBody::PendingText { .. }
            | CellBody::PendingDiff { .. }
            | CellBody::Diff { .. } => None,
        }
    }

    fn editor_view_mut(&mut self) -> Option<&mut EditorView> {
        match &mut self.body {
            CellBody::Markdown(editor) => Some(editor),
            CellBody::Diff { .. }
            | CellBody::PendingText { .. }
            | CellBody::PendingDiff { .. }
            | CellBody::Tools(_) => None,
        }
    }

    fn card_geometry(kind: CellKind, _chrome: &ChatChrome, width: f32) -> (f32, f32) {
        match kind {
            _ => (0.0, width),
        }
    }

    fn editor_width(kind: CellKind, chrome: &ChatChrome, width: f32) -> f32 {
        let (_, card) = Self::card_geometry(kind, chrome, width);
        (card - chrome.pad * 2.0).max(120.0)
    }
}

impl View for Cell {
    type Command = CellCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, CellCommand> {
        match &self.body {
            CellBody::Diff { view, .. } => view.focus_data(store, ui).map(CellCommand::Diff),
            _ => match self.editor_view() {
                Some(editor) => editor.focus_data(store, ui).map(CellCommand::Editor),
                None => imba::focus::FocusData::default(),
            },
        }
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        if let CellBody::Tools(group) = &mut self.body {
            group.destroy(store, fx);
            return;
        }
        if let CellBody::Diff { view, .. } = &mut self.body {
            fx.scope(CellCommand::Diff, |fx| view.destroy(store, fx));
            return;
        }
        if let Some(editor) = self.editor_view_mut() {
            fx.scope(CellCommand::Editor, |fx| editor.destroy(store, fx));
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            CellCommand::Editor(command) => {
                let landing = matches!(
                    command,
                    EditorCommand::ApplyRepair(_)
                        | EditorCommand::ApplyReparse(_)
                        | EditorCommand::ApplyEnrichment(_)
                        | EditorCommand::Retheme { .. }
                        | EditorCommand::Viewport { .. }
                );
                let Some(editor) = self.editor_view_mut() else {
                    return;
                };

                if !landing {
                    editor.focus_text();
                }
                fx.scope(CellCommand::Editor, |fx| {
                    View::perform(editor, store, ui, command, fx)
                });
            }
            CellCommand::Rewrite(text) => {
                if let CellBody::PendingText {
                    markdown, grown, ..
                } = &mut self.body
                {
                    *markdown = text.to_string();
                    *grown = Grown::Rewritten;
                    return;
                }
                let CellBody::Markdown(editor) = &mut self.body else {
                    return;
                };

                if *editor.document.text() == text {
                    return;
                }

                // A patch, not a picture: the minimal exact edit
                // (docs/editor/structural-diff.md, decision 3).
                let operation = myersdiff::diff(editor.document.text(), &text);
                let fonts = env::ui_collection(store, ui);
                let theme = env::Themes::of(store);
                fx.scope(CellCommand::Editor, |fx| {
                    editor
                        .document
                        .edit(&operation, store, ui, &fonts, &theme, fx);
                    if let Some(parsers) = env::Parsers::of(store) {
                        editor.document.launch_reparse(parsers, fx);
                    }
                });
            }
            CellCommand::Diff(command) => {
                let CellBody::Diff { view, .. } = &mut self.body else {
                    return;
                };
                fx.scope(CellCommand::Diff, |fx| view.perform(store, ui, command, fx));
            }
            CellCommand::Rewrap(width) => {
                // Not laid yet: the landing lays at the new width.
                if let CellBody::PendingText { width: held, .. } = &mut self.body {
                    *held = width;
                    return;
                }
                let fonts = env::ui_collection(store, ui);
                let theme = env::Themes::of(store);
                if let CellBody::Diff { view, .. } = &mut self.body {
                    // All three faces resize together: the halves stay
                    // width-matched (the pair lane insists) and the
                    // inline editor mirrors them.
                    let left_editor = view.split.left.editor;
                    let right_editor = view.split.right.editor;
                    let inline = view.inline_editor;
                    fx.scope(
                        |command: EditorCommand| {
                            CellCommand::Diff(editor::UnifiedDiffCommand::Split(
                                editor::SplitDiffCommand::Left(command),
                            ))
                        },
                        |fx| {
                            view.split.left.document.resize(
                                left_editor,
                                width,
                                0,
                                store,
                                ui,
                                &fonts,
                                &theme,
                                fx,
                            )
                        },
                    );
                    fx.scope(
                        |command: EditorCommand| {
                            CellCommand::Diff(editor::UnifiedDiffCommand::Split(
                                editor::SplitDiffCommand::Right(command),
                            ))
                        },
                        |fx| {
                            view.split.right.document.resize(
                                right_editor,
                                width,
                                0,
                                store,
                                ui,
                                &fonts,
                                &theme,
                                fx,
                            )
                        },
                    );
                    if let Some(inline) = inline {
                        fx.scope(
                            |command: EditorCommand| {
                                CellCommand::Diff(editor::UnifiedDiffCommand::Inline(command))
                            },
                            |fx| {
                                view.split
                                    .right
                                    .document
                                    .resize(inline, width, 0, store, ui, &fonts, &theme, fx)
                            },
                        );
                    }
                    fx.scope(CellCommand::Diff, |fx| {
                        view.perform(
                            store,
                            ui,
                            editor::UnifiedDiffCommand::Split(editor::SplitDiffCommand::Resync),
                            fx,
                        )
                    });
                    return;
                }
                let Some(editor) = self.editor_view_mut() else {
                    return;
                };
                let id = editor.editor;
                fx.scope(CellCommand::Editor, |fx| {
                    editor
                        .document
                        .resize(id, width, 0, store, ui, &fonts, &theme, fx);
                });
            }
            CellCommand::Append(chunk) => {
                if let CellBody::PendingText {
                    markdown, grown, ..
                } = &mut self.body
                {
                    if *grown == Grown::Nothing {
                        *grown = Grown::Appended {
                            built: markdown.len(),
                        };
                    }
                    markdown.push_str(&chunk);
                    return;
                }
                let CellBody::Markdown(editor) = &mut self.body else {
                    return;
                };
                let end = editor.document.text().byte_count().min(u32::MAX as usize) as u32;
                let operation = operation::Operation::insert_at(end, chunk);
                let fonts = env::ui_collection(store, ui);
                let theme = env::Themes::of(store);
                fx.scope(CellCommand::Editor, |fx| {
                    editor
                        .document
                        .edit(&operation, store, ui, &fonts, &theme, fx);
                    if let Some(parsers) = env::Parsers::of(store) {
                        editor.document.launch_reparse(parsers, fx);
                    }
                });
            }
            CellCommand::ResolveDiff(result) => self.resolve_diff(store, ui, result, fx),
            CellCommand::ResolveText { nonce, built } => {
                self.resolve_text(store, ui, nonce, built, fx)
            }
            CellCommand::Tool(update) => {
                let CellBody::Tools(group) = &mut self.body else {
                    return;
                };
                group.update(store, ui, update, fx);
            }
            CellCommand::ToolRows(command) => {
                let CellBody::Tools(group) = &mut self.body else {
                    return;
                };
                group.perform(store, ui, command, fx);
            }
            CellCommand::ToolRow { key, command } => {
                let CellBody::Tools(group) = &mut self.body else {
                    return;
                };
                group.perform_keyed(store, ui, key, command, fx);
            }
            // Navigation — the header's click bubbles UP to the panel,
            // which owns the client; nothing lands back on the cell.
            CellCommand::OpenFile(_) => {}
        }
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        CardFrame {
            cell: self,
            store,
            ui,
        }
    }
}

fn chrome_gutter(store: &Store) -> f32 {
    env::Themes::of(store).ui().editor_gutter.width
}

struct CellWidget<Inner> {
    inner: Inner,
    rewrap: Option<f32>,
}

impl<'a, Inner: Widget<'a, CellCommand>> Widget<'a, CellCommand> for CellWidget<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, CellCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<CellCommand> {
        let result = self.inner.handle_event(arena, event, viewport);
        if let (Event::Paint { .. }, Some(width)) = (event, self.rewrap) {
            return result.merge(EventResult::Command(CellCommand::Rewrap(width)));
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, CellCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

#[cfg(test)]
mod tests;

/// The chat cell CARD, reified (docs/ui/UI.md stage 2): an exact-extent
/// spacer pins the card's box (the same arithmetic the hand-rolled
/// container used), a match-parent backdrop paints beneath it, the
/// body sits padded inside, and the kind's adornments — the user
/// bar + caps label, the diff header band — stack on top.
struct CardFrame<'a> {
    cell: &'a Cell,
    store: &'a Store,
    ui: &'a UiCtx,
}

impl imba::LayoutValue for CardFrame<'_> {}

impl<'a> imba::Layout<'a, CellCommand> for CardFrame<'a> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, CellCommand> {
        use imba::LayoutExt as _;
        let CardFrame { cell, store, ui } = self;
        let chrome = env::Themes::of(store).ui().chat.clone();
        let width = constraints.max.width.max(1.0);
        let (_card_x, card_width) = Cell::card_geometry(cell.kind, &chrome, width);

        if let CellBody::Tools(group) = &cell.body {
            let inner_width = (card_width - chrome.pad * 2.0).max(120.0);
            let rows = group
                .layout(
                    arena,
                    store,
                    ui,
                    Constraints {
                        min: Size::new(inner_width, 0.0),
                        max: Size::new(inner_width, f32::MAX),
                    },
                )
                .map(CellCommand::ToolRows);
            return imba::fixed(rows)
                .pad(chrome.pad)
                .backdrop(
                    hikit::ui::Surface {
                        // Fill only — a border per card stacks stray
                        // hairlines through the transcript.
                        fill: cell_surface(CellKind::Tool, &chrome),
                        border: None,
                        radius: chrome.radius,
                    }
                    .painter(),
                )
                .pad_insets(imba::Insets {
                    left: 0.0,
                    top: 0.0,
                    right: 0.0,
                    bottom: chrome.gap,
                })
                .layout(arena, constraints);
        }

        let (header, editor, diff, header_h) = match &cell.body {
            CellBody::Markdown(editor) => (None, Some(editor), None, 0.0),
            CellBody::PendingDiff { header, .. } => {
                (Some((header, true)), None, None, Cell::header_band(&chrome))
            }
            CellBody::Diff { header, view } => (
                Some((header, false)),
                None,
                Some(view),
                Cell::header_band(&chrome),
            ),
            // A cell whose build has not landed: the band stands empty
            // at its estimated height until the document arrives.
            CellBody::PendingText { .. } => (None, None, None, 0.0),
            CellBody::Tools(_) => unreachable!("handled above"),
        };
        let editor_target = match &cell.body {
            CellBody::Diff { .. } => (card_width - chrome_gutter(store)).max(120.0),
            _ => Cell::editor_width(cell.kind, &chrome, width),
        };
        let header_h = match cell.kind {
            CellKind::User => Cell::header_band(&chrome),
            _ => header_h,
        };
        let diff_thunk = diff.map(|view| {
            imba::Layout::layout(
                view.display(arena, store, ui),
                arena,
                Constraints {
                    min: Size::new(editor_target, 0.0),
                    max: Size::new(editor_target + chrome_gutter(store), f32::MAX),
                },
            )
            .map(CellCommand::Diff)
        });
        let pending_height = match &cell.body {
            CellBody::PendingText { estimate, .. } => Some(*estimate),
            _ => None,
        };
        let editor_height = editor
            .map(|editor| editor.content_height().max(chrome.min_cell_height))
            .or_else(|| {
                diff_thunk
                    .as_ref()
                    .map(|thunk| thunk.size().height.max(chrome.min_cell_height))
            })
            .or(pending_height)
            .unwrap_or(0.0);
        let card_height = header_h + editor_height + chrome.pad * 2.0;

        let mut card = imba::ZBox::new(arena);
        // Only the ERROR card keeps a border (it is the signal);
        // bordering every tool/thought card stacked stray hairlines
        // through the transcript.
        let border = match cell.kind {
            CellKind::Error => Some(chrome.stop_color.0),
            CellKind::User | CellKind::Agent | CellKind::Notice => None,
            CellKind::Tool | CellKind::Reasoning => None,
        };
        let surface = cell_surface(cell.kind, &chrome);
        if surface.is_some() || border.is_some() {
            card = card.child_match_parent(
                imba::Fill::new().backdrop(
                    hikit::ui::Surface {
                        fill: surface,
                        border,
                        radius: chrome.radius,
                    }
                    .painter(),
                ),
            );
        }
        card = card.child(imba::spacer(card_width, card_height));
        let content = editor
            .map(|editor| {
                imba::Layout::layout(
                    editor.display(arena, store, ui),
                    arena,
                    Constraints {
                        min: Size::new(editor_target, editor_height),
                        max: Size::new(editor_target + editor.gutter_width, f32::MAX),
                    },
                )
                .map(CellCommand::Editor)
            })
            .map(|thunk| imba::ThunkBox::new(arena, thunk))
            .or(diff_thunk.map(|thunk| imba::ThunkBox::new(arena, thunk)));
        if let Some(content) = content {
            // Normal messages keep their padding; diff bodies own the
            // full card width.
            let body_pad = match &cell.body {
                CellBody::Diff { .. } => 0.0,
                _ => chrome.pad,
            };
            card = card.child(imba::fixed(content).pad_insets(imba::Insets {
                left: body_pad,
                top: header_h + chrome.pad,
                right: 0.0,
                bottom: 0.0,
            }));
        }
        if matches!(cell.kind, CellKind::User) {
            let bar_color = chrome.notice_color.0;
            card = card.child_match_parent(imba::Fill::new().width(3.0).backdrop(
                move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: Rect| {
                    let mut paint = Paint::default();
                    paint.set_color(bar_color);
                    canvas.draw_rect(rect, &paint);
                },
            ));
            let caps = hikit::ui::caps(store, ui).colored(chrome.notice_color.0);
            card = card.child(
                imba::ZBox::new(arena)
                    .child(imba::spacer(0.0, header_h))
                    .child_aligned(imba::Alignment::CenterStart, hikit::ui::text(&caps, "YOU"))
                    .pad_insets(imba::Insets {
                        left: chrome.pad,
                        ..Default::default()
                    }),
            );
        }
        if let Some((header, pending)) = header {
            // The canvas's own file band (diff_header): stats leading,
            // the name right-aligned before the corner-arrow button,
            // the band's body opening the working copy.
            use ::canvas::diff_header::{DiffHeaderFace, DiffHeaderPress, DiffHeaderSpec};
            let title = if pending {
                format!("{} — fetching contents…", header.title)
            } else {
                header.title.clone()
            };
            let size = chrome.title_size * 0.85;
            let face = DiffHeaderFace::new(
                store,
                ui,
                DiffHeaderSpec {
                    title,
                    added: header.added,
                    removed: header.removed,
                    chevron: None,
                    buttons: match header.uri.is_some() {
                        true => vec![DiffHeaderPress::OpenFile],
                        false => Vec::new(),
                    },
                    primary: header.uri.is_some().then_some(DiffHeaderPress::OpenFile),
                    title_size: size,
                    bold: false,
                    title_color: None,
                    baseline: header_h * 0.5 + size * 0.36,
                },
                header_h,
                card_width,
            );
            let uri = header.uri.clone();
            let widget = face.widget(move |press| match (press, uri.clone()) {
                (DiffHeaderPress::OpenFile, Some(uri)) => Some(CellCommand::OpenFile(uri)),
                _ => None,
            });
            card = card.child(imba::fixed(imba::eager(widget)));
        }

        let rewrap = match &cell.body {
            CellBody::Markdown(editor) => {
                ((editor.layout_width() - editor_target).abs() > 1.0).then_some(editor_target)
            }
            CellBody::Diff { view, .. } => {
                let laid = view
                    .split
                    .right
                    .document
                    .layout_width(view.split.right.editor);
                ((laid - editor_target).abs() > 1.0).then_some(editor_target)
            }
            _ => None,
        };
        let framed = card
            .pad_insets(imba::Insets {
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: chrome.gap,
            })
            .layout(arena, constraints);
        imba::ThunkBox::new(
            arena,
            framed.wrap(move |inner| CellWidget { inner, rewrap }),
        )
    }
}
