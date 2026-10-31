// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! gpui input → imba input. Pure translation: no engine, no
//! policy — a shell decides what to dispatch and when.

use imba::event::{Key, Modifiers, PointerShape};

/// gpui keystroke → the key imba speaks, and the text the stroke
/// would insert (the shell gates insertion on its own modifier
/// policy).
pub fn key_of(keystroke: &gpui::Keystroke) -> (Option<Key>, Option<String>) {
    let named = match keystroke.key.as_str() {
        "backspace" => Some(Key::Backspace),
        "enter" => Some(Key::Enter),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "escape" => Some(Key::Escape),
        "tab" => Some(Key::Tab),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" => Some(Key::PageUp),
        "pagedown" => Some(Key::PageDown),
        "delete" => Some(Key::Delete),
        "space" => Some(Key::Char(' ')),
        key if key.len() >= 2 && key.starts_with('f') => key[1..]
            .parse::<u8>()
            .ok()
            .filter(|n| (1..=12).contains(n))
            .map(Key::F),
        _ => None,
    };
    if let Some(key) = named {
        let text = matches!(key, Key::Char(' ')).then(|| {
            keystroke
                .key_char
                .clone()
                .unwrap_or_else(|| " ".to_owned())
        });
        return (Some(key), text);
    }

    let mut chars = keystroke.key.chars();
    let key = match (chars.next(), chars.next()) {
        (Some(ch), None) if ch >= ' ' => Some(Key::Char(ch)),
        _ => None,
    };
    (key, keystroke.key_char.clone())
}

pub fn modifiers_of(modifiers: gpui::Modifiers) -> Modifiers {
    Modifiers {
        shift: modifiers.shift,
        control: modifiers.control,
        alt: modifiers.alt,
        command: modifiers.platform,
    }
}

/// The answered pointer shape, back onto the window's cursor.
pub fn cursor_style_of(shape: PointerShape) -> gpui::CursorStyle {
    match shape {
        PointerShape::Pointer => gpui::CursorStyle::PointingHand,
        PointerShape::Default => gpui::CursorStyle::Arrow,
    }
}

/// A gpui window point in imba's device pixels.
pub fn device_point(position: gpui::Point<gpui::Pixels>, scale: f32) -> gfx::Point {
    gfx::Point::new(f32::from(position.x) * scale, f32::from(position.y) * scale)
}
