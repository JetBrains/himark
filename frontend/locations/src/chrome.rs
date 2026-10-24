// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::theme::{Key, Rgba, TypeRole};
use serde::Deserialize;
use skia_safe::Color;

/// The Find tab under the side column's tabs (the Find artboard): a
/// caps "FIND IN …" label, the query over a hairline, the counts
/// line, then the results tree — files strong, hits in mono with the
/// match bold and underlined LIVE, the line number dim at the right.
impl FindChrome {
    pub const KEY: Key<FindChrome> = Key::new("find");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct FindChrome {
    pub head_top: f32,
    /// The query's distance below its caps label.
    pub query_gap: f32,
    /// The query line's height — the QUERY-SMALL role's line.
    pub query_band: f32,
    /// The air between the query's line and its rule.
    pub query_pad_bottom: f32,
    pub query_rule: Rgba,
    /// The counts line's distance below the rule.
    pub options_top: f32,
    /// The counts line's words ("in", "files").
    pub options: TypeRole,
    /// The counts' own weight — the numbers lead the words.
    pub count_weight: f32,
    /// The air above the results; each group adds `group_gap`.
    pub results_top: f32,
    /// The air between top-level result groups.
    pub group_gap: f32,
    /// A file's name in the results.
    pub file: TypeRole,
    /// The hit line's mono size and ink.
    pub hit_size: f32,
    pub hit: Rgba,
    /// The match inside a hit line: this weight, the bright ink, a
    /// LIVE underline this thick, this far under the baseline.
    pub match_weight: f32,
    pub underline: f32,
    pub underline_offset: f32,
    /// The hit's line number, mono, at the row's right.
    pub line_size: f32,
}

impl Default for FindChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            head_top: 32.0,
            query_gap: 4.0,
            query_band: 72.0,
            query_pad_bottom: 20.0,
            query_rule: Rgba(Color::new(0xff2e2e33)),
            options_top: 20.0,
            options: TypeRole { size: 26.0, weight: 400.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
            count_weight: 600.0,
            results_top: 16.0,
            group_gap: 12.0,
            file: TypeRole { size: 28.0, weight: 600.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
            hit_size: 24.0,
            hit: Rgba(Color::new(0xffa9a6a0)),
            match_weight: 700.0,
            underline: 3.0,
            underline_offset: 6.0,
            line_size: 22.0,
        }
    }
}
