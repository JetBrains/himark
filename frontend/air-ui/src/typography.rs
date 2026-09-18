// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::theme::air::Typography;
use imba::{
    arena::Arena, constraints::Constraints, thunk_ext::ThunkExt, Layout, LayoutValue, Store,
    ThunkBox, UiCtx, WithBaseline,
};
use skia_safe::{
    font_arguments::{variation_position::Coordinate, VariationPosition},
    textlayout::{FontCollection, ParagraphBuilder, ParagraphStyle},
    Color, Font, FontArguments, FontStyle,
};

/// A resolved theme role, sharing the application's font collection and its
/// typeface cache. No font managers or global caches are created during layout.
#[derive(Clone)]
pub struct TextStyle {
    pub font: Font,
    pub color: Color,
    pub tracking: f32,
    pub line_height: f32,
    fonts: FontCollection,
    family: String,
    weight: u16,
}

fn coordinates(weight: u16, size: f32) -> [Coordinate; 2] {
    [
        Coordinate {
            axis: Coordinate::wght,
            value: f32::from(weight),
        },
        Coordinate {
            axis: Coordinate::opsz,
            value: size,
        },
    ]
}

impl TextStyle {
    pub(crate) fn new(store: &Store, ui: &UiCtx, role: &Typography, color: Color) -> Self {
        let mut fonts = editor::env::ui_collection(store, ui);
        let font = Self::resolve_font(&mut fonts, &role.family, role.weight, role.size);
        Self {
            font,
            color,
            tracking: role.tracking,
            line_height: role.line_height,
            fonts,
            family: role.family.clone(),
            weight: role.weight,
        }
    }

    fn resolve_font(fonts: &mut FontCollection, family: &str, weight: u16, size: f32) -> Font {
        let coords = coordinates(weight, size);
        let args = FontArguments::new().set_variation_design_position(VariationPosition {
            coordinates: &coords,
        });
        let args = skia_safe::textlayout::FontArguments::from(args);
        let face = fonts
            .find_typefaces_with_font_arguments(&[family], FontStyle::normal(), &args)
            .into_iter()
            .next()
            .or_else(|| {
                fonts
                    .find_typefaces(&[editor::embedded_fonts::FAMILY], FontStyle::normal())
                    .into_iter()
                    .next()
            })
            .expect("UI font collection has a fallback typeface");
        let mut font = Font::from_typeface(face, size);
        font.set_edging(skia_safe::font::Edging::AntiAlias);
        font.set_subpixel(true);
        font
    }

    pub fn colored(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn sized(mut self, size: f32) -> Self {
        let scale = size / self.font.size();
        self.line_height *= scale;
        self.tracking *= scale;
        self.font = Self::resolve_font(&mut self.fonts, &self.family, self.weight, size);
        self
    }
}

pub struct Text {
    content: String,
    style: TextStyle,
}

pub fn text(style: &TextStyle, content: impl Into<String>) -> Text {
    Text {
        content: content.into(),
        style: style.clone(),
    }
}

impl LayoutValue for Text {}

impl<'a, Command: 'a> Layout<'a, Command> for Text {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command> {
        let Text { content, style } = self;
        let height = style.line_height;
        let coords = coordinates(style.weight, style.font.size());
        let args = FontArguments::new().set_variation_design_position(VariationPosition {
            coordinates: &coords,
        });
        let mut text_style = skia_safe::textlayout::TextStyle::new();
        text_style.set_font_arguments(&args);
        text_style
            .set_font_families(&[&style.family])
            .set_font_style(FontStyle::normal())
            .set_font_size(style.font.size())
            .set_color(style.color)
            .set_letter_spacing(style.tracking)
            .set_height(height / style.font.size())
            .set_height_override(true)
            .set_half_leading(true)
            .set_font_edging(skia_safe::font::Edging::AntiAlias)
            .set_subpixel(true)
            .set_font_hinting(skia_safe::FontHinting::None);
        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style.set_text_style(&text_style).set_max_lines(1);
        paragraph_style.turn_hinting_off();
        let mut builder = ParagraphBuilder::new(&paragraph_style, style.fonts);
        builder.add_text(&content);
        let mut paragraph = builder.build();
        paragraph.layout(1_000_000.0);
        let width = paragraph
            .max_intrinsic_width()
            .min(constraints.max.width)
            .max(constraints.min.width);
        // Match the browser's symmetric leading and rounded ascent/descent.
        let metrics = style.font.metrics().1;
        let ascent = (-metrics.ascent).round();
        let descent = metrics.descent.round();
        let baseline = ascent + ((height - ascent - descent) * 0.5).floor();
        let text_y = baseline - paragraph.alphabetic_baseline();
        let label = imba::leaf::leaf::<Command>(width, height).paint_instead(
            move |_arena, canvas, rect| {
                canvas.save();
                canvas.clip_rect(rect, None, false);
                paragraph.paint(canvas, (rect.left, rect.top + text_y));
                canvas.restore();
            },
        );
        ThunkBox::new(
            arena,
            WithBaseline {
                thunk: label,
                baseline,
            },
        )
    }
}
