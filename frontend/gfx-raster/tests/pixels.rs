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
