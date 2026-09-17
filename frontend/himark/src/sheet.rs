// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use skia_safe::Size;

use imba::{
    anim::{Animation, AnimationClock, Easing, Motion},
    arena::Arena,
    constraints::Constraints,
    container::container,
    event::{Event, EventResult},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View,
};

const SLIDE_MS: f64 = 160.0;

#[derive(Clone)]
pub struct FloatingChat {
    pub on: bool,
}

impl Default for FloatingChat {
    fn default() -> Self {
        Self { on: true }
    }
}

impl FloatingChat {
    pub fn set(store: &mut Store, on: bool) {
        store.update::<FloatingChat>(|flag| flag.on = on);
    }

    pub fn on(store: &Store) -> bool {
        store
            .get::<FloatingChat>()
            .map_or_else(|| Self::default().on, |flag| flag.on)
    }
}

pub enum SheetCommand {
    Tick(AnimationClock),

    Toggle,

    Content(imba::DynCommand),
}

pub(crate) struct Sheet {
    pane: Box<dyn crate::DynPanelView>,
    expanded: bool,

    shown: bool,

    rise: Animation<f32>,
}

impl Clone for Sheet {
    fn clone(&self) -> Self {
        Self {
            pane: self.pane.clone_panel(),
            expanded: self.expanded,
            shown: self.shown,
            rise: self.rise,
        }
    }
}

impl Sheet {
    pub(crate) fn new(pane: Box<dyn crate::DynPanelView>) -> Self {
        Self {
            pane,
            expanded: false,
            shown: true,
            rise: Animation::done(
                0.0,
                Motion::Ease {
                    duration_ms: SLIDE_MS,
                    easing: Easing::EaseOut,
                },
            ),
        }
    }

    pub(crate) fn pane(&self) -> &dyn crate::DynPanelView {
        self.pane.as_ref()
    }

    pub(crate) fn expanded(&self) -> bool {
        self.expanded
    }

    pub(crate) fn shown(&self) -> bool {
        self.shown
    }

    pub(crate) fn show(&mut self) {
        self.shown = true;
    }

    pub(crate) fn hide(&mut self) {
        self.shown = false;
        self.expanded = false;
        self.rise = Animation::done(
            0.0,
            Motion::Ease {
                duration_ms: SLIDE_MS,
                easing: Easing::EaseOut,
            },
        );
    }

    pub(crate) fn expand(&mut self) {
        if !self.expanded {
            self.expanded = true;
            self.rise.set(1.0);
        }
    }

    pub(crate) fn toggle(&mut self) {
        self.expanded = !self.expanded;
        self.rise.set(if self.expanded { 1.0 } else { 0.0 });
    }

    pub(crate) fn focus_changed(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        focused: bool,
        fx: &mut imba::effect::Effects<'_, SheetCommand>,
    ) {
        if !focused {
            self.hide();
        }
        self.set_blur(store, ui, !focused, fx);
    }

    pub(crate) fn set_blur(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        blurred: bool,
        fx: &mut imba::effect::Effects<'_, SheetCommand>,
    ) {
        let blur: imba::DynCommand = Box::new(crate::higent::ChatPanelCommand::Blurred(blurred));
        fx.scope(SheetCommand::Content, |fx| {
            self.pane.as_mut().perform_dyn(store, ui, blur, fx)
        });
    }

    pub(crate) fn rect(
        &self,
        chrome: &::editor::theme::SheetChrome,
        store: &Store,
        size: Size,
    ) -> skia_safe::Rect {
        // The golden section's larger part of the window — wide
        // enough to read, framed enough to still float. The themed
        // width is the floor so small windows keep a usable sheet.
        let width = (size.width * 0.618)
            .max(chrome.width)
            .min(size.width - 2.0 * chrome.margin)
            .max(1.0);
        let expanded = (size.height - 2.0 * chrome.margin).max(1.0);
        let collapsed = self
            .pane
            .collapsed_height(store, expanded)
            .unwrap_or(chrome.collapsed)
            .min(expanded);
        let height = collapsed + self.rise.value() * (expanded - collapsed);
        let x = ((size.width - width) / 2.0).max(0.0);
        let y = size.height - chrome.margin - height;
        skia_safe::Rect::from_xywh(x, y, width, height)
    }
}

impl View for Sheet {
    type Command = SheetCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::UiCtx,
    ) -> imba::focus::FocusData<'w, SheetCommand> {
        use imba::focus::FocusData;
        let own = FocusData {
            on_key: Some(Box::new(|key, _mods| match key {
                imba::event::Key::Escape => EventResult::Command(SheetCommand::Toggle),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        // The content answers FIRST — the old shape was an event
        // FALLBACK on the whole surface: Escape folds the sheet only
        // when nothing inside (say, a standing completion popup)
        // wanted it.
        imba::DynView::focus_data_dyn(self.pane.as_ref(), store, ui)
            .map(SheetCommand::Content)
            .merge_under(own)
    }

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        fx.scope(SheetCommand::Content, |fx| {
            imba::DynView::destroy_dyn(self.pane.as_mut(), store, fx)
        })
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: SheetCommand,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            SheetCommand::Tick(now) => {
                self.rise.advance(now);
            }
            SheetCommand::Toggle => self.toggle(),
            SheetCommand::Content(command) => {
                if command
                    .downcast_ref::<crate::higent::ChatPanelCommand>()
                    .is_some_and(crate::higent::ChatPanelCommand::is_send)
                {
                    self.expand();
                }
                fx.scope(SheetCommand::Content, |fx| {
                    self.pane.as_mut().perform_dyn(store, ui, command, fx)
                });
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
            let size = constraints.max;
            let theme = ::editor::env::Themes::of(store);
            let chrome = theme.ui().sheet.clone();

            let fill = theme.ui().window.background.0;
            let rect = self.rect(&chrome, store, size);
            let mut surface = container(arena, size);

            let cast = chrome.cast;
            let cast_border = chrome.cast_border.0;
            surface.place(
                rect.left + cast,
                rect.top + cast,
                leaf::<SheetCommand>(rect.width(), rect.height()).paint_instead(
                    move |_arena, canvas, paint_rect| {
                        let mut paint = skia_safe::Paint::default();
                        paint.set_style(skia_safe::PaintStyle::Stroke);
                        paint.set_stroke_width(2.0);
                        paint.set_color(cast_border);
                        canvas.draw_rect(paint_rect.with_inset((1.0, 1.0)), &paint);
                    },
                ),
            );

            let border = chrome.border.0;
            surface.place(
                rect.left,
                rect.top,
                leaf::<SheetCommand>(rect.width(), rect.height())
                    .paint_instead(move |_arena, canvas, paint_rect| {
                        let mut paint = skia_safe::Paint::default();
                        paint.set_style(skia_safe::PaintStyle::Fill);
                        paint.set_color(fill);
                        canvas.draw_rect(paint_rect, &paint);
                        paint.set_style(skia_safe::PaintStyle::Stroke);
                        paint.set_stroke_width(2.0);
                        paint.set_color(border);
                        canvas.draw_rect(paint_rect.with_inset((1.0, 1.0)), &paint);
                    })
                    .hit_opaque(),
            );
            let content = self
                .pane
                .as_ref()
                .layout_dyn(
                    arena,
                    store,
                    ui,
                    Constraints::tight(Size::new(rect.width(), rect.height())),
                )
                .map(SheetCommand::Content);
            surface.place(rect.left, rect.top, content);

            let animating = self.rise.running();
            let clock =
                leaf::<SheetCommand>(size.width, size.height).event(move |_arena, event, _size| {
                    match event {
                        Event::AnimationClock { now } if animating => {
                            EventResult::Command(SheetCommand::Tick(*now))
                        }
                        _ => EventResult::Ignored,
                    }
                });
            surface.place(0.0, 0.0, clock);

            surface.event(|_arena, event, _size| match event {
                Event::KeyDown {
                    key: imba::event::Key::Escape,
                    ..
                } => EventResult::Command(SheetCommand::Toggle),
                _ => EventResult::Ignored,
            })
        })
    }
}

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
