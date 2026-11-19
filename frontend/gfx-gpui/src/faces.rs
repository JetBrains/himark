// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! gfx's faces and images as gpui's — what both painters hand the
//! text system and the sprite atlas.

use std::sync::Arc;

use gpui::{RenderImage, SharedString};
use smallvec::smallvec;

/// The bytes to register and the descriptor that then resolves them.
///
/// A face with variation coords gets its instance BAKED into the
/// bytes under a unique family (gpui's font-kit matching never
/// applies axes — see gfx-faces); a plain face rides its own bytes
/// and family name.
pub(crate) fn font(face: gfx::FaceId) -> (Vec<u8>, gpui::Font) {
    let entry = gfx::face(face);
    let (bytes, family, weight) = match gfx_faces::instanced(face.0, &entry) {
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
    if std::env::var_os("HIMARK_TRACE_FONTS").is_some() {
        eprintln!(
            "[fonts] face {} family={family:?} index={} coords={:?} weight={weight:?}",
            face.0, entry.index, entry.coords
        );
    }
    let font = gpui::Font {
        family: SharedString::from(family),
        features: Default::default(),
        fallbacks: None,
        weight,
        style: Default::default(),
    };
    (bytes, font)
}

pub(crate) fn color(color: gfx::Color) -> gpui::Hsla {
    gpui::Hsla::from(gpui::Rgba {
        r: color.r() as f32 / 255.0,
        g: color.g() as f32 / 255.0,
        b: color.b() as f32 / 255.0,
        a: color.a() as f32 / 255.0,
    })
}

/// The image's gpui twin, built once and kept on the image itself.
pub(crate) fn render_image(image: &gfx::Image) -> Option<Arc<RenderImage>> {
    image
        .backend(|image| {
            let width = image.width();
            let height = image.height();
            // gpui's RenderImage is BGRA behind an RGBA-typed
            // buffer, unpremultiplied — swizzle, and divide the
            // alpha back out when the raster arrived premultiplied.
            let mut pixels = image.pixels().to_vec();
            for px in pixels.chunks_exact_mut(4) {
                px.swap(0, 2);
                if image.is_premultiplied() && px[3] != 0 && px[3] != 255 {
                    let a = px[3] as u32;
                    px[0] = ((px[0] as u32 * 255) / a).min(255) as u8;
                    px[1] = ((px[1] as u32 * 255) / a).min(255) as u8;
                    px[2] = ((px[2] as u32 * 255) / a).min(255) as u8;
                }
            }
            image::RgbaImage::from_raw(width, height, pixels)
                .map(|buffer| Arc::new(RenderImage::new(smallvec![image::Frame::new(buffer)])))
        })
        .cloned()
        .flatten()
}
