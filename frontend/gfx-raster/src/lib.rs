// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! `gfx::Painter` on the CPU — the tests' paper. A [`Surface`] is a
//! premultiplied RGBA pixmap; [`Surface::canvas`] paints into it and
//! the oracles ([`Surface::rgba`], [`Surface::color_at`],
//! [`Surface::encode_png`]) read it back.
//!
//! Fills, strokes and images go through tiny-skia; glyphs are
//! skrifa outlines rasterized once per (face, glyph, size) into a
//! coverage mask and blended at whole-pixel positions. No hinting,
//! no subpixel phase, no color glyphs: the same pixels on every
//! machine, which is the point.

mod canvas;
mod glyphs;

pub use canvas::RasterCanvas;

/// A premultiplied RGBA surface.
pub struct Surface {
    pixmap: tiny_skia::Pixmap,
    /// The canvas's transform and clip live with the surface: a
    /// `translate` through one `canvas()` holds for the paint through
    /// the next, as one long-lived canvas would have it.
    state: canvas::State,
}

/// A surface of `width` × `height` pixels, transparent.
pub fn raster(width: i32, height: i32) -> Option<Surface> {
    Surface::new(width, height)
}

/// [`raster`] with a tuple size — the tests' one-line door.
pub fn raster_tuple(size: (i32, i32)) -> Option<Surface> {
    Surface::new(size.0, size.1)
}

impl Surface {
    pub fn new(width: i32, height: i32) -> Option<Self> {
        let pixmap = tiny_skia::Pixmap::new(width.try_into().ok()?, height.try_into().ok()?)?;
        Some(Self {
            pixmap,
            state: canvas::State::default(),
        })
    }

    pub fn width(&self) -> i32 {
        self.pixmap.width() as i32
    }

    pub fn height(&self) -> i32 {
        self.pixmap.height() as i32
    }

    /// The painter over this surface. Coerces to `&gfx::Canvas<'_>`.
    pub fn canvas(&mut self) -> RasterCanvas<'_> {
        RasterCanvas::new(&mut self.pixmap, &self.state)
    }

    /// The pixels as straight RGBA8 rows — `(width, height, bytes)`.
    /// Unpremultiplied, so a solid red reads `[255, 0, 0, 255]`.
    pub fn rgba(&self) -> (i32, i32, Vec<u8>) {
        let mut bytes = Vec::with_capacity(self.pixmap.data().len());
        for pixel in self.pixmap.pixels() {
            let color = pixel.demultiply();
            bytes.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
        }
        (self.width(), self.height(), bytes)
    }

    /// One pixel, unpremultiplied; transparent outside the surface.
    pub fn color_at(&self, x: i32, y: i32) -> gfx::Color {
        color_of(&self.pixmap, x, y)
    }

    /// The surface as PNG bytes.
    pub fn encode_png(&self) -> Vec<u8> {
        self.pixmap.encode_png().unwrap_or_default()
    }

    /// The pixels as they stand now — a copy a test keeps while it
    /// paints on.
    pub fn snapshot(&self) -> Snapshot {
        let mut bgra = self.pixmap.data().to_vec();
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        Snapshot {
            pixmap: self.pixmap.clone(),
            bgra,
        }
    }
}

/// A surface's pixels at one moment.
pub struct Snapshot {
    pixmap: tiny_skia::Pixmap,
    bgra: Vec<u8>,
}

impl Snapshot {
    pub fn width(&self) -> i32 {
        self.pixmap.width() as i32
    }

    pub fn height(&self) -> i32 {
        self.pixmap.height() as i32
    }

    /// One pixel, unpremultiplied; transparent outside the snapshot.
    pub fn get_color(&self, (x, y): (i32, i32)) -> gfx::Color {
        color_of(&self.pixmap, x, y)
    }

    /// The raw pixels, four bytes each in B, G, R, A order,
    /// premultiplied — the layout the byte-indexing tests were
    /// written against.
    pub fn bytes(&self) -> &[u8] {
        &self.bgra
    }

    /// The snapshot as PNG bytes.
    pub fn encode_png(&self) -> Option<Vec<u8>> {
        self.pixmap.encode_png().ok()
    }
}

fn color_of(pixmap: &tiny_skia::Pixmap, x: i32, y: i32) -> gfx::Color {
    let pixel = (x >= 0 && y >= 0)
        .then(|| pixmap.pixel(x as u32, y as u32))
        .flatten();
    match pixel {
        Some(pixel) => {
            let color = pixel.demultiply();
            gfx::Color::from_argb(color.alpha(), color.red(), color.green(), color.blue())
        }
        None => gfx::Color::TRANSPARENT,
    }
}
