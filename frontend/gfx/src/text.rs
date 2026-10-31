// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The glyph road a backend can actually walk. A skia `SkTextBlob`
//! is opaque — positions go in, nothing comes back out — so a blob
//! here carries the runs themselves: face, size, synthesis, and
//! positioned glyph ids. The skia blob rides along, built once at
//! shape time the way it always was, so the skia backend hands the
//! atlas the SAME object every frame and its sub-run cache keys
//! hold.

use std::sync::{Arc, OnceLock, RwLock};

use crate::Point;

/// A face the process has seen: bytes, collection index, and the
/// variation coordinates parley instanced it at. Interned once;
/// every backend keys its own face cache (a skia `Typeface`, a gpui
/// `FontId`) by the id.
pub struct FaceEntry {
    pub data: Arc<dyn AsRef<[u8]> + Send + Sync>,
    pub index: u32,
    /// `(tag, value)` variation coordinates, the instance shaped.
    pub coords: Vec<(u32, f32)>,
    /// The face's preferred family name, when the registrar knew it
    /// — what a name-resolving backend (gpui) matches by.
    pub family: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FaceId(pub u32);

static FACES: OnceLock<RwLock<Vec<Arc<FaceEntry>>>> = OnceLock::new();

fn faces() -> &'static RwLock<Vec<Arc<FaceEntry>>> {
    FACES.get_or_init(|| RwLock::new(Vec::new()))
}

/// Intern a face. The caller is expected to memoize per (font,
/// coords) — this pushes unconditionally and answers the new id.
pub fn register_face(entry: FaceEntry) -> FaceId {
    let mut faces = faces().write().expect("face registry");
    faces.push(Arc::new(entry));
    FaceId(faces.len() as u32 - 1)
}

/// Look a face up — a backend resolving its own handle for the id.
pub fn face(id: FaceId) -> Arc<FaceEntry> {
    faces().read().expect("face registry")[id.0 as usize].clone()
}

/// One run of positioned glyphs in one face at one size.
pub struct GlyphRun {
    pub face: FaceId,
    pub size: f32,
    /// Faux bold — the face had no true bold cut.
    pub embolden: bool,
    /// Faux italic: a horizontal shear on each glyph.
    pub skew_x: f32,
    pub glyphs: Vec<(u16, Point)>,
}

/// Positioned glyphs, shaped upstream. Cheap to clone — lines cache
/// these and paint hands them out by reference every frame.
#[derive(Clone)]
pub struct TextBlob {
    inner: Arc<TextBlobInner>,
}

struct TextBlobInner {
    runs: Vec<GlyphRun>,
    /// The skia twin, built at shape time next to the runs. `None`
    /// only when every run's face failed to bridge.
    skia: Option<skia_safe::TextBlob>,
}

impl TextBlob {
    pub fn new(runs: Vec<GlyphRun>, skia: Option<skia_safe::TextBlob>) -> Self {
        Self {
            inner: Arc::new(TextBlobInner { runs, skia }),
        }
    }

    pub fn runs(&self) -> &[GlyphRun] {
        &self.inner.runs
    }

    pub fn skia(&self) -> Option<&skia_safe::TextBlob> {
        self.inner.skia.as_ref()
    }
}
