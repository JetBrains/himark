// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The save/restore stream the widget tree speaks, folded into one
//! affine matrix and one intersected clip rect — both painters'
//! state. Everything here is in the painter's device pixels; a
//! painter that speaks another unit converts at its own edge.

use std::cell::RefCell;

/// Row-major 2D affine: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Clone, Copy)]
struct Mat {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Mat {
    const IDENTITY: Mat = Mat {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn concat(self, l: Mat) -> Mat {
        Mat {
            a: self.a * l.a + self.c * l.b,
            b: self.b * l.a + self.d * l.b,
            c: self.a * l.c + self.c * l.d,
            d: self.b * l.c + self.d * l.d,
            e: self.a * l.e + self.c * l.f + self.e,
            f: self.b * l.e + self.d * l.f + self.f,
        }
    }
}

#[derive(Clone, Copy)]
struct Frame {
    matrix: Mat,
    /// The intersected clip, in device pixels.
    clip: Option<gfx::Rect>,
}

struct State {
    current: Frame,
    stack: Vec<Frame>,
}

pub(crate) struct Transform(RefCell<State>);

impl Transform {
    pub(crate) fn new() -> Self {
        Self(RefCell::new(State {
            current: Frame {
                matrix: Mat::IDENTITY,
                clip: None,
            },
            stack: Vec::new(),
        }))
    }

    fn matrix(&self) -> Mat {
        self.0.borrow().current.matrix
    }

    fn concat(&self, local: Mat) {
        let mut state = self.0.borrow_mut();
        state.current.matrix = state.current.matrix.concat(local);
    }

    pub(crate) fn save(&self) {
        let mut state = self.0.borrow_mut();
        let current = state.current;
        state.stack.push(current);
    }

    pub(crate) fn restore(&self) {
        let mut state = self.0.borrow_mut();
        if let Some(frame) = state.stack.pop() {
            state.current = frame;
        }
    }

    pub(crate) fn translate(&self, dx: f32, dy: f32) {
        self.concat(Mat {
            e: dx,
            f: dy,
            ..Mat::IDENTITY
        });
    }

    pub(crate) fn scale(&self, sx: f32, sy: f32) {
        self.concat(Mat {
            a: sx,
            d: sy,
            ..Mat::IDENTITY
        });
    }

    pub(crate) fn skew(&self, sx: f32, sy: f32) {
        self.concat(Mat {
            b: sy,
            c: sx,
            ..Mat::IDENTITY
        });
    }

    pub(crate) fn rotate(&self, degrees: f32, pivot: Option<gfx::Point>) {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let rotation = Mat {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        };
        self.concat(match pivot {
            Some(p) => Mat {
                e: p.x,
                f: p.y,
                ..Mat::IDENTITY
            }
            .concat(rotation)
            .concat(Mat {
                e: -p.x,
                f: -p.y,
                ..Mat::IDENTITY
            }),
            None => rotation,
        });
    }

    /// A local point in device pixels.
    pub(crate) fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let m = self.matrix();
        (m.a * x + m.c * y + m.e, m.b * x + m.d * y + m.f)
    }

    /// A local rect in device pixels — exact while the matrix is
    /// axis-aligned; under rotation or skew callers go through paths.
    pub(crate) fn rect(&self, rect: gfx::Rect) -> gfx::Rect {
        let (left, top) = self.point(rect.left, rect.top);
        let (right, bottom) = self.point(rect.right, rect.bottom);
        gfx::Rect::new(left, top, right, bottom)
    }

    /// Translation and positive scale only — the fast lane where a
    /// rect stays a rect.
    pub(crate) fn axis_aligned(&self) -> bool {
        let m = self.matrix();
        m.b == 0.0 && m.c == 0.0 && m.a > 0.0 && m.d > 0.0
    }

    /// The uniform magnitude for widths, radii and font sizes.
    pub(crate) fn magnitude(&self) -> f32 {
        let m = self.matrix();
        (m.a * m.d - m.b * m.c).abs().sqrt()
    }

    /// The app clips to rects under translation only; under a rare
    /// rotated frame the transformed bounds over-approximate.
    pub(crate) fn clip_rect(&self, rect: gfx::Rect) {
        let bounds = self.rect(rect);
        let mut state = self.0.borrow_mut();
        state.current.clip = Some(match state.current.clip {
            Some(clip) => clip
                .intersect(&bounds)
                .unwrap_or(gfx::Rect::new(clip.left, clip.top, clip.left, clip.top)),
            None => bounds,
        });
    }

    /// The intersected clip in device pixels.
    pub(crate) fn clip(&self) -> Option<gfx::Rect> {
        self.0.borrow().current.clip
    }

    /// The clip back in local coordinates.
    pub(crate) fn local_clip_bounds(&self) -> Option<gfx::Rect> {
        let clip = self.clip()?;
        if !self.axis_aligned() {
            return None;
        }
        let m = self.matrix();
        Some(gfx::Rect::new(
            (clip.left - m.e) / m.a,
            (clip.top - m.f) / m.d,
            (clip.right - m.e) / m.a,
            (clip.bottom - m.f) / m.d,
        ))
    }
}
