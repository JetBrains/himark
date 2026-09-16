// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The same variable fonts as Air UI. Cache each optical weight per UI thread.
use skia_safe::{
    font_arguments::{variation_position::Coordinate, VariationPosition},
    Font, FontArguments, FontMgr, Typeface,
};
use std::cell::RefCell;
use std::collections::HashMap;

pub fn font(size: f32, weight: u16, mono: bool) -> Font {
    thread_local! {
        static FONTS: RefCell<HashMap<(u16, bool, u32), Typeface>> = RefCell::new(HashMap::new());
    }
    let typeface = FONTS.with(|fonts| {
        fonts
            .borrow_mut()
            .entry((weight, mono, size.to_bits()))
            .or_insert_with(|| {
                let bytes: &[u8] = if mono {
                    include_bytes!("../../assets/air-ui/JetBrainsMonoVariable-latin.ttf")
                } else {
                    include_bytes!("../../assets/air-ui/InterVariable-latin.ttf")
                };
                let base = FontMgr::new()
                    .new_from_data(bytes, None)
                    .expect("embedded Air font");
                let coordinates = [
                    Coordinate {
                        axis: Coordinate::wght,
                        value: f32::from(weight),
                    },
                    Coordinate {
                        axis: Coordinate::opsz,
                        value: size,
                    },
                ];
                let typeface = base
                    .clone_with_arguments(&FontArguments::new().set_variation_design_position(
                        VariationPosition {
                            coordinates: &coordinates,
                        },
                    ))
                    .expect("Air font weight");
                typeface
            })
            .clone()
    });
    let mut font = Font::from_typeface(typeface, size);
    font.set_edging(skia_safe::font::Edging::AntiAlias);
    font.set_subpixel(true);
    font
}
