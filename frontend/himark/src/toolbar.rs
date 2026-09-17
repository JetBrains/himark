// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    Layout as _, Thunk, UiCtx, View,
};
use skia_safe::{Canvas, Paint, Rect, Size};

use crate::{AppFx, EditorCommand, ModalView, Window};
use ::editor::EditorView;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolbarSide {
    #[default]
    Left,
    Right,

    Well,
}

#[derive(Clone)]
pub struct ToolbarButton {
    pub command: &'static str,

    pub order: f32,

    pub side: ToolbarSide,

    pub glyph: std::sync::Arc<dyn Fn(&Canvas, Rect, skia_safe::Color) + Send + Sync>,
}

#[derive(Clone, Default)]
pub struct ToolbarButtons(Vec<ToolbarButton>);

impl ToolbarButtons {
    pub fn of(store: &Store) -> ToolbarButtons {
        store.get::<ToolbarButtons>().cloned().unwrap_or_default()
    }

    pub(crate) fn register(store: &mut Store, button: ToolbarButton) {
        store.update::<ToolbarButtons>(|buttons| {
            buttons.0.push(button);
            buttons.0.sort_by(|a, b| a.order.total_cmp(&b.order));
        });
    }

    pub fn iter(&self) -> impl Iterator<Item = &ToolbarButton> {
        self.0.iter()
    }
}

#[derive(Clone)]
pub struct OverlaySurface {
    pub prefix: Option<char>,
    #[allow(clippy::type_complexity)]
    pub open: std::sync::Arc<
        dyn Fn(&mut Store, &imba::UiCtx, crate::WindowId, &mut AppFx<'_>) -> Box<dyn ModalView>
            + Send
            + Sync,
    >,
}

#[derive(Clone, Default)]
pub struct OverlaySurfaces(Vec<OverlaySurface>);

impl OverlaySurfaces {
    pub fn of(store: &Store) -> OverlaySurfaces {
        store.get::<OverlaySurfaces>().cloned().unwrap_or_default()
    }

    pub(crate) fn register(store: &mut Store, surface: OverlaySurface) {
        store.update::<OverlaySurfaces>(|surfaces| surfaces.0.push(surface));
    }

    fn find(&self, class: Option<char>) -> Option<&OverlaySurface> {
        self.0.iter().find(|surface| surface.prefix == class)
    }

    fn split(&self, raw: &str) -> (Option<char>, String) {
        let mut chars = raw.chars();
        match chars.next() {
            Some(first) if self.find(Some(first)).is_some() => {
                (Some(first), chars.as_str().to_owned())
            }
            _ => (None, raw.to_owned()),
        }
    }
}

#[derive(Clone)]
pub enum ToolbarRequest {
    Command(&'static str),

    Query(String),
}

pub enum ToolbarCommand {
    Button(usize),

    Begin,

    Input(EditorCommand),
}

#[derive(Clone)]
struct Session {
    class: Option<char>,
    input: EditorView,
}

#[derive(Clone, Default)]
pub struct Toolbar {
    session: Option<Session>,

    request: crate::modal::RequestSlot<ToolbarRequest>,
}

impl Toolbar {
    pub(crate) fn take_request(&mut self) -> Option<ToolbarRequest> {
        self.request.take()
    }

    pub(crate) fn session_class(&self) -> Option<Option<char>> {
        self.session.as_ref().map(|session| session.class)
    }

    pub(crate) fn query(&self) -> Option<String> {
        let session = self.session.as_ref()?;
        let mut view = session.input.document.text().view();
        let byte_count = view.byte_count();
        Some(view.byte_string(0, byte_count))
    }

    pub(crate) fn start_session(
        &mut self,
        store: &Store,
        class: Option<char>,
        text: &str,
        width: f32,
    ) {
        let mut markup = crate::Markup::new();

        markup.push_styled_covering(0..text.len() as u32, ::editor::theme::StyleId::Input);
        let document = crate::Document::new(text::Text::from_string_exact(text), markup);
        let fonts = ::editor::env::Fonts::of(store);
        let theme = ::editor::env::Themes::of(store);
        let mut input = EditorView::of_document(document, width.max(1.0), &fonts(), &theme);
        input.set_caret(text.len() as u32);
        input.focus_text();
        self.session = Some(Session { class, input });
    }

    pub(crate) fn set_session_class(&mut self, class: Option<char>) {
        if let Some(session) = &mut self.session {
            session.class = class;
        }
    }

    pub(crate) fn end_session(&mut self) {
        self.session = None;
    }

    pub(crate) fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ToolbarCommand> {
        match &self.session {
            Some(session) => session
                .input
                .focus_data(store, ui)
                .map(ToolbarCommand::Input),
            None => imba::focus::FocusData::default(),
        }
    }

    pub(crate) fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: ToolbarCommand,
        fx: &mut imba::effect::Effects<'_, ToolbarCommand>,
    ) {
        match command {
            ToolbarCommand::Button(index) => {
                if let Some(button) = ToolbarButtons::of(store).0.get(index) {
                    self.request.file(ToolbarRequest::Command(button.command));
                }
            }
            ToolbarCommand::Begin => {
                self.request.file(ToolbarRequest::Query(String::new()));
            }
            ToolbarCommand::Input(command) => {
                if let Some(session) = &mut self.session {
                    fx.scope(ToolbarCommand::Input, |fx| {
                        session.input.perform(store, ui, command, fx)
                    });
                    if let Some(query) = self.query() {
                        self.request.file(ToolbarRequest::Query(query));
                    }
                }
            }
        }
    }

    pub(crate) fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        width: f32,
        title: String,
        active: Option<&'static str>,
    ) -> impl Thunk<'a, ToolbarCommand> + 'a {
        let chrome = ::editor::env::Themes::of(store).ui().toolbar.clone();
        let size = Size::new(width, chrome.height);
        let title_font = crate::fonts::ui_text_font(ui, chrome.title_size);

        let clearance = ui
            .get::<crate::app::ChromeClearance>()
            .map(|clearance| clearance.0)
            .unwrap_or(0.0);

        let well_width = well_width(width, &chrome);
        let well_x = ((width - well_width) * 0.5).max(0.0);
        let well_y = ((chrome.height - chrome.well_height) * 0.5).max(0.0);

        let buttons = ToolbarButtons::of(store);

        let focused = self.session.is_some();

        let mode: Option<&'static str> = self.session.as_ref().map(|session| {
            let text = session.input.document.text();
            let head = text.page_at(0, text.byte_count(), 4);
            match head.first() {
                Some(b'>') => "COMMAND",
                Some(b'%') => "SEARCH",
                _ => "JUMP",
            }
        });

        let mut strip = imba::container::container(arena, size);

        let backdrop = leaf::<ToolbarCommand>(size.width, size.height).paint_below({
            let chrome = chrome.clone();
            move |_arena, canvas, rect| {
                let mut paint = Paint::default();
                paint.set_color(chrome.background.0);
                canvas.draw_rect(rect, &paint);

                paint.set_anti_alias(false);
                paint.set_color(match focused {
                    true => chrome.well_fill_focused.0,
                    false => chrome.well_fill.0,
                });
                canvas.draw_rect(
                    Rect::from_xywh(rect.left + well_x, rect.top, well_width, chrome.height),
                    &paint,
                );
                paint.set_color(chrome.rule.0);
                for edge in [well_x, well_x + well_width] {
                    canvas.draw_rect(
                        Rect::from_xywh(rect.left + edge, rect.top, 1.0, chrome.height),
                        &paint,
                    );
                }

                canvas.draw_rect(
                    Rect::from_xywh(rect.left, rect.bottom - 1.0, rect.width(), 1.0),
                    &paint,
                );
                paint.set_anti_alias(true);
            }
        });
        strip.place(0.0, 0.0, backdrop);

        // The well's texts, as PRIMITIVES with exact baseline parity:
        // Text paints its baseline at top + ascent, so placing each at
        // (the old hand-computed baseline − ascent) reproduces the
        // draw_str glyph positions bit for bit.
        let baseline = well_y + (chrome.well_height + chrome.title_size * 0.7) * 0.5;
        let ascent = -title_font.metrics().1.ascent;
        let well_bounds = Constraints {
            min: Size::default(),
            max: Size::new(well_width, chrome.height),
        };
        if let Some(mode) = mode {
            let label = imba::text(mode, title_font.clone(), chrome.title_color.0)
                .tracking(1.5)
                .layout(arena, well_bounds);
            strip.place_boxed(
                well_x + well_width - label.size().width - 18.0,
                baseline - ascent,
                label,
            );
        }
        if !focused {
            let label = imba::text(title, title_font.clone(), chrome.title_color.0)
                .layout(arena, well_bounds);
            strip.place_boxed(
                well_x + ((well_width - label.size().width) * 0.5).max(0.0),
                baseline - ascent,
                label,
            );
        }

        match &self.session {
            Some(session) => {
                let input = imba::Layout::layout(
                    session.input.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(
                        input_width(well_width, &chrome),
                        (chrome.well_height - chrome.input_shrink).max(1.0),
                    )),
                )
                .map(ToolbarCommand::Input);
                strip.place(
                    well_x + chrome.input_inset_x,
                    well_y + chrome.input_shrink * 0.5,
                    input,
                );
            }

            None => {
                let begin = leaf::<ToolbarCommand>(well_width, chrome.well_height).event(
                    |_arena, event, _size| match event {
                        Event::MouseDown { .. } => EventResult::Command(ToolbarCommand::Begin),
                        _ => EventResult::Ignored,
                    },
                );
                strip.place(well_x, well_y, begin);
            }
        }

        let button_widget = |index: usize, button: &ToolbarButton| {
            let glyph = button.glyph.clone();
            let box_size = chrome.button_size;
            let pressed = active == Some(button.command);

            leaf::<ToolbarCommand>(box_size, chrome.height)
                .paint_below({
                    let chrome = chrome.clone();
                    move |_arena, canvas, rect| {
                        let mut paint = Paint::default();
                        if pressed {
                            paint.set_color(chrome.well_fill_focused.0);
                            canvas.draw_rect(
                                Rect::from_xywh(
                                    rect.left,
                                    rect.top,
                                    rect.width(),
                                    rect.height() - 1.0,
                                ),
                                &paint,
                            );
                        }
                        paint.set_color(chrome.rule.0);
                        canvas.draw_rect(
                            Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
                            &paint,
                        );
                        paint.set_anti_alias(true);
                        let square = Rect::from_xywh(
                            rect.left,
                            rect.top + (rect.height() - rect.width()) * 0.5,
                            rect.width(),
                            rect.width(),
                        );
                        let inset = (rect.width() * 0.25).max(1.0);
                        glyph(
                            canvas,
                            square.with_inset((inset, inset)),
                            chrome.glyph_color.0,
                        );
                    }
                })
                .event(move |_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::Command(ToolbarCommand::Button(index)),
                    _ => EventResult::Ignored,
                })
        };

        let rule_color = chrome.rule.0;
        let closing_edge = || {
            leaf::<ToolbarCommand>(1.0, chrome.height).paint_below(move |_arena, canvas, rect| {
                let mut paint = Paint::default();
                paint.set_color(rule_color);
                canvas.draw_rect(rect, &paint);
            })
        };
        // Each side's buttons are a ROW (vec order, left to right —
        // exactly the order the old descending-x loops produced);
        // the hairline edges keep their absolute homes.
        let side_row = |side: ToolbarSide| -> Option<imba::ThunkBox<'a, ToolbarCommand>> {
            let mut row = imba::Row::new(arena);
            let mut any = false;
            for (index, button) in buttons.0.iter().enumerate() {
                if button.side != side {
                    continue;
                }
                row = row.child(imba::fixed(button_widget(index, button)));
                any = true;
            }
            any.then(|| {
                row.layout(
                    arena,
                    Constraints {
                        min: Size::default(),
                        max: Size::new(width, chrome.height),
                    },
                )
            })
        };
        if let Some(row) = side_row(ToolbarSide::Left) {
            let end = chrome.button_inset + clearance + row.size().width;
            strip.place_boxed(chrome.button_inset + clearance, 0.0, row);
            strip.place(end, 0.0, closing_edge());
        }
        if let Some(row) = side_row(ToolbarSide::Well) {
            strip.place_boxed(well_x - 6.0 - row.size().width, 0.0, row);
        }
        if let Some(row) = side_row(ToolbarSide::Right) {
            let end = width - chrome.button_inset;
            strip.place_boxed(end - row.size().width, 0.0, row);
            strip.place(end - 1.0, 0.0, closing_edge());
        }

        strip
    }
}

fn well_width(width: f32, chrome: &::editor::theme::ToolbarChrome) -> f32 {
    (width * chrome.well_width_ratio)
        .clamp(chrome.well_width_min, chrome.well_width_max)
        .min((width * 0.9).max(1.0))
}

fn input_width(well_width: f32, chrome: &::editor::theme::ToolbarChrome) -> f32 {
    (well_width - chrome.input_inset_x * 2.0).max(1.0)
}

fn session_input_width(store: &Store, entity: &Window) -> f32 {
    let theme = ::editor::env::Themes::of(store);
    let chrome = &theme.ui().toolbar;
    input_width(well_width(entity.viewport_size().width, chrome), chrome)
}

pub fn toggle_toolbar_session(
    store: &mut Store,
    ui: &imba::UiCtx,
    window: crate::WindowId,
    surface: &OverlaySurface,
    seed: &str,
    fx: &mut AppFx<'_>,
) {
    let mut entity = crate::Windows::window(store, window).expect("the window entity");
    if entity.toolbar_session_class() == Some(surface.prefix) && entity.has_modal() {
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        crate::Windows::put(store, window, entity);
        return;
    }

    let width = session_input_width(store, &entity);
    entity.toolbar_start_session(store, surface.prefix, seed, width);
    crate::Windows::put(store, window, entity);
    let payload = match surface.prefix {
        Some(prefix) => seed.strip_prefix(prefix).unwrap_or(seed).to_owned(),
        None => seed.to_owned(),
    };
    mount_surface(store, ui, window, surface, &payload, fx);
}

pub(crate) fn toolbar_query(
    store: &mut Store,
    ui: &imba::UiCtx,
    window: crate::WindowId,
    raw: &str,
    fx: &mut AppFx<'_>,
) {
    let surfaces = OverlaySurfaces::of(store);
    let (class, payload) = surfaces.split(raw);
    let entity = crate::Windows::window_ref(store, window).expect("the window entity");
    if entity.toolbar_session_class() == Some(class) && entity.has_modal() {
        return feed_query(store, ui, window, &payload, fx);
    }

    let Some(surface) = surfaces.find(class).cloned() else {
        return;
    };
    let mut entity = crate::Windows::window(store, window).expect("the window entity");
    match entity.toolbar_session_class() {
        None => {
            let width = session_input_width(store, &entity);
            entity.toolbar_start_session(store, class, raw, width);
        }

        Some(_) => entity.toolbar_set_session_class(class),
    }
    crate::Windows::put(store, window, entity);
    mount_surface(store, ui, window, &surface, &payload, fx);
}

fn mount_surface(
    store: &mut Store,
    ui: &imba::UiCtx,
    window: crate::WindowId,
    surface: &OverlaySurface,
    payload: &str,
    fx: &mut AppFx<'_>,
) {
    let mut entity = crate::Windows::window(store, window).expect("the window entity");
    fx.scope(
        move |command| crate::AppCommand::Content(window, command),
        |fx| entity.release_modal_for_swap(store, fx),
    );
    crate::Windows::put(store, window, entity);
    let modal = (surface.open)(store, ui, window, fx);
    let mut entity = crate::Windows::window(store, window).expect("the window entity");
    fx.scope(
        move |command| crate::AppCommand::Content(window, command),
        |fx| entity.set_overlay(store, modal, fx),
    );
    crate::Windows::put(store, window, entity);
    feed_query(store, ui, window, payload, fx);
}

fn feed_query(
    store: &mut Store,
    ui: &imba::UiCtx,
    window: crate::WindowId,
    payload: &str,
    fx: &mut AppFx<'_>,
) {
    let mut entity = crate::Windows::window(store, window).expect("the window entity");
    fx.scope(crate::modal_scope(window), |fx| {
        entity.modal_set_query(store, ui, payload, fx)
    });
    crate::Windows::put(store, window, entity);
}
