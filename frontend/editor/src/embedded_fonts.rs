// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::OnceLock;

use skia_safe::{FontMgr, Typeface};

#[cfg(not(target_os = "emscripten"))]
static NOTO_SANS: &[u8] = include_bytes!("../assets/NotoSans-Regular.ttf");

/// The raw face bytes — the parley road registers the same bytes
/// skia rasterizes with.
#[cfg(not(target_os = "emscripten"))]
pub fn bytes() -> &'static [u8] {
    NOTO_SANS
}

/// The CODE FONT ships in the binary: "JetBrains Mono" is nobody's
/// system font — naming it in the theme while trusting the platform
/// to have it handed skparagraph a missing family to re-walk on
/// every shape and let the two text roads disagree on the fallback.
/// Both roads register these same bytes, so the family always
/// resolves, identically, everywhere.
#[cfg(not(target_os = "emscripten"))]
static JETBRAINS_MONO: [&[u8]; 4] = [
    include_bytes!("../assets/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/JetBrainsMono-Italic.ttf"),
    include_bytes!("../assets/JetBrainsMono-BoldItalic.ttf"),
];

#[cfg(not(target_os = "emscripten"))]
pub fn code_font_cuts() -> &'static [&'static [u8]] {
    &JETBRAINS_MONO
}

static TYPEFACE: OnceLock<Typeface> = OnceLock::new();

#[cfg(not(target_os = "emscripten"))]
pub const FAMILY: &str = "Noto Sans";

#[cfg(target_os = "emscripten")]
pub const FAMILY: &str = "JetBrains Mono";

/// The web's face arrives over the wire. It is handed BACK to the
/// caller, who puts it in the font world the app boots with — the
/// shaper reads no registry of its own. Skia keeps a typeface for
/// the chrome it rasterizes directly.
#[cfg(target_os = "emscripten")]
pub fn install_bytes(bytes: Vec<u8>) -> &'static [u8] {
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let typeface = FontMgr::new()
        .new_from_data(skia_safe::Data::new_copy(bytes), None)
        .expect("the web font parses");
    assert!(
        TYPEFACE.set(typeface).is_ok(),
        "the web typeface was installed more than once"
    );
    bytes
}

pub fn typeface() -> Typeface {
    #[cfg(not(target_os = "emscripten"))]
    {
        TYPEFACE
            .get_or_init(|| {
                FontMgr::new()
                    .new_from_data(skia_safe::Data::new_copy(NOTO_SANS), None)
                    .expect("embedded Noto Sans loads")
            })
            .clone()
    }
    #[cfg(target_os = "emscripten")]
    {
        TYPEFACE
            .get()
            .expect("the web font must load before the application starts")
            .clone()
    }
}
