// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::{GalleryCommand, GalleryMode, GalleryView};
use imba::{
    arena::Arena,
    effect::Effects,
    scroll::{ScrollCommand, ScrollView},
    Layout, LayoutValue, Store, UiCtx, View,
};

/// The workbench supplies fonts, theme, event routing, and the scroll viewport.
#[derive(Clone)]
pub struct GalleryPanel {
    pub(crate) scroll: ScrollView<GalleryView>,
}

impl Default for GalleryPanel {
    fn default() -> Self {
        Self {
            scroll: ScrollView::new(GalleryView::new(GalleryMode::Interactive)),
        }
    }
}

impl View for GalleryPanel {
    type Command = ScrollCommand<GalleryCommand>;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        if matches!(command, ScrollCommand::Content(GalleryCommand::Mode(_))) {
            self.scroll.set_scroll_y(store, 0.0);
        }
        self.scroll.perform(store, ui, command, fx);
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl Layout<'a, Self::Command> + LayoutValue + 'a {
        self.scroll.display(arena, store, ui)
    }
}

impl himark::PanelView for GalleryPanel {
    type Place = himark::NoPlace;

    fn title(&self, _store: &Store) -> String {
        "UI Gallery".to_owned()
    }
    fn dismantle(&mut self, _store: &mut Store) {}
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct OpenGallery;

impl himark::DynamicCommand for OpenGallery {
    fn id(&self) -> &'static str {
        "gallery.open"
    }
    fn name(&self) -> String {
        "Open UI Gallery".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        store: &mut Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        let Some(mut entity) = himark::Windows::window(store, window) else {
            return;
        };
        let _ = entity.open_panel(store, Box::new(GalleryPanel::default()), fx);
        himark::Windows::put(store, window, entity);
    }
}
