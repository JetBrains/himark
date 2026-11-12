// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use gfx::Color;
use imba::theme::{Key, Rgba, TypeRole};
use serde::Deserialize;

/// The Sessions drawer (the Sessions mock): each HOST is a heading
/// with its own "New session" word; under it the sessions that are
/// live right now or open in this client, each a mark in the gutter,
/// a title with its age, and the folders it works in; then the
/// "Older · N" fold over the rest. Hairlines separate hosts;
/// whitespace groups everything else.
impl SessionsChrome {
    pub const KEY: Key<SessionsChrome> = Key::new("sessions");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct SessionsChrome {
    /// The content's side inset inside the drawer column.
    pub inset: f32,

    /// A host heading: its air above and below, the air above the
    /// hairline that parts it from the host before.
    pub host_top: f32,
    pub host_bottom: f32,
    pub host_rule_gap: f32,
    /// The host's name rung, its kind word's size (the OS, or its
    /// state), and the gap between the heading's words.
    pub host_name: TypeRole,
    pub host_kind_size: f32,
    pub host_word_gap: f32,
    /// The heading's trailing action ("New session", "Reconnect")
    /// and the mono key after it.
    pub host_action_size: f32,
    pub host_key_size: f32,
    pub host_key_gap: f32,
    /// The host error's mono size, its extra left inset, its air
    /// below.
    pub error_size: f32,
    pub error_inset: f32,
    pub error_gap: f32,

    /// A session row: the mark gutter and the gap after it, the
    /// mark's diameter and the running ring's stroke.
    pub gutter: f32,
    pub column_gap: f32,
    pub mark: f32,
    pub mark_stroke: f32,
    /// The title's size and line box, its ink (a shade under
    /// primary), the weight an unread title takes, and the row's
    /// symmetric air.
    pub title_size: f32,
    pub row_line: f32,
    pub row_pad: f32,
    pub title_color: Rgba,
    pub attention_weight: f32,
    /// The folders line under the title, and the age column.
    pub roots_size: f32,
    pub roots_line: f32,
    pub meta_size: f32,
    /// An OLDER row: a size down, a quieter ink, no folders line.
    pub old_title_size: f32,
    pub old_color: Rgba,

    /// The "Older · N — show all" fold: its size, air, word gap, and
    /// how many older rows preview under it while folded.
    pub fold_size: f32,
    pub fold_top: f32,
    pub fold_bottom: f32,
    pub fold_gap: f32,
    pub fold_preview: usize,
    /// Under the fold the rows GROUP by the folders they work in: one
    /// indent step per level (the group's heading one in from the
    /// fold, its rows one more), and the heading's air.
    pub old_indent: f32,
    pub group_top: f32,
    pub group_bottom: f32,

    /// The tailnet status line under the header while the node is on
    /// its way, and the add-host menu's row size.
    pub status_size: f32,
    pub status_line: f32,

    /// The synthetic oblique's skew — the chrome face has no italic
    /// cut ("untitled", slash commands, a host's "offline").
    pub oblique_skew: f32,
}

impl Default for SessionsChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            inset: 40.0,
            host_top: 36.0,
            host_bottom: 12.0,
            host_rule_gap: 28.0,
            host_name: TypeRole {
                size: 32.0,
                weight: 640.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.0,
            },
            host_kind_size: 24.0,
            host_word_gap: 16.0,
            host_action_size: 26.0,
            host_key_size: 22.0,
            host_key_gap: 8.0,
            error_size: 23.0,
            error_inset: 44.0,
            error_gap: 8.0,
            gutter: 28.0,
            column_gap: 16.0,
            mark: 16.0,
            mark_stroke: 3.0,
            title_size: 30.0,
            row_line: 40.0,
            row_pad: 12.0,
            title_color: Rgba(Color::new(0xffd9d6d0)),
            attention_weight: 640.0,
            roots_size: 24.0,
            roots_line: 32.0,
            meta_size: 24.0,
            old_title_size: 28.0,
            old_color: Rgba(Color::new(0xffa9a6a0)),
            fold_size: 24.0,
            fold_top: 24.0,
            fold_bottom: 8.0,
            fold_gap: 16.0,
            fold_preview: 5,
            old_indent: 24.0,
            group_top: 16.0,
            group_bottom: 4.0,
            status_size: 26.0,
            status_line: 56.0,
            oblique_skew: -0.2,
        }
    }
}
