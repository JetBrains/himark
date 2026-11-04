// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::OnceLock;

use gfx::Typeface;

static NOTO_SANS: &[u8] = include_bytes!("../assets/NotoSans-Regular.ttf");

/// The raw face bytes — the parley road registers the same bytes
/// skia rasterizes with.
pub fn bytes() -> &'static [u8] {
    NOTO_SANS
}

/// The CODE FONT ships in the binary: "JetBrains Mono" is nobody's
/// system font — naming it in the theme while trusting the platform
/// to have it handed skparagraph a missing family to re-walk on
/// every shape and let the two text roads disagree on the fallback.
/// Both roads register these same bytes, so the family always
/// resolves, identically, everywhere.
static JETBRAINS_MONO: [&[u8]; 4] = [
    include_bytes!("../assets/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/JetBrainsMono-Italic.ttf"),
    include_bytes!("../assets/JetBrainsMono-BoldItalic.ttf"),
];

pub fn code_font_cuts() -> &'static [&'static [u8]] {
    &JETBRAINS_MONO
}

/// The code family's name, as the font declares it.
pub const CODE_FAMILY: &str = "JetBrains Mono";

static CODE_TYPEFACE: OnceLock<Typeface> = OnceLock::new();

/// The code face's regular cut, for skia-side chrome that sets the
/// margin register in mono (keys, tool invocations, line numbers).
pub fn code_typeface() -> Typeface {
    CODE_TYPEFACE
        .get_or_init(|| {
            Typeface::from_bytes(JETBRAINS_MONO[0], 0).expect("embedded JetBrains Mono loads")
        })
        .clone()
}

/// The CHROME FONT ships in the binary too: Bricolage Grotesque, one
/// variable family (opsz · wdth · wght) for every chrome size — the
/// typographic redesign's whole UI voice (docs/Design.md). Both text
/// roads register these bytes; `chrome_typeface` instances it for
/// skia-side metrics.
static CHROME: &[u8] = include_bytes!("../assets/BricolageGrotesque-Variable.ttf");

pub fn chrome_font_bytes() -> &'static [u8] {
    CHROME
}

/// The chrome family's name, as the font declares it.
pub const CHROME_FAMILY: &str = "Bricolage Grotesque";

static CHROME_TYPEFACE: OnceLock<Typeface> = OnceLock::new();

/// The chrome face at its DEFAULT variation — callers instance a
/// weight/width with `clone_with_arguments`. Falls back to the text
/// face on the web, where the chrome arrives over the wire.
pub fn chrome_typeface() -> Typeface {
    #[cfg(not(target_os = "emscripten"))]
    {
        CHROME_TYPEFACE
            .get_or_init(|| {
                Typeface::from_bytes(CHROME, 0).expect("embedded Bricolage Grotesque loads")
            })
            .clone()
    }
    #[cfg(target_os = "emscripten")]
    {
        typeface()
    }
}

static TYPEFACE: OnceLock<Typeface> = OnceLock::new();

pub const FAMILY: &str = "Noto Sans";

pub fn typeface() -> Typeface {
    TYPEFACE
        .get_or_init(|| Typeface::from_bytes(NOTO_SANS, 0).expect("embedded Noto Sans loads"))
        .clone()
}
