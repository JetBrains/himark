// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The path the chrome actually draws: chevrons, ticks, arrows,
//! one squiggle — a verb list, not a path MODEL. Every shape the
//! codebase builds spells move/line/quad/close (and cubic for a
//! backend that lowers quads itself), so that is the whole
//! vocabulary; a backend walks the verbs and speaks its own
//! geometry. Fill rules, conics, booleans, measure — skia keeps
//! those, behind its own painter.

use crate::Point;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verb {
    Move(Point),
    Line(Point),
    /// Control, then end.
    Quad(Point, Point),
    /// Two controls, then end.
    Cubic(Point, Point, Point),
    Close,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    verbs: Vec<Verb>,
}

impl Path {
    pub fn verbs(&self) -> &[Verb] {
        &self.verbs
    }

    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct PathBuilder {
    verbs: Vec<Verb>,
}

impl PathBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn move_to(&mut self, to: impl Into<Point>) -> &mut Self {
        self.verbs.push(Verb::Move(to.into()));
        self
    }

    pub fn line_to(&mut self, to: impl Into<Point>) -> &mut Self {
        self.verbs.push(Verb::Line(to.into()));
        self
    }

    pub fn quad_to(&mut self, control: impl Into<Point>, to: impl Into<Point>) -> &mut Self {
        self.verbs.push(Verb::Quad(control.into(), to.into()));
        self
    }

    pub fn cubic_to(
        &mut self,
        first: impl Into<Point>,
        second: impl Into<Point>,
        to: impl Into<Point>,
    ) -> &mut Self {
        self.verbs
            .push(Verb::Cubic(first.into(), second.into(), to.into()));
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.verbs.push(Verb::Close);
        self
    }

    /// The built path, leaving the builder empty — skia's name,
    /// kept so the call sites read the same over either backend.
    pub fn detach(&mut self) -> Path {
        Path {
            verbs: std::mem::take(&mut self.verbs),
        }
    }

    /// The path so far, builder intact.
    pub fn snapshot(&self) -> Path {
        Path {
            verbs: self.verbs.clone(),
        }
    }
}
