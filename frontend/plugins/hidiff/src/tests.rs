// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use himark::AppExt;
use himark::{AppFonts, Application};
use std::sync::{mpsc, Arc};

#[test]
fn the_diff_panel_opens_edits_and_dismantles() {
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(
            "# Shared\n\nleft body line\n\ntail\n",
            app.store(),
            ui,
            &markdown_fonts,
            &theme
        ),
        "left.md".to_owned(),
        false,
    ));
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(
            "# Shared\n\nright body line — changed\n\ntail\nappended\n",
            app.store(),
            ui,
            &markdown_fonts,
            &theme
        ),
        "right.md".to_owned(),
        false,
    ));

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for _ in 0..4 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface);

    assert!(
        app.perform_registered(app.sole_window(), "diff.open"),
        "the diff panel opens"
    );
    settle(&mut app, &mut surface);
    let mut panel_open = false;
    app.for_each_plugin_panel(&mut |panel| {
        panel_open |= panel.as_any().is::<DiffPanelView>();
    });
    assert!(panel_open, "the diff panel took the focused pane");

    himark::test_driver::click(&mut app, 800.0, 300.0, 1100.0, 800.0);
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    let right_before = document_text(&app, "right.md");
    for _ in 0..3 {
        let _ = himark::test_driver::type_text(&mut app, "é");
        settle(&mut app, &mut surface);
    }
    let right_after = document_text(&app, "right.md");
    assert_ne!(
        right_after, right_before,
        "typing into the right half writes through"
    );
    assert!(
        right_after.contains("ééé"),
        "the typed run landed intact: {right_after:?}"
    );

    himark::test_driver::click(&mut app, 150.0, 300.0, 1100.0, 800.0);
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    let left_before = document_text(&app, "left.md");
    let right_frozen = document_text(&app, "right.md");
    for _ in 0..2 {
        let _ = himark::test_driver::type_text(&mut app, "ø");
        settle(&mut app, &mut surface);
    }
    assert_ne!(
        document_text(&app, "left.md"),
        left_before,
        "typing lands in the left half after clicking it"
    );
    assert_eq!(
        document_text(&app, "right.md"),
        right_frozen,
        "and the right half stops receiving keys"
    );

    assert!(
        app.perform_registered(app.sole_window(), "workbench.close-pane") || app.pane_count() == 1
    );
    settle(&mut app, &mut surface);
}

#[test]
fn the_optimizer_landing_cancels_matching_edits() {
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    for name in ["left.md", "right.md"] {
        assert!(app.add_document(
            app.sole_window(),
            himarkdown::document_from_markdown(
                "# Shared\n\nbody line\n\ntail\n",
                app.store(),
                ui,
                &markdown_fonts,
                &theme
            ),
            name.to_owned(),
            false,
        ));
    }

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for _ in 0..6 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface);
    assert!(
        app.perform_registered(app.sole_window(), "diff.open"),
        "the diff panel opens"
    );
    settle(&mut app, &mut surface);

    himark::test_driver::click(&mut app, 800.0, 300.0, 1100.0, 800.0);
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    for _ in 0..4 {
        let _ = himark::test_driver::type_text(&mut app, "x");
        settle(&mut app, &mut surface);
    }
    himark::test_driver::click(&mut app, 150.0, 300.0, 1100.0, 800.0);
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    for _ in 0..4 {
        let _ = himark::test_driver::type_text(&mut app, "x");
        settle(&mut app, &mut surface);
    }

    for _ in 0..6 {
        settle(&mut app, &mut surface);
    }

    let mut checked = false;
    let left_text = document_text(&app, "left.md");
    let right_text = document_text(&app, "right.md");
    assert_eq!(left_text, right_text, "both halves typed the same run");
    app.for_each_plugin_panel(&mut |panel| {
        let Some(panel) = panel.as_any().downcast_ref::<DiffPanelView>() else {
            return;
        };
        let state = panel.diff_state(app.store()).expect("the pair has settled");
        let fragments: Vec<_> = {
            let info = himark::OpenDocuments::list(app.store())
                .into_iter()
                .find(|(_, info)| info.name() == "left.md")
                .expect("left open");
            let document =
                himark::OpenDocuments::document_ref(app.store(), info.0).expect("document");
            editor::diff::fragments_from(state.diff(), document.text(), 0).collect()
        };
        assert!(
            fragments.is_empty(),
            "identical documents keep no washed fragments after the landing: {fragments:?}"
        );
        checked = true;
    });
    assert!(checked, "the diff panel was inspected");
}

#[test]
fn identical_documents_settle_spacer_free() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    let body = "# Torture Sample \u{1F680}\n\nThis file is intentionally more varied than the design notes. It mixes ordinary\nprose, emoji, inline `code`, **strong text**, _emphasis_, ~~deleted text~~, [links](https://example.com), and long lines that\nshould soft-wrap cleanly.\n\n## Inline Texture\n\n1. First ordered item\n2. Second ordered item with `inline_code()` and **bold** content.\n3. Third ordered item with a link: [tree-sitter markdown](https://example.com).\n\n- [x] Parse a tree-sitter tree\n- [ ] Build document elements\n\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\nA closing paragraph long enough to wrap a few times at narrow widths, deliberately plain but not short at all.\n";
    for name in ["one.md", "two.md"] {
        assert!(app.add_document(
            app.sole_window(),
            himarkdown::document_from_markdown(body, &store, ui, &markdown_fonts, &theme),
            name.to_owned(),
            false,
        ));
    }

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface, rounds: usize| {
        for _ in 0..rounds {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface, 6);
    assert!(
        app.perform_registered(app.sole_window(), "diff.open"),
        "the diff panel opens"
    );
    settle(&mut app, &mut surface, 30);

    let mut checked = false;
    app.for_each_plugin_panel(&mut |panel| {
        let Some(panel) = panel.as_any().downcast_ref::<DiffPanelView>() else {
            return;
        };
        let (left, right) = panel.halves(app.store());
        for (label, entity) in [("left", left), ("right", right)] {
            let document = himark::OpenDocuments::document_ref(app.store(), entity.document())
                .expect("document");
            let spacers = document.element_spacers(entity.editor());
            let ranges = document.element_byte_ranges(entity.editor());
            let nonzero: Vec<_> = ranges
                .iter()
                .zip(&spacers)
                .filter(|(_, s)| **s > 0.5)
                .collect();
            assert!(
                nonzero.is_empty(),
                "{label}: identical documents must carry no spacers: {nonzero:?}"
            );
        }
        checked = true;
    });
    assert!(checked, "the diff panel was inspected");
}

#[test]
fn every_keystroke_and_landing_keeps_the_pair_aligned() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    let body = "# Speculative Sample\n\nThe first paragraph wraps a couple of times at the pane width so its heights are not trivial at all.\n\n- [x] first task\n- [ ] second task\n\n```rust\nfn tick(x: f32) -> f32 {\n    x + 1.0\n}\n```\n\nA closing paragraph, again long enough to wrap once or twice at the half width.\n";
    for name in ["left.md", "right.md"] {
        let (mut document, blocks) =
            himarkdown::markdown_document(body, &store, ui, &markdown_fonts, &theme);
        demo::add_badges(&mut document, &blocks, &store, ui, &markdown_fonts, &theme);
        assert!(app.add_document(app.sole_window(), document, name.to_owned(), false,));
    }

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface, rounds: usize| {
        for _ in 0..rounds {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface, 8);
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    settle(&mut app, &mut surface, 30);
    assert_pair_aligned(&app);

    himark::test_driver::click(&mut app, 800.0, 300.0, 1100.0, 800.0);
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    assert_pair_aligned(&app);

    for (step, text) in ["a", "\n", "b", "\n", "typed line\n", "c"]
        .iter()
        .enumerate()
    {
        let _ = himark::test_driver::type_text(&mut app, text);
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
        assert_pair_aligned(&app);

        let mut landed = 0;
        loop {
            runner.run();
            let Ok(command) = arriving.try_recv() else {
                break;
            };
            landed += 1;
            app.perform_batch(vec![command]);
            let _ =
                himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
            assert_pair_aligned(&app);
            if landed > 200 {
                panic!("step {step}: landings never quiesce");
            }
        }
    }
    settle(&mut app, &mut surface, 10);
    assert_pair_aligned(&app);
}

#[test]
fn typed_insertions_paint_washes() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    let body = "# Wash Probe\n\nA plain paragraph that wraps at the half width without any code in it.\n\nAnother plain paragraph to give the pane some body to align against.\n";
    for name in ["left.md", "right.md"] {
        assert!(app.add_document(
            app.sole_window(),
            himarkdown::document_from_markdown(body, &store, ui, &markdown_fonts, &theme),
            name.to_owned(),
            false,
        ));
    }

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface, rounds: usize| {
        for _ in 0..rounds {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };

    let wash_pixels = |surface: &mut skia_safe::Surface| -> (usize, usize) {
        let info = skia_safe::ImageInfo::new(
            (1100, 800),
            skia_safe::ColorType::RGBA8888,
            skia_safe::AlphaType::Unpremul,
            None,
        );
        let mut pixels = vec![0u8; 1100 * 800 * 4];
        assert!(surface.read_pixels(&info, &mut pixels, 1100 * 4, (0, 0)));
        let (mut added, mut deleted) = (0usize, 0usize);
        for pixel in pixels.chunks_exact(4) {
            let (r, g, b) = (pixel[0] as i32, pixel[1] as i32, pixel[2] as i32);
            if g > r + 12 && g > b + 6 && g < 120 {
                added += 1;
            }
            if r > g + 12 && r > b + 6 && r < 130 {
                deleted += 1;
            }
        }
        (added, deleted)
    };

    settle(&mut app, &mut surface, 8);
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    settle(&mut app, &mut surface, 30);
    assert_eq!(
        wash_pixels(&mut surface),
        (0, 0),
        "identical documents paint no wash"
    );

    himark::test_driver::click(&mut app, 800.0, 300.0, 1100.0, 800.0);
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);

    let _ = himark::test_driver::type_text(&mut app, "a freshly added right-side line\n");
    settle(&mut app, &mut surface, 3);
    let (added_now, _) = wash_pixels(&mut surface);
    assert!(
        added_now > 50,
        "the added wash paints with the derivation landing ({added_now} pixels)"
    );
    settle(&mut app, &mut surface, 30);
    let (added, _) = wash_pixels(&mut surface);
    assert!(
        added > 50,
        "the added wash survives the landings ({added} pixels)"
    );

    himark::test_driver::click(&mut app, 150.0, 300.0, 1100.0, 800.0);
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    let _ = himark::test_driver::type_text(&mut app, "a left-only line typed here\n");
    settle(&mut app, &mut surface, 3);
    let (_, deleted_now) = wash_pixels(&mut surface);
    assert!(
        deleted_now > 50,
        "the deleted wash paints with the derivation landing ({deleted_now} pixels)"
    );
    settle(&mut app, &mut surface, 30);
    assert_pair_aligned(&app);
}

#[test]
fn theme_toggle_keeps_the_diff_pane_aligned_and_converges() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    let left_body = "# Torture \u{1F680}\n\nThis paragraph wraps a couple of times at the half width so heights are not trivial.\n\n- [x] Parse a tree-sitter tree\n- [ ] Paint real inline spans\n\n> A quote long enough to wrap once at the pane width, with **bold** and `code`.\n\n### Code Fence: Rust\n\n```rust\nfn tick(x: f32) -> f32 {\n    x + 1.0\n}\n```\n\nA closing paragraph, long enough to wrap at the half width as well.\n";
    let right_body = left_body
        .replace(
            "This paragraph wraps a couple of times",
            "This paragraph now wraps a few more times than it used to",
        )
        .replace("- [ ] Paint real inline spans\n", "");
    for (name, body) in [("left.md", left_body), ("right.md", right_body.as_str())] {
        let (mut document, blocks) =
            himarkdown::markdown_document(body, &store, ui, &markdown_fonts, &theme);
        demo::add_badges(&mut document, &blocks, &store, ui, &markdown_fonts, &theme);
        assert!(app.add_document(app.sole_window(), document, name.to_string(), false,));
    }

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface, rounds: usize| {
        for _ in 0..rounds {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface, 10);
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    settle(&mut app, &mut surface, 30);
    assert_pair_aligned(&app);

    assert!(app.perform_registered(app.sole_window(), "theme.toggle"));
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    assert_pair_aligned(&app);
    let mut landed = 0;
    loop {
        runner.run();
        let Ok(command) = arriving.try_recv() else {
            runner.run();
            match arriving.try_recv() {
                Ok(command) => {
                    landed += 1;
                    app.perform_batch(vec![command]);
                    continue;
                }
                Err(_) => break,
            }
        };
        landed += 1;
        assert!(
            landed < 600,
            "the post-toggle storm never quiesces (mark churn discarding repairs?)"
        );
        app.perform_batch(vec![command]);
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
        assert_pair_aligned(&app);
    }
    settle(&mut app, &mut surface, 10);
    assert_pair_aligned(&app);

    let light = himark::Theme::light();
    let mut checked = false;
    app.for_each_plugin_panel(&mut |panel| {
        let Some(panel) = panel.as_any().downcast_ref::<DiffPanelView>() else {
            return;
        };
        for entity in [panel.halves(app.store()).0, panel.halves(app.store()).1] {
            let document = himark::OpenDocuments::document_ref(app.store(), entity.document())
                .expect("document");
            let live: Vec<(u32, i64)> = document
                .element_heights(entity.editor())
                .into_iter()
                .map(|(byte, height)| (byte, height as i64))
                .collect();
            let fresh_heights: Vec<(u32, i64)> = document
                .fresh_layout_heights(entity.editor(), &store, ui, &markdown_fonts, &light)
                .into_iter()
                .map(|(byte, height)| (byte, height as i64))
                .collect();
            assert_eq!(
                live, fresh_heights,
                "the settled half must BE the fresh light layout"
            );
        }
        checked = true;
    });
    assert!(checked, "the diff panel was inspected");
}

#[test]
fn a_diff_opened_into_a_wide_window_reshapes_and_settles() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    let sample = include_str!("../../demo/sample.md");
    let left_body = sample.repeat(3);
    let right_body = left_body.replace("skia paragraph", "skia PARAGRAPH");
    for (name, body) in [
        ("left.md", left_body.as_str()),
        ("right.md", right_body.as_str()),
    ] {
        let (mut document, blocks) =
            himarkdown::markdown_document(body, &store, ui, &markdown_fonts, &theme);
        demo::add_badges(&mut document, &blocks, &store, ui, &markdown_fonts, &theme);
        assert!(app.add_document(app.sole_window(), document, name.to_string(), false));
    }

    let size = skia_safe::Size::new(2000.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((2000, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface, rounds: usize| {
        for _ in 0..rounds {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };

    let small = skia_safe::Size::new(1100.0, 800.0);
    for _ in 0..10 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let _ =
            himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), small);
    }
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    for _ in 0..60 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let _ =
            himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), small);
    }

    settle(&mut app, &mut surface, 120);

    let mut unsettled = 0;
    for _ in 0..30 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        if himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size) {
            unsettled += 1;
        }
    }
    assert_eq!(
        unsettled, 0,
        "the pane never settles after the window grows"
    );

    for _ in 0..40 {
        let _ = himark::test_driver::scroll(&mut app, 400.0);
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    }
    for step in 0..24 {
        let width = 2000.0 - (step as f32 + 1.0) * 35.0;
        let frame = skia_safe::Size::new(width, 800.0);
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let _ =
            himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), frame);
    }
    settle(&mut app, &mut surface, 120);
    let mut unsettled = 0;
    for _ in 0..30 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        if himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size) {
            unsettled += 1;
        }
    }
    assert_eq!(unsettled, 0, "the pane never settles after a live resize");

    for frame in 0..90u32 {
        let clock = imba::anim::AnimationClock::from_millis(f64::from(frame) * 16.6);
        let _ = himark::test_driver::animate(&mut app, clock);
        let width = 1160.0 + (frame as f32) * 10.0;
        let frame_size = skia_safe::Size::new(width.min(2000.0), 800.0);

        if frame % 15 == 14 {
            runner.run();
        }
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let _ = himark::Window::draw_with_size(
            app.sole_window(),
            &mut app,
            surface.canvas(),
            frame_size,
        );
    }
    settle(&mut app, &mut surface, 120);
    let mut unsettled = 0;
    for _ in 0..30 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        if himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size) {
            unsettled += 1;
        }
    }
    assert_eq!(
        unsettled, 0,
        "the pane never settles after the animated storm"
    );

    for _ in 0..30 {
        let _ = himark::test_driver::scroll(&mut app, 500.0);
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    }
    let mut checked_bands = 0;
    app.for_each_plugin_panel(&mut |panel| {
        let Some(panel) = panel.as_any().downcast_ref::<DiffPanelView>() else {
            return;
        };
        for entity in [panel.halves(app.store()).0, panel.halves(app.store()).1] {
            let document = himark::OpenDocuments::document_ref(app.store(), entity.document())
                .expect("document");
            let (_, viewport, _, _) = document.probe_state(entity.editor());
            let viewport = viewport.expect("the half reported its viewport");
            assert!(
                !document.visible_damage(entity.editor(), viewport.start, viewport.end),
                "the scrolled-into band re-shaped synchronously (no worker needed)"
            );
            checked_bands += 1;
        }
    });
    assert_eq!(checked_bands, 2, "both halves inspected");

    assert_pair_aligned(&app);

    let mut checked = false;
    app.for_each_plugin_panel(&mut |panel| {
        let Some(panel) = panel.as_any().downcast_ref::<DiffPanelView>() else {
            return;
        };
        for entity in [panel.halves(app.store()).0, panel.halves(app.store()).1] {
            let document = himark::OpenDocuments::document_ref(app.store(), entity.document())
                .expect("document");
            let width = document.layout_width(entity.editor());
            assert!(
                width > OPEN_HALF_WIDTH * 1.5,
                "the half re-laid at the pane width (got {width})"
            );
            let live: Vec<(u32, i64)> = document
                .element_heights(entity.editor())
                .into_iter()
                .map(|(byte, height)| (byte, height as i64))
                .collect();
            let fresh_heights: Vec<(u32, i64)> = document
                .fresh_layout_heights(entity.editor(), &store, ui, &markdown_fonts, &theme)
                .into_iter()
                .map(|(byte, height)| (byte, height as i64))
                .collect();
            assert_eq!(
                live, fresh_heights,
                "the settled half must BE the fresh layout at its width"
            );
        }
        checked = true;
    });
    assert!(checked, "the diff panel was inspected");
}

#[allow(dead_code)]
fn dump_pair(app: &Application) {
    app.for_each_plugin_panel(&mut |panel| {
        let Some(panel) = panel.as_any().downcast_ref::<DiffPanelView>() else {
            return;
        };
        let state = panel.diff_state(app.store()).expect("settled");
        let ops: Vec<_> = state.diff().iter().collect();
        eprintln!("[dump] diff: {ops:?}");
        let (left, right) = panel.halves(app.store());
        for (label, entity) in [("left", left), ("right", right)] {
            let document = himark::OpenDocuments::document_ref(app.store(), entity.document())
                .expect("document");
            let heights = document.element_heights(entity.editor());
            let spacers = document.element_spacers(entity.editor());
            let (pending, viewport, _, _) = document.probe_state(entity.editor());
            eprintln!("[dump] {label} pending={pending:?} viewport={viewport:?}");
            let mut y = 0f64;
            for ((byte, height), spacer) in heights.iter().zip(&spacers) {
                eprintln!("[dump]   {label} byte {byte} top {y} h {height} spacer {spacer}");
                y += ((*height + *spacer) as f64).ceil();
            }
        }
    });
}

#[test]
fn washes_follow_the_scroll_into_deep_documents() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();

    let block = "## Section\n\nA steady paragraph with enough words to wrap once at the half width, followed by another sentence for body.\n\n- item one\n- item two\n\n";
    let left_body: String = (0..500)
        .map(|index| format!("{block}paragraph number {index}\n\n"))
        .collect();

    let mut right_body = left_body.clone();
    for index in (0..500).step_by(20).rev() {
        let needle = format!("paragraph number {index}\n");
        let at = right_body.find(&needle).unwrap() + needle.len();
        right_body.insert_str(at, &format!("added right line {index}\n\n"));
    }
    let deep_at = right_body.find("added right line 480").unwrap();
    assert!(
        deep_at > 64 * 1024,
        "the deep change must sit past the head window"
    );
    for (name, body) in [("left.md", &left_body), ("right.md", &right_body)] {
        assert!(app.add_document(
            app.sole_window(),
            himarkdown::document_from_markdown(body, &store, ui, &markdown_fonts, &theme),
            name.to_string(),
            false,
        ));
    }

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface, rounds: usize| {
        for _ in 0..rounds {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface, 10);
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    settle(&mut app, &mut surface, 30);
    assert_pair_aligned(&app);

    let deep_range = {
        let start = right_body.find("added right line 480").unwrap() as u32;
        start..start + "added right line 480".len() as u32
    };
    // Line washes are THE diff markup's now — whole-document from
    // birth, no window to chase (docs/editor/scroll-stripe.md §7). The deep
    // change is washed before any scroll; the scroll still proves the
    // pair aligns all the way down.
    let right_marks = |app: &Application| -> Vec<std::ops::Range<u32>> {
        let info = himark::OpenDocuments::list(app.store())
            .into_iter()
            .find(|(_, info)| info.name() == "right.md")
            .expect("right open");
        let mut ranges = Vec::new();
        app.for_each_plugin_panel(&mut |panel| {
            let Some(panel) = panel.as_any().downcast_ref::<DiffPanelView>() else {
                return;
            };
            let Some(state) = panel.diff_state(app.store()) else {
                return;
            };
            ranges = himark::OpenDocuments::document_ref(app.store(), info.0)
                .expect("document")
                .markup_styled_ranges(state.hunk_markup_oracle());
        });
        ranges
    };
    assert!(
        right_marks(&app)
            .iter()
            .any(|range| range.start < deep_range.end && deep_range.start < range.end),
        "the deep change is washed from birth: {:?}",
        right_marks(&app)
    );

    for _ in 0..400 {
        let _ = himark::test_driver::scroll(&mut app, 2000.0);
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }
    settle(&mut app, &mut surface, 6);
    assert!(
        right_marks(&app)
            .iter()
            .any(|range| range.start < deep_range.end && deep_range.start < range.end),
        "the wash stands after the deep scroll: {:?}",
        right_marks(&app)
    );
    assert_pair_aligned(&app);
}

#[test]
fn scrolling_after_a_theme_toggle_converges() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    let block = "## Section\n\n| Feature | Status | Notes |\n| --- | --- | --- |\n| Persistent store | done | HAMT snapshots |\n| Effects | done | commands come home |\n\nThis file is intentionally more varied than the design notes. It mixes ordinary prose, emoji, inline `code`, **strong text**, and long lines that should soft-wrap cleanly.\n\n- [x] Parse a tree-sitter tree\n- [ ] Paint real inline spans\n\n";
    let left_body: String = (0..24)
        .map(|index| {
            format!(
                "{block}paragraph number {index}

"
            )
        })
        .collect();
    let right_body = left_body.replace(
        "paragraph number 11
",
        "paragraph number 11 changed on the right
",
    );
    for (name, body) in [("left.md", &left_body), ("right.md", &right_body)] {
        let (mut document, blocks) =
            himarkdown::markdown_document(body, &store, ui, &markdown_fonts, &theme);
        demo::add_badges(&mut document, &blocks, &store, ui, &markdown_fonts, &theme);
        assert!(app.add_document(app.sole_window(), document, name.to_string(), false,));
    }

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface, rounds: usize| {
        for _ in 0..rounds {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface, 10);
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    settle(&mut app, &mut surface, 30);
    assert_pair_aligned(&app);

    himark::test_driver::click(&mut app, 800.0, 300.0, 1100.0, 800.0);
    for _ in 0..3 {
        let _ = himark::test_driver::type_text(&mut app, "x");
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    }
    himark::test_driver::click(&mut app, 150.0, 500.0, 1100.0, 800.0);
    for _ in 0..2 {
        let _ = himark::test_driver::type_text(&mut app, "é");
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    }

    assert!(app.perform_registered(app.sole_window(), "theme.toggle"));

    for step in 0..40 {
        let _ = himark::test_driver::scroll(&mut app, 600.0);
        let _ = himark::test_driver::animate(
            &mut app,
            imba::anim::AnimationClock::from_millis(step as f64 * 16.0),
        );
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }
    settle(&mut app, &mut surface, 30);
    assert_pair_aligned(&app);

    let _ = himark::test_driver::scroll(&mut app, -1_000_000.0);
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    let cell_focused = |app: &Application| {
        let info = himark::OpenDocuments::list(app.store())
            .into_iter()
            .find(|(_, info)| info.name() == "right.md")
            .expect("right open")
            .0;
        {
            let document =
                himark::OpenDocuments::document_ref(app.store(), info).expect("document");
            document
                .editor_ids()
                .any(|editor| matches!(document.focus(editor), editor::EditorFocus::Inlay(_)))
        }
    };
    let mut focused = false;
    'probe: for dy in (0..=10).map(|step| step * 30) {
        for x in [620, 680, 740, 800, 860, 920, 1000] {
            let y = 60.0 + dy as f32;
            himark::test_driver::click(&mut app, x as f32, y, 1100.0, 800.0);
            let _ =
                himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
            if cell_focused(&app) {
                focused = true;
                break 'probe;
            }
        }
    }
    assert!(focused, "a table cell took focus post-toggle");
    for _ in 0..6 {
        let _ = himark::test_driver::type_text(&mut app, "grow the row after the switch ");
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    }
    settle(&mut app, &mut surface, 30);
    assert_pair_aligned(&app);

    let light = himark::Theme::light();
    let mut checked = false;
    app.for_each_plugin_panel(&mut |panel| {
        let Some(panel) = panel.as_any().downcast_ref::<DiffPanelView>() else {
            return;
        };
        for (side, entity) in [
            ("left", panel.halves(app.store()).0),
            ("right", panel.halves(app.store()).1),
        ] {
            let document = himark::OpenDocuments::document_ref(app.store(), entity.document())
                .expect("document");
            let live: Vec<(u32, i64)> = document
                .element_heights(entity.editor())
                .into_iter()
                .map(|(byte, height)| (byte, height as i64))
                .collect();
            let fresh_heights: Vec<(u32, i64)> = document
                .fresh_layout_heights(entity.editor(), &store, ui, &markdown_fonts, &light)
                .into_iter()
                .map(|(byte, height)| (byte, height as i64))
                .collect();
            assert_eq!(
                live, fresh_heights,
                "{side}: the settled half must BE the fresh light layout"
            );
        }
        checked = true;
    });
    assert!(checked, "the diff panel was inspected");
}

#[test]
fn a_repair_captured_before_a_caret_move_discards_itself() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let theme = himark::Theme::embedded();
    let fonts = himark::test_document::test_fonts_collection();

    let body: String = (0..80)
        .map(|index| {
            format!(
                "- [x] task {index} with **bold words** and `inline code` long enough to wrap at this width\n"
            )
        })
        .collect();
    let (mut document, _) = himarkdown::markdown_document(&body, &store, &ui, &fonts, &theme);
    let editor = document.add_editor(
        360.0,
        None,
        ::editor::EditorBuild::Complete,
        &[],
        &store,
        &ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let mut store = policy_store();
    let ui = himark::test_document::test_ui();
    let mut perform = |document: &mut himark::Document, command| {
        let mut batch = imba::effect::Batch::new();
        document.perform(&mut store, &ui, editor, command, &mut batch.effects());
        himark::test_support::surviving_launches(batch)
    };

    let _ = perform(
        &mut document,
        editor::EditorCommand::Viewport {
            width: 360.0,
            top: 0.0,
            bottom: 400.0,
            anchor: 0,
        },
    );
    let _ = perform(
        &mut document,
        editor::EditorCommand::Click {
            kind: editor::ClickKind::Set,
            point: skia_safe::Point::new(5.0, 50.0),
        },
    );

    let filler = "filler words to reshape and push the budget over\n\n".repeat(60);
    let effects = perform(
        &mut document,
        editor::EditorCommand::InsertText { text: filler },
    );
    let held: Vec<_> = effects
        .into_iter()
        .filter(|effect| effect.is::<editor::RepairEffect>())
        .collect();
    assert!(
        !held.is_empty(),
        "the budget-cut insert defers a repair tail"
    );

    let _ = perform(
        &mut document,
        editor::EditorCommand::Click {
            kind: editor::ClickKind::Set,
            point: skia_safe::Point::new(5.0, 300.0),
        },
    );

    let mut pending = Vec::new();
    let workshop = himark::test_support::test_workshop(theme.clone());
    for effect in held {
        let command = himark::test_support::handle_effect(effect, &workshop);
        pending.extend(perform(&mut document, command));
    }
    let mut rounds = 0;
    while let Some(effect) = pending.pop() {
        rounds += 1;
        assert!(rounds < 200, "repairs must quiesce");
        let command = himark::test_support::handle_effect(effect, &workshop);
        pending.extend(perform(&mut document, command));
    }

    let live: Vec<(u32, i64)> = document
        .element_heights(editor)
        .into_iter()
        .map(|(byte, height)| (byte, height as i64))
        .collect();
    let fresh: Vec<(u32, i64)> = document
        .fresh_layout_heights(editor, &store, &ui, &fonts, &theme)
        .into_iter()
        .map(|(byte, height)| (byte, height as i64))
        .collect();
    assert_eq!(
        live, fresh,
        "a stale-overlay landing must not survive: the layout IS the live overlay's"
    );
}

#[test]
fn typing_into_a_table_cell_keeps_the_pair_aligned() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    let body = "| Feature | Status | Notes |\n| --- | --- | --- |\n| Persistent store | done | HAMT snapshots |\n| Effects | done | commands come home |\n\nA paragraph under the table long enough to wrap at the half width once or twice.\n\nAnother paragraph so the pair has body below the table as well.\n";
    for name in ["left.md", "right.md"] {
        let (mut document, blocks) =
            himarkdown::markdown_document(body, &store, ui, &markdown_fonts, &theme);
        demo::add_badges(&mut document, &blocks, &store, ui, &markdown_fonts, &theme);
        assert!(app.add_document(app.sole_window(), document, name.to_string(), false,));
    }

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface, rounds: usize| {
        for _ in 0..rounds {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface, 10);
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    settle(&mut app, &mut surface, 30);
    assert_pair_aligned(&app);

    let mut cell_focused = false;
    'probe: for x in [760, 800, 840, 880] {
        for y in [95, 110, 125, 140] {
            himark::test_driver::click(&mut app, x as f32, y as f32, 1100.0, 800.0);
            let _ =
                himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
            let info = himark::OpenDocuments::list(app.store())
                .into_iter()
                .find(|(_, info)| info.name() == "right.md")
                .expect("right open");
            let focused = {
                let document =
                    himark::OpenDocuments::document_ref(app.store(), info.0).expect("document");
                document
                    .editor_ids()
                    .any(|editor| matches!(document.focus(editor), editor::EditorFocus::Inlay(_)))
            };
            if focused {
                cell_focused = true;
                break 'probe;
            }
        }
    }
    assert!(cell_focused, "a table cell took focus");

    let table_heights = |app: &Application, name: &str| -> (f32, f32) {
        let info = himark::OpenDocuments::list(app.store())
            .into_iter()
            .find(|(_, info)| info.name() == name)
            .expect("open");
        let document = himark::OpenDocuments::document_ref(app.store(), info.0).expect("document");
        let editor = document.editor_ids().next().expect("an editor");

        let heights = document.element_heights(editor);
        let spacers = document.element_spacers(editor);
        (heights[0].1, spacers.iter().copied().fold(0.0, f32::max))
    };
    let (right_table_before, _) = table_heights(&app, "right.md");

    for _ in 0..6 {
        let _ = himark::test_driver::type_text(&mut app, "grow the row ");
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    }
    settle(&mut app, &mut surface, 30);
    let (right_table_after, _) = table_heights(&app, "right.md");
    assert!(
        right_table_after > right_table_before + 20.0,
        "the typed cell must grow its row: {right_table_before} -> {right_table_after}"
    );
    let (_, left_spacer) = table_heights(&app, "left.md");
    assert!(
        left_spacer > 20.0,
        "the left half absorbs the growth with a spacer: max {left_spacer}"
    );
    assert_pair_aligned(&app);

    for step in 0..3 {
        let _ = himark::test_driver::type_text(&mut app, "\n");
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
        assert_pair_aligned(&app);
        let mut landed = 0;
        loop {
            runner.run();
            let Ok(command) = arriving.try_recv() else {
                break;
            };
            landed += 1;
            assert!(landed < 300, "enter {step}: landings never quiesce");
            app.perform_batch(vec![command]);
            let _ =
                himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
            assert_pair_aligned(&app);
        }
    }
    settle(&mut app, &mut surface, 10);
    assert_pair_aligned(&app);
}

pub(crate) fn assert_pair_aligned(app: &Application) {
    assert_pair_consistent(app, true);
}

pub(crate) fn assert_pair_consistent(app: &Application, expect_pairs: bool) {
    let mut checked = false;
    app.for_each_plugin_panel(&mut |panel| {
        let Some(panel) = panel.as_any().downcast_ref::<DiffPanelView>() else {
            return;
        };
        let state = panel.diff_state(app.store()).expect("the pair has settled");
        let diff = state.diff();
        let (left, right) = panel.halves(app.store());
        let sides = [left, right].map(|entity| {
            let document = himark::OpenDocuments::document_ref(app.store(), entity.document())
                .expect("document");
            let ranges = document.element_byte_ranges(entity.editor());
            let spacers = document.element_spacers(entity.editor());
            let heights: Vec<f32> = document
                .element_heights(entity.editor())
                .into_iter()
                .map(|(_, height)| height)
                .collect();

            let mut tops = std::collections::HashMap::new();
            let mut y = 0f64;
            for ((range, spacer), height) in ranges.iter().zip(&spacers).zip(&heights) {
                tops.insert(range.start, y + *spacer as f64);
                y += ((*height + *spacer) as f64).ceil();
            }
            (ranges, spacers, tops)
        });
        let (left_ranges, left_spacers, left_tops) = &sides[0];
        let (right_ranges, right_spacers, right_tops) = &sides[1];

        let mut aligned = 0;
        for range in left_ranges {
            let boundary = range.start;

            if range.is_empty() {
                continue;
            }
            let mapped = diff.transform_offset(boundary, operation::Bias::Right);
            if diff.transform_offset_back(mapped, operation::Bias::Right) != boundary {
                continue;
            }
            let (Some(left_top), Some(right_top)) =
                (left_tops.get(&boundary), right_tops.get(&mapped))
            else {
                continue;
            };
            aligned += 1;
            if *left_top as i64 != *right_top as i64 {
                for (tag, (ranges, spacers, tops)) in [("left", &sides[0]), ("right", &sides[1])] {
                    eprintln!("[pair-dump] {tag}:");
                    let heights = {
                        let entity = match tag {
                            "left" => left,
                            _ => right,
                        };
                        himark::OpenDocuments::document_ref(app.store(), entity.document())
                            .map(|document| document.element_heights(entity.editor()))
                            .unwrap_or_default()
                    };
                    for ((range, spacer), (byte, height)) in
                        ranges.iter().zip(spacers.iter()).zip(heights)
                    {
                        eprintln!(
                            "[pair-dump]   {}..{} height={height} spacer={spacer} top={:?}",
                            range.start,
                            range.end,
                            tops.get(&range.start),
                        );
                        let _ = byte;
                    }
                }
            }
            assert_eq!(
                *left_top as i64, *right_top as i64,
                "boundary {boundary}↔{mapped} left {left_top} right {right_top}"
            );
        }
        assert!(!expect_pairs || aligned > 0, "some boundaries must pair");

        for (ranges, spacers, forward) in [
            (left_ranges, left_spacers, true),
            (right_ranges, right_spacers, false),
        ] {
            for (range, spacer) in ranges.iter().zip(spacers) {
                assert!(
                    *spacer >= 0.0,
                    "negative spacer {spacer} at {} (forward {forward}) — rows would overlap",
                    range.start
                );
                if *spacer <= 0.5 {
                    continue;
                }
                let boundary = range.start;
                let round_trip = match forward {
                    true => diff.transform_offset_back(
                        diff.transform_offset(boundary, operation::Bias::Right),
                        operation::Bias::Right,
                    ),
                    false => diff.transform_offset(
                        diff.transform_offset_back(boundary, operation::Bias::Right),
                        operation::Bias::Right,
                    ),
                };
                assert_eq!(
                    round_trip, boundary,
                    "spacer {spacer} at unpaired boundary {boundary} (forward {forward})"
                );
            }
        }
        checked = true;
    });
    assert!(checked, "the diff panel was inspected");
}

#[test]
fn an_edited_markdown_pair_settles_aligned() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    let left_body = "# Himark Render Torture Sample \u{1F680}\n\nThis file is intentionally more varied than the design notes. It mixes ordinary prose, emoji, inline `code`, **strong text**, _emphasis_, ~~deleted text~~, [links](https://example.com), and long lines that should soft-wrap cleanly without making the renderer work harder than necessary.\n\n## Inline Texture\n\nThe quick brown fox edits markdown at 120 Hz while the cursor passes through Unicode: caf\u{e9}, r\u{e9}sum\u{e9}, na\u{ef}ve, and emoji clusters like \u{1F469}\u{200D}\u{1F4BB}.\n\n- [x] Parse a tree-sitter tree\n- [x] Build document elements\n- [ ] Paint real inline spans\n- [ ] Keep scrolling boringly fast\n\n> A quote should look like a quote eventually. It exercises punctuation, wrapping, and multiple inline styles with **bold claims** and `tiny identifiers`.\n\n### Code Fence: Rust\n\n```rust\nfn render_frame(viewport_y: f32) -> f32 {\n    viewport_y + 1.0\n}\n```\n\nA closing paragraph long enough to wrap a few times at narrow widths, deliberately plain but not short at all, so soft wrapping differences show up.\n";

    let right_body = left_body
        .replace(
            "This file is intentionally more varied than the design notes.",
            "This file is now deliberately far more varied than any of the design notes ever were.",
        )
        .replace(
            "- [ ] Keep scrolling boringly fast\n",
            "- [ ] Keep scrolling boringly fast\n- [ ] A brand new task added on the right\n",
        )
        .replace(
            "> A quote should look like a quote eventually. It exercises punctuation, wrapping, and multiple inline styles with **bold claims** and `tiny identifiers`.\n\n",
            "",
        )
        + "\n## Appended Section\n\nA whole section that only exists on the right side of the pair.\n";
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(&left_body, &store, ui, &markdown_fonts, &theme),
        "left.md".to_owned(),
        false,
    ));
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(&right_body, &store, ui, &markdown_fonts, &theme),
        "right.md".to_owned(),
        false,
    ));

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface, rounds: usize| {
        for _ in 0..rounds {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface, 6);
    assert!(
        app.perform_registered(app.sole_window(), "diff.open"),
        "the diff panel opens"
    );
    settle(&mut app, &mut surface, 30);
    assert_pair_aligned(&app);

    himark::test_driver::click(&mut app, 800.0, 300.0, 1100.0, 800.0);
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    for _ in 0..3 {
        let _ = himark::test_driver::type_text(&mut app, "x");
        settle(&mut app, &mut surface, 4);
    }
    settle(&mut app, &mut surface, 20);
    assert_pair_aligned(&app);
}

fn document_text(app: &Application, name: &str) -> String {
    let info = himark::OpenDocuments::list(app.store())
        .into_iter()
        .find(|(_, info)| info.name() == name)
        .expect("the document is open");
    let document = himark::OpenDocuments::document_ref(app.store(), info.0).expect("document");
    let count = document.text().byte_count();
    document.text().view().byte_string(0, count)
}

#[test]
fn dismantle_retracts_editors_and_removes_the_editorless_side() {
    let ui = himark::test_document::test_ui();
    let mut store = policy_store();
    let theme = himark::Theme::embedded();
    let fonts = himark::test_document::test_fonts_collection();
    let mut open = |body: &str, extra_editor: bool| {
        let mut document = himarkdown::document_from_markdown(body, &store, ui, &fonts, &theme);
        let pane = extra_editor.then(|| {
            document.add_editor(
                600.0,
                None,
                ::editor::EditorBuild::Bounded,
                &[],
                &store,
                ui,
                &fonts,
                &theme,
                &mut imba::effect::Batch::new().effects(),
            )
        });
        let editor = document.add_editor(
            420.0,
            None,
            ::editor::EditorBuild::Bounded,
            &[],
            &store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
        let id = himark::OpenDocuments::register(&mut store, document, None, "side".to_owned(), 0);
        (id, pane, himark::EditorIdView::new(id, editor))
    };

    let (old_doc, _, old_entity) = open("old side\n", false);
    let (new_doc, new_pane, new_entity) = open("new side\n", true);

    let diff = himark::OpenDocuments::track_diff(&mut store, old_doc, new_doc, false, None)
        .expect("both sides registered");
    let handle = himark::OpenDocuments::diff_handle(&store, diff).expect("tracked");
    let right_extras = {
        let mut document =
            himark::OpenDocuments::document(&mut store, new_doc).expect("registered");
        let id = document.add_owned_markup(new_entity.editor());
        himark::OpenDocuments::put_document(&mut store, new_doc, document);
        id
    };
    let mut panel = DiffPanelView::new(
        &mut store,
        old_entity,
        new_entity,
        handle,
        right_extras,
        None,
    );
    himark::PanelView::dismantle(&mut panel, &mut store);

    assert!(
        himark::OpenDocuments::document_ref(&store, old_doc).is_none(),
        "editorless after retraction — the pinned side leaves whole"
    );
    let new = himark::OpenDocuments::document(&store, new_doc).expect("the working copy stays");
    assert!(
        new.has_editor(new_pane.expect("the extra editor")),
        "its pane editor keeps it alive"
    );
    let _ = (old_entity, new_entity);
}

#[test]
fn located_diff_halves_offer_and_dispatch_editor_commands() {
    struct Probe(Arc<std::sync::Mutex<Vec<(String, String)>>>);
    impl himark::DynamicEditorCommand for Probe {
        fn id(&self) -> &'static str {
            "test.probe"
        }
        fn name(&self) -> String {
            "Probe".to_owned()
        }
        fn perform(
            &self,
            _store: &mut imba::store::Store,
            _ui: &imba::UiCtx,
            document: &mut himark::Document,
            _editor: himark::EditorId,
            location: &himark::ResourceLocation,
            _payload: Option<Box<dyn std::any::Any + Send + Sync>>,
            _fx: &mut imba::effect::Effects<'_, himark::EditorCommand>,
        ) {
            let end = document.text().byte_count().min(u32::MAX as usize) as u32;
            let text = document.text().view().substring(0..end);
            self.0
                .lock()
                .expect("probe log")
                .push((location.name().to_owned(), text));
        }
    }

    let mut app = Application::new(AppFonts::embedded());
    let window = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let hits: Arc<std::sync::Mutex<Vec<(String, String)>>> = Arc::default();
    app.register_editor_command(Arc::new(Probe(Arc::clone(&hits))));
    let located = |name: &str| {
        himark::ResourceLocation::new(
            himark::ResourceType::document(),
            himark::Authority::new("local"),
            vec!["project".to_owned(), name.to_owned()],
        )
    };
    for (name, body) in [("left.md", "left body\n"), ("right.md", "right body\n")] {
        assert!(app.perform_command(himark::AppCommand::Opened(
            window,
            himark::OpenedDocument {
                name: name.to_owned(),
                document: himark::test_document::plain_document(body),
                location: Some(located(name)),
                primary: false,
                target: None,
                focus: false,
            },
        )));
    }
    assert!(app.perform_registered(window, "diff.open"));

    let command = himark::palette_commands(app.store(), &app.ui_handle(), window)
        .into_iter()
        .find(|presentable| presentable.id == "test.probe")
        .expect("the diff half offers the registered command")
        .command;
    assert!(app.perform_command(command));

    let hits = hits.lock().expect("probe log");
    assert_eq!(hits.len(), 1, "one dispatch, at one half");
    assert_eq!(hits[0].0, "left.md", "the focused half's location");
    assert_eq!(hits[0].1, "left body\n", "the focused half's document");
}

fn policy_store() -> imba::store::Store {
    let mut store = imba::store::Store::new();
    // Production stores carry the edge-installed diff policy; a bare
    // store degrades to ReplaceAll and every diff becomes one hunk.
    store.put(himark::env::Differ(std::sync::Arc::new(myersdiff::Myers)));
    store
}

#[test]
fn the_panel_opens_dressed_with_no_effects_run() {
    let ui = himark::test_document::test_ui();
    let mut store = policy_store();
    let theme = himark::Theme::embedded();
    let fonts = himark::test_document::test_fonts_collection();
    let mut middle = String::new();
    for line in 0..300u32 {
        middle.push_str(&format!("line {line}: the quiet unchanged middle\n"));
    }
    let mut register = |body: &str, name: &str| {
        let document = himarkdown::document_from_markdown(body, &store, ui, &fonts, &theme);
        himark::OpenDocuments::register(&mut store, document, None, name.to_owned(), 0)
    };
    let old = register(&format!("{middle}old tail\n"), "old");
    let new = register(&format!("{middle}new tail\n"), "new");

    let panel = diff_panel(&mut store, &ui, old, new, None).expect("both registered");

    let state = panel.diff_state(&store).expect("attached at construction");
    let (left_marks, right_marks) = state.mark_markups();
    let washed = |document: himark::DocumentId, marks: himark::MarkupId| {
        himark::OpenDocuments::document_ref(&store, document)
            .and_then(|document| document.feature_markup(marks))
            .is_some_and(|markup| !markup.is_empty())
    };
    assert!(washed(old, left_marks), "the left wash is standing");
    assert!(washed(new, right_marks), "the right wash is standing");

    let (left_half, _) = panel.halves(&store);
    let height = himark::OpenDocuments::document_ref(&store, old)
        .and_then(|document| document.document_layout(left_half.editor()))
        .map(|layout| layout.height())
        .expect("the half mounted");
    assert!(
        height < 3_000.0,
        "the FIRST layout collapsed the unchanged middle behind strips \
         (got {height}px for 300+ lines)"
    );

    let generation = himark::OpenDocuments::document_ref(&store, new)
        .and_then(|document| {
            document
                .diff(state.diff_id())
                .map(|entry| entry.generation())
        })
        .expect("the entry rides the target");
    assert_eq!(generation, 1, "normalized at birth — folds keyed directly");
}

#[test]
fn a_shared_pair_ignores_a_handed_prep() {
    let ui = himark::test_document::test_ui();
    let mut store = policy_store();
    let theme = himark::Theme::embedded();
    let fonts = himark::test_document::test_fonts_collection();
    let mut register = |body: &str, name: &str| {
        let document = himarkdown::document_from_markdown(body, &store, ui, &fonts, &theme);
        himark::OpenDocuments::register(&mut store, document, None, name.to_owned(), 0)
    };
    let old = register("one\ntwo\n", "old");
    let new = register("one\nTWO\n", "new");

    himark::OpenDocuments::track_diff(&mut store, old, new, true, None).expect("tracked");

    let foreign_left = himark::Text::from_string_exact("something\nelse\n".to_owned());
    let foreign_right = himark::Text::from_string_exact("something\nELSE\n".to_owned());
    let operation = myersdiff::diff(&foreign_left, &foreign_right);
    let marks = himark::prepare_marks(&operation, &foreign_left);
    let panel = diff_panel(
        &mut store,
        &ui,
        old,
        new,
        Some(DiffPrep { operation, marks }),
    )
    .expect("the shared pair still opens");

    let state = panel.diff_state(&store).expect("attached");
    let entry_len = himark::OpenDocuments::document_ref(&store, new)
        .and_then(|document| {
            document
                .diff(state.diff_id())
                .map(|entry| entry.operation().new_len())
        })
        .expect("the entry stands");
    let live_len = himark::OpenDocuments::document_ref(&store, new)
        .expect("registered")
        .text()
        .view()
        .byte_count();
    assert_eq!(
        entry_len as usize, live_len,
        "the standing entry is the truth"
    );
}

#[test]
fn a_settled_diff_pane_goes_quiet() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = himark::AppFonts::embedded();
    let mut app = himark::Application::new(fonts);
    let _ = app.add_window();
    app.register_command(std::sync::Arc::new(OpenDiff));
    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        std::sync::Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        std::sync::Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    for (name, body) in [
        ("left.md", "# Shared\n\nleft body line\n\ntail\n"),
        (
            "right.md",
            "# Shared\n\nright body line — changed\n\ntail\nappended\n",
        ),
    ] {
        assert!(app.add_document(
            app.sole_window(),
            himarkdown::document_from_markdown(body, &store, ui, &markdown_fonts, &theme),
            name.to_owned(),
            false,
        ));
    }
    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut himark::Application, surface: &mut skia_safe::Surface| {
        for _ in 0..8 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface);
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    settle(&mut app, &mut surface);
    settle(&mut app, &mut surface);

    for ms in [0.0, 8.3, 16.7, 25.0, 33.4] {
        assert!(
            !himark::test_driver::animate(&mut app, imba::anim::AnimationClock::from_millis(ms)),
            "a settled diff pane must not answer the clock at {ms}ms"
        );
    }

    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    runner.run();
    let mut landed = 0;
    while arriving.try_recv().is_ok() {
        landed += 1;
    }
    assert_eq!(landed, 0, "a settled pane launches no further effects");
}

#[test]
fn the_unified_view_switches_between_split_and_inline() {
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();

    let unchanged: String = (0..30).map(|n| format!("same line {n}\n")).collect();
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(
            &format!("old head\n\n{unchanged}\nold tail\n"),
            app.store(),
            ui,
            &markdown_fonts,
            &theme
        ),
        "left.md".to_owned(),
        false,
    ));
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(
            &format!("new head\n\n{unchanged}\nnew tail\n"),
            app.store(),
            ui,
            &markdown_fonts,
            &theme
        ),
        "right.md".to_owned(),
        false,
    ));

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for _ in 0..4 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface);
    assert!(
        app.perform_registered(app.sole_window(), "diff.open"),
        "the diff panel opens"
    );
    settle(&mut app, &mut surface);

    let pair_id = {
        let mut id = None;
        app.for_each_plugin_panel(&mut |panel| {
            if let Some(himark::FamilyRow::Pair(pair)) = panel.family_row() {
                id = Some(pair);
            }
        });
        id.expect("the diff pane stands")
    };
    let pair_state = |app: &Application| {
        himark::OpenDocuments::diff_view_ref(app.store(), pair_id)
            .and_then(|pair| pair.state.clone())
            .expect("the pair's state row")
    };
    assert_eq!(pair_state(&app).unified_layout(), himark::DiffLayout::Split);

    let toggle = |app: &mut Application| {
        let command = himark::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
            .into_iter()
            .find(|presentable| presentable.id == "diff.toggle-layout")
            .expect("the pane offers the layout toggle")
            .command;
        assert!(app.perform_command(command));
    };
    toggle(&mut app);
    settle(&mut app, &mut surface);

    let state = pair_state(&app);
    assert_eq!(state.unified_layout(), himark::DiffLayout::Inline);
    let inline_editor = state.inline_editor().expect("the inline editor minted");
    let pair = himark::OpenDocuments::diff_view_ref(app.store(), pair_id).expect("the pair");
    let (left_id, right_id) = (pair.left.document(), pair.right.document());
    let right_editor = pair.right.editor();
    {
        let right = himark::OpenDocuments::document_ref(app.store(), right_id).expect("right");

        let cards = right.before_inlays(inline_editor);
        assert_eq!(cards.len(), 2, "both changed blocks carry cards: {cards:?}");
        assert!(
            cards.iter().any(|(_, _, text)| text.contains("old head")),
            "the head card shows the base text"
        );

        assert!(right.before_inlays(right_editor).is_empty());

        let strips = right
            .feature_markup(state.right_marks_oracle())
            .map(|markup| markup.all_inlays_in(0..u32::MAX).len())
            .unwrap_or(0);
        assert!(strips > 0, "the shared fold strips stand");
        let inline_height = right.content_height(inline_editor);
        let split_height = right.content_height(right_editor);

        assert!(
            inline_height < split_height * 1.5,
            "the inline face must fold the unchanged run: inline {inline_height}, split {split_height}"
        );
    }

    // The head card is born FULL SIZE now (settled programmatic
    // mounts): line 0 grew by the card's hole, so the old aim (60,
    // inside line 0's text) shifts down by exactly the hole — the
    // inline line-0 height minus the card-less split face's.
    let hole = {
        let right = himark::OpenDocuments::document_ref(app.store(), right_id).expect("right");
        right.height_before(inline_editor, 9) - right.height_before(right_editor, 9)
    };
    // Aim at line 0's TEXT: the settled before-card fills its full
    // hole from birth, so the text sits `hole` below the line's top
    // (~77px of pane chrome above; 88 lands mid-glyph).
    himark::test_driver::click(&mut app, 550.0, 88.0 + hole, 1100.0, 800.0);
    let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    {
        let right = himark::OpenDocuments::document_ref(app.store(), right_id).expect("right");
        assert_eq!(
            right.focus(inline_editor),
            himark::EditorFocus::Text,
            "the click should land on host text, not a card"
        );
    }
    let before = document_text(&app, "right.md");
    let _ = himark::test_driver::type_text(&mut app, "Z");
    settle(&mut app, &mut surface);
    assert_ne!(
        document_text(&app, "right.md"),
        before,
        "typing in the inline face writes through"
    );
    let _ = left_id;

    toggle(&mut app);
    settle(&mut app, &mut surface);
    assert_eq!(pair_state(&app).unified_layout(), himark::DiffLayout::Split);
}

/// PERF REGRESSION (the DiffCanvas trace, 2026-09-17): painting the
/// END of a large multi-hunk inline diff must cost the same as
/// painting its BEGINNING — every recomputation is O(viewport). A
/// linear-in-offset term shows up here as a top/bottom ratio in the
/// tens; the assert allows generous noise, never linearity.
#[test]
fn inline_diff_paint_cost_is_flat_across_the_document() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    app.register_command(Arc::new(OpenDiff));
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();

    // 6000 lines, a one-line change every 150 — forty hunks.
    let mut old_body = String::new();
    let mut new_body = String::new();
    for n in 0..6000 {
        if n % 150 == 0 {
            old_body.push_str(&format!("old change {n}\n"));
            new_body.push_str(&format!("new change {n}\n"));
        } else {
            old_body.push_str(&format!("same line {n}\n"));
            new_body.push_str(&format!("same line {n}\n"));
        }
    }
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(&old_body, &store, ui, &markdown_fonts, &theme),
        "left.md".to_owned(),
        false,
    ));
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(&new_body, &store, ui, &markdown_fonts, &theme),
        "right.md".to_owned(),
        false,
    ));

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for _ in 0..6 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface);
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    settle(&mut app, &mut surface);

    let toggle = himark::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
        .into_iter()
        .find(|presentable| presentable.id == "diff.toggle-layout")
        .expect("the pane offers the layout toggle")
        .command;
    assert!(app.perform_command(toggle));
    settle(&mut app, &mut surface);

    let paint_median_ms = |app: &mut Application, surface: &mut skia_safe::Surface| -> f64 {
        let mut times = Vec::new();
        for _ in 0..12 {
            let started = std::time::Instant::now();
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        times[times.len() / 2]
    };

    let top = paint_median_ms(&mut app, &mut surface);

    // One decisive fling to the very bottom, then let it settle.
    for _ in 0..4 {
        let _ = himark::test_driver::scroll(&mut app, 10_000_000.0);
        settle(&mut app, &mut surface);
    }
    let bottom = paint_median_ms(&mut app, &mut surface);

    eprintln!("[perf] paint median: top {top:.2}ms bottom {bottom:.2}ms");
    assert!(
        bottom < (top * 3.0).max(2.0),
        "painting the diff's end must not cost more than its start: \
         top {top:.2}ms bottom {bottom:.2}ms"
    );
}

/// PERF REGRESSION, canvas edition (the DiffCanvas trace,
/// 2026-09-17: frontend-host/src/tests.rs — a large file with dozens
/// of hunks — painted far slower at its END than at its start).
/// Canvas rendering must be O(viewport) wherever the band sits.
#[test]
fn canvas_diff_paint_cost_is_flat_across_the_document() {
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();

    // 8000 lines, a one-line change every 100 — eighty hunks.
    let mut old_body = String::new();
    let mut new_body = String::new();
    for n in 0..20000 {
        if n % 100 == 0 {
            old_body.push_str(&format!("old change {n}\n"));
            new_body.push_str(&format!("new change {n}\n"));
        } else {
            old_body.push_str(&format!("same line {n}\n"));
            new_body.push_str(&format!("same line {n}\n"));
        }
    }
    // Dense inline markup — the volume a syntax highlighter puts on
    // a real source file (several styled spans per line).
    let dense = |body: &str| {
        let mut markup = himark::Markup::new();
        let mut at = 0u32;
        for line in body.split_inclusive('\n') {
            let len = line.len() as u32;
            for word in 0..5u32 {
                let start = at + word * 4;
                let end = (start + 3).min(at + len.saturating_sub(1));
                if start < end {
                    markup.push_styled(start..end, himark::StyleId::DiffAdded);
                }
            }
            at += len;
        }
        markup
    };
    let old = himark::Document::new(
        himark::Text::from_string_exact(old_body.clone()),
        dense(&old_body),
    );
    let new = himark::Document::new(
        himark::Text::from_string_exact(new_body.clone()),
        dense(&new_body),
    );
    let _ = (&markdown_fonts, &theme);
    let operation = myersdiff::diff(old.text(), new.text());
    let marks = himark::prepare_marks(&operation, old.text());

    let location = |name: &str, kind| {
        himark::ResourceLocation::new(
            kind,
            himark::Authority::new("test"),
            vec!["proj".to_owned(), name.to_owned()],
        )
    };
    let file = himark::diff_canvas::CanvasFile {
        title: "big.md".to_owned(),
        old: location("big.md.old", himark::ResourceType::document()),
        new: location("big.md", himark::ResourceType::document()),
        added: Some(80),
        removed: Some(80),
        updated: 0,
    };
    let built = himark::BuiltFileDiff {
        old,
        new,
        operation,
        marks,
        width: 1100.0,
        failed: None,
    };
    let canvas = {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        DiffCanvasView::seeded_for_tests(
            &mut store,
            &ui,
            himark::diff_canvas::CanvasSource::WorkingCopy {
                folder: location("proj", himark::ResourceType::directory()),
            },
            file,
            built,
        )
    };
    assert!(app.open_panel(app.sole_window(), Box::new(canvas)));

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let settle = |app: &mut Application, surface: &mut skia_safe::Surface| {
        for _ in 0..6 {
            let _ = himark::test_driver::animate(
                &mut *app,
                imba::anim::AnimationClock::from_millis(0.0),
            );
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app, &mut surface);

    let paint_median_ms = |app: &mut Application, surface: &mut skia_safe::Surface| -> f64 {
        let mut times = Vec::new();
        for _ in 0..12 {
            let started = std::time::Instant::now();
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        times[times.len() / 2]
    };

    let top = paint_median_ms(&mut app, &mut surface);
    for _ in 0..4 {
        let _ = himark::test_driver::scroll(&mut app, 10_000_000.0);
        settle(&mut app, &mut surface);
    }
    let bottom = paint_median_ms(&mut app, &mut surface);

    eprintln!("[perf] canvas paint median: top {top:.2}ms bottom {bottom:.2}ms");
    // A linear-in-offset regression measures WAY past this (5.4x at
    // 20k lines before the lazy sweep seed; grows with file size) —
    // the bound is ratio-based so machine speed cancels out.
    assert!(
        bottom < (top * 3.0).max(2.0),
        "painting the canvas diff's end must not cost more than its start: \
         top {top:.2}ms bottom {bottom:.2}ms"
    );
}

/// PERF REGRESSION: a huge file squashed into ONE viewport by folds
/// (two hunks, everything between folded) must paint at the same
/// cost as a small one — the fold gap's markup is never traversed
/// (dedicated shape/style interval lanes + per-segment sweeps).
#[test]
fn folded_squash_paint_cost_is_size_independent() {
    let median_for = |line_count: usize| -> f64 {
        let fonts = AppFonts::embedded();
        let mut app = Application::new(fonts);
        let _ = app.add_window();

        let mut old_body = String::new();
        let mut new_body = String::new();
        for n in 0..line_count {
            if n == 0 || n == line_count - 1 {
                old_body.push_str(&format!("old change {n}\n"));
                new_body.push_str(&format!("new change {n}\n"));
            } else {
                old_body.push_str(&format!("same line {n}\n"));
                new_body.push_str(&format!("same line {n}\n"));
            }
        }
        let dense = |body: &str| {
            let mut markup = himark::Markup::new();
            let mut at = 0u32;
            for line in body.split_inclusive('\n') {
                let len = line.len() as u32;
                for word in 0..5u32 {
                    let start = at + word * 4;
                    let end = (start + 3).min(at + len.saturating_sub(1));
                    if start < end {
                        markup.push_styled(start..end, himark::StyleId::DiffAdded);
                    }
                }
                at += len;
            }
            markup
        };
        let old = himark::Document::new(
            himark::Text::from_string_exact(old_body.clone()),
            dense(&old_body),
        );
        let new = himark::Document::new(
            himark::Text::from_string_exact(new_body.clone()),
            dense(&new_body),
        );
        let operation = myersdiff::diff(old.text(), new.text());
        let marks = himark::prepare_marks(&operation, old.text());
        let location = |name: &str, kind| {
            himark::ResourceLocation::new(
                kind,
                himark::Authority::new("test"),
                vec!["proj".to_owned(), name.to_owned()],
            )
        };
        let file = himark::diff_canvas::CanvasFile {
            title: "big.md".to_owned(),
            old: location("big.md.old", himark::ResourceType::document()),
            new: location("big.md", himark::ResourceType::document()),
            added: Some(2),
            removed: Some(2),
            updated: 0,
        };
        let built = himark::BuiltFileDiff {
            old,
            new,
            operation,
            marks,
            width: 1100.0,
            failed: None,
        };
        let canvas = {
            let ui = app.ui_handle();
            let mut store = app.store_mut();
            DiffCanvasView::seeded_for_tests(
                &mut store,
                &ui,
                himark::diff_canvas::CanvasSource::WorkingCopy {
                    folder: location("proj", himark::ResourceType::directory()),
                },
                file,
                built,
            )
        };
        assert!(app.open_panel(app.sole_window(), Box::new(canvas)));

        let size = skia_safe::Size::new(1100.0, 800.0);
        let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
        for _ in 0..6 {
            let _ = himark::test_driver::animate(
                &mut app,
                imba::anim::AnimationClock::from_millis(0.0),
            );
            let _ =
                himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
        }
        let mut times = Vec::new();
        for _ in 0..10 {
            let started = std::time::Instant::now();
            let _ =
                himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        times[times.len() / 2]
    };

    let small = median_for(5_000);
    let large = median_for(200_000);
    eprintln!("[perf] squashed paint median: 5k lines {small:.2}ms, 200k lines {large:.2}ms");
    assert!(
        large < (small * 3.0).max(2.0),
        "a fold-squashed viewport must paint independent of file size: \
         5k {small:.2}ms vs 200k {large:.2}ms"
    );
}

#[test]
fn a_full_click_on_host_text_keeps_host_focus() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();

    // Small diff: a changed head, unchanged middle, changed tail —
    // two before-cards on the inline face.
    let mut old_body = String::from("old head\n");
    let mut new_body = String::from("new head\n");
    for n in 0..30 {
        old_body.push_str(&format!("same line {n}\n"));
        new_body.push_str(&format!("same line {n}\n"));
    }
    old_body.push_str("old tail\n");
    new_body.push_str("new tail\n");

    himarkdown::register_handlers(&mut app);
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    let old = himarkdown::document_from_markdown(&old_body, &store, ui, &markdown_fonts, &theme);
    let new = himarkdown::document_from_markdown(&new_body, &store, ui, &markdown_fonts, &theme);
    let operation = myersdiff::diff(old.text(), new.text());
    let marks = himark::prepare_marks(&operation, old.text());
    let location = |name: &str, kind| {
        himark::ResourceLocation::new(
            kind,
            himark::Authority::new("test"),
            vec!["proj".to_owned(), name.to_owned()],
        )
    };
    let file = himark::diff_canvas::CanvasFile {
        title: "small.md".to_owned(),
        old: location("small.md.old", himark::ResourceType::document()),
        new: location("small.md", himark::ResourceType::document()),
        added: Some(2),
        removed: Some(2),
        updated: 0,
    };
    // A MISMATCHED build width — the real canvas arms at one width
    // and lands after a resize; the rewrap ride reconciles.
    let built = himark::BuiltFileDiff {
        old,
        new,
        operation,
        marks,
        width: 700.0,
        failed: None,
    };
    let canvas = {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        DiffCanvasView::seeded_for_tests(
            &mut store,
            &ui,
            himark::diff_canvas::CanvasSource::WorkingCopy {
                folder: location("proj", himark::ResourceType::directory()),
            },
            file,
            built,
        )
    };
    assert!(app.open_panel(app.sole_window(), Box::new(canvas)));

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    for _ in 0..6 {
        let _ =
            himark::test_driver::animate(&mut app, imba::anim::AnimationClock::from_millis(0.0));
        let _ = himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
    }

    let host_focus = |app: &Application| -> String {
        let store = app.store();
        let mut shot = None;
        app.for_each_plugin_panel(&mut |panel| {
            if let Some(canvas) = panel.as_any().downcast_ref::<DiffCanvasView>() {
                shot = canvas
                    .probe_focus(store)
                    .into_iter()
                    .next()
                    .map(|(_, host, _)| host);
            }
        });
        shot.expect("the built row")
    };
    let full_click = |app: &mut Application, y: f32| {
        let _ = himark::test_driver::click(app, 550.0, y, 1100.0, 800.0);
        let _ = himark::test_driver::mouse_up(app, 550.0, y);
    };

    // REGRESSION (the focus-trace hunt, 2026-09-18): MouseUp is
    // BROADCAST by containers, and a once-clicked before-card kept
    // its inner editor text-focused — so it claimed every later
    // release and its DragEnd yanked host focus back into the card
    // on every host click. Releases belong to the DRAG OWNER only.
    let (card_y, text_y) = (200.0, 260.0);
    full_click(&mut app, card_y);
    assert!(
        host_focus(&app).starts_with("Inlay"),
        "the card click focuses the card: {}",
        host_focus(&app)
    );

    full_click(&mut app, text_y);
    assert_eq!(
        host_focus(&app),
        "Text",
        "a full host click keeps host focus"
    );
    full_click(&mut app, text_y);
    assert_eq!(host_focus(&app), "Text", "…and stays on EVERY later click");

    let _ = himark::test_driver::mouse_move(&mut app, 550.0, card_y);
    assert_eq!(host_focus(&app), "Text", "hovering the card moves nothing");
}

#[test]
fn the_header_folds_toggles_and_answers_from_the_sticky_band() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();

    let mut old_body = String::from("old head\n");
    let mut new_body = String::from("new head\n");
    for n in 0..60 {
        old_body.push_str(&format!("same line {n}\n"));
        new_body.push_str(&format!("same line {n}\n"));
    }
    old_body.push_str("old tail\n");
    new_body.push_str("new tail\n");

    himarkdown::register_handlers(&mut app);
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    let old = himarkdown::document_from_markdown(&old_body, &store, ui, &markdown_fonts, &theme);
    let new = himarkdown::document_from_markdown(&new_body, &store, ui, &markdown_fonts, &theme);
    let operation = myersdiff::diff(old.text(), new.text());
    let marks = himark::prepare_marks(&operation, old.text());
    let location = |name: &str, kind| {
        himark::ResourceLocation::new(
            kind,
            himark::Authority::new("test"),
            vec!["proj".to_owned(), name.to_owned()],
        )
    };
    let file = himark::diff_canvas::CanvasFile {
        title: "long.md".to_owned(),
        old: location("long.md.old", himark::ResourceType::document()),
        new: location("long.md", himark::ResourceType::document()),
        added: Some(2),
        removed: Some(2),
        updated: 0,
    };
    let built = himark::BuiltFileDiff {
        old,
        new,
        operation,
        marks,
        width: 1100.0,
        failed: None,
    };
    let canvas = {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        DiffCanvasView::seeded_for_tests(
            &mut store,
            &ui,
            himark::diff_canvas::CanvasSource::WorkingCopy {
                folder: location("proj", himark::ResourceType::directory()),
            },
            file,
            built,
        )
    };
    assert!(app.open_panel(app.sole_window(), Box::new(canvas)));

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let mut settle = |app: &mut Application| {
        for _ in 0..6 {
            let _ = himark::test_driver::animate(app, imba::anim::AnimationClock::from_millis(0.0));
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app);

    let probe = |app: &Application| -> (
        Vec<(String, canvas::RowPhase, f32)>,
        Vec<(String, himark::DiffLayout)>,
        f32,
    ) {
        let store = app.store();
        let mut shot = None;
        app.for_each_plugin_panel(&mut |panel| {
            if let Some(canvas) = panel.as_any().downcast_ref::<DiffCanvasView>() {
                shot = Some((
                    canvas.probe_rows(store),
                    canvas.probe_layouts(store),
                    canvas.probe_scroll_top(store),
                ));
            }
        });
        shot.expect("the canvas panel")
    };
    let click = |app: &mut Application, x: f32, y: f32| {
        let _ = himark::test_driver::click(app, x, y, 1100.0, 800.0);
        let _ = himark::test_driver::mouse_up(app, x, y);
    };

    let (rows, layouts, _) = probe(&app);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].2 > 0.0, "the diff row stands expanded: {rows:?}");
    assert_eq!(layouts[0].1, himark::DiffLayout::Inline, "mounted inline");

    // The header geometry, mirrored from HeaderFace::new: the chevron
    // strip at the left, the three buttons walking in from the right.
    let chrome_top = himark::env::Themes::of(app.store()).ui().toolbar.height;
    let chat_pad;
    let chat_title;
    {
        let ui = himark::env::Themes::of(app.store()).ui().chat.clone();
        chat_pad = ui.pad;
        chat_title = ui.title_size;
    }
    let zone = chat_title * 2.2;
    let header_y = chrome_top + 20.0;
    let chevron_x = chat_pad * 0.5;
    let face_x = 1100.0 - chat_pad - zone * 2.5;

    // The layout button flips the face…
    click(&mut app, face_x, header_y);
    settle(&mut app);
    assert_eq!(
        probe(&app).1[0].1,
        himark::DiffLayout::Split,
        "the face button flips inline → split"
    );

    // …the chevron folds the diff away (the row leaves the list, the
    // built view parks)…
    click(&mut app, chevron_x, header_y);
    settle(&mut app);
    let (rows, layouts, _) = probe(&app);
    assert_eq!(rows[0].2, 0.0, "collapsed: no diff row stands: {rows:?}");
    assert_eq!(rows[0].1, canvas::RowPhase::Built, "…but the build is kept");
    assert_eq!(layouts.len(), 1, "the parked row still answers probes");

    // …and the face button still routes INTO the parked row.
    click(&mut app, face_x, header_y);
    settle(&mut app);
    assert_eq!(
        probe(&app).1[0].1,
        himark::DiffLayout::Inline,
        "toggling a collapsed file reaches the parked view"
    );

    // The chevron brings the diff back at its parked height.
    click(&mut app, chevron_x, header_y);
    settle(&mut app);
    let (rows, _, _) = probe(&app);
    assert!(rows[0].2 > 0.0, "expanded again: {rows:?}");

    // Scroll the header behind the top edge: the list PLANTS it, and
    // the planted chevron folds the file exactly like the real one.
    let _ = himark::test_driver::scroll(&mut app, 400.0);
    settle(&mut app);
    let (_, _, scrolled) = probe(&app);
    assert!(scrolled > 0.0, "the canvas scrolled: {scrolled}");
    click(&mut app, chevron_x, chrome_top + 20.0);
    settle(&mut app);
    let (rows, _, _) = probe(&app);
    assert_eq!(
        rows[0].2, 0.0,
        "the planted header's chevron collapsed the file: {rows:?}"
    );

    // The focused row offers the open roads on the focus chain —
    // cmd-enter ("workbench.open-in-full") needs nothing more.
    let offered = himark::palette_commands(app.store(), &app.ui_handle(), app.sole_window());
    for id in ["workbench.open-in-full", "diff.open-pane"] {
        assert!(
            offered.iter().any(|presentable| presentable.id == id),
            "the focused canvas row offers {id}"
        );
    }
}

// REGRESSION (2026-09-18): the canvas halves shipped with
// `reports_geometry: false`, so on the split face the BOUNDED
// editors never got the Viewport road — beyond the initial layout
// budget every line stayed a ~6px estimate, folds never collapsed,
// and the face opened onto thousands of pixels of blank wash. The
// halves now report geometry like the standalone pane's, resize to
// the half-pane width, and heal through the pair lane.
#[test]
fn the_split_face_folds_and_wraps_to_its_halves() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();

    let mut old_body = String::from("old head\n");
    let mut new_body = String::from("new head\n");
    for n in 0..600 {
        old_body.push_str(&format!("same line {n}\n"));
        new_body.push_str(&format!("same line {n}\n"));
    }
    old_body.push_str("old tail\n");
    new_body.push_str("new tail\n");

    himarkdown::register_handlers(&mut app);
    let theme = himark::Theme::embedded();
    let markdown_fonts = himark::test_document::test_fonts_collection().clone();
    let old = himarkdown::document_from_markdown(&old_body, &store, ui, &markdown_fonts, &theme);
    let new = himarkdown::document_from_markdown(&new_body, &store, ui, &markdown_fonts, &theme);
    let operation = myersdiff::diff(old.text(), new.text());
    let marks = himark::prepare_marks(&operation, old.text());
    let location = |name: &str, kind| {
        himark::ResourceLocation::new(
            kind,
            himark::Authority::new("test"),
            vec!["proj".to_owned(), name.to_owned()],
        )
    };
    let file = himark::diff_canvas::CanvasFile {
        title: "long.md".to_owned(),
        old: location("long.md.old", himark::ResourceType::document()),
        new: location("long.md", himark::ResourceType::document()),
        added: Some(2),
        removed: Some(2),
        updated: 0,
    };
    let built = himark::BuiltFileDiff {
        old,
        new,
        operation,
        marks,
        width: 1100.0,
        failed: None,
    };
    let canvas = {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        DiffCanvasView::seeded_for_tests(
            &mut store,
            &ui,
            himark::diff_canvas::CanvasSource::WorkingCopy {
                folder: location("proj", himark::ResourceType::directory()),
            },
            file,
            built,
        )
    };
    assert!(app.open_panel(app.sole_window(), Box::new(canvas)));

    let size = skia_safe::Size::new(1100.0, 800.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let mut settle = |app: &mut Application| {
        for _ in 0..8 {
            let _ = himark::test_driver::animate(app, imba::anim::AnimationClock::from_millis(0.0));
            let _ = himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
        }
    };
    settle(&mut app);

    let probe = |app: &Application| {
        let store = app.store();
        let mut shot = None;
        app.for_each_plugin_panel(&mut |panel| {
            if let Some(canvas) = panel.as_any().downcast_ref::<DiffCanvasView>() {
                shot = Some((canvas.probe_rows(store), canvas.probe_layouts(store)));
            }
        });
        shot.expect("the canvas panel")
    };
    let halves = |app: &Application| {
        let store = app.store();
        let mut shot = None;
        app.for_each_plugin_panel(&mut |panel| {
            if let Some(canvas) = panel.as_any().downcast_ref::<DiffCanvasView>() {
                shot = Some(canvas.probe_half_heights(store));
            }
        });
        shot.expect("the canvas panel")
    };
    let (rows, layouts) = probe(&app);
    assert_eq!(layouts[0].1, himark::DiffLayout::Inline);
    let inline_height = rows[0].2;
    assert!(
        inline_height < 1200.0,
        "the inline face folds the 600-line run: {inline_height}"
    );

    let chrome_top = himark::env::Themes::of(app.store()).ui().toolbar.height;
    let (chat_pad, chat_title) = {
        let ui = himark::env::Themes::of(app.store()).ui().chat.clone();
        (ui.pad, ui.title_size)
    };
    let zone = chat_title * 2.2;
    let face_x = 1100.0 - chat_pad - zone * 2.5;
    let _ = himark::test_driver::click(&mut app, face_x, chrome_top + 20.0, 1100.0, 800.0);
    let _ = himark::test_driver::mouse_up(&mut app, face_x, chrome_top + 20.0);
    settle(&mut app);
    settle(&mut app);

    let (rows, layouts) = probe(&app);
    assert_eq!(layouts[0].1, himark::DiffLayout::Split);
    assert!(
        rows[0].2 < inline_height + 400.0,
        "the split face folds too — no blank wash of estimated lines \
         (was ~4350px when the halves never healed): {}",
        rows[0].2
    );
    let (left_height, right_height, _, left_width, right_width) = halves(&app)[0];
    assert!(
        (left_height - right_height).abs() <= 1.0,
        "the halves stand aligned: {left_height} vs {right_height}"
    );
    assert!(
        left_width < 700.0 && (left_width - right_width).abs() <= 1.0,
        "each half wraps to the HALF-pane width: {left_width}/{right_width}"
    );

    // Back to inline: the row-level rewrap returns all editors to the
    // full row width, so the pair sync engages again.
    let _ = himark::test_driver::click(&mut app, face_x, chrome_top + 20.0, 1100.0, 800.0);
    let _ = himark::test_driver::mouse_up(&mut app, face_x, chrome_top + 20.0);
    settle(&mut app);
    settle(&mut app);
    settle(&mut app);
    let (rows, layouts) = probe(&app);
    assert_eq!(layouts[0].1, himark::DiffLayout::Inline);
    assert!(
        (rows[0].2 - inline_height).abs() < 200.0,
        "the inline face comes back at its folded height: {} vs {inline_height}",
        rows[0].2
    );
    let (_, _, _, left_width, right_width) = halves(&app)[0];
    assert!(
        (left_width - right_width).abs() <= 1.0 && left_width > 700.0,
        "the halves rewrap to the full row width for the pair sync: \
         {left_width}/{right_width}"
    );
}

/// The unified-gate reconcile (docs/editor/diff-canvas.md §7): a populated
/// canvas follows the change set — stale rows rebuild IN PLACE (the
/// old build keeps showing until the fresh one lands), additions
/// splice in as lazy placeholders, removals retire, untouched rows
/// are not rebuilt.
#[test]
fn reconcile_follows_the_change_set_without_flashing() {
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();

    let location = |name: &str, kind| {
        himark::ResourceLocation::new(
            kind,
            himark::Authority::new("test"),
            vec!["proj".to_owned(), name.to_owned()],
        )
    };
    let canvas_file = |name: &str, updated: u64| himark::diff_canvas::CanvasFile {
        title: name.to_owned(),
        old: location(&format!("{name}.old"), himark::ResourceType::document()),
        new: location(name, himark::ResourceType::document()),
        added: Some(1),
        removed: Some(1),
        updated,
    };
    let built_diff = |old_body: &str, new_body: &str| {
        let old = himark::Document::new(
            himark::Text::from_string_exact(old_body),
            himark::Markup::new(),
        );
        let new = himark::Document::new(
            himark::Text::from_string_exact(new_body),
            himark::Markup::new(),
        );
        let operation = myersdiff::diff(old.text(), new.text());
        let marks = himark::prepare_marks(&operation, old.text());
        himark::BuiltFileDiff {
            old,
            new,
            operation,
            marks,
            width: 1100.0,
            failed: None,
        }
    };

    let file_a = canvas_file("a.md", 1);
    let view = {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        DiffCanvasView::seeded_for_tests(
            &mut store,
            &ui,
            himark::diff_canvas::CanvasSource::WorkingCopy {
                folder: location("proj", himark::ResourceType::directory()),
            },
            file_a.clone(),
            built_diff("one\ntwo\n", "one\nTWO\n"),
        )
    };

    let rows = view.probe_rows(&app.store());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, canvas::RowPhase::Built);

    // The same listing again: nothing moves, nothing rebuilds.
    let launched = view.reconcile_for_tests(&mut app.store_mut(), vec![file_a.clone()]);
    assert_eq!(launched, 0, "an unchanged entry must not rebuild");
    assert_eq!(view.probe_rows(&app.store())[0].1, canvas::RowPhase::Built);

    // The host touches a.md (stamp moves) and adds c.md.
    let mut file_a_touched = file_a.clone();
    file_a_touched.updated = 2;
    file_a_touched.added = Some(3);
    let file_c = canvas_file("c.md", 2);
    let launched = view.reconcile_for_tests(
        &mut app.store_mut(),
        vec![file_a_touched.clone(), file_c.clone()],
    );
    assert_eq!(launched, 1, "exactly the stale row relaunches its build");
    let rows = view.probe_rows(&app.store());
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        (rows[0].0.as_str(), rows[0].1),
        ("a.md", canvas::RowPhase::Built),
        "the stale row keeps showing its old build — no placeholder flash"
    );
    assert_eq!(
        (rows[1].0.as_str(), rows[1].1),
        ("c.md", canvas::RowPhase::Placeholder),
        "the addition arrives as a lazy placeholder"
    );

    // The relaunched build lands: still Built, swapped in place.
    {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        view.land_for_tests(
            &mut store,
            &ui,
            file_a.new.clone(),
            built_diff("one\ntwo\n", "one\nTWO\nthree\n"),
        );
    }
    assert_eq!(view.probe_rows(&app.store())[0].1, canvas::RowPhase::Built);

    // A touch that does NOT move the stamp (host silence) rebuilds
    // nothing even though the reconcile ran.
    let launched = view.reconcile_for_tests(
        &mut app.store_mut(),
        vec![file_a_touched.clone(), file_c.clone()],
    );
    assert_eq!(launched, 0);

    // a.md leaves the change set (e.g. reverted): its rows retire.
    let launched = view.reconcile_for_tests(&mut app.store_mut(), vec![file_c.clone()]);
    assert_eq!(launched, 0);
    let rows = view.probe_rows(&app.store());
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "c.md");

    // Cleared entirely (the commit case): the canvas empties.
    let launched = view.reconcile_for_tests(&mut app.store_mut(), Vec::new());
    assert_eq!(launched, 0);
    assert!(view.probe_rows(&app.store()).is_empty());
}

/// The registered-document fix (docs/editor/diff-canvas.md §7):
/// a canvas row's target is the registered `OpenDocuments` document
/// for its location, and TYPING into it updates the row's tracked
/// diff — the bug that motivated moving rows off throwaway snapshots.
#[test]
fn typing_in_a_canvas_row_updates_its_diff() {
    let ui = himark::test_document::test_ui();
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();

    let doc_loc = |name: &str| {
        himark::ResourceLocation::new(
            himark::ResourceType::document(),
            himark::Authority::new("test"),
            vec!["proj".to_owned(), name.to_owned()],
        )
    };

    // Identical sides: the initial diff is the identity.
    let body = "hello\nworld\n";
    let old = himark::Document::new(himark::Text::from_string_exact(body), himark::Markup::new());
    let new = himark::Document::new(himark::Text::from_string_exact(body), himark::Markup::new());
    let operation = myersdiff::diff(old.text(), new.text());
    let marks = himark::prepare_marks(&operation, old.text());
    let file = himark::diff_canvas::CanvasFile {
        title: "a.md".to_owned(),
        old: doc_loc("a.md.old"),
        new: doc_loc("a.md"),
        added: Some(0),
        removed: Some(0),
        updated: 0,
    };
    let built = himark::BuiltFileDiff {
        old,
        new,
        operation,
        marks,
        width: 1100.0,
        failed: None,
    };

    let view = {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        DiffCanvasView::seeded_for_tests(
            &mut store,
            &ui,
            himark::diff_canvas::CanvasSource::WorkingCopy {
                folder: himark::ResourceLocation::new(
                    himark::ResourceType::directory(),
                    himark::Authority::new("test"),
                    vec!["proj".to_owned()],
                ),
            },
            file,
            built,
        )
    };

    // The row's target IS the registered document for its location —
    // not a throwaway snapshot.
    let target_id = himark::OpenDocuments::by_location(&app.store(), &doc_loc("a.md"))
        .expect("the canvas row registered its target document");

    // The diff the Diffs subsystem tracks for this row.
    let pair = view
        .probe_pair(&app.store(), &doc_loc("a.md"))
        .expect("a built pair");
    let diff_id = himark::OpenDocuments::diff_view_ref(&app.store(), pair)
        .expect("the tracked diff view")
        .diff;

    let op_of = |app: &Application| {
        himark::OpenDocuments::document_ref(app.store(), target_id)
            .and_then(|document| {
                document
                    .diff(diff_id)
                    .map(|entry| entry.operation().clone())
            })
            .expect("the row's diff is tracked on the registered document")
    };
    let has_edit =
        |op: &operation::Operation| op.iter().any(|o| !matches!(o, operation::Op::Retain(_)));

    let before = op_of(&app);
    assert!(
        !has_edit(&before),
        "identical sides start as the identity diff: {before:?}"
    );

    // Type an "X" after "hello" into the registered target document.
    {
        let fonts = himark::env::Fonts::of(&app.store())();
        let theme = himark::env::Themes::of(&app.store());
        let mut store = app.store_mut();
        let mut document =
            himark::OpenDocuments::document(&store, target_id).expect("target document");
        let mut batch = imba::effect::Batch::new();
        document.edit(
            &operation::Operation::insert_at(5, "X"),
            &store,
            ui,
            &fonts,
            &theme,
            &mut batch.effects(),
        );
        himark::OpenDocuments::put_document(&mut store, target_id, document);
    }

    let after = op_of(&app);
    assert_ne!(
        before, after,
        "typing into the row must update its tracked diff"
    );
    assert!(
        after
            .iter()
            .any(|o| matches!(o, operation::Op::Insert(text) if text == "X")),
        "the diff reflects the typed insert: {after:?}"
    );
}

/// `Canvases::sync` is the tick driver that keeps canvas state current
/// off-paint (the reveal-after-change fix). It must reconcile stale
/// canvases and leave up-to-date ones untouched, without corrupting
/// the store-held state. (The change-set→row reconcile it delegates to
/// is covered by `reconcile_follows_the_change_set_without_flashing`;
/// full Changes population lives behind himark's session machinery.)
#[test]
fn canvases_sync_is_a_safe_no_op_when_current() {
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();

    let doc_loc = |name: &str| {
        himark::ResourceLocation::new(
            himark::ResourceType::document(),
            himark::Authority::new("test"),
            vec!["proj".to_owned(), name.to_owned()],
        )
    };
    let file = himark::diff_canvas::CanvasFile {
        title: "a.md".to_owned(),
        old: doc_loc("a.md.old"),
        new: doc_loc("a.md"),
        added: Some(0),
        removed: Some(0),
        updated: 0,
    };
    let make = |body: &str| {
        himark::Document::new(himark::Text::from_string_exact(body), himark::Markup::new())
    };
    let old = make("hello\n");
    let new = make("hello\n");
    let operation = myersdiff::diff(old.text(), new.text());
    let marks = himark::prepare_marks(&operation, old.text());
    let built = himark::BuiltFileDiff {
        old,
        new,
        operation,
        marks,
        width: 1100.0,
        failed: None,
    };

    let view = {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        DiffCanvasView::seeded_for_tests(
            &mut store,
            &ui,
            himark::diff_canvas::CanvasSource::WorkingCopy {
                folder: himark::ResourceLocation::new(
                    himark::ResourceType::directory(),
                    himark::Authority::new("test"),
                    vec!["proj".to_owned()],
                ),
            },
            file,
            built,
        )
    };

    let before = view.probe_rows(&app.store());
    assert_eq!(before.len(), 1, "seeded one row");

    // The tick driver runs against every store-held canvas.
    crate::Canvases::sync(&mut app.store_mut(), himark::test_document::test_ui());

    // With no live Changes source the sync is a no-op: the row and its
    // registered pair survive intact (state not corrupted).
    let after = view.probe_rows(&app.store());
    assert_eq!(after, before, "sync must not disturb an up-to-date canvas");
    assert!(
        view.probe_pair(&app.store(), &doc_loc("a.md")).is_some(),
        "the row's registered pair survives a sync tick"
    );
}

/// Sticky headers ride the File key's COVER span (header + diff
/// child). The working-copy canvas reconciles on every host touch,
/// and its header resplice must re-assert the full cover — the bug
/// where it covered only itself killed sticky headers in the changes
/// view while the (never-reconciling) commit view kept them.
#[test]
fn reconcile_keeps_the_sticky_cover() {
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();

    let (view, key, built2) = seeded_working_canvas(&mut app);
    assert_eq!(
        view.probe_cover(&app.store(), &key),
        Some(2),
        "a built expanded row: the File cover spans header + diff"
    );

    // A host touch: the stamp moves, reconcile resplices the header
    // (fresh stats) and relaunches the build.
    let launched = view.reconcile_for_tests(&mut app.store_mut(), vec![canvas_file(&key, 2)]);
    assert_eq!(launched, 1, "the stale row relaunches");
    assert_eq!(
        view.probe_cover(&app.store(), &key),
        Some(2),
        "the header resplice must keep the full sticky cover"
    );

    // The rebuild lands: still covered.
    {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        view.land_for_tests(&mut store, &ui, key.clone(), built2);
    }
    assert_eq!(
        view.probe_cover(&app.store(), &key),
        Some(2),
        "a landed rebuild keeps the cover too"
    );
}

/// The after-commit state reaches the canvas as `CanvasListing::Empty`
/// (Ready status, zero files) — a populated canvas must retire its
/// rows and show the note, not hold the committed change set. The bug:
/// empty-as-`Pending` made a populated canvas ignore it entirely.
#[test]
fn a_committed_change_set_empties_the_canvas() {
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();

    let (view, key, _built2) = seeded_working_canvas(&mut app);
    assert_eq!(view.probe_rows(&app.store()).len(), 1);

    // The commit: the host clears the set — READY and empty.
    view.adopt_for_tests(
        &mut app.store_mut(),
        himark::test_document::test_ui(),
        7,
        himark::diff_canvas::CanvasListing::Empty("no changes".to_owned()),
    );
    assert!(
        view.probe_rows(&app.store()).is_empty(),
        "the committed rows retire"
    );
    // Non-vacuous: the LIST rows are actually gone, not just `files`.
    assert!(
        view.probe_row_keys(&app.store()).is_empty(),
        "no list row survives the commit: {:?}",
        view.probe_row_keys(&app.store())
    );
    assert_eq!(
        view.probe_note(&app.store()).as_deref(),
        Some("no changes"),
        "…and the canvas says why it is empty"
    );

    // A transient computing state must NOT clear anything…
    view.adopt_for_tests(
        &mut app.store_mut(),
        himark::test_document::test_ui(),
        8,
        himark::diff_canvas::CanvasListing::Ready(vec![canvas_file(&key, 3)]),
    );
    assert_eq!(
        view.probe_rows(&app.store()).len(),
        1,
        "fresh change arrives"
    );
    view.adopt_for_tests(
        &mut app.store_mut(),
        himark::test_document::test_ui(),
        9,
        himark::diff_canvas::CanvasListing::Pending("computing…".to_owned()),
    );
    assert_eq!(
        view.probe_rows(&app.store()).len(),
        1,
        "a refetch's computing flicker holds the rows"
    );
}

fn canvas_file(key: &himark::ResourceLocation, updated: u64) -> himark::diff_canvas::CanvasFile {
    himark::diff_canvas::CanvasFile {
        title: "a.md".to_owned(),
        old: himark::ResourceLocation::new(
            himark::ResourceType::document(),
            himark::Authority::new("test"),
            vec!["proj".to_owned(), "a.md.old".to_owned()],
        ),
        new: key.clone(),
        added: Some(1),
        removed: Some(1),
        updated,
    }
}

fn seeded_working_canvas(
    app: &mut Application,
) -> (
    DiffCanvasView,
    himark::ResourceLocation,
    himark::BuiltFileDiff,
) {
    let key = himark::ResourceLocation::new(
        himark::ResourceType::document(),
        himark::Authority::new("test"),
        vec!["proj".to_owned(), "a.md".to_owned()],
    );
    let make = |body: &str| {
        himark::Document::new(himark::Text::from_string_exact(body), himark::Markup::new())
    };
    let built = |old_body: &str, new_body: &str| {
        let old = make(old_body);
        let new = make(new_body);
        let operation = myersdiff::diff(old.text(), new.text());
        let marks = himark::prepare_marks(&operation, old.text());
        himark::BuiltFileDiff {
            old,
            new,
            operation,
            marks,
            width: 1100.0,
            failed: None,
        }
    };
    let view = {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        DiffCanvasView::seeded_for_tests(
            &mut store,
            &ui,
            himark::diff_canvas::CanvasSource::WorkingCopy {
                folder: himark::ResourceLocation::new(
                    himark::ResourceType::directory(),
                    himark::Authority::new("test"),
                    vec!["proj".to_owned()],
                ),
            },
            canvas_file(&key, 1),
            built("one\n", "ONE\n"),
        )
    };
    (view, key, built("one\n", "ONE!\n"))
}

/// Retiring a file that was RESTAMPED (so its header was respliced by
/// `refresh_header`) must remove BOTH its rows — the header and the
/// diff child. The cover-shrinking bug orphaned the diff row: `retire`
/// spliced only the header the corrupted cover pointed at, `files`
/// dropped the key, and the diff row lingered forever — "stale files
/// not removed."
#[test]
fn a_retired_file_leaves_no_orphan_row() {
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();

    let loc = |name: &str| {
        himark::ResourceLocation::new(
            himark::ResourceType::document(),
            himark::Authority::new("test"),
            vec!["proj".to_owned(), name.to_owned()],
        )
    };
    let file = |name: &str, updated: u64| himark::diff_canvas::CanvasFile {
        title: name.to_owned(),
        old: loc(&format!("{name}.old")),
        new: loc(name),
        added: Some(1),
        removed: Some(1),
        updated,
    };
    let make =
        |b: &str| himark::Document::new(himark::Text::from_string_exact(b), himark::Markup::new());
    let built = |o: &str, n: &str| {
        let (old, new) = (make(o), make(n));
        let operation = myersdiff::diff(old.text(), new.text());
        let marks = himark::prepare_marks(&operation, old.text());
        himark::BuiltFileDiff {
            old,
            new,
            operation,
            marks,
            width: 1100.0,
            failed: None,
        }
    };

    // Seed a.md built, then reconcile in b.md and build it too.
    let view = {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        DiffCanvasView::seeded_for_tests(
            &mut store,
            &ui,
            himark::diff_canvas::CanvasSource::WorkingCopy {
                folder: himark::ResourceLocation::new(
                    himark::ResourceType::directory(),
                    himark::Authority::new("test"),
                    vec!["proj".to_owned()],
                ),
            },
            file("a.md", 1),
            built("a\n", "A\n"),
        )
    };
    view.reconcile_for_tests(&mut app.store_mut(), vec![file("a.md", 1), file("b.md", 1)]);
    {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        view.land_for_tests(&mut store, &ui, loc("b.md"), built("b\n", "B\n"));
    }

    // RESTAMP a.md — this runs `refresh_header(a.md)` (the cover
    // resplice), then relaunches; the relaunch lands.
    view.reconcile_for_tests(&mut app.store_mut(), vec![file("a.md", 2), file("b.md", 1)]);
    {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        view.land_for_tests(&mut store, &ui, loc("a.md"), built("a\n", "AA\n"));
    }
    assert_eq!(
        view.probe_cover(&app.store(), &loc("a.md")),
        Some(2),
        "the restamped file keeps its two-row cover"
    );

    // Now RETIRE a.md (it left the change set). Both its rows must go.
    view.reconcile_for_tests(&mut app.store_mut(), vec![file("b.md", 1)]);
    let keys = view.probe_row_keys(&app.store());
    assert!(
        !keys.iter().any(|k| k.contains("a.md")),
        "no a.md row of any kind survives: {keys:?}"
    );
    assert_eq!(
        keys,
        vec!["File:b.md".to_owned(), "Diff:b.md".to_owned()],
        "exactly b.md's header + diff remain: {keys:?}"
    );
}

/// The membership invariant behind the staleness gate: whenever the
/// canvas's stamp equals the feed's generation, its File rows must be
/// exactly the last adopted READY listing. A deterministic random walk
/// over adopt (Ready/Pending/Empty) following the probe discipline —
/// adopt only when stale — with the model checked after every step.
#[test]
fn membership_follows_every_adopted_listing() {
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();

    let pool: Vec<himark::ResourceLocation> = (0..8)
        .map(|index| {
            himark::ResourceLocation::new(
                himark::ResourceType::document(),
                himark::Authority::new("test"),
                vec!["proj".to_owned(), format!("f{index}.md")],
            )
        })
        .collect();
    let source = himark::diff_canvas::CanvasSource::WorkingCopy {
        folder: himark::ResourceLocation::new(
            himark::ResourceType::directory(),
            himark::Authority::new("test"),
            vec!["proj".to_owned()],
        ),
    };
    let view = {
        let mut store = app.store_mut();
        DiffCanvasView::over(&mut store, source)
    };

    let mut lcg: u64 = 0x5eed_cafe;
    let mut rand = move || {
        lcg = lcg
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        lcg >> 33
    };

    let mut generation: u64 = 1;
    let mut seen: Option<u64> = None;
    let mut expected: Option<Vec<String>> = None; // None = never populated
    let ui = himark::test_document::test_ui();

    for step in 0..400 {
        let roll = rand() % 10;
        let listing = match roll {
            0 => himark::diff_canvas::CanvasListing::Pending("computing…".to_owned()),
            1 => himark::diff_canvas::CanvasListing::Empty("no changes".to_owned()),
            _ => {
                let mut files: Vec<himark::diff_canvas::CanvasFile> = pool
                    .iter()
                    .filter(|_| rand() % 2 == 0)
                    .map(|key| canvas_file(key, generation))
                    .collect();
                if files.is_empty() {
                    files.push(canvas_file(&pool[0], generation));
                }
                files.sort_by(|a, b| a.new.name().cmp(b.new.name()));
                himark::diff_canvas::CanvasListing::Ready(files)
            }
        };

        // The probe discipline: refresh only when stale.
        if seen == Some(generation) {
            generation += 1; // the feed always bumps before the next look
            continue;
        }
        // Mirror adopt's stamping rules (stamp only on a consumed listing).
        match &listing {
            himark::diff_canvas::CanvasListing::Pending(_) => {}
            himark::diff_canvas::CanvasListing::Empty(_) => {
                seen = Some(generation);
                if expected.is_some() {
                    expected = Some(Vec::new());
                }
            }
            himark::diff_canvas::CanvasListing::Ready(files) => {
                seen = Some(generation);
                expected = Some(
                    files
                        .iter()
                        .map(|file| format!("File:{}", file.new.name()))
                        .collect(),
                );
            }
        }
        view.adopt_for_tests(&mut app.store_mut(), &ui, generation, listing);

        let rows: Vec<String> = view
            .probe_row_keys(&app.store())
            .into_iter()
            .filter(|key| key.starts_with("File:"))
            .collect();
        if let Some(expected) = &expected {
            assert_eq!(
                &rows, expected,
                "step {step}: rows diverged from the adopted listing (gen {generation})"
            );
        }
        generation += 1;
    }
}

#[test]
fn membership_minimal_repro() {
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let key = |name: &str| {
        himark::ResourceLocation::new(
            himark::ResourceType::document(),
            himark::Authority::new("test"),
            vec!["proj".to_owned(), name.to_owned()],
        )
    };
    let source = himark::diff_canvas::CanvasSource::WorkingCopy {
        folder: himark::ResourceLocation::new(
            himark::ResourceType::directory(),
            himark::Authority::new("test"),
            vec!["proj".to_owned()],
        ),
    };
    let view = {
        let mut store = app.store_mut();
        DiffCanvasView::over(&mut store, source)
    };
    let ui = himark::test_document::test_ui();
    let ready = |names: &[&str], gen: u64| {
        himark::diff_canvas::CanvasListing::Ready(
            names
                .iter()
                .map(|name| canvas_file(&key(name), gen))
                .collect(),
        )
    };

    view.adopt_for_tests(&mut app.store_mut(), &ui, 1, ready(&["f0.md", "f1.md"], 1));
    eprintln!("after populate: {:?}", view.probe_row_keys(&app.store()));

    view.adopt_for_tests(&mut app.store_mut(), &ui, 2, ready(&["f6.md"], 2));
    eprintln!("after swap to f6: {:?}", view.probe_row_keys(&app.store()));

    view.adopt_for_tests(&mut app.store_mut(), &ui, 3, ready(&["f0.md", "f6.md"], 3));
    let keys = view.probe_row_keys(&app.store());
    eprintln!("after f0+f6: {keys:?}");
    assert_eq!(
        keys,
        vec![
            "Banner".to_owned(),
            "File:f0.md".to_owned(),
            "Diff:f0.md".to_owned(),
            "File:f6.md".to_owned(),
            "Diff:f6.md".to_owned(),
        ],
    );
}

/// Canvas rows hold their pairs EMBEDDED: they never surface as
/// family rows (the peeker once drowned in bare "Diff" entries), and
/// a rebuild replaces the standing pair instead of leaking it.
#[test]
fn canvas_pairs_stay_embedded_and_rebuilds_do_not_leak() {
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let (view, key, built2) = seeded_working_canvas(&mut app);

    assert_eq!(
        himark::OpenDocuments::diff_view_count(&app.store()),
        1,
        "one built row, one pair"
    );
    assert!(
        himark::OpenDocuments::pair_ids(&app.store()).is_empty(),
        "an embedded pair is no family row"
    );

    // A host touch rebuilds the row: the landing replaces the pair.
    let launched = view.reconcile_for_tests(&mut app.store_mut(), vec![canvas_file(&key, 2)]);
    assert_eq!(launched, 1);
    {
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        view.land_for_tests(&mut store, &ui, key.clone(), built2);
    }
    assert_eq!(
        himark::OpenDocuments::diff_view_count(&app.store()),
        1,
        "the rebuild replaced the pair — no leak"
    );
    assert!(himark::OpenDocuments::pair_ids(&app.store()).is_empty());
}
