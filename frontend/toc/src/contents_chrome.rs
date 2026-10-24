// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::theme::{Key, Rgba};
use serde::Deserialize;
use skia_safe::Color;

/// The Contents drawer: an outline set like a book's table of
/// contents — the KIND word in a right-aligned margin column, the
/// name, a dotted LEADER to the line number. The entry the caret is
/// in stands a size up in the live ink.
impl ContentsChrome {
    pub const KEY: Key<ContentsChrome> = Key::new("contents");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ContentsChrome {
    /// The column's text edge — the header's title and the rows'
    /// left and right inset.
    pub text_edge: f32,
    /// The air above the first entry.
    pub top_gap: f32,
    /// The kind margin's width (its word right-aligned in it) and
    /// the air between that word and the column's end.
    pub kind_column: f32,
    pub kind_pad: f32,
    pub kind_size: f32,
    /// The kind word's oblique slant — the chrome face has no italic
    /// cut, so the word leans by skew.
    pub kind_slant: f32,
    /// The gap between a row's words (kind | name | leader | line).
    pub gap: f32,
    /// The plain entry's name (mono) and the type's (mono, bold).
    pub name_size: f32,
    pub name_color: Rgba,
    pub type_size: f32,
    /// A nested member's name and its indent per level.
    pub member_color: Rgba,
    pub member_indent: f32,
    pub number_size: f32,
    pub number_min_width: f32,
    pub leader_color: Rgba,
    pub leader_min: f32,
    /// The leader's dot (and the gap between dots) and its rise
    /// above the baseline.
    pub leader_dot: f32,
    pub leader_rise: f32,
    /// A row's height (its line plus the air above and below), and
    /// the current entry's.
    pub row_height: f32,
    pub current_row_height: f32,
    pub current_name_size: f32,
    /// The air above a group head (after the first) and below it.
    pub group_gap: f32,
    pub group_after: f32,
}

impl Default for ContentsChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            text_edge: 40.0,
            top_gap: 36.0,
            kind_column: 88.0,
            kind_pad: 16.0,
            kind_size: 22.0,
            kind_slant: 0.2,
            gap: 12.0,
            name_size: 25.0,
            name_color: Rgba(Color::new(0xffd9d6d0)),
            type_size: 28.0,
            member_color: Rgba(Color::new(0xffa9a6a0)),
            member_indent: 36.0,
            number_size: 24.0,
            number_min_width: 56.0,
            leader_color: Rgba(Color::new(0xff3a3a40)),
            leader_min: 32.0,
            leader_dot: 2.0,
            leader_rise: 8.0,
            row_height: 48.0,
            current_row_height: 60.0,
            current_name_size: 32.0,
            group_gap: 28.0,
            group_after: 12.0,
        }
    }
}
