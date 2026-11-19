// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! `gfx::Painter` straight into a gpui `Scene` — gpui's renderer
//! with no gpui window or app around it. The host owns the surface
//! (a `CAMetalLayer`, a canvas) and a renderer over it; a frame is
//! `scene.clear()`, a paint through [`ScenePainter`],
//! `scene.finish()`, `renderer.draw(&scene)`.
//!
//! Everything is device pixels end to end: the engine paints in
//! them and the scene stores them, so nothing is scaled on the way
//! except font sizes, which the text system wants logical.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    point, px, size, AtlasTile, Bounds, ContentMask, Corners, DevicePixels, Edges, FontId, FontRun,
    GlyphId, Hsla, IsZero, MonochromeSprite, PaintedPath, PlatformAtlas, PlatformTextSystem,
    PolychromeSprite, Quad, RenderGlyphParams, RenderImageParams, ScaledPixels, Scene, Shadow,
    SpriteAnimationId, TransformationMatrix, SUBPIXEL_VARIANTS_X, SUBPIXEL_VARIANTS_Y,
};

use crate::faces::{self, color};
use crate::transform::Transform;

/// The text system and what was resolved through it — one per
/// process, shared by every surface.
pub struct SceneText {
    platform: Arc<dyn PlatformTextSystem>,
    /// `None` is a face the platform refused — said once, not per frame.
    fonts: RefCell<HashMap<gfx::FaceId, Option<FontId>>>,
    /// Asking the platform for a glyph's raster bounds costs a font
    /// lookup per glyph per frame; the answer never changes.
    raster_bounds: RefCell<HashMap<RenderGlyphParams, Bounds<DevicePixels>>>,
}

impl SceneText {
    pub fn new(platform: Arc<dyn PlatformTextSystem>) -> Self {
        Self {
            platform,
            fonts: RefCell::default(),
            raster_bounds: RefCell::default(),
        }
    }

    fn font_id(&self, face: gfx::FaceId) -> Option<FontId> {
        if let Some(hit) = self.fonts.borrow().get(&face) {
            return *hit;
        }
        let (bytes, font) = faces::font(face);
        if let Err(error) = self.platform.add_fonts(vec![Cow::Owned(bytes)]) {
            eprintln!("[gfx-gpui] add_fonts({}): {error}", font.family);
        }
        let id = self
            .platform
            .font_id(&font)
            // gpui refuses any face with no 'm' glyph (a symbols
            // face), so this is expected for those — said once.
            .inspect_err(|error| eprintln!("[gfx-gpui] font_id({}): {error}", font.family))
            .ok();
        self.fonts.borrow_mut().insert(face, id);
        id
    }

    fn raster_bounds(&self, params: &RenderGlyphParams) -> anyhow::Result<Bounds<DevicePixels>> {
        if let Some(hit) = self.raster_bounds.borrow().get(params) {
            return Ok(*hit);
        }
        let bounds = self.platform.glyph_raster_bounds(params)?;
        self.raster_bounds
            .borrow_mut()
            .insert(params.clone(), bounds);
        Ok(bounds)
    }
}

/// Paints one frame into `scene`. The `Painter` trait paints
/// through `&self`, so the scene sits in a `RefCell`.
pub struct ScenePainter<'a> {
    scene: RefCell<&'a mut Scene>,
    atlas: &'a dyn PlatformAtlas,
    text: &'a SceneText,
    /// The surface, in device pixels.
    size: gfx::Size,
    /// Device pixels per logical pixel — only the text system asks.
    scale: f32,
    transform: Transform,
}

fn scaled(rect: gfx::Rect) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(rect.left), ScaledPixels(rect.top)),
        size: size(
            ScaledPixels(rect.right - rect.left),
            ScaledPixels(rect.bottom - rect.top),
        ),
    }
}

/// Rounds to the nearest integer with 0.5 ties toward zero — gpui's
/// own glyph quantization, so atlas keys match its rasters.
fn round_half_toward_zero(value: f32) -> f32 {
    (value.abs() - 0.5).ceil().copysign(value)
}

impl<'a> ScenePainter<'a> {
    pub fn new(
        scene: &'a mut Scene,
        atlas: &'a dyn PlatformAtlas,
        text: &'a SceneText,
        size: gfx::Size,
        scale: f32,
    ) -> Self {
        Self {
            scene: RefCell::new(scene),
            atlas,
            text,
            size,
            scale,
            transform: Transform::new(),
        }
    }

    fn insert(&self, primitive: impl Into<gpui::Primitive>) {
        self.scene.borrow_mut().insert_primitive(primitive);
    }

    fn surface(&self) -> gfx::Rect {
        gfx::Rect::from_wh(self.size.width, self.size.height)
    }

    fn mask(&self) -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: scaled(self.transform.clip().unwrap_or(self.surface())),
        }
    }

    fn len(&self, length: f32) -> f32 {
        length * self.transform.magnitude()
    }

    fn device(&self, x: f32, y: f32) -> gpui::Point<gpui::Pixels> {
        let (x, y) = self.transform.point(x, y);
        point(px(x), px(y))
    }

    /// Fill vs stroke, straight off the paint. A stroke becomes a
    /// border on the quad, outset by half the width to approximate
    /// skia's centered stroke.
    fn quad(&self, rect: gfx::Rect, radius: f32, paint: &gfx::Paint) {
        if !self.transform.axis_aligned() {
            // The rotated-rect road: corners square off when a
            // radius meets a rotation.
            let mut builder = self.path_builder(paint);
            builder.move_to(self.device(rect.left, rect.top));
            builder.line_to(self.device(rect.right, rect.top));
            builder.line_to(self.device(rect.right, rect.bottom));
            builder.line_to(self.device(rect.left, rect.bottom));
            builder.close();
            self.path(builder, paint);
            return;
        }
        let bounds = self.transform.rect(rect);
        let radius = self.len(radius);
        let fill = color(paint.color());
        let quad = match paint.style() {
            gfx::PaintStyle::Fill | gfx::PaintStyle::StrokeAndFill => Quad {
                bounds: scaled(bounds),
                content_mask: self.mask(),
                background: fill.into(),
                corner_radii: Corners::all(ScaledPixels(radius)),
                ..Default::default()
            },
            gfx::PaintStyle::Stroke => {
                let width = self.len(paint.stroke_width().max(1.0));
                let half = width * 0.5;
                Quad {
                    bounds: scaled(gfx::Rect::new(
                        bounds.left - half,
                        bounds.top - half,
                        bounds.right + half,
                        bounds.bottom + half,
                    )),
                    content_mask: self.mask(),
                    border_color: fill,
                    corner_radii: Corners::all(ScaledPixels(radius + half)),
                    border_widths: Edges::all(ScaledPixels(width)),
                    ..Default::default()
                }
            }
        };
        self.insert(quad);
    }

    fn path_builder(&self, paint: &gfx::Paint) -> gpui::PathBuilder {
        match paint.style() {
            gfx::PaintStyle::Stroke => {
                gpui::PathBuilder::stroke(px(self.len(paint.stroke_width().max(1.0))))
            }
            _ => gpui::PathBuilder::fill(),
        }
    }

    fn path(&self, builder: gpui::PathBuilder, paint: &gfx::Paint) {
        let Ok(path) = builder.build() else {
            return;
        };
        // The path is already in device pixels: a unit scale factor
        // and a mask in the same unit.
        let mask = self.transform.clip().unwrap_or(self.surface());
        let mask = ContentMask {
            bounds: Bounds::from_corners(
                point(px(mask.left), px(mask.top)),
                point(px(mask.right), px(mask.bottom)),
            ),
        };
        self.insert(PaintedPath::new(
            path,
            1.0,
            mask,
            color(paint.color()).into(),
        ));
    }

    /// One glyph with its baseline origin at device `(x, y)`.
    /// `font_size` is in device pixels.
    fn glyph(
        &self,
        (x, y): (f32, f32),
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: f32,
        ink: Hsla,
        emoji: bool,
    ) -> anyhow::Result<()> {
        // Monochrome glyphs keep a quarter-pixel horizontal phase;
        // emoji snap to whole pixels.
        let (origin, subpixel_variant) = if emoji {
            (
                point(round_half_toward_zero(x), round_half_toward_zero(y)),
                Default::default(),
            )
        } else {
            let variants = (SUBPIXEL_VARIANTS_X as f32, SUBPIXEL_VARIANTS_Y as f32);
            let quantized = point(
                round_half_toward_zero(x * variants.0) / variants.0,
                round_half_toward_zero(y * variants.1) / variants.1,
            );
            (
                point(quantized.x.trunc(), quantized.y.trunc()),
                point(
                    (quantized.x.fract() * variants.0) as u8,
                    (quantized.y.fract() * variants.1) as u8,
                ),
            )
        };
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size: px(font_size / self.scale),
            subpixel_variant,
            scale_factor: self.scale,
            is_emoji: emoji,
            subpixel_rendering: false,
            dilation: match emoji {
                true => 0,
                false => self.text.platform.glyph_dilation_for_color(ink),
            },
        };
        let raster_bounds = self.text.raster_bounds(&params)?;
        if raster_bounds.is_zero() {
            return Ok(());
        }
        let Some(tile) = self
            .atlas
            .get_or_insert_with(params.clone().into(), &mut || {
                let (size, bytes) = self.text.platform.rasterize_glyph(&params, raster_bounds)?;
                Ok(Some((size, Cow::Owned(bytes))))
            })?
        else {
            return Ok(());
        };
        let bounds = Bounds {
            origin: point(
                ScaledPixels(origin.x + raster_bounds.origin.x.0 as f32),
                ScaledPixels(origin.y + raster_bounds.origin.y.0 as f32),
            ),
            size: tile.bounds.size.map(Into::into),
        };
        match emoji {
            true => self.insert(PolychromeSprite {
                order: 0,
                pad: 0,
                grayscale: false.into(),
                bounds,
                corner_radii: Default::default(),
                content_mask: self.mask(),
                tile,
                opacity: 1.0,
            }),
            false => self.insert(MonochromeSprite {
                order: 0,
                animation: SpriteAnimationId::NONE,
                bounds,
                content_mask: self.mask(),
                color: ink,
                tile,
                transformation: TransformationMatrix::unit(),
            }),
        }
        Ok(())
    }

    fn image_tile(&self, image: &gfx::Image) -> Option<AtlasTile> {
        let rendered = faces::render_image(image)?;
        let params = RenderImageParams {
            image_id: rendered.id,
            frame_index: 0,
        };
        self.atlas
            .get_or_insert_with(params.into(), &mut || {
                Ok(rendered
                    .as_bytes(0)
                    .map(|bytes| (rendered.size(0), Cow::Borrowed(bytes))))
            })
            .ok()
            .flatten()
    }
}

impl gfx::Painter for ScenePainter<'_> {
    fn save(&self) {
        self.transform.save();
    }

    fn restore(&self) {
        self.transform.restore();
    }

    fn translate(&self, dx: f32, dy: f32) {
        self.transform.translate(dx, dy);
    }

    fn scale(&self, sx: f32, sy: f32) {
        self.transform.scale(sx, sy);
    }

    fn skew(&self, sx: f32, sy: f32) {
        self.transform.skew(sx, sy);
    }

    fn rotate(&self, degrees: f32, pivot: Option<gfx::Point>) {
        self.transform.rotate(degrees, pivot);
    }

    fn clip_rect(&self, rect: gfx::Rect, _anti_alias: bool) {
        self.transform.clip_rect(rect);
    }

    fn local_clip_bounds(&self) -> Option<gfx::Rect> {
        self.transform.local_clip_bounds()
    }

    fn base_layer_size(&self) -> gfx::Size {
        self.size
    }

    fn device_scale(&self) -> f32 {
        self.transform.magnitude()
    }

    fn clear(&self, color_value: gfx::Color) {
        // The scene starts empty and the renderer clears the
        // drawable, so a transparent clear has nothing to add.
        if color_value.a() == 0 {
            return;
        }
        self.insert(Quad {
            bounds: scaled(self.surface()),
            content_mask: self.mask(),
            background: color(color_value).into(),
            ..Default::default()
        });
    }

    fn draw_rect(&self, rect: gfx::Rect, paint: &gfx::Paint) {
        self.quad(rect, 0.0, paint);
    }

    fn draw_round_rect(&self, rect: gfx::Rect, rx: f32, ry: f32, paint: &gfx::Paint) {
        self.quad(rect, rx.max(ry), paint);
    }

    fn draw_circle(&self, center: gfx::Point, radius: f32, paint: &gfx::Paint) {
        let rect = gfx::Rect::new(
            center.x - radius,
            center.y - radius,
            center.x + radius,
            center.y + radius,
        );
        self.quad(rect, radius, paint);
    }

    fn draw_line(&self, from: gfx::Point, to: gfx::Point, paint: &gfx::Paint) {
        let mut builder = gpui::PathBuilder::stroke(px(self.len(paint.stroke_width().max(1.0))));
        builder.move_to(self.device(from.x, from.y));
        builder.line_to(self.device(to.x, to.y));
        self.path(builder, paint);
    }

    fn draw_path(&self, path: &gfx::Path, paint: &gfx::Paint) {
        let mut builder = self.path_builder(paint);
        for verb in path.verbs() {
            match *verb {
                gfx::path::Verb::Move(to) => builder.move_to(self.device(to.x, to.y)),
                gfx::path::Verb::Line(to) => builder.line_to(self.device(to.x, to.y)),
                gfx::path::Verb::Quad(control, to) => {
                    builder.curve_to(self.device(to.x, to.y), self.device(control.x, control.y));
                }
                gfx::path::Verb::Cubic(first, second, to) => {
                    builder.cubic_bezier_to(
                        self.device(to.x, to.y),
                        self.device(first.x, first.y),
                        self.device(second.x, second.y),
                    );
                }
                gfx::path::Verb::Close => builder.close(),
            }
        }
        self.path(builder, paint);
    }

    fn draw_text_blob(&self, blob: &gfx::TextBlob, origin: gfx::Point, paint: &gfx::Paint) {
        let ink = color(paint.color());
        for run in blob.runs() {
            let Some(font_id) = self.text.font_id(run.face) else {
                continue;
            };
            let font_size = self.len(run.size);
            for (glyph, position) in &run.glyphs {
                let at = self
                    .transform
                    .point(origin.x + position.x, origin.y + position.y);
                let glyph = GlyphId(*glyph as u32);
                // A color face refuses the monochrome raster.
                if self
                    .glyph(at, font_id, glyph, font_size, ink, false)
                    .is_err()
                {
                    let _ = self.glyph(at, font_id, glyph, font_size, ink, true);
                }
            }
        }
    }

    fn draw_str(&self, text: &str, origin: gfx::Point, font: &gfx::Font, paint: &gfx::Paint) {
        if text.is_empty() {
            return;
        }
        // By BYTES, not by family name: a system fallback face the
        // caller measured with is not in the text system's own
        // collection until it is registered.
        let typeface = font.typeface();
        let face = gfx::register_face(gfx::FaceEntry {
            data: typeface.data(),
            index: typeface.index(),
            coords: typeface.coords().to_vec(),
            family: Some(typeface.family_name()),
        });
        let Some(font_id) = self.text.font_id(face) else {
            return;
        };
        let font_size = self.len(font.size());
        let run = FontRun {
            len: text.len(),
            font_id,
        };
        let line = self
            .text
            .platform
            .layout_line(text, px(font_size / self.scale), &[run]);
        // `origin` is the BASELINE, and so is a shaped glyph's
        // position; the layout is logical, the scene is device.
        let (x, y) = self.transform.point(origin.x, origin.y);
        let ink = color(paint.color());
        for run in &line.runs {
            for glyph in &run.glyphs {
                let at = (
                    x + f32::from(glyph.position.x) * self.scale,
                    y + f32::from(glyph.position.y) * self.scale,
                );
                let _ = self.glyph(at, run.font_id, glyph.id, font_size, ink, glyph.is_emoji);
            }
        }
    }

    fn draw_image(
        &self,
        image: &gfx::Image,
        origin: gfx::Point,
        sampling: gfx::Sampling,
        paint: &gfx::Paint,
    ) {
        let rect = gfx::Rect::from_xywh(
            origin.x,
            origin.y,
            image.width() as f32,
            image.height() as f32,
        );
        self.draw_image_rect(image, rect, sampling, paint);
    }

    fn draw_image_rect(
        &self,
        image: &gfx::Image,
        dst: gfx::Rect,
        _sampling: gfx::Sampling,
        _paint: &gfx::Paint,
    ) {
        let Some(tile) = self.image_tile(image) else {
            return;
        };
        self.insert(PolychromeSprite {
            order: 0,
            pad: 0,
            grayscale: false.into(),
            bounds: scaled(self.transform.rect(dst)),
            corner_radii: Default::default(),
            content_mask: self.mask(),
            tile,
            opacity: 1.0,
        });
    }

    fn draw_blurred_round_rect(
        &self,
        rect: gfx::Rect,
        rx: f32,
        _ry: f32,
        sigma: f32,
        c: gfx::Color,
    ) {
        let shape_bounds = scaled(self.transform.rect(rect));
        // A CSS blur radius is twice the gaussian's sigma.
        let blur_radius = ScaledPixels(self.len(sigma * 2.0));
        self.insert(Shadow {
            order: 0,
            blur_radius,
            // Leave room for the tail of the blur.
            bounds: shape_bounds.dilate(blur_radius * 3.),
            content_mask: self.mask(),
            corner_radii: Corners::default(),
            color: color(c),
            shape_bounds,
            shape_corner_radii: Corners::all(ScaledPixels(self.len(rx))),
            inset: false.into(),
            pad: 0,
        });
    }
}
