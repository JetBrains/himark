// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

#[cfg(any(target_os = "emscripten", test))]
mod clicks;

#[cfg(not(target_os = "emscripten"))]
fn main() {
    eprintln!("build this crate for wasm32-unknown-emscripten");
}

#[cfg(target_os = "emscripten")]
mod browser;

#[cfg(all(target_os = "emscripten", target_feature = "atomics"))]
mod tailnet;

#[cfg(target_os = "emscripten")]
mod app {
    use std::{
        ffi::CStr,
        os::raw::{c_char, c_double, c_int, c_void},
    };

    use gfx::Painter as _;
    use himark::app::AppFonts;
    use himark::app::Himark;
    use skia_safe::{
        gpu::{
            self, backend_render_targets, direct_contexts, gl, interfaces, surfaces, DirectContext,
            SurfaceOrigin,
        },
        ColorType,
    };

    const CANVAS: *const c_char = b"#canvas\0".as_ptr().cast();
    const EVENT_TARGET_WINDOW: *const c_char = 2 as *const c_char;
    const CALLBACK_THREAD_CALLING: usize = 2;
    const DOM_DELTA_LINE: u32 = 1;
    const DOM_DELTA_PAGE: u32 = 2;
    const GL_STENCIL_BITS: usize = 8;

    #[repr(C)]
    struct EmscriptenWebGLContextAttributes {
        alpha: bool,
        depth: bool,
        stencil: bool,
        antialias: bool,
        premultiplied_alpha: bool,
        preserve_drawing_buffer: bool,
        power_preference: c_int,
        fail_if_major_performance_caveat: bool,
        major_version: c_int,
        minor_version: c_int,
        enable_extensions_by_default: bool,
        explicit_swap_control: bool,
        proxy_context_to_main_thread: c_int,
        render_via_offscreen_back_buffer: bool,
        desynchronized: bool,
    }

    #[repr(C)]
    struct EmscriptenKeyboardEvent {
        timestamp: c_double,
        location: u32,
        ctrl_key: bool,
        shift_key: bool,
        alt_key: bool,
        meta_key: bool,
        repeat: bool,
        char_code: u32,
        key_code: u32,
        which: u32,
        key: [c_char; 32],
        code: [c_char; 32],
        char_value: [c_char; 32],
        locale: [c_char; 32],
    }

    #[repr(C)]
    struct EmscriptenTouchPoint {
        identifier: c_int,
        screen_x: c_int,
        screen_y: c_int,
        client_x: c_int,
        client_y: c_int,
        page_x: c_int,
        page_y: c_int,
        is_changed: bool,
        on_target: bool,
        target_x: c_int,
        target_y: c_int,
        canvas_x: c_int,
        canvas_y: c_int,
    }

    #[repr(C)]
    struct EmscriptenTouchEvent {
        timestamp: c_double,
        num_touches: c_int,
        ctrl_key: bool,
        shift_key: bool,
        alt_key: bool,
        meta_key: bool,
        touches: [EmscriptenTouchPoint; 32],
    }

    #[repr(C)]
    struct EmscriptenMouseEvent {
        timestamp: c_double,
        screen_x: c_int,
        screen_y: c_int,
        client_x: c_int,
        client_y: c_int,
        ctrl_key: bool,
        shift_key: bool,
        alt_key: bool,
        meta_key: bool,
        button: u16,
        buttons: u16,
        movement_x: c_int,
        movement_y: c_int,
        target_x: c_int,
        target_y: c_int,
        canvas_x: c_int,
        canvas_y: c_int,
        padding: c_int,
    }

    #[repr(C)]
    struct EmscriptenWheelEvent {
        mouse: EmscriptenMouseEvent,
        delta_x: c_double,
        delta_y: c_double,
        delta_z: c_double,
        delta_mode: u32,
    }

    #[repr(C)]
    struct EmscriptenUiEvent {
        detail: c_int,
        document_body_client_width: c_int,
        document_body_client_height: c_int,
        window_inner_width: c_int,
        window_inner_height: c_int,
        window_outer_width: c_int,
        window_outer_height: c_int,
        scroll_top: c_int,
        scroll_left: c_int,
    }

    extern "C" {
        fn emscripten_webgl_init_context_attributes(
            attributes: *mut EmscriptenWebGLContextAttributes,
        );
        fn emscripten_webgl_create_context(
            target: *const c_char,
            attributes: *const EmscriptenWebGLContextAttributes,
        ) -> usize;
        fn emscripten_webgl_make_context_current(context: usize) -> c_int;
        fn emscripten_get_element_css_size(
            target: *const c_char,
            width: *mut c_double,
            height: *mut c_double,
        ) -> c_int;
        fn emscripten_set_canvas_element_size(
            target: *const c_char,
            width: c_int,
            height: c_int,
        ) -> c_int;
        fn emscripten_get_device_pixel_ratio() -> c_double;
        fn emscripten_request_animation_frame(
            callback: Option<extern "C" fn(c_double, *mut c_void) -> bool>,
            user_data: *mut c_void,
        ) -> c_int;
        fn emscripten_set_wheel_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<
                extern "C" fn(c_int, *const EmscriptenWheelEvent, *mut c_void) -> bool,
            >,
            target_thread: usize,
        ) -> c_int;
        fn emscripten_set_mousedown_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<
                extern "C" fn(c_int, *const EmscriptenMouseEvent, *mut c_void) -> bool,
            >,
            target_thread: usize,
        ) -> c_int;
        fn emscripten_set_keydown_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<
                extern "C" fn(c_int, *const EmscriptenKeyboardEvent, *mut c_void) -> bool,
            >,
            target_thread: usize,
        ) -> c_int;
        fn emscripten_set_touchstart_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<
                extern "C" fn(c_int, *const EmscriptenTouchEvent, *mut c_void) -> bool,
            >,
            target_thread: usize,
        ) -> c_int;
        fn emscripten_set_touchmove_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<
                extern "C" fn(c_int, *const EmscriptenTouchEvent, *mut c_void) -> bool,
            >,
            target_thread: usize,
        ) -> c_int;
        fn emscripten_set_touchend_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<
                extern "C" fn(c_int, *const EmscriptenTouchEvent, *mut c_void) -> bool,
            >,
            target_thread: usize,
        ) -> c_int;
        fn emscripten_set_touchcancel_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<
                extern "C" fn(c_int, *const EmscriptenTouchEvent, *mut c_void) -> bool,
            >,
            target_thread: usize,
        ) -> c_int;
        fn emscripten_set_mousemove_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<
                extern "C" fn(c_int, *const EmscriptenMouseEvent, *mut c_void) -> bool,
            >,
            target_thread: usize,
        ) -> c_int;
        fn emscripten_set_mouseleave_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<
                extern "C" fn(c_int, *const EmscriptenMouseEvent, *mut c_void) -> bool,
            >,
            target_thread: usize,
        ) -> c_int;
        fn emscripten_set_mouseup_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<
                extern "C" fn(c_int, *const EmscriptenMouseEvent, *mut c_void) -> bool,
            >,
            target_thread: usize,
        ) -> c_int;
        fn emscripten_set_blur_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<extern "C" fn(c_int, *const c_void, *mut c_void) -> bool>,
            target_thread: usize,
        ) -> c_int;
        fn emscripten_run_script_string(script: *const c_char) -> *const c_char;
        fn emscripten_run_script(script: *const c_char);
        fn emscripten_set_resize_callback_on_thread(
            target: *const c_char,
            user_data: *mut c_void,
            use_capture: bool,
            callback: Option<extern "C" fn(c_int, *const EmscriptenUiEvent, *mut c_void) -> bool>,
            target_thread: usize,
        ) -> c_int;
    }

    pub fn main() {
        unsafe {
            let mut attributes: EmscriptenWebGLContextAttributes = std::mem::zeroed();
            emscripten_webgl_init_context_attributes(&mut attributes);
            attributes.alpha = true;
            attributes.depth = false;
            attributes.stencil = true;
            attributes.antialias = true;
            attributes.premultiplied_alpha = true;
            attributes.major_version = 2;
            attributes.minor_version = 0;
            attributes.enable_extensions_by_default = true;

            let handle = emscripten_webgl_create_context(CANVAS, &attributes);
            assert!(handle != 0, "failed to create WebGL context");
            assert_eq!(
                emscripten_webgl_make_context_current(handle),
                0,
                "failed to make WebGL context current"
            );

            let interface =
                interfaces::make_web_gl().expect("failed to create Skia WebGL interface");
            let context = direct_contexts::make_gl(interface, None)
                .expect("failed to create Skia direct context");
            // No font fetch: every face the app speaks (Noto Sans,
            // the JetBrains Mono cuts, Bricolage Grotesque) ships in
            // the wasm — the world seeds them on every platform.
            let world = editor::parley_line::world();
            let app = Box::into_raw(Box::new(WebApp::new(context, world)));
            TEXT_SEAT.store(app, std::sync::atomic::Ordering::Release);
            let app = app.cast::<c_void>();
            emscripten_request_animation_frame(Some(animation_frame), app);
            install_callbacks(app);
        }
    }

    struct WebApp {
        state: Himark,

        scroll_gesture: imba::event::ScrollGesture,
        last_scroll_ms: Option<f64>,
        clicks: crate::clicks::ClickCounter,
        /// Only presses in this canvas can start a selection drag.
        drag_point: Option<gfx::Point>,
        /// The engine's text focus as last told to the page — the
        /// soft keyboard's state (`sync_keyboard`).
        wants_text: bool,
        /// Command performed → you render: every entry point ORs its
        /// dispatch's answer in; the next animation frame paints and
        /// clears it (the paint's own answer re-arms — belts perform).
        needs_redraw: bool,
        /// The standing one-finger TOUCH PAN, scroll by synthesis:
        /// taps stay the browser's (it synthesizes the mouse events
        /// the click road already speaks), a move past the slop
        /// becomes a scroll gesture.
        pan: Option<TouchPan>,
        /// The pan's momentum after the finger lifts — decayed and
        /// dispatched once per animation frame until it dies or a
        /// touch catches it.
        fling: Option<Fling>,
        /// The one browser canvas is one himark window.
        window: workbench::window::WindowId,
        context: DirectContext,

        #[cfg(target_feature = "atomics")]
        arriving: std::sync::mpsc::Receiver<himark::app::AppCommand>,
        width: i32,
        height: i32,
        logical_width: f32,
        logical_height: f32,
        scale: f32,
    }

    impl WebApp {
        fn new(context: DirectContext, world: editor::parley_line::FontWorld) -> Self {
            let mut state = workbench_shell::app::workbench_app(AppFonts::over(world));

            // The browser is the production workbench in a tab: the
            // SAME plugin list the desktop assembles — no hand-wired
            // twin of any plugin's install.
            let plugins: Vec<Box<dyn himark::plugin::Plugin>> = {
                let mut plugins = himark_plugins::engine_plugins();
                plugins.extend([
                    Box::new(palette::PalettePlugin) as Box<dyn himark::plugin::Plugin>,
                    Box::new(peeker::PeekerPlugin),
                    Box::new(workbench_shell::diff_pane::DiffPanePlugin),
                    Box::new(demo::DemoPlugin),
                    Box::new(workbench_shell::higent::AgentsPlugin),
                ]);
                plugins.extend(workbench_shell::plugins());
                plugins
            };
            let installed = himark::plugin::install_all(&mut state, &plugins);
            #[cfg(not(target_feature = "atomics"))]
            let _ = &installed;

            #[cfg(target_feature = "atomics")]
            let (posted, arriving) = std::sync::mpsc::channel();

            #[cfg(target_feature = "atomics")]
            {
                let url = unsafe {
                    let raw = emscripten_run_script_string(c"window.HIMARK_AHP_URL || ''".as_ptr());
                    match raw.is_null() {
                        true => String::new(),
                        false => std::ffi::CStr::from_ptr(raw).to_string_lossy().into_owned(),
                    }
                };
                // The wire stack boots with or without a served host:
                // a statically hosted page (GitHub Pages) starts
                // hostless and connects over the tailnet; a
                // daemon-served page also seeds its server below.
                {
                    let (handle_tx, handle_rx) = std::sync::mpsc::channel();
                    std::thread::Builder::new()
                        .name("himark-wire".to_owned())
                        .stack_size(8 << 20)
                        .spawn(move || {
                            let runtime = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .expect("wire runtime");
                            let _ = handle_tx.send(runtime.handle().clone());
                            runtime.block_on(std::future::pending::<()>());
                        })
                        .expect("wire thread");
                    let handle = handle_rx.recv().expect("wire handle");

                    use std::sync::Arc;
                    let post: Arc<dyn Fn(himark::app::AppCommand) + Send + Sync> = {
                        let posted = posted.clone();
                        Arc::new(move |command| {
                            let _ = posted.send(command);
                        })
                    };
                    let wires = himark_plugins::wire_doors(
                        &mut state,
                        handle.clone(),
                        post,
                        std::sync::Arc::new(crate::browser::BrowserConnector),
                        Arc::new(ahp_wire::uris::FileUris),
                        Arc::clone(&installed.languages),
                    );
                    let clients = Arc::clone(&wires.clients);

                    if !url.is_empty() {
                        let client = ahp_wire::client::Client::of(std::sync::Arc::new(
                            ahp_wire::wire::WireHost::at(
                                handle.clone(),
                                std::sync::Arc::new(crate::browser::BrowserConnector),
                                url,
                            ),
                        ));
                        let host = himark_plugins::register_agent_server(
                            &mut state,
                            &clients,
                            "This Host",
                            client,
                        );
                        state.designate_local_host(host);
                        clients.set_local(host);
                    }

                    // The in-page tailnet: the command loads the Go
                    // wasm node and logs in; discovery turns its
                    // netmap into host rows dialed tailnet://…:7298.
                    state.perform_batch(vec![himark::app::AppCommand::Register(
                        std::sync::Arc::new(crate::tailnet::ConnectTailnet),
                    )]);
                    crate::tailnet::spawn_discovery(
                        handle.clone(),
                        posted.clone(),
                        Arc::clone(&clients),
                    );

                    // The agent-host filesystem ROUTES — the one
                    // door the desktop uses (hiahp::agent_fs); the
                    // fetch/store/list wire handlers came with
                    // `wire_doors`.
                    hiahp::agent_fs::install_routes(
                        &mut state,
                        true,
                        true,
                        true,
                        &wires.languages,
                        &wires.clients,
                        &wires.resource_uris,
                    );
                }
            }
            let window = state.add_window();

            state.perform_batch(vec![workbench_shell::shell::windowed(
                window,
                std::sync::Arc::new(workbench_shell::higent::open_session::OpenNewSession {
                    host: None,
                }),
            )]);

            #[cfg(target_feature = "atomics")]
            {
                use std::sync::{mpsc, Arc};
                let (signal, work) = mpsc::channel::<()>();
                let runner = state.attach_host(
                    Arc::new(move |command| {
                        let _ = posted.send(command);
                    }),
                    Arc::new({
                        let signal = std::sync::Mutex::new(signal);
                        move || {
                            let _ = signal.lock().expect("signal channel").send(());
                        }
                    }),
                );

                std::thread::Builder::new()
                    .name("himark-effects".to_owned())
                    .stack_size(8 << 20)
                    .spawn(move || {
                        runner.run();
                        while work.recv().is_ok() {
                            runner.run();
                        }
                    })
                    .expect("effect worker");
            }
            Self {
                state,
                scroll_gesture: imba::event::ScrollGesture::default(),
                last_scroll_ms: None,
                clicks: crate::clicks::ClickCounter::default(),
                drag_point: None,
                wants_text: false,
                needs_redraw: true,
                pan: None,
                fling: None,
                window,
                context,
                #[cfg(target_feature = "atomics")]
                arriving,
                width: 1,
                height: 1,
                logical_width: 1.0,
                logical_height: 1.0,
                scale: 1.0,
            }
        }

        #[cfg(target_feature = "atomics")]
        fn drain_worker(&mut self) {
            while let Ok(command) = self.arriving.try_recv() {
                self.needs_redraw |= self.state.perform_batch(vec![command]);
            }
        }

        #[cfg(not(target_feature = "atomics"))]
        fn drain_worker(&mut self) {}

        fn draw(&mut self) -> bool {
            self.resize_canvas();
            let fb_info = gl::FramebufferInfo {
                fboid: 0,
                format: gl::Format::RGBA8.into(),
                protected: gpu::Protected::No,
            };
            let backend_render_target = backend_render_targets::make_gl(
                (self.width, self.height),
                None,
                GL_STENCIL_BITS,
                fb_info,
            );
            let mut surface = surfaces::wrap_backend_render_target(
                &mut self.context,
                &backend_render_target,
                SurfaceOrigin::BottomLeft,
                ColorType::RGBA8888,
                None,
                None,
            )
            .expect("failed to wrap default framebuffer");

            let canvas = gfx_skia::SkiaCanvas::new(surface.canvas());
            canvas.save();
            let reconciling = self.state.draw_window_sized(
                self.window,
                &canvas,
                gfx::Size::new(self.width.max(1) as f32, self.height.max(1) as f32),
            );
            canvas.restore();

            self.context.flush_and_submit_surface(&mut surface, None);
            reconciling
        }

        fn resize_canvas(&mut self) {
            let mut css_width = 1.0;
            let mut css_height = 1.0;
            unsafe {
                emscripten_get_element_css_size(CANVAS, &mut css_width, &mut css_height);
            }
            let scale = unsafe { emscripten_get_device_pixel_ratio() }.max(1.0) as f32;
            let width = (css_width.max(1.0) * scale as f64).round() as i32;
            let height = (css_height.max(1.0) * scale as f64).round() as i32;
            self.logical_width = css_width.max(1.0) as f32;
            self.logical_height = css_height.max(1.0) as f32;
            if width != self.width || height != self.height {
                unsafe {
                    emscripten_set_canvas_element_size(CANVAS, width, height);
                }
                self.width = width;
                self.height = height;
                self.needs_redraw = true;
            }
            if scale != self.scale {
                self.needs_redraw = true;
            }
            self.scale = scale;
        }
    }

    unsafe fn install_callbacks(app: *mut c_void) {
        emscripten_set_wheel_callback_on_thread(
            CANVAS,
            app,
            true,
            Some(wheel),
            CALLBACK_THREAD_CALLING,
        );
        emscripten_set_mousedown_callback_on_thread(
            CANVAS,
            app,
            true,
            Some(mouse_down),
            CALLBACK_THREAD_CALLING,
        );
        // Window listeners retain a canvas-started drag outside its bounds.
        emscripten_set_mousemove_callback_on_thread(
            EVENT_TARGET_WINDOW,
            app,
            true,
            Some(mouse_move),
            CALLBACK_THREAD_CALLING,
        );
        emscripten_set_mouseleave_callback_on_thread(
            CANVAS,
            app,
            true,
            Some(mouse_leave),
            CALLBACK_THREAD_CALLING,
        );
        emscripten_set_mouseup_callback_on_thread(
            EVENT_TARGET_WINDOW,
            app,
            true,
            Some(mouse_up),
            CALLBACK_THREAD_CALLING,
        );
        emscripten_set_blur_callback_on_thread(
            EVENT_TARGET_WINDOW,
            app,
            false,
            Some(blur),
            CALLBACK_THREAD_CALLING,
        );
        emscripten_set_keydown_callback_on_thread(
            EVENT_TARGET_WINDOW,
            app,
            true,
            Some(key_down),
            CALLBACK_THREAD_CALLING,
        );
        emscripten_set_resize_callback_on_thread(
            EVENT_TARGET_WINDOW,
            app,
            true,
            Some(resize),
            CALLBACK_THREAD_CALLING,
        );
        // Touch events stay on their start target by the DOM's own
        // contract — the canvas hears a pan to its end.
        let touch = [
            (
                "start",
                emscripten_set_touchstart_callback_on_thread(
                    CANVAS,
                    app,
                    true,
                    Some(touch_start),
                    CALLBACK_THREAD_CALLING,
                ),
            ),
            (
                "move",
                emscripten_set_touchmove_callback_on_thread(
                    CANVAS,
                    app,
                    true,
                    Some(touch_move),
                    CALLBACK_THREAD_CALLING,
                ),
            ),
            (
                "end",
                emscripten_set_touchend_callback_on_thread(
                    CANVAS,
                    app,
                    true,
                    Some(touch_end),
                    CALLBACK_THREAD_CALLING,
                ),
            ),
            (
                "cancel",
                emscripten_set_touchcancel_callback_on_thread(
                    CANVAS,
                    app,
                    true,
                    Some(touch_cancel),
                    CALLBACK_THREAD_CALLING,
                ),
            ),
        ];
        for (name, result) in touch {
            if result != 0 {
                eprintln!("[touch] registering {name} FAILED: {result}");
            }
        }
    }

    extern "C" fn animation_frame(time: c_double, user_data: *mut c_void) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            app.drain_worker();
            drain_host_commands(app, time / 1000.0);

            let size = app.state.viewport_size();
            let ticked = app.state.dispatch_timed(
                app.window,
                imba::event::Event::AnimationClock {
                    now: imba::anim::AnimationClock::from_millis(time),
                },
                size,
                time / 1000.0,
            );
            app.needs_redraw |= ticked;
            fling_frame(app, time / 1000.0);
            // Command performed → you render — and nothing performed,
            // nothing painted: the quiet 60fps loop stops burning a
            // core repainting the same pixels. The paint's own answer
            // re-arms the gate (its belts perform too).
            app.resize_canvas();
            if app.needs_redraw {
                app.needs_redraw = app.draw();
            }
            emscripten_request_animation_frame(Some(animation_frame), user_data);
        }
        true
    }

    unsafe fn drain_host_commands(app: &mut WebApp, timestamp: f64) {
        const POLL: *const c_char =
            b"(typeof Module !== 'undefined' && Module.himarkPoll) ? Module.himarkPoll() : ''\0"
                .as_ptr()
                .cast();
        loop {
            let raw = emscripten_run_script_string(POLL);
            if raw.is_null() {
                return;
            }
            let Ok(command) = CStr::from_ptr(raw).to_str() else {
                return;
            };
            if command.is_empty() {
                return;
            }
            app.needs_redraw = true;
            let mut fields = command.splitn(3, '\u{1}');
            match fields.next().unwrap_or_default() {
                "open" => {
                    let name = fields.next().unwrap_or("untitled.md").to_owned();
                    let source = fields.next().unwrap_or_default().to_owned();
                    app.state.open_async(
                        app.window,
                        name.clone(),
                        true,
                        None,
                        move |store, ui, theme| web_document(&name, &source, store, ui, theme),
                    );
                }
                "search" => {
                    let _ = app.state.perform_registered(app.window, "search.view");
                }
                "peeker" => {
                    let _ = app.state.perform_registered(app.window, "peeker.toggle");
                }
                "split" => {
                    let _ = app
                        .state
                        .perform_registered(app.window, "workbench.split-pane");
                }
                "demo" => {
                    app.state.open_async(
                        app.window,
                        "torture sample".to_owned(),
                        true,
                        None,
                        demo::monster_document,
                    );
                }
                "wall" => {
                    app.state.open_async(
                        app.window,
                        "wall of text".to_owned(),
                        true,
                        None,
                        demo::wall_of_text_document,
                    );
                }
                _ => {
                    let _ = timestamp;
                }
            }
        }
    }

    fn web_languages() -> editor::reparse::SyntaxLanguages {
        static LANGUAGES: std::sync::OnceLock<editor::reparse::SyntaxLanguages> =
            std::sync::OnceLock::new();
        LANGUAGES
            .get_or_init(|| {
                let mut languages = editor::reparse::SyntaxLanguages::new();
                hirust::register(&mut languages);
                hipython::register(&mut languages);
                hijavascript::register(&mut languages);
                hitypescript::register(&mut languages);
                higo::register(&mut languages);
                hijava::register(&mut languages);
                hic::register(&mut languages);
                hicpp::register(&mut languages);
                hicsharp::register(&mut languages);
                hiruby::register(&mut languages);
                hiphp::register(&mut languages);
                hibash::register(&mut languages);
                hilua::register(&mut languages);
                hiswift::register(&mut languages);
                hikotlin::register(&mut languages);
                hiscala::register(&mut languages);
                hihaskell::register(&mut languages);
                hielixir::register(&mut languages);
                hiocaml::register(&mut languages);
                hizig::register(&mut languages);
                hisql::register(&mut languages);
                hijson::register(&mut languages);
                hicss::register(&mut languages);
                hihtml::register(&mut languages);
                hiyaml::register(&mut languages);
                hitoml::register(&mut languages);
                hicmake::register(&mut languages);
                hid::register(&mut languages);
                hidart::register(&mut languages);
                hielm::register(&mut languages);
                hierlang::register(&mut languages);
                hifortran::register(&mut languages);
                hifsharp::register(&mut languages);
                higleam::register(&mut languages);
                higlsl::register(&mut languages);
                higraphql::register(&mut languages);
                higroovy::register(&mut languages);
                hihcl::register(&mut languages);
                hijulia::register(&mut languages);
                himake::register(&mut languages);
                hinix::register(&mut languages);
                hiobjc::register(&mut languages);
                hiodin::register(&mut languages);
                hiperl::register(&mut languages);
                hipowershell::register(&mut languages);
                hir::register(&mut languages);
                hisolidity::register(&mut languages);
                hixml::register(&mut languages);
                himarkdown::markdown_languages(languages)
            })
            .clone()
    }

    fn web_document(
        name: &str,
        source: &str,
        store: &imba::store::Store,
        ui: &imba::ui::UiCtx,
        theme: &editor::theme::Theme,
    ) -> editor::document::Document {
        let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
        if extension == "md" || extension == "markdown" {
            return himarkdown::document_from_markdown(source, store, ui, theme);
        }
        let languages = web_languages();
        if languages.knows(&extension) {
            return editor::document::Document::from_language(
                text::text::Text::from_string_exact(source),
                &extension,
                &languages,
                store,
                ui,
                theme,
            );
        }
        // No known language: PLAIN TEXT, never markdown.
        editor::document::Document::plain_text(text::text::Text::from_string_exact(source))
    }

    extern "C" fn wheel(
        _event_type: c_int,
        event: *const EmscriptenWheelEvent,
        user_data: *mut c_void,
    ) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            let event = &*event;

            let now_ms = event.mouse.timestamp;
            if app
                .last_scroll_ms
                .is_none_or(|last| now_ms - last > 250.0 || now_ms < last)
            {
                app.scroll_gesture.begin();
            }
            app.last_scroll_ms = Some(now_ms);
            let scale = app.scale as f64;
            let multiplier = match event.delta_mode {
                DOM_DELTA_LINE => 48.0,
                DOM_DELTA_PAGE => app.logical_height as f64,
                _ => 1.0,
            } * scale;
            let size = app.state.viewport_size();
            let changed = app.state.dispatch_timed(
                app.window,
                imba::event::Event::Scroll {
                    delta_x: (event.delta_x * multiplier) as f32,
                    point: gfx::Point::new(
                        event.mouse.target_x as f32 * app.scale,
                        event.mouse.target_y as f32 * app.scale,
                    ),
                    delta_y: (event.delta_y * multiplier) as f32,
                    gesture: &app.scroll_gesture,
                },
                size,
                event.mouse.timestamp / 1000.0,
            );
            app.needs_redraw |= changed;
            changed
        }
    }

    // ─── Touch: scroll by synthesis ──────────────────────────────────
    // Mobile browsers synthesize mouse events for TAPS only — a pan is
    // pure touchmove, which nothing here spoke. The pan becomes the
    // wheel road's own event: `Event::Scroll` deltas under the START
    // point, so the gesture-owner lock (`ScrollGesture::claims`) keeps
    // every delta — and the fling after the lift — on the surface the
    // finger first landed on, exactly the trackpad contract. Momentum
    // is the shell's to synthesize (the OS feeds it to wheels for
    // free): a velocity window at the lift, decayed per frame.

    struct TouchPan {
        id: c_int,
        start: gfx::Point,
        last: gfx::Point,
        /// Past the slop: the touch is a scroll, never a tap.
        panning: bool,
        /// The touch CAUGHT a running fling: it spends itself on the
        /// stop and never becomes a tap (the UIScrollView contract).
        caught: bool,
        /// Recent (seconds, dx, dy) — the lift's velocity window.
        samples: std::collections::VecDeque<(f64, f32, f32)>,
    }

    struct Fling {
        point: gfx::Point,
        velocity: (f32, f32),
        last: f64,
    }

    /// A tap stays a tap under this much finger wobble (CSS px).
    const PAN_SLOP: f32 = 8.0;
    /// The velocity window: deltas older than this don't throw.
    const PAN_WINDOW: f64 = 0.15;

    fn touch_point(app: &WebApp, touch: &EmscriptenTouchPoint) -> gfx::Point {
        gfx::Point::new(
            touch.target_x as f32 * app.scale,
            touch.target_y as f32 * app.scale,
        )
    }

    fn touches(event: &EmscriptenTouchEvent) -> &[EmscriptenTouchPoint] {
        &event.touches[..(event.num_touches.clamp(0, 32) as usize)]
    }

    extern "C" fn touch_start(
        _event_type: c_int,
        event: *const EmscriptenTouchEvent,
        user_data: *mut c_void,
    ) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            let event = &*event;
            // Any touch CATCHES a running fling.
            let caught = app.fling.take().is_some();
            // One finger pans; a second is ignored (pinch is future).
            if app.pan.is_none() {
                if let Some(touch) = touches(event).iter().find(|touch| touch.is_changed) {
                    let point = touch_point(app, touch);
                    app.pan = Some(TouchPan {
                        id: touch.identifier,
                        start: point,
                        last: point,
                        panning: false,
                        caught,
                        samples: std::collections::VecDeque::new(),
                    });
                }
            }
            // Never consumed: a tap must keep the browser's synthetic
            // mouse events — the click road already speaks them.
            false
        }
    }

    extern "C" fn touch_move(
        _event_type: c_int,
        event: *const EmscriptenTouchEvent,
        user_data: *mut c_void,
    ) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            let event = &*event;
            let Some(held) = app.pan.as_ref() else {
                return false;
            };
            let id = held.id;
            let Some(touch) = touches(event).iter().find(|touch| touch.identifier == id) else {
                return false;
            };
            let point = touch_point(app, touch);
            let now = event.timestamp / 1000.0;
            let (anchor, delta) = {
                let pan = app.pan.as_mut().expect("the pan just held");
                let delta = (pan.last.x - point.x, pan.last.y - point.y);
                pan.last = point;
                if !pan.panning {
                    let wobble =
                        ((point.x - pan.start.x).powi(2) + (point.y - pan.start.y).powi(2)).sqrt();
                    if wobble < PAN_SLOP * app.scale {
                        return false;
                    }
                    pan.panning = true;
                    app.scroll_gesture.begin();
                }
                pan.samples.push_back((now, delta.0, delta.1));
                while pan
                    .samples
                    .front()
                    .is_some_and(|(stamp, ..)| now - stamp > PAN_WINDOW)
                {
                    pan.samples.pop_front();
                }
                (pan.start, delta)
            };
            if delta != (0.0, 0.0) {
                let size = app.state.viewport_size();
                let changed = app.state.dispatch_timed(
                    app.window,
                    imba::event::Event::Scroll {
                        point: anchor,
                        delta_x: delta.0,
                        delta_y: delta.1,
                        gesture: &app.scroll_gesture,
                    },
                    size,
                    now,
                );
                app.needs_redraw |= changed;
            }
            // Consumed: past the slop the browser owes us no taps.
            true
        }
    }

    extern "C" fn touch_end(
        _event_type: c_int,
        event: *const EmscriptenTouchEvent,
        user_data: *mut c_void,
    ) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            let event = &*event;
            let Some(pan) = app.pan.as_ref() else {
                return false;
            };
            let lifted = touches(event)
                .iter()
                .any(|touch| touch.is_changed && touch.identifier == pan.id);
            if !lifted {
                return false;
            }
            let pan = app.pan.take().expect("the pan just held");
            if !pan.panning {
                // A catching tap spent itself stopping the fling; a
                // plain tap goes to the browser's mouse synthesis.
                return pan.caught;
            }
            let now = event.timestamp / 1000.0;
            let throw: Vec<_> = pan
                .samples
                .iter()
                .filter(|(stamp, ..)| now - stamp <= PAN_WINDOW)
                .collect();
            if let Some((oldest, ..)) = throw.first() {
                let spread = (now - oldest).max(1.0 / 60.0) as f32;
                let sum = throw.iter().fold((0.0f32, 0.0f32), |sum, (_, dx, dy)| {
                    (sum.0 + dx, sum.1 + dy)
                });
                let cap = 6000.0 * app.scale;
                let velocity = (
                    (sum.0 / spread).clamp(-cap, cap),
                    (sum.1 / spread).clamp(-cap, cap),
                );
                if (velocity.0.powi(2) + velocity.1.powi(2)).sqrt() > 100.0 * app.scale {
                    app.fling = Some(Fling {
                        point: pan.start,
                        velocity,
                        last: now,
                    });
                }
            }
            true
        }
    }

    extern "C" fn touch_cancel(
        _event_type: c_int,
        event: *const EmscriptenTouchEvent,
        user_data: *mut c_void,
    ) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            let event = &*event;
            if let Some(pan) = app.pan.as_ref() {
                let ours = touches(event)
                    .iter()
                    .any(|touch| touch.is_changed && touch.identifier == pan.id);
                if ours {
                    app.pan = None;
                }
            }
            false
        }
    }

    /// One frame of the fling: UIScrollView's normal deceleration
    /// (×0.998 per millisecond), dead when it slows to a crawl or the
    /// owning scroller stops moving (its edge).
    fn fling_frame(app: &mut WebApp, now: f64) {
        let Some(mut fling) = app.fling.take() else {
            return;
        };
        let dt = ((now - fling.last).clamp(0.0, 0.1)) as f32;
        fling.last = now;
        let delta = (fling.velocity.0 * dt, fling.velocity.1 * dt);
        let decay = 0.998f32.powf(dt * 1000.0);
        fling.velocity = (fling.velocity.0 * decay, fling.velocity.1 * decay);
        let size = app.state.viewport_size();
        let moved = app.state.dispatch_timed(
            app.window,
            imba::event::Event::Scroll {
                point: fling.point,
                delta_x: delta.0,
                delta_y: delta.1,
                gesture: &app.scroll_gesture,
            },
            size,
            now,
        );
        app.needs_redraw |= moved;
        let live = (fling.velocity.0.powi(2) + fling.velocity.1.powi(2)).sqrt() > 30.0 * app.scale;
        if moved && live {
            app.fling = Some(fling);
        }
    }

    extern "C" fn mouse_down(
        _event_type: c_int,
        event: *const EmscriptenMouseEvent,
        user_data: *mut c_void,
    ) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            let event = &*event;
            let point = gfx::Point::new(
                event.target_x as f32 * app.scale,
                event.target_y as f32 * app.scale,
            );
            let mods = imba::event::Modifiers {
                shift: event.shift_key,
                control: event.ctrl_key,
                alt: event.alt_key,
                command: event.meta_key,
            };
            // The secondary button skips the click run — a context
            // press never double-clicks.
            if event.button == 2 {
                let changed = app.state.dispatch_timed(
                    app.window,
                    imba::event::Event::MouseDown {
                        mods,
                        point,
                        button: imba::event::MouseButton::Right,
                        count: 1,
                    },
                    gfx::Size::new(app.width.max(1) as f32, app.height.max(1) as f32),
                    event.timestamp / 1000.0,
                );
                app.needs_redraw |= changed;
                return changed;
            }
            let Some(count) = app
                .clicks
                .count(event.timestamp, point.x, point.y, event.button)
            else {
                return false;
            };
            app.drag_point = Some(point);
            let changed = app.state.dispatch_timed(
                app.window,
                imba::event::Event::MouseDown {
                    mods,
                    point,
                    button: imba::event::MouseButton::Left,
                    count,
                },
                gfx::Size::new(app.width.max(1) as f32, app.height.max(1) as f32),
                event.timestamp / 1000.0,
            );
            app.needs_redraw |= changed;
            sync_keyboard(app);
            changed
        }
    }

    /// The canvas fills the viewport at (0, 0), so client coordinates
    /// remain canvas-relative even when a window listener receives input
    /// outside the canvas. Scale to the editor's physical pixels.
    fn window_mouse_point(app: &WebApp, event: &EmscriptenMouseEvent) -> gfx::Point {
        gfx::Point::new(
            event.client_x as f32 * app.scale,
            event.client_y as f32 * app.scale,
        )
    }

    fn end_drag(app: &mut WebApp, point: gfx::Point, timestamp: f64) -> bool {
        if app.drag_point.take().is_none() {
            return false;
        }
        let changed = app.state.dispatch_timed(
            app.window,
            imba::event::Event::MouseUp { point },
            app.state.viewport_size(),
            timestamp,
        );
        app.needs_redraw |= changed;
        sync_keyboard(app);
        changed
    }

    /// The soft keyboard follows the ENGINE's text focus — the same
    /// probe the iOS shell's `syncFirstResponder` rides: an IME seat
    /// in the focused frame means text is wanted. Ran at the tail of
    /// every input event, still inside the user gesture a keyboard
    /// raise needs; the page only flips the seat's inputmode, so the
    /// clipboard road never loses focus. (The mobile nuance — a tap
    /// in a focused editor should DISMISS, a repeat tap at the caret
    /// summon — wants imba to know it stands on a touch form; that
    /// enhancement is postponed, this probe is its transport.)
    fn sync_keyboard(app: &mut WebApp) {
        let wants = app
            .state
            .with_ime_client(
                himark::bridge::ViewBridgeId::from_raw(app.window.raw()),
                |_| (),
            )
            .is_some();
        if wants == app.wants_text {
            return;
        }
        app.wants_text = wants;
        let script = match wants {
            true => c"if (Module.himarkTextFocus) Module.himarkTextFocus(1);",
            false => c"if (Module.himarkTextFocus) Module.himarkTextFocus(0);",
        };
        unsafe { emscripten_run_script(script.as_ptr()) };
    }

    extern "C" fn mouse_move(
        _event_type: c_int,
        event: *const EmscriptenMouseEvent,
        user_data: *mut c_void,
    ) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            if app.drag_point.is_none() {
                return false;
            }
            let event = &*event;
            let point = window_mouse_point(app, event);
            let timestamp = event.timestamp / 1000.0;
            // Recover if the release happened outside the browser window.
            if event.buttons & 1 == 0 {
                return end_drag(app, point, timestamp);
            }
            app.drag_point = Some(point);
            let changed = app.state.dispatch_timed(
                app.window,
                imba::event::Event::MouseDrag {
                    point,
                    mods: imba::event::Modifiers {
                        shift: event.shift_key,
                        control: event.ctrl_key,
                        alt: event.alt_key,
                        command: event.meta_key,
                    },
                },
                app.state.viewport_size(),
                timestamp,
            );
            app.needs_redraw |= changed;
            changed
        }
    }

    extern "C" fn mouse_up(
        _event_type: c_int,
        event: *const EmscriptenMouseEvent,
        user_data: *mut c_void,
    ) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            let event = &*event;
            if event.button != 0 {
                return false;
            }
            let point = window_mouse_point(app, event);
            end_drag(app, point, event.timestamp / 1000.0)
        }
    }

    extern "C" fn mouse_leave(
        _event_type: c_int,
        _event: *const EmscriptenMouseEvent,
        user_data: *mut c_void,
    ) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            // A HitTest beyond any component's reach, or hover popups
            // stick to the last in-canvas point.
            let changed = app.state.dispatch(
                app.window,
                imba::event::Event::window_left(),
                app.state.viewport_size(),
            );
            app.needs_redraw |= changed;
            changed
        }
    }

    extern "C" fn blur(_event_type: c_int, _event: *const c_void, user_data: *mut c_void) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            app.clicks = crate::clicks::ClickCounter::default();
            if let Some(point) = app.drag_point {
                end_drag(app, point, 0.0);
            }
        }
        false
    }

    // ─── The text seat's engine half ─────────────────────────────────
    // The page keeps one hidden, always-focused textarea (index.html's
    // #input): the browser's clipboard commands, IME composition and
    // the mobile soft keyboard all address the focused editable, so
    // its DOM events are the app's whole text contract. Each handler
    // calls straight through these doors — synchronous, like the
    // emscripten key callbacks, so the input stream stays ordered.
    // One thread (the browser main thread) owns both sides, and a
    // door only ever runs between wasm turns; the tailnet callbacks
    // ride the same pattern. The doors are listed in the linkers'
    // EXPORTED_FUNCTIONS — MAIN_MODULE=2 strips them otherwise.

    /// The app the DOM-side text seat pokes — null until the app stands.
    static TEXT_SEAT: std::sync::atomic::AtomicPtr<WebApp> =
        std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

    unsafe fn seated_app<'a>() -> Option<&'a mut WebApp> {
        let app = TEXT_SEAT.load(std::sync::atomic::Ordering::Acquire);
        (!app.is_null()).then(|| &mut *app)
    }

    unsafe fn seated_text<'a>(text: *const c_char) -> Option<&'a str> {
        if text.is_null() {
            return None;
        }
        CStr::from_ptr(text).to_str().ok()
    }

    /// Copy or cut: the engine's clipboard seat answers with the text
    /// the page's copy/cut handler puts on `event.clipboardData`. The
    /// answer lives until the next take — the caller reads it before
    /// returning to the event loop.
    #[no_mangle]
    pub unsafe extern "C" fn himark_clipboard_take(cut: c_int) -> *const c_char {
        thread_local! {
            static TAKEN: std::cell::RefCell<std::ffi::CString> =
                std::cell::RefCell::new(std::ffi::CString::default());
        }
        let Some(app) = seated_app() else {
            return std::ptr::null();
        };
        let copied = app
            .state
            .with_clipboard_client(
                himark::bridge::ViewBridgeId::from_raw(app.window.raw()),
                |client| match cut != 0 {
                    true => client.cut(),
                    false => client.copy(),
                },
            )
            .flatten();
        if cut != 0 && copied.is_some() {
            app.needs_redraw = true;
        }
        let Some(content) = copied else {
            return std::ptr::null();
        };
        let Ok(text) = std::ffi::CString::new(content.text) else {
            return std::ptr::null();
        };
        TAKEN.with(|stash| {
            *stash.borrow_mut() = text;
            stash.borrow().as_ptr()
        })
    }

    #[no_mangle]
    pub unsafe extern "C" fn himark_clipboard_paste(text: *const c_char) {
        let Some(app) = seated_app() else { return };
        let Some(text) = seated_text(text).filter(|text| !text.is_empty()) else {
            return;
        };
        app.needs_redraw |= app
            .state
            .with_clipboard_client(
                himark::bridge::ViewBridgeId::from_raw(app.window.raw()),
                |client| {
                    client.paste(&imba::clipboard::ClipboardContent {
                        text: text.to_owned(),
                    })
                },
            )
            .unwrap_or(false);
    }

    /// Typed text the keydown road never saw — the soft keyboard's,
    /// a dead key's composed character.
    #[no_mangle]
    pub unsafe extern "C" fn himark_text_input(text: *const c_char) {
        let Some(app) = seated_app() else { return };
        let Some(text) = seated_text(text).filter(|text| !text.is_empty()) else {
            return;
        };
        let size = app.state.viewport_size();
        let typed = app
            .state
            .dispatch(app.window, imba::event::Event::TextInput { text }, size);
        app.needs_redraw |= typed;
    }

    /// The composition in flight mirrors into the engine's marked text.
    #[no_mangle]
    pub unsafe extern "C" fn himark_ime_set(text: *const c_char) {
        let Some(app) = seated_app() else { return };
        let Some(text) = seated_text(text) else {
            return;
        };
        let caret = text.encode_utf16().count() as u32;
        app.needs_redraw |= app
            .state
            .with_ime_client(
                himark::bridge::ViewBridgeId::from_raw(app.window.raw()),
                |client| {
                    client.set_marked_text(text, (caret, 0), None);
                },
            )
            .is_some();
    }

    /// Composition ended: commit through the marked-text door the
    /// native shells use, or unmark on a cancel. A commit with no IME
    /// client standing falls back to plain text input — the words
    /// must land somewhere.
    #[no_mangle]
    pub unsafe extern "C" fn himark_ime_commit(text: *const c_char) {
        let Some(app) = seated_app() else { return };
        let Some(text) = seated_text(text) else {
            return;
        };
        let committed = app.state.with_ime_client(
            himark::bridge::ViewBridgeId::from_raw(app.window.raw()),
            |client| match text.is_empty() {
                true => client.unmark_text(),
                false => client.insert_text(text, None),
            },
        );
        app.needs_redraw |= committed.is_some();
        if committed.is_none() && !text.is_empty() {
            let size = app.state.viewport_size();
            let typed =
                app.state
                    .dispatch(app.window, imba::event::Event::TextInput { text }, size);
            app.needs_redraw |= typed;
        }
    }

    extern "C" fn key_down(
        _event_type: c_int,
        event: *const EmscriptenKeyboardEvent,
        user_data: *mut c_void,
    ) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            let event = &*event;
            let timestamp = event.timestamp / 1000.0;
            let mods = imba::event::Modifiers {
                shift: event.shift_key,
                control: event.ctrl_key,
                alt: event.alt_key,
                command: event.meta_key,
            };
            let size = app.state.viewport_size();

            // A composing input method owns the keyboard — the same
            // gate every shell keeps. 229 is the processing keyCode
            // IMEs ride; "Process" its key-value spelling.
            if event.key_code == 229 || cstr(&event.key).to_bytes() == b"Process" {
                return false;
            }

            // Clipboard chords are LEFT to the browser: its copy, cut
            // and paste commands land as DOM events on the text seat
            // (index.html's #input), where the engine's clipboard
            // seat answers — the one road desktop keyboards, mobile
            // Edit menus and ⌘-chords all share.
            if (event.meta_key || event.ctrl_key) && !event.alt_key {
                if let b"c" | b"x" | b"v" = cstr(&event.key).to_bytes() {
                    return false;
                }
            }

            let mut key_down = |key: imba::event::Key| {
                app.state.dispatch_timed(
                    app.window,
                    imba::event::Event::KeyDown { key, mods },
                    size,
                    timestamp,
                )
            };
            let consumed = match cstr(&event.key).to_str().unwrap_or_default() {
                "Backspace" => key_down(imba::event::Key::Backspace),
                "ArrowLeft" => key_down(imba::event::Key::Left),
                "ArrowRight" => key_down(imba::event::Key::Right),
                "ArrowUp" => key_down(imba::event::Key::Up),
                "ArrowDown" => key_down(imba::event::Key::Down),
                "Escape" => key_down(imba::event::Key::Escape),
                "Enter" => key_down(imba::event::Key::Enter),
                "Tab" => app.state.dispatch_timed(
                    app.window,
                    imba::event::Event::TextInput { text: "\t" },
                    size,
                    timestamp,
                ),
                "Home" => key_down(imba::event::Key::Home),
                "End" => key_down(imba::event::Key::End),
                "PageUp" => key_down(imba::event::Key::PageUp),
                "PageDown" => key_down(imba::event::Key::PageDown),
                "Delete" => key_down(imba::event::Key::Delete),

                key if (event.ctrl_key || event.meta_key || event.alt_key)
                    && is_typed_character(key) =>
                {
                    match key.chars().next() {
                        Some(letter) => {
                            key_down(imba::event::Key::Char(letter.to_ascii_lowercase()))
                        }
                        None => false,
                    }
                }
                key if is_typed_character(key) => app.state.dispatch_timed(
                    app.window,
                    imba::event::Event::TextInput { text: key },
                    size,
                    timestamp,
                ),
                _ => false,
            };
            app.needs_redraw |= consumed;
            sync_keyboard(app);
            consumed
        }
    }

    extern "C" fn resize(
        _event_type: c_int,
        _event: *const EmscriptenUiEvent,
        user_data: *mut c_void,
    ) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            app.resize_canvas();
        }
        false
    }

    fn is_typed_character(key: &str) -> bool {
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => !ch.is_control() && ch != '\u{7f}',
            _ => false,
        }
    }

    fn cstr(bytes: &[c_char; 32]) -> &CStr {
        unsafe { CStr::from_ptr(bytes.as_ptr()) }
    }
}

#[cfg(target_os = "emscripten")]
fn main() {
    app::main();
}
