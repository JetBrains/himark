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
