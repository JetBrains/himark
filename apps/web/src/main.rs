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

#[cfg(target_os = "emscripten")]
mod app {
    use std::{
        ffi::CStr,
        os::raw::{c_char, c_double, c_int, c_void},
    };

    use himark::AppExt;
    use himark::{AppFonts, Application};
    use skia_safe::{
        gpu::{
            self, backend_render_targets, direct_contexts, gl, interfaces, surfaces, DirectContext,
            SurfaceOrigin,
        },
        ColorType, FontMgr, Size,
    };

    const CANVAS: *const c_char = b"#canvas\0".as_ptr().cast();
    const FONT_URL: *const c_char = b"JetBrainsMono-Regular.woff2\0".as_ptr().cast();
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
        fn emscripten_async_wget_data(
            url: *const c_char,
            user_data: *mut c_void,
            on_load: Option<extern "C" fn(*mut c_void, *mut c_void, c_int)>,
            on_error: Option<extern "C" fn(*mut c_void)>,
        );
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
        fn emscripten_set_mousemove_callback_on_thread(
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
            let context = Box::into_raw(Box::new(context)).cast::<c_void>();
            emscripten_async_wget_data(
                FONT_URL,
                context,
                Some(font_loaded),
                Some(font_load_failed),
            );
        }
    }

    extern "C" fn font_loaded(user_data: *mut c_void, data: *mut c_void, size: c_int) {
        let context = *unsafe { Box::from_raw(user_data.cast::<DirectContext>()) };
        assert!(
            !data.is_null() && size > 0,
            "the web font response was empty"
        );
        let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size as usize) };
        let typeface = FontMgr::new()
            .new_from_data(bytes, None)
            .expect("failed to load JetBrains Mono WOFF2");
        himark::embedded_fonts::install(typeface);

        let app = Box::into_raw(Box::new(WebApp::new(context))).cast::<c_void>();
        unsafe {
            install_callbacks(app);
            emscripten_request_animation_frame(Some(animation_frame), app);
        }
    }

    extern "C" fn font_load_failed(user_data: *mut c_void) {
        unsafe {
            drop(Box::from_raw(user_data.cast::<DirectContext>()));
        }
        panic!("failed to fetch JetBrains Mono WOFF2");
    }

    struct WebApp {
        state: Application,

        scroll_gesture: imba::event::ScrollGesture,
        last_scroll_ms: Option<f64>,
        clicks: crate::clicks::ClickCounter,
        /// Only presses in this canvas can start a selection drag.
        drag_point: Option<skia_safe::Point>,
        /// The one browser canvas is one himark window.
        window: himark::WindowId,
        context: DirectContext,

        #[cfg(target_feature = "atomics")]
        arriving: std::sync::mpsc::Receiver<himark::AppCommand>,
        width: i32,
        height: i32,
        logical_width: f32,
        logical_height: f32,
        scale: f32,
    }

    impl WebApp {
        fn new(context: DirectContext) -> Self {
            let mut state = Application::new(app_fonts());
            state.register_syntax_languages(web_languages());

            himarkdown::register_handlers(&mut state);
            state.register_command(std::sync::Arc::new(palette::TogglePalette));
            state.register_command(std::sync::Arc::new(peeker::TogglePeeker));
            state.register_command(std::sync::Arc::new(himark::hifiles::ToggleSessionSwitcher));
            state.register_command(std::sync::Arc::new(hidiff::OpenDiff));
            state.register_row_minter(hidiff::row_minter());
            state.register_sync_observer(hidiff::canvas_sync_observer());
            state.register_session_family(hidiff::canvases_session_family());
            state.register_navigator(hidiff::CanvasNavigator);
            state.register_command(std::sync::Arc::new(demo::OpenTreeDemo));

            state.register_overlay_surface(peeker::overlay_surface());
            state.register_overlay_surface(palette::overlay_surface());

            let change_refs = himark::hichanges::ChangeRefs::default();
            himark::hichanges::Changes::install(&mut state.store_mut(), change_refs.clone());
            himark::hicomments::Comments::install(&mut state.store_mut());
            himark::OpenDocuments::install_hook(
                &mut state.store_mut(),
                std::sync::Arc::new(himark::hicomments::CommentsHook),
            );
            state.register_command(std::sync::Arc::new(himark::hicomments::ToggleCommentsView));
            state.register_toolbar_button(himark::hicomments::toolbar_button());
            state.register_command(std::sync::Arc::new(himark::higent::ToggleAgentsView));
            state.register_command(std::sync::Arc::new(himark::higent::NewChat));
            state.register_toolbar_button(himark::higent::toolbar_button());
            state.register_command(std::sync::Arc::new(himark::hichanges::ToggleChangesView));
            state.register_command(std::sync::Arc::new(
                himark::hichanges::RefetchChanges::default(),
            ));
            state.register_toolbar_button(himark::hichanges::toolbar_button());
            state.register_command(std::sync::Arc::new(himark::hihistory::ToggleHistoryView));
            state.register_toolbar_button(himark::hihistory::toolbar_button());
            state.register_command(std::sync::Arc::new(himark::hifiles::ToggleSessionTree));
            state.register_toolbar_button(himark::hifiles::toolbar_button());

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
                if !url.is_empty() {
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
                    hiahp::registry::register_all(&mut state);

                    let seat: std::sync::Arc<dyn himark::higent::AhpServer> =
                        std::sync::Arc::new(hiahp::wire::WireHost::at(
                            handle.clone(),
                            std::sync::Arc::new(crate::browser::BrowserConnector),
                            url,
                        ));
                    let host = state.register_seat(std::sync::Arc::clone(&seat));
                    himark::higent::Agents::seed(&mut state.store_mut(), host, "This Host");
                    himark::higent::Hosts::install_uris(
                        &mut state.store_mut(),
                        host,
                        std::sync::Arc::new(hiahp::uris::FileUris),
                    );
                    state.designate_local_host(host);

                    use std::sync::Arc;
                    let seats = Arc::new(hiahp::fs::SeatDirectory::new({
                        let posted = posted.clone();
                        Arc::new(move |subscription| {
                            let _ = posted.send(himark::AppCommand::FileChanged(
                                himark::Subscription(subscription),
                            ));
                        })
                    }));
                    seats.record(host, seat);
                    seats.set_local(host);
                    let resource_uris: Arc<dyn himark::higent::ResourceUriMap> =
                        Arc::new(hiahp::uris::FileUris);

                    let document_channels = hiahp::docsync::DocumentChannels::new(
                        handle.clone(),
                        Arc::new({
                            let posted = posted.clone();
                            move |command| {
                                let _ = posted.send(command);
                            }
                        }),
                        Arc::clone(&resource_uris),
                    );
                    himark::OpenDocuments::install_hook(
                        &mut state.store_mut(),
                        Arc::new(hiahp::docsync::DocsyncHook {
                            channels: Arc::clone(&document_channels),
                            directory: Arc::clone(&seats),
                        }),
                    );

                    himark::InstalledChangeSink::install(
                        &mut state.store_mut(),
                        Arc::new(hiahp::docsync::SyncSink),
                    );
                    state.register_handler::<himark::FetchDocumentEffect>(
                        hiahp::fsroute::RouteFetch {
                            uris: Arc::clone(&resource_uris),
                            directory: Arc::clone(&seats),
                        },
                    );
                    state.register_handler::<himark::StoreDocumentEffect>(
                        hiahp::fsroute::RouteStore {
                            directory: Arc::clone(&seats),
                            uris: Arc::clone(&resource_uris),
                            channels: Arc::clone(&document_channels),
                        },
                    );
                    state.register_editor_command(Arc::new(himark::SaveDocument::existing_files()));
                    state.register_handler::<himark::ListDirectoryEffect>(
                        hiahp::fsroute::RouteList {
                            directory: Arc::clone(&seats),
                            uris: Arc::clone(&resource_uris),
                        },
                    );
                    state.register_handler::<himark::SubscribeEffect>(
                        hiahp::fsroute::RouteSubscribe {
                            directory: Arc::clone(&seats),
                            uris: Arc::clone(&resource_uris),
                        },
                    );
                    state.register_handler::<himark::UnsubscribeEffect>(
                        hiahp::fsroute::RouteUnsubscribe {
                            directory: Arc::clone(&seats),
                        },
                    );
                    state.observe_file_changes();

                    state.register_handler::<himark::FindEffect>(hiahp::find::NativeFindHandler {
                        directory: Arc::clone(&seats),
                    });

                    state.register_handler::<himark::LspCompletionEffect>(
                        hiahp::lsproute::CompletionRoute {
                            directory: Arc::clone(&seats),
                            uris: Arc::clone(&resource_uris),
                        },
                    );

                    state.register_handler::<himark::SearchLocationsEffect>(
                        hiahp::locations::RouteSearchLocations {
                            directory: Arc::clone(&seats),
                        },
                    );
                    state.register_command(Arc::new(himark::hisearch::ToggleSearchView));
                    state.register_command(Arc::new(himark::hisearch::FocusSearchView));
                    state.register_toolbar_button(himark::hisearch::toolbar_button());
                    state.register_handler::<himark::LspLocationsEffect>(
                        hiahp::locations::RouteLspLocations {
                            directory: Arc::clone(&seats),
                            uris: Arc::clone(&resource_uris),
                        },
                    );

                    state.register_handler::<himark::FetchBaseEffect>(hiahp::fsroute::RouteBase {
                        refs: change_refs.clone(),
                    });
                    state.observe_stripe_bases();

                    hiahp::open::install_open_handlers(
                        &mut state,
                        Arc::new(web_languages()),
                        Arc::new(myersdiff::Myers),
                    );
                }
            }
            let window = state.add_window();

            state.perform_command(himark::AppCommand::Dynamic(
                window,
                std::sync::Arc::new(himark::new_session::OpenNewSession { host: None }),
            ));

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
                let _ = self.state.perform_batch(vec![command]);
            }
        }

        #[cfg(not(target_feature = "atomics"))]
        fn drain_worker(&mut self) {}

        fn draw(&mut self) {
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

            let canvas = surface.canvas();
            canvas.save();
            himark::Window::draw_with_size(
                self.window,
                &mut self.state,
                canvas,
                Size::new(self.width.max(1) as f32, self.height.max(1) as f32),
            );
            canvas.restore();

            self.context.flush_and_submit_surface(&mut surface, None);
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
            }
            self.scale = scale;
        }
    }

    fn app_fonts() -> AppFonts {
        AppFonts::new(himark::fonts::source())
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
    }

    extern "C" fn animation_frame(time: c_double, user_data: *mut c_void) -> bool {
        unsafe {
            let app = &mut *user_data.cast::<WebApp>();
            app.drain_worker();
            drain_host_commands(app, time / 1000.0);

            let size = app.state.viewport_size();
            let _ = app.state.dispatch_timed(
                app.window,
                imba::event::Event::AnimationClock {
                    now: imba::anim::AnimationClock::from_millis(time),
                },
                size,
                time / 1000.0,
            );
            app.draw();
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
                        move |store, ui, fonts, theme| {
                            web_document(&name, &source, store, ui, fonts, theme)
                        },
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

    fn web_languages() -> himark::SyntaxLanguages {
        static LANGUAGES: std::sync::OnceLock<himark::SyntaxLanguages> = std::sync::OnceLock::new();
        LANGUAGES
            .get_or_init(|| {
                let mut languages = himark::SyntaxLanguages::new();
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
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) -> himark::Document {
        let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
        if extension != "md" && extension != "markdown" {
            let languages = web_languages();
            if languages.knows(&extension) {
                return himark::Document::from_language(
                    himark::Text::from_string_exact(source),
                    &extension,
                    &languages,
                    store,
                    ui,
                    fonts,
                    theme,
                );
            }
        }
        himarkdown::document_from_markdown(source, store, ui, fonts, theme)
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
            app.state.dispatch_timed(
                app.window,
                imba::event::Event::Scroll {
                    delta_x: (event.delta_x * multiplier) as f32,
                    point: skia_safe::Point::new(
                        event.mouse.target_x as f32 * app.scale,
                        event.mouse.target_y as f32 * app.scale,
                    ),
                    delta_y: (event.delta_y * multiplier) as f32,
                    gesture: &app.scroll_gesture,
                },
                size,
                event.mouse.timestamp / 1000.0,
            )
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
            let point = skia_safe::Point::new(
                event.target_x as f32 * app.scale,
                event.target_y as f32 * app.scale,
            );
            let Some(count) = app
                .clicks
                .count(event.timestamp, point.x, point.y, event.button)
            else {
                return false;
            };
            app.drag_point = Some(point);
            app.state.dispatch_timed(
                app.window,
                imba::event::Event::MouseDown {
                    mods: imba::event::Modifiers {
                        shift: event.shift_key,
                        control: event.ctrl_key,
                        alt: event.alt_key,
                        command: event.meta_key,
                    },
                    point,
                    button: imba::event::MouseButton::Left,
                    count,
                },
                skia_safe::Size::new(app.width.max(1) as f32, app.height.max(1) as f32),
                event.timestamp / 1000.0,
            )
        }
    }

    /// The canvas fills the viewport at (0, 0), so client coordinates
    /// remain canvas-relative even when a window listener receives input
    /// outside the canvas. Scale to the editor's physical pixels.
    fn window_mouse_point(app: &WebApp, event: &EmscriptenMouseEvent) -> skia_safe::Point {
        skia_safe::Point::new(
            event.client_x as f32 * app.scale,
            event.client_y as f32 * app.scale,
        )
    }

    fn end_drag(app: &mut WebApp, point: skia_safe::Point, timestamp: f64) -> bool {
        if app.drag_point.take().is_none() {
            return false;
        }
        app.state.dispatch_timed(
            app.window,
            imba::event::Event::MouseUp { point },
            app.state.viewport_size(),
            timestamp,
        )
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
            app.state.dispatch_timed(
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
            )
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
            let mut key_down = |key: imba::event::Key| {
                app.state.dispatch_timed(
                    app.window,
                    imba::event::Event::KeyDown { key, mods },
                    size,
                    timestamp,
                )
            };
            match cstr(&event.key).to_str().unwrap_or_default() {
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
            }
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
