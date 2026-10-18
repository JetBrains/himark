// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The editor's door to the shaper (`imba::shape`): re-exports the
//! shaping value types and registers the editor's embedded faces —
//! the hermetic test font and the bundled code font — exactly once
//! per process, before the first shape.

pub use imba::shape::{
    BoxAlign, DecorationSpec, InlineBoxSpec, ParleyLine, ShapeSpec, ShapedRow, SpanStyle,
};

/// Register the editor's embedded fonts with the shaper. Idempotent
/// and cheap after the first call; every editor shaping road calls
/// it before shaping.
pub fn ensure_fonts() {
    #[cfg(not(target_os = "emscripten"))]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            imba::shape::register_font(crate::embedded_fonts::bytes());
            for cut in crate::embedded_fonts::code_font_cuts() {
                imba::shape::register_font(cut);
            }
        });
    }
}

#[cfg(test)]
mod tests;
