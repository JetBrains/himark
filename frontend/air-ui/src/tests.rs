// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use imba::{
    event::{Event, EventResult, MouseButton},
    thunk_ext::ThunkExt,
    Layout, Thunk, Widget,
};
use skia_safe::{Point, Size};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn light_json() -> serde_json::Value {
    serde_json::from_str(include_str!("../../editor/assets/theme-light.json")).unwrap()
}

#[test]
fn custom_theme_controls_palette_and_typography_without_a_special_name() {
    let mut json = light_json();
    json["name"] = "custom".into();
    json["ui"]["air"]["text"] = "#ff123456".into();
    json["ui"]["air"]["typography"]["label"]["family"] = "Air JetBrains Mono".into();
    json["ui"]["air"]["typography"]["label"]["size"] = 17.0.into();
    json["ui"]["air"]["typography"]["label"]["line_height"] = 28.0.into();
    json["ui"]["air"]["primary"]["states"]["default"]["fill"] = "#ff00ff00".into();
    let mut store = Store::new();
    editor::env::Themes::set(
        &mut store,
        editor::theme::Theme::from_json(&json.to_string()).unwrap(),
    );
    let ui = UiCtx::cold();
    let style = label(&store, &ui);
    assert_eq!(style.color, Color::new(0xff123456));
    assert_eq!(style.font.typeface().family_name(), "JetBrains Mono");
    assert_eq!(style.font.size(), 17.0);
    assert_eq!(style.line_height, 28.0);
    let arena = Arena::default();
    let thunk = button(&arena, &store, &ui, ButtonRole::Primary, "", || ()).layout(
        &arena,
        Constraints {
            min: Size::default(),
            max: Size::new(100.0, 50.0),
        },
    );
    let rect = Rect::from_size(thunk.size());
    let widget = thunk.realize(&arena, rect);
    let mut surface = skia_safe::surfaces::raster_n32_premul((100, 50)).unwrap();
    widget.handle_event(
        &arena,
        &Event::Paint {
            canvas: surface.canvas(),
            focused: true,
        },
        rect,
    );
    assert_eq!(
        surface.peek_pixels().unwrap().get_color((5, 5)),
        Color::GREEN
    );

    // Switching themes must reuse fonts without retaining the old palette.
    editor::env::Themes::set(&mut store, editor::theme::Theme::embedded());
    let updated = label(&store, &ui);
    assert_eq!(
        updated.color,
        editor::theme::Theme::embedded().ui().air.text.0
    );
    assert_eq!(updated.font.typeface().family_name(), "Inter");
}

#[test]
fn text_roles_share_the_callers_font_collection_and_cached_variations() {
    let builds = Arc::new(AtomicUsize::new(0));
    let mut store = Store::new();
    let observed = builds.clone();
    store.put(editor::env::Fonts(Arc::new(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        editor::embedded_fonts::collection()
    })));
    let ui = UiCtx::cold();
    let first = label(&store, &ui);
    for _ in 0..3 {
        assert_eq!(
            first.font.typeface().unique_id(),
            label(&store, &ui).font.typeface().unique_id()
        );
        let _ = heading(&store, &ui);
        let _ = code(&store, &ui);
    }
    assert_eq!(builds.load(Ordering::SeqCst), 1);
    assert_ne!(
        first.font.typeface().unique_id(),
        first.clone().sized(19.0).font.typeface().unique_id()
    );
    editor::env::Themes::set(&mut store, editor::theme::Theme::light());
    assert_ne!(
        first.font.typeface().unique_id(),
        label(&store, &ui).font.typeface().unique_id()
    );
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}

#[test]
fn older_theme_files_keep_their_chrome_and_receive_air_defaults() {
    let mut json = light_json();
    json["ui"].as_object_mut().unwrap().remove("air");
    let theme = editor::theme::Theme::from_json(&json.to_string()).unwrap();
    assert_eq!(theme.ui().window.background.0, Color::WHITE);
    assert_eq!(
        theme.ui().air.background.0,
        editor::theme::Theme::embedded().ui().air.background.0
    );
}

#[test]
fn nested_focus_outlines_preserve_geometry_and_do_not_intercept_clicks() {
    let arena = Arena::default();
    let store = Store::new();
    let ui = UiCtx::cold();
    let layout = |state| {
        imba::Column::new(&arena)
            .child(button(&arena, &store, &ui, ButtonRole::Primary, "OK", || 7u32).state(state))
            .pad(10.0)
            .layout(
                &arena,
                Constraints {
                    min: Size::default(),
                    max: Size::new(200.0, 100.0),
                },
            )
    };
    let normal_size = layout(ControlState::Default).size();
    let focused = layout(ControlState::Focused);
    assert_eq!(focused.size(), normal_size);
    let rect = Rect::from_size(normal_size);
    let widget = focused
        .overlay_host(imba::overlay::WINDOW)
        .realize(&arena, rect);
    let mut surface = skia_safe::surfaces::raster_n32_premul((200, 100)).unwrap();
    surface.canvas().clear(Color::TRANSPARENT);
    widget.handle_event(
        &arena,
        &Event::Paint {
            canvas: surface.canvas(),
            focused: true,
        },
        rect,
    );
    // The button starts at (10, 10); its top focus ink is outside that box.
    assert!(surface.peek_pixels().unwrap().get_color((30, 8)).a() > 0);
    let press = |point| Event::MouseDown {
        point,
        button: MouseButton::Left,
        mods: Default::default(),
        count: 1,
    };
    assert!(matches!(
        widget.handle_event(&arena, &press(Point::new(30.0, 8.0)), rect),
        EventResult::Ignored
    ));
    assert!(matches!(
        widget.handle_event(&arena, &press(Point::new(30.0, 14.0)), rect),
        EventResult::Command(7)
    ));
    let disabled = layout(ControlState::Disabled)
        .overlay_host(imba::overlay::WINDOW)
        .realize(&arena, rect);
    assert!(matches!(
        disabled.handle_event(&arena, &press(Point::new(30.0, 14.0)), rect),
        EventResult::Handled
    ));
}
