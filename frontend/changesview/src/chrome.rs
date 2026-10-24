// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::theme::{Key, Rgba, TypeRole};
use serde::Deserialize;
use skia_safe::Color;

/// The History tab under the side column's tabs (docs/Design.md, the
/// Main artboard's right column): a branch line, then the RAIL — a
/// hairline in the gutter every commit hangs a node on — and the
/// commits' titles at the column's text edge. Colors are the ink's
/// (rail = `rule_strong`, nodes ring in `ink_3`, the newest in
/// `ink_2`, the here-node fills `live`); only the open title's
/// register is its own.
impl HistoryChrome {
    pub const KEY: Key<HistoryChrome> = Key::new("history");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct HistoryChrome {
    /// The rail hairline's left edge, from the column's left.
    pub rail_x: f32,
    pub rail_width: f32,

    /// A commit node: a ring this wide, centered on the rail.
    pub node_size: f32,
    pub node_stroke: f32,
    /// The node's top, from its commit row's top.
    pub node_top: f32,

    /// A commit row's air above and below its title block.
    pub commit_air: f32,
    /// A title line's box (the mock's 20px line).
    pub commit_line: f32,
    /// The collapsed commit: one line, ellipsized, in `ink_2`.
    pub commit: TypeRole,
    /// The open commit (the newest, or one whose files landed): the
    /// title wraps, a size up and strong.
    pub open_commit: TypeRole,
    pub open_title: Rgba,

    /// "4 files · +144 −0" under an open title.
    pub meta: TypeRole,
    pub meta_gap: f32,
    /// The open commit's air before the file tree hanging under it.
    pub tree_gap: f32,

    /// The branch line over the rail: the branch strong, then the
    /// folder and the order, brief.
    pub branch: TypeRole,
    pub folder: TypeRole,
    pub header_top: f32,
    pub header_bottom: f32,
    pub header_gap: f32,
}

impl Default for HistoryChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            rail_x: 56.0,
            rail_width: 2.0,
            node_size: 18.0,
            node_stroke: 3.0,
            node_top: 24.0,
            commit_air: 14.0,
            commit_line: 40.0,
            commit: TypeRole { size: 28.0, weight: 400.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
            open_commit: TypeRole { size: 30.0, weight: 560.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
            open_title: Rgba(Color::new(0xffd9d6d0)),
            meta: TypeRole { size: 24.0, weight: 400.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
            meta_gap: 4.0,
            tree_gap: 16.0,
            branch: TypeRole { size: 30.0, weight: 640.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
            folder: TypeRole { size: 26.0, weight: 400.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
            header_top: 32.0,
            header_bottom: 20.0,
            header_gap: 16.0,
        }
    }
}
