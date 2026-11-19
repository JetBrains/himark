// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The CPU painter's own pixels. Set `HIMARK_RASTER_PNG` to keep
//! the frame.

use gfx::Painter as _;

fn paint_of(color: gfx::Color) -> gfx::Paint {
    let mut paint = gfx::Paint::default();
    paint.set_color(color);
    paint
}

fn count(
    surface: &gfx_raster::Surface,
    rect: gfx::Rect,
    hit: impl Fn(gfx::Color) -> bool,
) -> usize {
    let mut found = 0;
    for y in rect.top as i32..rect.bottom as i32 {
        for x in rect.left as i32..rect.right as i32 {
            if hit(surface.color_at(x, y)) {
                found += 1;
            }
        }
    }
    found
}

#[test]
fn quads_clips_shapes_and_text_land_where_painted() {
    let mut surface = gfx_raster::raster(400, 200).expect("surface");
    {
        let canvas = surface.canvas();
        canvas.clear(gfx::Color::from_rgb(255, 255, 255));

        // A red square under a translation.
        canvas.save();
        canvas.translate(20.0, 20.0);
        canvas.draw_rect(
            gfx::Rect::from_xywh(0.0, 0.0, 60.0, 60.0),
            &paint_of(gfx::Color::from_rgb(255, 0, 0)),
        );
        canvas.restore();

        // A blue square clipped to its left half.
        canvas.save();
        canvas.clip_rect(gfx::Rect::from_xywh(100.0, 20.0, 30.0, 60.0), false);
        canvas.draw_rect(
            gfx::Rect::from_xywh(100.0, 20.0, 60.0, 60.0),
            &paint_of(gfx::Color::from_rgb(0, 0, 255)),
        );
        canvas.restore();

        // A green rounded square straddling a clip: the mask road.
        canvas.save();
        canvas.clip_rect(gfx::Rect::from_xywh(200.0, 20.0, 30.0, 60.0), false);
        canvas.draw_round_rect(
            gfx::Rect::from_xywh(200.0, 20.0, 60.0, 60.0),
            12.0,
            12.0,
            &paint_of(gfx::Color::from_rgb(0, 160, 0)),
        );
        canvas.restore();

        // A shadow, and black text on a baseline.
        canvas.draw_blurred_round_rect(
            gfx::Rect::from_xywh(300.0, 30.0, 60.0, 40.0),
            8.0,
            8.0,
            6.0,
            gfx::Color::from_argb(160, 0, 0, 0),
        );
        let typeface = gfx::font::default_typeface(gfx::FontStyle::normal())
            .or_else(|| gfx::font::match_family_style("Menlo", gfx::FontStyle::normal()))
            .expect("a system face");
        let font = gfx::Font::new(typeface, 40.0);
        canvas.draw_str(
            "himark",
            gfx::Point::new(20.0, 160.0),
            &font,
            &paint_of(gfx::Color::from_rgb(0, 0, 0)),
        );
    }
    if let Some(path) = std::env::var_os("HIMARK_RASTER_PNG") {
        std::fs::write(path, surface.encode_png()).expect("the frame saved");
    }

    let whole = gfx::Rect::from_wh(400.0, 200.0);
    let red = |c: gfx::Color| c.r() > 200 && c.g() < 50 && c.b() < 50;
    let blue = |c: gfx::Color| c.b() > 200 && c.r() < 50 && c.g() < 50;
    let green = |c: gfx::Color| c.g() > 120 && c.r() < 50 && c.b() < 50;
    let dark = |c: gfx::Color| c.r() < 80 && c.g() < 80 && c.b() < 80;

    assert_eq!(count(&surface, whole, red), 60 * 60);
    assert_eq!(
        count(&surface, gfx::Rect::from_xywh(20.0, 20.0, 60.0, 60.0), red),
        60 * 60
    );
    assert_eq!(count(&surface, whole, blue), 30 * 60);
    // The rounded square: ink inside the clip, none past it, and
    // its clipped-away corner stays white.
    assert!(
        count(
            &surface,
            gfx::Rect::from_xywh(200.0, 20.0, 30.0, 60.0),
            green
        ) > 1500
    );
    assert_eq!(
        count(
            &surface,
            gfx::Rect::from_xywh(230.0, 0.0, 60.0, 100.0),
            green
        ),
        0
    );
    assert_eq!(
        surface.color_at(201, 21),
        gfx::Color::from_rgb(255, 255, 255)
    );
    // The shadow darkens its middle and fades out past its reach.
    assert!(surface.color_at(330, 50).r() < 140);
    assert_eq!(
        surface.color_at(390, 100),
        gfx::Color::from_rgb(255, 255, 255)
    );
    // Ink sits on the baseline band, and none below it.
    assert!(
        count(
            &surface,
            gfx::Rect::from_xywh(20.0, 120.0, 200.0, 40.0),
            dark
        ) > 300
    );
    assert_eq!(
        count(
            &surface,
            gfx::Rect::from_xywh(0.0, 175.0, 290.0, 25.0),
            dark
        ),
        0
    );
}

#[test]
fn a_translucent_rounded_rect_is_seamless_and_round() {
    let mut surface = gfx_raster::raster(300, 120).expect("surface");
    {
        let canvas = surface.canvas();
        canvas.clear(gfx::Color::from_rgb(255, 255, 255));
        // Fractional edges and a half-transparent ink: a piece laid
        // twice, or a gap between pieces, shows as a line.
        canvas.draw_round_rect(
            gfx::Rect::new(20.3, 20.6, 270.7, 90.4),
            12.5,
            12.5,
            &paint_of(gfx::Color::from_argb(128, 0, 0, 255)),
        );
    }
    if let Some(path) = std::env::var_os("HIMARK_RASTER_PNG") {
        std::fs::write(path, surface.encode_png()).expect("the frame saved");
    }
    // Every pixel well inside the shape is the same blend.
    let inside = surface.color_at(150, 55);
    for y in 22..89 {
        for x in 34..257 {
            assert_eq!(surface.color_at(x, y), inside, "a seam at ({x}, {y})");
        }
    }
    for x in 22..269 {
        for y in 34..77 {
            assert_eq!(surface.color_at(x, y), inside, "a seam at ({x}, {y})");
        }
    }
    // The corners are cut round: the square's own corner pixel is
    // untouched, a point on the diagonal inside the arc is inked.
    let white = gfx::Color::from_rgb(255, 255, 255);
    for (x, y) in [(21, 21), (269, 21), (21, 89), (269, 89)] {
        assert_eq!(surface.color_at(x, y), white, "corner ({x}, {y}) is cut");
    }
    for (x, y) in [(27, 27), (263, 27), (27, 83), (263, 83)] {
        assert_eq!(
            surface.color_at(x, y),
            inside,
            "inside the arc at ({x}, {y})"
        );
    }
}

#[test]
fn a_translucent_pill_is_seamless() {
    let mut surface = gfx_raster::raster(300, 80).expect("surface");
    {
        let canvas = surface.canvas();
        canvas.clear(gfx::Color::from_rgb(255, 255, 255));
        // The radius is half the height: no straight side at all.
        canvas.draw_round_rect(
            gfx::Rect::new(20.3, 20.0, 270.7, 46.75),
            13.375,
            13.375,
            &paint_of(gfx::Color::from_argb(128, 0, 0, 255)),
        );
    }
    let inside = surface.color_at(150, 33);
    for x in 36..256 {
        for y in 21..46 {
            assert_eq!(surface.color_at(x, y), inside, "a seam at ({x}, {y})");
        }
    }
    let white = gfx::Color::from_rgb(255, 255, 255);
    assert_eq!(surface.color_at(21, 21), white);
    assert_eq!(surface.color_at(269, 45), white);
    assert_ne!(surface.color_at(22, 33), white, "the end's middle is inked");
}
