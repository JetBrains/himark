use skia_safe::{Rect, Size};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangeEnd {
    Begin,
    End,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Align {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreferredPosition {
    At {
        x: RangeEnd,
        side: Side,
        align: Align,
    },

    Cover,
}

pub fn resolve(
    host: Size,
    anchor: Rect,
    desired: Size,
    min: Size,
    position: PreferredPosition,
) -> Rect {
    let width = desired.width.max(min.width).min(host.width);
    match position {
        PreferredPosition::Cover => {
            let height = desired.height.max(min.height).min(host.height);
            let x = anchor.left.min(host.width - width).max(0.0);
            let y = anchor.top.min(host.height - height).max(0.0);
            Rect::from_xywh(x, y, width, height)
        }
        PreferredPosition::At { x, side, align } => {
            let at = match x {
                RangeEnd::Begin => anchor.left,
                RangeEnd::End => anchor.right,
            };
            let x = match align {
                Align::Left => at,
                Align::Right => at - width,
            }
            .min(host.width - width)
            .max(0.0);

            let above = anchor.top.max(0.0);
            let below = (host.height - anchor.bottom).max(0.0);
            let desired_height = desired.height.max(min.height);

            let (room, top) = match side {
                Side::Top if above >= desired_height => (desired_height, true),
                Side::Top if below >= desired_height => (desired_height, false),
                Side::Bottom if below >= desired_height => (desired_height, false),
                Side::Bottom if above >= desired_height => (desired_height, true),
                _ => {
                    let (preferred, other, preferred_top) = match side {
                        Side::Top => (above, below, true),
                        Side::Bottom => (below, above, false),
                    };
                    if preferred >= other {
                        (desired_height.min(preferred).max(min.height), preferred_top)
                    } else {
                        (desired_height.min(other).max(min.height), !preferred_top)
                    }
                }
            };
            let y = if top {
                (anchor.top - room).max(0.0)
            } else {
                anchor.bottom.min(host.height - room).max(0.0)
            };
            Rect::from_xywh(x, y, width, room)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: Size = Size::new(800.0, 600.0);

    fn anchor() -> Rect {
        Rect::from_xywh(300.0, 280.0, 60.0, 20.0)
    }

    #[test]
    fn the_preferred_corner_holds_with_room() {
        let desired = Size::new(200.0, 150.0);
        let min = Size::new(50.0, 30.0);

        let below = resolve(
            HOST,
            anchor(),
            desired,
            min,
            PreferredPosition::At {
                x: RangeEnd::Begin,
                side: Side::Bottom,
                align: Align::Left,
            },
        );
        assert_eq!((below.left, below.top), (300.0, 300.0));
        assert_eq!((below.width(), below.height()), (200.0, 150.0));

        let above_right = resolve(
            HOST,
            anchor(),
            desired,
            min,
            PreferredPosition::At {
                x: RangeEnd::End,
                side: Side::Top,
                align: Align::Right,
            },
        );
        assert_eq!(
            (above_right.left, above_right.top),
            (360.0 - 200.0, 280.0 - 150.0)
        );
    }

    #[test]
    fn a_short_side_flips() {
        let low = Rect::from_xywh(300.0, 560.0, 60.0, 20.0);
        let resolved = resolve(
            HOST,
            low,
            Size::new(200.0, 150.0),
            Size::new(50.0, 30.0),
            PreferredPosition::At {
                x: RangeEnd::Begin,
                side: Side::Bottom,
                align: Align::Left,
            },
        );
        assert_eq!(resolved.bottom, 560.0, "flipped above the anchor");
        assert_eq!(resolved.height(), 150.0);
    }

    #[test]
    fn shrinks_to_the_roomier_side_never_below_min() {
        let host = Size::new(800.0, 100.0);
        let mid = Rect::from_xywh(300.0, 40.0, 60.0, 20.0);
        let resolved = resolve(
            host,
            mid,
            Size::new(200.0, 400.0),
            Size::new(50.0, 30.0),
            PreferredPosition::At {
                x: RangeEnd::Begin,
                side: Side::Bottom,
                align: Align::Left,
            },
        );
        assert_eq!(resolved.top, 60.0, "below — the roomier side");
        assert_eq!(resolved.height(), 40.0, "shrunk to the room");

        let sliver = Rect::from_xywh(300.0, 10.0, 60.0, 80.0);
        let floored = resolve(
            host,
            sliver,
            Size::new(200.0, 400.0),
            Size::new(50.0, 30.0),
            PreferredPosition::At {
                x: RangeEnd::Begin,
                side: Side::Bottom,
                align: Align::Left,
            },
        );
        assert_eq!(floored.height(), 30.0, "the floor holds");
    }

    #[test]
    fn x_clamps_into_the_host() {
        let desired = Size::new(300.0, 100.0);
        let min = Size::new(50.0, 30.0);
        let right_edge = Rect::from_xywh(700.0, 280.0, 60.0, 20.0);
        let resolved = resolve(
            HOST,
            right_edge,
            desired,
            min,
            PreferredPosition::At {
                x: RangeEnd::Begin,
                side: Side::Bottom,
                align: Align::Left,
            },
        );
        assert_eq!(resolved.right, 800.0, "pushed back inside");

        let left_edge = Rect::from_xywh(10.0, 280.0, 60.0, 20.0);
        let resolved = resolve(
            HOST,
            left_edge,
            desired,
            min,
            PreferredPosition::At {
                x: RangeEnd::End,
                side: Side::Bottom,
                align: Align::Right,
            },
        );
        assert_eq!(resolved.left, 0.0, "pinned at the left edge");
    }

    #[test]
    fn cover_overlays_the_anchor() {
        let resolved = resolve(
            HOST,
            anchor(),
            Size::new(200.0, 100.0),
            Size::new(50.0, 30.0),
            PreferredPosition::Cover,
        );
        assert_eq!((resolved.left, resolved.top), (300.0, 280.0));

        let corner = Rect::from_xywh(750.0, 580.0, 60.0, 20.0);
        let clamped = resolve(
            HOST,
            corner,
            Size::new(200.0, 100.0),
            Size::new(50.0, 30.0),
            PreferredPosition::Cover,
        );
        assert_eq!(clamped.right, 800.0);
        assert_eq!(clamped.bottom, 600.0);
    }
}
