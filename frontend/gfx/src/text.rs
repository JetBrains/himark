// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The glyph road a backend can actually walk: a blob carries the
//! runs themselves — face, size, synthesis, positioned glyph ids —
//! and nothing of any renderer. A backend builds its native twin
//! (a skia `SkTextBlob`, a gpui run list) through the cache slot on
//! first draw and reuses it for the blob's life, so identity-keyed
//! atlas caches hold exactly as they did when the twin was built at
//! shape time.

use std::any::Any;
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

type FaceKey = (usize, usize, u32, Vec<(u32, u32)>);

struct Registry {
    faces: Vec<Arc<FaceEntry>>,
    /// Dedupe by identity — bytes address, collection index, coords:
    /// every thread interning the face the shaper met answers the
    /// SAME id, so backend caches stay one-per-face process-wide.
    ids: std::collections::HashMap<FaceKey, FaceId>,
}

static FACES: OnceLock<RwLock<Registry>> = OnceLock::new();

fn faces() -> &'static RwLock<Registry> {
    FACES.get_or_init(|| {
        RwLock::new(Registry {
            faces: Vec::new(),
            ids: std::collections::HashMap::new(),
        })
    })
}

fn key_of(entry: &FaceEntry) -> FaceKey {
    let bytes = (*entry.data).as_ref();
    (
        bytes.as_ptr() as usize,
        bytes.len(),
        entry.index,
        entry
            .coords
            .iter()
            .map(|(tag, value)| (*tag, value.to_bits()))
            .collect(),
    )
}

/// Intern a face — idempotent per (bytes, index, coords).
pub fn register_face(entry: FaceEntry) -> FaceId {
    let key = key_of(&entry);
    let mut registry = faces().write().expect("face registry");
    if let Some(id) = registry.ids.get(&key) {
        return *id;
    }
    registry.faces.push(Arc::new(entry));
    let id = FaceId(registry.faces.len() as u32 - 1);
    registry.ids.insert(key, id);
    id
}

/// How many faces the process has interned. Test support.
pub fn face_count() -> usize {
    faces().read().expect("face registry").faces.len()
}

/// Look a face up — a backend resolving its own handle for the id.
pub fn face(id: FaceId) -> Arc<FaceEntry> {
    faces().read().expect("face registry").faces[id.0 as usize].clone()
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
    backend: OnceLock<Box<dyn Any + Send + Sync>>,
}

impl TextBlob {
    pub fn new(runs: Vec<GlyphRun>) -> Self {
        Self {
            inner: Arc::new(TextBlobInner {
                runs,
                backend: OnceLock::new(),
            }),
        }
    }

    pub fn runs(&self) -> &[GlyphRun] {
        &self.inner.runs
    }

    /// The backend's twin, built once per blob — identity-stable
    /// for the blob's life. One backend per process.
    pub fn backend<T: Send + Sync + 'static>(
        &self,
        build: impl FnOnce(&[GlyphRun]) -> T,
    ) -> Option<&T> {
        self.inner
            .backend
            .get_or_init(|| Box::new(build(&self.inner.runs)))
            .downcast_ref::<T>()
    }
}
