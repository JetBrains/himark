// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The scene painter, rendered offscreen by gpui's Metal renderer:
//! no gpui app, no window. Set `HIMARK_SCENE_PNG` to keep the frame.

#![cfg(target_os = "macos")]

use std::sync::Arc;

use gfx::Painter as _;
use gfx_gpui::gpui::{size, DevicePixels, Scene};
use gfx_gpui::{ScenePainter, SceneText};
use gpui_apple::metal_renderer::MetalRenderer;
use gpui_apple::AppleTextSystem;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 200;

fn paint_of(color: gfx::Color) -> gfx::Paint {
    let mut paint = gfx::Paint::default();
    paint.set_color(color);
    paint
}

fn render(paint: impl FnOnce(&ScenePainter<'_>)) -> image::RgbaImage {
    let mut renderer = MetalRenderer::new_headless(Default::default()).expect("a Metal device");
    let text = SceneText::new(Arc::new(AppleTextSystem::new()));
    let atlas = renderer.sprite_atlas().clone();
    let mut scene = Scene::default();
    {
        let painter = ScenePainter::new(
            &mut scene,
            &*atlas,
            &text,
            gfx::Size::new(WIDTH as f32, HEIGHT as f32),
            2.0,
        );
        paint(&painter);
    }
    scene.finish();
    let frame = renderer
        .render_scene_to_image(
            &scene,
            size(DevicePixels(WIDTH as i32), DevicePixels(HEIGHT as i32)),
        )
        .expect("a rendered frame");
    if let Some(path) = std::env::var_os("HIMARK_SCENE_PNG") {
        frame.save(path).expect("the frame saved");
    }
    frame
}

/// Pixels that are mostly red, green or blue inside `rect`.
fn count(frame: &image::RgbaImage, rect: gfx::Rect, hit: impl Fn([u8; 4]) -> bool) -> usize {
    let mut found = 0;
    for y in rect.top as u32..rect.bottom as u32 {
        for x in rect.left as u32..rect.right as u32 {
            if hit(frame.get_pixel(x, y).0) {
                found += 1;
            }
        }
    }
    found
}

#[test]
fn quads_clips_and_text_land_where_painted() {
    let frame = render(|canvas| {
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

        // Black text on a baseline.
        let typeface = gfx::font::match_family_style("Menlo", gfx::FontStyle::normal())
            .expect("Menlo ships with macOS");
        let font = gfx::Font::new(typeface, 40.0);
        canvas.draw_str(
            "himark",
            gfx::Point::new(20.0, 160.0),
            &font,
            &paint_of(gfx::Color::from_rgb(0, 0, 0)),
        );
    });

    let red = |p: [u8; 4]| p[0] > 200 && p[1] < 50 && p[2] < 50;
    let blue = |p: [u8; 4]| p[2] > 200 && p[0] < 50 && p[1] < 50;
    let dark = |p: [u8; 4]| p[0] < 80 && p[1] < 80 && p[2] < 80;

    // The translated square, whole and nowhere else.
    assert_eq!(
        count(&frame, gfx::Rect::from_xywh(20.0, 20.0, 60.0, 60.0), red),
        60 * 60
    );
    assert_eq!(
        count(&frame, gfx::Rect::from_wh(WIDTH as f32, HEIGHT as f32), red),
        60 * 60
    );
    // The clip kept the left half only.
    assert_eq!(
        count(
            &frame,
            gfx::Rect::from_wh(WIDTH as f32, HEIGHT as f32),
            blue
        ),
        30 * 60
    );
    // Ink sits on the baseline band, and none below it.
    assert!(count(&frame, gfx::Rect::from_xywh(20.0, 120.0, 200.0, 40.0), dark) > 300);
    assert_eq!(
        count(&frame, gfx::Rect::from_xywh(0.0, 172.0, 400.0, 28.0), dark),
        0
    );
}
