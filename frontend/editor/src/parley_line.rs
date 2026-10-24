// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The editor's door to the shaper (`imba::shape`): re-exports the
//! shaping value types and seeds a font world with the editor's
//! embedded faces — the hermetic test font and the bundled code
//! font.

pub use imba::shape::{
    BoxAlign, DecorationSpec, FontWorld, InlineBoxSpec, ParleyLine, ShapeSpec, Shaper, ShapedRow,
    SpanStyle,
};

/// A font world carrying the editor's embedded faces. The edge
/// builds ONE at boot and hands it to every `UiCtx` it mints
/// (`ui.set(world.clone())`); a ctx without one builds its own and
/// pays for its own system scan, which is what a bare test wants.
pub fn world() -> FontWorld {
    let world = FontWorld::new();
    #[cfg(not(target_os = "emscripten"))]
    {
        world.register(crate::embedded_fonts::bytes());
        for cut in crate::embedded_fonts::code_font_cuts() {
            world.register(cut);
        }
        world.register(crate::embedded_fonts::chrome_font_bytes());
    }
    world
}

/// The shaper of this context, with the editor's faces in it: the
/// ctx keeps its own world when the edge seeded one, and otherwise
/// gets a seeded world here rather than a bare one.
pub fn shaper(ui: &imba::ui::UiCtx) -> std::rc::Rc<Shaper> {
    if ui.get::<FontWorld>().is_none() {
        ui.set(world());
    }
    Shaper::of(ui)
}

#[cfg(test)]
mod tests;
