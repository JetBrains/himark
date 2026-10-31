// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! `gfx::Painter` over gpui — the first non-skia backend. Quads,
//! glyphs, paths and images map onto gpui's scene primitives; the
//! save/restore stream the widget tree speaks becomes an affine
//! matrix and one intersected clip rect per draw call (the app only
//! ever clips to rects, so a `ContentMask` carries the whole stack).
//!
//! Known v1 degradations, all cosmetic: faux-italic skew leaves
//! glyphs upright (positions still shear), stroked shapes draw as
//! inside borders rather than skia's centered strokes, and
//! `draw_str` shapes through gpui's text system, whose advances may
//! drift a pixel from the skia metrics the caller measured with.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    fill, point, px, quad, App, Background, BorderStyle, Bounds, BoxShadow, ContentMask, Corners,
    Edges, FontId, Hsla, Pixels, Point, RenderImage, SharedString, TextRun, Window,
};
use smallvec::smallvec;

/// Backend state that outlives a frame: resolved faces and uploaded
/// images. Keyed by gfx's ids, owned by the root view.
#[derive(Default)]
pub struct PainterCaches {
    fonts: HashMap<gfx::FaceId, FontId>,
    images: HashMap<u32, Arc<RenderImage>>,
}

/// Row-major 2D affine: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Clone, Copy)]
struct Mat {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Mat {
    const IDENTITY: Mat = Mat {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn concat(self, l: Mat) -> Mat {
        Mat {
            a: self.a * l.a + self.c * l.b,
            b: self.b * l.a + self.d * l.b,
            c: self.a * l.c + self.c * l.d,
            d: self.b * l.c + self.d * l.d,
            e: self.a * l.e + self.c * l.f + self.e,
            f: self.b * l.e + self.d * l.f + self.f,
        }
    }

    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// Translation and positive scale only — the fast lane where a
    /// rect stays a rect.
    fn axis_aligned(&self) -> bool {
        self.b == 0.0 && self.c == 0.0 && self.a > 0.0 && self.d > 0.0
    }

    /// The uniform magnitude for widths and radii.
    fn scale(&self) -> f32 {
        ((self.a * self.d - self.b * self.c).abs()).sqrt()
    }
}

#[derive(Clone, Copy)]
struct Frame {
    matrix: Mat,
    /// The intersected clip, in window-logical pixels.
    clip: Option<Bounds<Pixels>>,
}

/// `gfx::Canvas` is `dyn Painter` behind a bare type alias, and the
/// object-lifetime default through an alias is `'static` — a painter
/// that BORROWS the window can never coerce. So the window rides as
/// a raw pointer, the same shape the ffi shells pass their canvases
/// in.
///
/// SAFETY: a `GpuiPainter` lives strictly inside one canvas-element
/// paint callback; `engine.draw` dispatches `Event::Paint` borrows
/// and retains nothing. The pointers are dereferenced one method at
/// a time on the ui thread.
pub struct GpuiPainter {
    window: *mut Window,
    cx: *mut App,
    /// The canvas element's origin — local (0,0) in window space.
    origin: Point<Pixels>,
    /// Device pixels per logical pixel: the engine speaks device,
    /// gpui speaks logical.
    scale: f32,
    caches: Rc<RefCell<PainterCaches>>,
    state: RefCell<State>,
}

struct State {
    current: Frame,
    stack: Vec<Frame>,
}

/// gpui's `GlyphId` field is `pub(crate)` — there is no public way
/// to hand the renderer a raw glyph id, which is this backend's
/// whole text road. It is `#[repr(C)]` over one `u32`, so until a
/// constructor lands upstream, transmute — the version is pinned to
/// `=0.2.2` in Cargo.toml for exactly this reason.
fn glyph_id(id: u16) -> gpui::GlyphId {
    const _: () = assert!(std::mem::size_of::<gpui::GlyphId>() == std::mem::size_of::<u32>());
    unsafe { std::mem::transmute::<u32, gpui::GlyphId>(id as u32) }
}

fn color(color: gfx::Color) -> Hsla {
    Hsla::from(gpui::Rgba {
        r: color.r() as f32 / 255.0,
        g: color.g() as f32 / 255.0,
        b: color.b() as f32 / 255.0,
        a: color.a() as f32 / 255.0,
    })
}

impl GpuiPainter {
    pub fn new(
        window: &mut Window,
        cx: &mut App,
        origin: Point<Pixels>,
        scale: f32,
        caches: Rc<RefCell<PainterCaches>>,
    ) -> Self {
        Self {
            window,
            cx,
            origin,
            scale,
            caches,
            state: RefCell::new(State {
                current: Frame {
                    matrix: Mat::IDENTITY,
                    clip: None,
                },
                stack: Vec::new(),
            }),
        }
    }

    /// One reborrow per method scope — see the type's SAFETY note.
    #[allow(clippy::mut_from_ref)]
    fn window(&self) -> &mut Window {
        unsafe { &mut *self.window }
    }

    #[allow(clippy::mut_from_ref)]
    fn app(&self) -> &mut App {
        unsafe { &mut *self.cx }
    }

    /// A local device-pixel point to window-logical.
    fn to_window(&self, x: f32, y: f32) -> Point<Pixels> {
        let m = self.state.borrow().current.matrix;
        let (dx, dy) = m.apply(x, y);
        point(
            self.origin.x + px(dx / self.scale),
            self.origin.y + px(dy / self.scale),
        )
    }

    /// A local rect to window-logical bounds — the matrix must be
    /// axis-aligned; under rotation or skew callers go through
    /// paths instead.
    fn to_bounds(&self, rect: gfx::Rect) -> Bounds<Pixels> {
        let top_left = self.to_window(rect.left, rect.top);
        let bottom_right = self.to_window(rect.right, rect.bottom);
        Bounds {
            origin: top_left,
            size: gpui::Size {
                width: bottom_right.x - top_left.x,
                height: bottom_right.y - top_left.y,
            },
        }
    }

    /// Widths, radii, font sizes: device-local scalar to logical.
    fn to_len(&self, length: f32) -> Pixels {
        let m = self.state.borrow().current.matrix;
        px(length * m.scale() / self.scale)
    }

    fn with_mask(&self, paint: impl FnOnce(&mut Window)) {
        let clip = self.state.borrow().current.clip;
        let window = self.window();
        match clip {
            Some(bounds) => window.with_content_mask(Some(ContentMask { bounds }), |window| {
                paint(window)
            }),
            None => paint(window),
        }
    }

    /// Fill vs stroke, straight off the paint. A stroke becomes a
    /// border on the quad, outset by half the width to approximate
    /// skia's centered stroke.
    fn paint_quad_styled(&self, rect: gfx::Rect, rx: f32, ry: f32, paint: &gfx::Paint) {
        if !self.state.borrow().current.matrix.axis_aligned() {
            self.draw_round_rect_as_path(rect, rx.max(ry), paint);
            return;
        }
        let fill_color = color(paint.color());
        let radius = self.to_len(rx.max(ry));
        let quad = match paint.style() {
            gfx::PaintStyle::Fill | gfx::PaintStyle::StrokeAndFill => quad(
                self.to_bounds(rect),
                Corners::all(radius),
                fill_color,
                Edges::all(px(0.0)),
                Hsla::transparent_black(),
                BorderStyle::Solid,
            ),
            gfx::PaintStyle::Stroke => {
                let width = self.to_len(paint.stroke_width().max(1.0));
                let mut bounds = self.to_bounds(rect);
                let half = width * 0.5;
                bounds.origin.x -= half;
                bounds.origin.y -= half;
                bounds.size.width += width;
                bounds.size.height += width;
                quad(
                    bounds,
                    Corners::all(radius + half),
                    Hsla::transparent_black(),
                    Edges::all(width),
                    fill_color,
                    BorderStyle::Solid,
                )
            }
        };
        self.with_mask(move |window| window.paint_quad(quad));
    }

    fn draw_round_rect_as_path(&self, rect: gfx::Rect, radius: f32, paint: &gfx::Paint) {
        // The rotated-rect road: good enough for the one spinning
        // chevron and any future rotated chrome — corners square
        // off when a radius meets a rotation.
        let _ = radius;
        let mut builder = self.path_builder(paint);
        builder.move_to(self.to_window(rect.left, rect.top));
        builder.line_to(self.to_window(rect.right, rect.top));
        builder.line_to(self.to_window(rect.right, rect.bottom));
        builder.line_to(self.to_window(rect.left, rect.bottom));
        builder.close();
        self.paint_built(builder, paint);
    }

    fn path_builder(&self, paint: &gfx::Paint) -> gpui::PathBuilder {
        match paint.style() {
            gfx::PaintStyle::Stroke => {
                gpui::PathBuilder::stroke(self.to_len(paint.stroke_width().max(1.0)))
            }
            _ => gpui::PathBuilder::fill(),
        }
    }

    fn paint_built(&self, builder: gpui::PathBuilder, paint: &gfx::Paint) {
        if let Ok(path) = builder.build() {
            let fill_color = color(paint.color());
            self.with_mask(move |window| window.paint_path(path, fill_color));
        }
    }

    fn font_id(&self, face: gfx::FaceId) -> FontId {
        if let Some(hit) = self.caches.borrow().fonts.get(&face) {
            return *hit;
        }
        let entry = gfx::face(face);
        let text_system = self.app().text_system();

        // A face with variation coords gets its instance BAKED into
        // the bytes under a unique family (gpui's font-kit matching
        // never applies axes — see instance.rs); a plain face rides
        // its own bytes and family name.
        let (bytes, family, weight) = match crate::instance::instanced(face.0, &entry) {
            Some((bytes, family)) => (bytes, family, gpui::FontWeight::default()),
            None => (
                entry.data.as_ref().as_ref().to_vec(),
                entry.family.clone().unwrap_or_default(),
                entry
                    .coords
                    .iter()
                    .find(|(tag, _)| *tag == u32::from_be_bytes(*b"wght"))
                    .map(|(_, value)| gpui::FontWeight(*value))
                    .unwrap_or_default(),
            ),
        };
        if let Err(error) = text_system.add_fonts(vec![std::borrow::Cow::Owned(bytes)]) {
            eprintln!("[himark-gpui] add_fonts({family}): {error}");
        }
        if std::env::var_os("HIMARK_TRACE_FONTS").is_some() {
            eprintln!(
                "[fonts] face {} family={family:?} coords={:?} weight={weight:?}",
                face.0, entry.coords
            );
        }
        let font = gpui::Font {
            family: SharedString::from(family),
            features: Default::default(),
            fallbacks: None,
            weight,
            style: Default::default(),
        };
        let id = text_system.resolve_font(&font);
        self.caches.borrow_mut().fonts.insert(face, id);
        id
    }

    fn render_image(&self, image: &gfx::Image) -> Option<Arc<RenderImage>> {
        let key = image.unique_id();
        if let Some(hit) = self.caches.borrow().images.get(&key) {
            return Some(hit.clone());
        }
        let width = image.width() as u32;
        let height = image.height() as u32;
        // gpui's RenderImage is BGRA behind an RGBA-typed buffer;
        // skia converts on readback.
        let info = gfx::ImageInfo::new(
            (width as i32, height as i32),
            gfx::ColorType::BGRA8888,
            gfx::AlphaType::Unpremul,
            None,
        );
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let row_bytes = width as usize * 4;
        if !image.read_pixels(
            &info,
            &mut pixels,
            row_bytes,
            (0, 0),
            gfx::image::CachingHint::Allow,
        ) {
            return None;
        }
        let buffer = image::RgbaImage::from_raw(width, height, pixels)?;
        let rendered = Arc::new(RenderImage::new(smallvec![image::Frame::new(buffer)]));
        self.caches
            .borrow_mut()
            .images
            .insert(key, rendered.clone());
        Some(rendered)
    }
}

impl gfx::Painter for GpuiPainter {
    fn save(&self) {
        let mut state = self.state.borrow_mut();
        let current = state.current;
        state.stack.push(current);
    }

    fn restore(&self) {
        let mut state = self.state.borrow_mut();
        if let Some(frame) = state.stack.pop() {
            state.current = frame;
        }
    }

    fn translate(&self, dx: f32, dy: f32) {
        let mut state = self.state.borrow_mut();
        state.current.matrix = state.current.matrix.concat(Mat {
            e: dx,
            f: dy,
            ..Mat::IDENTITY
        });
    }

    fn scale(&self, sx: f32, sy: f32) {
        let mut state = self.state.borrow_mut();
        state.current.matrix = state.current.matrix.concat(Mat {
            a: sx,
            d: sy,
            ..Mat::IDENTITY
        });
    }

    fn skew(&self, sx: f32, sy: f32) {
        let mut state = self.state.borrow_mut();
        state.current.matrix = state.current.matrix.concat(Mat {
            b: sy,
            c: sx,
            ..Mat::IDENTITY
        });
    }

    fn rotate(&self, degrees: f32, pivot: Option<gfx::Point>) {
        let radians = degrees.to_radians();
        let (sin, cos) = radians.sin_cos();
        let rotation = Mat {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        };
        let local = match pivot {
            Some(p) => Mat {
                e: p.x,
                f: p.y,
                ..Mat::IDENTITY
            }
            .concat(rotation)
            .concat(Mat {
                e: -p.x,
                f: -p.y,
                ..Mat::IDENTITY
            }),
            None => rotation,
        };
        let mut state = self.state.borrow_mut();
        state.current.matrix = state.current.matrix.concat(local);
    }

    fn clip_rect(&self, rect: gfx::Rect, _anti_alias: bool) {
        // The app clips to rects under translation only; under a
        // rare rotated frame the transformed bounds over-approximate.
        let bounds = self.to_bounds(rect);
        let mut state = self.state.borrow_mut();
        state.current.clip = Some(match state.current.clip {
            Some(clip) => clip.intersect(&bounds),
            None => bounds,
        });
    }

    fn local_clip_bounds(&self) -> Option<gfx::Rect> {
        let state = self.state.borrow();
        let clip = state.current.clip?;
        let m = state.current.matrix;
        if !m.axis_aligned() {
            return None;
        }
        // Window-logical back to local device: undo origin, scale
        // up, then invert the axis-aligned matrix.
        let local = |p: Point<Pixels>| {
            let dx = f32::from(p.x - self.origin.x) * self.scale;
            let dy = f32::from(p.y - self.origin.y) * self.scale;
            ((dx - m.e) / m.a, (dy - m.f) / m.d)
        };
        let (left, top) = local(clip.origin);
        let (right, bottom) = local(clip.bottom_right());
        Some(gfx::Rect::new(left, top, right, bottom))
    }

    fn base_layer_size(&self) -> gfx::Size {
        let window = self.window();
        let size = window.viewport_size();
        gfx::Size::new(
            f32::from(size.width) * self.scale,
            f32::from(size.height) * self.scale,
        )
    }

    fn device_scale(&self) -> f32 {
        self.state.borrow().current.matrix.scale() // local → device px
    }

    fn clear(&self, color_value: gfx::Color) {
        let window_bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: self.window().viewport_size(),
        };
        let quad = fill(window_bounds, color(color_value));
        self.with_mask(move |window| window.paint_quad(quad));
    }

    fn draw_rect(&self, rect: gfx::Rect, paint: &gfx::Paint) {
        self.paint_quad_styled(rect, 0.0, 0.0, paint);
    }

    fn draw_round_rect(&self, rect: gfx::Rect, rx: f32, ry: f32, paint: &gfx::Paint) {
        self.paint_quad_styled(rect, rx, ry, paint);
    }

    fn draw_circle(&self, center: gfx::Point, radius: f32, paint: &gfx::Paint) {
        let rect = gfx::Rect::new(
            center.x - radius,
            center.y - radius,
            center.x + radius,
            center.y + radius,
        );
        self.paint_quad_styled(rect, radius, radius, paint);
    }

    fn draw_line(&self, from: gfx::Point, to: gfx::Point, paint: &gfx::Paint) {
        let mut builder =
            gpui::PathBuilder::stroke(self.to_len(paint.stroke_width().max(1.0)));
        builder.move_to(self.to_window(from.x, from.y));
        builder.line_to(self.to_window(to.x, to.y));
        self.paint_built(builder, paint);
    }

    fn draw_path(&self, path: &gfx::Path, paint: &gfx::Paint) {
        let mut builder = self.path_builder(paint);
        let mut iter = skia_safe::path::Iter::new(path, false);
        while let Some((verb, points)) = iter.next() {
            match verb {
                skia_safe::path::Verb::Move => {
                    builder.move_to(self.to_window(points[0].x, points[0].y));
                }
                skia_safe::path::Verb::Line => {
                    builder.line_to(self.to_window(points[1].x, points[1].y));
                }
                skia_safe::path::Verb::Quad => {
                    builder.curve_to(
                        self.to_window(points[2].x, points[2].y),
                        self.to_window(points[1].x, points[1].y),
                    );
                }
                skia_safe::path::Verb::Cubic => {
                    builder.cubic_bezier_to(
                        self.to_window(points[3].x, points[3].y),
                        self.to_window(points[1].x, points[1].y),
                        self.to_window(points[2].x, points[2].y),
                    );
                }
                skia_safe::path::Verb::Close => builder.close(),
                _ => {}
            }
        }
        self.paint_built(builder, paint);
    }

    fn draw_text_blob(&self, blob: &gfx::TextBlob, origin: gfx::Point, paint: &gfx::Paint) {
        let glyph_color = color(paint.color());
        for run in blob.runs() {
            let font_id = self.font_id(run.face);
            let font_size = self.to_len(run.size);
            for (glyph, position) in &run.glyphs {
                let at = self.to_window(origin.x + position.x, origin.y + position.y);
                let clip = self.state.borrow().current.clip;
                let window = self.window();
                let paint_one = |window: &mut Window| {
                    let glyph = glyph_id(*glyph);
                    if window
                        .paint_glyph(at, font_id, glyph, font_size, glyph_color)
                        .is_err()
                    {
                        let _ = window.paint_emoji(at, font_id, glyph, font_size);
                    }
                };
                match clip {
                    Some(bounds) => window
                        .with_content_mask(Some(ContentMask { bounds }), paint_one),
                    None => paint_one(window),
                }
            }
        }
    }

    fn draw_str(&self, text: &str, origin: gfx::Point, font: &gfx::Font, paint: &gfx::Paint) {
        if text.is_empty() {
            return;
        }
        let typeface = font.typeface();
        let gpui_font = gpui::Font {
            family: SharedString::from(typeface.family_name()),
            features: Default::default(),
            fallbacks: None,
            weight: gpui::FontWeight(*typeface.font_style().weight() as f32),
            style: Default::default(),
        };
        let font_size = self.to_len(font.size());
        let run = TextRun {
            len: text.len(),
            font: gpui_font,
            color: color(paint.color()),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shared: SharedString = SharedString::from(text.to_string());
        let window = self.window();
        let shaped = window
            .text_system()
            .shape_line(shared, font_size, &[run], None);
        let _ = window;

        // `origin` is the BASELINE; gpui's line paint wants the top
        // of a line box. With the box exactly ascent+descent tall,
        // the baseline lands ascent below the top.
        let ascent = shaped.ascent;
        let descent = shaped.descent;
        let at = self.to_window(origin.x, origin.y);
        let top = point(at.x, at.y - ascent);
        let clip = self.state.borrow().current.clip;
        let window = self.window();
        let cx = self.app();
        match clip {
            Some(bounds) => {
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let _ = shaped.paint(top, ascent + descent, window, cx);
                });
            }
            None => {
                let _ = shaped.paint(top, ascent + descent, window, cx);
            }
        }
    }

    fn draw_image(&self, image: &gfx::Image, origin: gfx::Point, sampling: gfx::Sampling, paint: &gfx::Paint) {
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
        let Some(rendered) = self.render_image(image) else {
            return;
        };
        let bounds = self.to_bounds(dst);
        self.with_mask(move |window| {
            let _ = window.paint_image(bounds, Corners::default(), rendered, 0, false);
        });
    }

    fn draw_blurred_round_rect(&self, rect: gfx::Rect, rx: f32, _ry: f32, sigma: f32, c: gfx::Color) {
        let bounds = self.to_bounds(rect);
        let radius = self.to_len(rx);
        // A CSS blur radius is twice the gaussian's sigma.
        let shadow = BoxShadow {
            color: color(c),
            offset: point(px(0.0), px(0.0)),
            blur_radius: self.to_len(sigma * 2.0),
            spread_radius: px(0.0),
        };
        self.with_mask(move |window| {
            window.paint_shadows(bounds, Corners::all(radius), &[shadow]);
        });
    }
}
