// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::theme::{Key, Rgba, TypeRole};
use serde::Deserialize;
use skia_safe::Color;

/// The Sessions drawer: ONE recency-sorted list under caps group
/// heads, each row a three-column grid — the repo in a right-aligned
/// margin column, the title, the age — so projects interleave and
/// still read. The open session is the HERE row, a size up.
impl SessionsChrome {
    pub const KEY: Key<SessionsChrome> = Key::new("sessions");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct SessionsChrome {
    /// The content's side inset inside the drawer column.
    pub inset: f32,
    /// The air above the first group head.
    pub top_gap: f32,
    /// The air above every group head after the first.
    pub group_gap: f32,
    /// The repo column's width; the group heads indent past it.
    pub repo_width: f32,
    /// The gap between the grid's columns.
    pub column_gap: f32,
    /// A session row's line box and its symmetric air.
    pub row_line: f32,
    pub row_pad: f32,
    /// The repo and age columns' size (ink 3).
    pub meta_size: f32,
    /// A session title's ink — a shade under primary, so the here
    /// row's bright title outranks it.
    pub title_color: Rgba,
    /// The open session's title rung and its line box, with the
    /// asymmetric air that seats it (more above than below).
    pub here_title: TypeRole,
    pub here_line: f32,
    pub here_pad_top: f32,
    pub here_pad_bottom: f32,
    /// The synthetic oblique's skew — the chrome face has no italic
    /// cut ("untitled", slash commands, a host's "offline").
    pub oblique_skew: f32,
    /// The host foot: the air above its hairline, the air under it,
    /// and the air between two hosts.
    pub foot_gap: f32,
    pub foot_pad: f32,
    pub host_gap: f32,
    /// A host's name rung, its status word's size, and the gap
    /// between them.
    pub host_name: TypeRole,
    pub host_status_size: f32,
    pub host_word_gap: f32,
    /// The host error's mono size and its air above.
    pub error_size: f32,
    pub error_gap: f32,
}

impl Default for SessionsChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            inset: 40.0,
            top_gap: 40.0,
            group_gap: 32.0,
            repo_width: 124.0,
            column_gap: 20.0,
            row_line: 40.0,
            row_pad: 10.0,
            meta_size: 26.0,
            title_color: Rgba(Color::new(0xffd9d6d0)),
            here_title: TypeRole { size: 30.0, weight: 640.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
            here_line: 40.0,
            here_pad_top: 16.0,
            here_pad_bottom: 12.0,
            oblique_skew: -0.2,
            foot_gap: 44.0,
            foot_pad: 28.0,
            host_gap: 24.0,
            host_name: TypeRole { size: 34.0, weight: 520.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
            host_status_size: 26.0,
            host_word_gap: 20.0,
            error_size: 22.0,
            error_gap: 8.0,
        }
    }
}
