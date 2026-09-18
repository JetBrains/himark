// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;

#[derive(Clone)]
pub struct Fonts(pub crate::FontSource);

impl Fonts {
    pub fn of(store: &Store) -> crate::FontSource {
        store
            .get::<Fonts>()
            .map(|fonts| fonts.0.clone())
            .unwrap_or_else(crate::embedded_fonts::source)
    }
}

pub struct UiFonts(pub skia_safe::textlayout::FontCollection);

#[derive(Clone)]
pub struct Parsers(pub std::sync::Arc<crate::reparse::SyntaxLanguages>);

impl Parsers {
    pub fn of(store: &Store) -> Option<std::sync::Arc<crate::reparse::SyntaxLanguages>> {
        store.get::<Parsers>().map(|parsers| parsers.0.clone())
    }
}

#[derive(Clone)]
pub struct Enrichers(pub std::sync::Arc<crate::enrich::Enrichers>);

impl Enrichers {
    pub fn of(store: &Store) -> Option<std::sync::Arc<crate::enrich::Enrichers>> {
        store
            .get::<Enrichers>()
            .map(|enrichers| enrichers.0.clone())
    }
}

pub fn ui_collection(
    store: &imba::store::Store,
    ui: &imba::UiCtx,
) -> skia_safe::textlayout::FontCollection {
    ui.env(|| UiFonts(Fonts::of(store)())).0.clone()
}

pub fn ui_typeface(
    ui: &imba::UiCtx,
    families: &[impl AsRef<str>],
    style: skia_safe::FontStyle,
) -> Option<skia_safe::Typeface> {
    if let Some(fonts) = ui.get::<UiFonts>() {
        let mut collection = fonts.0.clone();

        if !families.is_empty() {
            if let Some(face) = collection
                .find_typefaces(families, style)
                .into_iter()
                .next()
            {
                return Some(face);
            }
        }

        if let Some(face) = collection
            .fallback_manager()
            .and_then(|manager| manager.legacy_make_typeface(None, style))
        {
            return Some(face);
        }

        if let Some(face) = collection
            .find_typefaces(&[crate::embedded_fonts::FAMILY], style)
            .into_iter()
            .next()
        {
            return Some(face);
        }
    }
    skia_safe::FontMgr::new().legacy_make_typeface(None, style)
}

pub struct Workshop {
    source: crate::FontSource,
    fonts: std::sync::Mutex<Option<skia_safe::textlayout::FontCollection>>,
    theme: std::sync::Mutex<crate::theme::Theme>,
}

unsafe impl Send for Workshop {}
unsafe impl Sync for Workshop {}

impl Workshop {
    pub fn new(source: crate::FontSource, theme: crate::theme::Theme) -> Self {
        Self {
            source,
            fonts: std::sync::Mutex::new(None),
            theme: std::sync::Mutex::new(theme),
        }
    }

    pub fn fonts(&self) -> skia_safe::textlayout::FontCollection {
        let mut slot = self.fonts.lock().expect("workshop fonts");
        slot.get_or_insert_with(|| (self.source)()).clone()
    }

    pub fn theme(&self) -> crate::theme::Theme {
        self.theme.lock().expect("workshop theme").clone()
    }

    pub fn set_theme(&self, theme: crate::theme::Theme) {
        *self.theme.lock().expect("workshop theme") = theme;
    }
}

#[derive(Clone)]
pub struct Themes(pub crate::theme::Theme);

impl Themes {
    pub fn set(store: &mut Store, theme: crate::theme::Theme) {
        store.put(Themes(theme));
    }

    pub fn of(store: &Store) -> crate::theme::Theme {
        store
            .get::<Themes>()
            .map(|themes| themes.0.clone())
            .unwrap_or_else(crate::theme::Theme::embedded)
    }
}

/// The frame's focused SEAT — the semantic walk's answer, stashed
/// into the frame store by whoever builds a tree. The editor derives
/// its selections-visible bit from it AT BUILD TIME, so the viewport
/// snapshot never needs a focused "upgrade" at paint. Absent (bare
/// harnesses, cells): fall back to the editor's own focus state.
#[derive(Clone, Copy)]
pub struct FrameFocus(pub imba::focus::SeatKey);

impl FrameFocus {
    pub fn set(store: &mut imba::store::Store, seat: imba::focus::SeatKey) {
        store.put(FrameFocus(seat));
    }

    pub fn of(store: &imba::store::Store) -> Option<imba::focus::SeatKey> {
        store.get::<FrameFocus>().map(|frame| frame.0)
    }
}
