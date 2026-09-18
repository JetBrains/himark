// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::{GalleryCommand, GalleryMode, GalleryView};
use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult},
    thunk_ext::ThunkExt,
    Layout, Store, Thunk, UiCtx, Widget,
};
use skia_safe::{surfaces, Canvas, Image, Rect, Size};
use std::error::Error;

pub const SCREENSHOT_WIDTH: f32 = 1100.0;

/// A standalone adapter: no Application, workbench, host services, or window.
/// Shares the editor's embedded font collection for deterministic rendering.
pub struct Gallery {
    pub(crate) store: Store,
    pub(crate) ui: UiCtx,
    pub(crate) view: GalleryView,
}

impl Gallery {
    pub fn new(mode: GalleryMode) -> Self {
        Self::with_theme(mode, himark::theme::Theme::embedded())
    }

    pub fn with_theme(mode: GalleryMode, theme: himark::theme::Theme) -> Self {
        let mut store = Store::new();
        store.put(himark::env::Themes(theme));
        let ui = UiCtx::cold();
        ui.set(himark::env::UiFonts(himark::embedded_fonts::collection()));
        Self {
            store,
            ui,
            view: GalleryView::new(mode),
        }
    }

    pub fn mode(&self) -> GalleryMode {
        self.view.mode()
    }
    pub fn set_mode(&mut self, mode: GalleryMode) {
        self.view.set_mode(mode);
    }

    pub(crate) fn content<'a>(&'a self, arena: &'a Arena) -> impl Layout<'a, GalleryCommand> + 'a {
        self.view.content(arena, &self.store, &self.ui)
    }

    pub fn screenshot(&self) -> Result<Image, Box<dyn Error>> {
        let height = self.content_height(SCREENSHOT_WIDTH).ceil();
        let mut surface = surfaces::raster_n32_premul((SCREENSHOT_WIDTH as i32, height as i32))
            .ok_or("could not allocate gallery screenshot")?;
        self.draw(surface.canvas(), Size::new(SCREENSHOT_WIDTH, height), 0.0);
        Ok(surface.image_snapshot())
    }

    pub(crate) fn constraints(width: f32) -> Constraints {
        Constraints {
            min: Size::default(),
            max: Size::new(width, f32::MAX),
        }
    }

    pub fn content_height(&self, width: f32) -> f32 {
        let arena = Arena::default();
        let height = self
            .content(&arena)
            .layout(&arena, Self::constraints(width))
            .size()
            .height;
        height
    }

    pub fn draw(&self, canvas: &Canvas, size: Size, scroll: f32) {
        canvas.clear(himark::env::Themes::of(&self.store).ui().air.background.0);
        let arena = Arena::default();
        let viewport = Rect::from_xywh(0.0, scroll, size.width, size.height);
        let widget = self
            .content(&arena)
            .layout(&arena, Self::constraints(size.width))
            .overlay_host(imba::overlay::WINDOW)
            .realize(&arena, viewport);
        canvas.save();
        canvas.clip_rect(Rect::from_size(size), None, false);
        canvas.translate((0.0, -scroll));
        widget.handle_event(
            &arena,
            &Event::Paint {
                canvas,
                focused: true,
            },
            viewport,
        );
        canvas.restore();
    }

    pub fn handle_event(&mut self, event: &Event<'_>, size: Size, scroll: f32) {
        let event = match event {
            Event::MouseMove { point } => Event::HitTest {
                point: *point,
                miss: false,
            },
            other => *other,
        };
        let result = {
            let arena = Arena::default();
            let viewport = Rect::from_xywh(0.0, scroll, size.width, size.height);
            let widget = self
                .content(&arena)
                .layout(&arena, Self::constraints(size.width))
                .overlay_host(imba::overlay::WINDOW)
                .realize(&arena, viewport);
            widget.handle_event(&arena, &event.translated(0.0, scroll), viewport)
        };
        match result {
            EventResult::Command(command) => self.view.apply(command),
            EventResult::Commands(commands) => commands
                .into_iter()
                .for_each(|command| self.view.apply(command)),
            _ => {}
        }
    }
}
