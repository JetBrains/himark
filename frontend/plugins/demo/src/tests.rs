// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::{
    env,
    time::{Duration, Instant},
};

use himark::AppExt;
use himark::InlayMode;
use skia_safe::{surfaces, textlayout::FontCollection};

use himark::{AppCommand, AppFonts, Application};

struct TestHost {
    arriving: std::sync::mpsc::Receiver<AppCommand>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    wake: std::sync::mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl std::ops::Deref for TestHost {
    type Target = std::sync::mpsc::Receiver<AppCommand>;
    fn deref(&self) -> &Self::Target {
        &self.arriving
    }
}

impl Drop for TestHost {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        let _ = self.wake.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn attach_test_host() -> (Application, TestHost) {
    let (mut app, host) = attach_bare_host();
    app.open_async(
        app.sole_window(),
        "torture sample".to_owned(),
        true,
        Some(himark::ResourceLocation::new(
            himark::ResourceType::document(),
            himark::Authority::new("demo"),
            vec!["torture sample".to_owned()],
        )),
        crate::monster_document,
    );
    (app, host)
}

fn attach_bare_host() -> (Application, TestHost) {
    use std::sync::{atomic::AtomicBool, atomic::Ordering, mpsc, Arc};
    let mut app = Application::new(app_fonts());
    let _ = app.add_window();

    app.register_command(Arc::new(peeker::TogglePeeker));
    let mut languages = himark::SyntaxLanguages::new();
    hirust::register(&mut languages);
    app.register_syntax_languages(himarkdown::markdown_languages(languages));
    let (posted, arriving) = mpsc::channel::<AppCommand>();
    let (signal, work) = mpsc::channel::<()>();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        {
            let signal = signal.clone();
            Arc::new(move || {
                let _ = signal.send(());
            })
        },
    );
    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            runner.run();
            while work.recv().is_ok() {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                runner.run();
            }
        })
    };
    (
        app,
        TestHost {
            arriving,
            stop,
            wake: signal,
            worker: Some(worker),
        },
    )
}

fn heavy() -> std::sync::MutexGuard<'static, ()> {
    static HEAVY: std::sync::Mutex<()> = std::sync::Mutex::new(());
    HEAVY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn registered(app: &Application, name: &str) -> bool {
    himark::OpenDocuments::list(app.store())
        .iter()
        .any(|(_, entity)| entity.name() == name)
}

fn boot() -> (Application, TestHost) {
    let (mut app, arriving) = attach_test_host();

    while !registered(&app, "torture sample") {
        let command = arriving
            .recv_timeout(Duration::from_secs(60))
            .expect("startup documents open");
        app.perform_batch(vec![command]);
    }
    (app, arriving)
}

fn drain_until_quiet(app: &mut Application, arriving: &std::sync::mpsc::Receiver<AppCommand>) {
    while let Ok(command) = arriving.recv_timeout(Duration::from_secs(30)) {
        app.perform_batch(vec![command]);
        if app.unconverged_panes().is_empty() {
            break;
        }
    }
}

#[test]
fn markdown_demo_inlays_do_not_replace_headers() {
    let source = "# Demo header\n\n---\n\nplain text";
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let (mut document, blocks) =
        himarkdown::markdown_document(source, store, ui, &font_collection(), &test_theme());
    crate::add_badges(
        &mut document,
        &blocks,
        store,
        ui,
        &font_collection(),
        &test_theme(),
    );
    let byte_count = document.text().byte_count().min(u32::MAX as usize) as u32;
    let mut counts = [0; 5];
    let heading_end = source.find('\n').expect("heading newline") as u32;

    let demo_markup = crate::inlay::demo_markup();
    let extras = [(
        demo_markup,
        document.feature_markup(demo_markup).expect("demo markup"),
    )];
    for interval in
        himark::OverlaidMarkup::new(document.markup(), &extras).all_inlays_in(0..byte_count)
    {
        counts[mode_index(interval.inlay.mode())] += 1;
        if interval.range.start < heading_end {
            assert!(!matches!(interval.inlay.mode(), InlayMode::Instead(_)));
        }
    }

    assert_eq!(counts, [1, 1, 1, 1, 1]);
}

#[test]
fn typing_markdown_into_the_startup_scratch_styles_it() {
    let (mut app, arriving) = attach_bare_host();
    let mut surface = surfaces::raster_n32_premul((800, 600)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert!(
        app.with_ime_client(app.sole_window(), |_| ()).is_some(),
        "the startup scratch has focus"
    );
    for ch in ["#", " ", "h", "i"] {
        assert!(himark::test_driver::type_text(&mut app, ch));
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if app.focused_document_is_header_at(0) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the scratch's reparse must land and style the heading"
        );
        if let Ok(command) = arriving.recv_timeout(Duration::from_millis(200)) {
            app.perform_batch(vec![command]);
        }
    }
}

#[test]
#[ignore = "writes screenshots into HIMARK_SHOT (a directory) for visual inspection"]
fn dump_rust_split_screenshot() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let Some(dir) = std::env::var_os("HIMARK_SHOT") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("shot dir");
    let (mut app, arriving) = attach_bare_host();
    let source = r#"fn main() {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");
    // Emit the C header next to the sources, for the Swift target to import.
    // Best-effort: a failure here must not block the library build.
    if let Ok(builder) = cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cbindgen::Config::from_root_or_default(&crate_dir))
        .generate()
    {
        builder.write_to_file(format!("{crate_dir}/include/himark.h"));
    }
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
"#;
    let fonts = himark::env::Fonts::of(app.store())();
    let theme = himark::env::Themes::of(app.store());
    let languages = {
        let mut languages = himark::SyntaxLanguages::new();
        hirust::register(&mut languages);
        himarkdown::markdown_languages(languages)
    };
    let document = himark::Document::from_language(
        himark::Text::from_string_exact(source),
        "rs",
        &languages,
        store,
        ui,
        &fonts,
        &theme,
    );
    app.add_document(app.sole_window(), document, "build.rs".to_owned(), true);
    let mut surface = surfaces::raster_n32_premul((1920, 1080)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    app.perform_registered(app.sole_window(), "workbench.split-pane");
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(100));
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    }
    let image = surface.image_snapshot();
    let data = image
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png encode");
    std::fs::write(dir.join("rust-split.png"), data.as_bytes()).expect("write screenshot");
}

#[test]
#[ignore = "writes screenshots into HIMARK_SHOT (a directory) for visual inspection"]
fn dump_workbench_screenshots() {
    let Some(dir) = std::env::var_os("HIMARK_SHOT") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("shot dir");
    let (mut app, arriving) = boot();
    let mut surface = surfaces::raster_n32_premul((1920, 1080)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    drain_until_quiet(&mut app, &arriving);

    let mut shoot = |app: &mut Application, name: &str| {
        himark::Window::draw(app.sole_window(), app, surface.canvas());
        himark::Window::draw(app.sole_window(), app, surface.canvas());
        let image = surface.image_snapshot();
        let data = image
            .encode(None, skia_safe::EncodedImageFormat::PNG, None)
            .expect("png encode");
        std::fs::write(dir.join(name), data.as_bytes()).expect("write screenshot");
    };

    shoot(&mut app, "top.png");
    himark::test_driver::scroll(&mut app, 1100.0);
    shoot(&mut app, "mid.png");
    himark::test_driver::scroll(&mut app, 1300.0);
    shoot(&mut app, "deep.png");

    let mut probe = surfaces::raster_n32_premul((1920, 1080)).expect("probe surface");
    let mut settle = move |app: &mut Application,
                           arriving: &std::sync::mpsc::Receiver<AppCommand>| {
        loop {
            himark::Window::draw(app.sole_window(), app, probe.canvas());
            match arriving.recv_timeout(Duration::from_millis(1200)) {
                Ok(command) => {
                    app.perform_batch(vec![command]);
                }
                Err(_) => break,
            }
        }
        for tick in 0..40 {
            himark::Window::draw(app.sole_window(), app, probe.canvas());
            if !himark::test_driver::animate(
                app,
                imba::anim::AnimationClock::from_millis(tick as f64 * 32.0),
            ) && tick > 1
            {
                break;
            }
        }
    };
    app.perform_registered(app.sole_window(), "toc.toggle");
    settle(&mut app, &arriving);
    shoot(&mut app, "toc.png");
    app.perform_registered(app.sole_window(), "toc.toggle");
    settle(&mut app, &arriving);
    app.perform_registered(app.sole_window(), "peeker.toggle");
    settle(&mut app, &arriving);
    shoot(&mut app, "peeker.png");
    app.perform_registered(app.sole_window(), "peeker.toggle");
    settle(&mut app, &arriving);
}

#[test]
#[ignore = "writes a screenshot to HIMARK_SHOT for visual inspection"]
fn dump_peeker_screenshot() {
    let Some(path) = std::env::var_os("HIMARK_SHOT") else {
        return;
    };
    let mut path = std::path::PathBuf::from(path);
    if path.is_dir() {
        path.push("peeker.png");
    }
    let (mut app, _arriving) = boot();
    let mut surface = surfaces::raster_n32_premul((1920, 1080)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    app.perform_registered(app.sole_window(), "peeker.toggle");
    himark::test_driver::type_text(&mut app, "s");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let image = surface.image_snapshot();
    let data = image
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png encode");
    std::fs::write(path, data.as_bytes()).expect("write screenshot");
}

#[test]
fn background_repair_finishes_the_whole_document_after_a_resize() {
    let (mut app, arriving) = boot();
    let mut surface = surfaces::raster_n32_premul((1280, 900)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    let drain = |app: &mut Application| drain_until_quiet(app, &arriving);
    drain(&mut app);
    assert_eq!(
        app.unconverged_panes(),
        vec![],
        "after startup settles, every pane matches a from-scratch layout"
    );

    let mut small = surfaces::raster_n32_premul((900, 700)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, small.canvas());
    assert!(
        !app.unconverged_panes().is_empty(),
        "right after the resize the tails are still pending"
    );
    drain(&mut app);
    assert_eq!(
        app.unconverged_panes(),
        vec![],
        "the background repair reflows everything beyond the viewport"
    );
}

#[test]
fn the_showcase_opens_styled_when_its_background_build_lands() {
    let (mut app, arriving) = attach_test_host();

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(Instant::now() < deadline, "the showcase build must land");
        let command = arriving
            .recv_timeout(Duration::from_secs(30))
            .expect("startup opens");
        app.perform_batch(vec![command]);
        if registered(&app, "torture sample") {
            break;
        }
    }
    assert!(
        app.focused_document_header_at_start()
            .expect("panes show the showcase")
            .is_some(),
        "the inserted document is already styled — no plain phase"
    );
}

#[test]
fn the_preview_is_a_real_editor_whose_tail_repairs_in_background() {
    let _serialized = heavy();
    let (mut app, arriving) = boot();
    let mut surface = surfaces::raster_n32_premul((1280, 900)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let bounded = peeker::preview_height(&app, app.store()).expect("the torture sample previews");
    assert!(bounded > 0.0);

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let height = peeker::preview_height(&app, app.store()).expect("preview stays");
        if height > 1_000_000.0 {
            break;
        }
        assert!(Instant::now() < deadline, "preview tail must arrive");
        let command = arriving
            .recv_timeout(Duration::from_secs(30))
            .expect("repairs keep coming");
        app.perform_batch(vec![command]);
    }
}

#[test]
fn window_resize_reflows_panes_around_their_viewports() {
    let (mut app, _arriving) = boot();
    let mut surface = surfaces::raster_n32_premul((1280, 900)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let widths_before = app.pane_widths();

    himark::test_driver::scroll_at(&mut app, 300.0, 400.0, 20_000.0);
    let mut small = surfaces::raster_n32_premul((900, 700)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, small.canvas());

    let widths_after = app.pane_widths();
    assert_ne!(widths_before, widths_after, "pane widths follow the window");
    for (before, after) in widths_before.iter().zip(&widths_after) {
        assert!(after < before, "narrower window, narrower editors");
    }

    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let widths_back = app.pane_widths();
    for (original, back) in widths_before.iter().zip(&widths_back) {
        assert!(
            (original - back).abs() < 1.5,
            "widths re-derive from geometry"
        );
    }
}

#[test]
fn split_then_peek_a_file_into_the_focused_pane() {
    let (mut app, _arriving) = boot();
    let mut surface = surfaces::raster_n32_premul((1280, 900)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert_eq!(app.pane_count(), 1, "startup is a single pane");

    assert!(app.perform_registered(app.sole_window(), "workbench.split-pane"));
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    assert_eq!(app.pane_count(), 2);

    let before = app.focused_editor_id();
    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let labels = peeker::labels(&app).expect("peeker is open");
    assert!(
        labels.iter().any(|label| label == "torture sample"),
        "the sample document is listed under its open name: {labels:?}"
    );

    assert!(himark::test_driver::type_text(&mut app, "torture"));
    let labels = peeker::labels(&app).expect("peeker is open");
    assert!(!labels.is_empty());
    assert!(labels.iter().all(|label| {
        let label = label.to_lowercase();
        "torture".chars().all(|wanted| label.contains(wanted))
    }));
    assert!(himark::test_driver::key(
        &mut app,
        imba::event::Key::Enter,
        Default::default()
    ));
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    assert!(peeker::labels(&app).is_none(), "picking closes the peeker");
    assert_eq!(app.pane_count(), 2, "picking replaces, never adds a pane");
    assert_eq!(
        app.focused_editor_id(),
        before,
        "picking the already-shown document navigates IN PLACE — same editor"
    );

    assert!(app.perform_registered(app.sole_window(), "peeker.toggle"));
    assert!(himark::test_driver::key(
        &mut app,
        imba::event::Key::Escape,
        Default::default()
    ));
    assert!(peeker::labels(&app).is_none());
}

#[test]
fn typing_and_scrolling_survive_the_reparse_pipeline() {
    let _serialized = heavy();
    let (mut app, arriving) = boot();
    let mut surface = surfaces::raster_n32_premul((1280, 900)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    himark::test_driver::click(&mut app, 200.0, 200.0, 1280.0, 900.0);
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    let mut seed = 0x9e3779b97f4a7c15u64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..600 {
        match rand() % 10 {
            0 | 1 => {
                let x = (rand() % 1200) as f32 + 40.0;
                let y = (rand() % 800) as f32 + 60.0;
                himark::test_driver::click(&mut app, x, y, 1280.0, 900.0);
            }
            2 | 3 | 4 => {
                let texts = ["# é😀", "x", "\n\n", "*emé*", "🚀", "```\n"];
                himark::test_driver::type_text(&mut app, texts[(rand() % 6) as usize]);
            }
            5 => {
                himark::test_driver::backspace(&mut app);
            }
            6 | 7 | 8 => {
                let delta = if rand() % 3 == 0 { -1500.0 } else { 2500.0 };
                himark::test_driver::scroll_at(
                    &mut app,
                    (rand() % 1200) as f32 + 40.0,
                    (rand() % 800) as f32 + 60.0,
                    delta,
                );
            }
            _ => {
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        }
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    while let Ok(command) = arriving.try_recv() {
        app.perform_batch(vec![command]);
    }
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
}

#[test]
fn scrolls_whole_editor_and_reports_render_time() {
    let _serialized = heavy();

    let (mut app, arriving) = boot();
    drain_until_quiet(&mut app, &arriving);
    let surface_size = profile_surface_size();
    let mut surface = surfaces::raster_n32_premul(surface_size).expect("raster surface");

    let warmup = scroll_pass(&mut app, &mut surface);
    assert!(!warmup.is_empty(), "document should be scrollable");
    assert!(
        warmup.len() < MAX_SCROLL_STEPS,
        "scrolling did not reach the bottom within the step cap"
    );

    let passes = profile_passes();
    let mut best: Vec<Duration> = Vec::new();
    for _ in 0..passes {
        scroll_to_top(&mut app, &mut surface);
        let samples = scroll_pass(&mut app, &mut surface);
        if best.is_empty() {
            best = samples;
        } else {
            assert_eq!(
                best.len(),
                samples.len(),
                "every pass must render the same frames"
            );
            for (best, sample) in best.iter_mut().zip(samples) {
                *best = (*best).min(sample);
            }
        }
    }

    let mut samples = best;
    samples.sort();
    let total: Duration = samples.iter().copied().sum();
    let average = total.as_secs_f64() * 1000.0 / samples.len() as f64;
    let p50 = samples[samples.len() / 2].as_secs_f64() * 1000.0;
    let p95 = samples[samples.len() * 95 / 100].as_secs_f64() * 1000.0;
    let max = samples[samples.len() - 1].as_secs_f64() * 1000.0;

    eprintln!(
        "render scroll: size={}x{}, frames={}, passes={passes}, avg={average:.3}ms, p50={p50:.3}ms, p95={p95:.3}ms, max={max:.3}ms",
        surface_size.0,
        surface_size.1,
        samples.len(),
    );
    imba::perf::record("render-scroll", "avg_ms", average);
    imba::perf::record("render-scroll", "p50_ms", p50);
    imba::perf::record("render-scroll", "p95_ms", p95);
}

#[test]
fn clicking_an_inlay_animates_its_size_through_the_clock() {
    let (mut app, arriving) = boot();
    drain_until_quiet(&mut app, &arriving);
    let mut surface = surfaces::raster_n32_premul((1280, 900)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    while app.styled_pane_count() == 0 {
        let command = arriving
            .recv_timeout(Duration::from_secs(60))
            .expect("the parse lands");
        app.perform_batch(vec![command]);
    }
    drain_until_quiet(&mut app, &arriving);
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    let (x, y) = app
        .first_inlay_probe_point()
        .expect("the showcase has inlays");
    let height_before = app.focused_pane_content_height();
    assert!(
        himark::test_driver::click(&mut app, x, y, 1280.0, 900.0),
        "the click must land"
    );

    assert!(
        himark::test_driver::animate(&mut app, imba::anim::AnimationClock::from_millis(0.0)),
        "a running inlay animation must answer the clock"
    );
    assert!(himark::test_driver::animate(
        &mut app,
        imba::anim::AnimationClock::from_millis(90.0)
    ));
    let height_mid = app.focused_pane_content_height();
    assert_ne!(
        height_mid, height_before,
        "mid-animation the inlay's size shifts the document layout"
    );

    assert!(himark::test_driver::animate(
        &mut app,
        imba::anim::AnimationClock::from_millis(200.0)
    ));
    for _ in 0..3 {
        if !himark::test_driver::animate(&mut app, imba::anim::AnimationClock::from_millis(400.0)) {
            break;
        }
    }
    assert!(
        !himark::test_driver::animate(&mut app, imba::anim::AnimationClock::from_millis(500.0)),
        "a settled animation must be silent"
    );
}

#[test]
fn an_idle_app_answers_the_animation_clock_with_silence() {
    let (mut app, arriving) = boot();
    drain_until_quiet(&mut app, &arriving);
    let mut surface = surfaces::raster_n32_premul((1280, 900)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    for ms in [0.0, 16.7, 33.4, 50.1] {
        assert!(
            !himark::test_driver::animate(&mut app, imba::anim::AnimationClock::from_millis(ms)),
            "an idle app must not answer the clock"
        );
    }
}

#[test]
fn types_into_the_editor_and_reports_latency() {
    let _serialized = heavy();
    let (mut app, arriving) = boot();
    drain_until_quiet(&mut app, &arriving);
    let surface_size = profile_surface_size();
    let mut surface = surfaces::raster_n32_premul(surface_size).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    for _ in 0..20 {
        himark::test_driver::type_text(&mut app, "x");
        himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }

    let mut samples = Vec::new();
    for step in 0..300 {
        let started = Instant::now();
        himark::test_driver::type_text(&mut app, if step % 7 == 0 { " " } else { "y" });
        himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        samples.push(started.elapsed());
    }

    samples.sort();
    let total: Duration = samples.iter().copied().sum();
    let average = total.as_secs_f64() * 1000.0 / samples.len() as f64;
    let p50 = samples[samples.len() / 2].as_secs_f64() * 1000.0;
    let p95 = samples[samples.len() * 95 / 100].as_secs_f64() * 1000.0;
    let max = samples[samples.len() - 1].as_secs_f64() * 1000.0;
    eprintln!(
        "typing: keystrokes={}, avg={average:.3}ms, p50={p50:.3}ms, p95={p95:.3}ms, max={max:.3}ms",
        samples.len(),
    );
    imba::perf::record("burst-typing", "p50_ms", p50);
    imba::perf::record("burst-typing", "p95_ms", p95);
}

#[test]
fn reparse_landings_during_typing_report_latency() {
    let _serialized = heavy();
    let (mut app, arriving) = boot();
    drain_until_quiet(&mut app, &arriving);
    let surface_size = profile_surface_size();
    let mut surface = surfaces::raster_n32_premul(surface_size).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(500));
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }

    let mut samples = Vec::new();
    for _ in 0..300 {
        std::thread::sleep(Duration::from_millis(20));
        let started = Instant::now();
        himark::test_driver::type_text(&mut app, "y");
        himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        samples.push(started.elapsed());
    }
    samples.sort();
    let p50 = samples[samples.len() / 2].as_secs_f64() * 1000.0;
    let p95 = samples[samples.len() * 95 / 100].as_secs_f64() * 1000.0;
    let p99 = samples[samples.len() * 99 / 100].as_secs_f64() * 1000.0;
    let max = samples[samples.len() - 1].as_secs_f64() * 1000.0;
    eprintln!("[probe] styled typing: p50={p50:.2}ms p95={p95:.2}ms p99={p99:.2}ms max={max:.2}ms");
    imba::perf::record("styled-typing", "p50_ms", p50);
    imba::perf::record("styled-typing", "p95_ms", p95);
}

#[test]
#[ignore = "real-window convergence probe; run with --nocapture"]
fn opening_at_window_size_converges_and_styles() {
    let (mut app, arriving) = boot();

    let mut surface = surfaces::raster_n32_premul((2560, 1440)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let mut surface = surfaces::raster_n32_premul((2880, 1620)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match arriving.recv_timeout(Duration::from_secs(10)) {
            Ok(command) => {
                app.perform_batch(vec![command]);
                himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
            }
            Err(_) => {
                eprintln!("[probe] no background results for 10s");
                break;
            }
        }
        if Instant::now() > deadline {
            break;
        }
        if app.unconverged_panes().is_empty() && app.styled_pane_count() > 0 {
            eprintln!("[probe] converged and styled");
            return;
        }
    }
    panic!(
        "did not converge: unconverged={:?} styled={}",
        app.unconverged_panes(),
        app.styled_pane_count()
    );
}

const MAX_SCROLL_STEPS: usize = 10_000;

fn scroll_steps(app: &himark::Application, step: f32) -> usize {
    let extent = himark::OpenDocuments::list(app.store())
        .into_iter()
        .filter_map(|(_, entity)| {
            let document = entity.document();
            let editor = document.editor_ids().next()?;
            Some(document.content_height(editor))
        })
        .fold(0.0, f32::max);
    (((extent / step).ceil() as usize).saturating_add(2)).min(MAX_SCROLL_STEPS - 1)
}

fn scroll_pass(app: &mut Application, surface: &mut skia_safe::Surface) -> Vec<Duration> {
    himark::Window::draw(app.sole_window(), app, surface.canvas());

    let mut samples = Vec::new();
    for _ in 0..scroll_steps(app, 4096.0) {
        let _ = himark::test_driver::scroll(app, 4096.0);

        let started = Instant::now();
        himark::Window::draw(app.sole_window(), app, surface.canvas());
        samples.push(started.elapsed());
    }
    samples
}

fn scroll_to_top(app: &mut Application, surface: &mut skia_safe::Surface) {
    himark::test_driver::scroll(app, f32::MIN);
    himark::Window::draw(app.sole_window(), app, surface.canvas());
}

fn profile_passes() -> usize {
    env::var("HIMARK_PROFILE_PASSES")
        .ok()
        .and_then(|passes| passes.parse().ok())
        .filter(|passes| *passes > 0)
        .unwrap_or(1)
}

fn profile_surface_size() -> (i32, i32) {
    env::var("HIMARK_PROFILE_SURFACE")
        .ok()
        .and_then(|value| {
            value
                .split_once('x')
                .map(|(w, h)| (w.to_owned(), h.to_owned()))
        })
        .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)))
        .filter(|(width, height)| *width > 0 && *height > 0)
        .unwrap_or((1280, 900))
}

trait RunReparse {
    fn run_reparse(self) -> himark::ReparseOutcome;
}

impl RunReparse for himark::ReparseWork {
    fn run_reparse(self) -> himark::ReparseOutcome {
        himark::ReparseHandler(himark::test_support::test_workshop(test_theme())).reparse(self)
    }
}

fn test_theme() -> himark::Theme {
    himark::Theme::embedded()
}

fn font_collection() -> FontCollection {
    himark::test_document::test_fonts_collection().clone()
}

fn app_fonts() -> AppFonts {
    AppFonts::embedded()
}

fn mode_index(mode: InlayMode) -> usize {
    match mode {
        InlayMode::Left => 0,
        InlayMode::Right => 1,
        InlayMode::Under => 2,
        InlayMode::Above => 3,
        InlayMode::Instead(_) => 4,
        InlayMode::Popup(_) => 5,
    }
}

#[test]
fn typing_in_the_monster_document_stays_frame_budgeted() {
    let _serialized = heavy();
    let (mut app, arriving) = boot();
    drain_until_quiet(&mut app, &arriving);
    let mut surface = surfaces::raster_n32_premul((1280, 900)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(250));
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }

    let measure = |app: &mut Application, surface: &mut skia_safe::Surface| {
        let mut samples = Vec::new();
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(10));
            let started = Instant::now();
            himark::test_driver::type_text(app, "y");
            himark::Window::draw(app.sole_window(), app, surface.canvas());
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            himark::Window::draw(app.sole_window(), app, surface.canvas());
            samples.push(started.elapsed());
        }
        samples.sort();
        samples[samples.len() / 2]
    };

    let monster = measure(&mut app, &mut surface);

    assert!(app.new_scratch(app.sole_window()));
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let scratch = measure(&mut app, &mut surface);

    let monster_ms = monster.as_secs_f64() * 1000.0;
    let scratch_ms = scratch.as_secs_f64() * 1000.0;
    eprintln!("[gate] typing p50: monster={monster_ms:.2}ms scratch={scratch_ms:.2}ms");
    imba::perf::record("typing", "monster_p50_ms", monster_ms);
    imba::perf::record("typing", "scratch_p50_ms", scratch_ms);

    let budget = (scratch_ms * 20.0).max(25.0);
    assert!(
        monster_ms <= budget,
        "monster-document keystroke p50 {monster_ms:.2}ms exceeds budget {budget:.2}ms \
         (scratch baseline {scratch_ms:.2}ms) — something per-block landed on the UI thread"
    );
}

#[test]
fn scroll_frame_breakdown() {
    let _serialized = heavy();
    let (mut app, arriving) = boot();
    drain_until_quiet(&mut app, &arriving);
    let (width, height) = profile_surface_size();
    let size = skia_safe::Size::new(width as f32, height as f32);
    let mut surface = surfaces::raster_n32_premul((width, height)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());

    let mut scrolls = Vec::new();
    let mut paints = Vec::new();
    for _ in 0..scroll_steps(&app, 4096.0) {
        let started = Instant::now();
        let _ = himark::test_driver::scroll(&mut app, 4096.0);
        scrolls.push(started.elapsed());
        paints.push(himark::Window::draw_profiled(
            app.sole_window(),
            &mut app,
            surface.canvas(),
            size,
        ));
    }
    let p = |mut samples: Vec<Duration>| {
        samples.sort();
        (
            samples[samples.len() / 2].as_secs_f64() * 1000.0,
            samples[samples.len() * 95 / 100].as_secs_f64() * 1000.0,
        )
    };
    let frames = scrolls.len();
    let (scroll_p50, scroll_p95) = p(scrolls);
    let (paint_p50, paint_p95) = p(paints);
    eprintln!(
        "[bench] monster scroll breakdown: frames={frames} \
         scroll p50={scroll_p50:.3}ms p95={scroll_p95:.3}ms | \
         paint p50={paint_p50:.3}ms p95={paint_p95:.3}ms"
    );
    imba::perf::record("monster-scroll", "paint_p50_ms", paint_p50);
    imba::perf::record("monster-scroll", "paint_p95_ms", paint_p95);
}

#[test]
#[ignore = "panic-hunt sweep; slow — run explicitly with --nocapture"]
fn typing_everywhere_in_the_monster_survives_reparse() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = himark::test_document::test_fonts_collection();
    let mut document = crate::monster_document(store, ui, &fonts, &test_theme());
    let _editor = document.add_editor(
        700.0,
        None,
        ::editor::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let parsers = std::sync::Arc::new({
        let mut languages = himark::SyntaxLanguages::new();
        hirust::register(&mut languages);
        himarkdown::markdown_languages(languages)
    });
    let outcome = himark::ReparseWork::capture(&document, parsers.clone())
        .expect("parse")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );

    let len = document.text().byte_count();
    let bytes = {
        let mut view = document.text().view();
        view.byte_string(0, len)
    };
    let step = (len / 500).max(1);
    let mut tested = 0usize;
    let mut position = 0usize;
    while position < len {
        let mut at = position;
        while at < len && !bytes.is_char_boundary(at) {
            at += 1;
        }
        let mut trial = document.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            trial.edit(
                &operation::Operation::insert_at(at as u32, "x"),
                store,
                ui,
                &fonts,
                &test_theme(),
                &mut imba::effect::Batch::new().effects(),
            );
            let outcome = himark::ReparseWork::capture(&trial, parsers.clone())
                .expect("parse")
                .run_reparse();
            trial.apply_reparse_outcome(
                outcome,
                store,
                ui,
                &fonts,
                &test_theme(),
                &mut imba::effect::Batch::new().effects(),
            );
        }));
        if let Err(payload) = result {
            let context = &bytes[at.saturating_sub(40)..(at + 40).min(len)];
            panic!(
                "typing at byte {at} panicked: {:?}\ncontext: {context:?}",
                payload.downcast_ref::<String>()
            );
        }
        tested += 1;
        position += step;
    }
    eprintln!("[sweep] {tested} positions typed and reparsed without panic");
}

#[test]
fn the_wall_of_text_opens_and_types() {
    let _serialized = heavy();
    let (mut app, host) = attach_bare_host();
    let opened = Instant::now();
    app.open_async(
        app.sole_window(),
        "wall of text".to_owned(),
        true,
        Some(himark::ResourceLocation::new(
            himark::ResourceType::document(),
            himark::Authority::new("demo"),
            vec!["wall of text".to_owned()],
        )),
        |_store, _ui, _fonts, _theme| crate::wall_of_text(20_000),
    );
    while !registered(&app, "wall of text") {
        let command = host
            .arriving
            .recv_timeout(Duration::from_secs(120))
            .expect("wall opens");
        app.perform_batch(vec![command]);
    }
    imba::perf::record("wall", "open_ms", opened.elapsed().as_secs_f64() * 1000.0);

    let mut surface = surfaces::raster_n32_premul((1280, 900)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    himark::test_driver::click(&mut app, 400.0, 300.0, 1280.0, 900.0);

    let mut samples = Vec::new();
    for _ in 0..24 {
        let started = Instant::now();
        himark::test_driver::type_text(&mut app, "x");
        himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
        samples.push(started.elapsed());
    }
    samples.sort();
    let p50 = samples[samples.len() / 2].as_secs_f64() * 1000.0;
    eprintln!("[bench] wall typing p50={p50:.3}ms");
    imba::perf::record("wall", "typing_p50_ms", p50);
    assert!(
        p50 < 50.0,
        "typing in the wall must stay per-line: p50 {p50:.3}ms"
    );
}

#[test]
fn resize_repair_matches_fresh_layout() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = himark::test_document::test_fonts_collection();
    let theme = himark::Theme::embedded();
    let (mut document, blocks) =
        himarkdown::markdown_document(&crate::SAMPLE.repeat(2), store, ui, &fonts, &theme);
    crate::add_badges(&mut document, &blocks, store, ui, &fonts, &theme);
    let editor = document.add_editor(
        900.0,
        None,
        ::editor::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let mut batch = imba::effect::Batch::new();
    document.resize(
        editor,
        1128.0,
        0,
        store,
        ui,
        &fonts,
        &theme,
        &mut batch.effects(),
    );
    let workshop = himark::test_support::test_workshop(theme.clone());
    for effect in himark::test_support::surviving_launches(batch) {
        if let himark::EditorCommand::ApplyRepair(items) =
            himark::test_support::handle_effect(effect, &workshop)
        {
            for item in items {
                document.apply_repair(item);
            }
        }
    }

    let live = document.element_heights(editor);
    let fresh = himark::EditorView::complete(document.clone(), 1128.0, store, ui, &fonts, &theme)
        .element_heights();
    for (index, (a, b)) in live.iter().zip(fresh.iter()).enumerate() {
        assert_eq!(a, b, "element {index} diverged (live vs fresh)");
    }
    assert_eq!(live.len(), fresh.len(), "element counts diverged");
}

#[test]
#[ignore = "divergence probe; run with --nocapture"]
fn where_do_heights_diverge() {
    let (mut app, arriving) = boot();
    drain_until_quiet(&mut app, &arriving);
    let mut surface = surfaces::raster_n32_premul((1280, 900)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    drain_until_quiet(&mut app, &arriving);
    let (live, fresh, text) = app.first_pane_heights_vs_fresh();
    let zero_sized = |elements: &[(u32, f32)]| -> Vec<(u32, f32)> {
        elements
            .windows(2)
            .filter(|pair| pair[0].0 == pair[1].0)
            .map(|pair| pair[0])
            .collect()
    };
    let live_ghosts = zero_sized(&live);
    let fresh_ghosts = zero_sized(&fresh);
    eprintln!(
        "[ghosts] live={} fresh={}",
        live_ghosts.len(),
        fresh_ghosts.len()
    );
    for (a, b) in live_ghosts.iter().zip(fresh_ghosts.iter()) {
        if a != b {
            eprintln!("[ghosts] first mismatch: live {a:?} fresh {b:?}");
            break;
        }
    }
    let mut divergences = 0;
    for (index, (a, b)) in live.iter().zip(fresh.iter()).enumerate() {
        if (a.0 != b.0) || (a.1 - b.1).abs() > 0.5 {
            let at = a.0 as usize;
            eprintln!(
                "[diverge] element {index}: live ({}, {}) fresh ({}, {}) text {:?}",
                a.0,
                a.1,
                b.0,
                b.1,
                &text[at.saturating_sub(60).min(text.len())..(at + 60).min(text.len())]
            );
            divergences += 1;
            if divergences > 5 {
                break;
            }
        }
    }
    eprintln!(
        "[diverge] total elements live={} fresh={}",
        live.len(),
        fresh.len()
    );
}

#[test]
fn tree_demo_panel_toggles_through_clicks() {
    let fonts = himark::test_document::test_fonts_collection();
    let _ = fonts;
    let (mut app, _arriving) = boot();

    himark::AppExt::register_command(&mut app, std::sync::Arc::new(crate::OpenTreeDemo));
    let window = app.sole_window();
    let mut surface = surfaces::raster_n32_premul((800, 600)).expect("surface");
    himark::Window::draw(window, &mut app, surface.canvas());
    assert!(himark::AppExt::perform_registered(
        &mut app,
        window,
        "demo.tree"
    ));
    himark::Window::draw(window, &mut app, surface.canvas());

    let row_count = |app: &himark::Application| -> usize {
        let mut count = None;
        app.for_each_plugin_panel(&mut |panel| {
            if let Some(tree) = panel.as_any().downcast_ref::<crate::TreeDemoView>() {
                count = Some(tree.row_count());
            }
        });
        count.expect("the tree panel is up")
    };
    assert_eq!(row_count(&app), 100_000);

    let theme = himark::Theme::embedded();
    let chrome = theme.ui();
    let content_height = 600.0 - chrome.toolbar.height;
    let top = chrome.toolbar.height
        + himark::workbench_geometry(800.0, content_height, &chrome.window).top;
    let first_row = skia_safe::Point::new(30.0, top + 13.0);

    let started = std::time::Instant::now();
    app.dispatch(
        window,
        imba::event::Event::MouseDown {
            mods: Default::default(),
            point: first_row,
            button: imba::event::MouseButton::Left,
            count: 1,
        },
        skia_safe::Size::new(800.0, 600.0),
    );
    let toggle_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(row_count(&app), 100_010, "the first root expanded");
    himark::Window::draw(window, &mut app, surface.canvas());

    let content_height = |app: &himark::Application| -> f32 {
        let mut height = None;
        app.for_each_plugin_panel(&mut |panel| {
            if let Some(tree) = panel.as_any().downcast_ref::<crate::TreeDemoView>() {
                height = Some(tree.content_height());
            }
        });
        height.expect("the tree panel is up")
    };
    let mut extents = Vec::new();
    for at_ms in [0.0, 60.0, 120.0, 400.0, 420.0] {
        app.dispatch(
            window,
            imba::event::Event::AnimationClock {
                now: imba::anim::AnimationClock::from_millis(at_ms),
            },
            skia_safe::Size::new(800.0, 600.0),
        );
        himark::Window::draw(window, &mut app, surface.canvas());
        extents.push(content_height(&app));
    }
    let settled = 100_010.0 * 26.0;
    assert!(
        (extents[0] - (100_000.0 * 26.0)).abs() < 40.0,
        "the block enters at the collapsed extent: {extents:?}"
    );
    assert!(
        extents[1] > extents[0] + 20.0 && extents[1] < settled - 20.0,
        "mid-flight the block is growing: {extents:?}"
    );
    assert!(
        (extents[4] - settled).abs() < 1.0,
        "the animation settles at the expanded extent: {extents:?}"
    );

    app.dispatch(
        window,
        imba::event::Event::MouseDown {
            mods: Default::default(),
            point: first_row,
            button: imba::event::MouseButton::Left,
            count: 1,
        },
        skia_safe::Size::new(800.0, 600.0),
    );
    assert_eq!(row_count(&app), 100_000, "the root collapsed");
    eprintln!("[probe] tree demo toggle over 100k rows: {toggle_ms:.2}ms");
    imba::perf::record("tree-demo", "toggle_ms", toggle_ms);
}

#[test]
fn rust_document_settles_and_stops_reconciling() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let _guard = heavy();
    let (mut app, host) = attach_bare_host();
    let source = include_str!("../../../editor/src/document.rs");
    let fonts = himark::env::Fonts::of(app.store())();
    let theme = himark::env::Themes::of(app.store());
    let languages = {
        let mut languages = himark::SyntaxLanguages::new();
        hirust::register(&mut languages);
        himarkdown::markdown_languages(languages)
    };
    let document = himark::Document::from_language(
        himark::Text::from_string_exact(source),
        "rs",
        &languages,
        store,
        ui,
        &fonts,
        &theme,
    );
    app.add_document(app.sole_window(), document, "document.rs".to_owned(), true);

    for (w, h) in [(1920.0f32, 1080.0f32), (1100.0, 763.0)] {
        let size = skia_safe::Size::new(w, h);
        let mut surface = surfaces::raster_n32_premul((w as i32, h as i32)).expect("surface");
        let settle = |app: &mut Application, surface: &mut skia_safe::Surface, tag: &str| {
            let mut settled_streak = 0;
            for frame in 0..1500 {
                let mut landed = false;
                while let Ok(command) = host.arriving.try_recv() {
                    landed = true;
                    app.perform_batch(vec![command]);
                }
                let reconciling =
                    himark::Window::draw_with_size(app.sole_window(), app, surface.canvas(), size);
                if reconciling || landed {
                    settled_streak = 0;
                } else {
                    settled_streak += 1;
                }
                if settled_streak >= 30 {
                    eprintln!("[probe] {w}x{h} {tag}: settled after {frame} frames");
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            panic!("{w}x{h} {tag}: 1500 frames and the window still reconciles");
        };
        settle(&mut app, &mut surface, "open");

        for hop in 0..8 {
            let gesture = imba::event::ScrollGesture::default();
            app.dispatch(
                app.sole_window(),
                imba::event::Event::Scroll {
                    delta_x: 0.0,
                    point: skia_safe::Point::new(w * 0.5, h * 0.5),
                    delta_y: -3000.0,
                    gesture: &gesture,
                },
                size,
            );
            let _ = hop;
            let _ =
                himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
        }
        settle(&mut app, &mut surface, "scrolled");

        let document = himark::Document::from_language(
            himark::Text::from_string_exact(source),
            "rs",
            &languages,
            store,
            ui,
            &fonts,
            &theme,
        );
        app.perform_batch(vec![AppCommand::Opened(
            app.sole_window(),
            himark::OpenedDocument {
                name: "document-target.rs".to_owned(),
                document,
                location: None,
                primary: true,
                target: Some(
                    himark::LineCol { line: 1830, col: 0 }..himark::LineCol { line: 1830, col: 4 },
                ),
                focus: false,
            },
        )]);
        let mut quiet_streak = 0;
        let mut clock_settled = false;
        for frame in 0..1500 {
            let mut landed = false;
            while let Ok(command) = host.arriving.try_recv() {
                landed = true;
                app.perform_batch(vec![command]);
            }
            let animating = app.dispatch(
                app.sole_window(),
                imba::event::Event::AnimationClock {
                    now: imba::anim::AnimationClock::from_millis(frame as f64 * 16.0),
                },
                size,
            );
            let reconciling =
                himark::Window::draw_with_size(app.sole_window(), &mut app, surface.canvas(), size);
            if reconciling || landed || animating {
                quiet_streak = 0;
            } else {
                quiet_streak += 1;
            }
            if quiet_streak >= 30 {
                eprintln!("[probe] {w}x{h} reveal: settled after {frame} frames");
                clock_settled = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            clock_settled,
            "{w}x{h}: the reveal never settles — the animation clock ticks forever"
        );
    }
}

#[test]
fn typing_after_deleting_everything_costs_what_a_scratch_costs() {
    let _serialized = heavy();
    let (mut app, arriving) = boot();
    drain_until_quiet(&mut app, &arriving);
    let mut surface = surfaces::raster_n32_premul((1280, 900)).expect("raster surface");
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(250));
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }

    let measure = |app: &mut Application, surface: &mut skia_safe::Surface| {
        let mut samples = Vec::new();
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(10));
            let started = Instant::now();
            himark::test_driver::type_text(app, "y");
            himark::Window::draw(app.sole_window(), app, surface.canvas());
            while let Ok(command) = arriving.try_recv() {
                app.perform_batch(vec![command]);
            }
            himark::Window::draw(app.sole_window(), app, surface.canvas());
            samples.push(started.elapsed());
        }
        samples.sort();
        samples[samples.len() / 2]
    };

    let monster = measure(&mut app, &mut surface);

    assert!(app.perform_registered(app.sole_window(), "editor.select-all"));
    assert!(himark::test_driver::backspace(&mut app));
    for _ in 0..8 {
        std::thread::sleep(Duration::from_millis(250));
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    }
    {
        let (document_id, _) = app.focused_editor_id();
        let document = himark::OpenDocuments::document(app.store(), document_id)
            .expect("the focused document");
        assert_eq!(
            document.text().view().byte_count(),
            0,
            "the delete emptied the text"
        );
        assert_eq!(
            document.markup().query_count(),
            0,
            "and took the markup's intervals with it"
        );
    }
    let emptied = measure(&mut app, &mut surface);

    assert!(app.new_scratch(app.sole_window()));
    himark::Window::draw(app.sole_window(), &mut app, surface.canvas());
    let scratch = measure(&mut app, &mut surface);

    let monster_ms = monster.as_secs_f64() * 1000.0;
    let emptied_ms = emptied.as_secs_f64() * 1000.0;
    let scratch_ms = scratch.as_secs_f64() * 1000.0;
    eprintln!(
        "[gate] typing p50: monster={monster_ms:.2}ms emptied={emptied_ms:.2}ms \
         scratch={scratch_ms:.2}ms"
    );
    imba::perf::record("typing", "emptied_monster_p50_ms", emptied_ms);

    let budget = (scratch_ms * 20.0).max(25.0);
    assert!(
        emptied_ms <= budget,
        "typing into the emptied monster cost {emptied_ms:.2}ms against a \
         {scratch_ms:.2}ms scratch keystroke (budget {budget:.2}ms): the \
         deleted text left its markers behind"
    );
}
