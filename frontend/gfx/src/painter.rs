// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::{text::TextBlob, Color, Font, Image, Paint, Path, Point, Rect, Size};

/// How an image is resampled into its destination. The app only
/// ever asks for these two; a backend never sees a free-form
/// sampling descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampling {
    Nearest,
    Linear,
}

/// Everything the widget tree is allowed to ask of a frame. The
/// methods are the surveyed inventory and nothing more: solid
/// paints, rect clips, positioned glyph blobs. Dyn-safe on purpose —
/// a paint pass holds `&Canvas` and never knows the backend.
///
/// `&self` throughout: a backend that records (rather than draws)
/// hides its mutability inside, the way a skia canvas does.
pub trait Painter {
    // ─── state ───
    fn save(&self);
    fn restore(&self);
    fn translate(&self, dx: f32, dy: f32);
    fn scale(&self, sx: f32, sy: f32);
    fn skew(&self, sx: f32, sy: f32);
    /// Rotate by `degrees` about `pivot` (the origin when `None`).
    fn rotate(&self, degrees: f32, pivot: Option<Point>);
    /// Intersect the clip with `rect` — the only clip shape there is.
    fn clip_rect(&self, rect: Rect, anti_alias: bool);
    fn local_clip_bounds(&self) -> Option<Rect>;
    /// The surface the frame paints, in device pixels.
    fn base_layer_size(&self) -> Size;
    /// The largest axis scale from local space to device pixels —
    /// what a raster (an SVG, a diagram) renders at to stay sharp.
    fn device_scale(&self) -> f32;

    // ─── drawing ───
    fn clear(&self, color: Color);
    fn draw_rect(&self, rect: Rect, paint: &Paint);
    fn draw_round_rect(&self, rect: Rect, rx: f32, ry: f32, paint: &Paint);
    fn draw_circle(&self, center: Point, radius: f32, paint: &Paint);
    fn draw_line(&self, from: Point, to: Point, paint: &Paint);
    fn draw_path(&self, path: &Path, paint: &Paint);
    /// Positioned glyphs, shaped upstream — the renderer never sees
    /// a string on this road.
    fn draw_text_blob(&self, blob: &TextBlob, origin: Point, paint: &Paint);
    /// The shaping-free road: gutter numbers, terminal cells, the
    /// menu. Owed to the glyph-run road eventually.
    fn draw_str(&self, text: &str, origin: Point, font: &Font, paint: &Paint);
    fn draw_image(&self, image: &Image, origin: Point, sampling: Sampling, paint: &Paint);
    fn draw_image_rect(&self, image: &Image, dst: Rect, sampling: Sampling, paint: &Paint);
    /// A gaussian-blurred round rect — the sheet drop shadow, and
    /// the only blur there is. `sigma` is the gaussian's, not a CSS
    /// radius.
    fn draw_blurred_round_rect(&self, rect: Rect, rx: f32, ry: f32, sigma: f32, color: Color);
}

impl From<Sampling> for skia_safe::SamplingOptions {
    fn from(sampling: Sampling) -> Self {
        match sampling {
            Sampling::Nearest => skia_safe::SamplingOptions::default(),
            Sampling::Linear => skia_safe::SamplingOptions::from(skia_safe::FilterMode::Linear),
        }
    }
}

/// The skia backend, implemented straight on the canvas a shell
/// owns: a `&skia_safe::Canvas` coerces to `&gfx::Canvas` wherever
/// one is asked for. The reference backend — any other must match
/// it pixel for pixel where the tests look.
impl Painter for skia_safe::Canvas {
    fn save(&self) {
        self.save();
    }

    fn restore(&self) {
        self.restore();
    }

    fn translate(&self, dx: f32, dy: f32) {
        self.translate((dx, dy));
    }

    fn scale(&self, sx: f32, sy: f32) {
        self.scale((sx, sy));
    }

    fn skew(&self, sx: f32, sy: f32) {
        self.skew((sx, sy));
    }

    fn rotate(&self, degrees: f32, pivot: Option<Point>) {
        self.rotate(degrees, pivot);
    }

    fn clip_rect(&self, rect: Rect, anti_alias: bool) {
        self.clip_rect(rect, None, anti_alias);
    }

    fn local_clip_bounds(&self) -> Option<Rect> {
        self.local_clip_bounds()
    }

    fn base_layer_size(&self) -> Size {
        let size = self.base_layer_size();
        Size::new(size.width as f32, size.height as f32)
    }

    fn device_scale(&self) -> f32 {
        let matrix = self.local_to_device_as_3x3();
        matrix.scale_x().abs().max(matrix.scale_y().abs())
    }

    fn clear(&self, color: Color) {
        self.clear(color);
    }

    fn draw_rect(&self, rect: Rect, paint: &Paint) {
        self.draw_rect(rect, paint);
    }

    fn draw_round_rect(&self, rect: Rect, rx: f32, ry: f32, paint: &Paint) {
        self.draw_round_rect(rect, rx, ry, paint);
    }

    fn draw_circle(&self, center: Point, radius: f32, paint: &Paint) {
        self.draw_circle(center, radius, paint);
    }

    fn draw_line(&self, from: Point, to: Point, paint: &Paint) {
        self.draw_line(from, to, paint);
    }

    fn draw_path(&self, path: &Path, paint: &Paint) {
        self.draw_path(path, paint);
    }

    fn draw_text_blob(&self, blob: &TextBlob, origin: Point, paint: &Paint) {
        if let Some(skia) = blob.skia() {
            skia_safe::Canvas::draw_text_blob(self, skia, origin, paint);
        }
    }

    fn draw_str(&self, text: &str, origin: Point, font: &Font, paint: &Paint) {
        self.draw_str(text, origin, font, paint);
    }

    fn draw_image(&self, image: &Image, origin: Point, sampling: Sampling, paint: &Paint) {
        self.draw_image_with_sampling_options(image, origin, sampling, Some(paint));
    }

    fn draw_image_rect(&self, image: &Image, dst: Rect, sampling: Sampling, paint: &Paint) {
        self.draw_image_rect_with_sampling_options(image, None, dst, sampling, paint);
    }

    fn draw_blurred_round_rect(&self, rect: Rect, rx: f32, ry: f32, sigma: f32, color: Color) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(color);
        paint.set_mask_filter(skia_safe::MaskFilter::blur(
            skia_safe::BlurStyle::Normal,
            sigma,
            None,
        ));
        skia_safe::Canvas::draw_round_rect(self, rect, rx, ry, &paint);
    }
}
