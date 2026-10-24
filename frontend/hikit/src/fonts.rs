// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The chrome faces: ONE variable family (Bricolage Grotesque) for
//! every chrome label, instanced per ramp rung — a weight on `wght`,
//! a width on `wdth` — plus the mono face for the margin register.
//! An instance is a skia `Typeface` clone standing at its axes; the
//! label shaper reads those axes back off the typeface
//! (`imba::layout::TextShaper`), so the skia metrics road and the
//! parley shaping road agree on the cut. Instances are cached per
//! `UiCtx` so a typeface's identity is stable across frames — the
//! label memo keys on it.

use std::cell::RefCell;
use std::collections::HashMap;

use editor::theme::TypeRole;

/// The chrome-face instances this ctx has minted, by (weight, width).
struct ChromeFaces(RefCell<HashMap<(u32, u32), skia_safe::Typeface>>);

struct MonoTypeface(skia_safe::Typeface);

fn chrome_typeface(ui: &imba::ui::UiCtx, weight: f32, width: f32) -> skia_safe::Typeface {
    let faces = ui.env(|| ChromeFaces(RefCell::new(HashMap::new())));
    let key = (weight.to_bits(), width.to_bits());
    if let Some(face) = faces.0.borrow().get(&key) {
        return face.clone();
    }
    let base = editor::embedded_fonts::chrome_typeface();
    let coordinates = [
        skia_safe::font_arguments::variation_position::Coordinate {
            axis: skia_safe::font_arguments::variation_position::Coordinate::wght,
            value: weight,
        },
        skia_safe::font_arguments::variation_position::Coordinate {
            axis: skia_safe::font_arguments::variation_position::Coordinate::wdth,
            value: width,
        },
    ];
    let arguments = skia_safe::FontArguments::new().set_variation_design_position(
        skia_safe::font_arguments::VariationPosition {
            coordinates: &coordinates,
        },
    );
    let face = base.clone_with_arguments(&arguments).unwrap_or(base);
    faces.0.borrow_mut().insert(key, face.clone());
    face
}

fn font_of(typeface: skia_safe::Typeface, size: f32) -> skia_safe::Font {
    let mut font = skia_safe::Font::from_typeface(typeface, size);
    font.set_edging(skia_safe::font::Edging::AntiAlias);
    font.set_subpixel(true);
    font
}

/// The chrome face at a size, weight and width — the ramp's door.
pub fn chrome_font(ui: &imba::ui::UiCtx, size: f32, weight: f32, width: f32) -> skia_safe::Font {
    font_of(chrome_typeface(ui, weight, width), size)
}

/// The chrome face at a ramp rung.
pub fn role_font(ui: &imba::ui::UiCtx, role: &TypeRole) -> skia_safe::Font {
    chrome_font(ui, role.size, role.weight, role.width)
}

/// The TITLE weight of the chrome face at `size` — the role every
/// emphasized label used to ask for as "bold".
pub fn ui_font(ui: &imba::ui::UiCtx, size: f32) -> skia_safe::Font {
    chrome_font(ui, size, TITLE_WEIGHT, NORMAL_WIDTH)
}

/// The regular weight of the chrome face at `size`.
pub fn ui_text_font(ui: &imba::ui::UiCtx, size: f32) -> skia_safe::Font {
    chrome_font(ui, size, TEXT_WEIGHT, NORMAL_WIDTH)
}

/// The mono face (the bundled code font) at `size` — keys, tool
/// invocations, line numbers: the margin register.
pub fn mono_font(ui: &imba::ui::UiCtx, size: f32) -> skia_safe::Font {
    let typeface = ui.env(|| MonoTypeface(editor::embedded_fonts::code_typeface()));
    font_of(typeface.0.clone(), size)
}

/// The weights `ui_font`/`ui_text_font` stand at — the title and
/// text rungs of the ramp (docs/Design.md).
const TITLE_WEIGHT: f32 = 640.0;
const TEXT_WEIGHT: f32 = 400.0;
const NORMAL_WIDTH: f32 = 100.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// The ramp is real: a label at the title weight shapes WIDER
    /// than the same label at the text weight, through the same
    /// shaper — the variable axis reached parley and the skia
    /// instance both, and the memo keeps the two apart.
    #[test]
    fn chrome_weights_instance_the_variable_axis() {
        let ui = imba::ui::UiCtx::dont_use_too_slow();
        ui.set(editor::parley_line::world());
        let shaper = imba::layout::TextShaper::of(&ui);
        let regular = ui_text_font(&ui, 34.0);
        let title = ui_font(&ui, 34.0);
        assert_eq!(
            regular.typeface().family_name(),
            editor::embedded_fonts::CHROME_FAMILY
        );
        let narrow = chrome_font(&ui, 34.0, 560.0, 88.0);
        let text = "the document feed is an ordered lane";
        let (thin, bold, condensed) = (
            shaper.advance(&regular, text),
            shaper.advance(&title, text),
            shaper.advance(&narrow, text),
        );
        assert!(bold > thin + 4.0, "title weight is heavier: {thin} vs {bold}");
        assert!(condensed < thin - 4.0, "wdth 88 is narrower: {thin} vs {condensed}");
        // Stable identity: the same rung answers the same typeface.
        assert_eq!(
            ui_font(&ui, 34.0).typeface().unique_id(),
            title.typeface().unique_id()
        );
    }
}
