// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The modal SHEET's typographic pieces, beside the chrome they
//! operate on (`SheetChrome`): the one boxed surface the design
//! keeps, the ramp/mono text styles and the key-hint footer — shared
//! by the jump sheet, the folder picker and the palette (the set the
//! peeker's sheet module kept locally before).

use editor::theme::TypeRole;
use gfx::{Rect, Size};
use imba::{
    arena::Arena,
    layout::{Insets, LayoutExt as _},
    ui::UiCtx,
};

use crate::chrome::SheetChrome;
use crate::ui::TextStyle;

/// THE one boxed surface the design keeps: a centered sheet — flat
/// fill, hairline border, soft radius, soft drop shadow. The page
/// behind dims nothing.
#[derive(Clone)]
pub struct Sheet(pub SheetChrome);

impl Sheet {
    /// The sheet's width in a window `room` wide: its own width, or
    /// the window less a gutter each side.
    pub fn width_in(&self, room: Size) -> f32 {
        self.0.width.min(room.width - 2.0 * self.0.gutter).max(1.0)
    }

    /// The tallest the sheet may grow in `room`.
    pub fn max_height_in(&self, room: Size) -> f32 {
        (room.height - self.0.top - self.0.bottom).max(1.0)
    }

    /// The sheet's rect for content `size`, centered under the top.
    pub fn rect_in(&self, room: Size, size: Size) -> Rect {
        Rect::from_xywh(
            ((room.width - size.width) * 0.5).max(0.0),
            self.0.top,
            size.width,
            size.height,
        )
    }

    /// The fill and border, under the content it decorates.
    pub fn wrap<'a, Command: 'a>(
        &self,
        inner: impl imba::thunk_ext::ThunkExt<'a, Command> + 'a,
    ) -> impl imba::Thunk<'a, Command> + 'a {
        crate::ui::Surface::bordered(self.0.fill.0, self.0.border.0)
            .radius(self.0.radius)
            .wrap(inner)
    }

    /// The soft drop shadow, painted by whatever stands BEHIND the
    /// sheet — the sheet's own box clips what it paints.
    pub fn shadow_painter(&self, sheet: Rect) -> impl Fn(&Arena, &gfx::Canvas<'_>, Rect) {
        let chrome = self.0.clone();
        move |_arena, canvas, _rect| {
            if chrome.shadow_blur <= 0.0 {
                return;
            }
            // A CSS blur radius is twice the gaussian's sigma.
            canvas.draw_blurred_round_rect(
                sheet.with_offset((0.0, chrome.shadow_dy)),
                chrome.radius,
                chrome.radius,
                chrome.shadow_blur * 0.5,
                chrome.shadow.0,
            );
        }
    }
}

/// A chrome style at a ramp rung, in `color`.
pub fn styled(ui: &UiCtx, role: &TypeRole, color: gfx::Color) -> TextStyle {
    TextStyle {
        font: crate::fonts::role_font(ui, role),
        color,
        tracking: role.tracking,
        shaper: imba::layout::TextShaper::of(ui),
    }
}

/// The mono face at `size`, in `color` — keys and shortcuts.
pub fn mono(ui: &UiCtx, size: f32, color: gfx::Color) -> TextStyle {
    TextStyle {
        font: crate::fonts::mono_font(ui, size),
        color,
        tracking: 0.0,
        shaper: imba::layout::TextShaper::of(ui),
    }
}

/// The footer's key hints — "↑↓ move   ⏎ run   esc close": each key
/// in the mono face, its word in the label's, the hints a fixed gap
/// apart on one baseline.
pub fn key_hints<'a, Command: 'a>(
    arena: &'a Arena,
    hints: &[(&str, &str)],
    key: &TextStyle,
    word: &TextStyle,
    gap: f32,
) -> imba::layout::Row<'a, Command> {
    let mut row = imba::layout::Row::new(arena);
    for (index, (keys, verb)) in hints.iter().enumerate() {
        let lead = match index {
            0 => 0.0,
            _ => gap,
        };
        row = row
            .child_by_baseline(crate::ui::text(key, *keys).pad_insets(Insets {
                left: lead,
                ..Default::default()
            }))
            .child_by_baseline(crate::ui::text(word, format!(" {verb}")));
    }
    row
}
