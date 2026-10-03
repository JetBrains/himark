// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The toolbar band: burger and action buttons around a centered
//! title. There is NO input here — goto-file and the command palette
//! are standalone overlays that own their own input boxes.

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
    #[default]
    Left,
    Right,

    /// Beside the centered title, hugging its left edge.
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

        let buttons = ToolbarButtons::of(store);

        let mut strip = imba::container::container(arena, size);

        let backdrop = leaf::<ToolbarCommand>(size.width, size.height).paint_below({
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
                paint.set_anti_alias(true);
            }
        });
        strip.place(0.0, 0.0, backdrop);

        // The title, centered in the band. Text paints its baseline
        // at top + ascent, so placing at (baseline − ascent) keeps
        // the old glyph positions.
        let baseline = (chrome.height + chrome.title_size * 0.7) * 0.5;
        let ascent = -title_font.metrics().1.ascent;
        let bounds = Constraints {
            min: Size::default(),
            max: Size::new(width, chrome.height),
        };
        let label = imba::text(ui, title, title_font.clone(), chrome.title_color.0)
            .layout(arena, bounds);
        let title_x = ((width - label.size().width) * 0.5).max(0.0);
        let title_width = label.size().width;
        strip.place_boxed(title_x, baseline - ascent, label);

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
                            paint.set_color(chrome.pressed_fill.0);
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
        // Each side's buttons are a ROW (vec order, left to right);
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
            let anchor = match title_width > 0.0 {
                true => title_x,
                false => width * 0.5,
            };
            strip.place_boxed((anchor - 6.0 - row.size().width).max(0.0), 0.0, row);
        }
        if let Some(row) = side_row(ToolbarSide::Right) {
            let end = width - chrome.button_inset;
            strip.place_boxed(end - row.size().width, 0.0, row);
            strip.place(end - 1.0, 0.0, closing_edge());
        }

        strip
    }
}

/// The chat bubble beside the title — `chat.composer` (⌘I): front
/// the session's chat.
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
