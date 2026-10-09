// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The text-band raster cache: re-issuing every visible line's glyph
//! ops is the paint profile's biggest block (ganesh rebuilds atlas
//! text ops per line, per frame), so the TEXT LAYER — washes +
//! glyphs, exactly `ShapedLine::paint_in_slot`'s output — is baked
//! into content-anchored bands and composited as images. Everything
//! under it (whole-line backgrounds, spacers, selections, rules) and
//! over it (carets, inlay widgets, sticky rows) stays live.
//!
//! Validity is IDENTITY: a band remembers the `Rc<ShapedLine>`s it
//! rasterized (held strongly, so a pointer cannot be reused while
//! the entry lives) and their slot geometry; the shape cache mints a
//! NEW line for every visual change (edit, markup, hover, selection
//! flag, theme), so a band is current exactly while its lines'
//! identities hold.
//!
//! Sub-pixel phase: while the canvas translation sits on a moving
//! fraction (mid-scroll), bands composite through bilinear sampling;
//! once the phase repeats across paints, the visible bands rebake at
//! the exact phase — crisp at rest, filtered only in motion.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use skia_safe::{Canvas, ImageInfo, Rect, SamplingOptions};

use crate::shaped_line::ShapedLine;
use crate::viewport::ViewportLine;

/// Content-space band height; bands tile the document vertically.
const BAND_HEIGHT: f32 = 256.0;

/// Glyphs may overhang their layout slot (tall scripts, shrunk line
/// heights): a band also renders lines whose slot ends within this
/// margin of it, so the overhanging pixels exist in the band that
/// owns them. Band surfaces never overlap, so nothing double-blends.
const OVERHANG: f32 = 32.0;

/// The horizontal window is the clip quantum: a band bakes the
/// visible x-range rounded out to this, so small horizontal shifts
/// stay inside one baked window.
const X_QUANTUM: f32 = 256.0;

/// The budget across every editor: a few retina screens of text.
const MAX_BYTES: usize = 96 << 20;

#[derive(PartialEq, Eq, Hash, Clone, Copy)]
struct Key {
    token: crate::document::DocumentToken,
    editor: crate::editor::EditorId,
    band: i64,
}

struct Entry {
    image: skia_safe::Image,
    fingerprint: u64,
    /// The device-space sub-pixel phase the band was baked at.
    phase: (f32, f32),
    bytes: usize,
    /// Strong refs to the rasterized lines: identity can't ABA.
    _held: Vec<Rc<ShapedLine>>,
    last_use: u64,
}

/// One editor view's phase watch: rebake only once the phase holds
/// still across paints, never per mid-scroll frame.
struct PhaseWatch {
    phase: (f32, f32),
    still: bool,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<Key, Entry>,
    phases: HashMap<(crate::document::DocumentToken, crate::editor::EditorId), PhaseWatch>,
    clock: u64,
    bytes: usize,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

pub(crate) struct BandPass<'a> {
    pub token: crate::document::DocumentToken,
    pub editor: crate::editor::EditorId,
    pub lines: &'a [&'a ViewportLine],
}

fn phases_close(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
}

/// Composite the text layer of every shaped line through the band
/// cache. Returns false when banding cannot serve this canvas (a
/// transformed matrix, no clip, the kill flag) — the caller paints
/// the lines directly.
pub(crate) fn composite(pass: &BandPass<'_>, canvas: &Canvas) -> bool {
    if crate::env_flags::no_bands() {
        return false;
    }
    let matrix = canvas.local_to_device_as_3x3();
    if !matrix.is_scale_translate() {
        return false;
    }
    let (scale_x, scale_y) = (matrix.scale_x(), matrix.scale_y());
    if scale_x <= 0.0 || (scale_x - scale_y).abs() > 1e-3 {
        return false;
    }
    let scale = scale_x;
    let Some(clip) = canvas.local_clip_bounds() else {
        return false;
    };
    let phase = (
        matrix.translate_x().rem_euclid(1.0),
        matrix.translate_y().rem_euclid(1.0),
    );

    // The baked x-window: the visible range rounded out to the
    // quantum, so horizontal jitter does not re-rasterize.
    let x0 = (clip.left / X_QUANTUM).floor() * X_QUANTUM;
    let x1 = (clip.right / X_QUANTUM).ceil() * X_QUANTUM;
    if !(x1 > x0) || !(x1 - x0).is_finite() {
        return false;
    }

    // The phase is REST when it repeats the previous paint's: only
    // then do stale-phase bands rebake, never per mid-scroll frame.
    let at_rest = CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        let watch = cache
            .phases
            .entry((pass.token, pass.editor))
            .or_insert(PhaseWatch { phase, still: false });
        watch.still = phases_close(watch.phase, phase);
        watch.phase = phase;
        watch.still
    });

    let first_band = (clip.top / BAND_HEIGHT).floor() as i64;
    let last_band = (clip.bottom / BAND_HEIGHT).ceil() as i64;
    for band in first_band..last_band {
        let band_top = band as f32 * BAND_HEIGHT;
        let band_bottom = band_top + BAND_HEIGHT;

        // Pixel-exactness needs the BAND ORIGIN's device fraction
        // baked too, not just the canvas translate's: at non-integer
        // scales `band_top * scale` lands off-grid by a per-band
        // amount.
        let band_phase = (
            (x0 * scale + matrix.translate_x()).rem_euclid(1.0),
            (band_top * scale + matrix.translate_y()).rem_euclid(1.0),
        );

        // The lines this band owns pixels of (slot overlap, padded
        // by the overhang so tall glyphs exist in the band they
        // spill into).
        let lines: Vec<&ViewportLine> = pass
            .lines
            .iter()
            .copied()
            .filter(|line| line.shaped.is_some())
            .filter(|line| {
                line.top < band_bottom + OVERHANG && line.top + line.height > band_top - OVERHANG
            })
            .collect();
        if lines.is_empty() {
            continue;
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        scale.to_bits().hash(&mut hasher);
        x0.to_bits().hash(&mut hasher);
        x1.to_bits().hash(&mut hasher);
        for line in &lines {
            let shaped = line.shaped.as_ref().expect("filtered on shaped");
            (Rc::as_ptr(shaped) as usize).hash(&mut hasher);
            line.top.to_bits().hash(&mut hasher);
            line.text_top.to_bits().hash(&mut hasher);
            line.height.to_bits().hash(&mut hasher);
            line.x.to_bits().hash(&mut hasher);
        }
        let fingerprint = hasher.finish();

        let key = Key {
            token: pass.token,
            editor: pass.editor,
            band,
        };
        let cached = CACHE.with(|cell| {
            let mut cache = cell.borrow_mut();
            let clock = cache.clock;
            cache.entries.get_mut(&key).and_then(|entry| {
                let phase_ok = phases_close(entry.phase, band_phase) || !at_rest;
                (entry.fingerprint == fingerprint && phase_ok).then(|| {
                    entry.last_use = clock;
                    entry.image.clone()
                })
            })
        });

        let image = match cached {
            Some(image) => image,
            None => {
                let Some(image) = bake(canvas, &lines, band_top, x0, x1, scale, band_phase) else {
                    return false;
                };
                let bytes = image.width() as usize * image.height() as usize * 4;
                let held: Vec<Rc<ShapedLine>> = lines
                    .iter()
                    .map(|line| Rc::clone(line.shaped.as_ref().expect("filtered on shaped")))
                    .collect();
                CACHE.with(|cell| {
                    let mut cache = cell.borrow_mut();
                    let clock = cache.clock;
                    if let Some(old) = cache.entries.insert(
                        key,
                        Entry {
                            image: image.clone(),
                            fingerprint,
                            phase: band_phase,
                            bytes,
                            _held: held,
                            last_use: clock,
                        },
                    ) {
                        cache.bytes -= old.bytes;
                    }
                    cache.bytes += bytes;
                    while cache.bytes > MAX_BYTES {
                        let Some((&victim, _)) = cache
                            .entries
                            .iter()
                            .min_by_key(|(_, entry)| entry.last_use)
                        else {
                            break;
                        };
                        if victim == key {
                            break;
                        }
                        if let Some(evicted) = cache.entries.remove(&victim) {
                            cache.bytes -= evicted.bytes;
                        }
                    }
                });
                image
            }
        };

        // The destination covers the baked window; with the phases
        // equal the image lands on its own pixel grid and the linear
        // sampling is the identity. The hard (non-AA) clip assigns
        // every device pixel to exactly ONE band — the pad row and
        // the overhang pixels a neighbor also rendered never blend
        // twice.
        let dst = Rect::from_xywh(
            x0,
            band_top,
            image.width() as f32 / scale,
            image.height() as f32 / scale,
        );
        canvas.save();
        canvas.clip_rect(
            Rect::new(x0, band_top, x1, band_bottom),
            skia_safe::ClipOp::Intersect,
            false,
        );
        canvas.draw_image_rect_with_sampling_options(
            &image,
            None,
            dst,
            SamplingOptions::from(skia_safe::FilterMode::Linear),
            &skia_safe::Paint::default(),
        );
        canvas.restore();
    }
    CACHE.with(|cell| cell.borrow_mut().clock += 1);
    true
}

/// Rasterize one band: a compatible surface (GPU on the metal
/// canvas, raster under the tests), the content transform with the
/// screen's sub-pixel phase baked in, and the exact same
/// `paint_in_slot` calls the direct path would make.
fn bake(
    canvas: &Canvas,
    lines: &[&ViewportLine],
    band_top: f32,
    x0: f32,
    x1: f32,
    scale: f32,
    phase: (f32, f32),
) -> Option<skia_safe::Image> {
    let width = ((x1 - x0) * scale).ceil() as i32 + 1;
    let height = (BAND_HEIGHT * scale).ceil() as i32 + 1;
    if width <= 0 || height <= 0 || width > 16384 || height > 16384 {
        return None;
    }
    let info = ImageInfo::new_n32_premul((width, height), canvas.image_info().color_space());
    let mut surface = canvas.new_surface(&info, None)?;
    let band_canvas = surface.canvas();
    band_canvas.clear(skia_safe::Color::TRANSPARENT);
    band_canvas.translate((phase.0, phase.1));
    band_canvas.scale((scale, scale));
    band_canvas.translate((-x0, -band_top));
    for line in lines {
        let shaped = line.shaped.as_ref().expect("filtered on shaped");
        shaped.paint_in_slot(
            band_canvas,
            line.top,
            line.text_top,
            line.top + line.height,
        );
    }
    Some(surface.image_snapshot())
}

/// TEST SUPPORT: drop every band so a test can compare banded and
/// direct output from a clean slate.
#[doc(hidden)]
pub fn purge_for_tests() {
    CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        cache.entries.clear();
        cache.phases.clear();
        cache.bytes = 0;
    });
}
