use std::collections::HashMap;
use std::sync::Arc;

use imba::event::{Key, Modifiers};
use imba::store::Store;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Chord {
    key: Key,
    mods: Modifiers,
}

impl Chord {
    fn of(key: Key, mods: Modifiers) -> Self {
        let key = match key {
            Key::Char(c) => Key::Char(c.to_ascii_lowercase()),
            key => key,
        };
        Self { key, mods }
    }

    fn display(&self) -> String {
        let mut out = String::new();
        if self.mods.control {
            out.push('⌃');
        }
        if self.mods.alt {
            out.push('⌥');
        }
        if self.mods.shift {
            out.push('⇧');
        }
        if self.mods.command {
            out.push('⌘');
        }
        match self.key {
            Key::Backspace => out.push('⌫'),
            Key::Enter => out.push('⏎'),
            Key::Left => out.push('←'),
            Key::Right => out.push('→'),
            Key::Up => out.push('↑'),
            Key::Down => out.push('↓'),
            Key::Escape => out.push('⎋'),
            Key::Tab => out.push('⇥'),
            Key::Home => out.push('↖'),
            Key::End => out.push('↘'),
            Key::PageUp => out.push('⇞'),
            Key::PageDown => out.push('⇟'),
            Key::Delete => out.push('⌦'),
            Key::F(n) => out.push_str(&format!("F{n}")),
            Key::Char(c) => out.push(c.to_ascii_uppercase()),
        }
        out
    }

    fn parse(chord: &str) -> Result<Self, String> {
        let mut mods = Modifiers::default();
        let mut key = None;
        let segments: Vec<&str> = chord.split('-').collect();

        for (index, segment) in segments.iter().enumerate() {
            let last = index + 1 == segments.len();
            let lower = segment.to_ascii_lowercase();
            match (last, lower.as_str()) {
                (false, "cmd") => mods.command = true,
                (false, "ctrl") => mods.control = true,
                (false, "alt") => mods.alt = true,
                (false, "shift") => mods.shift = true,
                (false, other) => {
                    return Err(format!("'{chord}': unknown modifier '{other}'"));
                }
                (true, name) => key = Some(Self::key_name(chord, name)?),
            }
        }
        let key = key.ok_or_else(|| format!("'{chord}': no key"))?;
        Ok(Self { key, mods })
    }

    fn key_name(chord: &str, name: &str) -> Result<Key, String> {
        let mut chars = name.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return Ok(Key::Char(c.to_ascii_lowercase()));
        }
        Ok(match name {
            "enter" => Key::Enter,
            "tab" => Key::Tab,
            "escape" => Key::Escape,
            "backspace" => Key::Backspace,
            "delete" => Key::Delete,
            "space" => Key::Char(' '),
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            _ => match name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                Some(n @ 1..=12) => Key::F(n),
                _ => return Err(format!("'{chord}': unknown key '{name}'")),
            },
        })
    }
}

#[derive(Clone)]
pub struct Keymap {
    bindings: HashMap<Chord, Arc<str>>,
}

impl Keymap {
    pub fn embedded() -> Self {
        static EMBEDDED: std::sync::OnceLock<Keymap> = std::sync::OnceLock::new();
        EMBEDDED
            .get_or_init(|| {
                Self::from_json(include_str!("../assets/keymap.json"))
                    .expect("the embedded keymap must parse")
            })
            .clone()
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let raw: HashMap<String, String> =
            serde_json::from_str(json).map_err(|error| error.to_string())?;
        let mut bindings = HashMap::with_capacity(raw.len());
        for (chord, id) in raw {
            bindings.insert(Chord::parse(&chord)?, Arc::from(id.as_str()));
        }
        Ok(Self { bindings })
    }

    pub fn command(&self, key: Key, mods: Modifiers) -> Option<&str> {
        self.bindings
            .get(&Chord::of(key, mods))
            .map(|id| id.as_ref())
    }

    fn binding(&self, key: Key, mods: Modifiers) -> Option<Arc<str>> {
        self.bindings.get(&Chord::of(key, mods)).cloned()
    }

    pub fn shortcut(&self, id: &str) -> Option<String> {
        self.bindings
            .iter()
            .find(|(_, held)| held.as_ref() == id)
            .map(|(chord, _)| chord.display())
    }

    pub fn shortcuts_by_id(&self) -> std::collections::HashMap<Arc<str>, String> {
        self.bindings
            .iter()
            .map(|(chord, id)| (Arc::clone(id), chord.display()))
            .collect()
    }
}

#[derive(Clone)]
pub struct Keymaps(pub Keymap);

impl Keymaps {
    pub fn set(store: &mut Store, keymap: Keymap) {
        store.put(Keymaps(keymap));
    }

    pub fn of(store: &Store) -> Keymap {
        store
            .get::<Keymaps>()
            .map(|keymaps| keymaps.0.clone())
            .unwrap_or_else(Keymap::embedded)
    }

    pub fn binding_of(store: &Store, key: Key, mods: Modifiers) -> Option<Arc<str>> {
        match store.get::<Keymaps>() {
            Some(keymaps) => keymaps.0.binding(key, mods),
            None => {
                static EMBEDDED: std::sync::OnceLock<Keymap> = std::sync::OnceLock::new();
                EMBEDDED.get_or_init(Keymap::embedded).binding(key, mods)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grammar_parses_and_normalizes() {
        let chord = |s: &str| Chord::parse(s).expect(s);
        assert_eq!(
            chord("cmd-shift-t"),
            Chord {
                key: Key::Char('t'),
                mods: Modifiers {
                    command: true,
                    shift: true,
                    ..Default::default()
                }
            }
        );
        assert_eq!(
            chord("CMD-Shift-T"),
            chord("cmd-shift-t"),
            "case-insensitive"
        );
        assert_eq!(
            chord("shift-cmd-t"),
            chord("cmd-shift-t"),
            "order-insensitive"
        );
        assert_eq!(chord("ctrl-alt-left").key, Key::Left);
        assert_eq!(chord("f5").key, Key::F(5));
        assert_eq!(chord("cmd-[").key, Key::Char('['));
        assert_eq!(chord("escape").key, Key::Escape);

        assert!(Chord::parse("cmd-meta-t").unwrap_err().contains("meta"));
        assert!(Chord::parse("cmd-f13").unwrap_err().contains("f13"));
        assert!(Chord::parse("").unwrap_err().contains("no key") || Chord::parse("").is_err());
    }

    #[test]
    fn lookups_normalize_char_case() {
        let keymap = Keymap::from_json(r#"{ "cmd-shift-z": "editor.redo" }"#).expect("parses");
        let mods = Modifiers {
            command: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(keymap.command(Key::Char('z'), mods), Some("editor.redo"));
        assert_eq!(
            keymap.command(Key::Char('Z'), mods),
            Some("editor.redo"),
            "shells disagree on case under shift — both forms hit"
        );
        assert_eq!(keymap.command(Key::Char('z'), Modifiers::default()), None);
    }

    #[test]
    fn the_embedded_keymap_parses() {
        let keymap = Keymap::embedded();
        let mods = Modifiers {
            command: true,
            ..Default::default()
        };
        assert_eq!(keymap.command(Key::Char('z'), mods), Some("editor.undo"));
    }
}
