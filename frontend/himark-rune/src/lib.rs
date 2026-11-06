// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Himark's BRIDGES, adapted to rune — a PURE adapter, the gpui
//! bridge's sibling. The app composes the engine (bare + plugins,
//! or more) and hands it to [`Engine::adopt`]; a [`BridgeView`]
//! pairs one `El::effect` slot with the compositor that paints the
//! bridge's recording at it. Rune keeps layout, animation, input
//! and the window; himark's glyphs rasterize into rune's own atlas
//! (`TextSystem::glyph_quad` — both sides shape with parley).

mod engine;
mod painter;
mod view;

pub use engine::Engine;
pub use view::{BridgeCompositor, BridgeView};

/// The embedding app's rune — re-exported so an example needs only
/// this crate for the view side.
pub use rune;
