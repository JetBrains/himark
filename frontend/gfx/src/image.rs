// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The raster the app actually draws: straight RGBA8 pixels with a
//! premultiplied flag — what resvg renders and what a decoder
//! yields. A backend uploads once into its own representation
//! through the cache slot and reuses it every frame, the same
//! bargain the text blobs strike.

use std::any::Any;
use std::sync::{Arc, OnceLock};

#[derive(Clone)]
pub struct Image {
    inner: Arc<ImageInner>,
}

struct ImageInner {
    width: u32,
    height: u32,
    premultiplied: bool,
    pixels: Vec<u8>,
    backend: OnceLock<Box<dyn Any + Send + Sync>>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Image")
            .field("width", &self.inner.width)
            .field("height", &self.inner.height)
            .finish()
    }
}

impl Image {
    /// RGBA8, row-major, `width * height * 4` bytes.
    pub fn from_rgba8(
        width: u32,
        height: u32,
        premultiplied: bool,
        pixels: Vec<u8>,
    ) -> Option<Image> {
        if width == 0 || height == 0 || pixels.len() != (width as usize) * (height as usize) * 4 {
            return None;
        }
        Some(Image {
            inner: Arc::new(ImageInner {
                width,
                height,
                premultiplied,
                pixels,
                backend: OnceLock::new(),
            }),
        })
    }

    pub fn width(&self) -> u32 {
        self.inner.width
    }

    pub fn height(&self) -> u32 {
        self.inner.height
    }

    pub fn is_premultiplied(&self) -> bool {
        self.inner.premultiplied
    }

    pub fn pixels(&self) -> &[u8] {
        &self.inner.pixels
    }

    /// The backend's uploaded twin, built on first use and shared
    /// for the image's life — identity-stable, so texture caches
    /// keyed by it hold. One backend per process; a second backend
    /// type asking would rebuild nothing and get `None`.
    pub fn backend<T: Send + Sync + 'static>(&self, build: impl FnOnce(&Image) -> T) -> Option<&T> {
        self.inner
            .backend
            .get_or_init(|| Box::new(build(self)))
            .downcast_ref::<T>()
    }
}
