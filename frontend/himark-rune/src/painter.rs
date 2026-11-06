// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The RECORDER: himark paints through `gfx::Painter` and this
//! backend answers with rune `Instance`s — rects, segments and
//! atlas glyph quads in device pixels, every one clipped to the
//! effect slot. The compositor splices the recording into the
//! frame's painter stream at the slot's position.

use std::cell::RefCell;
use std::collections::HashMap;

use rune::render::{Instance, KIND_GLYPH, KIND_RECT, KIND_SEGMENT};
use rune::text::TextSystem;

use gfx::{Color, Paint, PaintStyle, Point, Rect, Size};

#[derive(Clone, Copy)]
struct State {
    /// Local → device: scale then translate (rotate/skew unused by
    /// the surveyed inventory; logged once if asked).
    sx: f32,
    sy: f32,
    tx: f32,
    ty: f32,
    /// Device-pixel clip, already intersected with the slot's.
    clip: [f32; 4],
}

pub(crate) struct Recorder<'a> {
    text: RefCell<&'a mut TextSystem>,
    out: RefCell<Vec<Instance>>,
    stack: RefCell<Vec<State>>,
    faces: RefCell<HashMap<u32, parley::FontData>>,
    /// Encode premultiplied colours for sRGB blending when the
    /// frame does (`Frame::srgb_blend`).
    srgb: bool,
    /// The slot's device-pixel rect — the local origin and the
    /// base-layer size himark asks about.
    slot: [f32; 4],
    device_scale: f32,
}

impl<'a> Recorder<'a> {
    pub(crate) fn new(
        text: &'a mut TextSystem,
        slot: [f32; 4],
        clip: [f32; 4],
        srgb: bool,
        device_scale: f32,
    ) -> Self {
        Self {
            text: RefCell::new(text),
            out: RefCell::new(Vec::new()),
            stack: RefCell::new(vec![State {
                sx: 1.0,
                sy: 1.0,
                tx: slot[0],
                ty: slot[1],
                clip: [
                    clip[0].max(slot[0]),
                    clip[1].max(slot[1]),
                    clip[2].min(slot[0] + slot[2]),
                    clip[3].min(slot[1] + slot[3]),
                ],
            }]),
            faces: RefCell::new(HashMap::new()),
            srgb,
            slot,
            device_scale,
        }
    }

    pub(crate) fn finish(self) -> Vec<Instance> {
        self.out.into_inner()
    }

    fn state(&self) -> State {
        *self.stack.borrow().last().expect("the recorder's base state")
    }

    fn point(&self, p: Point) -> [f32; 2] {
        let s = self.state();
        [p.x * s.sx + s.tx, p.y * s.sy + s.ty]
    }

    fn rect(&self, r: Rect) -> [f32; 4] {
        let s = self.state();
        [
            r.left * s.sx + s.tx,
            r.top * s.sy + s.ty,
            (r.right - r.left) * s.sx,
            (r.bottom - r.top) * s.sy,
        ]
    }

    fn color(&self, color: Color, opacity: f32) -> [f32; 4] {
        let argb = color.0;
        let a = ((argb >> 24) & 0xFF) as f32 / 255.0 * opacity;
        let linearish = |v: u32| (v & 0xFF) as f32 / 255.0;
        let (r, g, b) = (linearish(argb >> 16), linearish(argb >> 8), linearish(argb));
        // Premultiplied; rune blends sRGB-encoded premultiplied
        // values when the frame says so — the channels above are
        // already sRGB-encoded bytes, so only the LINEAR frame
        // decodes them.
        if self.srgb {
            [r * a, g * a, b * a, a]
        } else {
            let lin = |v: f32| {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            [lin(r) * a, lin(g) * a, lin(b) * a, a]
        }
    }

    fn push(&self, instance: Instance) {
        self.out.borrow_mut().push(instance);
    }

    fn rect_instance(&self, rect: [f32; 4], radii: [f32; 4], paint: &Paint, sigma: f32) {
        let s = self.state();
        let fill = self.color(paint.color(), 1.0);
        let (fill, border, width) = match paint.style() {
            PaintStyle::Fill => (fill, [0.0; 4], 0.0),
            PaintStyle::Stroke => ([0.0; 4], fill, paint.stroke_width() * s.sx),
            PaintStyle::StrokeAndFill => (fill, fill, paint.stroke_width() * s.sx),
        };
        self.push(Instance {
            rect,
            radii,
            fill,
            border,
            params: [width, KIND_RECT, sigma, 1.0],
            uv: [0.0; 4],
            clip: s.clip,
            fade: [0; 4],
        });
    }

    fn font_data(&self, face: gfx::FaceId) -> parley::FontData {
        self.faces
            .borrow_mut()
            .entry(face.0)
            .or_insert_with(|| {
                let entry = gfx::face(face);
                parley::FontData::new(parley::fontique::Blob::new(entry.data.clone()), entry.index)
            })
            .clone()
    }

    fn glyph(&self, font: &parley::FontData, glyph: u16, px: f32, at: [f32; 2], fill: [f32; 4]) {
        let s = self.state();
        let quad = self.text.borrow_mut().glyph_quad(font, &[], glyph, px, at);
        if let Some(q) = quad {
            let page = q.atlas_page() as f32;
            let uv = q.atlas_uv();
            self.push(Instance {
                rect: q.rect,
                radii: [0.0; 4],
                fill,
                border: [0.0; 4],
                params: [0.0, KIND_GLYPH, page, 1.0],
                uv: [uv[0], uv[1], 0.0, 0.0],
                clip: s.clip,
                fade: [0; 4],
            });
        }
    }
}

fn warn_once(what: &'static str) {
    use std::sync::Mutex;
    use std::collections::HashSet;
    static SEEN: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut seen = SEEN.lock().expect("warn set");
    if seen.get_or_insert_with(HashSet::new).insert(what) {
        eprintln!("[himark-rune] {what} is not recorded yet");
    }
}

impl gfx::Painter for Recorder<'_> {
    fn save(&self) {
        let top = self.state();
        self.stack.borrow_mut().push(top);
    }

    fn restore(&self) {
        let mut stack = self.stack.borrow_mut();
        if stack.len() > 1 {
            stack.pop();
        }
    }

    fn translate(&self, dx: f32, dy: f32) {
        let mut stack = self.stack.borrow_mut();
        let top = stack.last_mut().expect("state");
        top.tx += dx * top.sx;
        top.ty += dy * top.sy;
    }

    fn scale(&self, sx: f32, sy: f32) {
        let mut stack = self.stack.borrow_mut();
        let top = stack.last_mut().expect("state");
        top.sx *= sx;
        top.sy *= sy;
    }

    fn skew(&self, _sx: f32, _sy: f32) {
        warn_once("skew");
    }

    fn rotate(&self, _degrees: f32, _pivot: Option<Point>) {
        warn_once("rotate");
    }

    fn clip_rect(&self, rect: Rect, _anti_alias: bool) {
        let device = self.rect(rect);
        let mut stack = self.stack.borrow_mut();
        let top = stack.last_mut().expect("state");
        top.clip = [
            top.clip[0].max(device[0]),
            top.clip[1].max(device[1]),
            top.clip[2].min(device[0] + device[2]),
            top.clip[3].min(device[1] + device[3]),
        ];
    }

    fn local_clip_bounds(&self) -> Option<Rect> {
        let s = self.state();
        if s.clip[2] <= s.clip[0] || s.clip[3] <= s.clip[1] {
            return None;
        }
        Some(Rect::new(
            (s.clip[0] - s.tx) / s.sx,
            (s.clip[1] - s.ty) / s.sy,
            (s.clip[2] - s.tx) / s.sx,
            (s.clip[3] - s.ty) / s.sy,
        ))
    }

    fn base_layer_size(&self) -> Size {
        Size::new(self.slot[2], self.slot[3])
    }

    fn device_scale(&self) -> f32 {
        let s = self.state();
        (s.sx.abs().max(s.sy.abs())) * self.device_scale
    }

    fn clear(&self, color: Color) {
        // The slot's floor — the host behind a mounted view owns
        // the backdrop; a clear paints the slot rect.
        if color != Color::TRANSPARENT {
            let s = self.state();
            self.push(Instance {
                rect: self.slot,
                radii: [0.0; 4],
                fill: self.color(color, 1.0),
                border: [0.0; 4],
                params: [0.0, KIND_RECT, 0.0, 1.0],
                uv: [0.0; 4],
                clip: s.clip,
                fade: [0; 4],
            });
        }
    }

    fn draw_rect(&self, rect: Rect, paint: &Paint) {
        self.rect_instance(self.rect(rect), [0.0; 4], paint, 0.0);
    }

    fn draw_round_rect(&self, rect: Rect, rx: f32, _ry: f32, paint: &Paint) {
        let s = self.state();
        let r = rx * s.sx;
        self.rect_instance(self.rect(rect), [r; 4], paint, 0.0);
    }

    fn draw_circle(&self, center: Point, radius: f32, paint: &Paint) {
        let s = self.state();
        let c = self.point(center);
        let r = radius * s.sx;
        self.rect_instance([c[0] - r, c[1] - r, 2.0 * r, 2.0 * r], [r; 4], paint, 0.0);
    }

    fn draw_line(&self, from: Point, to: Point, paint: &Paint) {
        let s = self.state();
        let a = self.point(from);
        let b = self.point(to);
        let half = (paint.stroke_width().max(1.0) * s.sx) / 2.0;
        let rect = [
            a[0].min(b[0]) - half,
            a[1].min(b[1]) - half,
            (a[0] - b[0]).abs() + 2.0 * half,
            (a[1] - b[1]).abs() + 2.0 * half,
        ];
        self.push(Instance {
            rect,
            radii: [a[0], a[1], b[0], b[1]],
            fill: self.color(paint.color(), 1.0),
            border: [0.0; 4],
            params: [half, KIND_SEGMENT, 0.0, 1.0],
            uv: [0.0; 4],
            clip: s.clip,
            fade: [0; 4],
        });
    }

    fn draw_path(&self, _path: &gfx::Path, _paint: &Paint) {
        warn_once("draw_path");
    }

    fn draw_text_blob(&self, blob: &gfx::TextBlob, origin: Point, paint: &Paint) {
        let s = self.state();
        let fill = self.color(paint.color(), 1.0);
        let base = self.point(origin);
        for run in blob.runs() {
            let entry = gfx::face(run.face);
            if !entry.coords.is_empty() {
                warn_once("variable-font coords");
            }
            let font = self.font_data(run.face);
            let px = run.size * s.sy;
            for (glyph, position) in run.glyphs.iter() {
                let at = [
                    base[0] + position.x * s.sx,
                    base[1] + position.y * s.sy,
                ];
                self.glyph(&font, *glyph, px, at, fill);
            }
        }
    }

    fn draw_str(&self, text: &str, origin: Point, font: &gfx::Font, paint: &Paint) {
        let s = self.state();
        let fill = self.color(paint.color(), 1.0);
        let typeface = font.typeface();
        let data = typeface.data();
        let Some(font_ref) =
            swash::FontRef::from_index((*data).as_ref(), typeface.index() as usize)
        else {
            return;
        };
        let charmap = font_ref.charmap();
        let glyph_metrics = font_ref.glyph_metrics(&[]).scale(font.size() * s.sy);
        let face_id = gfx::register_face(gfx::FaceEntry {
            data: data.clone(),
            index: typeface.index(),
            coords: Vec::new(),
            family: None,
        });
        let parley_font = self.font_data(face_id);
        let px = font.size() * s.sy;
        let mut pen = self.point(origin);
        for ch in text.chars() {
            let glyph = charmap.map(ch);
            self.glyph(&parley_font, glyph, px, pen, fill);
            pen[0] += glyph_metrics.advance_width(glyph);
        }
    }

    fn draw_image(
        &self,
        _image: &gfx::Image,
        _origin: Point,
        _sampling: gfx::Sampling,
        _paint: &Paint,
    ) {
        warn_once("draw_image");
    }

    fn draw_image_rect(
        &self,
        _image: &gfx::Image,
        _dst: Rect,
        _sampling: gfx::Sampling,
        _paint: &Paint,
    ) {
        warn_once("draw_image_rect");
    }

    fn draw_blurred_round_rect(&self, rect: Rect, rx: f32, _ry: f32, sigma: f32, color: Color) {
        let s = self.state();
        let mut paint = Paint::default();
        paint.set_color(color);
        self.rect_instance(self.rect(rect), [rx * s.sx; 4], &paint, sigma * s.sx);
    }
}
