// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The window's COLUMNS — one derivation of where the chat, the
//! main area and the side column stand (docs/ui/toolbar.md). The
//! toolbar band's sections and the body's columns both lay against
//! this, so a splitter in the band and the one under it are the
//! SAME x; the dock's slide and the chat's slide move both at once.

use skia_safe::{Rect, Size};

/// Which edge a drawer rolls in from: the LEFT one covers the chat
/// column (Sessions), the RIGHT one the side column (Contents).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DrawerSide {
    Left,
    Right,
}

/// A horizontal span: where a column starts and how wide it lays.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Span {
    pub x: f32,
    pub width: f32,
}

impl Span {
    pub fn right(&self) -> f32 {
        self.x + self.width
    }
}

/// How the chat shows: BESIDE the tree in its own column, or WHOLE —
/// over the main area, when the tree is vacant or the window is too
/// narrow for both and the chat was fronted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChatStance {
    Beside,
    Whole,
}

/// The chat column: where it LAYS (its x runs negative mid-slide —
/// the column rolls in from the window's left edge at its full
/// width) and how many pixels of it stand revealed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ChatSpan {
    pub span: Span,
    pub revealed: f32,
    pub stance: ChatStance,
}

/// The side column: where it LAYS (its right edge runs past the
/// window's mid-slide — the column rolls in from the right at its
/// full width) and how many pixels of it stand revealed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DockSpan {
    pub span: Span,
    pub revealed: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Columns {
    pub size: Size,

    /// The toolbar band's height: the body starts under it.
    pub band: f32,

    /// The chat column, while it has any width to show.
    pub chat: Option<ChatSpan>,

    /// The main area — the split tree — between the chat and the
    /// dock. Zero wide while a whole chat stands over it.
    pub main: Span,

    /// The side column, while it has any width to show.
    pub dock: Option<DockSpan>,

    /// Where the Sessions drawer stands: over the chat column, or
    /// where the column would stand while the chat hides.
    pub drawer: Span,

    /// Where the Contents drawer stands: over the side column, or
    /// where it would stand while the dock is closed — the left
    /// drawer's mirror.
    pub right_drawer: Span,
}

impl Columns {
    pub fn body_top(&self) -> f32 {
        self.band
    }

    pub fn body_height(&self) -> f32 {
        (self.size.height - self.band).max(1.0)
    }

    /// The chat's revealed pixels from the left edge.
    pub fn chat_revealed(&self) -> f32 {
        self.chat.map_or(0.0, |chat| chat.revealed)
    }

    /// The dock's revealed pixels from the right edge.
    pub fn dock_revealed(&self) -> f32 {
        self.dock.map_or(0.0, |dock| dock.revealed)
    }

    /// A drawer's span, by the edge it rolls in from.
    pub fn drawer_span(&self, side: DrawerSide) -> Span {
        match side {
            DrawerSide::Left => self.drawer,
            DrawerSide::Right => self.right_drawer,
        }
    }

    /// The body rect of a span: under the band, to the floor.
    pub fn body_rect(&self, span: Span) -> Rect {
        Rect::from_xywh(span.x, self.band, span.width, self.body_height())
    }
}
