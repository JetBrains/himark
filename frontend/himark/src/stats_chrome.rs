// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::theme::{Key, Rgba};
use serde::Deserialize;
use skia_safe::Color;

impl StatsChrome {
    pub const KEY: Key<StatsChrome> = Key::new("stats");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct StatsChrome {
    pub background: Rgba,
    pub text: Rgba,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub top: f32,
    pub right_margin: f32,
    pub pad_x: f32,
    pub first_baseline: f32,
    pub line_height: f32,
    pub font_size: f32,
}

impl Default for StatsChrome {
    fn default() -> Self {
        Self {
            background: Rgba(Color::new(0xff151518)),
            text: Rgba(Color::new(0xff8a8781)),
            width: 118.0,
            height: 66.0,
            radius: 8.0,
            top: 14.0,
            right_margin: 14.0,
            pad_x: 10.0,
            first_baseline: 19.0,
            line_height: 19.0,
            font_size: 15.0,
        }
    }
}
