// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The gpui backend of the painting boundary — rendering and
//! nothing more, `gfx-skia`'s twin. One side speaks `gfx::Painter`,
//! the other gpui scene primitives: quads, lyon-stroked paths, rect
//! clips as content masks, glyph runs through the atlas, images,
//! analytic shadows. No imba, no events, no shell — input bridging
//! lives in `imba-gpui`, and what hangs a workbench off a window is
//! the shell's business.
//!
//! Variable faces are pinned to static instances before they meet
//! gpui's font store (see `instance.rs` for why).

mod painter;

pub use painter::{GpuiPainter, PainterCaches};
