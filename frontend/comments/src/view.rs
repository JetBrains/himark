// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The comment CARD: the inline thread editor an annotation wears in
//! a document, its gesture command, and the small deferred commands
//! the card files. No window and no wire — provenance and channel
//! are the driver's, stamped when the announce drains.

use std::sync::Arc;

use imba::command::{Requests, Verb};
use imba::{arena::Arena, constraints::Constraints, event::{Event, EventResult, MouseButton}, store::Store, thunk_ext::ThunkExt, Thunk, ui::UiCtx, View, Widget};
use skia_safe::{Canvas, Color, Paint, Rect, Size};

use crate::AnnotationId;
use editor::document::Document;
use editor::{editor_view::EditorCommand, editor_view::EditorFocus, editor_view::EditorView, markup::Inlay, markup::InlayKey, markup::InlayMode};

type CommentChrome = editor::theme::CommentChrome;

pub const FALLBACK_WIDTH: f32 = 600.0;

/// The gesture command, WIRED: the session ceremony mints one per
/// session with the cards' collection in hand and registers it scoped
/// to the session's documents (docs/entities.md law 4) — the sibling
/// id is the instance's own record, never resolved back.
pub struct AddComment {
    pub comments: imba::store::Id<crate::Comments>,
}

impl documents::DocumentCommand for AddComment {
    fn id(&self) -> &'static str {
        "comments.add"
    }
    fn name(&self) -> String {
        "Add Comment".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        _documents: imba::store::Id<documents::OpenDocuments>,
        document_id: documents::DocumentId,
        document: &mut Document,
        editor: editor::editor::EditorId,
        location: &editor::location::ResourceLocation,
        _payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, EditorCommand>,
    ) {
        let selection = document.carets(editor).primary().selection();
        if selection.is_empty() {
            return;
        }
        let comments = self.comments;
        let fonts = editor::env::Fonts::of(store)();
        let theme = editor::env::Themes::of(store);
        let width = match document.layout_width(editor) {
            width if width > 1.0 => width,
            _ => FALLBACK_WIDTH,
        };
        let host = Some(document_id);

        // Provenance is the DRIVER's: the record is born with no
        // turn, and the announce drain stamps the session's freshest
        // one (the gesture never knew a wire). An uninstalled gate
        // still shows the card — it just never mints a record.
        let annotation = crate::installed(store).then(|| {
            let mut view = document.text().view();
            let range = documents::line_col_at(&mut view, selection.start as usize)
                ..documents::line_col_at(&mut view, selection.end as usize);
            crate::Comments::created(store, comments, location, range, String::new())
        });

        let view = CommentView::new(
            comments,
            host,
            width,
            store,
            ui,
            &fonts,
            &theme,
            annotation.clone(),
        );
        let markup = comments_markup();
        document.ensure_document_markup(markup);
        let key = document.push_inlay(
            markup,
            selection.clone(),
            Inlay::new(InlayMode::Under, view.clone()),
            store,
            ui,
            &fonts,
            &theme,
            fx,
        );
        document.swap_inlay(
            key,
            selection,
            Inlay::new(InlayMode::Under, view.keyed(key)),
        );

        document.set_focus(editor, EditorFocus::Inlay(key));
        if let (Some(id), Some(host)) = (annotation, host) {
            crate::Comments::card_born(store, comments, &id, host, key);
        }
    }
}

pub fn comments_markup() -> editor::markup::MarkupId {
    static ID: std::sync::OnceLock<editor::markup::MarkupId> = std::sync::OnceLock::new();
    *ID.get_or_init(editor::markup::MarkupId::mint)
}

pub struct RemoveComment {
    pub comments: imba::store::Id<crate::Comments>,
    pub document: documents::DocumentId,
    pub key: InlayKey,

    pub annotation: Option<AnnotationId>,
}

impl imba::command::DynamicCommand for RemoveComment {
    fn id(&self) -> &'static str {
        "comments.remove"
    }
    fn name(&self) -> String {
        "Remove Comment".to_owned()
    }
    fn perform(&self, store: &mut Store, ui: &imba::ui::UiCtx, fx: &mut imba::command::Fx<'_>) {
        if let Some(annotation) = &self.annotation {
            crate::Comments::removed(store, self.comments, annotation);
        }
        let Some(documents) = crate::Comments::documents_of(store, self.comments) else {
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
                Verb::at(
                    documents,
                    documents::DocumentsCommand::Editor(document, command),
                )
            },
            |fx| doc.remove_inlay(self.key, store, ui, &fonts, &theme, fx),
        );
        documents::OpenDocuments::put_document(store, documents, self.document, doc);
    }
}

pub struct SendComments {
    pub comments: imba::store::Id<crate::Comments>,
    pub ids: Vec<AnnotationId>,
}

impl imba::command::DynamicCommand for SendComments {
    fn id(&self) -> &'static str {
        "comments.send"
    }
    fn name(&self) -> String {
        "Send Comments to Agent".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::ui::UiCtx, _fx: &mut imba::command::Fx<'_>) {
        // A NOTE on the model — the wire lane drains it; no window,
        // no wire resolution, no drives check left to fail.
        crate::Comments::ask_send(store, self.comments, self.ids.clone());
    }
}

#[derive(Clone)]
pub enum CommentCommand {
    Editor(EditorCommand),

    Rewrap(f32),

    Remove,

    Resolve,

    Send,
}

impl std::fmt::Display for CommentCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommentCommand::Editor(command) => command.fmt(out),
            CommentCommand::Rewrap(_) => out.write_str("comment rewrap"),
            CommentCommand::Remove => out.write_str("comment remove"),
            CommentCommand::Resolve => out.write_str("comment resolve"),
            CommentCommand::Send => out.write_str("comment send"),
        }
    }
}

fn markdown_comment_document(text: text::text::Text) -> Document {
    Document::new(text, editor::markup::Markup::new()).with_syntax(
        editor::markup::Syntax::new("markdown", None, editor::markup::Markup::new()),
        &[],
    )
}

#[derive(Clone)]
pub struct CommentView {
    editor: EditorView,

    /// The collection that holds this card's thread.
    comments: imba::store::Id<crate::Comments>,

    host: Option<documents::DocumentId>,

    key: Option<InlayKey>,

    annotation: Option<AnnotationId>,

    foreign: Vec<EditorView>,

    thread_stamp: u64,

    reported_revision: u64,

    chrome: CommentChrome,
}

impl CommentView {
    fn new(
        comments: imba::store::Id<crate::Comments>,
        host: Option<documents::DocumentId>,
        width: f32,
        store: &imba::store::Store,
        ui: &imba::ui::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &editor::theme::Theme,
        annotation: Option<AnnotationId>,
    ) -> Self {
        let chrome = theme.ui().comment.clone();
        let mut editor = EditorView::of_document(
            markdown_comment_document(text::text::Text::from_string_exact("")),
            (width - chrome.pad * 2.0).max(120.0),
            store,
            ui,
            fonts,
            theme,
        );
        editor.focus_text();
        let reported_revision = editor.document.revision();
        Self {
            editor,
            comments,
            host,
            key: None,
            annotation,
            foreign: Vec::new(),
            thread_stamp: 0,
            reported_revision,
            chrome,
        }
    }

    pub(crate) fn materialized(
        comments: imba::store::Id<crate::Comments>,
        host: Option<documents::DocumentId>,
        width: f32,
        store: &imba::store::Store,
        ui: &imba::ui::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &editor::theme::Theme,
        annotation: AnnotationId,
        own_text: Option<text::text::Text>,
        foreign_texts: Vec<text::text::Text>,
        thread_stamp: u64,
    ) -> Self {
        let chrome = theme.ui().comment.clone();
        let inner = (width - chrome.pad * 2.0).max(120.0);
        let editor = EditorView::of_document(
            markdown_comment_document(
                own_text.unwrap_or_else(|| text::text::Text::from_string_exact("")),
            ),
            inner,
            store,
            ui,
            fonts,
            theme,
        );
        let foreign = foreign_texts
            .into_iter()
            .map(|text| {
                EditorView::of_document(
                    markdown_comment_document(text),
                    inner,
                    store,
                    ui,
                    fonts,
                    theme,
                )
            })
            .collect();
        let reported_revision = editor.document.revision();
        Self {
            editor,
            comments,
            host,
            key: None,
            annotation: Some(annotation),
            foreign,
            thread_stamp,
            reported_revision,
            chrome,
        }
    }

    pub(crate) fn thread_stamp(&self) -> u64 {
        self.thread_stamp
    }

    pub(crate) fn text_rope(&self) -> text::text::Text {
        self.editor.document.text().clone()
    }

    pub(crate) fn keyed(mut self, key: InlayKey) -> Self {
        self.key = Some(key);
        self
    }

    pub fn text(&self) -> String {
        let mut view = self.editor.document.text().view();
        let end = view.byte_count().min(u32::MAX as usize) as u32;
        view.substring(0..end)
    }

    pub fn parsed_markdown(&self) -> bool {
        self.editor
            .document
            .syntax()
            .is_some_and(|root| root.language == "markdown" && root.tree.is_some())
    }

    fn card_size(&self, constraints: Constraints) -> Size {
        let width = constraints.max.width.max(120.0);
        let editor_height = self
            .editor
            .content_height()
            .max(self.chrome.min_editor_height);
        let foreign: f32 = self
            .foreign
            .iter()
            .map(|entry| entry.content_height() + self.chrome.pad)
            .sum();
        Size::new(width, editor_height + foreign + self.chrome.pad * 2.0)
    }

    fn paint_card(&self, canvas: &Canvas, rect: Rect) {
        let radius = self.chrome.radius;
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(Color::from(self.chrome.surface.clone()));
        canvas.draw_round_rect(rect.with_inset((0.5, 0.5)), radius, radius, &paint);
        paint.set_stroke(true);
        paint.set_stroke_width(1.0);
        paint.set_color(Color::from(self.chrome.border.clone()));
        canvas.draw_round_rect(rect.with_inset((0.5, 0.5)), radius, radius, &paint);
    }

    fn paint_check(chrome: &CommentChrome, canvas: &Canvas, rect: Rect, resolved: bool) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_stroke(true);
        paint.set_stroke_width(1.5);
        let mut color = Color::from(chrome.cross.clone());
        if !resolved {
            color = color.with_a(color.a() / 2);
        }
        paint.set_color(color);
        let inset = rect.width() * 0.28;
        canvas.draw_line(
            (rect.left + inset, rect.top + rect.height() * 0.55),
            (rect.left + rect.width() * 0.45, rect.bottom - inset),
            &paint,
        );
        canvas.draw_line(
            (rect.left + rect.width() * 0.45, rect.bottom - inset),
            (rect.right - inset * 0.8, rect.top + inset),
            &paint,
        );
    }

    fn paint_plane(chrome: &CommentChrome, canvas: &Canvas, rect: Rect) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_stroke(true);
        paint.set_stroke_width(1.5);
        paint.set_color(Color::from(chrome.cross.clone()));
        let inset = rect.width() * 0.22;
        let nose = (rect.right - inset, rect.top + rect.height() * 0.5);
        let top = (rect.left + inset, rect.top + inset);
        let bottom = (rect.left + inset, rect.bottom - inset);
        let hinge = (
            rect.left + rect.width() * 0.42,
            rect.top + rect.height() * 0.5,
        );
        canvas.draw_line(top, nose, &paint);
        canvas.draw_line(bottom, nose, &paint);
        canvas.draw_line(top, hinge, &paint);
        canvas.draw_line(bottom, hinge, &paint);
    }

    fn paint_cross(chrome: &CommentChrome, canvas: &Canvas, rect: Rect) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_stroke(true);
        paint.set_stroke_width(1.5);
        paint.set_color(Color::from(chrome.cross.clone()));
        let inset = rect.width() * 0.3;
        canvas.draw_line(
            (rect.left + inset, rect.top + inset),
            (rect.right - inset, rect.bottom - inset),
            &paint,
        );
        canvas.draw_line(
            (rect.right - inset, rect.top + inset),
            (rect.left + inset, rect.bottom - inset),
            &paint,
        );
    }
}

impl View for CommentView {
    type Command = CommentCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, CommentCommand> {
        let own = imba::focus::FocusData::of_commands(vec![imba::PresentableCommand::new(
            "comments.remove",
            "Remove Comment",
            CommentCommand::Remove,
        )]);
        self.editor
            .focus_data(store, ui)
            .map(CommentCommand::Editor)
            .merge_under(own)
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            CommentCommand::Editor(command) => {
                if !matches!(
                    command,
                    EditorCommand::ApplyRepair(_)
                        | EditorCommand::ApplyReparse(_)
                        | EditorCommand::ApplyEnrichment(_)
                        | EditorCommand::Retheme { .. }
                        | EditorCommand::Viewport { .. }
                ) {
                    self.editor.focus_text();
                }

                fx.scope(CommentCommand::Editor, |fx| {
                    imba::View::perform(&mut self.editor, store, ui, command, fx)
                });

                if let Some(annotation) = &self.annotation {
                    let revision = self.editor.document.revision();
                    if revision != self.reported_revision {
                        self.reported_revision = revision;
                        crate::Comments::text_edited(
                            store,
                            self.comments,
                            annotation,
                            self.text_rope(),
                        );
                    }
                }
            }
            CommentCommand::Rewrap(width) => {
                let fonts = editor::env::ui_collection(store, ui);
                let theme = editor::env::Themes::of(store);
                fx.scope(CommentCommand::Editor, |fx| {
                    self.editor.document.resize(
                        self.editor.editor,
                        width,
                        0,
                        store,
                        ui,
                        &fonts,
                        &theme,
                        fx,
                    )
                });
            }
            CommentCommand::Remove => {
                let (Some(document), Some(key)) = (self.host, self.key) else {
                    return;
                };
                Requests::push(
                    store,
                    Arc::new(RemoveComment {
                        comments: self.comments,
                        document,
                        key,
                        annotation: self.annotation.clone(),
                    }),
                );
            }
            CommentCommand::Send => {
                let Some(annotation) = &self.annotation else {
                    return;
                };
                Requests::push(
                    store,
                    Arc::new(SendComments {
                        comments: self.comments,
                        ids: vec![annotation.clone()],
                    }),
                );
            }
            CommentCommand::Resolve => {
                let Some(annotation) = &self.annotation else {
                    return;
                };
                let resolved = crate::Comments::record(store, self.comments, annotation)
                    .map(|record| record.resolved)
                    .unwrap_or(false);
                crate::Comments::resolve(store, self.comments, annotation, !resolved);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = self.card_size(constraints);
            let mut container = imba::container::container(arena, size);

            let pad = self.chrome.pad;
            let want = (size.width - pad * 2.0).max(120.0);
            let editor_height = self
                .editor
                .content_height()
                .max(self.chrome.min_editor_height);
            let editor = imba::layout::Layout::layout(
                self.editor.display(arena, store, ui),
                arena,
                Constraints {
                    min: Size::new(want, editor_height),
                    max: Size::new(want, f32::MAX),
                },
            )
            .map(CommentCommand::Editor);
            container.place(pad, pad, editor);

            let mut y = pad + editor_height + pad;
            for entry in &self.foreign {
                let height = entry.content_height();
                let laid = imba::layout::Layout::layout(
                    entry.display(arena, store, ui),
                    arena,
                    Constraints {
                        min: Size::new(want, height),
                        max: Size::new(want, f32::MAX),
                    },
                );
                container.place(pad, y, ReadOnly(laid));
                y += height + pad;
            }

            let close = self.chrome.close_size;
            let chrome = &self.chrome;
            let cross = imba::leaf::leaf(close, close)
                .paint_instead(move |_arena, canvas, rect| Self::paint_cross(chrome, canvas, rect))
                .event(|_arena, event, _size| match event {
                    Event::MouseDown {
                        button: MouseButton::Left,
                        ..
                    } => EventResult::Command(CommentCommand::Remove),
                    _ => EventResult::Ignored,
                });
            container.place(size.width - close - 6.0, 6.0, cross);

            if self.annotation.is_some() {
                let resolved = self
                    .annotation
                    .as_ref()
                    .and_then(|id| crate::Comments::record(store, self.comments, id))
                    .map(|record| record.resolved)
                    .unwrap_or(false);
                let check = imba::leaf::leaf(close, close)
                    .paint_instead(move |_arena, canvas, rect| {
                        Self::paint_check(chrome, canvas, rect, resolved)
                    })
                    .event(|_arena, event, _size| match event {
                        Event::MouseDown {
                            button: MouseButton::Left,
                            ..
                        } => EventResult::Command(CommentCommand::Resolve),
                        _ => EventResult::Ignored,
                    });
                container.place(size.width - close * 2.0 - 12.0, 6.0, check);
                let plane = imba::leaf::leaf(close, close)
                    .paint_instead(move |_arena, canvas, rect| {
                        Self::paint_plane(chrome, canvas, rect)
                    })
                    .event(|_arena, event, _size| match event {
                        Event::MouseDown {
                            button: MouseButton::Left,
                            ..
                        } => EventResult::Command(CommentCommand::Send),
                        _ => EventResult::Ignored,
                    });
                container.place(size.width - close * 3.0 - 18.0, 6.0, plane);
            }

            let stale = (self.editor.layout_width() - want).abs() > 1.0;
            container
                .paint_below(move |_arena, canvas, _rect| {
                    self.paint_card(canvas, Rect::from_size(size))
                })
                .wrap(move |inner| RewrapOnPaint { stale, want, inner })
        })
    }
}

struct ReadOnly<Inner>(Inner);

impl<'a, Inner: Thunk<'a, EditorCommand> + 'a> Thunk<'a, CommentCommand> for ReadOnly<Inner> {
    fn size(&self) -> Size {
        self.0.size()
    }

    fn realize(
        self,
        arena: &'a imba::arena::Arena,
        viewport: Rect,
    ) -> imba::WidgetBox<'a, CommentCommand> {
        imba::WidgetBox::new(arena, ReadOnly(self.0.realize(arena, viewport)))
    }
}

impl<'a, Inner: Widget<'a, EditorCommand>> Widget<'a, CommentCommand> for ReadOnly<Inner> {
    fn size(&self) -> Size {
        self.0.size()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<CommentCommand> {
        match self.0.handle_event(arena, event, viewport) {
            EventResult::Ignored => EventResult::Ignored,

            _ => EventResult::Handled,
        }
    }
}

struct RewrapOnPaint<Inner> {
    stale: bool,
    want: f32,
    inner: Inner,
}

impl<'a, Inner: Widget<'a, CommentCommand>> Widget<'a, CommentCommand> for RewrapOnPaint<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, CommentCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<CommentCommand> {
        let result = self.inner.handle_event(arena, event, viewport);
        if self.stale
            && matches!(event, Event::Paint { .. })
            && matches!(result, EventResult::Ignored | EventResult::Handled)
        {
            return EventResult::Command(CommentCommand::Rewrap(self.want));
        }
        result
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, CommentCommand>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}
