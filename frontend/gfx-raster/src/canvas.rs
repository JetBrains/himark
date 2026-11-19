// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The painter. Shapes are moved to device pixels and handed to
//! tiny-skia; the clip is one device rect, applied by geometry
//! throughout: rects and images are intersected with it, and paths
//! and glyphs are coverage masks blended inside it.

use std::cell::RefCell;
use std::sync::Arc;

use gfx::Transform;
use skrifa::MetadataProvider as _;
use tiny_skia::{FillRule, Mask, Path, PathBuilder, Pixmap};

use crate::glyphs::{self, Face, GlyphMask};

/// What outlives one `canvas()` borrow: the transform and clip
/// stack.
#[derive(Default)]
pub(crate) struct State {
    transform: Transform,
}

/// Paints into a [`crate::Surface`]. The `Painter` trait paints
/// through `&self`, so the pixmap sits in a `RefCell`.
pub struct RasterCanvas<'a> {
    pixmap: RefCell<&'a mut Pixmap>,
    state: &'a State,
}

/// Source-over one ink at one coverage across a run of
/// premultiplied pixels.
fn over(run: &mut [[u8; 4]], ink: [u8; 4], covered: f32) {
    let source = (covered * ink[3] as f32 + 0.5) as u32;
    match source {
        0 => {}
        255.. => run.fill([ink[0], ink[1], ink[2], 255]),
        _ => {
            let keep = 255 - source;
            let add = [
                ink[0] as u32 * source,
                ink[1] as u32 * source,
                ink[2] as u32 * source,
                source * 255,
            ];
            for pixel in run {
                for channel in 0..4 {
                    pixel[channel] =
                        ((add[channel] + pixel[channel] as u32 * keep + 127) / 255) as u8;
                }
            }
        }
    }
}

fn device_rect(rect: gfx::Rect) -> Option<tiny_skia::Rect> {
    tiny_skia::Rect::from_ltrb(rect.left, rect.top, rect.right, rect.bottom)
}

impl<'a> RasterCanvas<'a> {
    pub(crate) fn new(pixmap: &'a mut Pixmap, state: &'a State) -> Self {
        Self {
            pixmap: RefCell::new(pixmap),
            state,
        }
    }

    fn surface(&self) -> gfx::Rect {
        let pixmap = self.pixmap.borrow();
        gfx::Rect::from_wh(pixmap.width() as f32, pixmap.height() as f32)
    }

    /// The clip in device pixels, inside the surface.
    fn clip(&self) -> gfx::Rect {
        let surface = self.surface();
        match self.state.transform.clip() {
            Some(clip) => clip
                .intersect(&surface)
                .unwrap_or(gfx::Rect::new(0.0, 0.0, 0.0, 0.0)),
            None => surface,
        }
    }

    fn matrix(&self) -> tiny_skia::Transform {
        let [a, b, c, d, e, f] = self.state.transform.matrix();
        tiny_skia::Transform::from_row(a, b, c, d, e, f)
    }

    /// Fills or strokes a path given in LOCAL coordinates.
    fn draw_local(&self, path: Path, paint: &gfx::Paint) {
        if let Some(path) = path.transform(self.matrix()) {
            self.draw_device(path, paint);
        }
    }

    /// Fills or strokes a path already in DEVICE pixels: its
    /// coverage is rasterized into a mask the size of its visible
    /// part and blended like a glyph — the clip is geometry, and the
    /// cost follows the pixels actually touched.
    fn draw_device(&self, path: Path, paint: &gfx::Paint) {
        let (fills, strokes) = match paint.style() {
            gfx::PaintStyle::Fill => (true, false),
            gfx::PaintStyle::Stroke => (false, true),
            gfx::PaintStyle::StrokeAndFill => (true, true),
        };
        let outline = match strokes {
            true => {
                let stroke = tiny_skia::Stroke {
                    width: (paint.stroke_width() * self.state.transform.magnitude()).max(1.0),
                    line_cap: match paint.stroke_cap() {
                        gfx::PaintCap::Butt => tiny_skia::LineCap::Butt,
                        gfx::PaintCap::Round => tiny_skia::LineCap::Round,
                        gfx::PaintCap::Square => tiny_skia::LineCap::Square,
                    },
                    line_join: match paint.stroke_join() {
                        gfx::PaintJoin::Miter => tiny_skia::LineJoin::Miter,
                        gfx::PaintJoin::Round => tiny_skia::LineJoin::Round,
                        gfx::PaintJoin::Bevel => tiny_skia::LineJoin::Bevel,
                    },
                    ..Default::default()
                };
                path.stroke(&stroke, 1.0)
            }
            false => None,
        };
        let shapes: [Option<&Path>; 2] = [fills.then_some(&path), outline.as_ref()];
        let Some(bounds) = shapes
            .iter()
            .flatten()
            .map(|shape| {
                let bounds = shape.bounds();
                gfx::Rect::new(bounds.left(), bounds.top(), bounds.right(), bounds.bottom())
            })
            .reduce(|all, next| all.union(&next))
        else {
            return;
        };
        let clip = self.clip();
        let left = (bounds.left.floor() as i32).max(clip.left.floor() as i32);
        let top = (bounds.top.floor() as i32).max(clip.top.floor() as i32);
        let right = (bounds.right.ceil() as i32).min(clip.right.ceil() as i32);
        let bottom = (bounds.bottom.ceil() as i32).min(clip.bottom.ceil() as i32);
        if left >= right || top >= bottom {
            return;
        }
        let (width, height) = ((right - left) as u32, (bottom - top) as u32);
        let Some(mut mask) = Mask::new(width, height) else {
            return;
        };
        let placed = tiny_skia::Transform::from_translate(-left as f32, -top as f32);
        let mut coverage: Option<Vec<u8>> = None;
        for shape in shapes.into_iter().flatten() {
            mask.clear();
            mask.fill_path(shape, FillRule::Winding, paint.is_anti_alias(), placed);
            // A fill and its stroke are one shape: the union, not
            // one laid over the other.
            coverage = Some(match coverage {
                Some(mut all) => {
                    for (into, from) in all.iter_mut().zip(mask.data()) {
                        *into = (*into).max(*from);
                    }
                    all
                }
                None => mask.data().to_vec(),
            });
        }
        let Some(coverage) = coverage else {
            return;
        };
        self.blend(
            &GlyphMask {
                left: 0,
                top: 0,
                width,
                height,
                coverage,
            },
            left,
            top,
            paint.color(),
        );
    }

    /// A device rect filled inside the clip, written directly: a
    /// pixel's coverage is how much of it the rect overlaps, so a
    /// fractional edge is exact and a whole-pixel one is free.
    fn fill_device_rect(&self, rect: gfx::Rect, paint: &gfx::Paint) {
        let (l, t, r, b) = match paint.is_anti_alias() {
            true => (rect.left, rect.top, rect.right, rect.bottom),
            false => (
                rect.left.round(),
                rect.top.round(),
                rect.right.round(),
                rect.bottom.round(),
            ),
        };
        let clip = self.clip();
        let from_x = (l.floor() as i32).max(clip.left.ceil() as i32);
        let from_y = (t.floor() as i32).max(clip.top.ceil() as i32);
        let to_x = (r.ceil() as i32).min(clip.right.floor() as i32);
        let to_y = (b.ceil() as i32).min(clip.bottom.floor() as i32);
        if from_x >= to_x || from_y >= to_y {
            return;
        }
        // How much of the unit pixel at `at` the span `from..to` covers.
        let overlap = |at: i32, from: f32, to: f32| {
            ((at as f32 + 1.0).min(to) - (at as f32).max(from)).clamp(0.0, 1.0)
        };
        let color = paint.color();
        let ink = [color.r(), color.g(), color.b(), color.a()];
        // The first and last columns may be partly covered; every
        // column between them is whole.
        let first = overlap(from_x, l, r);
        let last = overlap(to_x - 1, l, r);
        let width = (to_x - from_x) as usize;
        let mut pixmap = self.pixmap.borrow_mut();
        let stride = pixmap.width() as usize * 4;
        let data = pixmap.data_mut();
        for y in from_y..to_y {
            let down = overlap(y, t, b);
            let at = y as usize * stride + from_x as usize * 4;
            let row = data[at..at + width * 4].as_chunks_mut::<4>().0;
            match width {
                1 => over(&mut row[..1], ink, first.min(last) * down),
                _ => {
                    over(&mut row[..1], ink, first * down);
                    over(&mut row[1..width - 1], ink, down);
                    over(&mut row[width - 1..], ink, last * down);
                }
            }
        }
    }

    /// An axis-aligned filled rounded rect without one big path: the
    /// span between the rounded ends is a rect fill and only the two
    /// ends are curves (a pill is all ends and still splits). The
    /// pieces meet on whole-pixel columns, so no pixel is covered
    /// twice and none is left thin. Answers `false` when the shape
    /// is too narrow to split that way.
    fn round_rect_in_pieces(&self, rect: gfx::Rect, rx: f32, ry: f32, paint: &gfx::Paint) -> bool {
        const KAPPA: f32 = 0.552_284_8;
        let [a, _, _, d, _, _] = self.state.transform.matrix();
        let placed = self.state.transform.rect(rect);
        let (rx, ry) = (rx * a, ry * d);
        let (l, t, r, b) = (placed.left, placed.top, placed.right, placed.bottom);
        let (x1, x2) = ((l + rx).ceil(), (r - rx).floor());
        if x1 >= x2 {
            return false;
        }
        self.fill_device_rect(gfx::Rect::new(x1, t, x2, b), paint);
        let (kx, ky) = (rx * KAPPA, ry * KAPPA);
        // Each end: from the split column along the top, around
        // both corners, back along the bottom.
        let ends = [(x1, l, l + rx, l + rx - kx), (x2, r, r - rx, r - rx + kx)];
        for (split, edge, arc, control) in ends {
            let mut builder = PathBuilder::new();
            builder.move_to(split, t);
            builder.line_to(arc, t);
            builder.cubic_to(control, t, edge, t + ry - ky, edge, t + ry);
            builder.line_to(edge, b - ry);
            builder.cubic_to(edge, b - ry + ky, control, b, arc, b);
            builder.line_to(split, b);
            builder.close();
            if let Some(path) = builder.finish() {
                self.draw_device(path, paint);
            }
        }
        true
    }

    fn round_rect(&self, rect: gfx::Rect, rx: f32, ry: f32, paint: &gfx::Paint) {
        let rx = rx.min(rect.width() / 2.0).max(0.0);
        let ry = ry.min(rect.height() / 2.0).max(0.0);
        if rx <= 0.0 || ry <= 0.0 {
            self.rect(rect, paint);
            return;
        }
        if matches!(paint.style(), gfx::PaintStyle::Fill)
            && self.state.transform.axis_aligned()
            && self.round_rect_in_pieces(rect, rx, ry, paint)
        {
            return;
        }
        // Four quarter-ellipses as cubics.
        const KAPPA: f32 = 0.552_284_8;
        let (kx, ky) = (rx * KAPPA, ry * KAPPA);
        let (l, t, r, b) = (rect.left, rect.top, rect.right, rect.bottom);
        let mut builder = PathBuilder::new();
        builder.move_to(l + rx, t);
        builder.line_to(r - rx, t);
        builder.cubic_to(r - rx + kx, t, r, t + ry - ky, r, t + ry);
        builder.line_to(r, b - ry);
        builder.cubic_to(r, b - ry + ky, r - rx + kx, b, r - rx, b);
        builder.line_to(l + rx, b);
        builder.cubic_to(l + rx - kx, b, l, b - ry + ky, l, b - ry);
        builder.line_to(l, t + ry);
        builder.cubic_to(l, t + ry - ky, l + rx - kx, t, l + rx, t);
        builder.close();
        if let Some(path) = builder.finish() {
            self.draw_local(path, paint);
        }
    }

    fn rect(&self, rect: gfx::Rect, paint: &gfx::Paint) {
        // An axis-aligned fill is clipped by geometry and needs no
        // path at all.
        if matches!(paint.style(), gfx::PaintStyle::Fill) && self.state.transform.axis_aligned() {
            self.fill_device_rect(self.state.transform.rect(rect), paint);
            return;
        }
        if let Some(rect) = device_rect(rect) {
            self.draw_local(PathBuilder::from_rect(rect), paint);
        }
    }

    /// Blends `color` through a coverage mask whose top-left is at
    /// device `(left, top)`, inside the clip.
    fn blend(&self, coverage: &GlyphMask, left: i32, top: i32, color: gfx::Color) {
        let clip = self.clip();
        let from_x = left.max(clip.left.ceil() as i32);
        let from_y = top.max(clip.top.ceil() as i32);
        let to_x = (left + coverage.width as i32).min(clip.right.floor() as i32);
        let to_y = (top + coverage.height as i32).min(clip.bottom.floor() as i32);
        if from_x >= to_x || from_y >= to_y {
            return;
        }
        let mut pixmap = self.pixmap.borrow_mut();
        let stride = pixmap.width() as usize * 4;
        let data = pixmap.data_mut();
        let (red, green, blue, alpha) = (
            color.r() as u32,
            color.g() as u32,
            color.b() as u32,
            color.a() as u32,
        );
        for y in from_y..to_y {
            let row = (y - top) as usize * coverage.width as usize;
            for x in from_x..to_x {
                let covered = coverage.coverage[row + (x - left) as usize] as u32;
                if covered == 0 {
                    continue;
                }
                // Source-over on premultiplied bytes.
                let source = covered * alpha / 255;
                let keep = 255 - source;
                let at = y as usize * stride + x as usize * 4;
                let mix = |channel: u32, under: u8| {
                    ((channel * source + under as u32 * keep + 127) / 255) as u8
                };
                data[at] = mix(red, data[at]);
                data[at + 1] = mix(green, data[at + 1]);
                data[at + 2] = mix(blue, data[at + 2]);
                data[at + 3] = (source + (data[at + 3] as u32 * keep + 127) / 255).min(255) as u8;
            }
        }
    }

    /// One glyph with its baseline origin at LOCAL `(x, y)`.
    fn glyph(&self, face: &Face<'_>, glyph: u16, (x, y): (f32, f32), cut: Cut) {
        let size = cut.size * self.state.transform.magnitude();
        let Some(mask) = glyphs::mask(face, glyph, size, cut.skew_x, cut.embolden) else {
            return;
        };
        let (x, y) = self.state.transform.point(x, y);
        self.blend(
            &mask,
            x.round() as i32 + mask.left,
            y.round() as i32 + mask.top,
            cut.color,
        );
    }
}

/// How a run's glyphs are cut and inked.
#[derive(Clone, Copy)]
struct Cut {
    size: f32,
    skew_x: f32,
    embolden: bool,
    color: gfx::Color,
}

/// An image as a premultiplied pixmap — built once, kept on the
/// image itself.
fn pixmap_of(image: &gfx::Image) -> Option<Arc<Pixmap>> {
    image
        .backend(|image| {
            let mut bytes = image.pixels().to_vec();
            if !image.is_premultiplied() {
                for pixel in bytes.as_chunks_mut::<4>().0 {
                    let alpha = pixel[3] as u32;
                    for channel in &mut pixel[..3] {
                        *channel = ((*channel as u32 * alpha + 127) / 255) as u8;
                    }
                }
            }
            let size = tiny_skia::IntSize::from_wh(image.width(), image.height())?;
            Pixmap::from_vec(bytes, size).map(Arc::new)
        })
        .cloned()
        .flatten()
}

/// Three box passes approximate a gaussian of `sigma`.
fn blur(coverage: &mut [u8], width: usize, height: usize, sigma: f32) {
    let radius = ((sigma * 3.0_f32.sqrt()).round() as usize).max(1);
    let mut scratch = vec![0u8; coverage.len()];
    for _ in 0..3 {
        // Rows into scratch, then columns back.
        for y in 0..height {
            let row = &coverage[y * width..(y + 1) * width];
            let mut sum: u32 = 0;
            let window = (2 * radius + 1) as u32;
            for x in 0..width + radius {
                if x < width {
                    sum += row[x] as u32;
                }
                if x > 2 * radius {
                    sum -= row[x - 2 * radius - 1] as u32;
                }
                if x >= radius {
                    scratch[y * width + x - radius] = (sum / window) as u8;
                }
            }
        }
        for x in 0..width {
            let mut sum: u32 = 0;
            let window = (2 * radius + 1) as u32;
            for y in 0..height + radius {
                if y < height {
                    sum += scratch[y * width + x] as u32;
                }
                if y > 2 * radius {
                    sum -= scratch[(y - 2 * radius - 1) * width + x] as u32;
                }
                if y >= radius {
                    coverage[(y - radius) * width + x] = (sum / window) as u8;
                }
            }
        }
    }
}

impl gfx::Painter for RasterCanvas<'_> {
    fn save(&self) {
        self.state.transform.save();
    }

    fn restore(&self) {
        self.state.transform.restore();
    }

    fn translate(&self, dx: f32, dy: f32) {
        self.state.transform.translate(dx, dy);
    }

    fn scale(&self, sx: f32, sy: f32) {
        self.state.transform.scale(sx, sy);
    }

    fn skew(&self, sx: f32, sy: f32) {
        self.state.transform.skew(sx, sy);
    }

    fn rotate(&self, degrees: f32, pivot: Option<gfx::Point>) {
        self.state.transform.rotate(degrees, pivot);
    }

    fn clip_rect(&self, rect: gfx::Rect, _anti_alias: bool) {
        self.state.transform.clip_rect(rect);
    }

    fn local_clip_bounds(&self) -> Option<gfx::Rect> {
        self.state.transform.local_clip_bounds()
    }

    fn base_layer_size(&self) -> gfx::Size {
        let surface = self.surface();
        gfx::Size::new(surface.width(), surface.height())
    }

    fn device_scale(&self) -> f32 {
        self.state.transform.magnitude()
    }

    fn clear(&self, color: gfx::Color) {
        // A clear REPLACES what the clip covers, with the color's
        // premultiplied bytes.
        let clip = self.clip();
        let (from_x, to_x) = (clip.left.ceil() as usize, clip.right.floor() as usize);
        let (from_y, to_y) = (clip.top.ceil() as usize, clip.bottom.floor() as usize);
        if from_x >= to_x {
            return;
        }
        let alpha = color.a() as u32;
        let scaled = |channel: u8| ((channel as u32 * alpha + 127) / 255) as u8;
        let ink = [
            scaled(color.r()),
            scaled(color.g()),
            scaled(color.b()),
            color.a(),
        ];
        let mut pixmap = self.pixmap.borrow_mut();
        let stride = pixmap.width() as usize * 4;
        let data = pixmap.data_mut();
        for y in from_y..to_y {
            let row = &mut data[y * stride + from_x * 4..y * stride + to_x * 4];
            for pixel in row.as_chunks_mut::<4>().0 {
                *pixel = ink;
            }
        }
    }

    fn draw_rect(&self, rect: gfx::Rect, paint: &gfx::Paint) {
        self.rect(rect, paint);
    }

    fn draw_round_rect(&self, rect: gfx::Rect, rx: f32, ry: f32, paint: &gfx::Paint) {
        self.round_rect(rect, rx, ry, paint);
    }

    fn draw_circle(&self, center: gfx::Point, radius: f32, paint: &gfx::Paint) {
        if let Some(path) = PathBuilder::from_circle(center.x, center.y, radius) {
            self.draw_local(path, paint);
        }
    }

    fn draw_line(&self, from: gfx::Point, to: gfx::Point, paint: &gfx::Paint) {
        let mut builder = PathBuilder::new();
        builder.move_to(from.x, from.y);
        builder.line_to(to.x, to.y);
        let Some(path) = builder.finish() else {
            return;
        };
        // A line is its stroke, whatever the paint's style says.
        let mut stroke = paint.clone();
        stroke.set_style(gfx::PaintStyle::Stroke);
        self.draw_local(path, &stroke);
    }

    fn draw_path(&self, path: &gfx::Path, paint: &gfx::Paint) {
        let mut builder = PathBuilder::new();
        for verb in path.verbs() {
            match *verb {
                gfx::path::Verb::Move(to) => builder.move_to(to.x, to.y),
                gfx::path::Verb::Line(to) => builder.line_to(to.x, to.y),
                gfx::path::Verb::Quad(control, to) => {
                    builder.quad_to(control.x, control.y, to.x, to.y)
                }
                gfx::path::Verb::Cubic(first, second, to) => {
                    builder.cubic_to(first.x, first.y, second.x, second.y, to.x, to.y)
                }
                gfx::path::Verb::Close => builder.close(),
            }
        }
        if let Some(path) = builder.finish() {
            self.draw_local(path, paint);
        }
    }

    fn draw_text_blob(&self, blob: &gfx::TextBlob, origin: gfx::Point, paint: &gfx::Paint) {
        for run in blob.runs() {
            let entry = gfx::face(run.face);
            let face = Face {
                data: &entry.data,
                index: entry.index,
                coords: &entry.coords,
            };
            let cut = Cut {
                size: run.size,
                skew_x: run.skew_x,
                embolden: run.embolden,
                color: paint.color(),
            };
            for (glyph, position) in &run.glyphs {
                self.glyph(
                    &face,
                    *glyph,
                    (origin.x + position.x, origin.y + position.y),
                    cut,
                );
            }
        }
    }

    fn draw_str(&self, text: &str, origin: gfx::Point, font: &gfx::Font, paint: &gfx::Paint) {
        let typeface = font.typeface();
        let data = typeface.data();
        let Ok(reference) = skrifa::FontRef::from_index((*data).as_ref(), typeface.index()) else {
            return;
        };
        let location = reference.axes().location(
            typeface
                .coords()
                .iter()
                .map(|(tag, value)| (skrifa::Tag::from_be_bytes(tag.to_be_bytes()), *value)),
        );
        let metrics = reference.glyph_metrics(skrifa::instance::Size::new(font.size()), &location);
        let charmap = reference.charmap();
        let face = Face {
            data: &data,
            index: typeface.index(),
            coords: typeface.coords(),
        };
        let cut = Cut {
            size: font.size(),
            skew_x: 0.0,
            embolden: font.is_embolden(),
            color: paint.color(),
        };
        let mut pen = origin.x;
        for ch in text.chars() {
            let glyph = charmap.map(ch).unwrap_or_default();
            self.glyph(&face, glyph.to_u32() as u16, (pen, origin.y), cut);
            pen += metrics.advance_width(glyph).unwrap_or_default();
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
        sampling: gfx::Sampling,
        paint: &gfx::Paint,
    ) {
        let Some(source) = pixmap_of(image) else {
            return;
        };
        // Images are placed under translation and scale only.
        let placed = self.state.transform.rect(dst);
        let Some(visible) = placed.intersect(&self.clip()).and_then(device_rect) else {
            return;
        };
        let scale_x = placed.width() / source.width() as f32;
        let scale_y = placed.height() / source.height() as f32;
        let shader = tiny_skia::Pattern::new(
            source.as_ref().as_ref(),
            tiny_skia::SpreadMode::Pad,
            match sampling {
                gfx::Sampling::Nearest => tiny_skia::FilterQuality::Nearest,
                gfx::Sampling::Linear => tiny_skia::FilterQuality::Bilinear,
            },
            paint.color().a() as f32 / 255.0,
            tiny_skia::Transform::from_row(scale_x, 0.0, 0.0, scale_y, placed.left, placed.top),
        );
        let fill = tiny_skia::Paint {
            shader,
            ..Default::default()
        };
        self.pixmap
            .borrow_mut()
            .fill_rect(visible, &fill, tiny_skia::Transform::identity(), None);
    }

    fn draw_blurred_round_rect(
        &self,
        rect: gfx::Rect,
        rx: f32,
        ry: f32,
        sigma: f32,
        color: gfx::Color,
    ) {
        let placed = self.state.transform.rect(rect);
        let scale = self.state.transform.magnitude();
        let sigma = (sigma * scale).max(0.1);
        let reach = (sigma * 3.0).ceil();
        let left = (placed.left - reach).floor() as i32;
        let top = (placed.top - reach).floor() as i32;
        let width = ((placed.right + reach).ceil() as i32 - left).max(1) as u32;
        let height = ((placed.bottom + reach).ceil() as i32 - top).max(1) as u32;
        // A shape far larger than any surface is a geometry bug
        // upstream, not a shadow to allocate.
        if width > 16_384 || height > 16_384 {
            return;
        }
        let Some(mut mask) = Mask::new(width, height) else {
            return;
        };
        let shape = tiny_skia::Rect::from_ltrb(
            placed.left - left as f32,
            placed.top - top as f32,
            placed.right - left as f32,
            placed.bottom - top as f32,
        );
        let Some(shape) = shape else {
            return;
        };
        let radius = (rx.max(ry) * scale)
            .min(shape.width() / 2.0)
            .min(shape.height() / 2.0);
        let path = match radius > 0.0 {
            true => {
                const KAPPA: f32 = 0.552_284_8;
                let k = radius * KAPPA;
                let (l, t, r, b) = (shape.left(), shape.top(), shape.right(), shape.bottom());
                let mut builder = PathBuilder::new();
                builder.move_to(l + radius, t);
                builder.line_to(r - radius, t);
                builder.cubic_to(r - radius + k, t, r, t + radius - k, r, t + radius);
                builder.line_to(r, b - radius);
                builder.cubic_to(r, b - radius + k, r - radius + k, b, r - radius, b);
                builder.line_to(l + radius, b);
                builder.cubic_to(l + radius - k, b, l, b - radius + k, l, b - radius);
                builder.line_to(l, t + radius);
                builder.cubic_to(l, t + radius - k, l + radius - k, t, l + radius, t);
                builder.close();
                builder.finish()
            }
            false => Some(PathBuilder::from_rect(shape)),
        };
        let Some(path) = path else {
            return;
        };
        mask.fill_path(
            &path,
            FillRule::Winding,
            true,
            tiny_skia::Transform::identity(),
        );
        let mut coverage = mask.take();
        blur(&mut coverage, width as usize, height as usize, sigma);
        self.blend(
            &GlyphMask {
                left: 0,
                top: 0,
                width,
                height,
                coverage,
            },
            left,
            top,
            color,
        );
    }
}
