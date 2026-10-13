// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use skia_safe::Size;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraints {
    pub min: Size,
    pub max: Size,
}

impl Constraints {
    pub fn tight(size: Size) -> Self {
        Self {
            min: size,
            max: size,
        }
    }

    pub fn loosen(self) -> Self {
        Self {
            min: Size::default(),
            max: self.max,
        }
    }

    /// Reserve chrome out of the band before handing the rest to a
    /// child: BOTH ends shrink, so a tight parent stays tight, a floor
    /// stays a floor, and an open ceiling stays open (`inf - chrome`
    /// is still `inf`). Saturating — chrome taller than the band
    /// leaves the child nothing, never a negative.
    pub fn reserve(self, chrome: Size) -> Self {
        let shrink = |size: Size| {
            Size::new(
                (size.width - chrome.width).max(0.0),
                (size.height - chrome.height).max(0.0),
            )
        };
        Self {
            min: shrink(self.min),
            max: shrink(self.max),
        }
    }

    /// THE contract a laid widget owes its parent: the size it answers
    /// sits INSIDE the band it was handed. Both ends bite, and on
    /// different roads:
    ///
    /// - the ceiling is OPEN where a scroller measures content (the
    ///   workbench's `ScrollView` hands `f32::MAX`, the mounted stage
    ///   hands infinity). A view that echoes `max` answers infinity
    ///   there — the sizing assert's whole reason to exist.
    /// - the floor is the VIEWPORT where a scroller wants its pane
    ///   filled (`ScrollView` hands `min.height = viewport.height`).
    ///   A view that answers only its content underfills the pane.
    ///
    /// Content-sizing is therefore not "ignore the constraints", it is
    /// "measure, then constrain". Ordered `max(min).min(max)` on
    /// purpose: a degenerate band (min above max) resolves to the
    /// ceiling instead of panicking the way `f32::clamp` would.
    pub fn constrain(&self, size: Size) -> Size {
        Size::new(
            size.width.max(self.min.width).min(self.max.width),
            size.height.max(self.min.height).min(self.max.height),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_takes_chrome_from_both_ends() {
        // A tight parent stays TIGHT, one band shorter: this is the
        // whole point — reserving space may not loosen a child.
        let tight = Constraints::tight(Size::new(320.0, 600.0)).reserve(Size::new(0.0, 40.0));
        assert_eq!(tight.min, Size::new(320.0, 560.0));
        assert_eq!(tight.max, Size::new(320.0, 560.0));

        // An OPEN ceiling stays open; the floor still moves down.
        let paned = Constraints {
            min: Size::new(0.0, 600.0),
            max: Size::new(320.0, f32::INFINITY),
        }
        .reserve(Size::new(0.0, 40.0));
        assert_eq!(paned.min.height, 560.0);
        assert!(paned.max.height.is_infinite());

        // Chrome taller than the band leaves nothing, never negative.
        let crushed = Constraints::tight(Size::new(320.0, 10.0)).reserve(Size::new(0.0, 40.0));
        assert_eq!(crushed.max.height, 0.0);
        assert_eq!(crushed.min.height, 0.0);
    }

    #[test]
    fn constrain_holds_both_ends_of_the_band() {
        // The mounted road: an open ceiling, no floor — content wins.
        let mounted = Constraints {
            min: Size::new(560.0, 0.0),
            max: Size::new(560.0, f32::INFINITY),
        };
        let answered = mounted.constrain(Size::new(560.0, 920.0));
        assert_eq!(answered.height, 920.0);
        assert!(answered.height.is_finite(), "never the ceiling");

        // The workbench road: the scroller's floor is its viewport, so
        // a SHORT list still fills the pane...
        let paned = Constraints {
            min: Size::new(0.0, 600.0),
            max: Size::new(320.0, f32::MAX),
        };
        assert_eq!(paned.constrain(Size::new(320.0, 80.0)).height, 600.0);
        // ...and a TALL one keeps its height, for the scroller to pan.
        assert_eq!(paned.constrain(Size::new(320.0, 4000.0)).height, 4000.0);

        // A degenerate band resolves to the ceiling, never a panic.
        let upside_down = Constraints {
            min: Size::new(0.0, 100.0),
            max: Size::new(0.0, 10.0),
        };
        assert_eq!(upside_down.constrain(Size::new(0.0, 50.0)).height, 10.0);
    }
}
