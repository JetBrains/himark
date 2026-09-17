// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::higent::tool_group::{
    ToolCallSpec, ToolGroup, ToolRowCommand, ToolRowKey, ToolRowsCommand, ToolUpdate,
};
use crate::tree_item::TreeItemCommand;
use crate::{env, EditorCommand, EditorView};
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

type ChatChrome = crate::theme::ChatChrome;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CellKind {
    User,
    Agent,
    Reasoning,
    Tool,
    Notice,
    Error,
}

pub enum CellCommand {
    Editor(EditorCommand),

    Rewrite(crate::Text),

    Rewrap(f32),

    ResolveDiff(Result<crate::higent::FileEditContents, String>),

    Diff(crate::UnifiedDiffCommand),

    Tool(ToolUpdate),

    ToolRows(ToolRowsCommand),

    ToolRow {
        key: ToolRowKey,
        command: TreeItemCommand<ToolRowCommand>,
    },

    Append(String),
}

#[derive(Clone)]
pub(crate) struct DiffHeader {
    pub title: String,
    pub added: Option<i64>,
    pub removed: Option<i64>,
}

#[derive(Clone)]
enum CellBody {
    Markdown(EditorView),

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
        view: crate::UnifiedDiffView,
    },

    Tools(ToolGroup),
}

#[derive(Clone)]
pub struct Cell {
    kind: CellKind,
    body: CellBody,
}

fn markdown_document(text: crate::Text) -> crate::Document {
    crate::Document::new(text, crate::Markup::new()).with_syntax(
        crate::Syntax::new("markdown", None, crate::Markup::new()),
        &[],
    )
}

pub(crate) fn side_document(
    text: crate::Text,
    extension: &str,
    store: &Store,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &crate::Theme,
) -> crate::Document {
    if let Some(parsers) = env::Parsers::of(store) {
        let language = if !extension.is_empty() && parsers.knows(extension) {
            extension
        } else {
            "markdown"
        };
        return crate::Document::from_language(text, language, &parsers, fonts, theme);
    }
    markdown_document(text)
}

pub(crate) fn document_text(document: &crate::Document) -> String {
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
            crate::Text::from_string_exact(markdown),
            content_width,
            fx,
        )
    }

    pub(crate) fn build_text(
        store: &Store,
        ui: &UiCtx,
        kind: CellKind,
        text: crate::Text,
        content_width: f32,
        fx: &mut Effects<'_, EditorCommand>,
    ) -> (Self, f32) {
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

    fn resolve_diff(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        result: Result<crate::higent::FileEditContents, String>,
        fx: &mut Effects<'_, CellCommand>,
    ) {
        let CellBody::PendingDiff { header, width } = self.body.clone() else {
            return;
        };
        let fonts = env::ui_collection(store, ui);
        let theme = env::Themes::of(store);
        let _chrome = theme.ui().chat.clone();
        match result {
            Err(error) => {
                let markdown = format!("**{}** — contents unavailable: {error}", header.title);
                let (cell, _) = fx.scope(CellCommand::Editor, |fx| {
                    Cell::build(store, ui, CellKind::Error, &markdown, width, fx)
                });
                *self = cell;
            }
            Ok(contents) => {
                let before_text =
                    crate::Text::from_string_exact(contents.before.as_deref().unwrap_or(""));
                let after_text =
                    crate::Text::from_string_exact(contents.after.as_deref().unwrap_or(""));
                let extension = header.title.rsplit('.').next().unwrap_or("").to_lowercase();
                let mut before_doc =
                    side_document(before_text.clone(), &extension, store, &fonts, &theme);
                let mut after_doc = side_document(after_text, &extension, store, &fonts, &theme);

                // The seeded pair road (the hidiff recipe, cell-owned
                // documents): operation, THE diff markup, and the
                // prepared marks — washes, word tints, fold strips —
                // all settled before the first frame.
                let operation = crate::diff::diff(&before_text, after_doc.text());
                let diff_id = after_doc.add_diff(operation.clone(), before_doc.revision());
                after_doc.install_normalized_diff(
                    diff_id,
                    operation.clone(),
                    before_doc.revision(),
                );
                let hunks = after_doc.diff(diff_id).expect("just added").markup();
                let prepared = crate::prepare_marks(&operation, before_doc.text());

                let gutter = theme.ui().editor_gutter.width;
                // Diff bodies run edge-to-edge — the fold strips and
                // washes bump the chat's edges exactly.
                let editor_width = (width - gutter).max(120.0);
                let mut throwaway = imba::effect::Batch::new();
                let quiet = &mut throwaway.effects();

                let left_marks = before_doc.add_markup();
                before_doc.replace_markup(
                    left_marks,
                    prepared.left.clone(),
                    &[],
                    &fonts,
                    &theme,
                    quiet,
                );
                let left_editor = before_doc.add_editor(
                    editor_width,
                    None,
                    ::editor::EditorBuild::Bounded,
                    &[left_marks],
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
                    &fonts,
                    &theme,
                    quiet,
                );

                if let Some(parsers) = env::Parsers::of(store) {
                    fx.scope(
                        |command: EditorCommand| {
                            CellCommand::Diff(crate::UnifiedDiffCommand::Split(
                                crate::SplitDiffCommand::Left(command),
                            ))
                        },
                        |fx| before_doc.launch_reparse(parsers.clone(), fx),
                    );
                    fx.scope(
                        |command: EditorCommand| {
                            CellCommand::Diff(crate::UnifiedDiffCommand::Split(
                                crate::SplitDiffCommand::Right(command),
                            ))
                        },
                        |fx| after_doc.launch_reparse(parsers, fx),
                    );
                }

                let state = crate::DiffState::attach(
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
                    crate::UnifiedDiffView::new(crate::SplitDiffView::new(left, right, state));
                fx.scope(CellCommand::Diff, |fx| {
                    view.perform(
                        store,
                        ui,
                        crate::UnifiedDiffCommand::SetLayout(crate::DiffLayout::Inline),
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

    fn editor_view_mut(&mut self) -> Option<&mut EditorView> {
        match &mut self.body {
            CellBody::Markdown(editor) => Some(editor),
            CellBody::Diff { .. } | CellBody::PendingDiff { .. } | CellBody::Tools(_) => None,
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
                let CellBody::Markdown(editor) = &mut self.body else {
                    return;
                };

                if *editor.document.text() == text {
                    return;
                }

                let operation = crate::diff::diff(editor.document.text(), &text);
                let fonts = env::ui_collection(store, ui);
                let theme = env::Themes::of(store);
                fx.scope(CellCommand::Editor, |fx| {
                    editor.document.edit(&operation, &fonts, &theme, fx);
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
                            CellCommand::Diff(crate::UnifiedDiffCommand::Split(
                                crate::SplitDiffCommand::Left(command),
                            ))
                        },
                        |fx| {
                            view.split.left.document.resize(
                                left_editor,
                                width,
                                0,
                                &fonts,
                                &theme,
                                fx,
                            )
                        },
                    );
                    fx.scope(
                        |command: EditorCommand| {
                            CellCommand::Diff(crate::UnifiedDiffCommand::Split(
                                crate::SplitDiffCommand::Right(command),
                            ))
                        },
                        |fx| {
                            view.split.right.document.resize(
                                right_editor,
                                width,
                                0,
                                &fonts,
                                &theme,
                                fx,
                            )
                        },
                    );
                    if let Some(inline) = inline {
                        fx.scope(
                            |command: EditorCommand| {
                                CellCommand::Diff(crate::UnifiedDiffCommand::Inline(command))
                            },
                            |fx| {
                                view.split
                                    .right
                                    .document
                                    .resize(inline, width, 0, &fonts, &theme, fx)
                            },
                        );
                    }
                    fx.scope(CellCommand::Diff, |fx| {
                        view.perform(
                            store,
                            ui,
                            crate::UnifiedDiffCommand::Split(crate::SplitDiffCommand::Resync),
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
                    editor.document.resize(id, width, 0, &fonts, &theme, fx);
                });
            }
            CellCommand::Append(chunk) => {
                let CellBody::Markdown(editor) = &mut self.body else {
                    return;
                };
                let end = editor.document.text().byte_count().min(u32::MAX as usize) as u32;
                let operation = crate::Operation::insert_at(end, chunk);
                let fonts = env::ui_collection(store, ui);
                let theme = env::Themes::of(store);
                fx.scope(CellCommand::Editor, |fx| {
                    editor.document.edit(&operation, &fonts, &theme, fx);
                    if let Some(parsers) = env::Parsers::of(store) {
                        editor.document.launch_reparse(parsers, fx);
                    }
                });
            }
            CellCommand::ResolveDiff(result) => self.resolve_diff(store, ui, result, fx),
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

fn header_band_layout<'a>(
    arena: &'a Arena,
    ui: &UiCtx,
    chrome: &ChatChrome,
    header: &DiffHeader,
    pending: bool,
    height: f32,
) -> impl imba::Layout<'a, CellCommand> + imba::LayoutValue + 'a {
    let title = if pending {
        format!("{} — fetching contents…", header.title)
    } else {
        header.title.clone()
    };
    let text = crate::ui::TextStyle {
        font: crate::fonts::ui_text_font(ui, chrome.title_size * 0.85),
        color: chrome.text_color.0,
        tracking: 0.0,
    };
    let _ = height;
    let style = crate::ui::RowStyle {
        inset: chrome.pad,
        trail_inset: chrome.pad,
        label: text.clone(),
        trail: text.clone(),
        air: crate::ui::space::S,
    };
    let mut band = crate::ui::ListRow::new(arena, style).label(title);
    if let Some(added) = header.added.filter(|n| *n > 0) {
        band = band.trail_styled(
            &text.clone().colored(chrome.added_color.0),
            format!("+{added}"),
        );
    }
    if let Some(removed) = header.removed.filter(|n| *n > 0) {
        band = band.trail_styled(
            &text.clone().colored(chrome.removed_color.0),
            format!("-{removed}"),
        );
    }
    band
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

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, CellCommand>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}

#[cfg(test)]
mod tests;

/// The chat cell CARD, reified (docs/UI.md stage 2): an exact-extent
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
                    crate::ui::Surface {
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
            view.layout(
                arena,
                store,
                ui,
                Constraints {
                    min: Size::new(editor_target, 0.0),
                    max: Size::new(editor_target + chrome_gutter(store), f32::MAX),
                },
            )
            .map(CellCommand::Diff)
        });
        let editor_height = editor
            .map(|editor| editor.content_height().max(chrome.min_cell_height))
            .or_else(|| {
                diff_thunk
                    .as_ref()
                    .map(|thunk| thunk.size().height.max(chrome.min_cell_height))
            })
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
                    crate::ui::Surface {
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
                editor
                    .layout(
                        arena,
                        store,
                        ui,
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
            let caps = crate::ui::caps(store, ui).colored(chrome.notice_color.0);
            card = card.child(
                imba::ZBox::new(arena)
                    .child(imba::spacer(0.0, header_h))
                    .child_aligned(imba::Alignment::CenterStart, crate::ui::text(&caps, "YOU"))
                    .pad_insets(imba::Insets {
                        left: chrome.pad,
                        ..Default::default()
                    }),
            );
        }
        if let Some((header, pending)) = header {
            card = card.child(header_band_layout(
                arena, ui, &chrome, header, pending, header_h,
            ));
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
