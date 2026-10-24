// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::theme::{Key, Rgba};
use serde::Deserialize;
use skia_safe::Color;

impl TableChrome {
    pub const KEY: Key<TableChrome> = Key::new("table");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct TableChrome {
    pub grid: Rgba,
    pub grid_strong: Rgba,

    pub thickness: f32,
    pub cell_pad_x: f32,
    pub cell_pad_y: f32,

    pub column_floor: f32,

    pub column_cap: f32,

    pub quantum: f32,

    pub fallback_width: f32,

    pub row_floor: f32,
    pub control_size: f32,
    pub control_fill: Rgba,
    pub control_glyph: Rgba,
    pub control_remove: Rgba,
}

impl Default for TableChrome {
    fn default() -> Self {
        Self {
            grid: Rgba(Color::new(0x12667a9c)),
            grid_strong: Rgba(Color::new(0x42565f89)),
            thickness: 1.0,
            cell_pad_x: 10.0,
            cell_pad_y: 6.0,
            column_floor: 36.0,
            column_cap: 0.6,
            quantum: 8.0,
            fallback_width: 900.0,
            row_floor: 40.0,
            control_size: 24.0,
            control_fill: Rgba(Color::new(0xff0a0a10)),
            control_glyph: Rgba(Color::new(0xff5ca9ff)),
            control_remove: Rgba(Color::new(0xe0876b75)),
        }
    }
}

impl CheckboxChrome {
    pub const KEY: Key<CheckboxChrome> = Key::new("checkbox");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct CheckboxChrome {
    pub size: f32,
    pub radius: f32,
    pub stroke: f32,
    pub border: Rgba,
    pub fill: Rgba,
    pub check: Rgba,
}

impl Default for CheckboxChrome {
    fn default() -> Self {
        Self {
            size: 26.0,
            radius: 7.0,
            stroke: 2.0,
            border: Rgba(Color::new(0xff8a8781)),
            fill: Rgba(Color::new(0xfff2b544)),
            check: Rgba(Color::new(0xff0d0d0f)),
        }
    }
}
