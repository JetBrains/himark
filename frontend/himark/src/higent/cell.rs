use crate::higent::tool_group::{
    ToolCallSpec, ToolGroup, ToolRowCommand, ToolRowKey, ToolRowsCommand, ToolUpdate,
};
use crate::tree_item::TreeItemCommand;
use crate::{env, EditorCommand, EditorView};
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::Effects,
    event::{Event, EventResult},
    leaf::leaf,
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

    Diff {
        header: DiffHeader,
        editor: EditorView,
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
        let chrome = theme.ui().chat.clone();
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
                let before_doc =
                    side_document(before_text.clone(), &extension, store, &fonts, &theme);
                let mut after_doc = side_document(after_text, &extension, store, &fonts, &theme);

                let operation = crate::diff::diff(&before_text, after_doc.text());
                let diff_id = after_doc.add_diff(operation, before_doc.revision());
                let gutter = theme.ui().editor_gutter.width;
                let editor_width = (width - chrome.pad * 2.0 - gutter).max(120.0);
                let editor = fx.scope(CellCommand::Editor, |fx| {
                    let editor = after_doc.add_editor(
                        editor_width,
                        None,
                        ::editor::EditorBuild::Bounded,
                        &[],
                        &fonts,
                        &theme,
                        fx,
                    );

                    if let Some(parsers) = env::Parsers::of(store) {
                        after_doc.launch_reparse(parsers, fx);
                    }
                    editor
                });
                self.body = CellBody::Diff {
                    header,
                    editor: EditorView {
                        document: after_doc,
                        editor,
                        reports_geometry: false,
                        location: None,
                        gutter_width: gutter,
                        base: Some((before_doc, diff_id)),
                    },
                };
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
            CellBody::Diff { header, editor } => (
                kind,
                format!(
                    "[diff {} +{} -{}]\n{}",
                    header.title,
                    header.added.unwrap_or(0),
                    header.removed.unwrap_or(0),
                    document_text(&editor.document)
                ),
            ),
            CellBody::Tools(group) => (kind, group.oracle().join("\n")),
        }
    }

    fn editor_view_mut(&mut self) -> Option<&mut EditorView> {
        match &mut self.body {
            CellBody::Markdown(editor) => Some(editor),
            CellBody::Diff { editor, .. } => Some(editor),
            CellBody::PendingDiff { .. } | CellBody::Tools(_) => None,
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
            CellCommand::Rewrap(width) => {
                let fonts = env::ui_collection(store, ui);
                let theme = env::Themes::of(store);
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

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let chrome = env::Themes::of(store).ui().chat.clone();
        let width = constraints.max.width.max(1.0);
        let (card_x, card_width) = Self::card_geometry(self.kind, &chrome, width);

        let tools = if let CellBody::Tools(group) = &self.body {
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
            let card_height = rows.size().height + chrome.pad * 2.0;
            let mut card = container(arena, Size::new(width, card_height + chrome.gap));
            let border = chrome.input_border.0;
            let surface = cell_surface(CellKind::Tool, &chrome);
            let radius = chrome.radius;
            let backdrop = leaf::<CellCommand>(card_width, card_height).paint_instead(
                move |_arena, canvas, rect| {
                    let mut paint = Paint::default();
                    paint.set_anti_alias(true);
                    if let Some(surface) = surface {
                        paint.set_color(surface);
                        canvas.draw_round_rect(rect, radius, radius, &paint);
                    }
                    paint.set_stroke(true);
                    paint.set_stroke_width(1.0);
                    paint.set_color(border);
                    canvas.draw_round_rect(rect.with_inset((0.5, 0.5)), radius, radius, &paint);
                },
            );
            card.place(card_x, 0.0, backdrop);
            card.place(card_x + chrome.pad, chrome.pad, rows);
            Some(card)
        } else {
            None
        };

        let (header, editor, header_h) = match &self.body {
            CellBody::Markdown(editor) => (None, Some(editor), 0.0),
            CellBody::PendingDiff { header, .. } => {
                (Some((header, true)), None, Self::header_band(&chrome))
            }
            CellBody::Diff { header, editor } => (
                Some((header, false)),
                Some(editor),
                Self::header_band(&chrome),
            ),

            CellBody::Tools(_) => (None, None, 0.0),
        };

        let editor_target = match &self.body {
            CellBody::Diff { .. } => {
                (card_width - chrome.pad * 2.0 - chrome_gutter(store)).max(120.0)
            }
            _ => Self::editor_width(self.kind, &chrome, width),
        };

        let header_h = match self.kind {
            CellKind::User => Self::header_band(&chrome),
            _ => header_h,
        };
        let editor_height = editor
            .map(|editor| editor.content_height().max(chrome.min_cell_height))
            .unwrap_or(0.0);
        let card_height = header_h + editor_height + chrome.pad * 2.0;
        let height = card_height + chrome.gap;

        let is_tools = tools.is_some();
        let mut card = tools.unwrap_or_else(|| container(arena, Size::new(width, height)));
        if !is_tools {
            let border = match self.kind {
                CellKind::Tool | CellKind::Reasoning => Some(chrome.input_border.0),
                CellKind::Error => Some(chrome.stop_color.0),
                CellKind::User | CellKind::Agent | CellKind::Notice => None,
            };
            let surface = cell_surface(self.kind, &chrome);
            if surface.is_some() || border.is_some() {
                let radius = chrome.radius;
                let backdrop = leaf::<CellCommand>(card_width, card_height).paint_instead(
                    move |_arena, canvas, rect| {
                        let card_rect =
                            Rect::from_xywh(rect.left, rect.top, rect.width(), rect.height());
                        let mut paint = Paint::default();
                        paint.set_anti_alias(true);
                        if let Some(surface) = surface {
                            paint.set_color(surface);
                            canvas.draw_round_rect(card_rect, radius, radius, &paint);
                        }
                        if let Some(border) = border {
                            paint.set_stroke(true);
                            paint.set_stroke_width(1.0);
                            paint.set_color(border);
                            canvas.draw_round_rect(
                                card_rect.with_inset((0.5, 0.5)),
                                radius,
                                radius,
                                &paint,
                            );
                        }
                    },
                );
                card.place(card_x, 0.0, backdrop);
            }
            if matches!(self.kind, CellKind::User) {
                let bar_color = chrome.notice_color.0;
                let bar =
                    leaf::<CellCommand>(3.0, card_height).paint_instead(move |_arena, canvas, rect| {
                        let mut paint = Paint::default();
                        paint.set_color(bar_color);
                        canvas.draw_rect(rect, &paint);
                    });
                card.place(0.0, 0.0, bar);
                let label_font = crate::fonts::ui_text_font(ui, chrome.title_size * 0.8);
                let label_chrome = chrome.clone();
                let label = leaf::<CellCommand>(card_width, header_h).paint_instead(
                    move |_arena, canvas, rect| {
                        let mut paint = Paint::default();
                        paint.set_anti_alias(true);
                        paint.set_color(label_chrome.notice_color.0);
                        let mut x = rect.left + label_chrome.pad;
                        let baseline = rect.top + rect.height() * 0.65;
                        for ch in "YOU".chars() {
                            let glyph = ch.to_string();
                            canvas.draw_str(&glyph, (x, baseline), &label_font, &paint);
                            x += label_font.measure_str(&glyph, None).0 + 1.5;
                        }
                    },
                );
                card.place(0.0, 0.0, label);
            }
            if let Some((header, pending)) = header {
                card.place(
                    card_x,
                    0.0,
                    header_band_widget(ui, &chrome, header, pending, card_width, header_h),
                );
            }
            if let Some(editor) = editor {
                card.place(
                    card_x + chrome.pad,
                    header_h + chrome.pad,
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
                        .map(CellCommand::Editor),
                );
            }
        }
        let rewrap = editor.and_then(|editor| {
            ((editor.layout_width() - editor_target).abs() > 1.0).then_some(editor_target)
        });
        card.wrap(move |inner| CellWidget { inner, rewrap })
    }
}

fn chrome_gutter(store: &Store) -> f32 {
    env::Themes::of(store).ui().editor_gutter.width
}

fn header_band_widget<'a>(
    ui: &UiCtx,
    chrome: &ChatChrome,
    header: &DiffHeader,
    pending: bool,
    width: f32,
    height: f32,
) -> impl Thunk<'a, CellCommand> + use<'a> {
    let title = if pending {
        format!("{} — fetching contents…", header.title)
    } else {
        header.title.clone()
    };
    let added = header.added.filter(|n| *n > 0).map(|n| format!("+{n}"));
    let removed = header.removed.filter(|n| *n > 0).map(|n| format!("-{n}"));
    let font = crate::fonts::ui_text_font(ui, chrome.title_size * 0.85);
    let text_color = chrome.text_color.0;
    let added_color = chrome.added_color.0;
    let removed_color = chrome.removed_color.0;
    let pad = chrome.pad;
    leaf::<CellCommand>(width, height).paint_instead(move |_arena, canvas, rect| {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(text_color);
        let baseline = rect.top + height * 0.65;
        canvas.draw_str(title.as_str(), (rect.left + pad, baseline), &font, &paint);
        let mut x = rect.right - pad;
        if let Some(removed) = &removed {
            let w = font.measure_str(removed.as_str(), None).0;
            x -= w;
            paint.set_color(removed_color);
            canvas.draw_str(removed.as_str(), (x, baseline), &font, &paint);
            x -= pad * 0.5;
        }
        if let Some(added) = &added {
            let w = font.measure_str(added.as_str(), None).0;
            x -= w;
            paint.set_color(added_color);
            canvas.draw_str(added.as_str(), (x, baseline), &font, &paint);
        }
    })
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
