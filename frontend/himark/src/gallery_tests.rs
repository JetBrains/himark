// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use imba::event::MouseButton;
use skia_safe::{
    surfaces, AlphaType, ColorType, Data, EncodedImageFormat, Image, ImageInfo, Point,
};
use std::path::{Path, PathBuf};

const WIDTH: f32 = 1100.0;

fn render(gallery: &Gallery) -> Image {
    let height = gallery.content_height(WIDTH).ceil();
    assert!(
        height > 900.0 && height < 3000.0,
        "unexpected gallery height: {height}"
    );
    let mut surface = surfaces::raster_n32_premul((WIDTH as i32, height as i32)).unwrap();
    gallery.draw(surface.canvas(), Size::new(WIDTH, height), 0.0);
    surface.image_snapshot()
}

fn write_png(image: &Image, path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let png = image.encode(None, EncodedImageFormat::PNG, None).unwrap();
    std::fs::write(path, png.as_bytes()).unwrap();
}

fn rgba(image: &Image) -> Vec<u8> {
    let info = ImageInfo::new(
        image.dimensions(),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let mut pixels = vec![0u8; image.width() as usize * image.height() as usize * 4];
    assert!(image.read_pixels(
        &info,
        &mut pixels,
        image.width() as usize * 4,
        (0, 0),
        skia_safe::image::CachingHint::Allow
    ));
    pixels
}

fn screenshot(name: &str, gallery: &Gallery) {
    let actual = render(gallery);
    let baseline = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/gallery")
        .join(format!("{name}.png"));
    let output = std::env::var_os("HIMARK_SHOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/gallery-screenshots")
        });
    if std::env::var_os("HIMARK_SHOT").is_some() {
        write_png(&actual, &output.join(format!("{name}.png")));
    }
    if std::env::var("HIMARK_UPDATE_GALLERY").as_deref() == Ok("1") {
        write_png(&actual, &baseline);
        return;
    }
    let bytes = std::fs::read(&baseline).unwrap_or_else(|error| panic!(
        "gallery baseline {}: {error}; generate with HIMARK_UPDATE_GALLERY=1 cargo test -p himark gallery::tests::screenshot", baseline.display()
    ));
    let expected = Image::from_encoded(Data::new_copy(&bytes)).expect("valid baseline PNG");
    // Keep the actual image on any failure, including changed layout dimensions.
    if expected.dimensions() != actual.dimensions() {
        write_png(&actual, &output.join(format!("{name}.actual.png")));
        panic!(
            "{name}: gallery dimensions changed from {:?} to {:?}; actual in {}",
            expected.dimensions(),
            actual.dimensions(),
            output.display()
        );
    }
    let actual_pixels = rgba(&actual);
    let expected_pixels = rgba(&expected);
    let mut changed = 0usize;
    let mut diff = vec![0u8; actual_pixels.len()];
    for ((a, e), d) in actual_pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected_pixels.as_chunks::<4>().0.iter())
        .zip(diff.as_chunks_mut::<4>().0.iter_mut())
    {
        let different = a.iter().zip(e).any(|(a, e)| a.abs_diff(*e) > 8);
        changed += usize::from(different);
        d.copy_from_slice(if different {
            &[255, 0, 100, 255]
        } else {
            &[0, 0, 0, 255]
        });
    }
    // Allow small per-channel rounding differences, but do not ignore small
    // regions: a changed checkbox must fail just like a changed section.
    let fraction = changed as f64 / (actual_pixels.len() / 4) as f64;
    if changed > 0 {
        write_png(&actual, &output.join(format!("{name}.actual.png")));
        let info = ImageInfo::new(
            actual.dimensions(),
            ColorType::RGBA8888,
            AlphaType::Unpremul,
            None,
        );
        let mut surface =
            surfaces::wrap_pixels(&info, &mut diff, actual.width() as usize * 4, None).unwrap();
        write_png(
            &surface.image_snapshot(),
            &output.join(format!("{name}.diff.png")),
        );
        panic!(
            "{name}: {changed} pixels ({:.3}%) differ; actual and diff in {}",
            fraction * 100.0,
            output.display()
        );
    }
}

#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "reference images use Linux Skia rasterization"
)]
fn screenshot_interactive() {
    screenshot("interactive", &Gallery::new(GalleryMode::Interactive));
}

#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "reference images use Linux Skia rasterization"
)]
fn screenshot_all_states() {
    screenshot("all-states", &Gallery::new(GalleryMode::AllStates));
}

fn mouse_down(point: Point) -> Event<'static> {
    Event::MouseDown {
        point,
        button: MouseButton::Left,
        mods: Default::default(),
        count: 1,
    }
}

// Locate a control through its real hit testing, avoiding dependence on font
// metrics or hardcoded layout coordinates in the interaction regression test.
fn control(gallery: &Gallery, target: Command) -> Point {
    let arena = Arena::default();
    let height = gallery.content_height(WIDTH);
    let viewport = Rect::from_xywh(0.0, 0.0, WIDTH, height);
    let widget = gallery
        .content(&arena)
        .layout(&arena, Gallery::constraints(WIDTH))
        .realize(&arena, viewport);
    for y in (0..height as usize).step_by(8) {
        for x in (0..WIDTH as usize).step_by(8) {
            let point = Point::new(x as f32, y as f32);
            if matches!(widget.handle_event(&arena, &mouse_down(point), viewport), EventResult::Command(command) if command == target)
            {
                return point;
            }
        }
    }
    panic!("missing gallery control: {target:?}");
}

#[test]
fn interactive_controls_and_static_catalogue() {
    let mut gallery = Gallery::new(GalleryMode::Interactive);
    for command in [Command::Press, Command::Check, Command::Expand] {
        let point = control(&gallery, command);
        // Exercise the same viewport translation as scrolling in the desktop shell.
        let scroll = (point.y - 100.0).max(0.0);
        gallery.handle_event(
            &mouse_down(Point::new(point.x, point.y - scroll)),
            Size::new(WIDTH, 400.0),
            scroll,
        );
    }
    assert_eq!(gallery.presses, 1);
    assert!(gallery.checked && gallery.expanded);
    let mode_button = control(&gallery, Command::Mode(GalleryMode::AllStates));
    gallery.handle_event(&mouse_down(mode_button), Size::new(WIDTH, 860.0), 0.0);
    assert_eq!(gallery.mode(), GalleryMode::AllStates);
    let before = rgba(&render(&gallery));
    for command in [Command::Press, Command::Check, Command::Expand] {
        let point = control(&gallery, command);
        gallery.handle_event(&mouse_down(point), Size::new(WIDTH, 3000.0), 0.0);
    }
    assert_eq!(gallery.presses, 1);
    assert!(gallery.checked && gallery.expanded);
    assert_eq!(
        before,
        rgba(&render(&gallery)),
        "catalogue must stay deterministic after clicks"
    );
    let mode_button = control(&gallery, Command::Mode(GalleryMode::Interactive));
    gallery.handle_event(&mouse_down(mode_button), Size::new(WIDTH, 860.0), 0.0);
    assert_eq!(gallery.mode(), GalleryMode::Interactive);
}
