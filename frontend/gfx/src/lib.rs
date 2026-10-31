// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The painting boundary: every crate above draws through
//! [`Painter`], and only this crate (and the shells that create
//! surfaces) names skia. The surface is deliberately the app's own
//! inventory — ten draw calls, solid paints, rect clips — not a
//! canvas in general; a backend owes nothing more than what the
//! trait spells out.
//!
//! The types below are re-exported from skia for now. Each
//! re-export is a debt this crate owes, not a contract: replacing
//! one with our own type changes this file and nobody above.

mod painter;
mod text;

pub use painter::{Painter, Sampling};
pub use text::{face, register_face, FaceEntry, FaceId, GlyphRun, TextBlob};

/// THE canvas the widget tree paints into — any backend, behind the
/// trait. A type alias so `&Canvas` keeps meaning "somewhere to
/// draw" at every call site it always did.
pub type Canvas = dyn Painter;

// ─── geometry and color ───
pub use skia_safe::{Color, Contains, ISize, Point, Rect, Size};

// ─── paint ───
pub use skia_safe::{paint, BlurStyle, MaskFilter, Paint, PaintCap, PaintJoin, PaintStyle};

// ─── paths ───
pub use skia_safe::{Path, PathBuilder};

// ─── text ───
pub use skia_safe::{
    font, font_arguments, font_style, typeface, Font, FontArguments, FontMetrics, FontMgr,
    FontStyle, FourByteTag, TextBlobBuilder, Typeface,
};

// ─── images ───
pub use skia_safe::{
    image, images, AlphaType, ColorType, Data, FilterMode, Image, ImageInfo, SamplingOptions,
};

// ─── offscreen rasters: the test oracle and the property renders ───
pub use skia_safe::{surfaces, EncodedImageFormat, Surface};
