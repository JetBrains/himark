// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! imba on gpui: the bridge and nothing more. One side speaks
//! imba — `gfx::Painter`, `imba::event` — and the other speaks
//! gpui: scene primitives, keystrokes, cursor styles. What hangs a
//! workbench off this bridge (an engine, windows, a clipboard) is
//! the SHELL'S business; this crate knows neither himark nor
//! workbench.
//!
//! - [`GpuiPainter`]: `gfx::Painter` over a gpui window — quads,
//!   lyon-stroked paths, rect clips as content masks, glyph runs
//!   through the atlas, images, analytic shadows.
//! - [`events`]: gpui input → imba input, both directions nameable
//!   (a pointer shape back to a cursor style).
//! - variable faces are pinned to static instances before they meet
//!   gpui's font store (see `instance.rs` for why).

mod instance;
mod painter;

pub mod events;

pub use painter::{GpuiPainter, PainterCaches};
