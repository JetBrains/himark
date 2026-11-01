// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The painting boundary: every crate above draws through
//! [`Painter`], in this crate's own vocabulary — geometry, paint,
//! paths, faces, rasters. No renderer's name appears here: skia is
//! one backend (`gfx-skia`), gpui another (`imba-gpui`), and each
//! resolves faces, blobs and images from the neutral types through
//! identity-keyed caches of its own.
//!
//! The surface is deliberately the app's own inventory — ten draw
//! calls, solid paints, rect clips — not a canvas in general; a
//! backend owes nothing more than what the trait spells out.

mod geometry;
mod painter;
mod text;

pub mod font;
pub mod image;
pub mod paint;
pub mod path;

pub use font::{Edging, Font, FontStyle, Typeface};
pub use geometry::{Color, ISize, Point, Rect, Size};
pub use image::Image;
pub use paint::{Cap as PaintCap, Join as PaintJoin, Paint, Style as PaintStyle};
pub use painter::{Painter, Sampling};
pub use path::{Path, PathBuilder};
pub use text::{face, face_count, register_face, FaceEntry, FaceId, GlyphRun, TextBlob};

/// THE canvas the widget tree paints into — any backend, behind the
/// trait. The lifetime is the point: a bare `dyn Painter` alias
/// would default the object lifetime to `'static`, and a painter
/// that BORROWS its window (gpui's) could never coerce — the raw
/// pointers that workaround demanded are exactly what this spares.
pub type Canvas<'a> = dyn Painter + 'a;
