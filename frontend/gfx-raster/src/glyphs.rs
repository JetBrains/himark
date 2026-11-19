// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Glyph coverage masks: a skrifa outline filled once by tiny-skia,
//! kept per (face bytes, index, coords, glyph, size, skew, bold).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::MetadataProvider as _;

/// A glyph's coverage, positioned relative to its baseline origin.
pub(crate) struct GlyphMask {
    /// Top-left of the mask from the origin, whole pixels.
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) coverage: Vec<u8>,
}

/// What names a face: the bytes' address, the collection index and
/// the variation coordinates.
pub(crate) struct Face<'a> {
    pub(crate) data: &'a Arc<dyn AsRef<[u8]> + Send + Sync>,
    pub(crate) index: u32,
    pub(crate) coords: &'a [(u32, f32)],
}

type Key = (usize, u32, Vec<(u32, u32)>, u16, u32, u32, bool);

static MASKS: Mutex<Option<HashMap<Key, Option<Arc<GlyphMask>>>>> = Mutex::new(None);

struct Pen {
    builder: tiny_skia::PathBuilder,
}

impl OutlinePen for Pen {
    // Font outlines are y-up; the surface is y-down.
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(x, -y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(x, -y);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.builder.quad_to(cx, -cy, x, -y);
    }
    fn curve_to(&mut self, ax: f32, ay: f32, bx: f32, by: f32, x: f32, y: f32) {
        self.builder.cubic_to(ax, -ay, bx, -by, x, -y);
    }
    fn close(&mut self) {
        self.builder.close();
    }
}

pub(crate) fn mask(
    face: &Face<'_>,
    glyph: u16,
    size: f32,
    skew_x: f32,
    embolden: bool,
) -> Option<Arc<GlyphMask>> {
    let key: Key = (
        Arc::as_ptr(face.data) as *const u8 as usize,
        face.index,
        face.coords
            .iter()
            .map(|(tag, value)| (*tag, value.to_bits()))
            .collect(),
        glyph,
        size.to_bits(),
        skew_x.to_bits(),
        embolden,
    );
    let mut masks = MASKS.lock().expect("glyph masks");
    let masks = masks.get_or_insert_with(HashMap::new);
    if let Some(hit) = masks.get(&key) {
        return hit.clone();
    }
    let built = build(face, glyph, size, skew_x, embolden).map(Arc::new);
    masks.insert(key, built.clone());
    built
}

fn build(face: &Face<'_>, glyph: u16, size: f32, skew_x: f32, embolden: bool) -> Option<GlyphMask> {
    let font = skrifa::FontRef::from_index((**face.data).as_ref(), face.index).ok()?;
    let location = font.axes().location(
        face.coords
            .iter()
            .map(|(tag, value)| (skrifa::Tag::from_be_bytes(tag.to_be_bytes()), *value)),
    );
    let outline = font
        .outline_glyphs()
        .get(skrifa::GlyphId::new(glyph as u32))?;
    let mut pen = Pen {
        builder: tiny_skia::PathBuilder::new(),
    };
    let settings = DrawSettings::unhinted(skrifa::instance::Size::new(size), &location);
    outline.draw(settings, &mut pen).ok()?;
    // An empty outline (a space) has no ink and no mask.
    let path = pen.builder.finish()?;
    // The faux italic shears x by the height above the baseline
    // (y is already flipped, so "above" is negative).
    let path = match skew_x != 0.0 {
        true => path.transform(tiny_skia::Transform::from_row(
            1.0, 0.0, skew_x, 1.0, 0.0, 0.0,
        ))?,
        false => path,
    };
    // Faux bold: the fill plus a stroke around it.
    let grow = match embolden {
        true => (size / 24.0).max(0.5),
        false => 0.0,
    };
    let bounds = path.bounds();
    let left = (bounds.left() - grow).floor() as i32 - 1;
    let top = (bounds.top() - grow).floor() as i32 - 1;
    let right = (bounds.right() + grow).ceil() as i32 + 1;
    let bottom = (bounds.bottom() + grow).ceil() as i32 + 1;
    let width = (right - left).max(1) as u32;
    let height = (bottom - top).max(1) as u32;
    let mut mask = tiny_skia::Mask::new(width, height)?;
    let shift = tiny_skia::Transform::from_translate(-left as f32, -top as f32);
    mask.fill_path(&path, tiny_skia::FillRule::Winding, true, shift);
    if embolden {
        let stroke = tiny_skia::Stroke {
            width: grow * 2.0,
            ..Default::default()
        };
        if let Some(outline) = stroked(&path, &stroke) {
            let mut around = tiny_skia::Mask::new(width, height)?;
            around.fill_path(&outline, tiny_skia::FillRule::Winding, true, shift);
            for (into, from) in mask.data_mut().iter_mut().zip(around.data()) {
                *into = (*into).max(*from);
            }
        }
    }
    Some(GlyphMask {
        left,
        top,
        width,
        height,
        coverage: mask.take(),
    })
}

fn stroked(path: &tiny_skia::Path, stroke: &tiny_skia::Stroke) -> Option<tiny_skia::Path> {
    path.stroke(stroke, 1.0)
}
