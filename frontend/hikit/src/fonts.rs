// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

#[cfg(not(target_os = "emscripten"))]
use std::sync::Arc;

use editor::FontSource;
#[cfg(not(target_os = "emscripten"))]
use skia_safe::{textlayout::FontCollection, FontMgr};

pub fn source() -> FontSource {
    #[cfg(not(target_os = "emscripten"))]
    {
        Arc::new(|| {
            thread_local! {
                static COLLECTION: FontCollection = {
                    let mut collection = FontCollection::new();
                    collection.set_default_font_manager(FontMgr::new(), None);
                    collection
                };
            }
            COLLECTION.with(Clone::clone)
        })
    }
    #[cfg(target_os = "emscripten")]
    {
        editor::embedded_fonts::source()
    }
}

pub struct ChromeTypeface(pub skia_safe::Typeface);

pub struct ChromeTextTypeface(pub skia_safe::Typeface);

pub fn ui_font(ui: &imba::UiCtx, size: f32) -> skia_safe::Font {
    let typeface = ui.env(|| {
        ChromeTypeface(
            editor::env::ui_typeface(ui, &[] as &[&str], skia_safe::FontStyle::bold())
                .expect("a chrome typeface"),
        )
    });
    let mut font = skia_safe::Font::from_typeface(typeface.0.clone(), size);
    font.set_edging(skia_safe::font::Edging::AntiAlias);
    font
}

pub fn ui_text_font(ui: &imba::UiCtx, size: f32) -> skia_safe::Font {
    let typeface = ui.env(|| {
        ChromeTextTypeface(
            editor::env::ui_typeface(ui, &[] as &[&str], skia_safe::FontStyle::normal())
                .expect("a chrome typeface"),
        )
    });
    let mut font = skia_safe::Font::from_typeface(typeface.0.clone(), size);
    font.set_edging(skia_safe::font::Edging::AntiAlias);
    font
}
