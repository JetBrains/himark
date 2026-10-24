// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::theme::{Key, Rgba};
use serde::Deserialize;
use skia_safe::Color;

impl CommentChrome {
    pub const KEY: Key<CommentChrome> = Key::new("comment");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct CommentChrome {
    pub surface: Rgba,
    pub border: Rgba,
    pub cross: Rgba,

    pub pad: f32,

    pub close_size: f32,

    pub min_editor_height: f32,
    pub radius: f32,
}

impl Default for CommentChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            surface: Rgba(Color::new(0xff0d0d0f)),
            border: Rgba(Color::new(0xff222226)),
            cross: Rgba(Color::new(0xd8a5b3dd)),
            pad: 16.0,
            close_size: 24.0,
            min_editor_height: 40.0,
            radius: 10.0,
        }
    }
}
