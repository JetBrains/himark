// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Multi-minute diff probes over monster documents — ignored by
//! default; run with `--ignored --nocapture`.

#![allow(unused_imports, dead_code)]

use ::canvas::diff_pane::*;
use himark::app::AppFonts;
use himark::app::Application;
use himark::app_ext::AppExt;
use himark::diff_pane::*;
use std::sync::{mpsc, Arc};
use std::time::Instant;

fn monster_pair(repetitions: usize) -> (String, String) {
    let sample = include_str!("../../../plugins/demo/sample.md");
    let left = sample.repeat(repetitions);
    let mut right = String::with_capacity(left.len());
    for index in 0..repetitions {
        if index % 7 == 3 {
            right.push_str(&sample.replacen("the", "THE—", 2));
        } else if index % 7 == 1 {
            right.push_str(&sample.replacen("| done |", "| WIP |", 1));
        } else {
            right.push_str(sample);
        }
    }
    (left, right)
}

#[test]
#[ignore = "multi-minute probe; run with --ignored --nocapture"]
fn unrelated_pair_syncs_region_scale() {
    let sample = include_str!("../../../plugins/demo/sample.md");
    let left = sample.repeat(1409);
    let mut right = String::with_capacity(left.len());
    let mut index = 0usize;
    while right.len() < left.len() {
        right.push_str(&format!(
            "row {index:07}: entirely unrelated content, no line shared with the sample at all.\n"
        ));
        index += 1;
    }
    idle_pair_probe(left, right, false);
}

#[test]
#[ignore = "multi-second probe; run with --ignored --nocapture"]
fn wall_of_text_diff_stays_region_scale() {
    let unit = "0000 :: lorem ipsum dolor sit amet :: 00 ";
    let mut left = String::new();
    for line in 0..150_000u32 {
        let repeats = 1 + (line % 7) as usize;
        for _ in 0..repeats {
            left.push_str(unit);
        }
        left.push('\n');
    }
    probe_pair(left.clone(), left);
}

#[test]
#[ignore = "multi-second probe; run with --ignored --nocapture"]
fn self_diff_stays_region_scale() {
    let sample = include_str!("../../../plugins/demo/sample.md");
    let left = sample.repeat(1409);
    probe_pair(left.clone(), left);
}

#[test]
#[ignore = "multi-second probe; run with --ignored --nocapture"]
fn idle_diff_goes_quiet() {
    let (left, right) = monster_pair(1409);
    idle_pair_probe(left, right, true);
}

fn idle_pair_probe(left: String, right: String, expect_pairs: bool) {
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
    let theme = editor::theme::Theme::embedded();
    let markdown_fonts = ::editor::test_document::test_fonts_collection().clone();
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(
            &left,
            app.store(),
            &app.ui_ctx(),
            &markdown_fonts,
            &theme
        ),
        "left.md".to_owned(),
        false
    ));
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(
            &right,
            app.store(),
            &app.ui_ctx(),
            &markdown_fonts,
            &theme
        ),
        "right.md".to_owned(),
        false
    ));
    let size = skia_safe::Size::new(2000.0, 1200.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((2000, 1200)).expect("surface");
    let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    for _ in 0..80 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
    }

    let mut quiet = 0usize;
    let started_walk = std::time::Instant::now();
    let mut perform_total = std::time::Duration::ZERO;
    for round in 0..3000 {
        let started = std::time::Instant::now();
        runner.run();
        let mut arrived = 0usize;
        let perform_started = std::time::Instant::now();
        while let Ok(command) = arriving.try_recv() {
            arrived += 1;
            app.perform_batch(vec![command]);
        }
        perform_total += perform_started.elapsed();
        let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
        if round % 25 == 0 {
            eprintln!(
                "[idle] round {round}: {arrived} landings, round {:?}",
                started.elapsed()
            );
        }
        quiet = match arrived {
            0 => quiet + 1,
            _ => 0,
        };
        if quiet >= 3 {
            eprintln!(
                "[idle] QUIET after {round} rounds, walk {:?}, perform total {:?}",
                started_walk.elapsed(),
                perform_total
            );

            super::diffs::assert_pair_consistent(&app, expect_pairs);

            assert!(
                started_walk.elapsed() < std::time::Duration::from_secs(12),
                "the background walk must stay region-scale per landing: {:?}",
                started_walk.elapsed()
            );
            return;
        }
    }
    panic!("never went quiet in 3000 rounds");
}

fn probe(repetitions: usize) {
    let (left, right) = monster_pair(repetitions);
    probe_pair(left, right);
}

fn probe_pair(left: String, right: String) {
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
    let theme = editor::theme::Theme::embedded();
    let markdown_fonts = ::editor::test_document::test_fonts_collection().clone();
    eprintln!("[probe] sides: {} / {} bytes", left.len(), right.len());

    let started = Instant::now();
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(
            &left,
            app.store(),
            &app.ui_ctx(),
            &markdown_fonts,
            &theme
        ),
        "left.md".to_owned(),
        false,
    ));
    assert!(app.add_document(
        app.sole_window(),
        himarkdown::document_from_markdown(
            &right,
            app.store(),
            &app.ui_ctx(),
            &markdown_fonts,
            &theme
        ),
        "right.md".to_owned(),
        false,
    ));
    eprintln!("[probe] documents built+added: {:?}", started.elapsed());

    let size = skia_safe::Size::new(1200.0, 900.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1200, 900)).expect("surface");
    let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);

    let started = Instant::now();
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    let open = started.elapsed();
    eprintln!("[probe] diff.open (sync diff + complete layouts): {open:?}");
    imba::perf::record("diff-open", "open_ms", open.as_secs_f64() * 1000.0);

    let started = Instant::now();
    let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
    eprintln!("[probe] first frame: {:?}", started.elapsed());

    himark::test_driver::click(&mut app, 900.0, 400.0, 1200.0, 900.0);
    let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
    let mut keystrokes = Vec::new();
    for _ in 0..12 {
        let started = Instant::now();
        let _ = himark::test_driver::type_text(&mut app, "x");
        keystrokes.push(started.elapsed());
        let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
    }
    keystrokes.sort();
    let p50 = keystrokes[keystrokes.len() / 2];
    let worst = *keystrokes.last().unwrap();
    eprintln!("[probe] keystroke p50={p50:?} worst={worst:?}");
    imba::perf::record("diff-typing", "p50_ms", p50.as_secs_f64() * 1000.0);

    runner.run();
    while let Ok(command) = arriving.try_recv() {
        app.perform_batch(vec![command]);
    }
    let started = Instant::now();
    for _ in 0..40 {
        let _ = himark::test_driver::scroll(&mut app, 400.0);
        let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
    }
    eprintln!("[probe] 40 scroll frames: {:?}", started.elapsed());

    let mut resize_frames = Vec::new();
    for step in 0..12 {
        let width = 1200.0 - (step as f32 + 1.0) * 20.0;
        let frame_size = skia_safe::Size::new(width, 900.0);
        let started = Instant::now();
        let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), frame_size);
        resize_frames.push(started.elapsed());

        runner.run();
        let land_started = Instant::now();
        let mut landed = 0usize;
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
            landed += 1;
        }
        if landed > 0 {
            eprintln!(
                "[probe] resize step {step}: {landed} landings in {:?}",
                land_started.elapsed()
            );
        }
    }
    resize_frames.sort();
    let resize_p50 = resize_frames[resize_frames.len() / 2];
    let resize_worst = *resize_frames.last().unwrap();
    eprintln!("[probe] resize frame p50={resize_p50:?} worst={resize_worst:?}");
    imba::perf::record(
        "diff-resize",
        "frame_p50_ms",
        resize_p50.as_secs_f64() * 1000.0,
    );
    imba::perf::record(
        "diff-resize",
        "frame_worst_ms",
        resize_worst.as_secs_f64() * 1000.0,
    );

    let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);

    let has_table = right.contains("| Persistent store |");
    if has_table {
        for _ in 0..80 {
            let _ = himark::test_driver::scroll(&mut app, -2000.0);
            let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
        }
        let cell_focused = |app: &Application| {
            let info = documents::OpenDocuments::list(app.store(), app.sole_documents())
                .into_iter()
                .find(|(_, info)| info.name() == "right.md")
                .expect("right open")
                .0;
            {
                let document =
                    documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), info)
                        .expect("document");
                document.editor_ids().any(|editor| {
                    matches!(
                        document.focus(editor),
                        editor::editor_view::EditorFocus::Inlay(_)
                    )
                })
            }
        };

        let table_y = {
            let info = documents::OpenDocuments::list(app.store(), app.sole_documents())
                .into_iter()
                .find(|(_, info)| info.name() == "right.md")
                .expect("right open");
            let document =
                documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), info.0)
                    .expect("document");
            let editor = document.editor_ids().next().expect("an editor");

            let text = {
                let mut view = document.text().view();
                let count = view.byte_count();
                view.byte_string(0, count)
            };
            let layout = document.document_layout(editor).expect("layout");

            let needle = "| Feature | Status |";
            let byte = text
                .match_indices(needle)
                .map(|(at, _)| at as u32)
                .find(|at| {
                    let line_end = text[*at as usize..]
                        .find('\n')
                        .map(|nl| at + nl as u32 + 1)
                        .unwrap_or(*at + needle.len() as u32);
                    layout.height_before(line_end) > layout.height_before(*at)
                })
                .expect("an unfolded sample table") as u32;
            layout.height_before(byte)
        };

        let viewport_start = |app: &Application| -> f32 {
            let info = documents::OpenDocuments::list(app.store(), app.sole_documents())
                .into_iter()
                .find(|(_, info)| info.name() == "right.md")
                .expect("right open");
            let document =
                documents::OpenDocuments::document_ref(app.store(), app.sole_documents(), info.0)
                    .expect("document");
            let editor = document.editor_ids().next().expect("an editor");
            document
                .viewport(editor)
                .map(|viewport| viewport.start)
                .unwrap_or(0.0)
        };

        let _ = himark::test_driver::scroll(&mut app, -1_000_000.0);
        let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
        let _ = himark::test_driver::scroll(&mut app, table_y - 300.0);
        let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
        let _ = viewport_start(&app);
        let mut focused = false;
        'probe: for dy in (-4..=8).map(|step| step * 30) {
            for x in [650, 700, 760, 820, 880, 940, 1000, 1060] {
                let y = 300.0 + 44.0 + dy as f32;
                if !(0.0..890.0).contains(&y) {
                    continue;
                }
                himark::test_driver::click(&mut app, x as f32, y, 1200.0, 900.0);
                let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
                if cell_focused(&app) {
                    focused = true;
                    break 'probe;
                }
            }
        }
        assert!(
            focused,
            "a table cell took focus (table_y {table_y}, viewport {})",
            viewport_start(&app)
        );
        for _ in 0..8 {
            let _ = himark::test_driver::type_text(&mut app, "grow the row substantially ");
            let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
            super::diffs::assert_pair_aligned(&app);
        }
        for _ in 0..2 {
            let _ = himark::test_driver::type_text(&mut app, "\n");
            let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
            super::diffs::assert_pair_aligned(&app);
        }
    }

    for _ in 0..30 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
    }
    super::diffs::assert_pair_aligned(&app);

    for _ in 0..6 {
        let _ = himark::test_driver::type_text(&mut app, "z");
        for _ in 0..4 {
            runner.run();
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
        }
    }
    super::diffs::assert_pair_aligned(&app);

    let mut checked = false;
    app.for_each_plugin_panel(&mut |panel| {
        let Some(panel) = panel.as_any().downcast_ref::<DiffPanelView>() else {
            return;
        };
        let state = panel.diff_state(app.store()).expect("settled");
        let doc_lines = left.lines().count() as u64;
        eprintln!(
            "[probe] ui-synced boundaries: {} (document: {doc_lines} lines)",
            state.ui_synced_boundaries
        );

        assert!(
            state.ui_synced_boundaries < doc_lines * 4,
            "UI-thread spacer sync must stay viewport-scale: {} boundary visits \
             over a {doc_lines}-line document",
            state.ui_synced_boundaries,
        );
        checked = true;
    });
    assert!(checked, "the diff panel was inspected");
}

#[test]
fn diff_panel_survives_a_medium_monster() {
    probe(60);
}

#[test]
#[ignore = "profiling soak; run with --ignored --nocapture and sample it"]
fn scroll_soak_for_profiling() {
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
    let theme = editor::theme::Theme::embedded();
    let markdown_fonts = ::editor::test_document::test_fonts_collection().clone();
    let (left, right) = monster_pair(1409);
    let ui = &app.ui_ctx();
    let (mut ldoc, lblocks) =
        himarkdown::markdown_document(&left, app.store(), ui, &markdown_fonts, &theme);
    demo::inlay::add_badges(
        &mut ldoc,
        &lblocks,
        app.store(),
        ui,
        &markdown_fonts,
        &theme,
    );
    let (mut rdoc, rblocks) =
        himarkdown::markdown_document(&right, app.store(), ui, &markdown_fonts, &theme);
    demo::inlay::add_badges(
        &mut rdoc,
        &rblocks,
        app.store(),
        ui,
        &markdown_fonts,
        &theme,
    );
    assert!(app.add_document(app.sole_window(), ldoc, "left.md".to_owned(), false));
    assert!(app.add_document(app.sole_window(), rdoc, "right.md".to_owned(), false));

    let size = skia_safe::Size::new(2000.0, 1200.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((2000, 1200)).expect("surface");
    let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
    assert!(app.perform_registered(app.sole_window(), "diff.open"));
    for _ in 0..60 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
    }
    eprintln!("[soak] converged; scrolling — sample now");
    let mut paints = Vec::new();
    for frame in 0..3000 {
        let _ = himark::test_driver::scroll(&mut app, 60.0);
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        let started = std::time::Instant::now();
        let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
        paints.push(started.elapsed());
        if frame % 500 == 499 {
            let mut sorted = paints.clone();
            sorted.sort();
            eprintln!(
                "[soak] frame {frame}: paint p50={:?} p95={:?} worst={:?}",
                sorted[sorted.len() / 2],
                sorted[sorted.len() * 95 / 100],
                sorted[sorted.len() - 1],
            );
        }
    }
}

#[test]
#[ignore = "multi-second probe; run with --ignored --nocapture"]
fn diff_panel_full_monster_probe() {
    probe(1409);
}

/// Time each UI-thread piece of ONE canvas-row landing for a monster
/// pair — the freeze hunt: which part of `Landed` must move to the
/// worker.
#[test]
#[ignore = "landing breakdown probe; run with --ignored --nocapture"]
fn canvas_landing_breakdown_probe() {
    let repetitions: usize = std::env::var("HIMARK_PROBE_REPS")
        .ok()
        .and_then(|reps| reps.parse().ok())
        .unwrap_or(8);
    let (left, right) = monster_pair(repetitions);
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    himarkdown::register_handlers(&mut app);
    let (posted, arriving) = mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let theme = editor::theme::Theme::embedded();
    let markdown_fonts = ::editor::test_document::test_fonts_collection().clone();
    eprintln!("[landing] sides: {} / {} bytes", left.len(), right.len());

    let documents = himark::workspace::session_state(app.store(), app.sole_window())
        .expect("the window's session")
        .documents();
    let location = |name: &str| {
        editor::location::ResourceLocation::new(
            editor::location::ResourceType::document(),
            editor::location::Authority::new("probe"),
            vec![name.to_owned()],
        )
    };

    let started = Instant::now();
    let old_doc = himarkdown::document_from_markdown(
        &left,
        app.store(),
        &app.ui_ctx(),
        &markdown_fonts,
        &theme,
    );
    let new_doc = himarkdown::document_from_markdown(
        &right,
        app.store(),
        &app.ui_ctx(),
        &markdown_fonts,
        &theme,
    );
    eprintln!(
        "[landing] build documents (the open effect's WORKER half): {:?}",
        started.elapsed()
    );

    // One add_editor alone, on a scratch clone — the suspected bulk
    // of build_diff_view.
    {
        let mut scratch = old_doc.clone();
        let fonts_collection = editor::env::Fonts::of(app.store())();
        let env_theme = editor::env::Themes::of(app.store());
        let ui = app.ui_handle();
        let mut store = app.store_mut();
        let started = Instant::now();
        let _editor = scratch.add_editor(
            550.0,
            None,
            editor::document::EditorBuild::Bounded,
            &[],
            &mut store,
            &ui,
            &fonts_collection,
            &env_theme,
            &mut imba::effect::Batch::new().effects(),
        );
        eprintln!(
            "[landing]   one add_editor (Bounded): {:?}",
            started.elapsed()
        );
    }

    let started = Instant::now();
    let revision = old_doc.revision();
    let old_id = documents::OpenDocuments::register(
        &mut app.store_mut(),
        documents,
        old_doc,
        Some(location("old.md")),
        "old.md".to_owned(),
        revision,
    );
    let revision = new_doc.revision();
    let new_id = documents::OpenDocuments::register(
        &mut app.store_mut(),
        documents,
        new_doc,
        Some(location("new.md")),
        "new.md".to_owned(),
        revision,
    );
    eprintln!("[landing] register both sides: {:?}", started.elapsed());

    let id = {
        let ui = app.ui_handle();
        let started = Instant::now();
        let id = documents::diff_views::build_diff_view(
            &mut app.store_mut(),
            documents,
            &ui,
            old_id,
            new_id,
            550.0,
            true,
        )
        .expect("the tracked pair");
        eprintln!(
            "[landing] build_diff_view (track + 2 editors + attach): {:?}",
            started.elapsed()
        );
        id
    };

    {
        let ui = app.ui_handle();
        let mut pane = PairPane::over(documents, id);
        let mut batch = imba::effect::Batch::new();
        let started = Instant::now();
        {
            let mut fx = batch.effects();
            imba::View::perform(
                &mut pane,
                &mut app.store_mut(),
                &ui,
                editor::unified_diff::UnifiedDiffCommand::SetLayout(
                    editor::unified_diff::DiffLayout::Inline,
                ),
                &mut fx,
            );
        }
        eprintln!("[landing] SetLayout(Inline): {:?}", started.elapsed());
    }

    let size = skia_safe::Size::new(1200.0, 900.0);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1200, 900)).expect("surface");
    let started = Instant::now();
    let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
    eprintln!(
        "[landing] next frame (batch tail: lanes + normalize launch): {:?}",
        started.elapsed()
    );

    let mut round = 0usize;
    loop {
        let started = Instant::now();
        runner.run();
        let worker = started.elapsed();
        let mut commands = Vec::new();
        while let Ok(command) = arriving.try_recv() {
            commands.push(command);
        }
        if commands.is_empty() {
            break;
        }
        round += 1;
        let count = commands.len();
        let started = Instant::now();
        for command in commands {
            app.perform_batch(vec![command]);
        }
        let landing = started.elapsed();
        let started = Instant::now();
        let _ = app.draw_window_sized(app.sole_window(), surface.canvas(), size);
        eprintln!(
            "[landing] round {round}: worker {worker:?}, {count} landings {landing:?}, frame {:?}",
            started.elapsed()
        );
    }
}
