// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::theme::{Key, Rgba};
use serde::Deserialize;
use skia_safe::Color;

impl TerminalChrome {
    pub const KEY: Key<TerminalChrome> = Key::new("terminal");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct TerminalChrome {
    pub font_families: Vec<String>,
    pub font_size: f32,

    pub background: Rgba,

    pub foreground: Rgba,
    pub cursor: Rgba,

    pub selection: Rgba,

    pub pad: f32,

    pub palette: Vec<Rgba>,
}

impl Default for TerminalChrome {
    fn default() -> Self {
        Self {
            font_families: vec![
                "JetBrains Mono".to_owned(),
                "SF Mono".to_owned(),
                "Menlo".to_owned(),
                "Consolas".to_owned(),
                "monospace".to_owned(),
            ],
            font_size: 26.0,
            background: Rgba(Color::new(0xff0d0d0f)),
            foreground: Rgba(Color::new(0xffd8d8db)),
            cursor: Rgba(Color::new(0xff5ca9ff)),
            selection: Rgba(Color::new(0xff2e4668)),
            pad: 16.0,
            palette: vec![
                Rgba(Color::new(0xff10131d)),
                Rgba(Color::new(0xffff7a6e)),
                Rgba(Color::new(0xff7ee787)),
                Rgba(Color::new(0xfff2cc60)),
                Rgba(Color::new(0xff5ca9ff)),
                Rgba(Color::new(0xffc792ea)),
                Rgba(Color::new(0xff4ce5c4)),
                Rgba(Color::new(0xffd8d8db)),
                Rgba(Color::new(0xff515b78)),
                Rgba(Color::new(0xffff9e91)),
                Rgba(Color::new(0xff9ef7a5)),
                Rgba(Color::new(0xffffe08a)),
                Rgba(Color::new(0xff8ec2ff)),
                Rgba(Color::new(0xffdcb0f2)),
                Rgba(Color::new(0xff8ff0dd)),
                Rgba(Color::new(0xfff0f0f2)),
            ],
        }
    }
}
