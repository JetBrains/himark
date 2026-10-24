// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::theme::{Key, Rgba, TypeRole};
use serde::Deserialize;
use skia_safe::Color;

/// The Files tab under the side column's tabs (docs/Design.md, the
/// Files artboard): the tree itself, its root rows the session's
/// folders — no head restates them. The text edge and the trailing
/// inset are the window's side-column metrics.
impl FilesChrome {
    pub const KEY: Key<FilesChrome> = Key::new("files");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct FilesChrome {
    /// The hairline under a bare field (the inline rename).
    pub field_rule: Rgba,
    /// The air between the tabs' rule and the tree's first row.
    pub tree_top: f32,
    /// The current row's name: the row size in the title weight.
    pub current: TypeRole,
}

impl Default for FilesChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            field_rule: Rgba(Color::new(0xff2a2a2f)),
            tree_top: 16.0,
            current: TypeRole { size: 28.0, weight: 640.0, width: 100.0, tracking: 0.0, line_height: 1.0 },
        }
    }
}
