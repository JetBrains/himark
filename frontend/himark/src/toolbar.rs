// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The GLOBAL CLUSTER: the only window-level chrome — a small strip
//! pinned top-left, after the platform semaphore clearance, holding
//! the window-scoped buttons (drawer, toc, chat). Everything else a
//! toolbar used to show lives in the COLUMN headers now: the chat
//! column names its session, the split tree names its file, the dock
//! carries its own buttons ([docs/ui/toolbar.md]).

use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    Layout as _, Thunk, UiCtx,
};
use skia_safe::{Canvas, Paint, Rect, Size};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolbarSide {
    /// The global cluster, top-left of the window.
    #[default]
    Left,

    /// The dock's own header.
    Right,

    /// Legacy name — rides the global cluster after the Left group.
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
pub struct ToolbarButtons(pub(crate) Vec<ToolbarButton>);

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

    /// The global cluster's buttons: the Left side always, the Well
    /// side (the chat bubble) only while the chat is NOT displayed.
    fn cluster(&self, show_chat: bool) -> impl Iterator<Item = (usize, &ToolbarButton)> {
        self.0.iter().enumerate().filter(move |(_, button)| {
            matches!(button.side, ToolbarSide::Left)
                || (show_chat && matches!(button.side, ToolbarSide::Well))
        })
    }
}

#[derive(Clone)]
pub enum ToolbarRequest {
    Command(&'static str),
}

#[derive(Clone)]
pub enum ToolbarCommand {
    Button(usize),
}

impl std::fmt::Display for ToolbarCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolbarCommand::Button(_) => out.write_str("toolbar button"),
        }
    }
}

/// The width the global cluster occupies at the window's top-left:
/// the platform clearance plus its buttons. The leftmost column's
/// header insets its content by this much.
pub(crate) fn global_cluster_width(store: &Store, ui: &UiCtx, show_chat: bool) -> f32 {
    let chrome = ::editor::env::Themes::of(store).ui().toolbar.clone();
    let clearance = ui
        .get::<crate::app::ChromeClearance>()
        .map(|clearance| clearance.0)
        .unwrap_or(0.0);
    let buttons = ToolbarButtons::of(store).cluster(show_chat).count() as f32;
    chrome.button_inset + clearance + buttons * chrome.button_size + 1.0
}

/// The DOCK CLUSTER's width: the right-side mirror of the global
/// cluster — the dock's buttons, pinned top-right at all times. The
/// rightmost column header reserves this much trailing room while
/// the dock is closed (open, the dock's own header takes over in
/// the same pixels).
pub(crate) fn dock_cluster_width(store: &Store) -> f32 {
    let chrome = ::editor::env::Themes::of(store).ui().toolbar.clone();
    let buttons = ToolbarButtons::of(store)
        .iter()
        .filter(|button| matches!(button.side, ToolbarSide::Right))
        .count() as f32;
    match buttons > 0.0 {
        true => chrome.button_inset + buttons * chrome.button_size,
        false => 0.0,
    }
}

#[derive(Clone, Default)]
pub struct Toolbar {
    request: crate::modal::RequestSlot<ToolbarRequest>,
}

impl Toolbar {
    pub(crate) fn take_request(&mut self) -> Option<ToolbarRequest> {
        self.request.take()
    }

    pub(crate) fn focus_data<'w>(
        &'w self,
        _store: &'w Store,
        _ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ToolbarCommand> {
        imba::focus::FocusData::default()
    }

    pub(crate) fn perform(
        &mut self,
        store: &mut Store,
        _ui: &UiCtx,
        command: ToolbarCommand,
        _fx: &mut imba::effect::Effects<'_, ToolbarCommand>,
    ) {
        match command {
            ToolbarCommand::Button(index) => {
                if let Some(button) = ToolbarButtons::of(store).0.get(index) {
                    self.request.file(ToolbarRequest::Command(button.command));
                }
            }
        }
    }

    /// The cluster strip: transparent over the leftmost column's own
    /// header — buttons only, no backdrop of its own.
    pub(crate) fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        show_chat: bool,
        window_width: f32,
        dock_open: bool,
    ) -> impl Thunk<'a, ToolbarCommand> + 'a {
        let chrome = ::editor::env::Themes::of(store).ui().toolbar.clone();
        let clearance = ui
            .get::<crate::app::ChromeClearance>()
            .map(|clearance| clearance.0)
            .unwrap_or(0.0);
        let buttons = ToolbarButtons::of(store);
        let count = buttons.cluster(show_chat).count();
        let size = Size::new(window_width.max(1.0), chrome.height);

        let mut strip = imba::container::container(arena, size);
        let mut x = chrome.button_inset + clearance;
        for (index, button) in buttons.cluster(show_chat) {
            let glyph = button.glyph.clone();
            let chrome = chrome.clone();
            let cell = leaf::<ToolbarCommand>(chrome.button_size, chrome.height)
                .paint_below(move |_arena, canvas, rect| {
                    let mut paint = Paint::default();
                    // The group's STYLE: a hairline on every cell edge.
                    paint.set_anti_alias(false);
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
                })
                .event(move |_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::Command(ToolbarCommand::Button(index)),
                    _ => EventResult::Ignored,
                });
            strip.place(x, 0.0, cell);
            x += chrome.button_size;
        }
        if count > 0 {
            let rule = chrome.rule.0;
            let edge = leaf::<ToolbarCommand>(1.0, chrome.height).paint_below(
                move |_arena, canvas, rect| {
                    let mut paint = Paint::default();
                    paint.set_color(rule);
                    canvas.draw_rect(rect, &paint);
                },
            );
            strip.place(x, 0.0, edge);
        }

        // The right mirror: the dock's buttons, always discoverable.
        // While the dock is OPEN its own header renders them (same
        // pixels, pressed state, rides the slide) — the strip yields.
        if !dock_open {
            let mut x = window_width
                - chrome.button_inset
                - buttons
                    .iter()
                    .filter(|button| matches!(button.side, ToolbarSide::Right))
                    .count() as f32
                    * chrome.button_size;
            for (index, button) in buttons.iter().enumerate() {
                if !matches!(button.side, ToolbarSide::Right) {
                    continue;
                }
                let glyph = button.glyph.clone();
                let chrome = chrome.clone();
                let cell = leaf::<ToolbarCommand>(chrome.button_size, chrome.height)
                    .paint_below(move |_arena, canvas, rect| {
                        let mut paint = Paint::default();
                        paint.set_anti_alias(false);
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
                    })
                    .event(move |_arena, event, _size| match event {
                        Event::MouseDown { .. } => {
                            EventResult::Command(ToolbarCommand::Button(index))
                        }
                        _ => EventResult::Ignored,
                    });
                strip.place(x, 0.0, cell);
                x += chrome.button_size;
            }
        }
        imba::Layout::layout(
            imba::laid(move |_arena: &'a Arena, _constraints: Constraints| strip),
            arena,
            Constraints::tight(size),
        )
    }
}

/// A COLUMN HEADER: the per-column strip every column draws at its
/// own top — background, bottom rule, a left-aligned title. The
/// leftmost column passes the global cluster's width as `inset` so
/// its title clears the semaphore and the cluster buttons.
pub(crate) fn column_header<'a, Command: Clone + 'a>(
    arena: &'a Arena,
    store: &'a Store,
    ui: &'a UiCtx,
    width: f32,
    title: String,
    inset: f32,
    trailing: f32,
) -> imba::ThunkBox<'a, Command> {
    let chrome = ::editor::env::Themes::of(store).ui().toolbar.clone();
    let size = Size::new(width.max(1.0), chrome.height);
    let title_font = crate::fonts::ui_text_font(ui, chrome.title_size);

    // A long path DEGRADES gracefully: drop leading segments behind
    // an ellipsis until the title fits what the buttons leave it.
    let available = (width - inset - trailing - chrome.button_inset * 2.0).max(1.0);
    let mut title = title;
    while imba::text_advance(ui, &title_font, &title) > available {
        let Some((_, rest)) = title.trim_start_matches("…/").split_once('/') else {
            break;
        };
        title = format!("…/{rest}");
    }

    let mut strip = imba::container::container(arena, size);
    let backdrop = leaf::<Command>(size.width, size.height).paint_below({
        let chrome = chrome.clone();
        move |_arena, canvas, rect| {
            let mut paint = Paint::default();
            paint.set_color(chrome.background.0);
            canvas.draw_rect(rect, &paint);
            paint.set_anti_alias(false);
            paint.set_color(chrome.rule.0);
            canvas.draw_rect(
                Rect::from_xywh(rect.left, rect.bottom - 1.0, rect.width(), 1.0),
                &paint,
            );
        }
    });
    strip.place(0.0, 0.0, backdrop);

    let baseline = (chrome.height + chrome.title_size * 0.7) * 0.5;
    let ascent = -title_font.metrics().1.ascent;
    let bounds = Constraints {
        min: Size::default(),
        max: Size::new(
            (width - inset - trailing - chrome.button_inset).max(1.0),
            chrome.height,
        ),
    };
    let label =
        imba::text(ui, title, title_font.clone(), chrome.title_color.0).layout(arena, bounds);
    strip.place_boxed(inset + chrome.button_inset, baseline - ascent, label);

    imba::ThunkBox::new(
        arena,
        imba::Layout::layout(
            imba::laid(move |_arena: &'a Arena, _constraints: Constraints| strip),
            arena,
            Constraints::tight(size),
        ),
    )
}

/// The chat bubble in the global cluster — `chat.composer` (⌘I):
/// front the session's chat.
pub fn composer_button() -> crate::ToolbarButton {
    crate::ToolbarButton {
        command: "chat.composer",
        order: 0.0,
        side: crate::ToolbarSide::Well,
        glyph: std::sync::Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());

            let bubble = skia_safe::Rect::from_xywh(l, t + h * 0.04, w, h * 0.68);
            canvas.draw_round_rect(bubble, w * 0.18, w * 0.18, &paint);
            let mut tail = skia_safe::PathBuilder::new();
            tail.move_to((l + w * 0.24, t + h * 0.72));
            tail.line_to((l + w * 0.18, t + h * 0.96));
            tail.line_to((l + w * 0.46, t + h * 0.72));
            canvas.draw_path(&tail.detach(), &paint);

            let mut caret = skia_safe::PathBuilder::new();
            caret.move_to((l + w * 0.32, t + h * 0.22));
            caret.line_to((l + w * 0.32, t + h * 0.54));
            canvas.draw_path(&caret.detach(), &paint);
        }),
    }
}
