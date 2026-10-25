// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::tool_group::{
    ToolCallSpec, ToolGroup, ToolRowCommand, ToolRowKey, ToolRowsCommand, ToolUpdate,
};
use editor::{editor_view::EditorCommand, editor_view::EditorView, env};
use hikit::tree_item::TreeItemCommand;
use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult},
    store::Store,
    thunk_ext::ThunkExt,
    ui::UiCtx,
    Thunk, View, Widget,
};
use skia_safe::{Rect, Size};

type ChatChrome = ::hikit::chrome::ChatChrome;

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

    Rewrite(text::text::Text),

    Rewrap(f32),

    ResolveDiff(Result<crate::file_edit::BuiltFileEdit, String>),

    /// The off-thread markdown build landed, for the cell whose
    /// `pending_nonce` this is.
    ResolveText {
        nonce: u64,
        built: documents::BuiltDocument,
    },

    Diff(editor::unified_diff::UnifiedDiffCommand),

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
        view: editor::unified_diff::UnifiedDiffView,
    },

    Tools(ToolGroup),

    /// One quiet line of the turn's own (how it ended, what it
    /// spent): a label in its role, not a document.
    Line { text: String, role: LineRole },
}

/// The roles a turn's own lines speak in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineRole {
    /// "Turn cancelled." — the brief register.
    Notice,
    /// "8 in · 2,757 out" — smaller still.
    Usage,
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
    /// A cell nested in a tool row (a call's expanded body): it owns
    /// its whole width — no voice gutter, no row gap.
    bare: bool,
}

/// The transcript GRID a cell lays in (docs/Design.md): the voice
/// gutter, the text column after it, the air under the row. A bare
/// cell's grid is its whole width.
#[derive(Clone, Copy)]
pub(crate) struct Grid {
    /// Where the text column starts — the gutter's width.
    pub edge: f32,
    pub column: f32,
    pub gap: f32,
}

impl Grid {
    pub(crate) fn of(store: &Store, width: f32, bare: bool) -> Self {
        if bare {
            return Self {
                edge: 0.0,
                column: width.max(1.0),
                gap: 0.0,
            };
        }
        let theme = env::Themes::of(store);
        let chat = theme.part(::hikit::chrome::ChatChrome::KEY);
        let edge = theme.part(::editor::theme::WindowChrome::KEY).chat_text_edge;
        Self {
            edge,
            column: (width - edge - chat.text_trailing).max(120.0),
            gap: chat.row_gap,
        }
    }
}

/// The covering style a cell's prose wears: the prompt register for
/// your own message, the reading register for everything said back.
fn prose_style(kind: CellKind) -> editor::theme::StyleId {
    match kind {
        CellKind::User => editor::theme::StyleId::ChatPrompt,
        CellKind::Agent
        | CellKind::Reasoning
        | CellKind::Tool
        | CellKind::Notice
        | CellKind::Error => editor::theme::StyleId::Chat,
    }
}

/// The covering markup over a whole text — greedy both ways, so the
/// stream's appends wear it too.
fn prose_markup(kind: CellKind, length: u32) -> editor::markup::Markup {
    let mut markup = editor::markup::Markup::new();
    markup.push_styled_covering(0..length, prose_style(kind));
    markup
}

fn mint_nonce() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn markdown_document(text: text::text::Text, kind: CellKind) -> editor::document::Document {
    let length = text.byte_count().min(u32::MAX as usize) as u32;
    editor::document::Document::new(text, prose_markup(kind, length)).with_syntax(
        editor::markup::Syntax::new("markdown", None, editor::markup::Markup::new()),
        &[],
    )
}

/// One side of an edit as a document with its syntax. The parsers are
/// HANDED IN: this runs in the workshop's bare build store (off the UI
/// thread), which knows no languages.
pub(crate) fn side_document(
    text: text::text::Text,
    extension: &str,
    parsers: &std::sync::Arc<editor::reparse::SyntaxLanguages>,
    store: &Store,
    ui: &imba::ui::UiCtx,
    theme: &editor::theme::Theme,
) -> editor::document::Document {
    // An extension the registry knows parses as that language; any
    // other (and a bare unit-test registry) is PLAIN TEXT — code-set,
    // no syntax — never markdown.
    if extension.is_empty() || !parsers.knows(extension) {
        return editor::document::Document::plain_text(text);
    }
    editor::document::Document::from_language(text, extension, parsers, store, ui, theme)
}

pub(crate) fn document_text(document: &editor::document::Document) -> String {
    let end = document.text().byte_count().min(u32::MAX as usize) as u32;
    document.text().view().substring(0..end)
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
            text::text::Text::from_string_exact(markdown),
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
        let chrome = env::Themes::of(store).part(::hikit::chrome::ChatChrome::KEY);
        let grid = Grid::of(store, content_width, false);
        let lines = markdown.lines().count().clamp(1, 40) as f32;
        let estimate = (lines * chrome.min_cell_height).max(chrome.min_cell_height);
        (
            Self {
                kind,
                body: CellBody::PendingText {
                    markdown: markdown.to_owned(),
                    width: grid.column,
                    estimate,
                    nonce: mint_nonce(),
                    grown: Grown::Nothing,
                },
                bare: false,
            },
            estimate + grid.gap,
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
        text: text::text::Text,
        content_width: f32,
        fx: &mut Effects<'_, EditorCommand>,
    ) -> (Self, f32) {
        Self::build_text_in(store, ui, kind, text, content_width, false, fx)
    }

    /// A markdown cell NESTED in a tool row — a call's expanded body:
    /// the whole width, no gutter, no row gap.
    pub(crate) fn build_bare(
        store: &Store,
        ui: &UiCtx,
        kind: CellKind,
        text: text::text::Text,
        content_width: f32,
        fx: &mut Effects<'_, EditorCommand>,
    ) -> (Self, f32) {
        Self::build_text_in(store, ui, kind, text, content_width, true, fx)
    }

    fn build_text_in(
        store: &Store,
        ui: &UiCtx,
        kind: CellKind,
        text: text::text::Text,
        content_width: f32,
        bare: bool,
        fx: &mut Effects<'_, EditorCommand>,
    ) -> (Self, f32) {
        note_mount();
        let theme = env::Themes::of(store);
        let chrome = theme.part(::hikit::chrome::ChatChrome::KEY);
        let grid = Grid::of(store, content_width, bare);
        let mut document = markdown_document(text, kind);
        let editor = document.add_editor(
            grid.column,
            None,
            ::editor::document::EditorBuild::Bounded,
            &[],
            store,
            ui,
            &theme,
            fx,
        );
        if let Some(parsers) = env::Parsers::of(store) {
            document.launch_reparse(parsers, fx);
        }
        let height = document.content_height(editor).max(chrome.min_cell_height) + grid.gap;
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
                bare,
            },
            height,
        )
    }

    pub(crate) fn pending_diff(
        store: &Store,
        header: DiffHeader,
        content_width: f32,
    ) -> (Self, f32) {
        let chrome = env::Themes::of(store).part(::hikit::chrome::ChatChrome::KEY);
        let height = Self::header_band(&chrome) + Grid::of(store, content_width, false).gap;
        (
            Self {
                kind: CellKind::Tool,
                body: CellBody::PendingDiff {
                    header,
                    width: content_width,
                },
                bare: false,
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
        let grid = Grid::of(store, content_width, false);
        let (group, rows_height) =
            ToolGroup::new(store, ui, specs, Self::tools_width(store, grid));
        (
            Self {
                kind: CellKind::Tool,
                body: CellBody::Tools(group),
                bare: false,
            },
            rows_height + grid.gap,
        )
    }

    /// A turn's own quiet line: born laid, nothing to build.
    pub(crate) fn line(store: &Store, role: LineRole, text: &str) -> (Self, f32) {
        let cell = Self {
            kind: CellKind::Notice,
            body: CellBody::Line {
                text: text.to_owned(),
                role,
            },
            bare: false,
        };
        let height = cell.measured_height(store);
        (cell, height)
    }

    /// The tool rows hang their chevrons into the gutter: the rows
    /// start a chevron's hang plus the tree's text inset before the
    /// text column, so a call's name lands ON the column's edge.
    fn tools_lead(store: &Store, grid: Grid) -> f32 {
        let theme = env::Themes::of(store);
        let hang = theme.part(::hikit::chrome::ChatChrome::KEY).chevron_hang;
        let text_x = theme.part(::hikit::chrome::TreeChrome::KEY).text_x;
        (grid.edge - hang - text_x).max(0.0)
    }

    fn tools_width(store: &Store, grid: Grid) -> f32 {
        grid.edge + grid.column - Self::tools_lead(store, grid)
    }

    /// The diff header: one quiet row.
    fn header_band(chrome: &ChatChrome) -> f32 {
        chrome.tool_row_height
    }

    fn line_height(chrome: &ChatChrome) -> f32 {
        chrome.usage_line
    }

    /// The cell's height as DATA — the same number its display lays
    /// to, derived from the content without building a tree. Heights
    /// land through the COMMAND road (the editor-repair law): a
    /// landing that changed content re-splices its lists with this,
    /// and the list's paint report stays a belt, not the mechanism.
    pub(crate) fn measured_height(&self, store: &Store) -> f32 {
        let chrome = env::Themes::of(store).part(::hikit::chrome::ChatChrome::KEY);
        // The gap is width-free: only the column depends on the width.
        let gap = Grid::of(store, 1000.0, self.bare).gap;
        let content = match &self.body {
            CellBody::Tools(group) => group.rows_height(),
            CellBody::Line { .. } => Self::line_height(&chrome),
            CellBody::Markdown(editor) => editor
                .document
                .content_height(editor.editor)
                .max(chrome.min_cell_height),
            CellBody::PendingText { estimate, .. } => *estimate,
            CellBody::PendingDiff { .. } => Self::header_band(&chrome),
            CellBody::Diff { view, .. } => {
                Self::header_band(&chrome)
                    + view
                        .split
                        .right
                        .document
                        .content_height(view.split.right.editor)
                        .max(chrome.min_cell_height)
            }
        };
        content + gap
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
        let theme = env::Themes::of(store);
        let mut document = built.document;
        let kind = self.kind;
        let editor = fx.scope(CellCommand::Editor, |fx| {
            // The prose register rides a markup of its own, in place
            // before the editor lays — the build road knows nothing
            // of voices, and nothing is shaped twice.
            let prose = document.add_markup();
            let length = document.text().byte_count().min(u32::MAX as usize) as u32;
            document.replace_markup(
                prose,
                prose_markup(kind, length),
                &[],
                store,
                ui,
                &theme,
                fx,
            );
            document.add_editor(
                width,
                None,
                ::editor::document::EditorBuild::Bounded,
                &[prose],
                store,
                ui,
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
                let text = text::text::Text::from_string_exact(&markdown);
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
        result: Result<crate::file_edit::BuiltFileEdit, String>,
        fx: &mut Effects<'_, CellCommand>,
    ) {
        let CellBody::PendingDiff { header, width } = self.body.clone() else {
            return;
        };
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
                let crate::file_edit::BuiltFileEdit {
                    before: mut before_doc,
                    after: mut after_doc,
                    diff: diff_id,
                    hunks,
                    left_marks,
                    prepared,
                } = built;

                let gutter = theme.part(::editor::theme::EditorGutterChrome::KEY).width;
                // Diff bodies run edge-to-edge — the fold strips and
                // washes bump the chat's edges exactly.
                let editor_width = (width - gutter).max(120.0);
                let mut throwaway = imba::effect::Batch::new();
                let quiet = &mut throwaway.effects();

                let left_editor = before_doc.add_editor(
                    editor_width,
                    None,
                    ::editor::document::EditorBuild::Bounded,
                    &[left_marks],
                    store,
                    ui,
                    &theme,
                    quiet,
                );
                before_doc.manage_repairs_in_pair(left_editor);

                let right_editor = after_doc.add_editor(
                    editor_width,
                    None,
                    ::editor::document::EditorBuild::Bounded,
                    &[hunks],
                    store,
                    ui,
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
                    &theme,
                    quiet,
                );

                let state = editor::split_diff::DiffViewState::attach(
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
                let mut view = editor::unified_diff::UnifiedDiffView::new(
                    editor::split_diff::SplitDiffView::new(left, right, state),
                );
                fx.scope(CellCommand::Diff, |fx| {
                    view.perform(
                        store,
                        ui,
                        editor::unified_diff::UnifiedDiffCommand::SetLayout(
                            editor::unified_diff::DiffLayout::Inline,
                        ),
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
            CellBody::Line { text, .. } => (kind, text.clone()),
        }
    }

    fn editor_view(&self) -> Option<&EditorView> {
        match &self.body {
            CellBody::Markdown(view) => Some(view),
            CellBody::Tools(_)
            | CellBody::Line { .. }
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
            | CellBody::Line { .. }
            | CellBody::Tools(_) => None,
        }
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
                if let CellBody::Line { text: held, .. } = &mut self.body {
                    *held = text.to_string();
                    return;
                }
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
                let theme = env::Themes::of(store);
                fx.scope(CellCommand::Editor, |fx| {
                    editor
                        .document
                        .edit(&operation, store, ui, &theme, fx);
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
                            CellCommand::Diff(editor::unified_diff::UnifiedDiffCommand::Split(
                                editor::split_diff::SplitDiffCommand::Left(command),
                            ))
                        },
                        |fx| {
                            view.split.left.document.resize(
                                left_editor,
                                width,
                                0,
                                store,
                                ui,
                                &theme,
                                fx,
                            )
                        },
                    );
                    fx.scope(
                        |command: EditorCommand| {
                            CellCommand::Diff(editor::unified_diff::UnifiedDiffCommand::Split(
                                editor::split_diff::SplitDiffCommand::Right(command),
                            ))
                        },
                        |fx| {
                            view.split.right.document.resize(
                                right_editor,
                                width,
                                0,
                                store,
                                ui,
                                &theme,
                                fx,
                            )
                        },
                    );
                    if let Some(inline) = inline {
                        fx.scope(
                            |command: EditorCommand| {
                                CellCommand::Diff(editor::unified_diff::UnifiedDiffCommand::Inline(
                                    command,
                                ))
                            },
                            |fx| {
                                view.split
                                    .right
                                    .document
                                    .resize(inline, width, 0, store, ui, &theme, fx)
                            },
                        );
                    }
                    fx.scope(CellCommand::Diff, |fx| {
                        view.perform(
                            store,
                            ui,
                            editor::unified_diff::UnifiedDiffCommand::Split(
                                editor::split_diff::SplitDiffCommand::Resync,
                            ),
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
                        .resize(id, width, 0, store, ui, &theme, fx);
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
                let operation = operation::operation::Operation::insert_at(end, chunk);
                let theme = env::Themes::of(store);
                fx.scope(CellCommand::Editor, |fx| {
                    editor
                        .document
                        .edit(&operation, store, ui, &theme, fx);
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
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        CardFrame {
            cell: self,
            store,
            ui,
        }
    }
}

fn chrome_gutter(store: &Store) -> f32 {
    env::Themes::of(store).part(::editor::theme::EditorGutterChrome::KEY).width
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
            return result.merge(EventResult::command(CellCommand::Rewrap(width)));
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

/// The chat cell as a ROW of the transcript grid (docs/Design.md,
/// "size carries rank"): the voice gutter — "YOU" in the caps role
/// over your own message, empty for everything said back — then the
/// text column, then the air under the row. No card, no surface: the
/// register is the type. An exact-extent spacer pins the row's box to
/// the same number `measured_height` lands.
struct CardFrame<'a> {
    cell: &'a Cell,
    store: &'a Store,
    ui: &'a UiCtx,
}

impl imba::layout::LayoutValue for CardFrame<'_> {}

impl<'a> imba::layout::Layout<'a, CellCommand> for CardFrame<'a> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, CellCommand> {
        use imba::layout::LayoutExt as _;
        let CardFrame { cell, store, ui } = self;
        let theme = env::Themes::of(store);
        let chrome = theme.part(::hikit::chrome::ChatChrome::KEY);
        let width = constraints.max.width.max(1.0);
        let grid = Grid::of(store, width, cell.bare);
        let under = imba::layout::Insets {
            bottom: grid.gap,
            ..Default::default()
        };

        if let CellBody::Tools(group) = &cell.body {
            let lead = Cell::tools_lead(store, grid);
            let rows_width = Cell::tools_width(store, grid);
            let rows = group
                .layout(
                    arena,
                    store,
                    ui,
                    Constraints {
                        min: Size::new(rows_width, 0.0),
                        max: Size::new(rows_width, f32::MAX),
                    },
                )
                .map(CellCommand::ToolRows);
            return imba::layout::fixed(rows)
                .pad_insets(imba::layout::Insets {
                    left: lead,
                    ..under
                })
                .layout(arena, constraints);
        }

        if let CellBody::Line { text, role } = &cell.body {
            let style = match role {
                LineRole::Notice => hikit::ui::brief(store, ui).sized(chrome.notice_size),
                LineRole::Usage => hikit::ui::brief(store, ui).sized(chrome.usage_size),
            };
            return imba::layout::ZBox::new(arena)
                .child(imba::layout::spacer(grid.column, Cell::line_height(&chrome)))
                .child_aligned(
                    imba::layout::Alignment::CenterStart,
                    hikit::ui::text(&style, text.clone()),
                )
                .pad_insets(imba::layout::Insets {
                    left: grid.edge,
                    ..under
                })
                .layout(arena, constraints);
        }

        let header = match &cell.body {
            CellBody::PendingDiff { header, .. } => Some((header, true)),
            CellBody::Diff { header, .. } => Some((header, false)),
            _ => None,
        };
        let header_h = match header {
            Some(_) => Cell::header_band(&chrome),
            None => 0.0,
        };
        // A diff body runs edge to edge — its own line gutter is its
        // margin; prose keeps the text column.
        let (body_x, editor_target) = match &cell.body {
            CellBody::Diff { .. } => (0.0, (width - chrome_gutter(store)).max(120.0)),
            _ => (grid.edge, grid.column),
        };
        let editor = cell.editor_view();
        let diff_thunk = match &cell.body {
            CellBody::Diff { view, .. } => Some(
                imba::layout::Layout::layout(
                    view.display(arena, store, ui),
                    arena,
                    Constraints {
                        min: Size::new(editor_target, 0.0),
                        max: Size::new(editor_target + chrome_gutter(store), f32::MAX),
                    },
                )
                .map(CellCommand::Diff),
            ),
            _ => None,
        };
        let pending_height = match &cell.body {
            CellBody::PendingText { estimate, .. } => Some(*estimate),
            _ => None,
        };
        let body_height = editor
            .map(|editor| editor.content_height().max(chrome.min_cell_height))
            .or_else(|| {
                diff_thunk
                    .as_ref()
                    .map(|thunk| thunk.size().height.max(chrome.min_cell_height))
            })
            .or(pending_height)
            .unwrap_or(0.0);

        let mut row = imba::layout::ZBox::new(arena)
            .child(imba::layout::spacer(width, header_h + body_height));
        let content = editor
            .map(|editor| {
                imba::layout::Layout::layout(
                    editor.display(arena, store, ui),
                    arena,
                    Constraints {
                        min: Size::new(editor_target, body_height),
                        max: Size::new(editor_target + editor.gutter_width, f32::MAX),
                    },
                )
                .map(CellCommand::Editor)
            })
            .map(|thunk| imba::ThunkBox::new(arena, thunk))
            .or(diff_thunk.map(|thunk| imba::ThunkBox::new(arena, thunk)));
        if let Some(content) = content {
            row = row.child(imba::layout::fixed(content).pad_insets(imba::layout::Insets {
                left: body_x,
                top: header_h,
                ..Default::default()
            }));
        }
        if matches!(cell.kind, CellKind::User) && !cell.bare && chrome.voice_gutter {
            row = row.child(voice(arena, store, ui, &chrome, grid, chrome.gutter_line, "YOU"));
        }
        if let Some((header, pending)) = header {
            row = row.child(
                diff_header(arena, store, ui, header, pending, header_h).pad_insets(
                    imba::layout::Insets {
                        left: grid.edge,
                        ..Default::default()
                    },
                ),
            );
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
        let framed = row.pad_insets(under).layout(arena, constraints);
        imba::ThunkBox::new(
            arena,
            framed.wrap(move |inner| CellWidget { inner, rewrap }),
        )
    }
}

/// The VOICE in the gutter: a caps label right-aligned against the
/// text column, centered in a `line` box at the row's top — the
/// gutter's own line in the transcript, the input's in the footer.
pub(crate) fn voice<'a, Command: 'a>(
    arena: &'a Arena,
    store: &Store,
    ui: &UiCtx,
    chrome: &ChatChrome,
    grid: Grid,
    line: f32,
    label: &'static str,
) -> impl imba::layout::Layout<'a, Command> + imba::layout::LayoutValue + 'a {
    imba::layout::ZBox::new(arena)
        .child(imba::layout::spacer(
            (grid.edge - chrome.gutter_pad).max(0.0),
            line,
        ))
        .child_aligned(
            imba::layout::Alignment::CenterEnd,
            hikit::ui::text(&hikit::ui::caps(store, ui), label),
        )
}

/// A file edit's header as a quiet ROW: the file's name in the row
/// role, its counts in the added and removed inks; pressing the name
/// opens the working copy.
fn diff_header<'a>(
    arena: &'a Arena,
    store: &Store,
    ui: &UiCtx,
    header: &DiffHeader,
    pending: bool,
    height: f32,
) -> impl imba::layout::Layout<'a, CellCommand> + imba::layout::LayoutValue + 'a {
    use imba::layout::LayoutExt as _;
    let theme = env::Themes::of(store);
    let ink = theme.part(::editor::theme::InkChrome::KEY);
    let chrome = theme.part(::hikit::chrome::ChatChrome::KEY);
    let name = hikit::ui::row(store, ui).colored(ink.ink.0);
    let brief = hikit::ui::brief(store, ui);
    let mut words = imba::layout::Row::new(arena)
        .gap(chrome.tool_gap)
        .child_by_baseline(hikit::ui::text(&name, header.title.clone()));
    if pending {
        words = words.child_by_baseline(hikit::ui::text(&brief, "fetching contents…"));
    }
    if let Some(added) = header.added.filter(|n| *n > 0) {
        words = words.child_by_baseline(hikit::ui::text(
            &brief.clone().colored(ink.added.0),
            format!("+{added}"),
        ));
    }
    if let Some(removed) = header.removed.filter(|n| *n > 0) {
        words = words.child_by_baseline(hikit::ui::text(
            &brief.clone().colored(ink.removed.0),
            format!("\u{2212}{removed}"),
        ));
    }
    let uri = header.uri.clone();
    imba::layout::ZBox::new(arena)
        .child(imba::layout::spacer(0.0, height))
        .child_aligned(imba::layout::Alignment::CenterStart, words)
        .on_event(move |_arena: &Arena, event: &Event<'_>, _size: Size| match (event, uri.clone()) {
            (Event::MouseDown { .. }, Some(uri)) => {
                EventResult::command(CellCommand::OpenFile(uri))
            }
            _ => EventResult::Ignored,
        })
}
