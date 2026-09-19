// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    arena::Arena,
    constraints::Constraints,
    container::{container, Container},
    effect::Effects,
    event::{Event, EventResult, MouseButton},
    lazy::lazy,
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View, Widget,
};
use skia_safe::{Point, Rect, Size};

use crate::{
    document::Document,
    editor::EditorId,
    markup::{inlay_anchor_byte, inlay_anchors_line, InlayCommand, InlayMode},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditorFocus {
    Text,

    Inlay(crate::markup::InlayKey),
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    Left,
    Right,

    WordLeft,
    WordRight,

    Up,
    Down,

    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClickKind {
    Set,

    Extend,

    Add,

    Word,

    Line,
}

pub enum EditorCommand {
    InsertText {
        text: String,
    },

    Enter {
        soft: bool,
    },

    Indent,
    Outdent,
    Backspace,

    DeleteForward,

    DeleteWordBack,
    DeleteWordForward,

    DeleteSelections,

    Paste {
        text: String,
    },

    Undo,
    Redo,

    Move {
        motion: Motion,
        select: bool,
    },

    SelectAll,

    CollapseCarets,

    AddCaretAbove,
    AddCaretBelow,

    SelectNextOccurrence,

    SelectAllOccurrences,
    Click {
        point: skia_safe::Point,
        kind: ClickKind,
    },

    Drag {
        point: skia_safe::Point,
    },

    DragEnd,

    RevealSettled,

    RevealAt {
        byte: u32,
    },

    Dynamic {
        id: &'static str,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
    },

    ToggleFold {
        range: std::ops::Range<u32>,
    },

    ToggleBeforeInlay {
        at: u32,
    },
    Inlay {
        key: crate::markup::InlayKey,
        command: InlayCommand,
    },

    Viewport {
        width: f32,
        top: f32,
        bottom: f32,
        anchor: u32,
    },

    /// The widget re-observed its viewport top on a traversal — the
    /// settle pulse, or paint (docs/editor/viewport-preservation.md §3.1).
    /// Refreshes the retained viewport and supersedes any pending
    /// correction; the full `Viewport` report stays throttled.
    ViewportTop(f32),

    ToggleSoftwrap,

    HorizontalScroll(f32),

    Hover(Option<Point>),

    Retheme {
        top: f32,
        bottom: f32,
        anchor: u32,
    },

    ApplyRepair(Vec<crate::repair::RepairedLayout>),

    ApplyReparse(crate::reparse::ReparseOutcome),

    ApplyEnrichment(crate::enrich::EnrichOutcome),

    ApplyScrollStripes(crate::scroll_stripe::StripeOutcome),

    InsertTextReplacing {
        text: String,
        replacement: (u32, u32),
    },

    SetMarkedText {
        text: String,
        selected: (u32, u32),
        replacement: Option<(u32, u32)>,
    },
    UnmarkText,

    SetSelectionUtf16 {
        start: u32,
        end: u32,
    },
}

#[derive(Clone)]
enum WidgetFonts {
    Ready(skia_safe::textlayout::FontCollection),
    Lazy(crate::FontSource),
}

impl WidgetFonts {
    fn resolve(store: &Store, ui: &UiCtx) -> Self {
        match ui.get::<crate::env::UiFonts>() {
            Some(fonts) => Self::Ready(fonts.0.clone()),
            None => Self::Lazy(crate::env::Fonts::of(store)),
        }
    }

    fn collection(&self) -> skia_safe::textlayout::FontCollection {
        match self {
            Self::Ready(collection) => collection.clone(),
            Self::Lazy(source) => source(),
        }
    }
}

pub(crate) struct SharedViewport<'a> {
    document: &'a crate::document::Document,
    editor: crate::editor::EditorId,
    fonts: WidgetFonts,
    theme: crate::theme::Theme,

    number_lines: bool,

    stripes: Option<crate::diff::DiffId>,

    /// Selections-visible, decided FROM STATE at display time (the
    /// frame's focused seat vs this editor's) — so the one build is
    /// final and paint never re-derives it.
    focused: bool,
    cell: std::cell::RefCell<Option<crate::viewport::EditorViewport>>,
}

impl<'a> SharedViewport<'a> {
    /// The viewport is derived ONCE per frame and serves everyone —
    /// the gutter, the core paint, the inlay placement, the projected
    /// overlays. Focus is baked in from STATE at construction, so
    /// there is no focused "upgrade": the realize-time build is the
    /// build paint reads.
    fn viewport(
        &self,
        band: std::ops::Range<f32>,
    ) -> std::cell::Ref<'_, crate::viewport::EditorViewport> {
        if self.cell.borrow().is_none() {
            *self.cell.borrow_mut() = Some(crate::viewport::EditorViewport::build(
                self.document,
                self.editor,
                band,
                self.focused,
                self.number_lines,
                self.stripes,
                &self.fonts.collection(),
                &self.theme,
            ));
        }
        std::cell::Ref::map(self.cell.borrow(), |cell| {
            cell.as_ref().expect("just built")
        })
    }
}

struct EditorCoreView<'a> {
    shared: std::rc::Rc<SharedViewport<'a>>,
    size: Size,

    surface: imba::event::ScrollSurfaceId,

    target_width: f32,

    reports_geometry: bool,

    location: Option<&'a crate::ResourceLocation>,
}

struct EditorGutterView<'a> {
    shared: std::rc::Rc<SharedViewport<'a>>,
    size: Size,
}

pub(crate) fn gutter_font(size: f32) -> skia_safe::Font {
    static TYPEFACE: std::sync::OnceLock<skia_safe::Typeface> = std::sync::OnceLock::new();
    let typeface = TYPEFACE.get_or_init(|| {
        skia_safe::FontMgr::new()
            .legacy_make_typeface(None, skia_safe::FontStyle::normal())
            .expect("a system typeface")
    });
    let mut font = skia_safe::Font::from_typeface(typeface.clone(), size);
    font.set_edging(skia_safe::font::Edging::AntiAlias);
    font
}

impl<'a> Widget<'a, EditorCommand> for EditorGutterView<'a> {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<EditorCommand> {
        match event {
            Event::Paint { canvas, focused: _ } => {
                let chrome = self.shared.theme.ui().editor_gutter.clone();
                let data = self.shared.viewport(viewport.top..viewport.bottom);
                let font = gutter_font(chrome.number_size);
                let (_, metrics) = font.metrics();
                let mut paint = skia_safe::Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(chrome.number_color.0);

                let fold_right = self.size.width - chrome.pad;
                let right = fold_right - chrome.fold_size;
                let mut digits = String::with_capacity(8);

                for line in &data.lines {
                    if let Some(kind) = line.diff {
                        use crate::viewport::DiffLineKind;
                        let mut stripe = skia_safe::Paint::default();
                        stripe.set_anti_alias(true);
                        match kind {
                            DiffLineKind::Added | DiffLineKind::Modified => {
                                stripe.set_color(match kind {
                                    DiffLineKind::Added => chrome.stripe_added.0,
                                    _ => chrome.stripe_modified.0,
                                });
                                canvas.draw_rect(
                                    skia_safe::Rect::from_xywh(
                                        chrome.stripe_inset,
                                        line.top,
                                        chrome.stripe_width,
                                        line.height,
                                    ),
                                    &stripe,
                                );
                            }
                            DiffLineKind::DeletedAbove => {
                                stripe.set_color(chrome.stripe_deleted.0);
                                let size = chrome.stripe_width * 2.0;
                                let mut path = skia_safe::PathBuilder::new();
                                path.move_to((chrome.stripe_inset, line.top - size * 0.5));
                                path.line_to((chrome.stripe_inset, line.top + size * 0.5));
                                path.line_to((chrome.stripe_inset + size, line.top));
                                path.close();
                                canvas.draw_path(&path.detach(), &stripe);
                            }
                        }
                    }

                    let baseline = line.baseline.unwrap_or_else(|| {
                        line.text_top + (line.height - (metrics.descent - metrics.ascent)) * 0.5
                            - metrics.ascent
                    });
                    if let Some(number) = line.hard_line {
                        digits.clear();
                        use std::fmt::Write;
                        let _ = write!(digits, "{number}");
                        let width = font.measure_str(&digits, Some(&paint)).0;
                        canvas.draw_str(&digits, (right - width, baseline), &font, &paint);
                    }

                    if let Some(foldable) = &line.foldable {
                        let mut stroke = skia_safe::Paint::default();
                        stroke.set_anti_alias(true);
                        stroke.set_color(chrome.number_color.0);
                        stroke.set_style(skia_safe::paint::Style::Stroke);
                        stroke.set_stroke_width(2.0);
                        let cx = fold_right - chrome.fold_size * 0.5;
                        let cy = baseline + metrics.ascent * 0.35;
                        let arm = chrome.fold_size * 0.22;
                        let mut path = skia_safe::PathBuilder::new();
                        path.move_to((cx - arm, cy - arm * 0.6));
                        path.line_to((cx, cy + arm * 0.8));
                        path.line_to((cx + arm, cy - arm * 0.6));
                        canvas.save();
                        canvas.rotate(-90.0 * foldable.spin, Some(skia_safe::Point::new(cx, cy)));
                        canvas.draw_path(&path.detach(), &stroke);
                        canvas.restore();
                    }
                }
                EventResult::Handled
            }

            Event::MouseDown {
                button: MouseButton::Left,
                point,
                ..
            } => {
                let chrome = self.shared.theme.ui().editor_gutter.clone();
                let band = viewport.top..viewport.bottom;
                if self.shared.stripes.is_some()
                    && point.x <= chrome.stripe_inset + chrome.stripe_width * 2.0
                {
                    if let Some(at) = self.row_start_at(band.clone(), point.y) {
                        return EventResult::Command(EditorCommand::ToggleBeforeInlay { at });
                    }
                }
                match self.foldable_at(band, point.y) {
                    Some(range) => EventResult::Command(EditorCommand::ToggleFold { range }),
                    None => EventResult::Ignored,
                }
            }
            _ => EventResult::Ignored,
        }
    }
}

impl EditorGutterView<'_> {
    /// The line under a gutter click — a query on the frame's
    /// viewport (a line's extent covers its spacer above, like the
    /// layout cursor's).
    fn line_hit<R>(
        &self,
        band: std::ops::Range<f32>,
        y: f32,
        pick: impl FnOnce(&crate::viewport::ViewportLine) -> R,
    ) -> Option<R> {
        let data = self.shared.viewport(band);
        data.lines
            .iter()
            .find(|line| y < line.top + line.height)
            .map(pick)
    }

    fn row_start_at(&self, band: std::ops::Range<f32>, y: f32) -> Option<u32> {
        self.line_hit(band, y, |line| line.byte_start)
    }

    fn foldable_at(&self, band: std::ops::Range<f32>, y: f32) -> Option<std::ops::Range<u32>> {
        self.line_hit(band, y, |line| {
            line.foldable
                .as_ref()
                .map(|foldable| foldable.range.clone())
        })
        .flatten()
    }
}

#[derive(Clone)]
pub struct EditorView {
    pub document: Document,
    pub editor: EditorId,

    pub reports_geometry: bool,

    pub location: Option<crate::ResourceLocation>,

    pub gutter_width: f32,

    pub base: Option<(Document, crate::diff::DiffId)>,
}

impl EditorView {
    pub fn of_document(
        mut document: Document,
        width: f32,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Self {
        let mut discarded = imba::effect::Batch::new();
        let editor = document.add_editor(
            width,
            None,
            crate::document::EditorBuild::Bounded,
            &[],
            fonts,
            theme,
            &mut discarded.effects(),
        );
        Self {
            document,
            editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        }
    }

    pub fn complete(
        mut document: Document,
        width: f32,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Self {
        let mut discarded = imba::effect::Batch::new();
        let editor = document.add_editor(
            width,
            None,
            crate::document::EditorBuild::Complete,
            &[],
            fonts,
            theme,
            &mut discarded.effects(),
        );
        Self {
            document,
            editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        }
    }

    pub fn input(width: f32, fonts: crate::FontSource) -> Self {
        let mut markup = crate::markup::Markup::new();
        markup.push_styled_covering(0..0, crate::theme::StyleId::Input);

        Self::of_document(
            Document::new(text::Text::from_string_exact(""), markup),
            width,
            &fonts(),
            &crate::theme::Theme::embedded(),
        )
    }

    pub fn caret_byte(&self) -> u32 {
        self.document.caret_byte(self.editor)
    }

    pub fn set_caret(&mut self, byte: u32) {
        self.document.set_caret(self.editor, byte);
    }

    pub fn focus(&self) -> EditorFocus {
        self.document.focus(self.editor)
    }

    pub fn blur(&mut self) {
        self.document.set_focus(self.editor, EditorFocus::None);
    }

    pub fn focus_text(&mut self) {
        self.document.set_focus(self.editor, EditorFocus::Text);
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<std::sync::Arc<str>>,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) {
        self.document
            .set_placeholder(self.editor, placeholder, fonts, theme);
    }

    pub fn content_height(&self) -> f32 {
        self.document.content_height(self.editor)
    }

    pub fn layout_width(&self) -> f32 {
        self.document.layout_width(self.editor)
    }

    pub fn document_layout(&self) -> &crate::DocumentLayout {
        self.document
            .document_layout(self.editor)
            .expect("the bound editor exists")
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn element_heights(&self) -> Vec<(u32, f32)> {
        self.document.element_heights(self.editor)
    }

    pub fn reveal_caret(
        &mut self,
        byte: u32,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) {
        self.document.set_caret(self.editor, byte);
        self.document.refresh_unhide(self.editor, fonts, theme);
    }

    pub fn find_misaligned_boundary(&self) -> Option<u32> {
        self.document.find_misaligned_boundary(self.editor)
    }

    /// Places the inline inlay widgets by QUERYING the frame's
    /// `EditorViewport` — the walk over the document layout happened
    /// once, in `EditorViewport::build`.
    fn place_visible_inlays<'a>(
        &'a self,
        editor_id: crate::editor::EditorId,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        fonts: &WidgetFonts,
        editor: &mut Container<'a, EditorCommand>,
        data: &crate::viewport::EditorViewport,
    ) {
        let fonts = fonts.collection();
        let theme = &crate::env::Themes::of(store);
        let acting = self.document.editor(editor_id);
        let extras = self.document.extras_keyed(editor_id);
        let markups = crate::markup::OverlaidMarkup::new(self.document.markup(), &extras);
        if !markups.has_inlays() {
            return;
        }

        let constraints = Constraints {
            min: Size::default(),
            max: Size::new(data.layout_width.max(1.0), f32::MAX),
        };
        for line in &data.lines {
            let line_range = line.byte_start..line.byte_end;
            let hits = markups.all_inlays_in(line_range.clone());
            if hits.is_empty() {
                continue;
            }
            let line_top = line.top;
            let content_top = line.text_top;
            let content_height = line.inlays.content_height_from_total(line.height);
            let mut above_y = line_top;
            let mut under_y = content_top + content_height;
            let line_start_x = line.x;
            let mut positioner = None;

            for interval in &hits {
                if !inlay_anchors_line(interval.inlay.mode, &interval.range, &line_range) {
                    continue;
                }

                if matches!(interval.inlay.mode, InlayMode::Popup(_)) {
                    continue;
                }
                let widget = interval.inlay.layout(arena, store, ui, constraints);
                let size = widget.size();
                let y_centered = content_top + (content_height - size.height).max(0.0) * 0.5;
                let (x, y) = match interval.inlay.mode {
                    InlayMode::Left
                    | InlayMode::Right
                    | InlayMode::Instead(crate::markup::InsteadKind::Inline) => {
                        let byte = inlay_anchor_byte(interval.inlay.mode, &interval.range);
                        let positioner = positioner.get_or_insert_with(|| {
                            self.document.shape_line(
                                editor_id,
                                line_range.clone(),
                                0.0,
                                true,
                                &fonts,
                                theme,
                            )
                        });
                        positioner
                            .placeholder_rect_at_byte(byte)
                            .map(|rect| (rect.left, content_top + rect.top))
                            .unwrap_or_else(|| (positioner.x_at_byte(byte), y_centered))
                    }
                    InlayMode::Above => {
                        let y = above_y;
                        above_y += size.height;
                        (line_start_x, y)
                    }
                    InlayMode::Under => {
                        let y = under_y;
                        under_y += size.height;
                        (line_start_x, y)
                    }
                    InlayMode::Instead(crate::markup::InsteadKind::FullLine) => {
                        let positioner = positioner.get_or_insert_with(|| {
                            self.document.shape_line(
                                editor_id,
                                line_range.clone(),
                                0.0,
                                true,
                                &fonts,
                                theme,
                            )
                        });
                        let anchor_x = positioner
                            .x_at_byte(inlay_anchor_byte(interval.inlay.mode, &interval.range));
                        (anchor_x, y_centered)
                    }

                    InlayMode::Popup(_) => unreachable!("popups mint overlays"),
                };

                let key = interval.key;
                if interval.inlay.overlay.is_some() {
                    // Host-targeting inlays render through the overlay
                    // pass (`projected_overlays`) — the flow keeps a
                    // hole in their place, slots above/under stay
                    // bookkept.
                    continue;
                }
                let focused = acting.focus == EditorFocus::Inlay(key);
                editor.place(
                    x,
                    y,
                    widget
                        .map(move |command| EditorCommand::Inlay { key, command })
                        .wrap(move |inner| FocusGate { inner, focused }),
                );
            }
        }
    }
}

impl View for EditorView {
    type Command = EditorCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, EditorCommand> {
        use imba::event::EventResult;
        use imba::focus::FocusData;
        let inner = match self.document.focus(self.editor) {
            EditorFocus::Inlay(key) => {
                let wrap = move |command| EditorCommand::Inlay { key, command };
                let (commands, location, seat) =
                    match self.document.inlay_focus_data(store, ui, key) {
                        Some(mut data) => (
                            std::mem::take(&mut data.commands)
                                .into_iter()
                                .map(|presentable| presentable.map(wrap))
                                .collect(),
                            data.location.take(),
                            data.seat.take(),
                        ),
                        None => (Vec::new(), None, None),
                    };
                let with_inlay = move |f: &mut dyn FnMut(
                    imba::focus::FocusData<'_, crate::markup::InlayCommand>,
                ) -> EventResult<
                    crate::markup::InlayCommand,
                >| {
                    self.document
                        .inlay_focus_data(store, ui, key)
                        .map(|data| f(data))
                        .unwrap_or(EventResult::Ignored)
                        .map(wrap)
                };
                FocusData {
                    commands,
                    on_key: Some(Box::new(move |k, mods| {
                        with_inlay(&mut |mut data| data.key(k, mods))
                    })),
                    on_text: Some(Box::new(move |text| {
                        with_inlay(&mut |mut data| data.text(text))
                    })),
                    clipboard: Some(Box::new(move |visit| {
                        with_inlay(&mut |mut data| match data.clipboard.as_mut() {
                            Some(seat) => seat(visit),
                            None => EventResult::Ignored,
                        })
                    })),
                    location,
                    seat,
                }
            }
            EditorFocus::Text => FocusData {
                commands: self.caret_surface(),

                on_key: None,
                on_text: Some(Box::new(move |text| {
                    EventResult::Command(EditorCommand::InsertText {
                        text: text.to_owned(),
                    })
                })),

                clipboard: Some(Box::new(move |visit| {
                    let mut client = EditorClipboardClient {
                        document: &self.document,
                        editor: self.editor,
                        command: None,
                    };
                    visit(&mut client);
                    match client.command {
                        Some(command) => EventResult::Command(command),
                        None => EventResult::Handled,
                    }
                })),

                location: self
                    .location
                    .as_ref()
                    .map(|location| Box::new(location.clone()) as Box<dyn std::any::Any>),

                seat: self.document.seat_key(self.editor),
            },
            EditorFocus::None => FocusData::default(),
        };
        // Standing popups (completion, hover actions) filter the
        // keyboard before the text they decorate — semantic state,
        // not z-order: they exist, so they answer first.
        let inner = {
            let mut over = FocusData::default();
            for (key, _, _, _) in self.document.popups_in(self.editor, 0..u32::MAX) {
                if let Some(data) = self.document.inlay_focus_data(store, ui, key) {
                    over = over
                        .merge_over(data.map(move |command| EditorCommand::Inlay { key, command }));
                }
            }
            over.merge_over(inner)
        };
        inner.merge_under(FocusData::of_commands(self.dynamic_surface(store)))
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        if let EditorCommand::ToggleBeforeInlay { at } = command {
            if let Some((base, diff)) = &self.base {
                let fonts = crate::env::ui_collection(store, ui);
                let theme = crate::env::Themes::of(store);
                self.document.toggle_before_inlay(
                    self.editor,
                    at,
                    base,
                    *diff,
                    true,
                    &fonts,
                    &theme,
                    fx,
                );
            }
            return;
        }

        if let EditorCommand::Dynamic { id, payload } = command {
            let Some(location) = self.location.clone() else {
                return;
            };
            let Some(entry) = crate::dynamic::EditorCommands::of(store).find(id).cloned() else {
                return;
            };
            return entry.perform(
                store,
                &mut self.document,
                self.editor,
                &location,
                payload,
                fx,
            );
        }

        if let EditorCommand::ApplyReparse(outcome) = command {
            let fonts = crate::env::ui_collection(store, ui);
            let theme = crate::env::Themes::of(store);
            self.document
                .land_reparse(outcome, self.location.clone(), store, &fonts, &theme, fx);
            return;
        }
        let base_revision = self.document.revision();
        let text_before = self.document.text().clone();
        self.document.perform(store, ui, self.editor, command, fx);

        if self.document.revision() != base_revision {
            if let Some(location) = self.location.as_ref().filter(|l| !l.is_synthetic()) {
                for sink in crate::change_sink::InstalledChangeSink::of(store) {
                    sink.changed(
                        store,
                        &self.document,
                        location,
                        base_revision,
                        &text_before,
                        fx,
                    );
                }
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
            let document = &self.document;
            let editor_id = self.editor;
            let fonts = WidgetFonts::resolve(store, ui);
            let gutter = self.gutter_width.max(0.0);

            let stripes = self.base.as_ref().map(|(_, id)| *id);

            let target_width = constraints.max.width - gutter;
            let softwrap = document.softwrap(editor_id);
            let core_height = document
                .content_height(editor_id)
                .max(constraints.min.height);

            let core_size = match softwrap {
                true => Size::new(
                    document
                        .layout_width(editor_id)
                        .max(constraints.min.width - gutter),
                    core_height,
                ),
                false => Size::new(document.max_width(editor_id).max(target_width), core_height),
            };
            let size = match softwrap {
                true => Size::new(core_size.width + gutter, core_size.height),
                false => Size::new(gutter + target_width.max(0.0), core_size.height),
            };
            let focused = match crate::env::FrameFocus::of(store) {
                Some(seat) => document.seat_key(editor_id) == Some(seat),
                None => document.focus(editor_id) == EditorFocus::Text,
            };
            lazy(size, move |viewport| {
                let shared = std::rc::Rc::new(SharedViewport {
                    document,
                    editor: editor_id,
                    fonts: fonts.clone(),
                    theme: crate::env::Themes::of(store),
                    number_lines: gutter > 0.0,
                    stripes,
                    focused,
                    cell: std::cell::RefCell::new(None),
                });
                let mut root = container(arena, size);
                if gutter > 0.0 {
                    root.place(
                        0.0,
                        0.0,
                        imba::eager(EditorGutterView {
                            shared: shared.clone(),
                            size: Size::new(gutter, size.height),
                        }),
                    );
                }

                let mut core = container(arena, core_size);
                core.place(
                    0.0,
                    0.0,
                    imba::eager(EditorCoreView {
                        shared: shared.clone(),
                        size: core_size,
                        surface: imba::event::ScrollSurfaceId::keyed(editor_id.surface_key()),
                        target_width,
                        reports_geometry: self.reports_geometry,
                        location: self.location.as_ref(),
                    }),
                );
                // The frame's viewport, derived once — the gutter and
                // the core paints reuse this same build.
                let data = shared.viewport(viewport.top..viewport.bottom);
                self.place_visible_inlays(editor_id, arena, store, ui, &fonts, &mut core, &data);
                // The text origin's horizontal pan: softwrap never
                // pans; otherwise the core slides left by the clamped
                // horizontal scroll inside a clipping window. Popups
                // and overlays anchor from the same origin.
                let scroll_x = match softwrap {
                    true => 0.0,
                    false => document
                        .scroll_x(editor_id)
                        .min((core_size.width - target_width).max(0.0)),
                };
                match softwrap {
                    true => root.place(gutter, 0.0, core),
                    false => {
                        let mut window =
                            container(arena, Size::new(target_width.max(0.0), size.height));
                        window.place(-scroll_x, 0.0, core);
                        root.place(gutter, 0.0, window);
                    }
                }

                let popup_origin = skia_safe::Point::new(gutter - scroll_x, 0.0);
                let mut popups = crate::popup::visible_popups(
                    &document,
                    editor_id,
                    &fonts.collection(),
                    &crate::env::Themes::of(store),
                    arena,
                    store,
                    ui,
                    viewport,
                    popup_origin,
                );
                popups.extend(crate::popup::projected_overlays(
                    &document,
                    editor_id,
                    arena,
                    store,
                    ui,
                    &data,
                    popup_origin,
                ));

                if gutter > 0.0 {
                    popups.extend(crate::sticky::sticky_overlays(
                        &document,
                        editor_id,
                        &fonts.collection(),
                        &crate::env::Themes::of(store),
                        arena,
                        viewport,
                        popup_origin.x,
                        gutter,
                        size.width,
                    ));
                }
                EditorChain {
                    inner: root.realize_into(viewport),
                    view: self,
                    fonts: fonts.clone(),
                    theme: crate::env::Themes::of(store),
                    popups,
                }
            })
        })
    }
}

impl EditorView {
    pub fn scroll_stripe_overlays<'a>(
        &self,
        arena: &'a imba::arena::Arena,
        store: &'a Store,
        viewport: Rect,
    ) -> Vec<imba::overlay::Overlay<'a, EditorCommand>> {
        if self.gutter_width <= 0.0 {
            return Vec::new();
        }
        crate::scroll_stripe::scroll_stripe_overlays(
            &self.document,
            self.editor,
            &crate::env::Themes::of(store),
            arena,
            viewport,
        )
    }

    fn caret_surface(&self) -> Vec<imba::PresentableCommand<EditorCommand>> {
        use imba::PresentableCommand;
        {
            {
                let mut commands = vec![
                    PresentableCommand::new("editor.undo", "Undo", EditorCommand::Undo),
                    PresentableCommand::new("editor.redo", "Redo", EditorCommand::Redo),
                    PresentableCommand::new(
                        "editor.select-all",
                        "Select All",
                        EditorCommand::SelectAll,
                    ),
                    PresentableCommand::new(
                        "editor.toggle-softwrap",
                        "Toggle Soft Wrap",
                        EditorCommand::ToggleSoftwrap,
                    ),
                    PresentableCommand::new(
                        "editor.select-next-occurrence",
                        "Select Next Occurrence",
                        EditorCommand::SelectNextOccurrence,
                    ),
                    PresentableCommand::new(
                        "editor.select-all-occurrences",
                        "Select All Occurrences",
                        EditorCommand::SelectAllOccurrences,
                    ),
                    PresentableCommand::new(
                        "editor.add-caret-above",
                        "Add Caret Above",
                        EditorCommand::AddCaretAbove,
                    ),
                    PresentableCommand::new(
                        "editor.add-caret-below",
                        "Add Caret Below",
                        EditorCommand::AddCaretBelow,
                    ),
                    PresentableCommand::new(
                        "editor.backspace",
                        "Delete Backward",
                        EditorCommand::Backspace,
                    ),
                    PresentableCommand::new(
                        "editor.delete-forward",
                        "Delete Forward",
                        EditorCommand::DeleteForward,
                    ),
                    PresentableCommand::new(
                        "editor.delete-word-back",
                        "Delete Word Backward",
                        EditorCommand::DeleteWordBack,
                    ),
                    PresentableCommand::new(
                        "editor.delete-word-forward",
                        "Delete Word Forward",
                        EditorCommand::DeleteWordForward,
                    ),
                    PresentableCommand::new(
                        "editor.newline",
                        "Insert Newline",
                        EditorCommand::Enter { soft: false },
                    ),
                    PresentableCommand::new(
                        "editor.newline-soft",
                        "Insert Line Break",
                        EditorCommand::Enter { soft: true },
                    ),
                    PresentableCommand::new("editor.indent", "Indent", EditorCommand::Indent),
                    PresentableCommand::new("editor.outdent", "Outdent", EditorCommand::Outdent),
                ];

                for (id, name, motion, select) in [
                    ("editor.move-left", "Move Left", Motion::Left, false),
                    ("editor.select-left", "Select Left", Motion::Left, true),
                    ("editor.move-right", "Move Right", Motion::Right, false),
                    ("editor.select-right", "Select Right", Motion::Right, true),
                    ("editor.move-up", "Move Up", Motion::Up, false),
                    ("editor.select-up", "Select Up", Motion::Up, true),
                    ("editor.move-down", "Move Down", Motion::Down, false),
                    ("editor.select-down", "Select Down", Motion::Down, true),
                    (
                        "editor.move-word-left",
                        "Move Word Left",
                        Motion::WordLeft,
                        false,
                    ),
                    (
                        "editor.select-word-left",
                        "Select Word Left",
                        Motion::WordLeft,
                        true,
                    ),
                    (
                        "editor.move-word-right",
                        "Move Word Right",
                        Motion::WordRight,
                        false,
                    ),
                    (
                        "editor.select-word-right",
                        "Select Word Right",
                        Motion::WordRight,
                        true,
                    ),
                    (
                        "editor.move-line-start",
                        "Move to Line Start",
                        Motion::LineStart,
                        false,
                    ),
                    (
                        "editor.select-line-start",
                        "Select to Line Start",
                        Motion::LineStart,
                        true,
                    ),
                    (
                        "editor.move-line-end",
                        "Move to Line End",
                        Motion::LineEnd,
                        false,
                    ),
                    (
                        "editor.select-line-end",
                        "Select to Line End",
                        Motion::LineEnd,
                        true,
                    ),
                    (
                        "editor.move-doc-start",
                        "Move to Document Start",
                        Motion::DocumentStart,
                        false,
                    ),
                    (
                        "editor.select-doc-start",
                        "Select to Document Start",
                        Motion::DocumentStart,
                        true,
                    ),
                    (
                        "editor.move-doc-end",
                        "Move to Document End",
                        Motion::DocumentEnd,
                        false,
                    ),
                    (
                        "editor.select-doc-end",
                        "Select to Document End",
                        Motion::DocumentEnd,
                        true,
                    ),
                ] {
                    commands.push(PresentableCommand::new(
                        id,
                        name,
                        EditorCommand::Move { motion, select },
                    ));
                }
                let carets = self.document.carets(self.editor);
                if carets.len() > 1 || carets.has_selection() {
                    commands.push(PresentableCommand::new(
                        "editor.collapse-carets",
                        "Collapse to Single Caret",
                        EditorCommand::CollapseCarets,
                    ));
                }
                commands
            }
        }
    }

    fn dynamic_surface(&self, store: &Store) -> Vec<imba::PresentableCommand<EditorCommand>> {
        let mut commands = Vec::new();
        if let Some(location) = &self.location {
            for entry in crate::dynamic::EditorCommands::of(store).iter() {
                if !entry.offers_at(location) {
                    continue;
                }
                commands.push(imba::PresentableCommand::new(
                    entry.id(),
                    entry.name(),
                    EditorCommand::Dynamic {
                        id: entry.id(),
                        payload: None,
                    },
                ));
            }
        }
        commands
    }
}

struct EditorChain<'a> {
    inner: imba::container::RealizedContainer<'a, EditorCommand>,
    view: &'a EditorView,
    fonts: WidgetFonts,
    theme: crate::theme::Theme,

    popups: Vec<imba::overlay::Overlay<'a, EditorCommand>>,
}

impl<'a> imba::Widget<'a, EditorCommand> for EditorChain<'a> {
    fn size(&self) -> Size {
        imba::Widget::size(&self.inner)
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &imba::event::Event<'_>,
        viewport: Rect,
    ) -> imba::event::EventResult<EditorCommand> {
        self.inner.handle_event(arena, event, viewport)
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, EditorCommand>> {
        let mut overlays = self.inner.overlays();
        overlays.append(&mut self.popups);
        overlays
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, EditorCommand>
    where
        'a: 'w,
    {
        use imba::event::EventResult;
        use imba::focus::LayoutData;
        let view = self.view;
        let bounds = imba::Widget::size(&self.inner);

        // RECOGNITION, not routing: this editor answers only when
        // the target names ITS seat; anything else forwards into the
        // fold (nested inlay editors carry their own keys).
        match view.document.seat_key(view.editor) == Some(target) {
            false => self.inner.layout_data(target),
            true => {
                let fonts = &self.fonts;
                let theme = &self.theme;
                LayoutData {
                    ime: Some(imba::focus::ImeSeat {
                        origin: skia_safe::Point::new(view.gutter_width.max(0.0), 0.0),
                        clip: None,
                        ask: Box::new(move |origin, clip, visit| {
                            let mut client = EditorImeClient {
                                document: &view.document,
                                editor: view.editor,
                                fonts: fonts.collection(),
                                theme,
                                origin: skia_safe::Point::new(-origin.x, -origin.y),
                                clip,

                                bounds: Size::new(
                                    (bounds.width - view.gutter_width.max(0.0)).max(0.0),
                                    bounds.height,
                                ),
                                command: None,
                            };
                            visit(&mut client);
                            match client.command {
                                Some(command) => EventResult::Command(command),
                                None => EventResult::Handled,
                            }
                        }),
                    }),
                }
            }
        }
    }
}

impl EditorCoreView<'_> {
    fn document(&self) -> &crate::document::Document {
        self.shared.document
    }

    fn dragging(&self) -> bool {
        self.document().editor(self.editor()).drag.is_some()
    }

    fn editor(&self) -> crate::editor::EditorId {
        self.shared.editor
    }

    fn theme_stale(&self) -> bool {
        self.document()
            .document_layout(self.editor())
            .is_some_and(|layout| {
                !layout.shaped_theme().is_empty()
                    && layout.shaped_theme() != self.shared.theme.name()
            })
    }

    fn retheme_report(&self, viewport: Rect) -> EditorCommand {
        EditorCommand::Retheme {
            top: viewport.top,
            bottom: viewport.bottom,
            anchor: self
                .document()
                .first_visible_byte(self.editor(), viewport.top.max(0.0)),
        }
    }
}

impl<'a> Widget<'a, EditorCommand> for EditorCoreView<'a> {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<EditorCommand> {
        let text_focused = self.document().focus(self.editor()) == EditorFocus::Text;
        match event {
            Event::Settle => {
                // The pulse delivers the honest viewport: a drifted
                // retained top means the scroll moved since the last
                // observation — refresh it first; its perform also
                // supersedes any pending correction (docs §3.1).
                let retained = self.document().viewport(self.editor());
                let drifted = match &retained {
                    Some(held) => (held.start - viewport.top).abs() > 0.5,
                    None => true,
                };
                if drifted {
                    return EventResult::Command(EditorCommand::ViewportTop(viewport.top));
                }
                // The settle pulse (docs/editor/viewport-preservation.md
                // §3.2): if this editor holds the viewport's corner
                // (its top is clipped from above) and a door left a
                // pending correction, re-aim the owning scroll —
                // exactly. The target is absolute; re-emitting until
                // the next Viewport report clears it converges at
                // the scroll.
                if viewport.top <= 0.5 {
                    return EventResult::Ignored;
                }
                match self.document().settle_target(self.editor()) {
                    Some(fresh) => {
                        EventResult::Reveal(imba::event::Reveal::top_left_at(Rect::from_xywh(
                            0.0,
                            fresh,
                            imba::Widget::size(self).width,
                            viewport.height(),
                        )))
                    }
                    None => EventResult::Ignored,
                }
            }
            Event::Paint { canvas, focused } => {
                {
                    let data = self.shared.viewport(viewport.top..viewport.bottom);
                    self.document().paint_with(
                        self.editor(),
                        &data,
                        canvas,
                        *focused,
                        &self.shared.fonts.collection(),
                        &self.shared.theme,
                    );
                }
                if !self.reports_geometry {
                    return EventResult::Handled;
                }

                let width = self.target_width;

                let current = match self.document().reported_width(self.editor()) {
                    width if width > 0.0 => width,
                    _ => self.document().layout_width(self.editor()),
                };
                let resized = width.is_finite() && width > 0.0 && (width - current).abs() > 1.0;
                let painted = viewport.top..viewport.bottom;

                let bounded = self
                    .document()
                    .document_layout(self.editor())
                    .is_some_and(|layout| layout.bounded());
                let known = self.document().viewport(self.editor());
                let grown = !bounded
                    && known.clone().is_none_or(|known| {
                        ((known.end - known.start) - (painted.end - painted.start)).abs() > 0.5
                    });

                let moved = !bounded
                    && known.is_some_and(|known| {
                        (known.start - painted.start).abs()
                            > ((painted.end - painted.start) * 0.5).max(1.0)
                    });

                if self.theme_stale() {
                    return EventResult::Command(self.retheme_report(viewport));
                }

                let damaged = !bounded
                    && self
                        .document()
                        .visible_damage(self.editor(), painted.start, painted.end);
                if resized || grown || moved || damaged {
                    return EventResult::Command(EditorCommand::Viewport {
                        width,
                        top: painted.start,
                        bottom: painted.end,
                        anchor: self
                            .document()
                            .first_visible_byte(self.editor(), viewport.top.max(0.0)),
                    });
                }
                EventResult::Handled
            }

            Event::ThemeChanged => match self.theme_stale() {
                true => EventResult::Command(self.retheme_report(viewport)),
                false => EventResult::Ignored,
            },

            Event::Scroll {
                delta_x,
                delta_y,
                gesture,
                ..
            } => {
                let pans = !self.document().softwrap(self.editor());
                if gesture.owned_by(self.surface) {
                    return match pans && delta_x.abs() > 0.01 {
                        true => EventResult::Command(EditorCommand::HorizontalScroll(*delta_x)),
                        false => EventResult::Handled,
                    };
                }
                if gesture.owned_by_other(self.surface)
                    || !pans
                    || delta_x.abs() <= delta_y.abs()
                    || delta_x.abs() <= 0.01
                {
                    return EventResult::Ignored;
                }
                let _ = gesture.claims(self.surface);
                EventResult::Command(EditorCommand::HorizontalScroll(*delta_x))
            }

            Event::AnimationClock { .. } => {
                if !self.document().reveal_pending(self.editor()) {
                    return EventResult::Ignored;
                }
                let caret = self.document().caret_byte(self.editor());
                let (x, y, w, h) = match self.document().caret_content_rect(
                    self.editor(),
                    caret,
                    &self.shared.fonts.collection(),
                    &self.shared.theme,
                ) {
                    Some(rect) => rect,

                    None => (
                        0.0,
                        self.document().height_before(self.editor(), caret),
                        2.0,
                        24.0,
                    ),
                };
                let content_bottom = self.document().content_height(self.editor());
                let rect = Rect::from_ltrb(
                    x,
                    (y - h).max(0.0),
                    x + w,
                    (y + h * 2.0).min(content_bottom.max(y + h)),
                );
                if imba::event::reveal_satisfied(viewport, rect) {
                    return EventResult::Command(EditorCommand::RevealSettled);
                }
                EventResult::Reveal(imba::event::Reveal::visible(rect))
            }
            Event::MouseDown {
                button: MouseButton::Left,
                point,
                mods,
                count,
            } => EventResult::Command(EditorCommand::Click {
                point: *point,

                kind: match (mods.alt, mods.shift, count) {
                    (true, _, _) => ClickKind::Add,
                    (false, true, _) => ClickKind::Extend,
                    (false, false, 2) => ClickKind::Word,
                    (false, false, count) if *count >= 3 => ClickKind::Line,
                    (false, false, _) => ClickKind::Set,
                },
            }),

            // Drags and releases are BROADCAST by containers (no
            // rect cull), so they may only be claimed by the editor
            // that OWNS the drag — the one whose click armed it. A
            // text-focused gate here let a once-clicked before-card
            // claim every later release and steal the host's focus.
            Event::MouseDrag { point, .. } if self.dragging() => {
                EventResult::Command(EditorCommand::Drag { point: *point })
            }
            Event::MouseUp { .. } if self.dragging() => {
                EventResult::Command(EditorCommand::DragEnd)
            }

            Event::HitTest { point, miss } if self.location.is_some() => {
                use skia_safe::Contains;
                let inside = !miss && Rect::from_size(self.size).contains(*point);
                EventResult::Command(EditorCommand::Hover(inside.then_some(*point)))
            }
            _ => EventResult::Ignored,
        }
    }
}

struct EditorImeClient<'a> {
    document: &'a crate::document::Document,
    editor: crate::editor::EditorId,
    fonts: skia_safe::textlayout::FontCollection,
    theme: &'a crate::theme::Theme,
    origin: skia_safe::Point,

    bounds: Size,

    clip: Option<Rect>,
    command: Option<EditorCommand>,
}

impl imba::ImeClient for EditorImeClient<'_> {
    fn has_marked_text(&self) -> bool {
        self.document.marked_range(self.editor).is_some()
    }

    fn marked_range(&self) -> Option<(u32, u32)> {
        let range = self.document.marked_range(self.editor)?;
        let mut view = self.document.text().view();
        Some((
            view.byte_to_utf16(range.start),
            view.byte_to_utf16(range.end),
        ))
    }

    fn selected_range(&self) -> Option<(u32, u32)> {
        let selection = self.document.carets(self.editor).primary().selection();
        let mut view = self.document.text().view();
        Some((
            view.byte_to_utf16(selection.start),
            view.byte_to_utf16(selection.end),
        ))
    }

    fn set_selected_range(&mut self, start: u32, len: u32) {
        self.command = Some(EditorCommand::SetSelectionUtf16 {
            start,
            end: start.saturating_add(len),
        });
    }

    fn substring_utf16(&self, start: u32, len: u32) -> Option<String> {
        let mut view = self.document.text().view();
        let from = view.utf16_to_byte(start);
        let to = view.utf16_to_byte(start.saturating_add(len));
        Some(view.substring(from..to))
    }

    fn document_length(&self) -> u32 {
        self.document.text().view().byte_to_utf16(u32::MAX)
    }

    fn first_rect(&self, start: u32, _len: u32) -> Option<(f32, f32, f32, f32)> {
        let byte = self.document.text().view().utf16_to_byte(start);
        let (x, y, w, h) =
            self.document
                .caret_content_rect(self.editor, byte, &self.fonts, self.theme)?;
        Some((x - self.origin.x, y - self.origin.y, w, h))
    }

    fn selection_rects(&self, start: u32, len: u32) -> Vec<(f32, f32, f32, f32)> {
        let (from, to) = {
            let mut view = self.document.text().view();
            (
                view.utf16_to_byte(start),
                view.utf16_to_byte(start.saturating_add(len)),
            )
        };
        self.document
            .selection_content_rects(self.editor, from..to, &self.fonts, self.theme)
            .into_iter()
            .map(|(x, y, w, h)| (x - self.origin.x, y - self.origin.y, w, h))
            .collect()
    }

    fn reveal_selection(&mut self) {
        let byte = self.document.carets(self.editor).primary().offset();
        self.command = Some(EditorCommand::RevealAt { byte });
    }

    fn char_index_at(&self, x: f32, y: f32) -> Option<u32> {
        if self.clip.is_some_and(|clip| {
            use skia_safe::Contains;
            !clip.contains(skia_safe::Point::new(x, y))
        }) {
            return None;
        }

        let (cx, cy) = (x + self.origin.x, y + self.origin.y);

        if cx < 0.0 || cx > self.bounds.width {
            return None;
        }
        if cy < 0.0 || cy > self.bounds.height {
            return None;
        }

        let byte = match self
            .document
            .byte_at_point(self.editor, cx, cy, &self.fonts, self.theme)
        {
            Some(byte) => byte,
            None => self.document.text().byte_count().min(u32::MAX as usize) as u32,
        };
        Some(self.document.text().view().byte_to_utf16(byte))
    }

    fn insert_text(&mut self, text: &str, replacement: Option<(u32, u32)>) {
        self.command = Some(match replacement {
            Some(replacement) => EditorCommand::InsertTextReplacing {
                text: text.to_owned(),
                replacement,
            },
            None => EditorCommand::InsertText {
                text: text.to_owned(),
            },
        });
    }

    fn set_marked_text(
        &mut self,
        text: &str,
        selected: (u32, u32),
        replacement: Option<(u32, u32)>,
    ) {
        self.command = Some(EditorCommand::SetMarkedText {
            text: text.to_owned(),
            selected,
            replacement,
        });
    }

    fn unmark_text(&mut self) {
        self.command = Some(EditorCommand::UnmarkText);
    }
}

struct EditorClipboardClient<'a> {
    document: &'a crate::document::Document,
    editor: crate::editor::EditorId,
    command: Option<EditorCommand>,
}

impl EditorClipboardClient<'_> {
    fn selections_text(&self) -> Option<String> {
        let carets = self.document.carets(self.editor);
        let mut view = self.document.text().view();
        let parts: Vec<String> = carets
            .carets()
            .iter()
            .map(|caret| caret.selection())
            .filter(|selection| selection.start < selection.end)
            .map(|selection| view.substring(selection.start..selection.end))
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }
}

impl imba::ClipboardClient for EditorClipboardClient<'_> {
    fn copy(&mut self) -> Option<imba::ClipboardContent> {
        Some(imba::ClipboardContent {
            text: self.selections_text()?,
        })
    }

    fn cut(&mut self) -> Option<imba::ClipboardContent> {
        let text = self.selections_text()?;
        self.command = Some(EditorCommand::DeleteSelections);
        Some(imba::ClipboardContent { text })
    }

    fn paste(&mut self, content: &imba::ClipboardContent) -> bool {
        self.command = Some(EditorCommand::Paste {
            text: content.text.clone(),
        });
        true
    }
}

struct FocusGate<Inner> {
    inner: Inner,
    focused: bool,
}

impl<'a, Inner, Command: 'a> Widget<'a, Command> for FocusGate<Inner>
where
    Inner: Widget<'a, Command>,
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, Command>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        match event {
            Event::Paint { canvas, focused } => self.inner.handle_event(
                arena,
                &Event::Paint {
                    canvas,
                    focused: *focused && self.focused,
                },
                viewport,
            ),
            Event::MouseDown { .. } | Event::AnimationClock { .. } | Event::ThemeChanged => {
                self.inner.handle_event(arena, event, viewport)
            }
            _ if self.focused => self.inner.handle_event(arena, event, viewport),
            _ => EventResult::Ignored,
        }
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, Command>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}
