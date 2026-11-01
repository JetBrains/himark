// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The app's own geometry — the handful of shapes the painting
//! boundary speaks, with exactly the API the codebase was surveyed
//! to use. Mirrors skia's names where call sites grew up on them
//! (`from_xywh`, `with_offset`) so the migration is a vocabulary
//! swap, not a rewrite.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl From<(f32, f32)> for Point {
    fn from((x, y): (f32, f32)) -> Self {
        Self { x, y }
    }
}

impl std::ops::Add for Point {
    type Output = Point;
    fn add(self, other: Point) -> Point {
        Point::new(self.x + other.x, self.y + other.y)
    }
}

impl std::ops::Sub for Point {
    type Output = Point;
    fn sub(self, other: Point) -> Point {
        Point::new(self.x - other.x, self.y - other.y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

impl From<(f32, f32)> for Size {
    fn from((width, height): (f32, f32)) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ISize {
    pub width: i32,
    pub height: i32,
}

impl ISize {
    pub const fn new(width: i32, height: i32) -> Self {
        Self { width, height }
    }
}

impl From<(i32, i32)> for ISize {
    fn from((width, height): (i32, i32)) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn from_ltrb(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self::new(left, top, right, bottom)
    }

    pub fn from_xywh(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self::new(x, y, x + width, y + height)
    }

    pub fn from_wh(width: f32, height: f32) -> Self {
        Self::new(0.0, 0.0, width, height)
    }

    pub fn from_size(size: impl Into<Size>) -> Self {
        let size = size.into();
        Self::from_wh(size.width, size.height)
    }

    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    pub fn x(&self) -> f32 {
        self.left
    }

    pub fn y(&self) -> f32 {
        self.top
    }

    pub fn size(&self) -> Size {
        Size::new(self.width(), self.height())
    }

    pub fn center(&self) -> Point {
        Point::new(
            (self.left + self.right) * 0.5,
            (self.top + self.bottom) * 0.5,
        )
    }

    pub fn center_x(&self) -> f32 {
        (self.left + self.right) * 0.5
    }

    pub fn center_y(&self) -> f32 {
        (self.top + self.bottom) * 0.5
    }

    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub fn with_offset(self, delta: impl Into<Point>) -> Self {
        let delta = delta.into();
        Self::new(
            self.left + delta.x,
            self.top + delta.y,
            self.right + delta.x,
            self.bottom + delta.y,
        )
    }

    pub fn with_inset(self, inset: impl Into<Point>) -> Self {
        let inset = inset.into();
        Self::new(
            self.left + inset.x,
            self.top + inset.y,
            self.right - inset.x,
            self.bottom - inset.y,
        )
    }

    pub fn with_outset(self, outset: impl Into<Point>) -> Self {
        let outset = outset.into();
        self.with_inset((-outset.x, -outset.y))
    }

    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.left && point.x < self.right && point.y >= self.top && point.y < self.bottom
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }

    /// The intersection, when the rects meet — skia's `intersect`
    /// answered a bool and mutated; ours answers the rect.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let rect = Rect::new(
            self.left.max(other.left),
            self.top.max(other.top),
            self.right.min(other.right),
            self.bottom.min(other.bottom),
        );
        (!rect.is_empty()).then_some(rect)
    }

    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        Rect::new(
            self.left.min(other.left),
            self.top.min(other.top),
            self.right.max(other.right),
            self.bottom.max(other.bottom),
        )
    }

    /// Outward to whole pixels.
    pub fn round_out(&self) -> Rect {
        Rect::new(
            self.left.floor(),
            self.top.floor(),
            self.right.ceil(),
            self.bottom.ceil(),
        )
    }

    /// To the nearest whole pixels.
    pub fn round(&self) -> Rect {
        Rect::new(
            self.left.round(),
            self.top.round(),
            self.right.round(),
            self.bottom.round(),
        )
    }
}

/// Unpremultiplied ARGB, 8 bits a channel — skia's layout, kept so
/// every hex literal in the themes reads the same.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color(pub u32);

impl Color {
    pub const TRANSPARENT: Color = Color(0);
    pub const BLACK: Color = Color(0xFF00_0000);
    pub const WHITE: Color = Color(0xFFFF_FFFF);
    pub const RED: Color = Color(0xFFFF_0000);
    pub const GREEN: Color = Color(0xFF00_FF00);
    pub const BLUE: Color = Color(0xFF00_00FF);

    /// ARGB in one word — `Color::new(0xFF336699)`.
    pub const fn new(argb: u32) -> Self {
        Self(argb)
    }

    pub const fn from_argb(a: u8, r: u8, g: u8, b: u8) -> Self {
        Self(((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }

    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self::from_argb(0xFF, r, g, b)
    }

    pub const fn a(&self) -> u8 {
        (self.0 >> 24) as u8
    }

    pub const fn r(&self) -> u8 {
        (self.0 >> 16) as u8
    }

    pub const fn g(&self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub const fn b(&self) -> u8 {
        self.0 as u8
    }

    pub const fn with_a(&self, a: u8) -> Color {
        Color((self.0 & 0x00FF_FFFF) | ((a as u32) << 24))
    }
}

impl From<u32> for Color {
    fn from(argb: u32) -> Self {
        Self(argb)
    }
}
