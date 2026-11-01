// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The app's own face handle. A `Typeface` is BYTES — a font file
//! (possibly one cut of a collection) plus the variation coordinates
//! it stands at; parsing is skrifa, the engine parley already
//! shapes with. A backend resolves the bytes into its native face
//! (a skia `SkTypeface`, a gpui `FontId`) and caches by identity.
//!
//! `Font::measure_str` is the shaping-free road's ruler: cmap plus
//! advances, no shaping — the same thing skia's `measureText`
//! answered, computed from the same tables.

use std::sync::{Arc, Mutex, OnceLock};

use skrifa::raw::TableProvider as _;
use skrifa::MetadataProvider as _;

/// A four-byte axis tag — `wght`, `wdth` — as a big-endian u32.
pub type Tag = u32;

pub const WGHT: Tag = u32::from_be_bytes(*b"wght");
pub const WDTH: Tag = u32::from_be_bytes(*b"wdth");

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FontStyle {
    pub weight: f32,
    pub italic: bool,
}

impl FontStyle {
    pub fn normal() -> Self {
        Self {
            weight: 400.0,
            italic: false,
        }
    }

    pub fn bold() -> Self {
        Self {
            weight: 700.0,
            italic: false,
        }
    }

    pub fn italic() -> Self {
        Self {
            weight: 400.0,
            italic: true,
        }
    }

    pub fn bold_italic() -> Self {
        Self {
            weight: 700.0,
            italic: true,
        }
    }
}

struct TypefaceInner {
    data: Arc<dyn AsRef<[u8]> + Send + Sync>,
    index: u32,
    /// The variation position this handle stands at — applied by
    /// the measuring here and by every backend when it instances.
    coords: Vec<(Tag, f32)>,
    family: String,
    weight: f32,
    italic: bool,
    /// The number of glyphs — identity guard and notdef oracle.
    glyph_count: u16,
    units_per_em: u16,
}

/// Cheap to clone, keyed by pointer identity plus coords — two
/// clones of one load ARE the same face to a backend cache.
#[derive(Clone)]
pub struct Typeface {
    inner: Arc<TypefaceInner>,
}

impl std::fmt::Debug for Typeface {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Typeface")
            .field("family", &self.inner.family)
            .field("index", &self.inner.index)
            .field("coords", &self.inner.coords)
            .finish()
    }
}

impl PartialEq for Typeface {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
            || (self.data_id() == other.data_id()
                && self.inner.index == other.inner.index
                && self.inner.coords == other.inner.coords)
    }
}

impl Typeface {
    pub fn from_data(data: Arc<dyn AsRef<[u8]> + Send + Sync>, index: u32) -> Option<Typeface> {
        let font = skrifa::FontRef::from_index((*data).as_ref(), index).ok()?;
        let family = font
            .localized_strings(skrifa::string::StringId::TYPOGRAPHIC_FAMILY_NAME)
            .english_or_first()
            .or_else(|| {
                font.localized_strings(skrifa::string::StringId::FAMILY_NAME)
                    .english_or_first()
            })
            .map(|name| name.chars().collect::<String>())
            .unwrap_or_default();
        let attributes = font.attributes();
        let glyph_count = font.maxp().map(|maxp| maxp.num_glyphs()).unwrap_or(0);
        let units_per_em = font.head().map(|head| head.units_per_em()).unwrap_or(1000);
        Some(Typeface {
            inner: Arc::new(TypefaceInner {
                data,
                index,
                coords: Vec::new(),
                family,
                weight: attributes.weight.value(),
                italic: !matches!(attributes.style, skrifa::attribute::Style::Normal),
                glyph_count,
                units_per_em,
            }),
        })
    }

    pub fn from_bytes(bytes: impl Into<Vec<u8>>, index: u32) -> Option<Typeface> {
        struct Owned(Vec<u8>);
        impl AsRef<[u8]> for Owned {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }
        Self::from_data(Arc::new(Owned(bytes.into())), index)
    }

    /// This face standing at `coords` — the handle only; a backend
    /// instances when it resolves, the ruler applies them here.
    pub fn with_coords(&self, coords: Vec<(Tag, f32)>) -> Typeface {
        Typeface {
            inner: Arc::new(TypefaceInner {
                data: self.inner.data.clone(),
                index: self.inner.index,
                coords,
                family: self.inner.family.clone(),
                weight: self.inner.weight,
                italic: self.inner.italic,
                glyph_count: self.inner.glyph_count,
                units_per_em: self.inner.units_per_em,
            }),
        }
    }

    pub fn family_name(&self) -> String {
        self.inner.family.clone()
    }

    pub fn weight(&self) -> f32 {
        self.inner
            .coords
            .iter()
            .find(|(tag, _)| *tag == WGHT)
            .map(|(_, value)| *value)
            .unwrap_or(self.inner.weight)
    }

    pub fn is_italic(&self) -> bool {
        self.inner.italic
    }

    pub fn count_glyphs(&self) -> u16 {
        self.inner.glyph_count
    }

    pub fn units_per_em(&self) -> u16 {
        self.inner.units_per_em
    }

    pub fn coords(&self) -> &[(Tag, f32)] {
        &self.inner.coords
    }

    pub fn data(&self) -> Arc<dyn AsRef<[u8]> + Send + Sync> {
        self.inner.data.clone()
    }

    pub fn index(&self) -> u32 {
        self.inner.index
    }

    /// A stable identity for backend caches: the bytes' address and
    /// length stand in for content (faces are loaded once and
    /// shared).
    pub fn data_id(&self) -> (usize, usize) {
        let bytes = (*self.inner.data).as_ref();
        (bytes.as_ptr() as usize, bytes.len())
    }

    /// A stable id for memo keys: two handles to the same bytes at
    /// the same coords answer the same id, an instanced clone
    /// answers its own.
    pub fn unique_id(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        self.data_id().hash(&mut hasher);
        self.inner.index.hash(&mut hasher);
        for (tag, value) in &self.inner.coords {
            tag.hash(&mut hasher);
            value.to_bits().hash(&mut hasher);
        }
        hasher.finish()
    }

    /// The design-space range of one variation axis, when the face
    /// has it: `(min, default, max)`.
    pub fn axis(&self, tag: Tag) -> Option<(f32, f32, f32)> {
        let data = self.inner.data.clone();
        let bytes: &[u8] = (*data).as_ref();
        let font = skrifa::FontRef::from_index(bytes, self.inner.index).ok()?;
        let found = font
            .axes()
            .iter()
            .find(|axis| u32::from_be_bytes(axis.tag().to_be_bytes()) == tag)
            .map(|axis| (axis.min_value(), axis.default_value(), axis.max_value()));
        found
    }

    fn skrifa<'a>(&self, bytes: &'a [u8]) -> Option<skrifa::FontRef<'a>> {
        skrifa::FontRef::from_index(bytes, self.inner.index).ok()
    }
}

/// Line metrics in skia's convention: ascent NEGATIVE above the
/// baseline, descent positive below.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
}

/// How glyph edges rasterize — kept for call-site compatibility;
/// every backend anti-aliases regardless.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Edging {
    Alias,
    #[default]
    AntiAlias,
    SubpixelAntiAlias,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    typeface: Typeface,
    size: f32,
    embolden: bool,
    subpixel: bool,
    edging: Edging,
}

impl Font {
    pub fn new(typeface: Typeface, size: f32) -> Self {
        Self {
            typeface,
            size,
            embolden: false,
            subpixel: false,
            edging: Edging::AntiAlias,
        }
    }

    pub fn from_typeface(typeface: Typeface, size: f32) -> Self {
        Self::new(typeface, size)
    }

    pub fn typeface(&self) -> &Typeface {
        &self.typeface
    }

    pub fn size(&self) -> f32 {
        self.size
    }

    pub fn set_size(&mut self, size: f32) -> &mut Self {
        self.size = size;
        self
    }

    pub fn set_embolden(&mut self, embolden: bool) -> &mut Self {
        self.embolden = embolden;
        self
    }

    pub fn is_embolden(&self) -> bool {
        self.embolden
    }

    pub fn set_subpixel(&mut self, subpixel: bool) -> &mut Self {
        self.subpixel = subpixel;
        self
    }

    pub fn set_edging(&mut self, edging: Edging) -> &mut Self {
        self.edging = edging;
        self
    }

    /// Line metrics at this size — skia's sign convention kept:
    /// ascent NEGATIVE above the baseline, descent positive below.
    /// Answers `(recommended line spacing, metrics)` as skia did.
    pub fn metrics(&self) -> (f32, FontMetrics) {
        let data = self.typeface.data();
        let bytes: &[u8] = (*data).as_ref();
        let Some(font) = self.typeface.skrifa(bytes) else {
            let fallback = FontMetrics {
                ascent: -self.size * 0.8,
                descent: self.size * 0.2,
                leading: 0.0,
            };
            return (self.size, fallback);
        };
        let location = font.axes().location(
            self.typeface
                .coords()
                .iter()
                .map(|(tag, value)| (skrifa::Tag::from_be_bytes(tag.to_be_bytes()), *value)),
        );
        let metrics =
            skrifa::metrics::Metrics::new(&font, skrifa::instance::Size::new(self.size), &location);
        let out = FontMetrics {
            ascent: -metrics.ascent.abs(),
            descent: metrics.descent.abs(),
            leading: metrics.leading,
        };
        (out.descent - out.ascent + out.leading, out)
    }

    /// The unshaped advance of `text` — cmap and advances, exactly
    /// what skia's `measureText` summed. Missing glyphs advance by
    /// notdef's width, as skia did.
    pub fn measure_str(&self, text: &str) -> f32 {
        let data = self.typeface.data();
        let bytes = (*data).as_ref();
        let Some(font) = self.typeface.skrifa(bytes) else {
            return 0.0;
        };
        let location = font.axes().location(
            self.typeface
                .coords()
                .iter()
                .map(|(tag, value)| (skrifa::Tag::from_be_bytes(tag.to_be_bytes()), *value)),
        );
        let metrics = font.glyph_metrics(skrifa::instance::Size::new(self.size), &location);
        let charmap = font.charmap();
        text.chars()
            .map(|ch| {
                let glyph = charmap.map(ch).unwrap_or_default();
                metrics.advance_width(glyph).unwrap_or(0.0)
            })
            .sum()
    }

    /// Advances of glyph IDS at this font's size and coords — the
    /// parity meter the shaping tests read.
    pub fn glyph_widths(&self, glyphs: &[u16]) -> Vec<f32> {
        let data = self.typeface.data();
        let bytes: &[u8] = (*data).as_ref();
        let Some(font) = self.typeface.skrifa(bytes) else {
            return vec![0.0; glyphs.len()];
        };
        let location = font.axes().location(
            self.typeface
                .coords()
                .iter()
                .map(|(tag, value)| (skrifa::Tag::from_be_bytes(tag.to_be_bytes()), *value)),
        );
        let metrics = font.glyph_metrics(skrifa::instance::Size::new(self.size), &location);
        glyphs
            .iter()
            .map(|glyph| {
                metrics
                    .advance_width(skrifa::GlyphId::new(*glyph as u32))
                    .unwrap_or(0.0)
            })
            .collect()
    }

    /// How many of `text`'s chars the face has no glyph for — the
    /// notdef count `measure_str` alone hides.
    pub fn missing_glyphs(&self, text: &str) -> usize {
        let data = self.typeface.data();
        let bytes = (*data).as_ref();
        let Some(font) = self.typeface.skrifa(bytes) else {
            return text.chars().count();
        };
        let charmap = font.charmap();
        text.chars().filter(|ch| charmap.map(*ch).is_none()).count()
    }
}

// ─── the system's faces ───

struct System {
    collection: fontique::Collection,
    sources: fontique::SourceCache,
}

static SYSTEM: OnceLock<Mutex<System>> = OnceLock::new();

fn system() -> &'static Mutex<System> {
    SYSTEM.get_or_init(|| {
        Mutex::new(System {
            collection: fontique::Collection::new(fontique::CollectionOptions {
                system_fonts: true,
                ..Default::default()
            }),
            sources: fontique::SourceCache::default(),
        })
    })
}

fn typeface_of(system: &mut System, font: &fontique::FontInfo) -> Option<Typeface> {
    let blob = font.load(Some(&mut system.sources))?;
    struct Blob(fontique::Blob<u8>);
    impl AsRef<[u8]> for Blob {
        fn as_ref(&self) -> &[u8] {
            self.0.as_ref()
        }
    }
    Typeface::from_data(Arc::new(Blob(blob)), font.index())
}

/// The system face for `family` nearest `style` — what
/// `FontMgr::matchFamilyStyle` answered.
pub fn match_family_style(family: &str, style: FontStyle) -> Option<Typeface> {
    let mut system = system().lock().expect("the system font collection");
    let id = system.collection.family_id(family)?;
    let info = system.collection.family(id)?;
    let font = info
        .match_font(
            fontique::FontWidth::NORMAL,
            match style.italic {
                true => fontique::FontStyle::Italic,
                false => fontique::FontStyle::Normal,
            },
            fontique::FontWeight::new(style.weight),
            false,
        )?
        .clone();
    typeface_of(&mut system, &font)
}

/// The platform's default UI face — `legacy_make_typeface(None)`.
pub fn default_typeface(style: FontStyle) -> Option<Typeface> {
    let family = {
        let mut system = system().lock().expect("the system font collection");
        let id = {
            let mut generics = system
                .collection
                .generic_families(fontique::GenericFamily::SystemUi);
            generics.next()
        }
        .or_else(|| {
            let mut generics = system
                .collection
                .generic_families(fontique::GenericFamily::SansSerif);
            generics.next()
        })?;
        system.collection.family_name(id)?.to_string()
    };
    match_family_style(&family, style)
}

/// Does the face map `c` at all — the fallback probe's gate.
impl Typeface {
    pub fn has_glyph(&self, c: char) -> bool {
        let data = self.inner.data.clone();
        let bytes: &[u8] = (*data).as_ref();
        self.skrifa(bytes)
            .map(|font| font.charmap().map(c).is_some())
            .unwrap_or(false)
    }
}

/// The system face that COVERS `c`, by the char's script fallback
/// chain — `matchFamilyStyleCharacter`, the terminal's odd-glyph
/// road.
pub fn fallback_for_char(c: char, style: FontStyle) -> Option<Typeface> {
    use unicode_script::UnicodeScript as _;
    let short = c.script().short_name();
    let tag: [u8; 4] = short.as_bytes().try_into().ok()?;
    let script = fontique::Script::from_bytes(tag);
    let ids: Vec<fontique::FamilyId> = {
        let mut system = system().lock().expect("the system font collection");
        system.collection.fallback_families(script).collect()
    };
    for id in ids {
        let mut system = system().lock().expect("the system font collection");
        let Some(info) = system.collection.family(id) else {
            continue;
        };
        let Some(font) = info
            .match_font(
                fontique::FontWidth::NORMAL,
                match style.italic {
                    true => fontique::FontStyle::Italic,
                    false => fontique::FontStyle::Normal,
                },
                fontique::FontWeight::new(style.weight),
                false,
            )
            .cloned()
        else {
            continue;
        };
        if let Some(face) = typeface_of(&mut system, &font) {
            if face.has_glyph(c) {
                return Some(face);
            }
        }
    }
    None
}
