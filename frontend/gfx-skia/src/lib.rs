// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The skia backend of the painting boundary. The ONLY crate (with
//! the shells that own skia surfaces) that names skia: everything
//! above draws through `gfx`, and this crate resolves the neutral
//! vocabulary — geometry, paint, faces, blobs, rasters — into skia
//! calls. The reference backend: any other must match it pixel for
//! pixel where the tests look.
//!
//! `SkiaCanvas` wraps a `&skia_safe::Canvas` (the orphan rule bars
//! implementing `gfx::Painter` on the foreign canvas directly) and
//! coerces to `&gfx::Canvas` wherever one is asked for.

use std::collections::HashMap;
use std::sync::Mutex;

pub use skia_safe;

use gfx::Painter;

// ─── vocabulary conversions ───

pub fn point(point: gfx::Point) -> skia_safe::Point {
    skia_safe::Point::new(point.x, point.y)
}

pub fn rect(rect: gfx::Rect) -> skia_safe::Rect {
    skia_safe::Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

pub fn back_rect(rect: skia_safe::Rect) -> gfx::Rect {
    gfx::Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

pub fn color(color: gfx::Color) -> skia_safe::Color {
    skia_safe::Color::new(color.0)
}

pub fn paint(paint: &gfx::Paint) -> skia_safe::Paint {
    let mut out = skia_safe::Paint::default();
    out.set_color(color(paint.color()));
    out.set_anti_alias(paint.is_anti_alias());
    out.set_style(match paint.style() {
        gfx::PaintStyle::Fill => skia_safe::PaintStyle::Fill,
        gfx::PaintStyle::Stroke => skia_safe::PaintStyle::Stroke,
        gfx::PaintStyle::StrokeAndFill => skia_safe::PaintStyle::StrokeAndFill,
    });
    out.set_stroke_width(paint.stroke_width());
    out.set_stroke_cap(match paint.stroke_cap() {
        gfx::PaintCap::Butt => skia_safe::PaintCap::Butt,
        gfx::PaintCap::Round => skia_safe::PaintCap::Round,
        gfx::PaintCap::Square => skia_safe::PaintCap::Square,
    });
    out.set_stroke_join(match paint.stroke_join() {
        gfx::PaintJoin::Miter => skia_safe::PaintJoin::Miter,
        gfx::PaintJoin::Round => skia_safe::PaintJoin::Round,
        gfx::PaintJoin::Bevel => skia_safe::PaintJoin::Bevel,
    });
    out
}

fn sampling(sampling: gfx::Sampling) -> skia_safe::SamplingOptions {
    match sampling {
        gfx::Sampling::Nearest => skia_safe::SamplingOptions::default(),
        gfx::Sampling::Linear => skia_safe::SamplingOptions::from(skia_safe::FilterMode::Linear),
    }
}

// ─── faces ───

/// A face resolved for skia: `Typeface::from_data` on the neutral
/// bytes, instanced at the handle's coords. Cached by the handle's
/// identity — the bytes are loaded once and shared, so the address
/// is the face.
fn typeface(face: &gfx::Typeface) -> Option<skia_safe::Typeface> {
    type Key = ((usize, usize), u32, Vec<(u32, u32)>);
    static CACHE: Mutex<Option<HashMap<Key, Option<skia_safe::Typeface>>>> = Mutex::new(None);
    let key: Key = (
        face.data_id(),
        face.index(),
        face.coords()
            .iter()
            .map(|(tag, value)| (*tag, value.to_bits()))
            .collect(),
    );
    let mut cache = CACHE.lock().expect("skia typeface cache");
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some(hit) = cache.get(&key) {
        return hit.clone();
    }
    let data = face.data();
    let resolved = skia_safe::FontMgr::new()
        .new_from_data(
            skia_safe::Data::new_copy((*data).as_ref()),
            Some(face.index()),
        )
        .and_then(|base| {
            if face.coords().is_empty() {
                return Some(base);
            }
            let coordinates: Vec<skia_safe::font_arguments::variation_position::Coordinate> = face
                .coords()
                .iter()
                .map(
                    |(tag, value)| skia_safe::font_arguments::variation_position::Coordinate {
                        axis: skia_safe::FourByteTag::new(*tag),
                        value: *value,
                    },
                )
                .collect();
            let arguments = skia_safe::FontArguments::new().set_variation_design_position(
                skia_safe::font_arguments::VariationPosition {
                    coordinates: &coordinates,
                },
            );
            base.clone_with_arguments(&arguments).or(Some(base))
        });
    cache.insert(key, resolved.clone());
    resolved
}

/// A registered face (the shaped road) resolved for skia, cached by
/// `FaceId`.
fn face_typeface(id: gfx::FaceId) -> Option<skia_safe::Typeface> {
    static CACHE: Mutex<Option<HashMap<gfx::FaceId, Option<skia_safe::Typeface>>>> =
        Mutex::new(None);
    let mut cache = CACHE.lock().expect("skia face cache");
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some(hit) = cache.get(&id) {
        return hit.clone();
    }
    let entry = gfx::face(id);
    let resolved = skia_safe::FontMgr::new()
        .new_from_data(
            skia_safe::Data::new_copy((*entry.data).as_ref()),
            Some(entry.index),
        )
        .and_then(|base| {
            if entry.coords.is_empty() {
                return Some(base);
            }
            let coordinates: Vec<skia_safe::font_arguments::variation_position::Coordinate> = entry
                .coords
                .iter()
                .map(
                    |(tag, value)| skia_safe::font_arguments::variation_position::Coordinate {
                        axis: skia_safe::FourByteTag::new(*tag),
                        value: *value,
                    },
                )
                .collect();
            let arguments = skia_safe::FontArguments::new().set_variation_design_position(
                skia_safe::font_arguments::VariationPosition {
                    coordinates: &coordinates,
                },
            );
            base.clone_with_arguments(&arguments).or(Some(base))
        });
    cache.insert(id, resolved.clone());
    resolved
}

pub fn font(font: &gfx::Font) -> Option<skia_safe::Font> {
    let typeface = typeface(font.typeface())?;
    let mut out = skia_safe::Font::from_typeface(typeface, font.size());
    out.set_edging(skia_safe::font::Edging::AntiAlias);
    out.set_subpixel(true);
    if font.is_embolden() {
        out.set_embolden(true);
    }
    Some(out)
}

/// The blob's skia twin, built ONCE through the blob's cache slot —
/// the atlas sees the same object every frame, so its sub-run cache
/// keys hold exactly as when the twin was built at shape time.
fn blob(blob: &gfx::TextBlob) -> Option<skia_safe::TextBlob> {
    blob.backend(|runs| {
        let mut builder = skia_safe::TextBlobBuilder::new();
        let mut inked = false;
        for run in runs {
            let Some(typeface) = face_typeface(run.face) else {
                continue;
            };
            let mut font = skia_safe::Font::from_typeface(typeface, run.size);
            font.set_edging(skia_safe::font::Edging::AntiAlias);
            font.set_subpixel(true);
            if run.embolden {
                font.set_embolden(true);
            }
            if run.skew_x != 0.0 {
                font.set_skew_x(run.skew_x);
            }
            let (ids, points) = builder.alloc_run_pos(&font, run.glyphs.len(), None);
            for (slot, (glyph, _)) in ids.iter_mut().zip(&run.glyphs) {
                *slot = *glyph;
            }
            for (slot, (_, position)) in points.iter_mut().zip(&run.glyphs) {
                *slot = skia_safe::Point::new(position.x, position.y);
            }
            inked = true;
        }
        inked.then(|| builder.make()).flatten()
    })
    .cloned()
    .flatten()
}

/// The image's skia twin through its cache slot — uploaded once,
/// reused for the image's life.
fn image(image: &gfx::Image) -> Option<skia_safe::Image> {
    image
        .backend(|image| {
            let info = skia_safe::ImageInfo::new(
                (image.width() as i32, image.height() as i32),
                skia_safe::ColorType::RGBA8888,
                match image.is_premultiplied() {
                    true => skia_safe::AlphaType::Premul,
                    false => skia_safe::AlphaType::Unpremul,
                },
                None,
            );
            skia_safe::images::raster_from_data(
                &info,
                skia_safe::Data::new_copy(image.pixels()),
                image.width() as usize * 4,
            )
        })
        .cloned()
        .flatten()
}

// ─── the canvas ───

/// A skia canvas standing where a `&gfx::Canvas` is asked for.
/// `repr(transparent)` over the canvas itself — `wrap` is a
/// reference cast, and the type carries no lifetime, so it
/// satisfies the alias's implicit `dyn Painter + 'static`.
#[repr(transparent)]
pub struct SkiaCanvas(skia_safe::Canvas);

impl SkiaCanvas {
    pub fn wrap(canvas: &skia_safe::Canvas) -> &SkiaCanvas {
        // SAFETY: repr(transparent) — the two types share one layout.
        unsafe { &*(canvas as *const skia_safe::Canvas as *const SkiaCanvas) }
    }
}

impl Painter for SkiaCanvas {
    fn save(&self) {
        self.0.save();
    }

    fn restore(&self) {
        self.0.restore();
    }

    fn translate(&self, dx: f32, dy: f32) {
        self.0.translate((dx, dy));
    }

    fn scale(&self, sx: f32, sy: f32) {
        self.0.scale((sx, sy));
    }

    fn skew(&self, sx: f32, sy: f32) {
        self.0.skew((sx, sy));
    }

    fn rotate(&self, degrees: f32, pivot: Option<gfx::Point>) {
        self.0.rotate(degrees, pivot.map(point));
    }

    fn clip_rect(&self, bounds: gfx::Rect, anti_alias: bool) {
        self.0.clip_rect(rect(bounds), None, anti_alias);
    }

    fn local_clip_bounds(&self) -> Option<gfx::Rect> {
        self.0.local_clip_bounds().map(back_rect)
    }

    fn base_layer_size(&self) -> gfx::Size {
        let size = self.0.base_layer_size();
        gfx::Size::new(size.width as f32, size.height as f32)
    }

    fn device_scale(&self) -> f32 {
        let matrix = self.0.local_to_device_as_3x3();
        matrix.scale_x().abs().max(matrix.scale_y().abs())
    }

    fn clear(&self, value: gfx::Color) {
        self.0.clear(color(value));
    }

    fn draw_rect(&self, bounds: gfx::Rect, style: &gfx::Paint) {
        self.0.draw_rect(rect(bounds), &paint(style));
    }

    fn draw_round_rect(&self, bounds: gfx::Rect, rx: f32, ry: f32, style: &gfx::Paint) {
        self.0.draw_round_rect(rect(bounds), rx, ry, &paint(style));
    }

    fn draw_circle(&self, center: gfx::Point, radius: f32, style: &gfx::Paint) {
        self.0.draw_circle(point(center), radius, &paint(style));
    }

    fn draw_line(&self, from: gfx::Point, to: gfx::Point, style: &gfx::Paint) {
        self.0.draw_line(point(from), point(to), &paint(style));
    }

    fn draw_path(&self, path: &gfx::Path, style: &gfx::Paint) {
        let mut builder = skia_safe::PathBuilder::new();
        for verb in path.verbs() {
            match *verb {
                gfx::path::Verb::Move(to) => builder.move_to(point(to)),
                gfx::path::Verb::Line(to) => builder.line_to(point(to)),
                gfx::path::Verb::Quad(control, to) => builder.quad_to(point(control), point(to)),
                gfx::path::Verb::Cubic(first, second, to) => {
                    builder.cubic_to(point(first), point(second), point(to))
                }
                gfx::path::Verb::Close => builder.close(),
            };
        }
        self.0.draw_path(&builder.detach(), &paint(style));
    }

    fn draw_text_blob(&self, text: &gfx::TextBlob, origin: gfx::Point, style: &gfx::Paint) {
        if let Some(twin) = blob(text) {
            self.0.draw_text_blob(&twin, point(origin), &paint(style));
        }
    }

    fn draw_str(&self, text: &str, origin: gfx::Point, face: &gfx::Font, style: &gfx::Paint) {
        if let Some(font) = font(face) {
            self.0.draw_str(text, point(origin), &font, &paint(style));
        }
    }

    fn draw_image(
        &self,
        raster: &gfx::Image,
        origin: gfx::Point,
        quality: gfx::Sampling,
        style: &gfx::Paint,
    ) {
        if let Some(twin) = image(raster) {
            self.0.draw_image_with_sampling_options(
                &twin,
                point(origin),
                sampling(quality),
                Some(&paint(style)),
            );
        }
    }

    fn draw_image_rect(
        &self,
        raster: &gfx::Image,
        dst: gfx::Rect,
        quality: gfx::Sampling,
        style: &gfx::Paint,
    ) {
        if let Some(twin) = image(raster) {
            self.0.draw_image_rect_with_sampling_options(
                &twin,
                None,
                rect(dst),
                sampling(quality),
                &paint(style),
            );
        }
    }

    fn draw_blurred_round_rect(
        &self,
        bounds: gfx::Rect,
        rx: f32,
        ry: f32,
        sigma: f32,
        value: gfx::Color,
    ) {
        let mut style = skia_safe::Paint::default();
        style.set_anti_alias(true);
        style.set_color(color(value));
        style.set_mask_filter(skia_safe::MaskFilter::blur(
            skia_safe::BlurStyle::Normal,
            sigma,
            None,
        ));
        self.0.draw_round_rect(rect(bounds), rx, ry, &style);
    }
}

// ─── offscreen rasters: the test oracle and the property renders ───

/// A premultiplied RGBA surface — the oracle's paper. Draw through
/// `canvas(&mut surface)`.
pub fn raster(width: i32, height: i32) -> Option<skia_safe::Surface> {
    skia_safe::surfaces::raster_n32_premul((width, height))
}

/// TEST ORACLE: the surface's pixels as straight RGBA8 rows —
/// `(width, height, bytes)`. Unpremultiplied, so a solid red reads
/// `[255, 0, 0, 255]` regardless of the surface's storage.
pub fn rgba(surface: &mut skia_safe::Surface) -> (i32, i32, Vec<u8>) {
    let info = skia_safe::ImageInfo::new(
        surface.image_info().dimensions(),
        skia_safe::ColorType::RGBA8888,
        skia_safe::AlphaType::Unpremul,
        None,
    );
    let width = info.width();
    let height = info.height();
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let row = (width * 4) as usize;
    surface
        .canvas()
        .read_pixels(&info, &mut pixels, row, (0, 0));
    (width, height, pixels)
}

/// `raster` with skia's tuple-size call shape — the test oracles'
/// one-line door.
pub fn raster_tuple(size: (i32, i32)) -> Option<skia_safe::Surface> {
    raster(size.0, size.1)
}

/// TEST ORACLE: a skia-read pixel in the app's own color type.
pub fn back_color(color: skia_safe::Color) -> gfx::Color {
    gfx::Color::from_argb(color.a(), color.r(), color.g(), color.b())
}
