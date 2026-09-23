// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::ptr::{null, null_mut};

mod fake_host {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    /// One PICKER SEAT per engine, hung off the callbacks' `ctx` —
    /// no process-global statics, so parallel (or merely successive)
    /// tests cannot bleed request ids into each other. Reads CONSUME
    /// (`take_*`), so a second pick in the same test must wait for
    /// its own request instead of replaying the first.
    pub struct Seat {
        pub picks: AtomicU64,
        pub save_picks: Mutex<(u64, String)>,
        pub fetches: Mutex<Vec<(u64, String)>>,
        pub stores: Mutex<Vec<(u64, String, String)>>,
        pub lists: Mutex<Vec<(u64, String)>>,
    }

    impl Seat {
        pub fn new() -> Arc<Seat> {
            Arc::new(Seat {
                picks: AtomicU64::new(0),
                save_picks: Mutex::new((0, String::new())),
                fetches: Mutex::new(Vec::new()),
                stores: Mutex::new(Vec::new()),
                lists: Mutex::new(Vec::new()),
            })
        }

        /// The raw ctx for `HimarkHostCallbacks` — the test keeps the
        /// `Arc` alive for the engine's whole life.
        pub fn ctx(self: &Arc<Self>) -> *mut std::ffi::c_void {
            Arc::as_ptr(self) as *mut std::ffi::c_void
        }

        pub fn take_pick(&self) -> u64 {
            self.picks.swap(0, Ordering::SeqCst)
        }

        pub fn take_save_pick(&self) -> (u64, String) {
            std::mem::take(&mut *self.save_picks.lock().expect("save picks"))
        }

        unsafe fn of<'a>(ctx: *mut std::ffi::c_void) -> &'a Seat {
            &*(ctx as *const Seat)
        }
    }

    unsafe fn joined_path(location: *const crate::HimarkLocation) -> String {
        let location = crate::host::location_from_abi(&*location).expect("a valid location");
        location.path().join("/")
    }

    pub unsafe extern "C" fn pick_files(ctx: *mut std::ffi::c_void, request: u64, _window: u64) {
        Seat::of(ctx).picks.store(request, Ordering::SeqCst);
    }

    pub unsafe extern "C" fn pick_save(
        ctx: *mut std::ffi::c_void,
        request: u64,
        suggested: *const std::ffi::c_char,
        suggested_len: usize,
    ) {
        let name = match suggested.is_null() {
            true => String::new(),
            false => {
                let bytes = std::slice::from_raw_parts(suggested as *const u8, suggested_len);
                String::from_utf8_lossy(bytes).into_owned()
            }
        };
        *Seat::of(ctx).save_picks.lock().expect("save picks") = (request, name);
    }

    pub unsafe extern "C" fn fetch_document(
        ctx: *mut std::ffi::c_void,
        request: u64,
        location: *const crate::HimarkLocation,
    ) {
        Seat::of(ctx)
            .fetches
            .lock()
            .expect("fetches")
            .push((request, joined_path(location)));
    }

    pub unsafe extern "C" fn store_document(
        ctx: *mut std::ffi::c_void,
        request: u64,
        location: *const crate::HimarkLocation,
        text: *const std::ffi::c_char,
        text_len: usize,
    ) {
        let text = std::str::from_utf8(std::slice::from_raw_parts(text.cast(), text_len))
            .expect("utf8 text")
            .to_owned();
        Seat::of(ctx)
            .stores
            .lock()
            .expect("stores")
            .push((request, joined_path(location), text));
    }

    pub unsafe extern "C" fn list_directory(
        ctx: *mut std::ffi::c_void,
        request: u64,
        location: *const crate::HimarkLocation,
    ) {
        Seat::of(ctx)
            .lists
            .lock()
            .expect("lists")
            .push((request, joined_path(location)));
    }
}

static HOSTED: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct HostedFs {
    root: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl HostedFs {
    fn doc(&self, rel: &[&str]) -> himark::ResourceLocation {
        self.location(himark::ResourceType::document(), rel)
    }

    fn dir(&self, rel: &[&str]) -> himark::ResourceLocation {
        self.location(himark::ResourceType::directory(), rel)
    }

    fn location(&self, kind: himark::ResourceType, rel: &[&str]) -> himark::ResourceLocation {
        let mut path: Vec<String> = self
            .root
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(segment) => {
                    Some(segment.to_string_lossy().into_owned())
                }
                _ => None,
            })
            .collect();
        path.extend(rel.iter().map(|segment| segment.to_string()));
        himark::ResourceLocation::new(kind, himark::Authority::new("local"), path)
    }

    fn path(&self, rel: &[&str]) -> std::path::PathBuf {
        rel.iter()
            .fold(self.root.clone(), |path, seg| path.join(seg))
    }

    fn write(&self, rel: &[&str], text: &str) {
        self.write_bytes(rel, text.as_bytes());
    }

    /// ATOMIC write (temp + rename), like a real editor's save. A bare
    /// `fs::write` truncates then fills, and the host's mirror watcher
    /// (server.rs `reload_mirror`) can read the file in that empty
    /// window and clobber the synced channel to empty — the intermittent
    /// "document opened empty" flake. Rename replaces in one step, so
    /// the watcher only ever sees whole content.
    fn write_bytes(&self, rel: &[&str], bytes: &[u8]) {
        let path = self.path(rel);
        let parent = path.parent().expect("a parent");
        std::fs::create_dir_all(parent).expect("mkdir");
        let tmp = parent.join(format!(
            ".{}.tmp{}",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("f"),
            std::process::id()
        ));
        std::fs::write(&tmp, bytes).expect("write temp");
        std::fs::rename(&tmp, &path).expect("atomic rename");
    }

    fn read(&self, rel: &[&str]) -> Option<String> {
        std::fs::read_to_string(self.path(rel)).ok()
    }
}

/// The hosted guard plus THIS engine's picker seat.
struct Hosted {
    _guard: std::sync::MutexGuard<'static, ()>,
    seat: std::sync::Arc<fake_host::Seat>,
}

/// The picker request for THIS seat — pumping settles until the
/// async picker effect actually fires, then CONSUMING the id. This
/// is what the old one-settle-then-read-a-static could not promise.
fn pick_request(engine: &mut HimarkEngine, seat: &fake_host::Seat) -> u64 {
    for _ in 0..400 {
        let request = seat.take_pick();
        if request != 0 {
            return request;
        }
        settle(engine);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the picker request never reached the host");
}

fn save_pick_request(engine: &mut HimarkEngine, seat: &fake_host::Seat) -> (u64, String) {
    for _ in 0..400 {
        let (request, name) = seat.take_save_pick();
        if request != 0 {
            return (request, name);
        }
        settle(engine);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the save panel request never reached the host");
}

fn hosted_engine() -> (Hosted, HimarkEngine, u64, HostedFs) {
    hosted_engine_with_language_servers(Vec::new())
}

fn hosted_engine_with_language_servers(
    language_servers: Vec<agent_host::LanguageServer>,
) -> (Hosted, HimarkEngine, u64, HostedFs) {
    let host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let picker = fake_host::Seat::new();
    let dir = tempfile::tempdir().expect("hosted fs");

    std::env::set_var("HIMARK_HOST_HOME", dir.path().join("home"));

    let socket = dir.path().join("backend.sock");
    let serving = socket.clone();
    let config = agent_host::HostConfig {
        agents: Vec::new(),
        data_dir: dir.path().join("data"),
        claude_binary: "false".to_owned(),
        codex_binary: "false".to_owned(),
        claude_home: dir.path().join("dot-claude"),
        codex_home: dir.path().join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers,
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("backend runtime");
        let backend = agent_host::Host::new(config);
        let _ = runtime.block_on(backend.bind(&serving));
    });
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 500, "the backend never bound its socket");
    }
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let seat: Arc<dyn himark::higent::AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(),
        crate::test_connector(),
        format!("unix:{}", socket.display()),
    ));
    let local = engine.register_agent_server("Local Backend", seat);
    engine.set_local_backend(local);
    engine.set_host(HimarkHostCallbacks {
        ctx: picker.ctx(),
        pick_files: Some(fake_host::pick_files),
        pick_save: Some(fake_host::pick_save),
        fetch_document: Some(fake_host::fetch_document),
        store_document: Some(fake_host::store_document),
        list_directory: Some(fake_host::list_directory),
        subscribe: None,
        unsubscribe: None,
        set_clipboard: None,
    });

    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    let root = dir.path().join("files");
    std::fs::create_dir_all(&root).expect("files root");
    let fs = HostedFs { root, _dir: dir };
    (
        Hosted {
            _guard: host,
            seat: picker,
        },
        engine,
        window,
        fs,
    )
}

fn settle_until(
    engine: &mut HimarkEngine,
    what: &str,
    mut done: impl FnMut(&mut HimarkEngine) -> bool,
) {
    // Wall-clock bounded, generous: CI runs one nextest process per
    // test in parallel across the suite, and a starved executor slice
    // can stretch an effect chain far past what a quiet machine needs.
    let deadline = std::time::Duration::from_secs(30);
    let started = std::time::Instant::now();
    while started.elapsed() < deadline {
        settle(engine);
        if done(engine) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!(
        "never settled: {what} (waited {:?} — a full deadline means a          parked effect, not a slow one)",
        started.elapsed()
    );
}

fn settle_into_session(engine: &mut HimarkEngine) {
    settle_until(engine, "the folder session opened", |engine| {
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .is_some_and(|entity| entity.current_session().names_session())
    });
}

fn open_picked(
    engine: &mut HimarkEngine,
    seat: &fake_host::Seat,
    window: u64,
    fs: &HostedFs,
    rel: &[&str],
    body: &str,
) {
    fs.write(rel, body);
    assert!(engine.perform_command(window, "file.open"));
    settle(engine);
    let request = pick_request(engine, seat);
    assert!(engine.host_picked(request, vec![fs.doc(rel)]));
    let probe: String = body.chars().take(24).collect();
    settle_until(engine, "the picked file opened", |engine| {
        engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_some_and(|text| text.contains(&probe))
    });
}

fn settle(engine: &mut HimarkEngine) {
    for _ in 0..4 {
        engine.worker().run_pending();
        engine.drain();
    }
}

#[test]
fn plain_chars_over_the_peeker_stay_unconsumed() {
    let (_host, mut engine, window, _fs) = hosted_engine();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");

    assert!(
        engine.key_down(window, u32::from('p'), HIMARK_MOD_COMMAND),
        "cmd-p opens the peeker"
    );
    settle(&mut engine);
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);

    assert!(
        !engine.key_down(window, u32::from('x'), 0),
        "a plain char over the peeker must stay unconsumed — the shell's text path follows"
    );
    assert!(
        engine.text_input(window, "x"),
        "the text lands in the query input"
    );
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);

    assert!(
        engine.key_down(window, HIMARK_KEY_BACKSPACE, 0),
        "backspace reaches the query input via the keymap"
    );
    assert!(
        engine.key_down(window, HIMARK_KEY_LEFT, 0),
        "a caret motion reaches the query input via the keymap"
    );
}

#[test]
fn probe_scratch_after_wall_close() {
    let (_host, mut engine, window, _fs) = hosted_engine();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);

    assert!(engine.text_input(window, "hello scratch line one"));
    assert!(engine.key_down(window, HIMARK_KEY_ENTER, 0));
    assert!(engine.text_input(window, "second line"));
    settle(&mut engine);
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);

    engine.open_demo_wall(window);
    for _ in 0..60 {
        settle(&mut engine);
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        if engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: 64,
                },
            )
            .is_some_and(|text| !text.contains("hello scratch"))
        {
            break;
        }
    }
    assert!(
        engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: 64
                }
            )
            .is_some_and(|text| text.contains("line 0000")),
        "the wall is showing"
    );

    for _ in 0..5 {
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        settle(&mut engine);
    }
    assert!(engine.perform_command(window, "workbench.close"));
    settle(&mut engine);
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
    settle(&mut engine);

    let text = engine
        .substring(
            window,
            HimarkRange {
                start: 0,
                length: u32::MAX,
            },
        )
        .expect("the scratch is back");
    assert!(text.contains("hello scratch"), "scratch content: {text:?}");
    let (document_id, editor_id) = engine.app.focused_editor_id();
    let document =
        himark::OpenDocuments::document_ref(engine.app.store(), document_id).expect("document");
    let heights = document.element_heights(editor_id);
    let tallest = heights
        .iter()
        .map(|(_, height)| *height)
        .fold(0.0f32, f32::max);
    assert!(tallest < 120.0, "no scratch element balloons: {heights:?}");
    surface.canvas().clear(skia_safe::Color::WHITE);
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
    let image = surface.image_snapshot();
    let pixels = image.peek_pixels().expect("pixels");
    let mut inked = 0usize;
    for y in 60..300 {
        for x in 150..900 {
            let color: skia_safe::Color = pixels.get_color((x, y));
            if color.r() < 200 || color.g() < 200 || color.b() < 200 {
                inked += 1;
            }
        }
    }
    assert!(
        inked > 300,
        "the scratch paints its text: {inked} inked pixels"
    );
}

#[test]
fn probe_history_reopen_layout() {
    let (_host, mut engine, window, fs) = hosted_engine();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);

    let body_a: String = (0..300)
        .map(|i| format!("// a longer comment line number {i} with enough words to be real\nfn item_{i}() {{}}\n\n"))
        .collect();
    open_picked(&mut engine, &_host.seat, window, &fs, &["a.rs"], &body_a);
    let _ = engine.draw(window, surface_stub(), 1100.0, 800.0, 1.0);
    settle(&mut engine);

    let _ = himark::test_driver::scroll(&mut engine.app, 6000.0);
    let _ = engine.draw(window, surface_stub(), 1100.0, 800.0, 1.0);
    let _ = himark::test_driver::click(&mut engine.app, 400.0, 400.0, 1100.0, 800.0);
    let _ = engine.draw(window, surface_stub(), 1100.0, 800.0, 1.0);
    settle(&mut engine);

    engine.open_demo_wall(window);
    for _ in 0..40 {
        settle(&mut engine);
        let _ = engine.draw(window, surface_stub(), 1100.0, 800.0, 1.0);
        if engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: 64,
                },
            )
            .is_some_and(|text| !text.contains("fn item_0"))
        {
            break;
        }
    }
    assert!(engine.perform_command(window, "workbench.close"));

    settle_until(&mut engine, "the walk re-opened a.rs", |engine| {
        let _ = engine.draw(window, surface_stub(), 1100.0, 800.0, 1.0);
        engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_some_and(|text| text.contains("fn item_0"))
    });
    let _ = engine.draw(window, surface_stub(), 1100.0, 800.0, 1.0);

    let text = engine
        .substring(
            window,
            HimarkRange {
                start: 0,
                length: u32::MAX,
            },
        )
        .expect("a.rs shows");
    assert!(text.contains("fn item_0"), "a.rs is back: {text:?}");
    let (document_id, editor_id) = engine.app.focused_editor_id();
    {
        let document =
            himark::OpenDocuments::document_ref(engine.app.store(), document_id).expect("document");
        let heights = document.element_heights(editor_id);
        let tallest = heights
            .iter()
            .map(|(_, height)| *height)
            .fold(0.0f32, f32::max);
        assert!(
            tallest < 120.0,
            "no element balloons (a short source line is 1-2 rows): {heights:?}"
        );
    }

    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    surface.canvas().clear(skia_safe::Color::WHITE);
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
    let image = surface.image_snapshot();
    let pixels = image.peek_pixels().expect("pixels");
    let mut inked = 0usize;
    for y in 60..400 {
        for x in 200..900 {
            let color: skia_safe::Color = pixels.get_color((x, y));
            if color.r() < 200 || color.g() < 200 || color.b() < 200 {
                inked += 1;
            }
        }
    }
    assert!(
        inked > 500,
        "the re-mounted document paints its text: {inked} inked pixels"
    );
}

fn surface_stub() -> &'static skia_safe::Canvas {
    let surface = Box::leak(Box::new(
        skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface"),
    ));
    surface.canvas()
}

#[test]
fn the_file_picker_round_trip_opens_the_picked_files() {
    let (_host, mut engine, window, fs) = hosted_engine();
    fs.write(&["docs", "picked.md"], "# picked\n\nhello");
    fs.write(&["docs", "second.md"], "second");
    assert!(engine.perform_command(window, "file.open"));

    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);

    assert!(engine.host_picked(
        request,
        vec![
            fs.doc(&["docs", "picked.md"]),
            fs.doc(&["docs", "second.md"]),
        ],
    ));
    settle_until(&mut engine, "the primary pick shows", |engine| {
        engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_some_and(|text| text.contains("hello"))
    });

    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);
    assert!(engine.host_picked(request, vec![fs.doc(&["docs", "picked.md"])]));
    settle(&mut engine);
    assert_eq!(
        himark::OpenDocuments::list(engine.app.store())
            .iter()
            .filter(|(_, entity)| entity.name() == "picked.md")
            .count(),
        1,
        "no twin"
    );
}

fn dock_x(x: f32) -> f32 {
    900.0 - himark::DOCK_WIDTH + x
}

/// The center of the canvas header's side-by-side button — the
/// RIGHTMOST of the three header buttons, inset from the canvas
/// pane's right edge (the pane ends where the dock begins). Mirrors
/// `HeaderFace::new` in hidiff/src/canvas.rs.
fn canvas_pane_button_x() -> f32 {
    let ui = himark::Theme::embedded();
    let chat = &ui.ui().chat;
    let zone = chat.title_size * 1.2 + chat.title_size;
    900.0 - himark::DOCK_WIDTH - chat.pad - zone * 0.5
}

/// Mirrors `ListRow`'s own sizing (text block + 8px each side) so the
/// click helpers land where the rows actually laid themselves.
fn derived_row_height(font_size: f32) -> f32 {
    let typeface = skia_safe::FontMgr::new()
        .legacy_make_typeface(None, skia_safe::FontStyle::normal())
        .expect("a system typeface");
    let metrics = skia_safe::Font::from_typeface(typeface, font_size)
        .metrics()
        .1;
    // Tree rows breathe with space::M on each side.
    (-metrics.ascent + metrics.descent).ceil() + 24.0
}

fn tree_row_y(index: usize) -> f32 {
    let ui = himark::Theme::embedded();
    let row = derived_row_height(ui.ui().tree.font_size);

    ui.ui().toolbar.height + 6.0 + row * index as f32 + row / 2.0
}

fn changes_row_y(index: usize) -> f32 {
    let ui = himark::Theme::embedded();
    let row = derived_row_height(ui.ui().tree.font_size);
    // REFRESH rides the repository root row now — the dock is just
    // the tree under a slim pad.
    ui.ui().toolbar.height + 6.0 + row * index as f32 + row / 2.0
}

/// The height of the canvas's first row on a working-copy canvas —
/// the commit composer at the CHAT composer's footprint: input band
/// plus the toolbar row. Mirrors `composer_band` in
/// hidiff/src/canvas.rs with an empty box.
fn canvas_composer_band() -> f32 {
    let ui = himark::Theme::embedded();
    let chat = &ui.ui().chat;
    chat.title_size * 1.6 + chat.pad * 1.5 + ui.ui().toolbar.height
}

fn history_row_y(index: usize) -> f32 {
    let ui = himark::Theme::embedded();
    let row = derived_row_height(ui.ui().tree.font_size);
    ui.ui().toolbar.height + 6.0 + row * index as f32 + row / 2.0
}

fn directory_location(path: &[&str]) -> himark::ResourceLocation {
    himark::ResourceLocation::new(
        himark::ResourceType::directory(),
        himark::Authority::new("test"),
        path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    )
}

#[test]
fn keymap_backspace_edits_the_dock_speed_search() {
    let (_host, mut engine, window, _fs) = hosted_engine();
    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);
    assert!(engine.host_picked(request, vec![directory_location(&["project"])]));
    settle_into_session(&mut engine);
    assert!(engine.perform_command(window, "files.tree"));
    settle(&mut engine);
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(0.0),
    );
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(1_000.0),
    );
    settle(&mut engine);

    let query = |engine: &HimarkEngine| -> Option<String> {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())?;
        let tree = entity
            .dock_panel()?
            .as_any()
            .downcast_ref::<himark::hifiles::SessionTreeView>()?;
        Some(tree.search_query())
    };

    for ch in ['p', 'r'] {
        assert!(
            !engine.key_down(window, u32::from(ch), 0),
            "a plain char stays unconsumed over the dock"
        );
        assert!(engine.text_input(window, &ch.to_string()));
    }
    assert_eq!(query(&engine).as_deref(), Some("pr"), "the query stands");

    assert!(
        engine.key_down(window, HIMARK_KEY_BACKSPACE, 0),
        "backspace resolves through the keymap"
    );
    assert_eq!(query(&engine).as_deref(), Some("p"), "the QUERY shrank");
    let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
        .expect("the window entity");
    assert!(entity.dock_panel().is_some(), "the dock never closed");

    let _ = himark::test_driver::click(&mut engine.app, 200.0, 400.0, 900.0, 700.0);
    settle(&mut engine);
    {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        assert!(
            entity.dock_panel().is_some(),
            "a click outside keeps the split panel"
        );
    }
    assert_eq!(
        query(&engine).as_deref(),
        Some("p"),
        "the query still stands"
    );
    assert!(engine.perform_command(window, "files.tree"));
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(4_000.0),
    );
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(5_000.0),
    );
    settle(&mut engine);
    let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
        .expect("the window entity");
    assert!(entity.dock_panel().is_none(), "the toggle closed the dock");
    assert!(engine.perform_command(window, "files.tree"));
    settle(&mut engine);
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    assert_eq!(
        query(&engine).as_deref(),
        Some(""),
        "the discarded panel's search died with it"
    );
}

#[test]
fn the_workspace_tree_lists_lazily_and_opens_documents() {
    let (_host, mut engine, window, fs) = hosted_engine();

    fs.write(&["project", "README.md"], "# readme");
    fs.write(&["project", "src", "lib.rs"], "fn main() {}");

    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);

    assert!(engine.host_picked(request, vec![fs.dir(&["project"])]));

    settle_until(&mut engine, "the folder session opened", |engine| {
        let entity_id = engine.app.sole_window();
        let workspace = himark::Windows::window_ref(engine.app.store(), entity_id)
            .expect("the window entity")
            .current_session();
        !himark::higent::session_folders(engine.app.store(), &workspace).is_empty()
    });
    let entity_id = engine.app.sole_window();
    let entity =
        himark::Windows::window_ref(engine.app.store(), entity_id).expect("the window entity");
    let workspace = entity.current_session();
    assert!(
        workspace.names_session(),
        "the pick entered a session workspace"
    );
    let folders = himark::higent::session_folders(engine.app.store(), &workspace);
    assert_eq!(
        folders
            .iter()
            .map(|folder| folder.path().to_vec())
            .collect::<Vec<_>>(),
        vec![fs.dir(&["project"]).path().to_vec()],
        "the session's working directory is the picked folder"
    );
    assert!(
        entity.plugin_modal().is_none(),
        "no overlay — the pick is silent"
    );

    assert!(engine.perform_command(window, "files.tree"));
    settle(&mut engine);
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    settle(&mut engine);

    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(0.0),
    );
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(1_000.0),
    );
    settle(&mut engine);

    let clicked =
        himark::test_driver::click(&mut engine.app, dock_x(40.0), tree_row_y(0), 900.0, 700.0);
    assert!(clicked, "the root row took the click");

    settle_until(&mut engine, "the root listing landed", |engine| {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        entity
            .dock_panel()
            .and_then(|side| {
                side.as_any()
                    .downcast_ref::<himark::hifiles::SessionTreeView>()
            })
            .is_some_and(|view| view.row_count() == 3)
    });

    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(2_000.0),
    );
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(3_000.0),
    );
    settle(&mut engine);

    let clicked =
        himark::test_driver::click(&mut engine.app, dock_x(40.0), tree_row_y(2), 900.0, 700.0);
    assert!(clicked, "the document row took the click");
    settle(&mut engine);
    let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
        .expect("the window entity");
    assert!(
        entity.dock_panel().is_some(),
        "picking a document keeps the split panel"
    );

    let _ = himark::test_driver::click(&mut engine.app, 150.0, 300.0, 900.0, 700.0);
    settle(&mut engine);
    {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        assert!(
            entity.dock_panel().is_some(),
            "a click outside keeps the split panel"
        );
    }
    settle_until(&mut engine, "the clicked document shows", |engine| {
        engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_some_and(|text| text.contains("readme"))
    });
    assert!(engine.perform_command(window, "files.tree"));
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(4_000.0),
    );
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(5_000.0),
    );
    settle(&mut engine);
    let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
        .expect("the window entity");
    assert!(entity.dock_panel().is_none(), "the toggle closed the dock");

    assert!(engine.perform_command(window, "files.tree"));
    settle(&mut engine);
    {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        let view = entity
            .dock_panel()
            .and_then(|side| {
                side.as_any()
                    .downcast_ref::<himark::hifiles::SessionTreeView>()
            })
            .expect("the tree is up");
        assert_eq!(view.row_count(), 3, "expansion survived the close");
        assert_eq!(view.selected_name().as_deref(), Some("README.md"));
        assert!(!view.reveal_pending());
    }

    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(6_000.0),
    );
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(6_500.0),
    );
    settle(&mut engine);
    let clicked =
        himark::test_driver::click(&mut engine.app, dock_x(40.0), tree_row_y(0), 900.0, 700.0);
    assert!(clicked, "the root row collapses");
    settle(&mut engine);
    {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        let rows = entity
            .dock_panel()
            .and_then(|side| {
                side.as_any()
                    .downcast_ref::<himark::hifiles::SessionTreeView>()
            })
            .map(|view| view.row_count());
        assert_eq!(rows, Some(1), "the root folded shut");
    }
    assert!(engine.perform_command(window, "files.tree"));
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(7_000.0),
    );
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(8_000.0),
    );
    settle(&mut engine);

    assert!(engine.perform_command(window, "files.tree"));

    settle_until(&mut engine, "the reveal walked to the row", |engine| {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        entity
            .dock_panel()
            .and_then(|side| {
                side.as_any()
                    .downcast_ref::<himark::hifiles::SessionTreeView>()
            })
            .is_some_and(|view| {
                view.selected_name().as_deref() == Some("README.md") && !view.reveal_pending()
            })
    });
}

#[test]
fn the_docked_tree_follows_the_focused_document() {
    struct OpenAt(himark::ResourceLocation);
    impl himark::DynamicCommand for OpenAt {
        fn id(&self) -> &'static str {
            "test.open-at"
        }
        fn name(&self) -> String {
            "Open".to_owned()
        }
        fn perform(
            &self,
            app: &mut himark::Application,
            store: &mut imba::store::Store,
            window: himark::WindowId,
            fx: &mut himark::AppFx<'_>,
        ) {
            let ui = &app.ui_ctx();
            himark::open_locations(store, ui, window, &[self.0.clone()], fx);
        }
    }

    let (_host, mut engine, window, fs) = hosted_engine();
    fs.write(&["project", "README.md"], "# readme");
    fs.write(&["project", "src", "lib.rs"], "fn main() {}");
    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);
    assert!(engine.host_picked(request, vec![fs.dir(&["project"])]));
    settle_into_session(&mut engine);

    let entity_id = engine.app.sole_window();
    assert!(engine.app.perform_batch(vec![himark::AppCommand::Dynamic(
        entity_id,
        std::sync::Arc::new(OpenAt(fs.doc(&["project", "README.md"]))),
    )]));
    settle_until(&mut engine, "README opened", |engine| {
        engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_some_and(|text| text.contains("# readme"))
    });
    assert!(engine.perform_command(window, "files.tree"));
    settle(&mut engine);

    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(0.0),
    );
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(1_000.0),
    );
    settle(&mut engine);
    let tree_selection = |engine: &mut HimarkEngine| {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        entity
            .dock_panel()
            .and_then(|side| {
                side.as_any()
                    .downcast_ref::<himark::hifiles::SessionTreeView>()
            })
            .and_then(|view| view.selected_name())
    };
    settle_until(&mut engine, "the open-time reveal landed", |engine| {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        tree_selection(engine).as_deref() == Some("README.md")
    });

    assert!(engine.app.perform_batch(vec![himark::AppCommand::Dynamic(
        entity_id,
        std::sync::Arc::new(OpenAt(fs.doc(&["project", "src", "lib.rs"]))),
    )]));
    settle_until(&mut engine, "the tree followed the open", |engine| {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        tree_selection(engine).as_deref() == Some("lib.rs")
    });

    assert!(engine.perform_command(window, "navigation.back"));
    settle_until(&mut engine, "the tree followed the back-walk", |engine| {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        tree_selection(engine).as_deref() == Some("README.md")
    });
}

#[test]
fn the_changes_view_lists_changes_and_opens_a_diff() {
    let (_host, mut engine, window, _fs) = hosted_engine();

    let dir = tempfile::tempdir().expect("tempdir");

    let root = dir.path().canonicalize().expect("canonical root");

    let sh = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    sh(&["init", "-q", "-b", "main"]);
    sh(&["config", "user.email", "test@example.com"]);
    sh(&["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "# readme\n\nold body\n").unwrap();
    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "first commit"]);
    std::fs::write(root.join("README.md"), "# readme\n\nnew body\n").unwrap();
    let root_segments: Vec<String> = root
        .to_str()
        .unwrap()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    let folder = himark::ResourceLocation::new(
        himark::ResourceType::directory(),
        himark::Authority::new("local"),
        root_segments.clone(),
    );

    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);
    assert!(engine.host_picked(request, vec![folder]));
    settle_into_session(&mut engine);

    assert!(engine.perform_command(window, "changes.view"));
    settle(&mut engine);
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    for ms in [0.0, 1_000.0] {
        let _ = himark::test_driver::animate(
            &mut engine.app,
            imba::anim::AnimationClock::from_millis(ms),
        );
    }
    let rows = |engine: &HimarkEngine| {
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window()).and_then(
            |entity| {
                entity
                    .dock_panel()
                    .and_then(|side| {
                        side.as_any()
                            .downcast_ref::<himark::hichanges::ChangesView>()
                    })
                    .map(|view| view.rows())
            },
        )
    };
    settle_until(&mut engine, "the changed file landed", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        rows(engine).is_some_and(|rows| {
            rows.iter()
                .any(|(depth, label, pick)| *depth == 1 && label.starts_with("README.md") && *pick)
        })
    });
    let listed = rows(&engine).expect("the dock is up");
    assert_eq!(
        listed.len(),
        2,
        "the folder root and its one change: {listed:?}"
    );
    assert_eq!(listed[0].0, 0, "the workspace folder is the root row");
    assert!(
        listed[1].1.contains("+1 −1"),
        "the change dims carry the counts: {listed:?}"
    );

    let clicked = himark::test_driver::click(
        &mut engine.app,
        dock_x(90.0),
        changes_row_y(1),
        900.0,
        700.0,
    );
    assert!(clicked, "the file row took the click");
    settle(&mut engine);
    {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        assert!(
            entity.dock_panel().is_some(),
            "picking a file keeps the split panel"
        );
    }

    // The file row reveals into the DIFF CANVAS now
    // (docs/editor/diff-canvas.md §6): rows populate from the adopted
    // changeset, the visible placeholder arms its off-thread build,
    // and the landing swaps in atomically.
    let canvas_probe = |engine: &HimarkEngine| {
        let mut shot = None;
        engine.app.for_each_plugin_panel(&mut |panel| {
            if let Some(canvas) = panel.as_any().downcast_ref::<hidiff::DiffCanvasView>() {
                shot = Some(canvas.probe_rows(engine.app.store()));
            }
        });
        shot
    };
    settle_until(&mut engine, "the canvas row built its diff", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        canvas_probe(engine).is_some_and(|rows| {
            rows.iter().any(|(title, phase, _)| {
                title == "README.md" && *phase == hidiff::canvas::RowPhase::Built
            })
        })
    });

    // The header's side-by-side button opens the standalone pane —
    // the old road, still reachable per file (the header BODY opens
    // the live file now). The working-copy canvas heads with the
    // commit composer, so the file header sits one band down.
    let chrome_top = himark::env::Themes::of(engine.app.store())
        .ui()
        .toolbar
        .height;
    assert!(himark::test_driver::click(
        &mut engine.app,
        canvas_pane_button_x(),
        chrome_top + canvas_composer_band() + 40.0,
        900.0,
        700.0,
    ));
    settle(&mut engine);

    let mut halves = None;
    settle_until(&mut engine, "the diff pane mounted", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        engine.app.for_each_plugin_panel(&mut |panel| {
            if let Some(panel) = panel.as_any().downcast_ref::<hidiff::DiffPanelView>() {
                halves = Some(panel.halves(engine.app.store()));
            }
        });
        halves.is_some()
    });
    let (left, right) = halves.expect("the diff pane mounted");
    let left_doc = left.document();
    let right_doc = right.document();

    let pinned = himark::OpenDocuments::entity(engine.app.store(), left_doc)
        .expect("the before side registers");
    let pinned_location = pinned
        .location()
        .cloned()
        .expect("under its before-ref location");
    assert_eq!(pinned_location.name(), "README.md");
    assert!(
        himark::hichanges::scoped(&pinned_location),
        "the location is the changeset before ref: {pinned_location:?}"
    );
    assert_eq!(
        pinned.saved_revision(),
        himark::OpenDocuments::document_ref(engine.app.store(), left_doc)
            .expect("old document")
            .revision(),
        "born clean — never dirty at open"
    );

    let session_folder = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
        .map(|entity| entity.current_session())
        .and_then(|workspace| {
            himark::higent::session_folders(engine.app.store(), &workspace)
                .first()
                .cloned()
        })
        .expect("the session folder");
    let new_side = himark::ResourceLocation::new(
        himark::ResourceType::document(),
        session_folder.authority().clone(),
        {
            let mut segments = root_segments.clone();
            segments.push("README.md".to_owned());
            segments
        },
    );
    let registered = himark::OpenDocuments::by_location(engine.app.store(), &new_side)
        .expect("the working-copy side registered as a real open");
    assert_eq!(registered, right_doc);

    let old_text = {
        let document = himark::OpenDocuments::document_ref(engine.app.store(), left_doc)
            .expect("old document");
        let count = document.text().byte_count();
        document.text().view().byte_string(0, count)
    };
    assert!(
        old_text.contains("old body"),
        "the old side is HEAD's content: {old_text:?}"
    );

    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    settle(&mut engine);
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);

    for _ in 0..4 {
        settle(&mut engine);
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    }
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(10_000.0),
    );
    for _ in 0..2 {
        settle(&mut engine);
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    }
    for ms in [20_000.0, 20_008.3, 20_016.7, 20_025.0] {
        assert!(
            !himark::test_driver::animate(
                &mut engine.app,
                imba::anim::AnimationClock::from_millis(ms),
            ),
            "a settled diff pane must not answer the clock at {ms}ms"
        );
    }
    let quiet = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    assert!(
        !quiet,
        "a settled diff pane's paint reports no change — the display link can rest"
    );

    std::fs::write(
        root.join("README.md"),
        "# readme

new body
and another
",
    )
    .unwrap();
    // The per-repository REFRESH chip rides the root row, right
    // aligned — the press refetches THIS folder's changeset.
    assert!(himark::test_driver::click(
        &mut engine.app,
        850.0,
        changes_row_y(0),
        900.0,
        700.0
    ));
    settle_until(&mut engine, "the refetch recomputed the counts", |engine| {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        rows(engine).is_some_and(|rows| {
            rows.iter()
                .any(|(_, label, _)| label.starts_with("README.md") && label.contains("+2"))
        })
    });
}

#[test]
fn stripes_take_the_changesets_old_text_as_base() {
    let (_host, mut engine, window, _fs) = hosted_engine();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonical root");
    let sh = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    sh(&["init", "-q", "-b", "main"]);
    sh(&["config", "user.email", "test@example.com"]);
    sh(&["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "# readme\n\nold body\n").unwrap();
    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "first commit"]);
    std::fs::write(root.join("README.md"), "# readme\n\nnew body\n").unwrap();
    let root_segments: Vec<String> = root
        .to_str()
        .unwrap()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    let folder = himark::ResourceLocation::new(
        himark::ResourceType::directory(),
        himark::Authority::new("local"),
        root_segments.clone(),
    );
    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);
    assert!(engine.host_picked(request, vec![folder]));
    settle_into_session(&mut engine);

    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);
    let file = himark::ResourceLocation::new(
        himark::ResourceType::document(),
        himark::Authority::new("local"),
        {
            let mut segments = root_segments.clone();
            segments.push("README.md".to_owned());
            segments
        },
    );
    assert!(engine.host_picked(request, vec![file.clone()]));
    settle_until(&mut engine, "the working copy opened", |engine| {
        engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_some_and(|text| text.contains("new body"))
    });

    settle_until(&mut engine, "the stripes diff tracked", |engine| {
        himark::OpenDocuments::list(engine.app.store())
            .into_iter()
            .find(|(_, entity)| {
                entity
                    .location()
                    .is_some_and(|location| location.path() == file.path())
            })
            .and_then(|(document, _)| {
                himark::OpenDocuments::stripe_diff(engine.app.store(), document)
            })
            .is_some()
    });

    let base_text = himark::OpenDocuments::list(engine.app.store())
        .into_iter()
        .find(|(_, entity)| entity.location().is_some_and(himark::hichanges::scoped))
        .map(|(document, _)| {
            let document = himark::OpenDocuments::document_ref(engine.app.store(), document)
                .expect("the base document");
            let count = document.text().byte_count();
            document.text().view().byte_string(0, count)
        })
        .expect("a before-ref base registered");
    assert!(
        base_text.contains("old body"),
        "the stripes base is HEAD's content: {base_text:?}"
    );
}

#[test]
fn saving_stores_the_focused_document_to_its_location() {
    let (_host, mut engine, window, fs) = hosted_engine();
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["notes.md"],
        "# notes",
    );

    assert!(engine.text_input(window, "x"));
    settle(&mut engine);
    assert!(engine.perform_command(window, "file.save"));
    settle_until(&mut engine, "the save landed on disk", |engine| {
        let stored = fs
            .read(&["notes.md"])
            .is_some_and(|text| text.contains('x'));
        let clean = himark::OpenDocuments::list(engine.app.store())
            .into_iter()
            .find(|(_, entity)| entity.name() == "notes.md")
            .is_some_and(|(id, entity)| {
                himark::OpenDocuments::document(engine.app.store(), id)
                    .is_some_and(|document| entity.saved_revision() == document.revision())
            });
        stored && clean
    });
}

#[test]
fn a_cancelled_pick_lands_and_opens_nothing() {
    let (_host, mut engine, window, _fs) = hosted_engine();
    let before = engine.substring(
        window,
        HimarkRange {
            start: 0,
            length: u32::MAX,
        },
    );
    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);

    assert!(engine.host_picked(request, Vec::new()));
    settle(&mut engine);
    assert_eq!(
        engine.substring(
            window,
            HimarkRange {
                start: 0,
                length: u32::MAX
            }
        ),
        before
    );
}

#[test]
fn the_terminal_round_trip_shows_the_panel_over_a_live_session() {
    let (_host, mut engine, window, _fs) = hosted_engine();
    assert!(engine.perform_command(window, "terminal.open"));

    settle_until(&mut engine, "the terminal panel took the pane", |engine| {
        engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_none()
    });

    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let mut ink = |engine: &mut HimarkEngine| -> usize {
        let mut inked = 0usize;
        for _ in 0..200 {
            settle(engine);
            surface.canvas().clear(skia_safe::Color::WHITE);
            let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
            let image = surface.image_snapshot();
            let pixels = image.peek_pixels().expect("pixels");
            inked = 0;
            for y in 60..300 {
                for x in 20..880 {
                    let color: skia_safe::Color = pixels.get_color((x, y));
                    if color.r() < 200 || color.g() < 200 || color.b() < 200 {
                        inked += 1;
                    }
                }
            }
            if inked > 50 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        inked
    };
    let inked = ink(&mut engine);
    assert!(inked > 50, "the prompt painted: {inked} inked pixels");

    assert!(engine.app.open_panel(
        engine.app.sole_window(),
        Box::new(himark::higent::ChatPane::new(
            "test-chat:displacer".to_owned()
        ))
    ));
    settle(&mut engine);
    let mut mounted = false;
    engine.app.for_each_plugin_panel(&mut |panel| {
        mounted |= panel.as_any().is::<himark::terminal::TerminalView>();
    });
    assert!(!mounted, "the terminal handle dropped with the pane");
    assert_eq!(
        himark::terminal::Terminals::list(engine.app.store()).len(),
        1,
        "the PTY session survives in the family"
    );

    let terminal = himark::mint_unfronted(engine.app.store(), &[])
        .into_iter()
        .find(|widget| widget.as_any().is::<himark::terminal::TerminalView>())
        .expect("the family lists the surviving terminal");
    assert!(engine.app.open_panel(engine.app.sole_window(), terminal));
    let inked = ink(&mut engine);
    assert!(
        inked > 50,
        "the remounted terminal painted: {inked} inked pixels"
    );

    assert!(engine.perform_command(window, "workbench.close"));
    settle(&mut engine);
    let mut mounted = false;
    engine.app.for_each_plugin_panel(&mut |panel| {
        mounted |= panel.as_any().is::<himark::terminal::TerminalView>();
    });
    assert!(!mounted, "the terminal panel closed");
}

#[test]
fn a_failed_spawn_lands_and_shows_no_panel() {
    let _guard = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    std::env::set_var("HIMARK_HOST_HOME", dir.path());
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    let before = engine.substring(
        window,
        HimarkRange {
            start: 0,
            length: u32::MAX,
        },
    );

    assert!(engine.perform_command(window, "terminal.open"));
    for _ in 0..40 {
        settle(&mut engine);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        engine.substring(
            window,
            HimarkRange {
                start: 0,
                length: u32::MAX
            }
        ),
        before,
        "the scratch stays — no panel over a dead backend"
    );
}

#[test]
fn hostless_engines_do_not_register_the_host_commands() {
    let _guard = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    std::env::set_var("HIMARK_HOST_HOME", dir.path());
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    assert!(!engine.perform_command(window, "file.open"));
    assert!(engine.perform_command(window, "terminal.open"));
}

#[test]
fn key_codes_map_only_known_keys() {
    assert!(map_key(HIMARK_KEY_BACKSPACE).is_some());
    assert!(map_key(HIMARK_KEY_ENTER).is_some());
    assert!(map_key(HIMARK_KEY_LEFT).is_some());
    assert!(map_key(HIMARK_KEY_RIGHT).is_some());
    assert!(map_key(HIMARK_KEY_UP).is_some());
    assert!(map_key(HIMARK_KEY_DOWN).is_some());
    assert!(map_key(HIMARK_KEY_ESCAPE).is_some());
    assert!(map_key(u32::MAX).is_none());
}

#[test]
fn scroll_gestures_break_on_touch_not_on_momentum() {
    // A fresh touch always starts a new gesture, so the scroller under
    // the pointer gets to re-claim the routing — even mid-flurry.
    assert!(scroll_gesture_boundary(
        HIMARK_SCROLL_PHASE_MAY_BEGIN,
        false
    ));
    assert!(scroll_gesture_boundary(HIMARK_SCROLL_PHASE_BEGAN, false));

    // Momentum belongs to the flick that spawned it: it must keep
    // scrolling the surface the flick claimed, wherever the pointer is.
    assert!(!scroll_gesture_boundary(
        HIMARK_SCROLL_PHASE_MOMENTUM_BEGAN,
        true
    ));
    assert!(!scroll_gesture_boundary(
        HIMARK_SCROLL_PHASE_MOMENTUM_CHANGED,
        true
    ));
    assert!(!scroll_gesture_boundary(
        HIMARK_SCROLL_PHASE_MOMENTUM_ENDED,
        true
    ));
    assert!(!scroll_gesture_boundary(HIMARK_SCROLL_PHASE_ENDED, true));

    // Phaseless wheels and bare `changed` streams (hosts that never send
    // a began) fall back to the pause heuristic, so an owner can never
    // wedge the routing forever.
    assert!(scroll_gesture_boundary(HIMARK_SCROLL_PHASE_NONE, true));
    assert!(!scroll_gesture_boundary(HIMARK_SCROLL_PHASE_NONE, false));
    assert!(scroll_gesture_boundary(HIMARK_SCROLL_PHASE_CHANGED, true));
    assert!(!scroll_gesture_boundary(HIMARK_SCROLL_PHASE_CHANGED, false));
}

#[test]
fn phased_scrolls_route_to_the_gesture_owner_per_window() {
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let a = engine.add_window();
    let b = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(a, surface.canvas(), 1100.0, 800.0, 1.0);
    let _ = engine.draw(b, surface.canvas(), 1100.0, 800.0, 1.0);
    engine.open_demo_wall(a);
    engine.open_demo_wall(b);
    for _ in 0..10 {
        settle(&mut engine);
        let _ = engine.draw(a, surface.canvas(), 1100.0, 800.0, 1.0);
        let _ = engine.draw(b, surface.canvas(), 1100.0, 800.0, 1.0);
    }
    assert!(engine.perform_command(a, "workbench.split-pane"));
    for _ in 0..10 {
        settle(&mut engine);
        let _ = engine.draw(a, surface.canvas(), 1100.0, 800.0, 1.0);
    }
    let scroll = |engine: &mut HimarkEngine, window: u64, x: f32, dy: f32, phase: u32| {
        engine.scroll_phased_at_time(window, x, 400.0, 0.0, dy, phase, 0.0)
    };
    let (left, right) = (275.0, 825.0);

    // A zero-delta began is bookkeeping only: nothing may claim a fresh
    // gesture before its direction is known.
    assert!(!scroll(
        &mut engine,
        a,
        left,
        0.0,
        HIMARK_SCROLL_PHASE_BEGAN
    ));
    // The first real delta claims the left pane and scrolls it.
    assert!(scroll(
        &mut engine,
        a,
        left,
        120.0,
        HIMARK_SCROLL_PHASE_CHANGED
    ));
    assert!(!scroll(
        &mut engine,
        a,
        left,
        0.0,
        HIMARK_SCROLL_PHASE_ENDED
    ));
    assert!(scroll(
        &mut engine,
        a,
        left,
        40.0,
        HIMARK_SCROLL_PHASE_MOMENTUM_BEGAN
    ));

    // A touch beginning in ANOTHER window must not break this gesture.
    assert!(!scroll(
        &mut engine,
        b,
        left,
        0.0,
        HIMARK_SCROLL_PHASE_BEGAN
    ));

    // Momentum drifting over the right pane stays the left pane's: the
    // right pane must not claim it.
    assert!(!scroll(
        &mut engine,
        a,
        right,
        40.0,
        HIMARK_SCROLL_PHASE_MOMENTUM_CHANGED
    ));
    assert!(!scroll(
        &mut engine,
        a,
        right,
        0.0,
        HIMARK_SCROLL_PHASE_MOMENTUM_ENDED
    ));

    // A fresh touch re-resolves from scratch: now the right pane scrolls.
    assert!(scroll(
        &mut engine,
        a,
        right,
        40.0,
        HIMARK_SCROLL_PHASE_BEGAN
    ));
}

#[test]
fn ffi_null_engine_calls_are_noops() {
    unsafe {
        assert_eq!(himark_add_window(null_mut()), 0);
        assert!(!himark_drain(null_mut()));
        assert!(!himark_key_down(null_mut(), 0, HIMARK_KEY_BACKSPACE, 0));
        assert!(!himark_text_input(null_mut(), 0, null(), 0));
        assert!(!himark_mouse_down(null_mut(), 0, 0.0, 0.0, 0, 1));
        assert!(!himark_scroll(null_mut(), 0, 0.0, 0.0, 0.0, 0.0));
        assert!(!himark_scroll_phased(
            null_mut(),
            0,
            0.0,
            0.0,
            0.0,
            0.0,
            HIMARK_SCROLL_PHASE_BEGAN
        ));
        assert!(!himark_animation_tick(null_mut(), 0.0));
        assert!(!himark_perform_command(
            null_mut(),
            0,
            c"workbench.split-pane".as_ptr(),
        ));
        assert!(!himark_has_text_focus(null_mut(), 0));
        assert!(!himark_has_marked_text(null_mut(), 0));
        assert!(!himark_unmark_text(null_mut(), 0));
        assert!(!himark_marked_range(null_mut(), 0, null_mut()));
        assert!(!himark_selected_range(null_mut(), 0, null_mut()));
        assert!(!himark_first_rect(
            null_mut(),
            0,
            HimarkRange::default(),
            null_mut()
        ));
        assert_eq!(himark_char_index_at(null_mut(), 0, 0.0, 0.0), -1);
        assert_eq!(
            himark_substring(null_mut(), 0, HimarkRange::default(), null_mut(), 0),
            0
        );
        assert!(!himark_set_selected_range(
            null_mut(),
            0,
            HimarkRange::default()
        ));
        assert!(!himark_document_length(null_mut(), 0, null_mut()));
        assert!(!himark_reveal_selection(null_mut(), 0));
        assert_eq!(
            himark_selection_rects(null_mut(), 0, HimarkRange::default(), null_mut(), 0),
            0
        );
    }
}

#[test]
fn registered_commands_dispatch_by_id() {
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();

    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);

    assert!(!engine.perform_command(window, "no.such.command"));
    // search.view is capability-gated (it needs a seat serving
    // searchLocations); hicode's references e2e covers its dock.

    assert!(engine.perform_command(window, "palette.toggle"));
    assert!(
        engine.app.plugin_modal().is_some(),
        "palette.toggle opened the palette modal"
    );
    assert!(engine.perform_command(window, "palette.toggle"));
    assert!(engine.app.plugin_modal().is_none(), "and toggles it away");
}

#[test]
fn table_cell_typing_keeps_the_diff_aligned_through_the_engine() {
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1280, 860)).expect("surface");
    let draw = |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface| {
        let _ = engine.draw(window, surface.canvas(), 1280.0, 860.0, 1.0);
    };

    let clock = std::cell::Cell::new(0.0f64);
    let pump = |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface, rounds: usize| {
        for _ in 0..rounds {
            engine.run_pending();
            let _ = engine.drain();
            clock.set(clock.get() + 8.33);
            let _ = engine.animation_tick(clock.get());
            let _ = engine.draw(window, surface.canvas(), 1280.0, 860.0, 1.0);
        }
    };

    let body = "| Feature | Status | Notes |\n| --- | --- | --- |\n| Persistent store | done | HAMT snapshots |\n| Effects | done | commands come home |\n\nA paragraph under the table long enough to wrap at the half width once or twice.\n\nAnother paragraph so the pair has body below the table as well.\n";
    assert!(engine.open_document(window, "left.md", body, false));
    assert!(engine.open_document(window, "right.md", body, false));
    pump(&mut engine, &mut surface, 8);
    assert!(engine.perform_command(window, "diff.open"));
    pump(&mut engine, &mut surface, 20);

    let cell_focused = |engine: &HimarkEngine| {
        let info = himark::OpenDocuments::list(engine.app.store())
            .into_iter()
            .find(|(_, info)| info.name() == "right.md")
            .expect("right open")
            .0;
        {
            let document =
                himark::OpenDocuments::document_ref(engine.app.store(), info).expect("document");
            document
                .editor_ids()
                .any(|editor| matches!(document.focus(editor), himark::EditorFocus::Inlay(_)))
        }
    };
    let mut focused = false;
    'probe: for y in [90, 120, 150, 180, 70] {
        for x in [720, 780, 850, 930, 1020, 1100] {
            let _ = engine.mouse_down(window, x as f32, y as f32, 0, 1);
            draw(&mut engine, &mut surface);
            if cell_focused(&engine) {
                focused = true;
                break 'probe;
            }
        }
    }
    assert!(focused, "a table cell took focus");

    let assert_aligned = |engine: &HimarkEngine, when: &str| {
        let mut checked = false;
        engine.app.for_each_plugin_panel(&mut |panel| {
            let Some(panel) = panel.as_any().downcast_ref::<hidiff::DiffPanelView>() else {
                return;
            };
            let diff_state = panel.diff_state(engine.app.store()).expect("settled");
            let diff = diff_state.diff();
            let (left, right) = panel.halves(engine.app.store());
            let sides = [left, right].map(|entity| {
                let document =
                    himark::OpenDocuments::document_ref(engine.app.store(), entity.document())
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
                (ranges, tops)
            });
            let (left_ranges, left_tops) = &sides[0];
            let (_, right_tops) = &sides[1];
            let mut aligned = 0;
            for range in left_ranges {
                let boundary = range.start;
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
                assert_eq!(
                    *left_top as i64, *right_top as i64,
                    "{when}: boundary {boundary}<->{mapped}"
                );
            }
            assert!(aligned > 0, "{when}: some boundaries must pair");
            checked = true;
        });
        assert!(checked, "{when}: the diff panel was inspected");
    };
    assert_aligned(&engine, "before typing");

    for (step, text) in ["grow the row plenty ", "grow it further still "]
        .iter()
        .enumerate()
    {
        assert!(engine.text_input(window, text));
        draw(&mut engine, &mut surface);
        assert_aligned(&engine, &format!("text step {step}, undrained"));
        pump(&mut engine, &mut surface, 6);
        assert_aligned(&engine, &format!("text step {step}, drained"));
    }
    for step in 0..3 {
        let _ = engine.key_down(window, HIMARK_KEY_ENTER, 0);
        draw(&mut engine, &mut surface);
        assert_aligned(&engine, &format!("enter step {step}, undrained"));
        pump(&mut engine, &mut surface, 6);
        assert_aligned(&engine, &format!("enter step {step}, drained"));
    }
}

#[test]
fn raw_string_conversion_validates_utf8_and_null_lengths() {
    unsafe {
        assert_eq!(str_from(null(), 0), Some(""));
        assert_eq!(str_from(null(), 1), None);

        let text = "hello";
        assert_eq!(
            str_from(text.as_ptr() as *const c_char, text.len()),
            Some(text)
        );

        let invalid = [0xff];
        assert_eq!(
            str_from(invalid.as_ptr() as *const c_char, invalid.len()),
            None
        );
    }
}

#[test]
fn external_edits_reach_documents_through_the_channel() {
    let (_host, mut engine, window, fs) = hosted_engine();
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["watched.md"],
        "alpha\n",
    );

    settle_until(&mut engine, "the document channel went live", |engine| {
        himark::OpenDocuments::list(engine.app.store())
            .into_iter()
            .find(|entity| {
                entity
                    .1
                    .location()
                    .is_some_and(|location| !himark::is_synthetic(location))
            })
            .is_some_and(|(id, entity)| {
                // Mode one against the real host: the channel is the
                // reload road; the client holds NO file watch.
                himark::OpenDocuments::host_synced(engine.app.store(), id)
                    && entity.watch().is_none()
            })
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        fs.write(&["watched.md"], "alpha\nEXTERNAL\n");
        std::thread::sleep(std::time::Duration::from_millis(200));
        settle(&mut engine);
        let showing = engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_some_and(|text| text.contains("EXTERNAL"));
        if showing {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the external write never drove a re-fetch"
        );
    }
}

#[test]
fn an_external_edit_merges_into_unsaved_typing() {
    let (_host, mut engine, window, fs) = hosted_engine();
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["merged.md"],
        "alpha\nbeta\n",
    );
    settle_until(&mut engine, "the document channel went live", |engine| {
        himark::OpenDocuments::list(engine.app.store())
            .into_iter()
            .find(|entity| {
                entity
                    .1
                    .location()
                    .is_some_and(|location| !himark::is_synthetic(location))
            })
            .is_some_and(|(id, entity)| {
                // Mode one against the real host: the channel is the
                // reload road; the client holds NO file watch.
                himark::OpenDocuments::host_synced(engine.app.store(), id)
                    && entity.watch().is_none()
            })
    });

    assert!(himark::test_driver::type_text(&mut engine.app, "MINE "));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        fs.write(&["merged.md"], "alpha\nEXTERNAL\n");
        std::thread::sleep(std::time::Duration::from_millis(200));
        settle(&mut engine);
        let showing = engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_some_and(|text| text.contains("MINE") && text.contains("EXTERNAL"));
        if showing {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the external change never merged into the dirty document"
        );
    }

    let (_, entity) = himark::OpenDocuments::list(engine.app.store())
        .into_iter()
        .find(|entity| entity.1.name() == "merged.md")
        .expect("the merged document");
    assert_ne!(
        entity.document().revision(),
        entity.saved_revision(),
        "the merge keeps the unsaved typing unsaved"
    );
}

#[test]
fn a_script_runs_reads_and_writes_over_the_real_host() {
    let (_host, mut engine, window, fs) = hosted_engine();
    fs.write(&["plan.md"], "alpha");
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["walk.js"],
        "export default async function (himark) {\n\
         const plan = await himark.docs.read(\"plan.md\");\n\
         await himark.docs.write(\"plan.md\", plan + \"-MORE\");\n\
         await himark.docs.write(\"out.md\", \"made by script\");\n\
         await himark.docs.show(\"out.md\");\n\
         himark.log(\"ran\");\n}\n",
    );
    assert!(
        engine.perform_command(window, "script.run"),
        "the command reaches the focused script"
    );
    settle_until(&mut engine, "both writes reached the disk", |engine| {
        settle(engine);
        std::fs::read_to_string(fs.root.join("plan.md")).is_ok_and(|text| text == "alpha-MORE")
            && std::fs::read_to_string(fs.root.join("out.md"))
                .is_ok_and(|text| text == "made by script")
    });
    let runs = hiscript::ScriptRuns::of(engine.app.store());
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].error, None, "log: {:?}", runs[0].log);
    assert_eq!(runs[0].log, vec!["ran"]);

    settle_until(&mut engine, "the generated file opened", |engine| {
        engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_some_and(|text| text == "made by script")
    });
}

#[test]
fn an_opened_mermaid_fence_renders_through_the_pipeline() {
    let (_host, mut engine, window, fs) = hosted_engine();
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["diagram.md"],
        "# Doc\n\n```mermaid\nflowchart TD\n    Start --> Finish\n```\n\ntail\n",
    );
    assert!(
        himark::env::Enrichers::of(engine.app.store()).is_some_and(|passes| !passes.is_empty()),
        "the enrichment registry reached the store"
    );
    settle_until(&mut engine, "the diagram landed", |engine| {
        himark::OpenDocuments::list(engine.app.store())
            .into_iter()
            .any(|entity| {
                let document = entity.1.document();
                let len = document.text().byte_count() as u32;
                document
                    .all_inlays_in(0..len)
                    .iter()
                    .any(|interval| interval.inlay.view_as::<himermaid::MermaidView>().is_some())
            })
    });
}

#[test]
fn a_caret_move_lights_the_bracket_pair_in_an_opened_rust_file() {
    let (_host, mut engine, window, fs) = hosted_engine();
    let source = "fn main() { let value = 1; }\n";
    open_picked(&mut engine, &_host.seat, window, &fs, &["code.rs"], source);
    let open = source.find('(').expect("opener") as u32;
    let close = source.find(')').expect("closer") as u32;

    for _ in 0..open {
        engine.key_down(window, HIMARK_KEY_RIGHT, 0);
    }
    settle_until(&mut engine, "the bracket pair lit", |engine| {
        let store = engine.app.store();
        himark::OpenDocuments::list(store)
            .into_iter()
            .any(|entity| {
                let document = entity.1.document();
                document.editor_ids().any(|editor| {
                    document
                        .enrichment_markup(himark::EnricherId("ts-brace-match"), Some(editor))
                        .is_some_and(|markup| {
                            document.markup_styled_ranges(markup)
                                == vec![open..open + 1, close..close + 1]
                        })
                })
            })
    });

    settle_until(&mut engine, "the identifier lit", |engine| {
        let store = engine.app.store();
        himark::OpenDocuments::list(store)
            .into_iter()
            .any(|entity| {
                let document = entity.1.document();
                document.editor_ids().any(|editor| {
                    document
                        .enrichment_markup(himark::EnricherId("ts-occurrences"), Some(editor))
                        .is_some_and(|markup| !document.markup_styled_ranges(markup).is_empty())
                })
            })
    });
}

#[test]
fn an_addressed_fence_embeds_a_sibling_file() {
    let ui = himark::test_document::test_ui();
    let (_host, mut engine, window, fs) = hosted_engine();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1400, 800)).expect("surface");

    fs.write(
        &["sidecar.rs"],
        "fn sidecar() -> u32 {\n    let a_very_long_binding_name_to_force_wrapping_at_the_install_width_but_not_the_pane = \"0123456789012345678901234567890123456789012345678901234567890123456789\";\n    42\n}\n",
    );
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["page.md"],
        "# Page\n\n``` rust sidecar.rs\nplaceholder\n```\n",
    );
    settle_until(&mut engine, "the fence embedded the sibling", |engine| {
        let store = engine.app.store();
        himark::OpenDocuments::list(store)
            .into_iter()
            .any(|entity| {
                let document = entity.1.document();
                let len = document.text().byte_count() as u32;
                document.all_inlays_in(0..len).iter().any(|interval| {
                    interval
                        .inlay
                        .view_as::<himarkdown::EmbedView>()
                        .is_some_and(|view| {
                            view.height() > 10.0
                                && himark::OpenDocuments::document_ref(store, view.document())
                                    .is_some_and(|target| {
                                        let text = target.text();
                                        text.byte_string(0, text.byte_count())
                                            .contains("fn sidecar")
                                    })
                        })
                })
            })
    });

    for _ in 0..4 {
        let _ = engine.draw(window, surface.canvas(), 1400.0, 800.0, 1.0);
        settle(&mut engine);
    }

    {
        let (host_id, inlay_key) = himark::OpenDocuments::list(engine.app.store())
            .into_iter()
            .find_map(|entity| {
                let document = entity.1.document();
                let len = document.text().byte_count() as u32;
                let key = document.all_inlays_in(0..len).iter().find_map(|interval| {
                    interval
                        .inlay
                        .view_as::<himarkdown::EmbedView>()
                        .map(|_| interval.key)
                })?;
                Some((entity.0, key))
            })
            .expect("the embed inlay");
        let editor = himark::OpenDocuments::document_ref(engine.app.store(), host_id)
            .and_then(|document| document.editor_ids().next())
            .expect("the host pane editor");
        let mut view = himark::EditorIdView::new(host_id, editor);
        let mut batch = imba::effect::Batch::new();
        let _ = imba::View::perform(
            &mut view,
            &mut *engine.app.store_mut(),
            himark::test_document::test_ui(),
            himark::EditorCommand::Inlay {
                key: inlay_key,
                command: Box::new(himark::EditorCommand::Viewport {
                    width: 1300.0,
                    top: 0.0,
                    bottom: 400.0,
                    anchor: 0,
                }) as himark::InlayCommand,
            },
            &mut batch.effects(),
        );
    }
    let store = engine.app.store();
    let (reserved, live) = himark::OpenDocuments::list(store)
        .into_iter()
        .find_map(|entity| {
            let document = entity.1.document();
            let len = document.text().byte_count() as u32;
            document.all_inlays_in(0..len).iter().find_map(|interval| {
                let view = interval.inlay.view_as::<himarkdown::EmbedView>()?;
                let target = himark::OpenDocuments::document_ref(store, view.document())?;
                Some((view.height(), target.content_height(view.editor())))
            })
        })
        .expect("the embed after the resize");
    let at_install_width = himark::EditorView::complete(
        himark::OpenDocuments::list(store)
            .into_iter()
            .find(|entity| {
                entity
                    .1
                    .location()
                    .is_some_and(|location| location.name() == "sidecar.rs")
            })
            .expect("target")
            .1
            .document()
            .clone(),
        720.0,
        store,
        &ui,
        himark::test_document::test_fonts_collection(),
        &himark::env::Themes::of(store),
    )
    .content_height();
    assert!(
        (reserved - live).abs() < 1.0,
        "the reserved height follows the live layout: reserved={reserved} live={live}"
    );
    assert!(
        live < at_install_width - 1.0,
        "the resize actually re-wrapped shorter (the oracle bites): live={live} at720={at_install_width}"
    );
}

#[test]
fn opening_the_embedded_file_in_a_pane_dedups_and_survives() {
    let (_host, mut engine, window, fs) = hosted_engine();
    fs.write(&["sidecar.rs"], "fn sidecar() -> u32 {\n    42\n}\n");
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["page.md"],
        "# Page\n\n``` rust sidecar.rs\nplaceholder\n```\n",
    );
    settle_until(&mut engine, "the fence embedded the sibling", |engine| {
        let store = engine.app.store();
        himark::OpenDocuments::list(store)
            .into_iter()
            .any(|entity| {
                let document = entity.1.document();
                let len = document.text().byte_count() as u32;
                document
                    .all_inlays_in(0..len)
                    .iter()
                    .any(|interval| interval.inlay.view_as::<himarkdown::EmbedView>().is_some())
            })
    });

    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["sidecar.rs"],
        "fn sidecar() -> u32 {\n    42\n}\n",
    );

    assert!(engine.text_input(window, "X"));
    settle(&mut engine);
    let store = engine.app.store();
    let shared: Vec<_> = himark::OpenDocuments::list(store)
        .into_iter()
        .filter(|entity| {
            entity
                .1
                .location()
                .is_some_and(|location| location.name() == "sidecar.rs")
        })
        .collect();
    assert_eq!(
        shared.len(),
        1,
        "ONE registered document serves the pane and the embed"
    );
}

#[test]
fn splitting_and_opening_the_embedded_file_survives() {
    let (_host, mut engine, window, fs) = hosted_engine();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    fs.write(&["sidecar.rs"], "fn sidecar() -> u32 {\n    42\n}\n");
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["page.md"],
        "# Page\n\n``` rust sidecar.rs\nplaceholder\n```\n",
    );
    settle_until(&mut engine, "the fence embedded the sibling", |engine| {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        let store = engine.app.store();
        himark::OpenDocuments::list(store)
            .into_iter()
            .any(|entity| {
                let document = entity.1.document();
                let len = document.text().byte_count() as u32;
                document
                    .all_inlays_in(0..len)
                    .iter()
                    .any(|interval| interval.inlay.view_as::<himarkdown::EmbedView>().is_some())
            })
    });
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);

    assert!(engine.perform_command(window, "workbench.split-pane"));
    settle(&mut engine);
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["sidecar.rs"],
        "fn sidecar() -> u32 {\n    42\n}\n",
    );

    for _ in 0..3 {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        settle(&mut engine);
    }

    assert!(engine.text_input(window, "X"));
    for _ in 0..3 {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        settle(&mut engine);
    }
}

#[test]
fn a_line_window_embed_is_bounded_and_survives_the_split_gauntlet() {
    let ui = himark::test_document::test_ui();
    let (_host, mut engine, window, fs) = hosted_engine();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let body = "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\nfn five() {}\n";
    fs.write(&["sidecar.rs"], body);
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["page.md"],
        "# Page\n\n``` rust sidecar.rs#L2-3\nplaceholder\n```\n",
    );
    let mut probe: Option<(f32, Option<std::ops::Range<u32>>, String)> = None;
    settle_until(&mut engine, "the windowed embed landed", |engine| {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        let store = engine.app.store();
        probe = himark::OpenDocuments::list(store)
            .into_iter()
            .find_map(|entity| {
                let document = entity.1.document();
                let len = document.text().byte_count() as u32;
                document.all_inlays_in(0..len).iter().find_map(|interval| {
                    let view = interval.inlay.view_as::<himarkdown::EmbedView>()?;
                    let target = himark::OpenDocuments::document_ref(store, view.document())?;
                    let window_range = Some(target.window(view.editor()));
                    let text = target.text();
                    Some((
                        view.height(),
                        window_range,
                        text.byte_string(0, text.byte_count()),
                    ))
                })
            });
        probe.is_some()
    });
    let (height, window_range, target_text) = probe.expect("the embed probe");

    let start = target_text.find("fn two").expect("line 2") as u32;
    let end = (target_text.find("fn three").expect("line 3") + "fn three() {}".len()) as u32;
    assert_eq!(
        window_range,
        Some(start..end),
        "the mounted editor is BOUNDED to the #L2-3 byte window"
    );

    let whole = {
        let store = engine.app.store();
        let target = himark::OpenDocuments::list(store)
            .into_iter()
            .find(|entity| {
                entity
                    .1
                    .location()
                    .is_some_and(|location| location.name() == "sidecar.rs")
            })
            .expect("the target document");
        himark::EditorView::complete(
            target.1.document().clone(),
            720.0,
            store,
            &ui,
            himark::test_document::test_fonts_collection(),
            &himark::env::Themes::of(store),
        )
        .content_height()
    };
    assert!(
        height > 10.0 && height < whole * 0.7,
        "a two-line window measures two lines, not the file: window={height} whole={whole}"
    );

    assert!(engine.perform_command(window, "workbench.split-pane"));
    settle(&mut engine);
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    open_picked(&mut engine, &_host.seat, window, &fs, &["sidecar.rs"], body);
    for _ in 0..3 {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        settle(&mut engine);
    }
    assert!(engine.text_input(window, "X"));
    for _ in 0..3 {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        settle(&mut engine);
    }
    let store = engine.app.store();
    let sidecars = himark::OpenDocuments::list(store)
        .into_iter()
        .filter(|entity| {
            entity
                .1
                .location()
                .is_some_and(|location| location.name() == "sidecar.rs")
        })
        .count();
    assert_eq!(sidecars, 1, "one shared document, pane and embed alike");
}

#[test]
fn saving_a_scratch_runs_save_as_and_re_points() {
    let (_host, mut engine, window, fs) = hosted_engine();

    assert!(engine.text_input(window, "hello scratch"));

    assert!(engine.perform_command(window, "file.save"));
    settle(&mut engine);
    let (request, suggested) = save_pick_request(&mut engine, &_host.seat);
    assert_eq!(suggested, "scratch.md", "the scratch suggests its name");

    let picked = fs.doc(&["notes", "kept.md"]);
    assert!(engine.host_picked_folder(request, Some(picked.clone())));
    settle_until(&mut engine, "the save-as stored and re-pointed", |engine| {
        fs.read(&["notes", "kept.md"]).as_deref() == Some("hello scratch")
            && himark::OpenDocuments::list(engine.app.store())
                .into_iter()
                .find(|(_, entity)| entity.name() == "kept.md")
                .is_some_and(|(_, entity)| {
                    entity.location() == Some(&picked)
                        && entity.saved_revision() == entity.document().revision()
                })
    });
    assert!(
        himark::RecentLocations::list(engine.app.store()).contains(&picked),
        "the recents follow the re-point"
    );
}

#[test]
fn typing_multibyte_text_survives_the_worker() {
    let (_lock, mut engine, window, _fs) = hosted_engine();
    settle(&mut engine);
    for chunk in ["привет", " мир", "\n", "ещё строка", "й", "ё"] {
        assert!(engine.text_input(window, chunk));
        settle(&mut engine);
    }
    let text = engine
        .substring(
            window,
            HimarkRange {
                start: 0,
                length: u32::MAX,
            },
        )
        .expect("the scratch");
    assert!(text.contains("привет мир"), "typed: {text:?}");
}

#[test]
fn taps_below_the_content_never_abort() {
    let (_host, mut engine, window, _fs) = hosted_engine();
    settle(&mut engine);
    for x in [3.0_f32, 12.0, 24.0, 40.0, 60.0, 90.0, 300.0] {
        for y in [400.0_f32, 550.0, 690.0] {
            let _ = engine.mouse_down(window, x, y, 0, 1);
            let _ = engine.mouse_up(window, x, y);
        }
    }
    settle(&mut engine);
}

#[test]
fn a_one_sided_diff_goes_quiet() {
    let (_host, mut engine, window, _fs) = hosted_engine();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonical root");
    let sh = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    sh(&["init", "-q", "-b", "main"]);
    sh(&["config", "user.email", "test@example.com"]);
    sh(&["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "# readme\n").unwrap();
    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "first commit"]);

    std::fs::write(root.join("fresh.json"), "brand new\n").unwrap();
    let root_segments: Vec<String> = root
        .to_str()
        .unwrap()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    let folder = himark::ResourceLocation::new(
        himark::ResourceType::directory(),
        himark::Authority::new("local"),
        root_segments.clone(),
    );
    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);
    assert!(engine.host_picked(request, vec![folder]));
    settle_into_session(&mut engine);

    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);
    let file = himark::ResourceLocation::new(
        himark::ResourceType::document(),
        himark::Authority::new("local"),
        {
            let mut segments = root_segments.clone();
            segments.push("fresh.json".to_owned());
            segments
        },
    );
    assert!(engine.host_picked(request, vec![file]));
    settle_until(&mut engine, "the working copy opened", |engine| {
        engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_some_and(|text| text.contains("brand new"))
    });

    assert!(engine.perform_command(window, "changes.view"));
    settle(&mut engine);
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    for ms in [0.0, 1_000.0] {
        let _ = himark::test_driver::animate(
            &mut engine.app,
            imba::anim::AnimationClock::from_millis(ms),
        );
    }
    settle_until(&mut engine, "the untracked row landed", |engine| {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .and_then(|entity| {
                entity
                    .dock_panel()
                    .and_then(|side| {
                        side.as_any()
                            .downcast_ref::<himark::hichanges::ChangesView>()
                    })
                    .map(|view| view.rows())
            })
            .is_some_and(|rows| {
                rows.iter().any(|(depth, label, pick)| {
                    *depth == 1 && label.starts_with("fresh.json") && *pick
                })
            })
    });

    assert!(himark::test_driver::click(
        &mut engine.app,
        dock_x(90.0),
        changes_row_y(1),
        900.0,
        700.0
    ));
    // The file row reveals into the canvas; the one-sided build
    // (empty old side) must land like any other.
    settle_until(&mut engine, "the one-sided canvas row built", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        let mut built = false;
        engine.app.for_each_plugin_panel(&mut |panel| {
            if let Some(canvas) = panel.as_any().downcast_ref::<hidiff::DiffCanvasView>() {
                built = canvas
                    .probe_rows(engine.app.store())
                    .iter()
                    .any(|(title, phase, _)| {
                        title == "fresh.json" && *phase == hidiff::canvas::RowPhase::Built
                    });
            }
        });
        built
    });

    // The header BODY opens the live file in an ordinary pane. The
    // canvas sits in a split half here, so the body span is narrow —
    // x=90 clears the chevron and stays left of the three buttons.
    // The composer banner heads the canvas; the header sits below it.
    let chrome_top = himark::env::Themes::of(engine.app.store())
        .ui()
        .toolbar
        .height;
    let header_y = chrome_top + canvas_composer_band() + 40.0;
    assert!(himark::test_driver::click(
        &mut engine.app,
        90.0,
        header_y,
        900.0,
        700.0,
    ));
    settle_until(&mut engine, "the header click opened the file", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        engine
            .substring(
                window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .is_some_and(|text| text.contains("brand new"))
    });

    // Clicking the changes row again REUSES the store-held canvas —
    // the fresh view references it, so the diff stands ALREADY BUILT
    // with no placeholder frame and no rebuild.
    assert!(himark::test_driver::click(
        &mut engine.app,
        dock_x(90.0),
        changes_row_y(1),
        900.0,
        700.0
    ));
    {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        let mut built = false;
        engine.app.for_each_plugin_panel(&mut |panel| {
            if let Some(canvas) = panel.as_any().downcast_ref::<hidiff::DiffCanvasView>() {
                built |= canvas
                    .probe_rows(engine.app.store())
                    .iter()
                    .any(|(title, phase, _)| {
                        title == "fresh.json" && *phase == hidiff::canvas::RowPhase::Built
                    });
            }
        });
        assert!(built, "the reused canvas is built on its FIRST frame");
    }
    settle(&mut engine);

    // The header's side-by-side BUTTON opens the standalone pane; the
    // QUIET guarantee below is the pane's.
    assert!(himark::test_driver::click(
        &mut engine.app,
        canvas_pane_button_x(),
        header_y,
        900.0,
        700.0,
    ));
    settle_until(&mut engine, "the one-sided diff pane mounted", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        let mut mounted = false;
        engine.app.for_each_plugin_panel(&mut |panel| {
            mounted |= panel.as_any().is::<hidiff::DiffPanelView>();
        });
        mounted
    });

    for _ in 0..4 {
        settle(&mut engine);
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    }
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(10_000.0),
    );
    for _ in 0..2 {
        settle(&mut engine);
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    }
    for ms in [20_000.0, 20_008.3, 20_016.7, 20_025.0] {
        assert!(
            !himark::test_driver::animate(
                &mut engine.app,
                imba::anim::AnimationClock::from_millis(ms),
            ),
            "a settled one-sided diff must not answer the clock at {ms}ms"
        );
    }
    let quiet = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    assert!(!quiet, "a settled one-sided diff paints quiet");
}

#[test]
fn a_rolled_away_drawer_leaves_and_goes_silent() {
    let (_host, mut engine, window, _fs) = hosted_engine();
    settle(&mut engine);
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");

    assert!(engine.perform_command(window, "files.tree"));
    let mut now = 0.0_f64;
    let tick = |engine: &mut HimarkEngine, now: &mut f64| -> bool {
        *now += 8.3;
        himark::test_driver::animate(
            &mut engine.app,
            imba::anim::AnimationClock::from_millis(*now),
        )
    };
    for _ in 0..60 {
        let _ = tick(&mut engine, &mut now);
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    }
    {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        assert!(entity.dock_panel().is_some(), "the dock is up");
    }

    assert!(engine.perform_command(window, "files.tree"));
    for _ in 0..120 {
        let _ = tick(&mut engine, &mut now);
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        settle(&mut engine);
    }
    {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        assert!(
            entity.dock_panel().is_none(),
            "a closed dock actually leaves — it must not stay mounted invisibly"
        );
    }
    let mut answered = 0;
    for _ in 0..8 {
        if tick(&mut engine, &mut now) {
            answered += 1;
        }
    }
    assert_eq!(answered, 0, "the clock is silent after the dock left");
}

struct StubNewSession {
    server: himark::higent::HostId,
    directory: String,
}

impl himark::DynamicCommand for StubNewSession {
    fn id(&self) -> &'static str {
        "test.stub-new-session"
    }
    fn name(&self) -> String {
        "Stub New Session".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        store: &mut imba::store::Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        let Some(seat) = himark::higent::Servers::seat(store, self.server) else {
            return;
        };
        let server = self.server;
        let _ = fx.push(
            imba::effect::AnyEffect::new(himark::higent::CreateSessionEffect {
                options: himark::higent::SessionOptions::default(),
                seat,
                working_directories: vec![self.directory.clone()],
            })
            .map(move |result| {
                himark::AppCommand::Dynamic(
                    window,
                    std::sync::Arc::new(himark::higent::OpenCreatedSession {
                        server,
                        open_chat: true,
                        initial_prompt: None,
                        result,
                    }),
                )
            }),
        );
    }
}

struct LaunchProbe {
    launch: Box<dyn Fn(himark::WindowId, &mut himark::AppFx<'_>) + Send + Sync>,
}

impl himark::DynamicCommand for LaunchProbe {
    fn id(&self) -> &'static str {
        "test.launch-probe"
    }
    fn name(&self) -> String {
        "Launch Probe".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        _store: &mut imba::store::Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        (self.launch)(window, fx);
    }
}

struct FillProbe<T: Send + Sync + 'static> {
    slot: std::sync::Arc<std::sync::Mutex<Option<T>>>,
    value: std::sync::Mutex<Option<T>>,
}

impl<T: Send + Sync + 'static> himark::DynamicCommand for FillProbe<T> {
    fn id(&self) -> &'static str {
        "test.fill-probe"
    }
    fn name(&self) -> String {
        "Fill Probe".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        _store: &mut imba::store::Store,
        _window: himark::WindowId,
        _fx: &mut himark::AppFx<'_>,
    ) {
        *self.slot.lock().expect("probe slot") = self.value.lock().expect("probe value").take();
    }
}

macro_rules! fs_probe {
    ($name:ident, $effect:ty, $result:ty, $build:expr) => {
        fn $name(
            engine: &mut HimarkEngine,
            surface: &mut skia_safe::Surface,
            window: u64,
            location: himark::ResourceLocation,
        ) -> Option<$result> {
            let slot: std::sync::Arc<std::sync::Mutex<Option<$result>>> =
                std::sync::Arc::new(std::sync::Mutex::new(None));
            let filled = std::sync::Arc::clone(&slot);
            let id = engine.app.sole_window();
            engine.app.perform_batch(vec![himark::AppCommand::Dynamic(
                id,
                std::sync::Arc::new(LaunchProbe {
                    launch: Box::new(move |window, fx| {
                        let filled = std::sync::Arc::clone(&filled);
                        #[allow(clippy::redundant_closure_call)]
                        let effect: $effect = ($build)(&location);
                        let _ = fx.push(imba::effect::AnyEffect::new(effect).map(move |result| {
                            himark::AppCommand::Dynamic(
                                window,
                                std::sync::Arc::new(FillProbe {
                                    slot: std::sync::Arc::clone(&filled),
                                    value: std::sync::Mutex::new(Some(result)),
                                }),
                            )
                        }));
                    }),
                }),
            )]);

            for _ in 0..40 {
                let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
                settle(engine);
                if slot.lock().expect("probe slot").is_some() {
                    break;
                }
            }
            let answer = slot.lock().expect("probe slot").take();
            answer
        }
    };
}

fs_probe!(
    probe_fetch,
    himark::FetchDocumentEffect,
    Option<String>,
    |location: &himark::ResourceLocation| himark::FetchDocumentEffect {
        location: location.clone(),
    }
);

fn drawer_rows(engine: &HimarkEngine) -> Option<Vec<(String, usize)>> {
    let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())?;
    let panel = entity
        .side_panel()?
        .as_any()
        .downcast_ref::<himark::higent::AgentsPanel>()?;
    Some(panel.rows())
}

fn shown_chat(engine: &HimarkEngine) -> Option<himark::higent::ChatPanel> {
    let mut uri = None;
    engine.app.for_each_plugin_panel(&mut |panel| {
        if let Some(pane) = panel.as_any().downcast_ref::<himark::higent::ChatPane>() {
            uri = Some(pane.chat().clone());
        }
    });

    if uri.is_none() {
        uri = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .and_then(|entity| entity.bottom_pane())
            .and_then(|pane| pane.as_any().downcast_ref::<himark::higent::ChatPane>())
            .map(|pane| pane.chat().clone());
    }
    himark::higent::Chats::chat(engine.app.store(), &uri?)
}

fn chat_transcript(engine: &HimarkEngine) -> Option<Vec<(String, Vec<(String, String)>)>> {
    shown_chat(engine).map(|chat| chat.transcript())
}

fn pick_drawer_row(engine: &mut HimarkEngine, index: usize) {
    for _ in 0..index {
        let _ = himark::test_driver::key(
            &mut engine.app,
            imba::event::Key::Down,
            imba::event::Modifiers::default(),
        );
    }
    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers::default(),
    );
}

fn block_on_seat<T>(mut future: himark::higent::SeatFuture<T>) -> T {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop_raw() -> RawWaker {
        fn clone(_: *const ()) -> RawWaker {
            noop_raw()
        }
        fn noop(_: *const ()) {}
        RawWaker::new(
            std::ptr::null(),
            &RawWakerVTable::new(clone, noop, noop, noop),
        )
    }
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,

            Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
}

#[test]
#[ignore = "needs a running agent host and a claude.ai login; spends a real prompt"]
fn real_claude_answers_through_the_agent_host() {
    std::env::remove_var("HIMARK_AHP_URL");
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);

    let pump = |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface, seconds: u64| {
        for _ in 0..seconds * 4 {
            let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
            settle(engine);
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    };

    let servers = himark::higent::Servers::list(engine.app.store());
    let vscode = servers[0];
    let repo = std::env::current_dir().expect("cwd");
    himark::higent::Agents::install_new_session(
        &mut engine.app.store_mut(),
        std::sync::Arc::new(move |server| {
            std::sync::Arc::new(StubNewSession {
                server,
                directory: format!("file://{}", repo.to_string_lossy()),
            })
        }),
    );

    assert!(engine.perform_command(window, "agent.toggle-agents"));
    let mut waited = 0;
    loop {
        pump(&mut engine, &mut surface, 1);
        let rows = drawer_rows(&engine).expect("the drawer is up");
        if rows
            .iter()
            .any(|(label, depth)| *depth == 1 && label == "+ New Session…")
            && !rows.iter().any(|(label, _)| label == "connecting…")
        {
            println!("drawer: {rows:?}");
            break;
        }
        waited += 1;
        assert!(waited < 30, "the live host never connected: {rows:?}");
    }

    let rows = drawer_rows(&engine).expect("the drawer is up");
    let plus = rows
        .iter()
        .rposition(|(label, _)| label == "+ New Session…")
        .expect("the + row");
    pick_drawer_row(&mut engine, plus);

    let chat_ready =
        |engine: &HimarkEngine| -> bool { shown_chat(engine).is_some_and(|chat| chat.ready()) };
    let mut waited = 0;
    while !chat_ready(&engine) && waited < 30 {
        pump(&mut engine, &mut surface, 1);
        waited += 1;
    }
    assert!(chat_ready(&engine), "the real chat subscribed");
    let rows = chat_transcript(&engine).expect("the real chat panel mounted");
    println!("mounted; rows: {}", rows.len());
    let store = engine.app.store();
    let entity = himark::Windows::window_ref(store, engine.app.sole_window()).expect("window");
    let key = himark::higent::Agents::live_session(store, &entity.current_session())
        .expect("the session bound its workspace");
    assert_eq!(key.host, vscode);
    println!("session: {}", key.session);

    assert!(himark::test_driver::type_text(
        &mut engine.app,
        "Reply with the single word OK and nothing else."
    ));
    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers {
            command: true,
            ..Default::default()
        },
    );

    let mut answered = false;
    for _ in 0..120 {
        pump(&mut engine, &mut surface, 1);
        if let Some(rows) = chat_transcript(&engine) {
            if let Some((_, cells)) = rows.last() {
                let done = cells
                    .iter()
                    .any(|(kind, text)| kind == "Agent" && text.contains("OK"));
                if done {
                    answered = true;
                    break;
                }
            }
        }
    }
    let rows = chat_transcript(&engine).expect("still mounted");
    println!("final turn: {:?}", rows.last());
    assert!(answered, "real Claude's OK streamed into the transcript");
}

#[test]
#[ignore = "needs a running agent host"]
fn the_wire_serves_a_session_filesystem() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    std::env::remove_var("HIMARK_AHP_URL");
    let root = "/tmp/ahp-fs-live";
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(root).expect("probe dir");
    std::fs::write(format!("{root}/seeded.txt"), "seeded on disk\n").expect("seed");

    let wire: std::sync::Arc<dyn himark::higent::AhpServer> =
        std::sync::Arc::new(crate::hiahp::wire::WireHost::new(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
        ));
    let session = block_on_seat(wire.create_session(
        vec![format!("file://{root}")],
        himark::higent::SessionOptions::default(),
    ))
    .expect("createSession");
    println!("session: {session}");

    let listed = block_on_seat(wire.resource_list(
        session.clone(),
        himark::higent::ResourceUri::new(format!("file://{root}")),
    ))
    .expect("resourceList answers");
    println!("listed: {listed:?}");
    assert!(listed
        .iter()
        .any(|(name, dir)| name == "seeded.txt" && !dir));

    let read = block_on_seat(wire.resource_read(
        session.clone(),
        himark::higent::ResourceUri::new(format!("file://{root}/seeded.txt")),
    ))
    .expect("resourceRead answers");
    assert_eq!(read, "seeded on disk\n");

    assert!(block_on_seat(wire.resource_write(
        session.clone(),
        himark::higent::ResourceUri::new(format!("file://{root}/written.txt")),
        "written over the wire\n".to_owned(),
    )));
    assert_eq!(
        std::fs::read_to_string(format!("{root}/written.txt")).expect("on disk"),
        "written over the wire\n",
        "the write reached the server's filesystem"
    );

    let fired = std::sync::Arc::new(AtomicUsize::new(0));
    let counter = std::sync::Arc::clone(&fired);
    let handle = block_on_seat(wire.resource_watch(
        session.clone(),
        himark::higent::ResourceUri::new(format!("file://{root}")),
        std::sync::Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        }),
    ))
    .expect("createResourceWatch answers");
    std::fs::write(format!("{root}/touched.txt"), "poke\n").expect("touch");
    let mut waited = 0;
    while fired.load(Ordering::Relaxed) == 0 && waited < 100 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        waited += 1;
    }
    assert!(
        fired.load(Ordering::Relaxed) > 0,
        "the watch delivered within 10s"
    );

    block_on_seat(wire.resource_unwatch(handle));
    block_on_seat(wire.dispose_session(session)).expect("dispose");
    let _ = std::fs::remove_dir_all(root);
}

struct SpawnedHost {
    child: std::process::Child,
    socket: std::path::PathBuf,
}

impl Drop for SpawnedHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Re-exec entry point for `spawn_host_inner`: it launches THIS
/// freshly-built test binary with `--ignored --exact
/// tests::agent_host_subprocess` and `HIMARK_TEST_AGENT_HOST=1`, so
/// the agent host under test is always current host code — there is
/// no separate `himark-agent-host` binary to rebuild and forget. A
/// plain `--ignored` run without the env is a no-op.
#[test]
#[ignore = "internal re-exec entry point (spawn_host_inner)"]
fn agent_host_subprocess() {
    if std::env::var_os("HIMARK_TEST_AGENT_HOST").is_none() {
        return;
    }
    let socket = std::env::var_os("HIMARK_TEST_AGENT_HOST_SOCKET").map(std::path::PathBuf::from);
    let http = std::env::var("HIMARK_TEST_AGENT_HOST_HTTP").ok();
    let web_root =
        std::env::var_os("HIMARK_TEST_AGENT_HOST_WEB_ROOT").map(std::path::PathBuf::from);
    let code = agent_host::run_with(socket.as_deref(), http.as_deref(), web_root.as_deref());
    std::process::exit(code);
}

fn spawn_host(dir: &std::path::Path, claude_binary: &str) -> SpawnedHost {
    spawn_host_with_args(dir, claude_binary, &[])
}

fn spawn_host_with_args(dir: &std::path::Path, claude_binary: &str, args: &[&str]) -> SpawnedHost {
    let spawned = spawn_host_inner(dir, claude_binary, true, args);
    spawned
}

fn spawn_host_with_env(dir: &std::path::Path, claude_binary: &str, clean: bool) -> SpawnedHost {
    spawn_host_inner(dir, claude_binary, clean, &[])
}

fn spawn_host_inner(
    dir: &std::path::Path,
    claude_binary: &str,
    clean: bool,
    args: &[&str],
) -> SpawnedHost {
    // Re-exec THIS freshly-built test binary as the host (the hidden
    // `agent_host_subprocess` entry point calls `agent_host::run_with`),
    // so the subprocess is always current host code. libtest owns argv,
    // so the host's own flags travel as env instead.
    let bin = std::env::current_exe().expect("test exe");
    let home = dir.join("host-home");
    std::fs::create_dir_all(&home).expect("host home");
    let socket = home.join("host.sock");
    let log = std::fs::File::create(home.join("host.log")).expect("host log");
    let flag = |name: &str| {
        args.iter()
            .position(|arg| *arg == name)
            .and_then(|at| args.get(at + 1))
            .copied()
    };
    let mut command = std::process::Command::new(&bin);
    command.args([
        "tests::agent_host_subprocess",
        "--exact",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    // env_clear FIRST — it would otherwise wipe the vars set below.
    if clean {
        command
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", std::env::var("HOME").unwrap_or_default());
    }
    if let Some(http) = flag("--http") {
        command.env("HIMARK_TEST_AGENT_HOST_HTTP", http);
    }
    if let Some(web_root) = flag("--web-root") {
        command.env("HIMARK_TEST_AGENT_HOST_WEB_ROOT", web_root);
    }
    let child = command
        .env("HIMARK_TEST_AGENT_HOST", "1")
        .env("HIMARK_TEST_AGENT_HOST_SOCKET", &socket)
        .env("HIMARK_HOST_HOME", &home)
        .env("HIMARK_LOG_DIR", &home)
        .env("HIMARK_CLAUDE_BIN", claude_binary)
        .env("HIMARK_CLAUDE_HOME", dir.join("dot-claude"))
        .env("HIHOST_TRACE", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log)
        .spawn()
        .expect("the test binary re-execs as the host");
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(20));
        waited += 1;
        assert!(waited < 500, "the host never bound {socket:?}");
    }
    SpawnedHost { child, socket }
}

fn drive<T>(mut future: himark::higent::SeatFuture<T>) -> T {
    use std::task::{Context, Poll};
    let waker = std::task::Waker::noop();
    let mut ctx = Context::from_waker(waker);
    for _ in 0..6_000 {
        match future.as_mut().poll(&mut ctx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    panic!("the seat future never resolved");
}

#[test]
fn a_host_added_by_url_connects_over_the_http_face() {
    std::env::set_var("HIMARK_AGENT_LATENCY_MS", "0");
    std::env::set_var("HIMARK_AHP_URL", "ws://127.0.0.1:9/unreachable");
    let host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let stub = agent_host::testing::fake_cli_command(dir.path());
    let spawned = spawn_host_with_args(dir.path(), &stub, &["--http", "127.0.0.1:0"]);
    let home = dir.path().join("host-home");
    let mut url = None;
    for _ in 0..250 {
        if let Some(live) = host_discovery::read_live(&home) {
            if let Some(http) = live.http {
                url = Some(http);
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let ws = url
        .expect("the daemon published its http url")
        .replacen("http://", "ws://", 1);

    std::env::set_var("HIMARK_HOST_HOME", dir.path().join("client-home"));
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);

    let entity = engine.app.sole_window();
    assert!(engine.app.perform_batch(vec![himark::AppCommand::Dynamic(
        entity,
        std::sync::Arc::new(himark::higent::AddHost { url: ws.clone() }),
    )]));

    let mut rows = Vec::new();
    for _ in 0..400 {
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        settle(&mut engine);
        rows = drawer_rows(&engine).expect("the AddHost reopen leaves the drawer up");
        let connected = rows
            .iter()
            .position(|(label, depth)| *depth == 0 && label == ws.trim())
            .is_some_and(|index| rows.get(index + 1) == Some(&("+ New Session…".to_owned(), 1)));
        if connected {
            drop(spawned);
            drop(host);
            return;
        }

        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the URL-added host row never connected: {rows:?}");
}

#[test]
fn the_seat_speaks_ahp_over_the_http_face() {
    use himark::higent::AhpServer;
    let host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let spawned = spawn_host_with_args(dir.path(), "false", &["--http", "127.0.0.1:0"]);
    let home = dir.path().join("host-home");
    let mut url = None;
    for _ in 0..250 {
        if let Some(live) = host_discovery::read_live(&home) {
            if let Some(http) = live.http {
                url = Some(http);
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let url = url.expect("the daemon published its http url");
    let ws = url.replacen("http://", "ws://", 1);

    std::env::set_var("HIMARK_LOG_DIR", dir.path().join("logs"));
    let seat = crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(),
        crate::test_connector(),
        ws,
    );
    drive(seat.connect()).expect("the ws seat connects");
    let session = drive(seat.create_session(
        vec![format!("file://{}", dir.path().display())],
        himark::higent::SessionOptions::default(),
    ))
    .expect("a session over the http face");
    let subscribed = drive(seat.subscribe_session(session)).expect("the session subscribes");
    assert!(
        subscribed.default_chat.is_some(),
        "the session carries its default chat over ws like over unix"
    );
    drop(spawned);
    drop(host);
}

fn reconnect_seat(dir: &std::path::Path, socket: &std::path::Path) -> crate::hiahp::wire::WireHost {
    std::env::set_var("HIMARK_LOG_DIR", dir.join("logs"));
    crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(),
        crate::test_connector(),
        format!("unix:{}", socket.display()),
    )
}

#[test]
fn the_seat_reconnects_after_a_host_restart() {
    use himark::higent::AhpServer;
    let host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let spawned = spawn_host(dir.path(), "false");
    let socket = spawned.socket.clone();
    let seat = reconnect_seat(dir.path(), &socket);

    drive(seat.connect()).expect("the first connect");
    let session = drive(seat.create_session(
        vec![format!("file://{}", dir.path().display())],
        himark::higent::SessionOptions::default(),
    ))
    .expect("a session");

    let chat = drive(seat.subscribe_session(session.clone()))
        .expect("the session subscribes")
        .default_chat
        .expect("the session carries its default chat");
    drive(seat.subscribe_chat(chat.clone())).expect("the chat subscribes");

    drive(seat.start_turn(chat.clone(), "before the crash".to_owned(), None, None))
        .expect("the pre-crash turn");
    let seeded = drive(seat.poll_chat(chat.clone()));
    assert!(!seeded.is_empty(), "the pre-crash send echoes");

    drop(spawned);
    let mut failed = false;
    for _ in 0..50 {
        if drive(seat.list_sessions(None)).is_err() {
            failed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(failed, "a call against the dead host must fail");

    let respawned = spawn_host(dir.path(), "false");
    let mut recovered = false;
    for _ in 0..100 {
        if drive(seat.list_sessions(None)).is_ok() {
            recovered = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(recovered, "the seat must reconnect to the respawned host");

    drive(seat.start_turn(chat.clone(), "hello again".to_owned(), None, None))
        .expect("the turn starts against the respawned host");
    let mut poll = seat.poll_chat(chat.clone());
    let actions = {
        use std::task::{Context, Poll};
        let waker = std::task::Waker::noop();
        let mut ctx = Context::from_waker(waker);
        let mut answered = None;
        for _ in 0..2_000 {
            if let Poll::Ready(value) = poll.as_mut().poll(&mut ctx) {
                answered = Some(value);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        match answered {
            Some(actions) => actions,
            None => {
                let host_log = dir.path().join("host-home").join("host.log");
                eprintln!(
                    "==== host trace ====\n{}",
                    std::fs::read_to_string(&host_log).unwrap_or_default()
                );
                if let Ok(entries) = std::fs::read_dir(dir.path().join("logs")) {
                    for entry in entries.flatten() {
                        eprintln!(
                            "==== {:?} ====\n{}",
                            entry.file_name(),
                            std::fs::read_to_string(entry.path()).unwrap_or_default()
                        );
                    }
                }
                panic!("the resumed chat feed answered nothing");
            }
        }
    };
    assert!(
        !actions.is_empty(),
        "the resumed chat feed must carry the send's actions"
    );
    drop(respawned);
    drop(host);
}

#[test]
fn the_seat_replays_the_gap_after_a_connection_drop() {
    use himark::higent::AhpServer;
    let host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let spawned = spawn_host(dir.path(), "false");
    let socket = spawned.socket.clone();

    let proxy_path = dir.path().join("proxy.sock");
    let proxy = SeverableProxy::start(&proxy_path, &socket);
    let seat = reconnect_seat(dir.path(), &proxy_path);

    drive(seat.connect()).expect("the first connect");
    let session = drive(seat.create_session(
        vec![format!("file://{}", dir.path().display())],
        himark::higent::SessionOptions::default(),
    ))
    .expect("a session");
    let chat = drive(seat.create_chat(session.clone())).expect("a chat");
    drive(seat.subscribe_chat(chat.clone())).expect("the chat subscribes");

    proxy.sever();

    let second = reconnect_seat(dir.path(), &socket);
    drive(second.connect()).expect("the second seat connects");
    drive(second.start_turn(chat.clone(), "missed you".to_owned(), None, None))
        .expect("the gap turn starts");

    let _proxy = SeverableProxy::start(&proxy_path, &socket);
    let mut recovered = false;
    for _ in 0..100 {
        if drive(seat.list_sessions(None)).is_ok() {
            recovered = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        recovered,
        "the seat must reconnect through the restored proxy"
    );
    let actions = drive(seat.poll_chat(chat.clone()));
    assert!(
        !actions.is_empty(),
        "the gap's actions must REPLAY into the standing chat feed"
    );
    drop(spawned);
    drop(host);
}

/// A document channel must survive reconnects: the host's reconnect
/// handler used to omit document channels from its known-channel
/// test, so a reconnect dumped them into `missing` and NEVER
/// re-subscribed the outbox — after which broadcasts reached nobody
/// and the buffer silently desynced. This drives one client (alice)
/// through a proxy we can sever while a second client (bob) edits
/// the shared document live, across several sever/restore cycles,
/// plus one edit made WHILE alice is disconnected (the gap replay).
#[test]
fn a_document_channel_survives_reconnects() {
    use documents::sync::{SyncEdit, SyncState};
    use himark::higent::AhpServer;
    use rebase::RebaseLog;

    let host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let spawned = spawn_host(dir.path(), "false");
    let socket = spawned.socket.clone();

    // Bob talks straight to the host; alice rides a severable proxy.
    let proxy_path = dir.path().join("proxy.sock");
    let mut proxy = SeverableProxy::start(&proxy_path, &socket);
    let alice_seat: Arc<dyn AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(),
        crate::test_connector(),
        format!("unix:{}", proxy_path.display()),
    ));
    let bob_seat: Arc<dyn AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(),
        crate::test_connector(),
        format!("unix:{}", socket.display()),
    ));
    drive(alice_seat.connect()).expect("alice connects");
    drive(bob_seat.connect()).expect("bob connects");

    let file = dir.path().join("shared.md");
    std::fs::write(&file, "shared\n").unwrap();
    let uri = himark::higent::ResourceUri::new(format!(
        "file://{}",
        file.canonicalize().expect("canonical").display()
    ));
    let session = "hihost-fs:/local".to_owned();
    let opened_a = drive(alice_seat.open_document(session.clone(), Some(uri.clone()), None))
        .expect("alice opens");
    let opened_b =
        drive(bob_seat.open_document(session.clone(), Some(uri.clone()), None)).expect("bob opens");
    assert_eq!(opened_a.document, opened_b.document, "idempotent open");
    let channel = opened_a.document;
    let snapshot_a =
        drive(alice_seat.subscribe_document(channel.clone())).expect("alice subscribes");
    let snapshot_b = drive(bob_seat.subscribe_document(channel.clone())).expect("bob subscribes");

    let replica = |snapshot: &himark_ahp_ext_types::DocumentState| {
        RebaseLog::new(
            SyncState::new(
                himark::Text::from_string_exact(&snapshot.text),
                himark::EditLog::new(),
            ),
            snapshot.version,
        )
    };
    let mut alice = replica(&snapshot_a);
    let mut bob = replica(&snapshot_b);

    fn typed(
        log: &mut RebaseLog<himark_ahp_ext_types::Uid, SyncEdit>,
        id: himark_ahp_ext_types::Uid,
        at: usize,
        insert: &str,
    ) -> himark_ahp_ext_types::DocumentApplied {
        let state = log.display();
        let mut builder = operation::OperationBuilder::new();
        if at > 0 {
            builder.push_retain(at as u32);
        }
        builder.push_insert(insert.to_owned());
        let tail = state.text.byte_count() - at;
        if tail > 0 {
            builder.push_retain(tail as u32);
        }
        let edit = SyncEdit::captured(
            state.log.clone(),
            builder.finish(),
            himark::EditIdentity::mint(),
        );
        let dispatch = log.local(id, edit).expect("a settled edit dispatches");
        himark_ahp_ext_types::DocumentApplied {
            base: dispatch.base,
            operation: crate::hiahp::docsync::wire_operation(
                &dispatch.before.text,
                dispatch.action.operation().expect("it landed"),
            ),
            id: dispatch.id,
            origin: None,
        }
    }

    fn absorb(
        log: &mut RebaseLog<himark_ahp_ext_types::Uid, SyncEdit>,
        action: &himark_ahp_ext_types::DocumentApplied,
    ) {
        if log.ack(&action.id) {
            return;
        }
        log.remote(
            action.id,
            SyncEdit::Theirs {
                resolve: crate::hiahp::docsync::resolve_wire(action.operation.clone()),
            },
        );
    }

    let shown = |log: &RebaseLog<himark_ahp_ext_types::Uid, SyncEdit>| {
        let text = &log.display().text;
        text.view().substring(0..text.byte_count() as u32)
    };

    // `poll_document` runs in the caller's context and parks on a
    // tokio timer while it waits for a feed, so it must be driven
    // INSIDE the runtime (unlike the other seat calls, which do their
    // work on the runtime and hand `drive` a plain result slot). The
    // active is always live here — the reconnect is forced via
    // `list_sessions` on the bare thread first.
    // A bounded drain that never hangs on an empty feed — the
    // straggler-duplicate probe.
    let blk_try =
        |future: himark::higent::SeatFuture<Vec<himark_ahp_ext_types::DocumentApplied>>,
         millis: u64| {
            crate::hiahp::wire::test_runtime()
                .block_on(async move {
                    tokio::time::timeout(std::time::Duration::from_millis(millis), future).await
                })
                .unwrap_or_default()
        };

    // One bob edit reaches alice EXACTLY ONCE and converges. The
    // second, bounded poll is the duplicate detector: a leaked or
    // doubled subscription would enqueue a straggler here.
    let mut nonce = 0u128;
    let mut bob_types_and_alice_hears = |alice_seat: &Arc<dyn AhpServer>,
                                         alice: &mut RebaseLog<_, _>,
                                         bob: &mut RebaseLog<_, _>,
                                         at: usize,
                                         insert: &str| {
        nonce += 1;
        let action = typed(bob, himark_ahp_ext_types::Uid(0xb000 + nonce), at, insert);
        bob_seat.dispatch_document(&channel, action);
        // Bob drains his own echo so his log settles.
        let echo = blk_try(bob_seat.poll_document(channel.clone()), 800);
        for a in &echo {
            absorb(bob, a);
        }
        // Alice hears it once.
        let heard = blk_try(alice_seat.poll_document(channel.clone()), 800);

        assert_eq!(
            heard.len(),
            1,
            "alice hears bob's edit exactly once: {heard:?}"
        );
        for a in &heard {
            absorb(alice, a);
        }
        let straggler = blk_try(alice_seat.poll_document(channel.clone()), 300);

        assert!(straggler.is_empty(), "no duplicate delivery: {straggler:?}");
        assert_eq!(shown(alice), shown(bob), "alice and bob converge");
    };

    // Baseline, connection healthy.
    bob_types_and_alice_hears(&alice_seat, &mut alice, &mut bob, 0, "A");
    assert_eq!(shown(&alice), "Ashared\n");

    // Several sever/restore cycles, a live bob edit across each.
    for cycle in 0..3 {
        proxy.sever();
        drop(proxy);
        proxy = SeverableProxy::start(&proxy_path, &socket);
        // Force alice's reconnect from the TEST thread (the runtime
        // refuses a reconnect requested from within itself).
        let mut recovered = false;
        for _ in 0..100 {
            if drive(alice_seat.list_sessions(None)).is_ok() {
                recovered = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(recovered, "alice reconnects on cycle {cycle}");
        bob_types_and_alice_hears(&alice_seat, &mut alice, &mut bob, 0, "B");
    }

    // The gap: bob edits WHILE alice is severed; the reconnect replay
    // must deliver the missed edit.
    proxy.sever();
    nonce += 1;
    let missed = typed(&mut bob, himark_ahp_ext_types::Uid(0xb000 + nonce), 0, "C");
    bob_seat.dispatch_document(&channel, missed);
    for a in &blk_try(bob_seat.poll_document(channel.clone()), 800) {
        absorb(&mut bob, a);
    }
    drop(proxy);
    let _proxy = SeverableProxy::start(&proxy_path, &socket);
    let mut recovered = false;
    for _ in 0..100 {
        if drive(alice_seat.list_sessions(None)).is_ok() {
            recovered = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(recovered, "alice reconnects after the gap");
    // The missed edit replays into alice's standing feed.
    let mut converged = false;
    for _ in 0..200 {
        for a in &blk_try(alice_seat.poll_document(channel.clone()), 200) {
            absorb(&mut alice, a);
        }
        if shown(&alice) == shown(&bob) {
            converged = true;
            break;
        }
    }
    assert!(
        converged,
        "the gap edit replays on reconnect: alice={:?} bob={:?}",
        shown(&alice),
        shown(&bob)
    );

    // Alice's OWN edits still flow to bob after all the churn.
    nonce += 1;
    let from_alice = typed(
        &mut alice,
        himark_ahp_ext_types::Uid(0xa000 + nonce),
        0,
        "Z",
    );
    alice_seat.dispatch_document(&channel, from_alice);
    for a in &blk_try(alice_seat.poll_document(channel.clone()), 800) {
        absorb(&mut alice, a);
    }
    let mut bob_saw = false;
    for _ in 0..200 {
        for a in &blk_try(bob_seat.poll_document(channel.clone()), 200) {
            absorb(&mut bob, a);
        }
        if shown(&bob) == shown(&alice) {
            bob_saw = true;
            break;
        }
    }
    assert!(
        bob_saw,
        "alice's post-reconnect edit reaches bob: alice={:?} bob={:?}",
        shown(&alice),
        shown(&bob)
    );

    drop(spawned);
    drop(host);
}

#[test]
fn the_keepalive_detects_a_deaf_host_and_recovers() {
    use himark::higent::AhpServer;
    let host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("HIHOST_PING_SECS", "1");
    std::env::set_var("HIHOST_CONNECT_SECS", "2");
    let dir = tempfile::tempdir().expect("tempdir");
    let spawned = spawn_host(dir.path(), "false");
    let socket = spawned.socket.clone();
    let seat = reconnect_seat(dir.path(), &socket);
    drive(seat.connect()).expect("the first connect");

    let pid = spawned.child.id().to_string();
    assert!(std::process::Command::new("kill")
        .args(["-STOP", &pid])
        .status()
        .expect("kill -STOP")
        .success());

    let mut failed = false;
    for _ in 0..20 {
        if drive(seat.list_sessions(None)).is_err() {
            failed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert!(
        failed,
        "the deaf host must surface as failing calls, never hangs"
    );

    assert!(std::process::Command::new("kill")
        .args(["-CONT", &pid])
        .status()
        .expect("kill -CONT")
        .success());
    let mut recovered = false;
    for _ in 0..100 {
        if drive(seat.list_sessions(None)).is_ok() {
            recovered = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(recovered, "the woken host must serve the reconnected seat");
    std::env::remove_var("HIHOST_PING_SECS");
    std::env::remove_var("HIHOST_CONNECT_SECS");
    drop(spawned);
    drop(host);
}

struct SeverableProxy {
    cut: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl SeverableProxy {
    fn start(at: &std::path::Path, to: &std::path::Path) -> SeverableProxy {
        let _ = std::fs::remove_file(at);
        let listener = std::os::unix::net::UnixListener::bind(at).expect("proxy binds");
        let cut = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&cut);
        let to = to.to_path_buf();
        let at = at.to_path_buf();
        std::thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("nonblocking listener");
            loop {
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = std::fs::remove_file(&at);
                    return;
                }
                match listener.accept() {
                    Ok((client, _)) => {
                        let upstream = match std::os::unix::net::UnixStream::connect(&to) {
                            Ok(upstream) => upstream,
                            Err(_) => return,
                        };
                        let pump = |from: std::os::unix::net::UnixStream,
                                    to: std::os::unix::net::UnixStream,
                                    flag: std::sync::Arc<std::sync::atomic::AtomicBool>| {
                            std::thread::spawn(move || {
                                use std::io::{Read, Write};
                                let mut from = from;
                                let mut to = to;
                                from.set_read_timeout(Some(std::time::Duration::from_millis(50)))
                                    .ok();
                                let mut buffer = [0u8; 4096];
                                loop {
                                    if flag.load(std::sync::atomic::Ordering::Relaxed) {
                                        let _ = to.shutdown(std::net::Shutdown::Both);
                                        let _ = from.shutdown(std::net::Shutdown::Both);
                                        return;
                                    }
                                    match from.read(&mut buffer) {
                                        Ok(0) => {
                                            let _ = to.shutdown(std::net::Shutdown::Both);
                                            return;
                                        }
                                        Ok(count) => {
                                            if to.write_all(&buffer[..count]).is_err() {
                                                return;
                                            }
                                        }
                                        Err(error)
                                            if error.kind()
                                                == std::io::ErrorKind::WouldBlock => {}
                                        Err(_) => {
                                            let _ = to.shutdown(std::net::Shutdown::Both);
                                            return;
                                        }
                                    }
                                }
                            });
                        };
                        let back = client.try_clone().expect("proxy clone");
                        let forward = upstream.try_clone().expect("proxy clone");
                        pump(client, forward, std::sync::Arc::clone(&flag));
                        pump(upstream, back, std::sync::Arc::clone(&flag));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });
        SeverableProxy { cut }
    }

    fn sever(&self) {
        self.cut.store(true, std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[test]
fn the_drawer_connects_to_the_himark_host() {
    std::env::set_var("HIMARK_AGENT_LATENCY_MS", "0");
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("host.sock");
    let serving = socket.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("host runtime");
        let host = agent_host::Host::new(agent_host::HostConfig::default());
        let _ = runtime.block_on(host.bind(&serving));
    });
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(20));
        waited += 1;
        assert!(waited < 250, "the host never bound its socket");
    }

    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);

    let seat: std::sync::Arc<dyn himark::higent::AhpServer> =
        std::sync::Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
    let _ours = engine.register_agent_server("himark Host", seat);
    himark::higent::Agents::install_new_session(
        &mut engine.app.store_mut(),
        std::sync::Arc::new(move |server| {
            std::sync::Arc::new(StubNewSession {
                server,
                directory: "file:///tmp/hihost-e2e".to_owned(),
            })
        }),
    );

    assert!(engine.perform_command(window, "agent.toggle-agents"));
    let mut rows = Vec::new();
    for _ in 0..30 {
        for _ in 0..6 {
            let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
            settle(&mut engine);
        }
        rows = drawer_rows(&engine).expect("the agents drawer is up");
        let connected = rows
            .iter()
            .position(|(label, depth)| *depth == 0 && label == "himark Host")
            .is_some_and(|index| rows.get(index + 1) == Some(&("+ New Session…".to_owned(), 1)));
        if connected {
            return;
        }
    }
    panic!("the himark host row never connected: {rows:?}");
}

#[test]
fn the_floating_chat_rides_the_bottom_sheet() {
    std::env::set_var("HIMARK_AGENT_LATENCY_MS", "0");
    std::env::set_var("HIMARK_AHP_URL", "ws://127.0.0.1:9/unreachable");
    let dir = tempfile::tempdir().expect("tempdir");
    let stub = agent_host::testing::fake_cli_command(dir.path());
    let host = spawn_host(dir.path(), &stub);
    let socket = host.socket.clone();

    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    himark::FloatingChat::set(&mut engine.app.store_mut(), true);
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);

    let seat: std::sync::Arc<dyn himark::higent::AhpServer> =
        std::sync::Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
    let _ours = engine.register_agent_server("himark Host", seat);
    let workdir = dir.path().to_owned();
    himark::higent::Agents::install_new_session(
        &mut engine.app.store_mut(),
        std::sync::Arc::new(move |server| {
            std::sync::Arc::new(StubNewSession {
                server,
                directory: format!("file://{}", workdir.display()),
            })
        }),
    );
    let pump = |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface| {
        for _ in 0..6 {
            let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
            settle(engine);
        }
    };

    assert!(engine.perform_command(window, "agent.toggle-agents"));
    let mut plus = None;
    for _ in 0..30 {
        pump(&mut engine, &mut surface);
        let rows = drawer_rows(&engine).expect("the agents drawer is up");
        let settled = !rows.iter().any(|(label, _)| label.contains("connecting"));
        plus = rows
            .iter()
            .position(|(label, depth)| *depth == 0 && label == "himark Host")
            .and_then(|index| {
                (rows.get(index + 1) == Some(&("+ New Session…".to_owned(), 1)))
                    .then_some(index + 1)
            })
            .filter(|_| settled);
        if plus.is_some() {
            break;
        }
    }
    pick_drawer_row(&mut engine, plus.expect("the himark host row connected"));

    let floating = |engine: &HimarkEngine| -> Option<himark::higent::ChatPanel> {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())?;
        let pane = entity.bottom_pane()?;
        let pane = pane.as_any().downcast_ref::<himark::higent::ChatPane>()?;
        himark::higent::Chats::chat(engine.app.store(), pane.chat())
    };
    let mut waited = 0;
    while floating(&engine).is_none_or(|chat| !chat.ready()) && waited < 50 {
        pump(&mut engine, &mut surface);
        waited += 1;
    }
    assert!(
        floating(&engine).is_some_and(|chat| chat.ready()),
        "the sheet's chat subscribed"
    );
    {
        let mut pane_mounted = false;
        engine.app.for_each_plugin_panel(&mut |panel| {
            pane_mounted |= panel
                .as_any()
                .downcast_ref::<himark::higent::ChatPane>()
                .is_some();
        });
        assert!(
            !pane_mounted,
            "no chat pane joins the workbench under the flag"
        );
    }
    let expanded = |engine: &HimarkEngine| {
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .and_then(|entity| entity.bottom_expanded())
    };
    assert_eq!(expanded(&engine), Some(false), "collapsed at mount");

    assert!(himark::test_driver::type_text(&mut engine.app, "hello"));
    pump(&mut engine, &mut surface);
    assert_eq!(
        floating(&engine).map(|chat| chat.composer_text()),
        Some("hello".to_owned()),
        "typing lands in the floating composer"
    );

    if let Ok(path) = std::env::var("HIMARK_SHEET_SNAPSHOT_COLLAPSED") {
        surface.canvas().clear(skia_safe::Color::BLACK);
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        let image = surface.image_snapshot();
        let data = image
            .encode(None, skia_safe::EncodedImageFormat::PNG, None)
            .expect("png");
        std::fs::write(path, data.as_bytes()).expect("snapshot");
    }

    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers {
            command: true,
            ..Default::default()
        },
    );
    pump(&mut engine, &mut surface);
    assert_eq!(expanded(&engine), Some(true), "a send expands the sheet");

    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Escape,
        imba::event::Modifiers::default(),
    );
    pump(&mut engine, &mut surface);
    assert_eq!(expanded(&engine), Some(false), "Escape collapses");

    let layer_focus = |engine: &HimarkEngine| {
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .map(|entity| entity.layer_focus())
    };
    assert_eq!(layer_focus(&engine), Some(himark::LayerFocus::Bottom));
    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Escape,
        imba::event::Modifiers::default(),
    );
    pump(&mut engine, &mut surface);
    assert_eq!(
        expanded(&engine),
        Some(true),
        "Escape in the still-focused chat rolls the drawer back out"
    );

    let _ = himark::test_driver::click(&mut engine.app, 200.0, 300.0, 1100.0, 800.0);
    pump(&mut engine, &mut surface);
    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Escape,
        imba::event::Modifiers::default(),
    );
    pump(&mut engine, &mut surface);
    assert_eq!(
        expanded(&engine),
        Some(false),
        "an editor-focused Escape leaves the sheet collapsed"
    );
    let _ = himark::test_driver::type_text(&mut engine.app, "stray");
    pump(&mut engine, &mut surface);
    assert_eq!(
        floating(&engine).map(|chat| chat.composer_text()),
        Some(String::new()),
        "typing after an editor-focused Escape stays out of the composer"
    );

    assert!(
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .and_then(|entity| {
                entity.bottom_rect(engine.app.store(), skia_safe::Size::new(1100.0, 800.0))
            })
            .is_none(),
        "the keys leaving hid the sheet entirely"
    );
    let sole = engine.app.sole_window();
    assert!(engine.app.perform_registered(sole, "chat.composer"));
    pump(&mut engine, &mut surface);
    assert_eq!(
        layer_focus(&engine),
        Some(himark::LayerFocus::Bottom),
        "the composer command brought the sheet back with the keys"
    );
    assert!(himark::test_driver::type_text(&mut engine.app, " again"));
    pump(&mut engine, &mut surface);
    assert_eq!(
        floating(&engine).map(|chat| chat.composer_text()),
        Some(" again".to_owned()),
        "the focused sheet takes the typing"
    );

    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Backspace,
        imba::event::Modifiers::default(),
    );
    pump(&mut engine, &mut surface);
    assert_eq!(
        floating(&engine).map(|chat| chat.composer_text()),
        Some(" agai".to_owned()),
        "keymap commands route to the floating composer"
    );

    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers {
            command: true,
            ..Default::default()
        },
    );
    pump(&mut engine, &mut surface);
    assert_eq!(expanded(&engine), Some(true), "the send expanded again");

    let _ = himark::test_driver::click(&mut engine.app, 10.0, 300.0, 1100.0, 800.0);
    pump(&mut engine, &mut surface);
    assert_eq!(
        expanded(&engine),
        Some(false),
        "losing the keys rolls the drawer away"
    );
    assert_eq!(
        floating(&engine).map(|chat| chat.blurred()),
        Some(true),
        "the prompt blurred to one line"
    );
    if let Ok(path) = std::env::var("HIMARK_SHEET_SNAPSHOT_BLURRED") {
        surface.canvas().clear(skia_safe::Color::BLACK);
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        let image = surface.image_snapshot();
        let data = image
            .encode(None, skia_safe::EncodedImageFormat::PNG, None)
            .expect("png");
        std::fs::write(path, data.as_bytes()).expect("snapshot");
    }
    assert!(engine.app.perform_registered(sole, "chat.composer"));
    pump(&mut engine, &mut surface);
    assert_eq!(
        floating(&engine).map(|chat| chat.blurred()),
        Some(false),
        "showing the sheet again grows the prompt back"
    );

    let replied = |engine: &HimarkEngine| -> bool {
        floating(engine).is_some_and(|chat| {
            chat.transcript().iter().any(|(_, cells)| {
                cells
                    .iter()
                    .any(|(kind, text)| kind == "Agent" && text.contains("OK"))
            })
        })
    };
    let mut waited = 0;
    while !replied(&engine) && waited < 100 {
        pump(&mut engine, &mut surface);
        waited += 1;
    }
    assert!(
        replied(&engine),
        "the reply streamed into the floating chat; host log:\n{}",
        std::fs::read_to_string(dir.path().join("host-home/host.log")).unwrap_or_default()
    );

    if let Ok(path) = std::env::var("HIMARK_SHEET_SNAPSHOT") {
        for beat in 0..10 {
            let _ = himark::test_driver::animate(
                &mut engine.app,
                imba::anim::AnimationClock::from_millis(10_000.0 + 1_000.0 * beat as f64),
            );
            pump(&mut engine, &mut surface);
        }
        surface.canvas().clear(skia_safe::Color::BLACK);
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        let image = surface.image_snapshot();
        let data = image
            .encode(None, skia_safe::EncodedImageFormat::PNG, None)
            .expect("png");
        std::fs::write(path, data.as_bytes()).expect("snapshot");
    }
}

#[test]
#[ignore = "writes /tmp/chat_pane_*.png for visual inspection of chat rendering"]
fn dump_chat_pane_snapshot() {
    std::env::set_var("HIMARK_AGENT_LATENCY_MS", "0");
    std::env::set_var("HIMARK_AHP_URL", "ws://127.0.0.1:9/unreachable");
    let dir = tempfile::tempdir().expect("tempdir");
    let stub = agent_host::testing::fake_cli_command(dir.path());
    let host = spawn_host(dir.path(), &stub);
    let socket = host.socket.clone();

    let floating = std::env::var("HIMARK_DUMP_FLOATING").is_ok();
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    himark::FloatingChat::set(&mut engine.app.store_mut(), floating);
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);

    let seat: std::sync::Arc<dyn himark::higent::AhpServer> =
        std::sync::Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
    let _ours = engine.register_agent_server("himark Host", seat);
    let workdir = dir.path().to_owned();
    himark::higent::Agents::install_new_session(
        &mut engine.app.store_mut(),
        std::sync::Arc::new(move |server| {
            std::sync::Arc::new(StubNewSession {
                server,
                directory: format!("file://{}", workdir.display()),
            })
        }),
    );

    let pump = |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface| {
        for _ in 0..6 {
            let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
            settle(engine);
        }
    };
    let wait_for = |engine: &mut HimarkEngine,
                    surface: &mut skia_safe::Surface,
                    what: &str,
                    done: &mut dyn FnMut(&HimarkEngine) -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !done(engine) {
            assert!(
                std::time::Instant::now() < deadline,
                "never settled within 20s: {what}; transcript: {:?}",
                chat_transcript(engine),
            );
            pump(engine, surface);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };

    assert!(engine.perform_command(window, "agent.toggle-agents"));
    let plus_row = |engine: &HimarkEngine| -> Option<usize> {
        let rows = drawer_rows(engine)?;
        let settled = !rows.iter().any(|(label, _)| label.contains("connecting"));
        rows.iter()
            .position(|(label, depth)| *depth == 0 && label == "himark Host")
            .and_then(|index| {
                (rows.get(index + 1) == Some(&("+ New Session…".to_owned(), 1)))
                    .then_some(index + 1)
            })
            .filter(|_| settled)
    };
    wait_for(&mut engine, &mut surface, "host row", &mut |engine| {
        plus_row(engine).is_some()
    });
    let plus = plus_row(&engine).expect("the himark host row connected");
    pick_drawer_row(&mut engine, plus);
    wait_for(&mut engine, &mut surface, "chat ready", &mut |engine| {
        shown_chat(engine).is_some_and(|chat| chat.ready())
    });

    let send = |engine: &mut HimarkEngine, text: &str| {
        assert!(himark::test_driver::type_text(&mut engine.app, text));
        let _ = himark::test_driver::key(
            &mut engine.app,
            imba::event::Key::Enter,
            imba::event::Modifiers {
                command: true,
                ..Default::default()
            },
        );
    };
    let replies = |engine: &HimarkEngine| -> usize {
        chat_transcript(engine)
            .map(|rows| {
                rows.iter()
                    .flat_map(|(_, cells)| cells.clone())
                    .filter(|(kind, text)| kind == "Agent" && text.contains("OK"))
                    .count()
            })
            .unwrap_or(0)
    };
    for round in 0..3usize {
        send(&mut engine, "hello");
        wait_for(&mut engine, &mut surface, "reply", &mut |engine| {
            replies(engine) > round
        });
    }

    send(&mut engine, "ask permission first");
    wait_for(&mut engine, &mut surface, "ask card", &mut |engine| {
        shown_chat(engine).is_some_and(|chat| chat.permission_oracle().is_some())
    });
    assert!(himark::test_driver::type_text(&mut engine.app, "1"));
    wait_for(&mut engine, &mut surface, "tool ran", &mut |engine| {
        chat_transcript(engine).is_some_and(|rows| {
            rows.iter().any(|(_, cells)| {
                cells
                    .iter()
                    .any(|(kind, text)| kind == "Agent" && text.contains("ran it"))
            })
        })
    });
    pump(&mut engine, &mut surface);
    if floating {
        // A send expands the sheet; slow the reply down so the sheet
        // is still up when the snapshot lands.
        std::env::set_var("HIMARK_AGENT_LATENCY_MS", "3000");
        send(&mut engine, "hello again");
        pump(&mut engine, &mut surface);
        assert_eq!(
            himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
                .and_then(|entity| entity.bottom_expanded()),
            Some(true),
            "the send expanded the sheet"
        );
        // The expand ride is a wall-clock animation; give it real time.
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(30));
            let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
            settle(&mut engine);
        }
        let mut shot = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
        shot.canvas().clear(skia_safe::Color::BLACK);
        let _ = engine.draw(window, shot.canvas(), 1100.0, 800.0, 1.0);
        let image = shot.image_snapshot();
        let data = image
            .encode(None, skia_safe::EncodedImageFormat::PNG, None)
            .expect("png");
        std::fs::write("/tmp/chat_sheet_midexpand.png", data.as_bytes()).expect("snapshot");
        wait_for(&mut engine, &mut surface, "last reply", &mut |engine| {
            replies(engine) > 3
        });
        pump(&mut engine, &mut surface);
    }

    for (scale, path) in [
        (1.0f32, "/tmp/chat_pane_1x.png"),
        (2.0, "/tmp/chat_pane_2x.png"),
    ] {
        let px = (1100.0 * scale) as i32;
        let py = (800.0 * scale) as i32;
        let mut shot = skia_safe::surfaces::raster_n32_premul((px, py)).expect("surface");
        shot.canvas().clear(skia_safe::Color::BLACK);
        let _ = engine.draw(window, shot.canvas(), 1100.0, 800.0, scale);
        let image = shot.image_snapshot();
        let data = image
            .encode(None, skia_safe::EncodedImageFormat::PNG, None)
            .expect("png");
        std::fs::write(path, data.as_bytes()).expect("snapshot");
    }
}

#[test]
fn the_chat_runs_through_the_himark_host() {
    std::env::set_var("HIMARK_AGENT_LATENCY_MS", "0");

    std::env::set_var("HIMARK_AHP_URL", "ws://127.0.0.1:9/unreachable");
    let dir = tempfile::tempdir().expect("tempdir");
    eprintln!("[e2e] dir: {:?}", dir.path());
    let stub = agent_host::testing::fake_cli_command(dir.path());
    let host = spawn_host(dir.path(), &stub);
    let socket = host.socket.clone();

    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());

    himark::FloatingChat::set(&mut engine.app.store_mut(), false);
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);

    struct NoFind;
    impl imba::effect::EffectHandler<himark::FindEffect> for NoFind {
        async fn handle(&self, _effect: himark::FindEffect) -> Vec<himark::ResourceLocation> {
            Vec::new()
        }
    }
    engine.app.register_handler::<himark::FindEffect>(NoFind);

    let seat: std::sync::Arc<dyn himark::higent::AhpServer> =
        std::sync::Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
    let _ours = engine.register_agent_server("himark Host", seat);
    let workdir = dir.path().to_owned();
    himark::higent::Agents::install_new_session(
        &mut engine.app.store_mut(),
        std::sync::Arc::new(move |server| {
            std::sync::Arc::new(StubNewSession {
                server,
                directory: format!("file://{}", workdir.display()),
            })
        }),
    );

    let pump = |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface| {
        for _ in 0..6 {
            let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
            settle(engine);
        }
    };

    let wait_for = |engine: &mut HimarkEngine,
                    surface: &mut skia_safe::Surface,
                    what: &str,
                    done: &mut dyn FnMut(&HimarkEngine) -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !done(engine) {
            assert!(
                std::time::Instant::now() < deadline,
                "never settled within 20s: {what}; transcript: {:?}; drawer: {:?}; host log:\n{}",
                chat_transcript(engine),
                drawer_rows(engine),
                std::fs::read_to_string(dir.path().join("host-home/host.log")).unwrap_or_default()
            );
            pump(engine, surface);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };

    assert!(engine.perform_command(window, "agent.toggle-agents"));
    let plus_row = |engine: &HimarkEngine| -> Option<usize> {
        let rows = drawer_rows(engine)?;
        let settled = !rows.iter().any(|(label, _)| label.contains("connecting"));
        rows.iter()
            .position(|(label, depth)| *depth == 0 && label == "himark Host")
            .and_then(|index| {
                (rows.get(index + 1) == Some(&("+ New Session…".to_owned(), 1)))
                    .then_some(index + 1)
            })
            .filter(|_| settled)
    };
    wait_for(
        &mut engine,
        &mut surface,
        "the himark host row connected",
        &mut |engine| plus_row(engine).is_some(),
    );
    let plus = plus_row(&engine).expect("the himark host row connected");
    pick_drawer_row(&mut engine, plus);
    let chat_ready =
        |engine: &HimarkEngine| -> bool { shown_chat(engine).is_some_and(|chat| chat.ready()) };
    wait_for(
        &mut engine,
        &mut surface,
        "the chat over our host subscribed",
        &mut |engine| chat_ready(engine),
    );

    assert!(himark::test_driver::type_text(&mut engine.app, "hello"));
    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers {
            command: true,
            ..Default::default()
        },
    );
    let replied = |engine: &HimarkEngine| -> bool {
        chat_transcript(engine).is_some_and(|rows| {
            rows.iter().any(|(_, cells)| {
                cells
                    .iter()
                    .any(|(kind, text)| kind == "Agent" && text.contains("OK"))
            })
        })
    };
    wait_for(
        &mut engine,
        &mut surface,
        "the reply streamed home",
        &mut |engine| replied(engine),
    );

    let ask_up = |engine: &HimarkEngine| -> bool {
        shown_chat(engine).is_some_and(|chat| chat.permission_oracle().is_some())
    };
    assert!(himark::test_driver::type_text(
        &mut engine.app,
        "ask permission first"
    ));
    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers {
            command: true,
            ..Default::default()
        },
    );
    wait_for(
        &mut engine,
        &mut surface,
        "the ask card raised",
        &mut |engine| ask_up(engine),
    );

    assert!(himark::test_driver::type_text(&mut engine.app, "1"));
    let ran = |engine: &HimarkEngine| -> bool {
        chat_transcript(engine).is_some_and(|rows| {
            rows.iter().any(|(_, cells)| {
                cells
                    .iter()
                    .any(|(kind, text)| kind == "Agent" && text.contains("ran it"))
            })
        })
    };
    wait_for(
        &mut engine,
        &mut surface,
        "the approved tool completed and the reply landed",
        &mut |engine| ran(engine),
    );
    assert!(!ask_up(&engine), "the ask cleared");

    let tool_rows: Vec<String> = chat_transcript(&engine)
        .expect("the transcript")
        .into_iter()
        .flat_map(|(_, cells)| cells)
        .filter(|(kind, _)| kind == "Tool")
        .map(|(_, text)| text)
        .collect();
    assert!(
        tool_rows.iter().any(|text| text.starts_with("> Bash")),
        "the tool call reads as one collapsed line: {tool_rows:?}"
    );
    assert!(
        tool_rows.iter().all(|text| !text.contains("```")),
        "no fenced body while the run is closed: {tool_rows:?}"
    );

    struct SendLikeComments(&'static str);
    impl himark::DynamicCommand for SendLikeComments {
        fn id(&self) -> &'static str {
            "test.send-like-comments"
        }
        fn name(&self) -> String {
            "Send".to_owned()
        }
        fn perform(
            &self,
            _app: &mut himark::Application,
            store: &mut imba::store::Store,
            window: himark::WindowId,
            fx: &mut himark::AppFx<'_>,
        ) {
            let workspace = himark::Windows::window_ref(store, window)
                .expect("the window entity")
                .current_session();
            let key = himark::higent::Agents::live_session(store, &workspace)
                .expect("the open flow bound the session");
            let chat = himark::higent::Agents::channel(store, &key)
                .and_then(|channel| channel.default_chat)
                .expect("the session names its default chat");
            let seat = himark::higent::Servers::seat(store, key.host).expect("the seat");
            struct DropLanding;
            impl himark::DynamicCommand for DropLanding {
                fn id(&self) -> &'static str {
                    "test.send-landed"
                }
                fn name(&self) -> String {
                    "Sent".to_owned()
                }
                fn perform(
                    &self,
                    _app: &mut himark::Application,
                    _store: &mut imba::store::Store,
                    _window: himark::WindowId,
                    _fx: &mut himark::AppFx<'_>,
                ) {
                }
            }
            fx.push(
                imba::effect::AnyEffect::new(himark::higent::StartTurnEffect {
                    seat,
                    chat,
                    text: self.0.to_owned(),
                    attachments: None,
                    model: None,
                })
                .map(move |_result| {
                    himark::AppCommand::Dynamic(window, std::sync::Arc::new(DropLanding))
                }),
            );
        }
    }
    assert!(engine.app.perform_batch(vec![himark::AppCommand::Dynamic(
        engine.app.sole_window(),
        std::sync::Arc::new(SendLikeComments("attached review comment")),
    )]));
    let shows = |engine: &HimarkEngine, needle: &str| -> bool {
        chat_transcript(engine).is_some_and(|rows| {
            rows.iter()
                .any(|(_, cells)| cells.iter().any(|(_, text)| text.contains(needle)))
        })
    };
    wait_for(
        &mut engine,
        &mut surface,
        "the dock-sent turn shows in the LIVE transcript",
        &mut |engine| shows(engine, "attached review comment"),
    );

    assert!(engine.perform_command(window, "workbench.new-document"));
    pump(&mut engine, &mut surface);
    assert!(
        chat_transcript(&engine).is_none(),
        "the scratch displaced the chat panel"
    );
    assert!(engine.app.perform_batch(vec![himark::AppCommand::Dynamic(
        engine.app.sole_window(),
        std::sync::Arc::new(SendLikeComments("sent while hidden")),
    )]));
    pump(&mut engine, &mut surface);
    let folded = {
        let key = himark::higent::Agents::live_session(
            engine.app.store(),
            &himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
                .expect("the window entity")
                .current_session(),
        )
        .expect("the bound session");
        let chat = himark::higent::Agents::channel(engine.app.store(), &key)
            .and_then(|channel| channel.default_chat)
            .expect("the default chat");
        let seat = himark::higent::Servers::seat(engine.app.store(), key.host).expect("the seat");
        fn block_on<T>(future: himark::higent::SeatFuture<T>) -> T {
            use std::task::{Context, Poll, Wake, Waker};
            struct Unpark(std::thread::Thread);
            impl Wake for Unpark {
                fn wake(self: std::sync::Arc<Self>) {
                    self.0.unpark();
                }
            }
            let waker = Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
            let mut context = Context::from_waker(&waker);
            let mut future = future;
            loop {
                match future.as_mut().poll(&mut context) {
                    Poll::Ready(value) => return value,
                    Poll::Pending => {
                        std::thread::park_timeout(std::time::Duration::from_millis(100))
                    }
                }
            }
        }
        let mut waited = 0;
        loop {
            let snapshot = block_on(seat.subscribe_chat(chat.clone()));
            if let Ok(state) = &snapshot {
                if state
                    .turns
                    .iter()
                    .any(|turn| turn.message.text.contains("sent while hidden"))
                {
                    break true;
                }
            }
            waited += 1;
            if waited > 400 {
                break false;
            }
            pump(&mut engine, &mut surface);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    assert!(folded, "the hidden-send turn folded host-side");

    let title = {
        let store = engine.app.store();
        let chats = himark::higent::Chats::list(store);
        assert_eq!(chats.len(), 1, "the displaced chat's row survives");
        himark::PanelView::title(&himark::higent::ChatPane::new(chats[0].clone()), store)
    };
    assert!(engine.perform_command(window, "peeker.toggle"));
    pump(&mut engine, &mut surface);
    assert!(himark::test_driver::type_text(&mut engine.app, &title));
    pump(&mut engine, &mut surface);
    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers::default(),
    );
    wait_for(
        &mut engine,
        &mut surface,
        "the hidden panel's feed kept folding — the missed turn is present",
        &mut |engine| shows(engine, "sent while hidden"),
    );
    let occurrences = chat_transcript(&engine)
        .map(|rows| {
            rows.iter()
                .flat_map(|(_, cells)| cells.iter())
                .filter(|(_, text)| text.contains("attached review comment"))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(
        occurrences,
        1,
        "one subscription, every message exactly once: {:?}",
        chat_transcript(&engine)
    );
}

#[test]
#[ignore]
fn real_claude_answers_through_the_himark_host() {
    std::env::set_var("HIMARK_AGENT_LATENCY_MS", "0");
    let dir = tempfile::tempdir().expect("tempdir");
    let host = spawn_host_with_env(dir.path(), "claude", false);
    let socket = host.socket.clone();

    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);

    let seat: std::sync::Arc<dyn himark::higent::AhpServer> =
        std::sync::Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
    let _ours = engine.register_agent_server("himark Host", seat);
    let workdir = dir.path().to_owned();
    himark::higent::Agents::install_new_session(
        &mut engine.app.store_mut(),
        std::sync::Arc::new(move |server| {
            std::sync::Arc::new(StubNewSession {
                server,
                directory: format!("file://{}", workdir.display()),
            })
        }),
    );

    let pump = |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface| {
        for _ in 0..6 {
            let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
            settle(engine);
        }
    };
    assert!(engine.perform_command(window, "agent.toggle-agents"));
    let mut plus = None;
    for _ in 0..30 {
        pump(&mut engine, &mut surface);
        let rows = drawer_rows(&engine).expect("the agents drawer is up");
        let settled = !rows.iter().any(|(label, _)| label.contains("connecting"));
        plus = rows
            .iter()
            .position(|(label, depth)| *depth == 0 && label == "himark Host")
            .and_then(|index| {
                (rows.get(index + 1) == Some(&("+ New Session…".to_owned(), 1)))
                    .then_some(index + 1)
            })
            .filter(|_| settled);
        if plus.is_some() {
            break;
        }
    }
    pick_drawer_row(&mut engine, plus.expect("connected"));
    let chat_ready =
        |engine: &HimarkEngine| -> bool { shown_chat(engine).is_some_and(|chat| chat.ready()) };
    let mut waited = 0;
    while !chat_ready(&engine) && waited < 50 {
        pump(&mut engine, &mut surface);
        waited += 1;
    }
    assert!(chat_ready(&engine), "the chat subscribed");

    assert!(himark::test_driver::type_text(
        &mut engine.app,
        "Reply with the single word OK and nothing else."
    ));
    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers {
            command: true,
            ..Default::default()
        },
    );
    let replied = |engine: &HimarkEngine| -> bool {
        chat_transcript(engine).is_some_and(|rows| {
            rows.iter().any(|(_, cells)| {
                cells
                    .iter()
                    .any(|(kind, text)| kind == "Agent" && text.contains("OK"))
            })
        })
    };

    let started = std::time::Instant::now();
    while !replied(&engine) && started.elapsed() < std::time::Duration::from_secs(120) {
        pump(&mut engine, &mut surface);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    println!("elapsed: {:?}", started.elapsed());
    assert!(
        replied(&engine),
        "real Claude answered through OUR host: {:?}",
        chat_transcript(&engine)
    );
}

#[test]
fn the_session_workspace_lists_and_opens_files_through_the_himark_host() {
    std::env::set_var("HIMARK_AGENT_LATENCY_MS", "0");
    std::env::set_var("HIMARK_AHP_URL", "ws://127.0.0.1:9/unreachable");
    let dir = tempfile::tempdir().expect("tempdir");

    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).expect("workdir");
    std::fs::write(work.join("hello.txt"), "from the host\n").expect("seed");
    let stub = agent_host::testing::fake_cli_command(dir.path());
    let host = spawn_host(dir.path(), &stub);
    let socket = host.socket.clone();

    let (_host, mut engine, window, _fs) = hosted_engine();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);

    let seat: std::sync::Arc<dyn himark::higent::AhpServer> =
        std::sync::Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
    let _ours = engine.register_agent_server("himark Host", seat);
    let workdir = work.clone();
    himark::higent::Agents::install_new_session(
        &mut engine.app.store_mut(),
        std::sync::Arc::new(move |server| {
            std::sync::Arc::new(StubNewSession {
                server,
                directory: format!("file://{}", workdir.display()),
            })
        }),
    );

    let pump = |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface| {
        for _ in 0..6 {
            let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
            settle(engine);
        }
    };
    assert!(engine.perform_command(window, "agent.toggle-agents"));
    let mut plus = None;
    for _ in 0..30 {
        pump(&mut engine, &mut surface);
        let rows = drawer_rows(&engine).expect("the agents drawer is up");
        let settled = !rows.iter().any(|(label, _)| label.contains("connecting"));
        plus = rows
            .iter()
            .position(|(label, depth)| *depth == 0 && label == "himark Host")
            .and_then(|index| {
                (rows.get(index + 1) == Some(&("+ New Session…".to_owned(), 1)))
                    .then_some(index + 1)
            })
            .filter(|_| settled);
        if plus.is_some() {
            break;
        }
    }
    let plus = plus.expect("connected");
    pick_drawer_row(&mut engine, plus);

    let current_folders = |engine: &HimarkEngine| -> Vec<himark::ResourceLocation> {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        himark::higent::session_folders(engine.app.store(), &entity.current_session())
    };
    let mut waited = 0;
    while current_folders(&engine).is_empty() && waited < 50 {
        pump(&mut engine, &mut surface);
        waited += 1;
    }
    let folders = current_folders(&engine);
    assert_eq!(folders.len(), 1, "the session bound its workspace");

    {
        let (server, session) =
            himark::higent::seat::parse(folders[0].authority().as_str()).expect("an ahp authority");
        let seat = himark::higent::Servers::seat(engine.app.store(), server).expect("the seat");
        let listed = block_on_seat(seat.resource_list(
            session,
            himark::higent::ResourceUri::new(format!("file://{}", work.display())),
        ))
        .expect("the seat lists");
        assert!(
            listed.iter().any(|(name, _)| name == "hello.txt"),
            "the seat sees the seeded file: {listed:?}"
        );
    }

    assert!(engine.perform_command(window, "agent.toggle-agents"));
    pump(&mut engine, &mut surface);
    assert!(engine.perform_command(window, "agent.toggle-agents"));
    let mut waited = 0;
    loop {
        pump(&mut engine, &mut surface);
        let _ = himark::test_driver::animate(
            &mut engine.app,
            imba::anim::AnimationClock::from_millis(500.0 * (waited + 1) as f64),
        );
        pump(&mut engine, &mut surface);
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        if entity.side_panel().is_none() {
            break;
        }
        waited += 1;
        assert!(waited < 30, "the drawer never rolled away");
    }
    assert!(engine.perform_command(window, "files.tree"));
    pump(&mut engine, &mut surface);
    {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity");
        assert!(entity.dock_panel().is_some(), "the tree panel is up");
    }
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(0.0),
    );
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(1_000.0),
    );
    pump(&mut engine, &mut surface);
    let clicked =
        himark::test_driver::click(&mut engine.app, dock_x(40.0), tree_row_y(0), 900.0, 700.0);
    assert!(clicked, "the root row took the click");
    pump(&mut engine, &mut surface);
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(2_000.0),
    );
    let _ = himark::test_driver::animate(
        &mut engine.app,
        imba::anim::AnimationClock::from_millis(3_000.0),
    );
    pump(&mut engine, &mut surface);

    if let Ok(path) = std::env::var("HIHOST_FS_SNAPSHOT") {
        surface.canvas().clear(skia_safe::Color::BLACK);
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        let image = surface.image_snapshot();
        let data = image
            .encode(None, skia_safe::EncodedImageFormat::PNG, None)
            .expect("png");
        std::fs::write(path, data.as_bytes()).expect("snapshot");
    }

    let tree_rows = |engine: &HimarkEngine| -> usize {
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .and_then(|entity| {
                Some(
                    entity
                        .dock_panel()?
                        .as_any()
                        .downcast_ref::<himark::hifiles::SessionTreeView>()?
                        .row_count(),
                )
            })
            .unwrap_or(0)
    };
    let mut waited = 0;
    while tree_rows(&engine) < 2 {
        pump(&mut engine, &mut surface);
        waited += 1;
        assert!(waited < 50, "the expansion listing never landed");
    }

    let clicked =
        himark::test_driver::click(&mut engine.app, dock_x(40.0), tree_row_y(1), 900.0, 700.0);
    assert!(clicked, "the file row took the click");
    let file = himark::ResourceLocation::new(
        himark::ResourceType::document(),
        folders[0].authority().clone(),
        {
            let mut segments = folders[0].path().to_vec();
            segments.push("hello.txt".to_owned());
            segments
        },
    );

    let fetched = probe_fetch(&mut engine, &mut surface, window, file.clone());
    assert_eq!(
        fetched.flatten().as_deref(),
        Some("from the host\n"),
        "the fetch route reads through the seat"
    );
    let mut opened = None;
    for _ in 0..50 {
        pump(&mut engine, &mut surface);
        opened = himark::OpenDocuments::by_location(engine.app.store(), &file);
        if opened.is_some() {
            break;
        }
    }
    let opened = opened.unwrap_or_else(|| {
        let all: Vec<String> = himark::OpenDocuments::list(engine.app.store())
            .iter()
            .filter_map(|(_, doc)| doc.location().map(|l| format!("{:?}", l)))
            .collect();
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("entity");
        panic!(
            "hello.txt opened through the seat; wanted {file:?}; open: {all:?}; host lists: {:?}; host fetches: {:?}; modal up: {}",
            _host.seat.lists.lock().expect("lists"),
            _host.seat.fetches.lock().expect("fetches"),
            entity.plugin_modal().is_some(),
        )
    });
    let text = {
        let document = himark::OpenDocuments::document_ref(engine.app.store(), opened)
            .expect("the opened document");
        let count = document.text().byte_count();
        document.text().view().byte_string(0, count)
    };
    assert!(
        text.contains("from the host"),
        "the content came through the seat: {text:?}"
    );

    settle_until(
        &mut engine,
        "the served open seated its sync loop",
        |engine| hiahp::docsync::SyncSeats::count(engine.app.store()) == 1,
    );
    assert!(engine.perform_command(window, "workbench.close"));
    pump(&mut engine, &mut surface);
    assert_eq!(
        hiahp::docsync::SyncSeats::count(engine.app.store()),
        0,
        "the closed document took its loop with it"
    );
}

#[test]
fn two_wire_clients_converge_on_one_document() {
    let _host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("hosted fs");
    std::env::set_var("HIMARK_HOST_HOME", dir.path().join("home"));
    let socket = dir.path().join("backend.sock");
    let serving = socket.clone();
    let config = agent_host::HostConfig {
        agents: Vec::new(),
        data_dir: dir.path().join("data"),
        claude_binary: "false".to_owned(),
        codex_binary: "false".to_owned(),
        claude_home: dir.path().join("dot-claude"),
        codex_home: dir.path().join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("backend runtime");
        let backend = agent_host::Host::new(config);
        let _ = runtime.block_on(backend.bind(&serving));
    });
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 500, "the backend never bound its socket");
    }

    fn block_on<T>(future: himark::higent::SeatFuture<T>) -> T {
        use std::task::{Context, Poll, Wake, Waker};
        struct Unpark(std::thread::Thread);
        impl Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = future;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "a seat future never answered"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(250));
                }
            }
        }
    }

    let file = dir.path().join("shared.md");
    std::fs::write(&file, "shared text\n").unwrap();
    let uri = himark::higent::ResourceUri::new(format!(
        "file://{}",
        file.canonicalize().expect("canonical").display()
    ));

    let alice_seat: Arc<dyn himark::higent::AhpServer> =
        Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
    let bob_seat: Arc<dyn himark::higent::AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(),
        crate::test_connector(),
        format!("unix:{}", socket.display()),
    ));

    let opened_a =
        block_on(alice_seat.open_document("hihost-fs:/local".to_owned(), Some(uri.clone()), None))
            .expect("alice opens");
    let opened_b =
        block_on(bob_seat.open_document("hihost-fs:/local".to_owned(), Some(uri.clone()), None))
            .expect("bob opens");
    assert_eq!(opened_a.document, opened_b.document, "idempotent open");
    let channel = opened_a.document;

    let snapshot_a =
        block_on(alice_seat.subscribe_document(channel.clone())).expect("alice subscribes");
    let snapshot_b =
        block_on(bob_seat.subscribe_document(channel.clone())).expect("bob subscribes");
    assert_eq!(snapshot_a.text, "shared text\n");
    assert_eq!(snapshot_a.uri.as_deref(), Some(uri.as_str()));

    use documents::sync::{SyncEdit, SyncState};
    use rebase::RebaseLog;
    let replica = |snapshot: &himark_ahp_ext_types::DocumentState| {
        RebaseLog::new(
            SyncState::new(
                himark::Text::from_string_exact(&snapshot.text),
                himark::EditLog::new(),
            ),
            snapshot.version,
        )
    };
    let mut alice = replica(&snapshot_a);
    let mut bob = replica(&snapshot_b);

    fn typed(
        log: &mut RebaseLog<himark_ahp_ext_types::Uid, SyncEdit>,
        id: himark_ahp_ext_types::Uid,
        at: usize,
        insert: &str,
    ) -> himark_ahp_ext_types::DocumentApplied {
        let state = log.display();
        let mut builder = operation::OperationBuilder::new();
        if at > 0 {
            builder.push_retain(at as u32);
        }
        builder.push_insert(insert.to_owned());
        let tail = state.text.byte_count() - at;
        if tail > 0 {
            builder.push_retain(tail as u32);
        }
        let edit = SyncEdit::captured(
            state.log.clone(),
            builder.finish(),
            himark::EditIdentity::mint(),
        );
        let dispatch = log.local(id, edit).expect("a settled edit dispatches");
        himark_ahp_ext_types::DocumentApplied {
            base: dispatch.base,
            operation: crate::hiahp::docsync::wire_operation(
                &dispatch.before.text,
                dispatch.action.operation().expect("it landed"),
            ),
            id: dispatch.id,
            origin: None,
        }
    }

    fn heard(
        log: &mut RebaseLog<himark_ahp_ext_types::Uid, SyncEdit>,
        action: &himark_ahp_ext_types::DocumentApplied,
    ) -> Vec<himark_ahp_ext_types::DocumentApplied> {
        if log.ack(&action.id) {
            return Vec::new();
        }
        log.remote(
            action.id,
            SyncEdit::Theirs {
                resolve: crate::hiahp::docsync::resolve_wire(action.operation.clone()),
            },
        );
        let mut again = Vec::new();
        while let Some(dispatch) = log.step() {
            again.push(himark_ahp_ext_types::DocumentApplied {
                base: dispatch.base,
                operation: crate::hiahp::docsync::wire_operation(
                    &dispatch.before.text,
                    dispatch.action.operation().expect("it landed"),
                ),
                id: dispatch.id,
                origin: None,
            });
        }
        again
    }

    let shown = |log: &RebaseLog<himark_ahp_ext_types::Uid, SyncEdit>| {
        let text = &log.display().text;
        let end = text.byte_count() as u32;
        text.view().substring(0..end)
    };

    let from_alice = typed(&mut alice, himark_ahp_ext_types::Uid(0xa1), 0, "Alice: ");
    alice_seat.dispatch_document(&channel, from_alice);
    let echoes = block_on(alice_seat.poll_document(channel.clone()));
    assert_eq!(echoes.len(), 1, "{echoes:?}");
    assert!(
        heard(&mut alice, &echoes[0]).is_empty(),
        "her own echo confirms, nothing to re-send"
    );

    let from_bob = typed(&mut bob, himark_ahp_ext_types::Uid(0xb1), 6, " (edited)");
    bob_seat.dispatch_document(&channel, from_bob);
    let seen = block_on(bob_seat.poll_document(channel.clone()));
    let mut redispatched = Vec::new();
    for action in &seen {
        redispatched.extend(heard(&mut bob, action));
    }
    assert!(
        !redispatched.is_empty(),
        "bob's stale chain died and rebases: {seen:?}"
    );
    for action in redispatched {
        bob_seat.dispatch_document(&channel, action);
    }

    loop {
        let actions = block_on(bob_seat.poll_document(channel.clone()));
        for action in &actions {
            let _ = heard(&mut bob, action);
        }
        if *bob.version() == himark_ahp_ext_types::Uid(0xb1) {
            break;
        }
    }
    loop {
        let actions = block_on(alice_seat.poll_document(channel.clone()));
        for action in &actions {
            let _ = heard(&mut alice, action);
        }
        if *alice.version() == himark_ahp_ext_types::Uid(0xb1) {
            break;
        }
    }
    assert_eq!(shown(&alice), "Alice: shared (edited) text\n");
    assert_eq!(shown(&bob), shown(&alice), "CONVERGED");
    assert!(alice.is_settled() && bob.is_settled());
}

#[test]
fn two_wire_clients_share_annotations() {
    let _host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("hosted fs");
    std::env::set_var("HIMARK_HOST_HOME", dir.path().join("home"));
    let socket = dir.path().join("backend.sock");
    let serving = socket.clone();
    let config = agent_host::HostConfig {
        agents: Vec::new(),
        data_dir: dir.path().join("data"),
        claude_binary: "false".to_owned(),
        codex_binary: "false".to_owned(),
        claude_home: dir.path().join("dot-claude"),
        codex_home: dir.path().join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("backend runtime");
        let backend = agent_host::Host::new(config);
        let _ = runtime.block_on(backend.bind(&serving));
    });
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 500, "the backend never bound its socket");
    }

    fn block_on<T>(future: himark::higent::SeatFuture<T>) -> T {
        use std::task::{Context, Poll, Wake, Waker};
        struct Unpark(std::thread::Thread);
        impl Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = future;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "a seat future never answered"
                    );
                    std::thread::park_timeout(std::time::Duration::from_millis(250));
                }
            }
        }
    }

    let session = "hihost-fs:/local".to_owned();
    let alice: Arc<dyn himark::higent::AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(),
        crate::test_connector(),
        format!("unix:{}", socket.display()),
    ));
    let bob: Arc<dyn himark::higent::AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(),
        crate::test_connector(),
        format!("unix:{}", socket.display()),
    ));

    let snapshot =
        block_on(alice.subscribe_annotations(session.clone())).expect("alice subscribes");
    assert!(snapshot.annotations.is_empty());
    block_on(bob.subscribe_annotations(session.clone())).expect("bob subscribes");

    use himark::higent::ahp_types::actions as wire;
    use himark::higent::ahp_types::state;
    let annotation = state::Annotation {
        id: "e2e-1".to_owned(),
        turn_id: String::new(),
        resource: "file:///tmp/shared.md".to_owned(),
        range: Some(state::TextRange {
            start: state::TextPosition {
                line: 2,
                character: 0,
            },
            end: state::TextPosition {
                line: 2,
                character: 5,
            },
        }),
        resolved: false,
        entries: vec![state::AnnotationEntry {
            id: "e2e-1-e1".to_owned(),
            text: himark::higent::ahp_types::common::StringOrMarkdown::Markdown {
                markdown: "shared thought".to_owned(),
            },
            meta: None,
        }],
        meta: None,
    };
    alice.dispatch_annotations(
        &session,
        wire::StateAction::AnnotationsSet(wire::AnnotationsSetAction {
            annotation: annotation.clone(),
        }),
    );

    let seen = block_on(bob.poll_annotations(session.clone()));
    assert!(
        seen.iter().any(|action| matches!(
            action,
            wire::StateAction::AnnotationsSet(set) if set.annotation.id == "e2e-1"
        )),
        "{seen:?}"
    );
    let echoes = block_on(alice.poll_annotations(session.clone()));
    assert!(echoes
        .iter()
        .any(|action| matches!(action, wire::StateAction::AnnotationsSet(_))));

    let carol: Arc<dyn himark::higent::AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(),
        crate::test_connector(),
        format!("unix:{}", socket.display()),
    ));
    let folded = block_on(carol.subscribe_annotations(session.clone())).expect("carol subscribes");
    assert_eq!(folded.annotations.len(), 1);
    assert_eq!(folded.annotations[0].entries.len(), 1);

    bob.dispatch_annotations(
        &session,
        wire::StateAction::AnnotationsRemoved(wire::AnnotationsRemovedAction {
            annotation_id: "e2e-1".to_owned(),
        }),
    );
    let seen = block_on(alice.poll_annotations(session.clone()));
    assert!(
        seen.iter().any(|action| matches!(
            action,
            wire::StateAction::AnnotationsRemoved(removed) if removed.annotation_id == "e2e-1"
        )),
        "{seen:?}"
    );
}

#[test]
fn two_engines_sync_a_live_document() {
    let _host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("hosted fs");
    std::env::set_var("HIMARK_HOST_HOME", dir.path().join("home"));
    let socket = dir.path().join("backend.sock");
    let serving = socket.clone();
    let config = agent_host::HostConfig {
        agents: Vec::new(),
        data_dir: dir.path().join("data"),
        claude_binary: "false".to_owned(),
        codex_binary: "false".to_owned(),
        claude_home: dir.path().join("dot-claude"),
        codex_home: dir.path().join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("backend runtime");
        let backend = agent_host::Host::new(config);
        let _ = runtime.block_on(backend.bind(&serving));
    });
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 500, "the backend never bound its socket");
    }

    let engine_at = |name: &str| -> (HimarkEngine, u64, std::sync::Arc<fake_host::Seat>) {
        let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
        let window = engine.add_window();
        let seat: Arc<dyn himark::higent::AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
        let local = engine.register_agent_server(name, seat);
        engine.set_local_backend(local);
        let host_seat = fake_host::Seat::new();
        engine.set_host(HimarkHostCallbacks {
            ctx: host_seat.ctx(),
            pick_files: Some(fake_host::pick_files),
            pick_save: Some(fake_host::pick_save),
            fetch_document: Some(fake_host::fetch_document),
            store_document: Some(fake_host::store_document),
            list_directory: Some(fake_host::list_directory),
            subscribe: None,
            unsubscribe: None,
            set_clipboard: None,
        });
        let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        (engine, window, host_seat)
    };
    let (mut alice, alice_window, alice_seat) = engine_at("Backend for Alice");
    let (mut bob, bob_window, bob_seat) = engine_at("Backend for Bob");

    let file_path = dir.path().join("files/shared.md");
    std::fs::create_dir_all(file_path.parent().unwrap()).expect("mkdir");
    std::fs::write(&file_path, "shared story\n").expect("seed");
    let location = himark::ResourceLocation::new(
        himark::ResourceType::document(),
        himark::Authority::new("local"),
        file_path
            .canonicalize()
            .expect("canonical")
            .to_str()
            .unwrap()
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect::<Vec<String>>(),
    );

    for (engine, window, seat) in [
        (&mut alice, alice_window, &alice_seat),
        (&mut bob, bob_window, &bob_seat),
    ] {
        let window = window;
        assert!(engine.perform_command(window, "file.open"));
        settle(engine);
        let request = pick_request(engine, seat);
        assert!(engine.host_picked(request, vec![location.clone()]));
        settle_until(engine, "the shared file opened", |engine| {
            engine
                .substring(
                    window,
                    HimarkRange {
                        start: 0,
                        length: u32::MAX,
                    },
                )
                .is_some_and(|text| text.contains("shared story"))
        });
    }

    assert!(himark::test_driver::type_text(
        &mut alice.app,
        "Alice was here. "
    ));
    let mut waited = 0;
    loop {
        settle(&mut alice);
        settle(&mut bob);
        let shown = bob
            .substring(
                bob_window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .unwrap_or_default();
        if shown.contains("Alice was here.") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 500, "bob never saw alice's edit: {shown:?}");
    }

    assert!(himark::test_driver::type_text(&mut bob.app, "Bob too. "));
    let mut waited = 0;
    loop {
        settle(&mut alice);
        settle(&mut bob);
        let alice_shown = alice
            .substring(
                alice_window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .unwrap_or_default();
        let bob_shown = bob
            .substring(
                bob_window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .unwrap_or_default();
        if alice_shown.contains("Bob too.") && alice_shown == bob_shown {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(
            waited < 500,
            "the buffers never converged: alice {alice_shown:?} bob {bob_shown:?}"
        );
    }
}

#[test]
fn a_late_joiner_adopts_a_document_edited_before_it_opened() {
    let _host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("hosted fs");
    std::env::set_var("HIMARK_HOST_HOME", dir.path().join("home"));
    let socket = dir.path().join("backend.sock");
    let serving = socket.clone();
    let config = agent_host::HostConfig {
        agents: Vec::new(),
        data_dir: dir.path().join("data"),
        claude_binary: "false".to_owned(),
        codex_binary: "false".to_owned(),
        claude_home: dir.path().join("dot-claude"),
        codex_home: dir.path().join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("backend runtime");
        let backend = agent_host::Host::new(config);
        let _ = runtime.block_on(backend.bind(&serving));
    });
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 500, "the backend never bound its socket");
    }

    let engine_at = |name: &str| -> (HimarkEngine, u64, std::sync::Arc<fake_host::Seat>) {
        let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
        let window = engine.add_window();
        let seat: Arc<dyn himark::higent::AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
        let local = engine.register_agent_server(name, seat);
        engine.set_local_backend(local);
        let host_seat = fake_host::Seat::new();
        engine.set_host(HimarkHostCallbacks {
            ctx: host_seat.ctx(),
            pick_files: Some(fake_host::pick_files),
            pick_save: Some(fake_host::pick_save),
            fetch_document: Some(fake_host::fetch_document),
            store_document: Some(fake_host::store_document),
            list_directory: Some(fake_host::list_directory),
            subscribe: None,
            unsubscribe: None,
            set_clipboard: None,
        });
        let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        (engine, window, host_seat)
    };
    let (mut alice, alice_window, alice_seat) = engine_at("Backend for Alice");
    let (mut bob, bob_window, bob_seat) = engine_at("Backend for Bob");

    let file_path = dir.path().join("files/shared.md");
    std::fs::create_dir_all(file_path.parent().unwrap()).expect("mkdir");
    std::fs::write(&file_path, "shared story\n").expect("seed");
    let location = himark::ResourceLocation::new(
        himark::ResourceType::document(),
        himark::Authority::new("local"),
        file_path
            .canonicalize()
            .expect("canonical")
            .to_str()
            .unwrap()
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect::<Vec<String>>(),
    );

    for (engine, window, seat) in [(&mut alice, alice_window, &alice_seat)] {
        let window = window;
        assert!(engine.perform_command(window, "file.open"));
        settle(engine);
        let request = pick_request(engine, seat);
        assert!(engine.host_picked(request, vec![location.clone()]));
        settle_until(engine, "the shared file opened", |engine| {
            engine
                .substring(
                    window,
                    HimarkRange {
                        start: 0,
                        length: u32::MAX,
                    },
                )
                .is_some_and(|text| text.contains("shared story"))
        });
    }

    assert!(himark::test_driver::type_text(
        &mut alice.app,
        "Alice was here. "
    ));
    for _ in 0..20 {
        settle(&mut alice);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for (engine, window, seat) in [(&mut bob, bob_window, &bob_seat)] {
        let window = window;
        assert!(engine.perform_command(window, "file.open"));
        settle(engine);
        let request = pick_request(engine, seat);
        assert!(engine.host_picked(request, vec![location.clone()]));
        settle_until(engine, "the shared file opened", |engine| {
            engine
                .substring(
                    window,
                    HimarkRange {
                        start: 0,
                        length: u32::MAX,
                    },
                )
                .is_some_and(|text| text.contains("shared story"))
        });
    }

    let mut waited = 0;
    loop {
        settle(&mut alice);
        settle(&mut bob);
        let shown = bob
            .substring(
                bob_window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .unwrap_or_default();
        if shown.contains("Alice was here.") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(
            waited < 500,
            "a late joiner never adopted the live edit: {shown:?}"
        );
    }

    assert!(himark::test_driver::type_text(&mut bob.app, "Bob too. "));
    let mut waited = 0;
    loop {
        settle(&mut alice);
        settle(&mut bob);
        let alice_shown = alice
            .substring(
                alice_window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .unwrap_or_default();
        let bob_shown = bob
            .substring(
                bob_window,
                HimarkRange {
                    start: 0,
                    length: u32::MAX,
                },
            )
            .unwrap_or_default();
        if alice_shown.contains("Bob too.") && alice_shown == bob_shown {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(
            waited < 500,
            "the buffers never converged: alice {alice_shown:?} bob {bob_shown:?}"
        );
    }
}

#[test]
fn the_new_session_composer_starts_the_session_with_the_prompt() {
    let _host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("hosted fs");
    std::env::set_var("HIMARK_HOST_HOME", dir.path().join("home"));
    let socket = dir.path().join("backend.sock");
    let serving = socket.clone();

    let config = agent_host::HostConfig {
        data_dir: dir.path().join("data"),
        claude_binary: agent_host::testing::fake_cli_command(dir.path()),
        codex_binary: agent_host::testing::fake_codex_command(dir.path()),
        claude_home: dir.path().join("dot-claude"),
        codex_home: dir.path().join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
        ..agent_host::HostConfig::default()
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("backend runtime");
        let backend = agent_host::Host::new(config);
        let _ = runtime.block_on(backend.bind(&serving));
    });
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 500, "the backend never bound its socket");
    }

    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());

    engine.compose_new_windows = true;
    let window = engine.add_window();
    let seat: std::sync::Arc<dyn himark::higent::AhpServer> =
        std::sync::Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
    let local = engine.register_agent_server("Local Backend", seat);
    engine.set_local_backend(local);
    let host_seat = fake_host::Seat::new();
    engine.set_host(HimarkHostCallbacks {
        ctx: host_seat.ctx(),
        pick_files: Some(fake_host::pick_files),
        pick_save: Some(fake_host::pick_save),
        fetch_document: Some(fake_host::fetch_document),
        store_document: Some(fake_host::store_document),
        list_directory: Some(fake_host::list_directory),
        subscribe: None,
        unsubscribe: None,
        set_clipboard: None,
    });
    let root = dir.path().join("files");
    std::fs::create_dir_all(&root).expect("files root");
    let fs = HostedFs { root, _dir: dir };

    let try_probe = |engine: &HimarkEngine| -> Option<himark::new_session::NewSessionProbe> {
        let mut probe = None;
        engine.app.for_each_plugin_panel(&mut |panel| {
            if let Some(pane) = panel
                .as_any()
                .downcast_ref::<himark::new_session::ComposerPane>()
            {
                probe =
                    himark::new_session::Composers::composer_ref(engine.app.store(), pane.window())
                        .map(|composer| composer.probe());
            }
        });
        probe
    };
    let probe = |engine: &HimarkEngine| try_probe(engine).expect("the composer panel");

    let mut surface = skia_safe::surfaces::raster_n32_premul((1200, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
    settle_until(
        engine_mut(&mut engine),
        "the composer combos populated",
        |engine| {
            let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
            let probe = probe(engine);
            !probe.model.labels.is_empty() && !probe.edits.labels.is_empty()
        },
    );
    let booted = probe(&engine);
    assert_eq!(
        booted.model.labels,
        vec![
            "Claude",
            "Fable",
            "Opus",
            "Sonnet",
            "Haiku",
            "Codex",
            "GPT-6 Astra",
            "GPT-5.6 Sol",
            "GPT-5.6 Terra",
            "GPT-5.6 Luna",
        ],
        "one MODEL menu groups models under provider headings"
    );
    assert_eq!(booted.model.picked.as_deref(), Some("Fable"));
    assert_eq!(booted.effort.picked.as_deref(), Some("High"));
    assert_eq!(booted.edits.picked.as_deref(), Some("Ask first"));
    assert_eq!(booted.mode.picked.as_deref(), Some("Agent"));
    assert_eq!(
        booted.dir.labels,
        vec!["No folder".to_owned(), "Choose folder…".to_owned()],
        "a fresh host offers the folderless start and the picker row"
    );
    assert!(!booted.ready, "no prompt yet — Start stays disarmed");

    let model_cell = booted.cells[3];
    assert!(himark::test_driver::click(
        &mut engine.app,
        model_cell.0 + model_cell.1 * 0.5,
        800.0 - 37.0,
        1200.0,
        800.0,
    ));
    let opened = probe(&engine);
    assert!(opened.model.open, "the grouped MODEL menu stands");
    for _ in 0..5 {
        assert!(himark::test_driver::key(
            &mut engine.app,
            imba::event::Key::Down,
            imba::event::Modifiers::default()
        ));
    }
    assert!(himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers::default()
    ));
    let codex = probe(&engine);
    assert_eq!(codex.model.picked.as_deref(), Some("GPT-5.6 Sol"));
    assert_eq!(codex.effort.picked.as_deref(), Some("Medium"));

    settle_until(
        engine_mut(&mut engine),
        "the Codex placeholder session stood",
        |engine| {
            let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
            himark::new_session::Placeholders::session_of(
                engine.app.store(),
                engine.app.sole_window(),
            )
            .is_some_and(|(_, provider, _)| provider == "codex")
        },
    );
    let placeholder = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
        .expect("window")
        .current_session();

    assert!(himark::test_driver::type_text(&mut engine.app, "Build me"));
    assert!(himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers::default()
    ));
    assert!(himark::test_driver::type_text(&mut engine.app, "a parser"));
    assert_eq!(probe(&engine).prompt, "Build me\na parser");

    let row_mid = 800.0 - booted.cells[1].1.mul_add(0.0, 37.0);
    let dir_cell = booted.cells[1];
    assert!(himark::test_driver::click(
        &mut engine.app,
        dir_cell.0 + dir_cell.1 * 0.5,
        800.0 - 37.0,
        1200.0,
        800.0,
    ));
    assert!(probe(&engine).dir.open, "the DIR menu stands");

    assert!(himark::test_driver::click(
        &mut engine.app,
        dir_cell.0 + 30.0,
        800.0 - 74.0 - 20.0,
        1200.0,
        800.0,
    ));
    assert!(!probe(&engine).dir.open, "the pick dismissed the menu");
    settle(&mut engine);
    let request = pick_request(&mut engine, &host_seat);
    assert!(engine.host_picked(request, vec![fs.dir(&[])]));
    settle(&mut engine);
    let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
    settle_until(
        engine_mut(&mut engine),
        "the picked folder landed",
        |engine| {
            let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
            probe(engine).dir.picked.as_deref() == Some("files")
        },
    );

    settle_until(
        engine_mut(&mut engine),
        "the placeholder gained the folder",
        |engine| {
            let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
            himark::higent::Agents::channel(engine.app.store(), &placeholder)
                .is_some_and(|channel| !channel.working_directories.is_empty())
        },
    );

    let cells = probe(&engine).cells.clone();
    assert!(himark::test_driver::click(
        &mut engine.app,
        cells[3].0 + cells[3].1 * 0.5,
        800.0 - 37.0,
        1200.0,
        800.0,
    ));
    assert!(probe(&engine).model.open);
    assert!(himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Down,
        imba::event::Modifiers::default()
    ));
    assert!(himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers::default()
    ));
    let after = probe(&engine);
    assert!(!after.model.open, "Enter picked and dismissed");
    assert_eq!(after.model.picked.as_deref(), Some("GPT-5.6 Terra"));
    assert!(after.ready, "prompt + folder + connected host arm Start");
    let _ = row_mid;

    // Move effort and edits off their defaults (Medium and Ask first), so
    // the reopened composer provably restores the session's values rather
    // than landing on the defaults again. The toolbar overflows the 1200px
    // window by now, so those two cells need a wider viewport to reach.
    let mut wide = skia_safe::surfaces::raster_n32_premul((2000, 800)).expect("surface");
    let _ = engine.draw(window, wide.canvas(), 2000.0, 800.0, 1.0);
    let cells = probe(&engine).cells.clone();
    assert!(himark::test_driver::click(
        &mut engine.app,
        cells[4].0 + cells[4].1 * 0.5,
        800.0 - 37.0,
        2000.0,
        800.0,
    ));
    assert!(probe(&engine).effort.open, "the EFFORT menu stands");
    assert!(himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Down,
        imba::event::Modifiers::default()
    ));
    assert!(himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers::default()
    ));
    assert_eq!(probe(&engine).effort.picked.as_deref(), Some("High"));
    let _ = engine.draw(window, wide.canvas(), 2000.0, 800.0, 1.0);
    let cells = probe(&engine).cells.clone();
    assert!(himark::test_driver::click(
        &mut engine.app,
        cells[5].0 + 20.0,
        800.0 - 37.0,
        2000.0,
        800.0,
    ));
    assert!(probe(&engine).edits.open, "the EDITS menu stands");
    assert!(himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Down,
        imba::event::Modifiers::default()
    ));
    assert!(himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers::default()
    ));
    assert_eq!(probe(&engine).edits.picked.as_deref(), Some("Accept edits"));
    let _ = engine.draw(window, wide.canvas(), 2000.0, 800.0, 1.0);
    let cells = probe(&engine).cells.clone();
    assert!(himark::test_driver::click(
        &mut engine.app,
        cells[6].0 + 20.0,
        800.0 - 37.0,
        2000.0,
        800.0,
    ));
    assert!(probe(&engine).worktree, "the worktree checkbox toggled on");

    assert!(himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers {
            command: true,
            ..Default::default()
        }
    ));
    settle_until(engine_mut(&mut engine), "the session opened", |engine| {
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .is_some_and(|entity| entity.current_session().names_session())
    });
    let session = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
        .expect("window")
        .current_session();

    assert_eq!(
        session, placeholder,
        "start continued the placeholder session"
    );
    let channel = himark::higent::Agents::channel(engine.app.store(), &session)
        .expect("the session channel mirror");
    assert_eq!(
        channel.provider, "codex",
        "the selected agent created the session"
    );
    let chat = channel.default_chat.clone().expect("the default chat");

    let _ = chat;
    settle_until(
        engine_mut(&mut engine),
        "the prompt landed in the transcript",
        |engine| {
            let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
            chat_transcript(engine).is_some_and(|rows| {
                rows.iter().any(|(_, cells)| {
                    cells
                        .iter()
                        .any(|(_, text)| text.contains("Build me\na parser"))
                })
            })
        },
    );
    assert_eq!(
        shown_chat(&engine)
            .expect("chat panel")
            .toolbar_probe()
            .model
            .labels,
        vec![
            "GPT-6 Astra",
            "GPT-5.6 Sol",
            "GPT-5.6 Terra",
            "GPT-5.6 Luna"
        ],
        "the live Codex toolbar stays provider-filtered"
    );

    // Reopening the composer over the live session carries its folder,
    // agent, model, effort, and edits mode into the fresh form.
    let app_window = engine.app.sole_window();
    assert!(engine.app.perform_registered(app_window, "session.new"));
    settle_until(
        engine_mut(&mut engine),
        "the reopened composer prefilled from the current session",
        |engine| {
            let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
            try_probe(engine).is_some_and(|probe| {
                probe.dir.picked.as_deref() == Some("files")
                    && probe.model.picked.as_deref() == Some("GPT-5.6 Terra")
                    && probe.effort.picked.as_deref() == Some("High")
                    && probe.edits.picked.as_deref() == Some("Accept edits")
                    && probe.worktree
            })
        },
    );
    let reopened = probe(&engine);
    assert_eq!(
        reopened.dir.picked.as_deref(),
        Some("files"),
        "the session's folder carried over"
    );
    assert_eq!(
        reopened.model.picked.as_deref(),
        Some("GPT-5.6 Terra"),
        "the session's agent and model carried over"
    );
    assert_eq!(
        reopened.effort.picked.as_deref(),
        Some("High"),
        "the session's effort carried over"
    );
    assert_eq!(
        reopened.edits.picked.as_deref(),
        Some("Accept edits"),
        "the session's edits mode carried over"
    );
    assert!(
        reopened.worktree,
        "the session's worktree flag carried over"
    );
    assert_eq!(
        reopened.prompt, "",
        "the prompt starts empty — only the setup carries over"
    );
}

fn engine_mut(engine: &mut HimarkEngine) -> &mut HimarkEngine {
    engine
}

#[test]
fn a_dirless_session_gains_a_folder_and_switches_edits() {
    let _host = HOSTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("hosted fs");
    std::env::set_var("HIMARK_HOST_HOME", dir.path().join("home"));
    let socket = dir.path().join("backend.sock");
    let serving = socket.clone();
    let data_dir = dir.path().join("data");
    let config = agent_host::HostConfig {
        data_dir: data_dir.clone(),
        claude_binary: agent_host::testing::fake_cli_command(dir.path()),
        claude_home: dir.path().join("dot-claude"),
        codex_home: dir.path().join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
        ..agent_host::HostConfig::default()
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("backend runtime");
        let backend = agent_host::Host::new(config);
        let _ = runtime.block_on(backend.bind(&serving));
    });
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 500, "the backend never bound its socket");
    }

    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());

    himark::FloatingChat::set(&mut engine.app.store_mut(), false);
    engine.compose_new_windows = true;
    let window = engine.add_window();
    let seat: std::sync::Arc<dyn himark::higent::AhpServer> =
        std::sync::Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
    let local = engine.register_agent_server("Local Backend", seat);
    engine.set_local_backend(local);
    let host_seat = fake_host::Seat::new();
    engine.set_host(HimarkHostCallbacks {
        ctx: host_seat.ctx(),
        pick_files: Some(fake_host::pick_files),
        pick_save: Some(fake_host::pick_save),
        fetch_document: Some(fake_host::fetch_document),
        store_document: Some(fake_host::store_document),
        list_directory: Some(fake_host::list_directory),
        subscribe: None,
        unsubscribe: None,
        set_clipboard: None,
    });
    let root = dir.path().join("files");
    std::fs::create_dir_all(&root).expect("files root");
    let fs = HostedFs { root, _dir: dir };

    let probe = |engine: &HimarkEngine| -> himark::new_session::NewSessionProbe {
        let mut probe = None;
        engine.app.for_each_plugin_panel(&mut |panel| {
            if let Some(pane) = panel
                .as_any()
                .downcast_ref::<himark::new_session::ComposerPane>()
            {
                probe =
                    himark::new_session::Composers::composer_ref(engine.app.store(), pane.window())
                        .map(|composer| composer.probe());
            }
        });
        probe.expect("the composer panel")
    };

    let mut surface = skia_safe::surfaces::raster_n32_premul((1200, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
    settle_until(
        engine_mut(&mut engine),
        "the composer combos populated",
        |engine| {
            let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
            !probe(engine).model.labels.is_empty()
        },
    );
    assert_eq!(probe(&engine).dir.picked.as_deref(), Some("No folder"));
    assert!(himark::test_driver::type_text(&mut engine.app, "hello"));
    assert!(
        probe(&engine).ready,
        "prompt + host arm Start — no folder needed"
    );

    settle(&mut engine);
    assert!(himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers {
            command: true,
            ..Default::default()
        }
    ));

    settle_until(engine_mut(&mut engine), "the session opened", |engine| {
        let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .map(|entity| entity.current_session())
            .filter(|session| session.names_session())
            .is_some_and(|session| {
                himark::higent::Agents::channel(engine.app.store(), &session).is_some()
            })
    });
    let session = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
        .expect("window")
        .current_session();
    assert!(
        himark::higent::Agents::channel(engine.app.store(), &session)
            .expect("the session channel mirror")
            .working_directories
            .is_empty(),
        "born without a directory"
    );

    let toolbar = |engine: &HimarkEngine| -> himark::higent::ToolbarProbe {
        shown_chat(engine).expect("the chat panel").toolbar_probe()
    };
    settle_until(engine_mut(&mut engine), "the toolbar seeded", |engine| {
        let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
        shown_chat(engine).is_some_and(|chat| {
            let toolbar = chat.toolbar_probe();
            toolbar.model.picked.is_some() && toolbar.edits.picked.is_some()
        })
    });

    settle(&mut engine);
    let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
    settle(&mut engine);
    let seeded = toolbar(&engine);
    assert_eq!(seeded.model.picked.as_deref(), Some("Fable"));
    assert_eq!(seeded.edits.picked.as_deref(), Some("Ask first"));

    let edits_cell = seeded.cells[2];

    let window_chrome = himark::env::Themes::of(engine.app.store())
        .ui()
        .toolbar
        .height;
    let (strip_x, strip_y) = (seeded.origin.0, seeded.origin.1 + window_chrome);
    assert!(himark::test_driver::click(
        &mut engine.app,
        strip_x + edits_cell.0 + edits_cell.1 * 0.5,
        strip_y + 20.0,
        1200.0,
        800.0,
    ));
    assert!(toolbar(&engine).edits.open, "the EDITS menu stands");

    let row_h = himark::env::Themes::of(engine.app.store())
        .ui()
        .combo
        .menu_row_height;
    assert!(himark::test_driver::click(
        &mut engine.app,
        strip_x + edits_cell.0 + 20.0,
        strip_y - row_h * 2.5 - 1.0,
        1200.0,
        800.0,
    ));
    assert!(!toolbar(&engine).edits.open, "the pick dismissed the menu");
    assert_eq!(
        toolbar(&engine).edits.picked.as_deref(),
        Some("Accept edits")
    );

    // More than one session dir can exist (the host's local-fs
    // session persists a manifest too): scan them ALL — polling
    // whichever `read_dir` yields first is a coin flip.
    let manifests = || -> Vec<String> {
        std::fs::read_dir(data_dir.join("sessions"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_to_string(entry.path().join("session.json")).ok())
            .collect()
    };
    settle_until(
        engine_mut(&mut engine),
        "the permission persisted",
        |engine| {
            let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
            manifests()
                .iter()
                .any(|manifest| manifest.contains("\"permissionMode\": \"acceptEdits\""))
        },
    );

    let add_cell = toolbar(&engine).cells[3];
    assert!(himark::test_driver::click(
        &mut engine.app,
        strip_x + add_cell.0 + add_cell.1 * 0.5,
        strip_y + 20.0,
        1200.0,
        800.0,
    ));
    settle(&mut engine);
    let request = pick_request(&mut engine, &host_seat);
    assert!(engine.host_picked(request, vec![fs.dir(&[])]));
    settle_until(
        engine_mut(&mut engine),
        "the granted folder landed",
        |engine| {
            let _ = engine.draw(window, surface.canvas(), 1200.0, 800.0, 1.0);
            himark::higent::Agents::channel(engine.app.store(), &session).is_some_and(|channel| {
                channel
                    .working_directories
                    .iter()
                    .any(|held| held.contains("files"))
            })
        },
    );
    assert!(
        manifests()
            .iter()
            .any(|manifest| manifest.contains("files")),
        "the grant persisted: {:?}",
        manifests()
    );
}

#[test]
fn an_existing_session_row_pick_switches_and_remounts_the_chat() {
    std::env::set_var("HIMARK_AGENT_LATENCY_MS", "0");
    std::env::set_var("HIMARK_AHP_URL", "ws://127.0.0.1:9/unreachable");
    let dir = tempfile::tempdir().expect("tempdir");
    let stub = agent_host::testing::fake_cli_command(dir.path());
    let host = spawn_host(dir.path(), &stub);
    let socket = host.socket.clone();

    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    himark::FloatingChat::set(&mut engine.app.store_mut(), true);
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);

    let seat: std::sync::Arc<dyn himark::higent::AhpServer> =
        std::sync::Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
    let _ours = engine.register_agent_server("himark Host", seat);
    let workdir = dir.path().to_owned();
    himark::higent::Agents::install_new_session(
        &mut engine.app.store_mut(),
        std::sync::Arc::new(move |server| {
            std::sync::Arc::new(StubNewSession {
                server,
                directory: format!("file://{}", workdir.display()),
            })
        }),
    );
    let pump = |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface| {
        for _ in 0..6 {
            let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
            settle(engine);
        }
    };
    let floating = |engine: &HimarkEngine| -> Option<himark::higent::ChatPanel> {
        let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())?;
        let pane = entity.bottom_pane()?;
        let pane = pane.as_any().downcast_ref::<himark::higent::ChatPane>()?;
        himark::higent::Chats::chat(engine.app.store(), pane.chat())
    };
    let current = |engine: &HimarkEngine| -> himark::SessionId {
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("window")
            .current_session()
    };

    assert!(engine.perform_command(window, "agent.toggle-agents"));
    let mut plus = None;
    for _ in 0..30 {
        pump(&mut engine, &mut surface);
        let rows = drawer_rows(&engine).expect("the agents drawer is up");
        let settled = !rows.iter().any(|(label, _)| label.contains("connecting"));
        plus = rows
            .iter()
            .position(|(label, depth)| *depth == 0 && label == "himark Host")
            .and_then(|index| {
                (rows.get(index + 1) == Some(&("+ New Session…".to_owned(), 1)))
                    .then_some(index + 1)
            })
            .filter(|_| settled);
        if plus.is_some() {
            break;
        }
    }
    pick_drawer_row(&mut engine, plus.expect("the himark host row connected"));
    let mut waited = 0;
    while floating(&engine).is_none_or(|chat| !chat.ready()) && waited < 50 {
        pump(&mut engine, &mut surface);
        waited += 1;
    }
    assert!(
        floating(&engine).is_some_and(|chat| chat.ready()),
        "the created session's chat mounted the sheet"
    );
    let created = current(&engine);

    struct SwitchScratch;
    impl himark::DynamicCommand for SwitchScratch {
        fn id(&self) -> &'static str {
            "test.switch-scratch"
        }
        fn name(&self) -> String {
            "Switch Scratch".to_owned()
        }
        fn perform(
            &self,
            _app: &mut himark::Application,
            store: &mut imba::store::Store,
            window: himark::WindowId,
            fx: &mut himark::AppFx<'_>,
        ) {
            let target = himark::SessionId::mint_scratch(store);
            himark::switch_session(store, window, target, fx);
        }
    }
    assert!(engine.app.perform_command(himark::AppCommand::Dynamic(
        engine.app.sole_window(),
        std::sync::Arc::new(SwitchScratch),
    )));
    pump(&mut engine, &mut surface);
    assert_ne!(current(&engine), created, "the scratch took the window");

    assert!(engine.perform_command(window, "agent.toggle-agents"));
    let mut session_row = None;
    for _ in 0..30 {
        pump(&mut engine, &mut surface);
        let rows = drawer_rows(&engine).expect("the agents drawer is up");
        let host_at = rows
            .iter()
            .position(|(label, depth)| *depth == 0 && label == "himark Host");
        // Sessions with a folder list at depth 2 under the folder row;
        // only folderless strays stay at depth 1.
        session_row = host_at.and_then(|index| {
            let under_host = rows.iter().enumerate().skip(index + 1);
            under_host
                .filter(|(_, (label, depth))| {
                    (*depth == 1 || *depth == 2)
                        && label != "+ New Session…"
                        && !label.contains("connecting")
                })
                .max_by_key(|(_, (_, depth))| *depth)
                .map(|(at, _)| at)
        });
        if session_row.is_some() {
            break;
        }
    }
    pick_drawer_row(
        &mut engine,
        session_row.expect("the existing session lists in the drawer"),
    );
    let mut waited = 0;
    while (current(&engine) != created || floating(&engine).is_none()) && waited < 50 {
        pump(&mut engine, &mut surface);
        waited += 1;
    }
    assert_eq!(current(&engine), created, "the pick switched the window");
    assert!(
        floating(&engine).is_some(),
        "the existing session's chat remounted the sheet"
    );
}

#[test]
fn the_sheet_centers_on_the_window_over_the_dock() {
    std::env::set_var("HIMARK_AGENT_LATENCY_MS", "0");
    std::env::set_var("HIMARK_AHP_URL", "ws://127.0.0.1:9/unreachable");
    let dir = tempfile::tempdir().expect("tempdir");
    let stub = agent_host::testing::fake_cli_command(dir.path());
    let host = spawn_host(dir.path(), &stub);
    let socket = host.socket.clone();

    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    himark::FloatingChat::set(&mut engine.app.store_mut(), true);
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);

    let seat: std::sync::Arc<dyn himark::higent::AhpServer> =
        std::sync::Arc::new(crate::hiahp::wire::WireHost::at(
            crate::hiahp::wire::test_runtime(),
            crate::test_connector(),
            format!("unix:{}", socket.display()),
        ));
    let _ours = engine.register_agent_server("himark Host", seat);

    let host_seat = fake_host::Seat::new();
    engine.set_host(HimarkHostCallbacks {
        ctx: host_seat.ctx(),
        pick_files: Some(fake_host::pick_files),
        pick_save: Some(fake_host::pick_save),
        fetch_document: Some(fake_host::fetch_document),
        store_document: Some(fake_host::store_document),
        list_directory: Some(fake_host::list_directory),
        subscribe: None,
        unsubscribe: None,
        set_clipboard: None,
    });
    let workdir = dir.path().to_owned();
    himark::higent::Agents::install_new_session(
        &mut engine.app.store_mut(),
        std::sync::Arc::new(move |server| {
            std::sync::Arc::new(StubNewSession {
                server,
                directory: format!("file://{}", workdir.display()),
            })
        }),
    );
    let pump = |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface| {
        for _ in 0..6 {
            let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
            settle(engine);
        }
    };

    assert!(engine.perform_command(window, "agent.toggle-agents"));
    let mut plus = None;
    for _ in 0..30 {
        pump(&mut engine, &mut surface);
        let rows = drawer_rows(&engine).expect("the agents drawer is up");
        let settled = !rows.iter().any(|(label, _)| label.contains("connecting"));
        plus = rows
            .iter()
            .position(|(label, depth)| *depth == 0 && label == "himark Host")
            .and_then(|index| {
                (rows.get(index + 1) == Some(&("+ New Session…".to_owned(), 1)))
                    .then_some(index + 1)
            })
            .filter(|_| settled);
        if plus.is_some() {
            break;
        }
    }
    pick_drawer_row(&mut engine, plus.expect("the himark host row connected"));
    let mounted = |engine: &HimarkEngine| {
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .and_then(|entity| entity.bottom_pane().map(|_| ()))
            .is_some()
    };
    let mut waited = 0;
    while !mounted(&engine) && waited < 50 {
        pump(&mut engine, &mut surface);
        waited += 1;
    }
    assert!(mounted(&engine), "the sheet mounted");

    assert!(engine.perform_command(window, "changes.view"));
    for ms in [0.0, 500.0, 1_000.0, 2_000.0] {
        let _ = himark::test_driver::animate(
            &mut engine.app,
            imba::anim::AnimationClock::from_millis(ms),
        );
        pump(&mut engine, &mut surface);
    }

    let entity = || {
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .expect("the window entity")
    };
    let band = entity()
        .bottom_rect(engine.app.store(), skia_safe::Size::new(900.0, 700.0))
        .expect("the sheet's rectangle");
    // The sheet centers on the WHOLE window — the dock does not push
    // it aside; the sheet floats over the dock's band instead.
    assert!(
        (band.center_x() - 450.0).abs() <= 0.5,
        "the sheet drifted off the window's center: band {band:?}"
    );

    // The dock still takes presses above the sheet's band.
    let _ = himark::test_driver::click(&mut engine.app, dock_x(90.0), 200.0, 900.0, 700.0);
    pump(&mut engine, &mut surface);
    assert_eq!(
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .map(|entity| entity.layer_focus()),
        Some(himark::LayerFocus::Dock),
        "the graph press focused the dock"
    );
}

#[test]
fn the_graph_section_expands_commits_and_commits_from_the_box() {
    let (_host, mut engine, window, _fs) = hosted_engine();

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonical root");
    let sh = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    sh(&["init", "-q", "-b", "main"]);
    sh(&["config", "user.email", "test@example.com"]);
    sh(&["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "one\n").unwrap();
    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "first commit"]);
    std::fs::write(root.join("README.md"), "one\ntwo\n").unwrap();
    sh(&["commit", "-q", "-am", "second commit"]);
    std::fs::write(root.join("README.md"), "one\ntwo\nworking\n").unwrap();
    let root_segments: Vec<String> = root
        .to_str()
        .unwrap()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    let folder = himark::ResourceLocation::new(
        himark::ResourceType::directory(),
        himark::Authority::new("local"),
        root_segments,
    );
    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);
    assert!(engine.host_picked(request, vec![folder]));
    settle_into_session(&mut engine);

    assert!(engine.perform_command(window, "history.view"));
    settle(&mut engine);
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    for ms in [0.0, 1_000.0] {
        let _ = himark::test_driver::animate(
            &mut engine.app,
            imba::anim::AnimationClock::from_millis(ms),
        );
    }
    let rows = |engine: &HimarkEngine| {
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window()).and_then(
            |entity| {
                entity
                    .dock_panel()
                    .and_then(|side| {
                        side.as_any()
                            .downcast_ref::<himark::hihistory::HistoryView>()
                    })
                    .map(|view| view.rows())
            },
        )
    };
    let painted_rows = |engine: &mut HimarkEngine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        rows(engine)
    };

    settle_until(&mut engine, "the graph landed", |engine| {
        painted_rows(engine).is_some_and(|rows| {
            rows.iter()
                .any(|(_, label, _)| label.starts_with("second commit"))
        })
    });
    let listed = rows(&engine).expect("the dock is up");

    assert_eq!(listed[0].0, 0, "the root sits at depth 0: {listed:?}");
    assert!(
        listed[0].1.contains("main"),
        "the root carries the branch: {listed:?}"
    );
    let second_at = listed
        .iter()
        .position(|(_, label, _)| label.starts_with("second commit"))
        .expect("the newest commit row");
    assert_eq!(second_at, 1, "newest first: {listed:?}");

    assert!(himark::test_driver::click(
        &mut engine.app,
        dock_x(90.0),
        history_row_y(second_at),
        900.0,
        700.0
    ));
    settle_until(&mut engine, "the commit's files landed", |engine| {
        painted_rows(engine).is_some_and(|rows| {
            rows.iter()
                .any(|(depth, label, pick)| *depth == 2 && label.starts_with("README.md") && *pick)
        })
    });
    let listed = rows(&engine).expect("the dock is up");
    let file_at = listed
        .iter()
        .position(|(depth, label, _)| *depth == 2 && label.starts_with("README.md"))
        .expect("the commit's file row");

    assert!(himark::test_driver::click(
        &mut engine.app,
        dock_x(90.0),
        history_row_y(file_at),
        900.0,
        700.0
    ));
    settle_until(&mut engine, "the commit diff mounted", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        let mut built = false;
        engine.app.for_each_plugin_panel(&mut |panel| {
            if let Some(canvas) = panel.as_any().downcast_ref::<hidiff::DiffCanvasView>() {
                built = matches!(
                    canvas.source(),
                    himark::diff_canvas::CanvasSource::Commit { .. }
                ) && canvas.probe_rows(engine.app.store()).iter().any(
                    |(title, phase, _)| {
                        title == "README.md" && *phase == hidiff::canvas::RowPhase::Built
                    },
                );
            }
        });
        built
    });

    // The commit canvas heads with the commit's message and author.
    {
        let mut banner = None;
        engine.app.for_each_plugin_panel(&mut |panel| {
            if let Some(canvas) = panel.as_any().downcast_ref::<hidiff::DiffCanvasView>() {
                if matches!(
                    canvas.source(),
                    himark::diff_canvas::CanvasSource::Commit { .. }
                ) {
                    banner = canvas.probe_banner(engine.app.store());
                }
            }
        });
        let (message, author) = banner.expect("the commit banner heads the canvas");
        assert!(
            message.starts_with("second commit"),
            "the banner carries the message: {message:?}"
        );
        assert!(
            author.contains("Test") && author.contains("test@example.com"),
            "…and the author: {author:?}"
        );
    }

    // The banner's message box is a REAL editor: a click focuses it
    // and the keyboard lands in the document.
    {
        let commit_banner = |engine: &HimarkEngine| {
            let mut shot = None;
            engine.app.for_each_plugin_panel(&mut |panel| {
                if let Some(canvas) = panel.as_any().downcast_ref::<hidiff::DiffCanvasView>() {
                    if matches!(
                        canvas.source(),
                        himark::diff_canvas::CanvasSource::Commit { .. }
                    ) {
                        shot = canvas.probe_banner(engine.app.store());
                    }
                }
            });
            shot
        };
        let chrome_top = himark::env::Themes::of(engine.app.store())
            .ui()
            .toolbar
            .height;
        assert!(himark::test_driver::click(
            &mut engine.app,
            60.0,
            chrome_top + 14.0,
            900.0,
            700.0
        ));
        assert!(himark::test_driver::type_text(&mut engine.app, "amended "));
        let (message, _) = commit_banner(&engine).expect("the commit banner");
        assert!(
            message.contains("amended"),
            "typing lands in the banner box: {message:?}"
        );
    }

    let history_cursor = |engine: &HimarkEngine| {
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window()).and_then(
            |entity| {
                entity
                    .dock_panel()
                    .and_then(|side| {
                        side.as_any()
                            .downcast_ref::<himark::hihistory::HistoryView>()
                    })
                    .and_then(|view| view.cursor_name())
            },
        )
    };
    let listed = rows(&engine).expect("the dock is up");
    let second_at = listed
        .iter()
        .position(|(_, label, _)| label.starts_with("second commit"))
        .expect("the commit row stands");
    assert!(himark::test_driver::click(
        &mut engine.app,
        dock_x(90.0),
        history_row_y(second_at),
        900.0,
        700.0
    ));
    let before_step = history_cursor(&engine);
    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Down,
        imba::event::Modifiers::default(),
    );
    let after_step = history_cursor(&engine);
    assert!(after_step.is_some(), "the graph tree holds a cursor");
    assert_ne!(
        before_step, after_step,
        "Down stepped the graph's cursor — the section held the keys"
    );

    assert!(engine.perform_command(window, "changes.view"));
    for ms in [3_000.0, 4_000.0] {
        let _ = himark::test_driver::animate(
            &mut engine.app,
            imba::anim::AnimationClock::from_millis(ms),
        );
        settle(&mut engine);
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    }
    // The commit composer lives in the working-copy CANVAS's first
    // row now: pick the changed file, the canvas opens, the box heads
    // the list.
    settle_until(&mut engine, "the changed row listed", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .and_then(|entity| {
                entity
                    .dock_panel()
                    .and_then(|side| {
                        side.as_any()
                            .downcast_ref::<himark::hichanges::ChangesView>()
                    })
                    .map(|view| view.rows())
            })
            .is_some_and(|rows| {
                rows.iter().any(|(depth, label, pick)| {
                    *depth == 1 && label.starts_with("README.md") && *pick
                })
            })
    });
    assert!(himark::test_driver::click(
        &mut engine.app,
        dock_x(90.0),
        changes_row_y(1),
        900.0,
        700.0
    ));
    let composer = |engine: &HimarkEngine| {
        let mut shot = None;
        engine.app.for_each_plugin_panel(&mut |panel| {
            if let Some(canvas) = panel.as_any().downcast_ref::<hidiff::DiffCanvasView>() {
                if matches!(
                    canvas.source(),
                    himark::diff_canvas::CanvasSource::WorkingCopy { .. }
                ) {
                    shot = canvas.probe_composer(engine.app.store());
                }
            }
        });
        shot
    };
    settle_until(&mut engine, "the composer banner stands", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        composer(engine).is_some()
    });

    let chrome_top = himark::env::Themes::of(engine.app.store())
        .ui()
        .toolbar
        .height;
    // The canvas sits in a split half here, so the COMMIT cell eats
    // the right side of the band — click well inside the editor zone.
    assert!(himark::test_driver::click(
        &mut engine.app,
        60.0,
        chrome_top + 14.0,
        900.0,
        700.0
    ));
    assert!(
        composer(&engine).expect("the composer").0,
        "the well click focused the box"
    );
    assert!(himark::test_driver::type_text(
        &mut engine.app,
        "wired commit"
    ));
    assert_eq!(
        composer(&engine).expect("the composer").1,
        "wired commit",
        "typing lands in the box"
    );

    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Backspace,
        imba::event::Modifiers::default(),
    );
    assert_eq!(
        composer(&engine).expect("the composer").1,
        "wired commi",
        "keymap commands route to the box editor"
    );
    assert!(himark::test_driver::type_text(&mut engine.app, "t"));
    let _ = himark::test_driver::key(
        &mut engine.app,
        imba::event::Key::Enter,
        imba::event::Modifiers {
            command: true,
            ..Default::default()
        },
    );
    settle_until(&mut engine, "the working tree emptied", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .and_then(|entity| {
                entity
                    .dock_panel()
                    .and_then(|side| {
                        side.as_any()
                            .downcast_ref::<himark::hichanges::ChangesView>()
                    })
                    .map(|view| view.rows())
            })
            .is_some_and(|rows| {
                rows.iter()
                    .any(|(depth, label, _)| *depth == 1 && label == "no changes")
            })
    });

    assert!(engine.perform_command(window, "history.view"));
    settle_until(&mut engine, "the box's commit topped the graph", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 900.0, 700.0, 1.0);
        rows(engine).is_some_and(|rows| {
            rows.iter()
                .any(|(_, label, _)| label.starts_with("wired commit"))
        })
    });
}

#[test]
#[ignore = "profiling probe; run with --ignored --nocapture"]
fn diff_resize_probe_over_real_code() {
    let (_host, mut engine, window, _fs) = hosted_engine();

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonical root");
    let sh = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    sh(&["init", "-q", "-b", "main"]);
    sh(&["config", "user.email", "test@example.com"]);
    sh(&["config", "user.name", "Test"]);

    let unit = std::fs::read_to_string("frontend/plugins/hidiff/src/lib.rs")
        .unwrap_or_else(|_| "fn main() { println!(\"x\"); }\n".repeat(2_000));
    let body: String = std::iter::repeat(unit.as_str()).take(40).collect();
    std::fs::write(root.join("big.rs"), &body).unwrap();
    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "first"]);

    let edited = format!("{body}\n// the one edited line at the end\n");
    std::fs::write(root.join("big.rs"), &edited).unwrap();
    eprintln!("[probe] big.rs: {} bytes", edited.len());

    let root_segments: Vec<String> = root
        .to_str()
        .unwrap()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    let folder = himark::ResourceLocation::new(
        himark::ResourceType::directory(),
        himark::Authority::new("local"),
        root_segments,
    );
    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);
    assert!(engine.host_picked(request, vec![folder]));
    settle_into_session(&mut engine);

    assert!(engine.perform_command(window, "changes.view"));
    settle(&mut engine);
    let mut surface = skia_safe::surfaces::raster_n32_premul((1400, 900)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1400.0, 900.0, 1.0);
    for ms in [0.0, 1_000.0] {
        let _ = himark::test_driver::animate(
            &mut engine.app,
            imba::anim::AnimationClock::from_millis(ms),
        );
    }
    settle_until(&mut engine, "the changed file landed", |engine| {
        let mut paint = skia_safe::surfaces::raster_n32_premul((1400, 900)).expect("surface");
        let _ = engine.draw(window, paint.canvas(), 1400.0, 900.0, 1.0);
        himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
            .and_then(|entity| {
                entity
                    .dock_panel()
                    .and_then(|side| {
                        side.as_any()
                            .downcast_ref::<himark::hichanges::ChangesView>()
                    })
                    .map(|view| view.rows())
            })
            .is_some_and(|rows| {
                rows.iter()
                    .any(|(depth, label, pick)| *depth == 1 && label.starts_with("big.rs") && *pick)
            })
    });

    assert!(himark::test_driver::click(
        &mut engine.app,
        1400.0 - himark::DOCK_WIDTH + 90.0,
        changes_row_y(1),
        1400.0,
        900.0
    ));
    settle_until(&mut engine, "the diff pane mounted", |engine| {
        let mut mounted = false;
        engine.app.for_each_plugin_panel(&mut |panel| {
            mounted |= panel.as_any().is::<hidiff::DiffPanelView>();
        });
        mounted
    });
    let started = std::time::Instant::now();
    let _ = engine.draw(window, surface.canvas(), 1400.0, 900.0, 1.0);
    eprintln!("[probe] settled frame: {:?}", started.elapsed());

    for _ in 0..40 {
        settle(&mut engine);
    }
    let started = std::time::Instant::now();
    let _ = engine.draw(window, surface.canvas(), 1400.0, 900.0, 1.0);
    eprintln!("[probe] post-repair frame: {:?}", started.elapsed());

    let resize_leg =
        |engine: &mut HimarkEngine, surface: &mut skia_safe::Surface, label: &str, scale: f32| {
            let mut frames = Vec::new();
            for step in 0..16 {
                let width = 1400.0 - (step as f32 + 1.0) * 12.0;
                let started = std::time::Instant::now();
                let _ = engine.draw(window, surface.canvas(), width, 900.0, scale);
                frames.push(started.elapsed());
                settle(engine);
            }
            frames.sort();
            eprintln!(
                "[probe] {label}: resize p50={:?} worst={:?}",
                frames[frames.len() / 2],
                frames.last().unwrap()
            );
        };
    resize_leg(&mut engine, &mut surface, "sole pane @1x", 1.0);
    resize_leg(&mut engine, &mut surface, "sole pane @2x", 2.0);

    assert!(engine.perform_command(window, "workbench.split-pane"));
    settle(&mut engine);
    let _ = engine.draw(window, surface.canvas(), 1400.0, 900.0, 1.0);
    resize_leg(&mut engine, &mut surface, "split @1x", 1.0);
    resize_leg(&mut engine, &mut surface, "split @2x", 2.0);
}

#[test]
fn a_drain_hands_the_ui_thread_back_mid_burst() {
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);

    engine.set_drain_pacing(8, std::time::Duration::ZERO);
    for subscription in 1..=64u64 {
        assert!(engine.file_changed(subscription), "the landing queued");
    }
    assert_eq!(engine.queued_landings(), 64);

    assert!(engine.drain(), "the pass performed work");
    assert_eq!(
        engine.queued_landings(),
        56,
        "ONE chunk performed, then the thread went back — the burst is \
         not drained to empty inside a single call"
    );

    let mut passes = 1;
    while engine.queued_landings() > 0 {
        engine.drain();
        passes += 1;
        assert!(passes < 32, "the lane never emptied");
    }
    assert_eq!(passes, 8, "64 landings, 8 per pass");

    engine.set_drain_pacing(8, std::time::Duration::from_secs(1));
    for subscription in 1..=64u64 {
        assert!(engine.file_changed(subscription));
    }
    assert!(engine.drain());
    assert_eq!(
        engine.queued_landings(),
        0,
        "the burst finished in one pass"
    );
}

#[test]
fn a_markdown_image_shows_under_its_line() {
    let (_host, mut engine, window, fs) = hosted_engine();

    let png = {
        let mut surface = skia_safe::surfaces::raster_n32_premul((6, 3)).expect("surface");
        surface.canvas().clear(skia_safe::Color::GREEN);
        surface
            .image_snapshot()
            .encode(None, skia_safe::EncodedImageFormat::PNG, None)
            .expect("encoded")
            .as_bytes()
            .to_vec()
    };
    fs.write_bytes(&["shot.png"], &png);
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["page.md"],
        "# Page\n\n![a screenshot](shot.png)\n",
    );
    settle_until(&mut engine, "the picture loaded under its line", |engine| {
        let store = engine.app.store();
        himark::OpenDocuments::list(store)
            .into_iter()
            .any(|entity| {
                let document = entity.1.document();
                let len = document.text().byte_count() as u32;
                document.all_inlays_in(0..len).iter().any(|interval| {
                    interval
                        .inlay
                        .view_as::<himarkdown::image::ImageInlay>()
                        .is_some_and(|picture| {
                            interval.inlay.mode() == himark::InlayMode::Under
                                && picture.reference() == "shot.png"
                                && picture.intrinsic().width == 6.0
                                && picture.intrinsic().height == 3.0
                        })
                })
            })
    });
}

#[test]
fn hover_rest_mounts_a_markdown_popup_over_the_word() {
    let (_host, mut engine, window, fs) = hosted_engine();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["hover.rs"],
        "let value = other;\n",
    );
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);

    struct FakeHover;
    impl imba::effect::EffectHandler<himark::hover::LspHoverEffect> for FakeHover {
        async fn handle(
            &self,
            _effect: himark::hover::LspHoverEffect,
        ) -> Option<himark::hover::HoverInfo> {
            Some(himark::hover::HoverInfo {
                markdown: "```rust\nfn value()\n```\n\ndocs for value".to_owned(),
            })
        }
    }
    engine
        .app
        .register_handler::<himark::hover::LspHoverEffect>(FakeHover);

    let sole = engine.app.sole_window();
    let (x, y, w, h) = engine
        .app
        .with_ime_client(sole, |client| client.first_rect(4, 5))
        .expect("focused editor")
        .expect("the word is on screen");
    assert!(
        engine.mouse_move(window, x + w * 0.5, y + h * 0.5),
        "the move over text must be consumed"
    );
    fn popup_standing(engine: &mut HimarkEngine) -> bool {
        let (document_id, editor_id) = engine.app.focused_editor_id();
        himark::OpenDocuments::document_ref(engine.app.store(), document_id)
            .is_some_and(|document| document.has_popups(editor_id))
    }

    settle(&mut engine);
    assert!(
        !popup_standing(&mut engine),
        "no card before the rest matures"
    );

    let rest = |engine: &mut HimarkEngine| {
        for at in [0.0, 500.0, 1_000.0, 1_500.0] {
            let _ = himark::test_driver::animate(
                &mut engine.app,
                imba::anim::AnimationClock::from_millis(at),
            );
        }
    };
    rest(&mut engine);
    settle_until(&mut engine, "the hover popup mounted", popup_standing);

    for _ in 0..3 {
        let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
        settle(&mut engine);
    }
    assert!(
        popup_standing(&mut engine),
        "painting frames must not dismiss the card"
    );

    let _ = engine.mouse_move(window, x + w * 0.5, 690.0);
    settle(&mut engine);
    assert!(
        !popup_standing(&mut engine),
        "leaving the text must dismiss the card"
    );

    assert!(engine.perform_command(window, "workbench.split-pane"));
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    settle(&mut engine);
    for side in [0.25f32, 0.75] {
        let _ = engine.mouse_down(window, 900.0 * side, y + h * 0.5, 0, 1);
        let _ = engine.mouse_up(window, 900.0 * side, y + h * 0.5);
        settle(&mut engine);
        let (x, y, w, h) = engine
            .app
            .with_ime_client(sole, |client| client.first_rect(4, 5))
            .expect("focused editor")
            .expect("the word is on screen after the split");
        assert!(engine.mouse_move(window, x + w * 0.5, y + h * 0.5));
        rest(&mut engine);
        settle_until(
            &mut engine,
            "the hover popup mounted beside a sibling pane",
            popup_standing,
        );

        let _ = engine.mouse_move(window, x + w * 0.5, 690.0);
        settle(&mut engine);
    }

    let (x, y, w, h) = engine
        .app
        .with_ime_client(sole, |client| client.first_rect(4, 5))
        .expect("focused editor")
        .expect("the word is on screen");
    assert!(engine.mouse_move(window, x + w * 0.5, y + h * 0.5));
    rest(&mut engine);
    settle_until(
        &mut engine,
        "the card stood before the cover",
        popup_standing,
    );
    assert!(engine.perform_command(window, "peeker.toggle"));
    settle(&mut engine);
    let _ = engine.mouse_move(window, x + w * 0.5 + 1.0, y + h * 0.5);
    settle(&mut engine);
    assert!(
        !popup_standing(&mut engine),
        "a covered pane's card dismisses on the first missed tick"
    );
    let _ = engine.mouse_move(window, x + w * 0.5, y + h * 0.5);
    rest(&mut engine);
    settle(&mut engine);
    assert!(
        !popup_standing(&mut engine),
        "no card mounts beneath a covering modal"
    );
}

#[test]
fn the_two_call_cut_protocol_cuts_once_and_fills_the_pasteboard() {
    // The apple host asks twice — a sizing probe, then the fill
    // (`sizedString`). Cut deletes the selection, so a naive second
    // run finds nothing and the pasteboard stays empty; the probe
    // must cut once and the fill must drain the stash.
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);

    // The fresh scratch pane holds text focus: type, then cmd-a.
    let body = "the text about to be cut";
    assert!(engine.text_input(window, body));
    assert!(
        engine.key_down(window, u32::from('a'), crate::HIMARK_MOD_COMMAND),
        "cmd-a selects all"
    );

    let two_call_cut = |engine: &mut HimarkEngine| -> Option<String> {
        let engine: *mut HimarkEngine = engine;
        let needed = unsafe { crate::himark_cut(engine, window, std::ptr::null_mut(), 0) };
        if needed == 0 {
            return None;
        }
        let mut buffer = vec![0i8; needed];
        let written =
            unsafe { crate::himark_cut(engine, window, buffer.as_mut_ptr(), buffer.len()) };
        assert_eq!(written, needed, "the fill answers what the probe sized");
        let bytes: Vec<u8> = buffer.iter().map(|byte| *byte as u8).collect();
        Some(String::from_utf8(bytes).expect("utf8"))
    };

    assert_eq!(
        two_call_cut(&mut engine).as_deref(),
        Some(body),
        "the pasteboard gets the selection"
    );
    assert_eq!(
        engine.clipboard_copy(window),
        None,
        "the selection was consumed by the cut"
    );
    assert_eq!(
        two_call_cut(&mut engine),
        None,
        "a second cut with nothing selected sizes to zero"
    );
}

/// Mode one, end to end: with a documents-extension host the CLIENT
/// stops watching the file — the host reloads the mirror on disk
/// changes and broadcasts its own edit, which reaches the buffer
/// through the document channel alone.
#[test]
fn a_hosted_documents_disk_change_arrives_through_the_channel() {
    let (_host, mut engine, window, fs) = hosted_engine();
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["agent.md"],
        "alpha\nbeta\n",
    );

    // The document channel goes live: the host owns the file now.
    settle_until(&mut engine, "the channel went live", |engine| {
        himark::OpenDocuments::list(engine.app.store())
            .into_iter()
            .any(|(id, entity)| {
                entity.name() == "agent.md"
                    && himark::OpenDocuments::host_synced(engine.app.store(), id)
            })
    });
    let document_id = himark::OpenDocuments::list(engine.app.store())
        .into_iter()
        .find(|(_, entity)| entity.name() == "agent.md")
        .map(|(id, _)| id)
        .expect("the open");
    assert_eq!(
        himark::OpenDocuments::entity(engine.app.store(), document_id)
            .expect("registered")
            .watch(),
        None,
        "no client-side file watch in mode one"
    );

    // The agent writes the file behind everyone's back.
    fs.write(&["agent.md"], "alpha\nAGENT\nbeta\n");

    settle_until(&mut engine, "the host's edit landed", |engine| {
        himark::OpenDocuments::document_ref(engine.app.store(), document_id).is_some_and(
            |document| {
                let mut view = document.text().view();
                let end = view.byte_count().min(u32::MAX as usize) as u32;
                view.substring(0..end) == "alpha\nAGENT\nbeta\n"
            },
        )
    });
}

/// The regression that shipped 2026-09-15: every external change
/// applied TWICE. Emacs-style saves (write a temp file, rename it
/// over the target), repeated edits, local typing interleaved, and
/// FULL-EQUALITY assertions — `contains` hid the duplication.
#[test]
fn repeated_external_saves_land_exactly_once_each() {
    let (_host, mut engine, window, fs) = hosted_engine();
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["emacs.md"],
        "alpha\nbeta\n",
    );
    settle_until(&mut engine, "the channel went live", |engine| {
        himark::OpenDocuments::list(engine.app.store())
            .into_iter()
            .any(|(id, entity)| {
                entity.name() == "emacs.md"
                    && himark::OpenDocuments::host_synced(engine.app.store(), id)
            })
    });
    let document_id = himark::OpenDocuments::list(engine.app.store())
        .into_iter()
        .find(|(_, entity)| entity.name() == "emacs.md")
        .map(|(id, _)| id)
        .expect("the open");

    let emacs_save = |fs: &HostedFs, text: &str| {
        let target = fs.path(&["emacs.md"]);
        let temp = fs.path(&["#emacs.md.tmp#"]);
        std::fs::write(&temp, text).expect("temp write");
        std::fs::rename(&temp, &target).expect("rename over");
    };
    let text_of = |engine: &HimarkEngine| -> String {
        let document =
            himark::OpenDocuments::document_ref(engine.app.store(), document_id).expect("open");
        let mut view = document.text().view();
        let end = view.byte_count().min(u32::MAX as usize) as u32;
        view.substring(0..end)
    };
    // The ladder is the duplicate detector: byte-EQUALITY is each
    // step's settle condition, so a straggling second application
    // from step N corrupts step N+1's wait and fails it loudly. No
    // sleeps — a wrong text never settles.
    let landed = |engine: &mut HimarkEngine, expected: &str| {
        settle_until(engine, "the save landed byte-exact", |engine| {
            text_of(engine) == expected
        });
    };

    // Ladder one: a clean buffer follows repeated saves, once each.
    for expected in [
        "alpha\nONE\nbeta\n",
        "alpha\nONE\nTWO\nbeta\n",
        "alpha\nONE\nTWO\nbeta\nTHREE\n",
    ] {
        emacs_save(&fs, expected);
        landed(&mut engine, expected);
    }

    // Local typing joins the history and flows to the host's mirror.
    assert!(himark::test_driver::type_text(&mut engine.app, "typed "));
    settle(&mut engine);
    let merged = text_of(&engine);
    assert!(
        merged.starts_with("typed "),
        "the caret sat at 0: {merged:?}"
    );

    // Its echo (the buffer saved to disk) must change nothing — the
    // next ladder step's equality catches it if it does.
    emacs_save(&fs, &merged);

    // Ladder two: external saves over a document WITH history.
    for tail in ["FOUR\n", "FOUR\nFIVE\n"] {
        let expected = format!("{merged}{tail}");
        emacs_save(&fs, &expected);
        landed(&mut engine, &expected);
    }
}

/// The character-doubling regression: closing a document must
/// UNSUBSCRIBE its channel. A close used to abort the sync loop
/// mid-flight and leak the host-side subscription; reopening the
/// file subscribed AGAIN, every `document/applied` arrived once per
/// leaked row, and the rebase log applied the duplicates as foreign
/// edits — each typed character landed once per leak on top of
/// itself, and the shared applies kept the dirty flag stuck. Two
/// close/reopen cycles, then byte-exact typing: a duplicate never
/// settles.
#[test]
fn a_reopened_document_types_exactly_once() {
    let (_host, mut engine, window, fs) = hosted_engine();
    let live = |engine: &HimarkEngine| {
        himark::OpenDocuments::list(engine.app.store())
            .into_iter()
            .find(|(id, entity)| {
                entity.name() == "cycle.md"
                    && himark::OpenDocuments::host_synced(engine.app.store(), *id)
            })
            .map(|(id, _)| id)
    };

    for _ in 0..2 {
        open_picked(
            &mut engine,
            &_host.seat,
            window,
            &fs,
            &["cycle.md"],
            "alpha\n",
        );
        settle_until(&mut engine, "the channel went live", |engine| {
            live(engine).is_some()
        });
        assert_eq!(
            hiahp::docsync::SyncSeats::count(engine.app.store()),
            1,
            "one open, one sync loop"
        );
        assert!(engine.perform_command(window, "workbench.close"));
        settle_until(&mut engine, "the close took the sync loop", |engine| {
            hiahp::docsync::SyncSeats::count(engine.app.store()) == 0
        });
    }

    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["cycle.md"],
        "alpha\n",
    );
    settle_until(&mut engine, "the channel went live again", |engine| {
        live(engine).is_some()
    });
    let document_id = live(&engine).expect("the reopened document");
    let text_of = |engine: &HimarkEngine| -> String {
        let document =
            himark::OpenDocuments::document_ref(engine.app.store(), document_id).expect("open");
        let mut view = document.text().view();
        let end = view.byte_count().min(u32::MAX as usize) as u32;
        view.substring(0..end)
    };

    assert!(himark::test_driver::type_text(&mut engine.app, "typed "));
    settle_until(&mut engine, "the typed text landed byte-exact", |engine| {
        text_of(engine) == "typed alpha\n"
    });
    // The echo round trip is the duplicator's moment, and external
    // appends are the deterministic wait for it: the channel is
    // ordered, so by the time an external edit shows, the typed
    // echo — and every duplicate a leaked subscription would add —
    // has already applied. Byte-EQUALITY makes any duplicate fail
    // the ladder loudly. The external writes touch only the TAIL
    // (never the typed prefix), so the three-way merge has no shared
    // insert to race on, whichever of the echo and the reload lands
    // first.
    for (disk, expected) in [
        ("alpha\nONE\n", "typed alpha\nONE\n"),
        ("alpha\nONE\nTWO\n", "typed alpha\nONE\nTWO\n"),
    ] {
        fs.write(&["cycle.md"], disk);
        settle_until(&mut engine, "the append landed byte-exact", |engine| {
            text_of(engine) == expected
        });
    }
    assert_eq!(
        hiahp::docsync::SyncSeats::count(engine.app.store()),
        1,
        "one document, one sync loop"
    );
}

/// The user's exact repro (2026-09-15): open a WORKING-COPY DIFF of a
/// file first, then the file in a normal editor, then edit it
/// externally — the diff's fetch used to orphan a document channel
/// whose leaked subscription made every later broadcast apply twice.
#[test]
fn a_diff_opened_before_the_editor_does_not_double_reloads() {
    struct OpenWorkingDiff {
        old: himark::ResourceLocation,
        new: himark::ResourceLocation,
    }
    impl himark::DynamicCommand for OpenWorkingDiff {
        fn id(&self) -> &'static str {
            "test.open-working-diff"
        }
        fn name(&self) -> String {
            "Open Working Diff".to_owned()
        }
        fn perform(
            &self,
            _app: &mut himark::Application,
            _store: &mut imba::store::Store,
            window: himark::WindowId,
            fx: &mut himark::AppFx<'_>,
        ) {
            fx.push(imba::effect::AnyEffect::new(
                himark::OpenDiffByLocationsEffect {
                    window,
                    old: self.old.clone(),
                    new: self.new.clone(),
                },
            ));
        }
    }

    let (_host, mut engine, window, fs) = hosted_engine();
    fs.write(&["old.md"], "alpha\n");
    fs.write(&["live.md"], "alpha\nbeta\n");

    // The diff FIRST — its sides fetch and register.
    let _ = engine.app.perform_batch(vec![himark::AppCommand::Dynamic(
        himark::WindowId::from_raw(window),
        std::sync::Arc::new(OpenWorkingDiff {
            old: fs.doc(&["old.md"]),
            new: fs.doc(&["live.md"]),
        }),
    )]);
    settle_until(&mut engine, "the diff registered its sides", |engine| {
        himark::OpenDocuments::list(engine.app.store())
            .into_iter()
            .any(|(id, entity)| {
                entity.name() == "live.md"
                    && himark::OpenDocuments::host_synced(engine.app.store(), id)
            })
    });

    // THEN the normal editor.
    open_picked(
        &mut engine,
        &_host.seat,
        window,
        &fs,
        &["live.md"],
        "alpha\nbeta\n",
    );
    let document_id = himark::OpenDocuments::list(engine.app.store())
        .into_iter()
        .find(|(_, entity)| entity.name() == "live.md")
        .map(|(id, _)| id)
        .expect("the open");
    let text_of = |engine: &HimarkEngine| -> String {
        let document =
            himark::OpenDocuments::document_ref(engine.app.store(), document_id).expect("open");
        let mut view = document.text().view();
        let end = view.byte_count().min(u32::MAX as usize) as u32;
        view.substring(0..end)
    };

    // External edits, byte-exact, several in a row. Equality is the
    // settle condition: a duplicate from any step corrupts the next
    // step's wait — the ladder is the detector, no sleeps.
    for expected in [
        "alpha\nEXTERNAL\nbeta\n",
        "alpha\nEXTERNAL\nbeta\nMORE\n",
        "alpha\nEXTERNAL\nbeta\nMORE\nSTILL\n",
    ] {
        let target = fs.path(&["live.md"]);
        let temp = fs.path(&["#live.md.tmp#"]);
        std::fs::write(&temp, expected).expect("temp write");
        std::fs::rename(&temp, &target).expect("rename over");
        settle_until(&mut engine, "the external change landed once", |engine| {
            text_of(engine) == expected
        });
    }
}

#[test]
fn implementations_stream_into_the_search_dock_over_the_wire() {
    let ls_dir = tempfile::tempdir().expect("ls dir");
    let (_host, mut engine, window, fs) =
        hosted_engine_with_language_servers(vec![agent_host::LanguageServer {
            extensions: vec!["rs".to_owned()],
            command: agent_host::testing::fake_ls_command(ls_dir.path()),
        }]);
    fs.write(&["project", "lib.rs"], "fn answer() -> u32 { 42 }\n");

    assert!(engine.perform_command(window, "file.open"));
    settle(&mut engine);
    let request = pick_request(&mut engine, &_host.seat);
    assert!(engine.host_picked(request, vec![fs.dir(&["project"])]));
    settle_until(&mut engine, "the folder session opened", |engine| {
        let entity_id = engine.app.sole_window();
        let workspace = himark::Windows::window_ref(engine.app.store(), entity_id)
            .expect("the window entity")
            .current_session();
        !himark::higent::session_folders(engine.app.store(), &workspace).is_empty()
    });
    let session = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
        .expect("the window entity")
        .current_session();
    let folders = himark::higent::session_folders(engine.app.store(), &session);
    let file = himark::ResourceLocation::new(
        himark::ResourceType::document(),
        folders[0].authority().clone(),
        {
            let mut segments = folders[0].path().to_vec();
            segments.push("lib.rs".to_owned());
            segments
        },
    );

    struct Open(himark::ResourceLocation);
    impl himark::DynamicCommand for Open {
        fn id(&self) -> &'static str {
            "test.open-lib"
        }
        fn name(&self) -> String {
            "Open".to_owned()
        }
        fn perform(
            &self,
            app: &mut himark::Application,
            store: &mut imba::store::Store,
            window: himark::WindowId,
            fx: &mut himark::AppFx<'_>,
        ) {
            let ui = &app.ui_ctx();
            himark::open_locations(store, ui, window, &[self.0.clone()], fx);
        }
    }
    assert!(engine.app.perform_command(himark::AppCommand::Dynamic(
        wid(window),
        Arc::new(Open(file.clone()))
    )));
    settle_until(&mut engine, "lib.rs opened", |engine| {
        himark::OpenDocuments::by_location(engine.app.store(), &file).is_some()
    });

    let command =
        himark::palette_commands(engine.app.store(), &engine.app.ui_handle(), wid(window))
            .into_iter()
            .find(|presentable| presentable.id == "code.implementations")
            .expect("the located editor offers implementations")
            .command;
    assert!(engine.app.perform_command(command));

    settle_until(&mut engine, "the stream resolved into the feed", |engine| {
        himark::locations::SessionSearchFeeds::feed(engine.app.store(), &session)
            .and_then(|feed| himark::locations::LocationsFeeds::row(engine.app.store(), feed))
            .is_some_and(|row| row.done)
    });
    let feed = himark::locations::SessionSearchFeeds::feed(engine.app.store(), &session)
        .expect("the session fronts the feed");
    let row =
        himark::locations::LocationsFeeds::row(engine.app.store(), feed).expect("the feed row");
    assert!(!row.truncated, "the ask answered whole");
    assert_eq!(
        row.locations.len(),
        2,
        "the fake server's two implementation targets landed resolved"
    );
    assert!(row
        .locations
        .iter()
        .all(|found| found.location.path().join("/").ends_with("lib.rs")
            && found.context == "fn answer() -> u32 { 42 }"));
    let entity = himark::Windows::window_ref(engine.app.store(), engine.app.sole_window())
        .expect("the window entity");
    assert_eq!(
        entity.dock_owner(),
        Some(himark::hisearch::OWNER),
        "the Search tab activated"
    );

    // The references leg: same wire, the fake answers two plain
    // Locations; the fresh feed DISPLACES the implementations one
    // (the supersession rule), and the old feed disposes.
    let command =
        himark::palette_commands(engine.app.store(), &engine.app.ui_handle(), wid(window))
            .into_iter()
            .find(|presentable| presentable.id == "code.references")
            .expect("the located editor offers references")
            .command;
    assert!(engine.app.perform_command(command));
    settle_until(&mut engine, "the references resolved", |engine| {
        himark::locations::SessionSearchFeeds::feed(engine.app.store(), &session)
            .and_then(|next| himark::locations::LocationsFeeds::row(engine.app.store(), next))
            .is_some_and(|row| row.done && row.title.starts_with("References"))
    });
    let referenced = himark::locations::SessionSearchFeeds::feed(engine.app.store(), &session)
        .expect("the session fronts the references feed");
    assert_ne!(referenced, feed, "a fresh feed displaced the old one");
    let row = himark::locations::LocationsFeeds::row(engine.app.store(), referenced)
        .expect("the references feed row");
    assert!(!row.truncated, "the references ask answered whole");
    assert_eq!(row.locations.len(), 2, "the fake's two references landed");
    settle_until(&mut engine, "the displaced feed disposed", |engine| {
        himark::locations::LocationsFeeds::row(engine.app.store(), feed).is_none()
    });
}

/// cmd-shift-f through the REAL key road: the chord falls through the
/// focus chain to the keymap, lands on `search.focus`, opens the
/// search dock — and re-invoked on an open one it FOCUSES (never
/// toggles away); the toolbar's `search.view` keeps the toggle.
#[test]
fn cmd_shift_f_opens_then_focuses_the_search_dock() {
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    // The capability gate normally registers these when the seat
    // serves searchLocations; the KEY ROAD under test is the same.
    engine
        .app
        .register_command(std::sync::Arc::new(himark::hisearch::ToggleSearchView));
    engine
        .app
        .register_command(std::sync::Arc::new(himark::hisearch::FocusSearchView));

    let mut surface = skia_safe::surfaces::raster_n32_premul((900, 700)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);

    assert!(
        engine.key_down(
            window,
            u32::from('f'),
            HIMARK_MOD_COMMAND | HIMARK_MOD_SHIFT
        ),
        "the chord reached the keymap fallback"
    );
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    assert_eq!(
        engine.app.dock_owner_for_tests(wid(window)),
        Some("search.view"),
        "cmd-shift-f opened the search dock"
    );

    // Again on an open dock: still open (focused, not toggled away).
    assert!(engine.key_down(
        window,
        u32::from('f'),
        HIMARK_MOD_COMMAND | HIMARK_MOD_SHIFT
    ));
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    assert_eq!(
        engine.app.dock_owner_for_tests(wid(window)),
        Some("search.view"),
        "re-invoke keeps the dock, focusing the input"
    );

    // The toolbar's command still TOGGLES like every dock button.
    assert!(engine.perform_command(window, "search.view"));
    let _ = engine.draw(window, surface.canvas(), 900.0, 700.0, 1.0);
    assert_eq!(
        engine.app.dock_owner_for_tests(wid(window)),
        None,
        "search.view rolls the fronting dock away"
    );
}
