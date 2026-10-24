// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The open theme: a bag of named SECTIONS, each owned by the
//! subsystem that reads it. The theme file is one JSON object; a
//! subsystem declares a [`Key`] — a section name closed over the
//! section's type — and [`Theme::part`] deserializes that section on
//! first lookup and caches it. Nothing here knows any section's
//! shape, so no crate learns another's styling by depending on the
//! theme.

use std::any::Any;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};

use serde::de::DeserializeOwned;
use serde::Deserialize;
use skia_safe::Color;

/// A color in a theme file: `#rrggbb` or `#aarrggbb`.
#[derive(Clone, Copy, Deserialize)]
#[serde(try_from = "String")]
pub struct Rgba(pub Color);

impl TryFrom<String> for Rgba {
    type Error = String;
    fn try_from(hex: String) -> Result<Self, String> {
        parse_color(&hex).map(Rgba)
    }
}

impl From<Rgba> for Color {
    fn from(rgba: Rgba) -> Color {
        rgba.0
    }
}

pub fn parse_color(hex: &str) -> Result<Color, String> {
    let digits = hex.strip_prefix('#').unwrap_or(hex);
    let value = u32::from_str_radix(digits, 16).map_err(|error| format!("{hex}: {error}"))?;
    Ok(match digits.len() {
        6 => Color::from_argb(0xff, (value >> 16) as u8, (value >> 8) as u8, value as u8),
        8 => Color::from_argb(
            (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ),
        _ => return Err(format!("{hex}: expected #rrggbb or #aarrggbb")),
    })
}

/// One rung of the type ramp: a size, a weight on the chrome face's
/// `wght` axis, a width on `wdth` (100 = normal), and the tracking
/// in physical pixels.
#[derive(Clone, Copy, Deserialize)]
#[serde(default)]
pub struct TypeRole {
    pub size: f32,
    pub weight: f32,
    pub width: f32,
    pub tracking: f32,
    /// The line height, as a multiple of the size, where a role
    /// sets lines (prose, rows).
    pub line_height: f32,
}

impl Default for TypeRole {
    fn default() -> Self {
        Self {
            size: 28.0,
            weight: 400.0,
            width: 100.0,
            tracking: 0.0,
            line_height: 1.0,
        }
    }
}

impl TypeRole {
    pub const fn new(size: f32, weight: f32) -> Self {
        Self {
            size,
            weight,
            width: 100.0,
            tracking: 0.0,
            line_height: 1.0,
        }
    }
}

/// A section's name, closed over the type its JSON deserializes
/// into. Each subsystem mints its own as a `const`:
///
/// ```ignore
/// impl ToolbarChrome {
///     pub const KEY: Key<ToolbarChrome> = Key::new("toolbar");
/// }
/// ```
pub struct Key<T> {
    name: &'static str,
    marker: PhantomData<fn() -> T>,
}

impl<T> Key<T> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            marker: PhantomData,
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }
}

impl<T> Clone for Key<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Key<T> {}

impl<T> std::fmt::Debug for Key<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Key<{}>({:?})",
            std::any::type_name::<T>().rsplit("::").next().unwrap_or(""),
            self.name
        )
    }
}

/// One theme: a name and the raw JSON of every section, with the
/// deserialized parts cached by section name on first lookup. Clones
/// share the cache; switching themes mints a new `Theme`, so stale
/// parts can't outlive their JSON.
#[derive(Clone)]
pub struct Theme {
    name: Arc<str>,
    sections: Arc<HashMap<String, serde_json::Value>>,
    /// Keyed by (section name, part type): a section usually has one
    /// owner, but a crate that can't depend on the owner may read its
    /// own typed SLICE of the same section under a second type.
    cache: Arc<RwLock<HashMap<(&'static str, std::any::TypeId), Arc<dyn Any + Send + Sync>>>>,
}

impl Theme {
    /// Parse a theme file. The envelope is `{"name": …, "ui": {…},
    /// …}`: `ui`'s children become sections, as does every other
    /// top-level key — the envelope is the only shape this crate
    /// knows; the sections' insides stay each owner's business.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let raw: serde_json::Value =
            serde_json::from_str(json).map_err(|error| error.to_string())?;
        let serde_json::Value::Object(mut top) = raw else {
            return Err("a theme is a JSON object".to_owned());
        };
        let name = match top.remove("name") {
            Some(serde_json::Value::String(name)) => name,
            Some(_) => return Err("the theme's name is a string".to_owned()),
            None => "dark".to_owned(),
        };
        let mut sections = HashMap::new();
        if let Some(ui) = top.remove("ui") {
            let serde_json::Value::Object(ui) = ui else {
                return Err("the theme's ui is a JSON object".to_owned());
            };
            sections.extend(ui);
        }
        sections.extend(top);
        Ok(Self {
            name: name.into(),
            sections: Arc::new(sections),
            cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// An empty theme — every part answers its default. Bare-store
    /// tests seed this.
    pub fn empty(name: &str) -> Self {
        Self {
            name: name.into(),
            sections: Arc::new(HashMap::new()),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn name_shared(&self) -> Arc<str> {
        Arc::clone(&self.name)
    }

    /// This theme's section under the key's name. An absent section
    /// is the part's default; a malformed one PANICS with the section
    /// named — themes ship with the app, so a parse error is a
    /// developer's, caught loudly on first paint and in tests.
    pub fn part<T>(&self, key: Key<T>) -> Arc<T>
    where
        T: DeserializeOwned + Default + Send + Sync + 'static,
    {
        let slot = (key.name, std::any::TypeId::of::<T>());
        if let Some(cached) = self.cache.read().expect("theme cache").get(&slot) {
            return cached
                .clone()
                .downcast::<T>()
                .expect("the slot's TypeId keyed this type");
        }
        let part: Arc<T> = Arc::new(match self.sections.get(key.name) {
            None => T::default(),
            Some(section) => serde_json::from_value(section.clone()).unwrap_or_else(|error| {
                panic!("theme {:?}, section {:?}: {error}", &*self.name, key.name)
            }),
        });
        self.cache
            .write()
            .expect("theme cache")
            .entry(slot)
            .or_insert(part.clone());
        part
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize, Default, PartialEq, Debug)]
    #[serde(default)]
    struct Knobs {
        size: f32,
        label: String,
    }

    impl Knobs {
        const KEY: Key<Knobs> = Key::new("knobs");
    }

    #[test]
    fn sections_come_from_ui_children_and_top_level_keys() {
        let theme = Theme::from_json(
            r#"{ "name": "test", "ui": { "knobs": { "size": 4.0 } }, "styles": {} }"#,
        )
        .expect("parses");
        assert_eq!(theme.name(), "test");
        assert_eq!(theme.part(Knobs::KEY).size, 4.0);
    }

    #[test]
    fn an_absent_section_answers_the_default() {
        let theme = Theme::empty("bare");
        assert_eq!(*theme.part(Knobs::KEY), Knobs::default());
    }

    #[test]
    fn a_partial_section_merges_over_serde_defaults() {
        let theme =
            Theme::from_json(r#"{ "ui": { "knobs": { "label": "hi" } } }"#).expect("parses");
        let knobs = theme.part(Knobs::KEY);
        assert_eq!(knobs.label, "hi");
        assert_eq!(knobs.size, 0.0);
    }

    #[test]
    fn clones_share_the_parsed_part() {
        let theme = Theme::from_json(r#"{ "ui": { "knobs": { "size": 2.0 } } }"#).expect("parses");
        let first = theme.clone().part(Knobs::KEY);
        let again = theme.part(Knobs::KEY);
        assert!(Arc::ptr_eq(&first, &again), "one deserialize, shared");
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct KnobSlice {
        size: f32,
    }

    impl KnobSlice {
        const KEY: Key<KnobSlice> = Key::new("knobs");
    }

    #[test]
    fn two_typed_views_can_read_one_section() {
        let theme = Theme::from_json(r#"{ "ui": { "knobs": { "size": 7.0, "label": "x" } } }"#)
            .expect("parses");
        assert_eq!(theme.part(Knobs::KEY).label, "x");
        assert_eq!(theme.part(KnobSlice::KEY).size, 7.0);
    }

    #[test]
    #[should_panic(expected = "section \"knobs\"")]
    fn a_malformed_section_panics_loudly() {
        let theme =
            Theme::from_json(r#"{ "ui": { "knobs": { "size": "wide" } } }"#).expect("parses");
        let _ = theme.part(Knobs::KEY);
    }
}

/// The store resource every view reads the current theme from.
#[derive(Clone)]
pub struct Themes(pub Theme);

impl Themes {
    pub fn set(store: &mut crate::store::Store, theme: Theme) {
        store.put(Themes(theme));
    }

    /// The seeded theme; a bare store (a unit test) answers the
    /// empty one, whose every part is its default — the dark values.
    pub fn of(store: &crate::store::Store) -> Theme {
        store
            .get::<Themes>()
            .map(|themes| themes.0.clone())
            .unwrap_or_else(|| Theme::empty("dark"))
    }
}
