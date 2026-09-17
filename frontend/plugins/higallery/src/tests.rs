// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use imba::event::MouseButton;
use imba::thunk_ext::ThunkExt;
use imba::{
    arena::Arena,
    event::{Event, EventResult},
    Layout, Thunk, Widget,
};
use skia_safe::{
    surfaces, AlphaType, ColorType, Data, EncodedImageFormat, Image, ImageInfo, Point,
};
use skia_safe::{Rect, Size};
use std::path::{Path, PathBuf};
use GalleryCommand as Command;

const WIDTH: f32 = 1100.0;

fn render(gallery: &Gallery) -> Image {
    let height = gallery.content_height(WIDTH).ceil();
    assert!(
        height > 500.0 && height < 3000.0,
        "unexpected gallery height: {height}"
    );
    gallery.screenshot().expect("gallery screenshot")
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
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/gallery-screenshots")
        });
    if std::env::var_os("HIMARK_SHOT").is_some() {
        write_png(&actual, &output.join(format!("{name}.png")));
    }
    if std::env::var("HIMARK_UPDATE_GALLERY").as_deref() == Ok("1") {
        write_png(&actual, &baseline);
        return;
    }
    let bytes = std::fs::read(&baseline).unwrap_or_else(|error| panic!(
        "gallery baseline {}: {error}; generate with HIMARK_UPDATE_GALLERY=1 cargo test -p higallery tests::screenshot", baseline.display()
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
            if match widget.handle_event(&arena, &mouse_down(point), viewport) {
                EventResult::Command(command) => command == target,
                EventResult::Commands(commands) => commands.contains(&target),
                _ => false,
            } {
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
    assert_eq!(gallery.view.presses, 1);
    assert!(gallery.view.checked && gallery.view.expanded);
    let mode_button = control(&gallery, Command::Mode(GalleryMode::AllStates));
    gallery.handle_event(&mouse_down(mode_button), Size::new(WIDTH, 860.0), 0.0);
    assert_eq!(gallery.mode(), GalleryMode::AllStates);
    let before = rgba(&render(&gallery));
    for command in [Command::Press, Command::Check, Command::Expand] {
        let point = control(&gallery, command);
        gallery.handle_event(&mouse_down(point), Size::new(WIDTH, 3000.0), 0.0);
    }
    assert_eq!(gallery.view.presses, 1);
    assert!(gallery.view.checked && gallery.view.expanded);
    assert_eq!(
        before,
        rgba(&render(&gallery)),
        "catalogue must stay deterministic after clicks"
    );
    let mode_button = control(&gallery, Command::Mode(GalleryMode::Interactive));
    gallery.handle_event(&mouse_down(mode_button), Size::new(WIDTH, 860.0), 0.0);
    assert_eq!(gallery.mode(), GalleryMode::Interactive);
}

#[test]
fn registered_action_opens_a_scrollable_gallery_panel() {
    use himark::{AppExt, DynamicCommand};
    let mut app = himark::Application::new(himark::AppFonts::embedded());
    let window = app.add_window();
    app.register_command(std::sync::Arc::new(OpenGallery));
    assert_eq!(OpenGallery.name(), "Open UI Gallery");
    assert!(app.perform_registered(window, OpenGallery.id()));

    let mut surface = surfaces::raster_n32_premul((1100, 700)).unwrap();
    himark::Window::draw(window, &mut app, surface.canvas());
    let gallery_scroll = |app: &himark::Application, mode: GalleryMode| {
        let mut found = None;
        app.for_each_plugin_panel(&mut |panel| {
            if let Some(gallery) = panel.as_any().downcast_ref::<GalleryPanel>() {
                assert_eq!(panel.title(app.store()), "UI Gallery");
                assert_eq!(gallery.scroll.content().mode(), mode);
                found = Some(gallery.scroll.scroll_y());
            }
        });
        found.expect("the registered command opened the plugin")
    };
    assert_eq!(gallery_scroll(&app, GalleryMode::Interactive), 0.0);
    let chrome = himark::Theme::embedded();
    let top = chrome.ui().toolbar.height
        + himark::workbench_geometry(
            1100.0,
            700.0 - chrome.ui().toolbar.height,
            &chrome.ui().window,
        )
        .top;
    let mode_button = control(
        &Gallery::new(GalleryMode::Interactive),
        Command::Mode(GalleryMode::AllStates),
    );
    himark::test_driver::click(&mut app, mode_button.x, top + mode_button.y, 1100.0, 700.0);
    assert_eq!(gallery_scroll(&app, GalleryMode::AllStates), 0.0);
    himark::test_driver::scroll(&mut app, 300.0);
    himark::Window::draw(window, &mut app, surface.canvas());
    assert!(
        gallery_scroll(&app, GalleryMode::AllStates) > 0.0,
        "workbench scrolling reaches the gallery"
    );
    if let Some(output) = std::env::var_os("HIMARK_SHOT") {
        write_png(
            &surface.image_snapshot(),
            &PathBuf::from(output).join("workbench.png"),
        );
    }
}

// The web counterpart uses the original Air SCSS and font assets, with the same
// specimen content and positions. Export to HIMARK_SHOT for cross-render review.
#[test]
fn air_reference_specimens() {
    use air_ui::checkbox::CheckboxValue;
    use air_ui::*;
    use imba::{constraints::Constraints, LayoutExt};
    let output = std::env::var_os("HIMARK_SHOT").map(PathBuf::from);
    for light in [false, true] {
        let mut store = imba::Store::new();
        store.put(himark::env::Themes(if light {
            himark::Theme::light()
        } else {
            himark::Theme::embedded()
        }));
        let ui = imba::UiCtx::cold();
        let mut surface = surfaces::raster_n32_premul((700, 740)).unwrap();
        let canvas = surface.canvas();
        canvas.clear(himark::env::Themes::of(&store).ui().air.background.0);
        let arena = Arena::default();
        let mut specimens: Vec<(f32, f32, imba::LayoutBox<'_, ()>)> = Vec::new();
        for (index, (style, sample)) in [
            (heading(&store, &ui), "Heading — a section title"),
            (label(&store, &ui), "Label — the quick brown fox"),
            (caption(&store, &ui), "Caption — secondary information"),
            (caps(&store, &ui), "CAPS — TRACKED SECTION LABEL"),
            (key_hint(&store, &ui), "Key hint — Ctrl+Shift+P"),
            (code(&store, &ui), "Code — fn main() {}"),
        ]
        .into_iter()
        .enumerate()
        {
            specimens.push((
                20.0,
                20.0 + index as f32 * 32.0,
                imba::LayoutBox::new(&arena, text(&style, sample)),
            ));
        }
        let states = [
            ControlState::Default,
            ControlState::Hovered,
            ControlState::Pressed,
            ControlState::Focused,
            ControlState::Disabled,
        ];
        for (row, (name, role)) in [
            ("Primary", ButtonRole::Primary),
            ("Secondary", ButtonRole::Secondary),
            ("Ghost", ButtonRole::Ghost),
        ]
        .into_iter()
        .enumerate()
        {
            for (col, state) in states.into_iter().enumerate() {
                specimens.push((
                    20.0 + col as f32 * 130.0,
                    240.0 + row as f32 * 60.0,
                    imba::LayoutBox::new(
                        &arena,
                        button(&arena, &store, &ui, role, name, || ()).state(state),
                    ),
                ));
            }
        }
        for (row, value) in [
            CheckboxValue::Unchecked,
            CheckboxValue::Checked,
            CheckboxValue::Indeterminate,
        ]
        .into_iter()
        .enumerate()
        {
            for (col, state) in states.into_iter().enumerate() {
                specimens.push((
                    20.0 + col as f32 * 130.0,
                    440.0 + row as f32 * 44.0,
                    imba::LayoutBox::new(
                        &arena,
                        imba::fixed(air_checkbox(&store, value, state)).map_layout(|_| ()),
                    ),
                ));
            }
        }
        let mut style = RowStyle::standard(&store, &ui);
        style.trail = key_hint(&store, &ui);
        specimens.push((
            20.0,
            600.0,
            imba::LayoutBox::new(
                &arena,
                ListRow::new(&arena, style)
                    .label("Standard")
                    .trail("Metadata")
                    .width(600.0),
            ),
        ));
        for (col, (name, surface)) in [
            (
                "Fill",
                Surface::fill(himark::env::Themes::of(&store).ui().air.surface.0),
            ),
            (
                "Bordered",
                Surface::bordered(
                    himark::env::Themes::of(&store).ui().air.surface.0,
                    himark::env::Themes::of(&store).ui().air.border.0,
                ),
            ),
            (
                "Outline",
                Surface::outline(himark::env::Themes::of(&store).ui().air.border.0),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            specimens.push((
                20.0 + col as f32 * 210.0,
                660.0,
                imba::LayoutBox::new(
                    &arena,
                    text(&label(&store, &ui), name)
                        .pad(12.0)
                        .width(190.0)
                        .backdrop(surface.painter()),
                ),
            ));
        }
        let expected = if light {
            include_str!("../tests/air-ui/light-metrics.csv")
        } else {
            include_str!("../tests/air-ui/dark-metrics.csv")
        };
        let expected: Vec<Vec<f32>> = expected
            .lines()
            .map(|line| line.split(',').map(|n| n.parse().unwrap()).collect())
            .collect();
        assert_eq!(expected.len(), specimens.len());
        let mut metrics = String::new();
        for ((x, y, specimen), expected) in specimens.into_iter().zip(expected) {
            let thunk = specimen.layout(
                &arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(660.0, 740.0),
                },
            );
            assert!(
                (thunk.size().width - expected[2]).abs() <= 1.0 / 32.0,
                "Air web width at ({x}, {y}): {:?} vs {expected:?}",
                thunk.size()
            );
            assert_eq!(
                thunk.size().height,
                expected[3],
                "Air web height at ({x}, {y})"
            );
            if expected.len() == 5 {
                assert_eq!(
                    thunk.first_baseline(),
                    Some(expected[4]),
                    "Air web baseline at ({x}, {y})"
                );
            }
            metrics.push_str(&format!(
                "{x},{y},{},{}\n",
                thunk.size().width,
                thunk.size().height
            ));
            let rect = Rect::from_size(thunk.size());
            let widget = thunk
                .overlay_host(imba::overlay::WINDOW)
                .realize(&arena, rect);
            canvas.save();
            canvas.translate((x, y));
            widget.handle_event(
                &arena,
                &Event::Paint {
                    canvas,
                    focused: true,
                },
                rect,
            );
            canvas.restore();
        }
        if let Some(output) = &output {
            write_png(
                &surface.image_snapshot(),
                &output.join(format!(
                    "air-native-{}.png",
                    if light { "light" } else { "dark" }
                )),
            );
            std::fs::write(
                output.join(format!(
                    "air-native-{}-metrics.csv",
                    if light { "light" } else { "dark" }
                )),
                metrics,
            )
            .unwrap();
        }
    }
}

#[test]
fn button_hover_press_release_and_leave_survive_view_rebuilds() {
    use air_ui::ControlState;
    let mut gallery = Gallery::new(GalleryMode::Interactive);
    let point = control(&gallery, Command::Press);
    let size = Size::new(WIDTH, 1400.0);
    gallery.handle_event(&Event::MouseMove { point }, size, 0.0);
    assert_eq!(gallery.view.button_states[0], ControlState::Hovered);
    gallery.handle_event(&mouse_down(point), size, 0.0);
    assert_eq!(gallery.view.button_states[0], ControlState::Pressed);
    gallery.handle_event(&Event::MouseUp { point }, size, 0.0);
    assert_eq!(gallery.view.button_states[0], ControlState::Hovered);
    gallery.handle_event(
        &Event::MouseMove {
            point: Point::new(0.0, 0.0),
        },
        size,
        0.0,
    );
    assert_eq!(gallery.view.button_states[0], ControlState::Default);
    assert_eq!(gallery.view.presses, 1);
}

#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "reference images use Linux Skia rasterization"
)]
fn screenshot_light_interactive() {
    screenshot(
        "light-interactive",
        &Gallery::with_theme(GalleryMode::Interactive, himark::Theme::light()),
    );
}

#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "reference images use Linux Skia rasterization"
)]
fn screenshot_light_all_states() {
    screenshot(
        "light-all-states",
        &Gallery::with_theme(GalleryMode::AllStates, himark::Theme::light()),
    );
}
