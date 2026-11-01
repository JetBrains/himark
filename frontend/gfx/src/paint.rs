// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The paint the app actually mixes: a solid color, fill or stroke,
//! a width, a cap, anti-aliased or not. No shaders, no blend modes,
//! no filters — the one blur the app draws is its own Painter
//! method, not a paint knob.

use crate::Color;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    #[default]
    Fill,
    Stroke,
    StrokeAndFill,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Join {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Paint {
    color: Color,
    style: Style,
    stroke_width: f32,
    stroke_cap: Cap,
    stroke_join: Join,
    anti_alias: bool,
}

impl Default for Paint {
    fn default() -> Self {
        Self {
            color: Color::BLACK,
            style: Style::Fill,
            stroke_width: 0.0,
            stroke_cap: Cap::Butt,
            stroke_join: Join::Miter,
            anti_alias: false,
        }
    }
}

impl Paint {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_color(&mut self, color: impl Into<Color>) -> &mut Self {
        self.color = color.into();
        self
    }

    pub fn color(&self) -> Color {
        self.color
    }

    pub fn set_style(&mut self, style: Style) -> &mut Self {
        self.style = style;
        self
    }

    pub fn style(&self) -> Style {
        self.style
    }

    /// skia's convenience: `true` strokes, `false` fills.
    pub fn set_stroke(&mut self, stroke: bool) -> &mut Self {
        self.style = match stroke {
            true => Style::Stroke,
            false => Style::Fill,
        };
        self
    }

    pub fn set_stroke_width(&mut self, width: f32) -> &mut Self {
        self.stroke_width = width;
        self
    }

    pub fn stroke_width(&self) -> f32 {
        self.stroke_width
    }

    pub fn set_stroke_cap(&mut self, cap: Cap) -> &mut Self {
        self.stroke_cap = cap;
        self
    }

    pub fn stroke_cap(&self) -> Cap {
        self.stroke_cap
    }

    pub fn set_stroke_join(&mut self, join: Join) -> &mut Self {
        self.stroke_join = join;
        self
    }

    pub fn stroke_join(&self) -> Join {
        self.stroke_join
    }

    pub fn set_anti_alias(&mut self, anti_alias: bool) -> &mut Self {
        self.anti_alias = anti_alias;
        self
    }

    pub fn is_anti_alias(&self) -> bool {
        self.anti_alias
    }
}
