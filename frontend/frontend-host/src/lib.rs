// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::VecDeque;
use std::ffi::{c_char, c_void};
use std::sync::{Arc, Mutex};

pub use himark::terminal::TerminalBackend;
pub use himark::AppFonts;
pub use host::{HimarkHostCallbacks, HimarkLocation, HimarkStr};

pub use hiahp;
use hiahp::{docsync, find, fsroute};
mod host;
mod lsproute;
use hiahp::uris;

use himark::{AppCommand, AppExt, Application, BackgroundRunner};
use imba::anim::AnimationClock;
use imba::event::{Event, Key, MouseButton};
use skia_safe::{Canvas, Point, Size};

type WakeCallback = Box<dyn Fn() + Send + 'static>;

fn wid(window: u64) -> himark::WindowId {
    himark::WindowId::from_raw(window)
}

#[derive(Clone, Copy, Default)]
struct AgentHostFilesystemCapabilities {
    fetch_document: bool,
    store_document: bool,
    list_directory: bool,
}

impl AgentHostFilesystemCapabilities {
    const fn all() -> Self {
        Self {
            fetch_document: true,
            store_document: true,
            list_directory: true,
        }
    }

    fn include(&mut self, other: Self) {
        self.fetch_document |= other.fetch_document;
        self.store_document |= other.store_document;
        self.list_directory |= other.list_directory;
    }
}

pub struct HimarkEngine {
    app: Application,

    scroll_gesture: imba::event::ScrollGesture,
    last_scroll: std::cell::Cell<Option<std::time::Instant>>,

    compose_new_windows: bool,

    host: Option<Arc<host::HostBridge>>,

    agent_host_filesystem: AgentHostFilesystemCapabilities,

    seats: Arc<hiahp::fs::SeatDirectory>,

    shared: Arc<Shared>,

    change_refs: himark::hichanges::ChangeRefs,

    _document_channels: Arc<docsync::DocumentChannels>,

    drain_chunk: usize,
    drain_budget: std::time::Duration,

    resource_uris: Arc<dyn himark::higent::ResourceUriMap>,

    clicks: ClickCounter,

    /// The cut text between the host's two sized-string calls
    /// (`himark_cut`): the sizing probe performs the cut — it deletes
    /// the selection, so it must run exactly once — and the fill call
    /// drains this stash.
    pending_cut: Option<String>,

    runtime: tokio::runtime::Runtime,
}

#[derive(Default)]
struct ClickCounter {
    last: Option<std::time::Instant>,
    point: (f32, f32),
    count: u8,
}

impl ClickCounter {
    const SLACK: f32 = 6.0;

    fn count(&mut self, x: f32, y: f32) -> u8 {
        let now = std::time::Instant::now();
        let run = self
            .last
            .is_some_and(|last| now.duration_since(last).as_millis() < 500)
            && (x - self.point.0).abs() <= Self::SLACK
            && (y - self.point.1).abs() <= Self::SLACK;
        self.count = if run { (self.count + 1).min(3) } else { 1 };
        self.last = Some(now);
        self.point = (x, y);
        self.count
    }
}

fn register_agent_server(
    app: &mut Application,
    seats: &hiahp::fs::SeatDirectory,
    name: &str,
    seat: Arc<dyn himark::higent::AhpServer>,
) -> himark::higent::HostId {
    let id = app.register_seat(Arc::clone(&seat));
    himark::higent::Agents::seed(&mut app.store_mut(), id, name);

    himark::higent::Hosts::install_uris(&mut app.store_mut(), id, Arc::new(uris::FileUris));
    seats.record(id, seat);
    id
}

fn demo_location(name: &str) -> himark::ResourceLocation {
    himark::ResourceLocation::new(
        himark::ResourceType::document(),
        himark::Authority::new("demo"),
        vec![name.to_owned()],
    )
}

#[derive(Clone)]
pub struct HimarkWorker {
    shared: Arc<Shared>,
}

struct Shared {
    inbox: Arc<Mutex<VecDeque<AppCommand>>>,

    wake: Arc<WakeSlot>,

    effect_wake: Arc<WakeSlot>,

    runner: BackgroundRunner,
}

#[derive(Default)]
struct WakeSlot {
    callback: Mutex<Option<WakeCallback>>,
}

#[derive(Clone, Copy)]
struct Wake {
    callback: extern "C" fn(*mut c_void),
    context: *mut c_void,
}

unsafe impl Send for Wake {}

impl Wake {
    fn fire(self) {
        (self.callback)(self.context);
    }
}

impl WakeSlot {
    fn fire(&self) {
        if let Some(wake) = self.callback.lock().expect("wake slot").as_ref() {
            wake();
        }
    }
    fn set(&self, callback: impl Fn() + Send + 'static) {
        *self.callback.lock().expect("wake slot") = Some(Box::new(callback));
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HimarkRange {
    pub start: u32,

    pub length: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HimarkRect {
    pub x: f32,

    pub y: f32,

    pub width: f32,

    pub height: f32,
}

pub const HIMARK_KEY_BACKSPACE: u32 = 1;

pub const HIMARK_KEY_ENTER: u32 = 2;

pub const HIMARK_KEY_LEFT: u32 = 3;

pub const HIMARK_KEY_RIGHT: u32 = 4;

pub const HIMARK_KEY_UP: u32 = 5;

pub const HIMARK_KEY_DOWN: u32 = 6;

pub const HIMARK_KEY_ESCAPE: u32 = 7;

pub const HIMARK_KEY_TAB: u32 = 8;

pub const HIMARK_KEY_HOME: u32 = 9;

pub const HIMARK_KEY_END: u32 = 10;

pub const HIMARK_KEY_PAGE_UP: u32 = 11;

pub const HIMARK_KEY_PAGE_DOWN: u32 = 12;

pub const HIMARK_KEY_DELETE: u32 = 13;

pub const HIMARK_KEY_F1: u32 = 20;

pub const HIMARK_KEY_CHAR_BASE: u32 = 0x20;

pub const HIMARK_MOD_SHIFT: u32 = 1;

pub const HIMARK_MOD_CONTROL: u32 = 2;

pub const HIMARK_MOD_ALT: u32 = 4;

pub const HIMARK_MOD_COMMAND: u32 = 8;

fn map_key(code: u32) -> Option<Key> {
    Some(match code {
        HIMARK_KEY_BACKSPACE => Key::Backspace,
        HIMARK_KEY_ENTER => Key::Enter,
        HIMARK_KEY_LEFT => Key::Left,
        HIMARK_KEY_RIGHT => Key::Right,
        HIMARK_KEY_UP => Key::Up,
        HIMARK_KEY_DOWN => Key::Down,
        HIMARK_KEY_ESCAPE => Key::Escape,
        HIMARK_KEY_TAB => Key::Tab,
        HIMARK_KEY_HOME => Key::Home,
        HIMARK_KEY_END => Key::End,
        HIMARK_KEY_PAGE_UP => Key::PageUp,
        HIMARK_KEY_PAGE_DOWN => Key::PageDown,
        HIMARK_KEY_DELETE => Key::Delete,
        code if (HIMARK_KEY_F1..HIMARK_KEY_F1 + 12).contains(&code) => {
            Key::F((code - HIMARK_KEY_F1 + 1) as u8)
        }
        code if code >= HIMARK_KEY_CHAR_BASE => Key::Char(char::from_u32(code)?),
        _ => return None,
    })
}

fn map_mods(mods: u32) -> imba::event::Modifiers {
    imba::event::Modifiers {
        shift: mods & HIMARK_MOD_SHIFT != 0,
        control: mods & HIMARK_MOD_CONTROL != 0,
        alt: mods & HIMARK_MOD_ALT != 0,
        command: mods & HIMARK_MOD_COMMAND != 0,
    }
}

fn app_fonts() -> AppFonts {
    AppFonts::platform()
}

fn enrichment_passes() -> himark::Enrichers {
    let mut enrichers = himarkdown::markdown_enrichers(himark::Enrichers::new());
    himermaid::register_enricher(&mut enrichers);

    hisitter::register_caret_enrichers(&mut enrichers);
    enrichers
}

fn syntax_languages() -> himark::SyntaxLanguages {
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
            himermaid::register(&mut languages);
            himarkdown::markdown_languages(languages)
        })
        .clone()
}

impl Default for HimarkEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl HimarkEngine {
    pub fn new() -> Self {
        let mut engine = Self::with_fonts(app_fonts());

        engine.compose_new_windows = true;
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            let _ = host_discovery::autostart();
            engine.install_agent_host_filesystem(AgentHostFilesystemCapabilities::all());
        }
        engine
    }

    pub fn with_fonts(fonts: AppFonts) -> Self {
        let mut app = Application::new(fonts);
        app.register_syntax_languages(syntax_languages());
        app.register_diff_policy(Arc::new(structdiff::Structural::new(Arc::new(
            syntax_languages(),
        ))));
        app.register_enrichers(enrichment_passes());
        search::register_handlers(&mut app);

        app.register_command(Arc::new(search::OpenSearch));
        app.register_command(Arc::new(palette::TogglePalette));
        app.register_command(Arc::new(peeker::TogglePeeker));

        app.register_overlay_surface(peeker::overlay_surface());
        app.register_overlay_surface(palette::overlay_surface());
        app.register_overlay_surface(search::overlay_surface());

        app.register_command(Arc::new(himark::hifiles::ToggleSessionSwitcher));
        app.register_command(Arc::new(hidiff::OpenDiff));
        app.register_row_minter(hidiff::row_minter());
        app.register_sync_observer(hidiff::canvas_sync_observer());
        app.register_navigator(hidiff::CanvasNavigator);
        app.register_command(Arc::new(demo::OpenTreeDemo));

        app.register_editor_command(Arc::new(himark::hicomments::AddComment));

        himarkdown::register_handlers(&mut app);
        app.register_editor_command(Arc::new(himarkdown::InsertTable));

        hiahp::registry::register_all(&mut app);

        let resource_uris: Arc<dyn himark::higent::ResourceUriMap> = Arc::new(uris::FileUris);
        let change_refs = himark::hichanges::ChangeRefs::default();
        himark::hichanges::Changes::install(&mut app.store_mut(), change_refs.clone());
        himark::hicomments::Comments::install(&mut app.store_mut());
        himark::OpenDocuments::install_hook(
            &mut app.store_mut(),
            Arc::new(himark::hicomments::CommentsHook),
        );
        app.register_command(Arc::new(himark::hicomments::ToggleCommentsView));
        app.register_toolbar_button(himark::hicomments::toolbar_button());

        app.register_command(Arc::new(himark::higent::ToggleAgentsView));
        app.register_command(Arc::new(himark::higent::NewChat));
        app.register_toolbar_button(himark::higent::toolbar_button());

        app.register_toolbar_button(himark::composer_button());

        if let Ok(value) = std::env::var("HIMARK_FLOATING_CHAT") {
            himark::FloatingChat::set(&mut app.store_mut(), value != "0");
        }
        let inbox: Arc<Mutex<VecDeque<AppCommand>>> = Arc::new(Mutex::new(VecDeque::new()));
        let wake = Arc::new(WakeSlot::default());
        let effect_wake = Arc::new(WakeSlot::default());

        let dispatcher = {
            let inbox = inbox.clone();
            let wake = wake.clone();
            Arc::new(move |command: AppCommand| {
                inbox.lock().expect("inbox").push_back(command);
                wake.fire();
            })
        };

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("himark-runtime")
            .build()
            .expect("the engine runtime");

        let document_channels = docsync::DocumentChannels::new(
            runtime.handle().clone(),
            dispatcher.clone(),
            Arc::clone(&resource_uris),
        );

        // The docsync hook installs after the seat directory exists (below).

        let scheduler = {
            let effect_wake = effect_wake.clone();
            Arc::new(move || effect_wake.fire())
        };
        let runner = app.attach_host(dispatcher, scheduler);

        let seats = Arc::new(hiahp::fs::SeatDirectory::new({
            let inbox = inbox.clone();
            let wake = wake.clone();
            Arc::new(move |subscription| {
                inbox
                    .lock()
                    .expect("inbox")
                    .push_back(AppCommand::FileChanged(himark::Subscription(subscription)));
                wake.fire();
            })
        }));

        let connector: Arc<dyn hiahp::transport::Connector> = Arc::new(desktop::DesktopConnector);

        register_agent_server(
            &mut app,
            &seats,
            "VS Code Agent Host",
            Arc::new(hiahp::wire::WireHost::new(
                runtime.handle().clone(),
                Arc::clone(&connector),
            )),
        );

        let local_backend = register_agent_server(
            &mut app,
            &seats,
            "himark Agent Host",
            Arc::new(hiahp::wire::WireHost::himark_host(
                runtime.handle().clone(),
                Arc::clone(&connector),
            )),
        );
        seats.set_local(local_backend);
        app.designate_local_host(local_backend);

        {
            let seats = seats.clone();
            let handle = runtime.handle().clone();
            let connector = Arc::clone(&connector);
            himark::higent::Agents::install_add_host(
                &mut app.store_mut(),
                Arc::new(move |app, store, url| {
                    let seat: Arc<dyn himark::higent::AhpServer> =
                        Arc::new(hiahp::wire::WireHost::at(
                            handle.clone(),
                            Arc::clone(&connector),
                            url.to_owned(),
                        ));
                    let id = app.register_seat(Arc::clone(&seat));
                    himark::higent::Agents::seed(store, id, url.trim());
                    himark::higent::Hosts::install_uris(store, id, Arc::new(uris::FileUris));
                    seats.record(id, seat);
                    Some(id)
                }),
            );
        }

        let refresh: Arc<
            dyn Fn(himark::WindowId, Arc<std::sync::atomic::AtomicBool>) + Send + Sync,
        > = {
            let inbox = inbox.clone();
            let wake = wake.clone();
            Arc::new(move |window, flag| {
                inbox.lock().expect("inbox").push_back(AppCommand::Dynamic(
                    window,
                    Arc::new(host::RefreshTerminal(flag)),
                ));
                wake.fire();
            })
        };
        app.register_handler::<host::NewTerminalEffect>(host::SessionTerminalHandler { refresh });
        app.register_command(Arc::new(host::OpenTerminal));
        himark::OpenDocuments::install_hook(
            &mut app.store_mut(),
            Arc::new(docsync::DocsyncHook {
                channels: Arc::clone(&document_channels),
                directory: Arc::clone(&seats),
            }),
        );
        app.register_handler::<himark::FetchDocumentEffect>(fsroute::RouteFetch {
            uris: Arc::clone(&resource_uris),
            directory: Arc::clone(&seats),
        });
        app.register_handler::<himark::FetchResourceBytesEffect>(fsroute::RouteFetchBytes {
            directory: Arc::clone(&seats),
            uris: Arc::clone(&resource_uris),
        });
        app.register_handler::<himark::StoreDocumentEffect>(fsroute::RouteStore {
            directory: Arc::clone(&seats),
            uris: Arc::clone(&resource_uris),
            channels: Arc::clone(&document_channels),
        });
        app.register_handler::<himark::ListDirectoryEffect>(fsroute::RouteList {
            directory: Arc::clone(&seats),
            uris: Arc::clone(&resource_uris),
        });
        app.register_handler::<himark::SubscribeEffect>(fsroute::RouteSubscribe {
            directory: Arc::clone(&seats),
            uris: Arc::clone(&resource_uris),
        });
        app.register_handler::<himark::UnsubscribeEffect>(fsroute::RouteUnsubscribe {
            directory: Arc::clone(&seats),
        });
        app.observe_file_changes();

        Self {
            app,
            compose_new_windows: false,
            scroll_gesture: imba::event::ScrollGesture::default(),
            last_scroll: std::cell::Cell::new(None),
            drain_chunk: Self::DRAIN_CHUNK,
            drain_budget: Self::DRAIN_BUDGET,
            clicks: ClickCounter::default(),
            pending_cut: None,
            host: None,
            agent_host_filesystem: AgentHostFilesystemCapabilities::default(),
            seats,
            change_refs,
            _document_channels: document_channels,
            resource_uris,
            shared: Arc::new(Shared {
                inbox,
                wake,
                effect_wake,
                runner,
            }),
            runtime,
        }
    }

    pub fn add_window(&mut self) -> u64 {
        let window = self.app.add_window();
        if self.compose_new_windows {
            self.app.perform_command(himark::AppCommand::Dynamic(
                window,
                std::sync::Arc::new(himark::new_session::OpenNewSession { host: None }),
            ));
        }
        window.raw()
    }

    fn window_size(&self, window: u64) -> Size {
        himark::Windows::window_ref(self.app.store(), wid(window))
            .map(|entity| entity.viewport_size())
            .unwrap_or_else(|| Size::new(1.0, 1.0))
    }

    pub fn worker(&self) -> HimarkWorker {
        HimarkWorker {
            shared: self.shared.clone(),
        }
    }

    pub fn set_wake(&self, callback: impl Fn() + Send + 'static) {
        self.shared.wake.set(callback);
    }

    fn set_wake_raw(&self, callback: extern "C" fn(*mut c_void), context: *mut c_void) {
        let wake = Wake { callback, context };
        self.set_wake(move || wake.fire());
    }

    pub fn set_effect_wake(&self, callback: impl Fn() + Send + 'static) {
        self.shared.effect_wake.set(callback);
    }

    fn set_effect_wake_raw(&self, callback: extern "C" fn(*mut c_void), context: *mut c_void) {
        let wake = Wake { callback, context };
        self.set_effect_wake(move || wake.fire());
    }

    pub fn run_pending(&self) {
        self.worker().run_pending();
    }

    const DRAIN_CHUNK: usize = 32;

    const DRAIN_BUDGET: std::time::Duration = std::time::Duration::from_millis(8);

    pub fn set_drain_pacing(&mut self, chunk: usize, budget: std::time::Duration) {
        self.drain_chunk = chunk;
        self.drain_budget = budget;
    }

    pub fn queued_landings(&self) -> usize {
        self.shared.inbox.lock().expect("inbox").len()
    }

    pub fn drain(&mut self) -> bool {
        let started = std::time::Instant::now();
        let mut needs_redraw = false;
        loop {
            let chunk: Vec<AppCommand> = {
                let mut inbox = self.shared.inbox.lock().expect("inbox");
                let take = inbox.len().min(self.drain_chunk.max(1));
                inbox.drain(..take).collect()
            };
            if chunk.is_empty() {
                break;
            }
            needs_redraw |= self.app.perform_batch(chunk);
            if started.elapsed() >= self.drain_budget {
                break;
            }
        }

        if !self.shared.inbox.lock().expect("inbox").is_empty() {
            self.shared.wake.fire();
        }
        needs_redraw
    }

    pub fn draw(
        &mut self,
        window: u64,
        canvas: &Canvas,
        width: f32,
        height: f32,
        _scale: f32,
    ) -> bool {
        himark::Window::draw_with_size(wid(window), &mut self.app, canvas, Size::new(width, height))
    }

    pub fn record_latency(&mut self, now_secs: f64) {
        let latency_ns = self.app.stats().latency_ns();
        if let Some(event_started_at) = self.app.stats().latency_event_start() {
            let latency = (now_secs - event_started_at).max(0.0);
            latency_ns.store(
                (latency * 1_000_000_000.0) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            self.app.stats_mut().clear_latency_event_start();
        }
    }

    pub fn key_down(&mut self, window: u64, key: u32, mods: u32) -> bool {
        self.key_down_at(window, key, mods, 0.0)
    }

    pub fn key_down_at(&mut self, window: u64, key: u32, mods: u32, event_started_at: f64) -> bool {
        match map_key(key) {
            Some(key) => {
                let size = self.window_size(window);
                self.app.dispatch_timed(
                    wid(window),
                    Event::KeyDown {
                        key,
                        mods: map_mods(mods),
                    },
                    size,
                    event_started_at,
                )
            }
            None => false,
        }
    }

    pub fn text_input(&mut self, window: u64, text: &str) -> bool {
        self.text_input_at(window, text, 0.0)
    }

    pub fn text_input_at(&mut self, window: u64, text: &str, event_started_at: f64) -> bool {
        if text.is_empty() {
            return false;
        }
        let size = self.window_size(window);
        self.app.dispatch_timed(
            wid(window),
            Event::TextInput { text },
            size,
            event_started_at,
        )
    }

    pub fn clipboard_copy(&mut self, window: u64) -> Option<String> {
        self.app
            .with_clipboard_client(wid(window), |client| {
                client.copy().map(|content| content.text)
            })
            .flatten()
    }

    pub fn clipboard_cut(&mut self, window: u64) -> Option<String> {
        self.app
            .with_clipboard_client(wid(window), |client| {
                client.cut().map(|content| content.text)
            })
            .flatten()
    }

    pub fn clipboard_paste(&mut self, window: u64, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        self.app
            .with_clipboard_client(wid(window), |client| {
                client.paste(&himark::ClipboardContent {
                    text: text.to_owned(),
                })
            })
            .unwrap_or(false)
    }

    pub fn text_input_replacing(
        &mut self,
        window: u64,
        text: &str,
        replacement: HimarkRange,
    ) -> bool {
        self.app
            .with_ime_client(wid(window), |client| {
                client.insert_text(text, Some((replacement.start, replacement.length)))
            })
            .is_some()
    }

    pub fn mouse_down(&mut self, window: u64, x: f32, y: f32, mods: u32, click_count: u32) -> bool {
        self.mouse_down_at(window, x, y, mods, click_count, 0.0)
    }

    pub fn mouse_down_at(
        &mut self,
        window: u64,
        x: f32,
        y: f32,
        mods: u32,
        click_count: u32,
        event_started_at: f64,
    ) -> bool {
        let count = match click_count {
            0 => self.clicks.count(x, y),
            real => real.min(3) as u8,
        };
        let size = self.window_size(window);
        self.app.dispatch_timed(
            wid(window),
            Event::MouseDown {
                point: Point::new(x, y),
                button: MouseButton::Left,
                mods: map_mods(mods),
                count,
            },
            size,
            event_started_at,
        )
    }

    pub fn toolbar_height(&self) -> f32 {
        ::himark::env::Themes::of(self.app.store())
            .ui()
            .toolbar
            .height
    }

    pub fn mouse_drag(&mut self, window: u64, x: f32, y: f32, mods: u32) -> bool {
        let size = self.window_size(window);
        self.app.dispatch(
            wid(window),
            Event::MouseDrag {
                point: Point::new(x, y),
                mods: map_mods(mods),
            },
            size,
        )
    }

    pub fn mouse_move(&mut self, window: u64, x: f32, y: f32) -> bool {
        let size = self.window_size(window);
        self.app.dispatch(
            wid(window),
            Event::MouseMove {
                point: Point::new(x, y),
            },
            size,
        )
    }

    pub fn mouse_up(&mut self, window: u64, x: f32, y: f32) -> bool {
        let size = self.window_size(window);
        self.app.dispatch(
            wid(window),
            Event::MouseUp {
                point: Point::new(x, y),
            },
            size,
        )
    }

    pub fn scroll(&mut self, window: u64, x: f32, y: f32, delta_x: f32, delta_y: f32) -> bool {
        self.scroll_at_time(window, x, y, delta_x, delta_y, 0.0)
    }

    pub fn scroll_at_time(
        &mut self,
        window: u64,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        event_started_at: f64,
    ) -> bool {
        let size = self.window_size(window);

        let now = std::time::Instant::now();
        let paused = self
            .last_scroll
            .get()
            .is_none_or(|last| now.duration_since(last).as_millis() > 250);
        if paused {
            self.scroll_gesture.begin();
        }
        self.last_scroll.set(Some(now));
        self.app.dispatch_timed(
            wid(window),
            Event::Scroll {
                point: Point::new(x, y),
                delta_x,
                delta_y,
                gesture: &self.scroll_gesture,
            },
            size,
            event_started_at,
        )
    }

    pub fn animation_tick(&mut self, now_ms: f64) -> bool {
        let windows: Vec<_> = self
            .app
            .window_ids()
            .into_iter()
            .filter_map(|id| self.app.window_viewport(id).map(|size| (id, size)))
            .collect();
        let mut animating = false;
        for (window, size) in windows {
            animating |= self.app.dispatch(
                window,
                Event::AnimationClock {
                    now: AnimationClock::from_millis(now_ms),
                },
                size,
            );
        }
        animating
    }

    pub fn perform_command(&mut self, window: u64, id: &str) -> bool {
        self.app.perform_registered(wid(window), id)
    }

    pub fn set_chrome_clearance(&mut self, width: f32) {
        self.app.set_chrome_clearance(width);
    }

    fn install_agent_host_filesystem(&mut self, capabilities: AgentHostFilesystemCapabilities) {
        let installed = self.agent_host_filesystem;

        if capabilities.fetch_document && !installed.fetch_document {
            hiahp::open::install_open_handlers(
                &mut self.app,
                Arc::new(syntax_languages()),
                Arc::new(structdiff::Structural::new(Arc::new(syntax_languages()))),
            );
            self.app
                .register_handler::<himark::FetchBaseEffect>(fsroute::RouteBase {
                    refs: self.change_refs.clone(),
                });
            self.app.observe_stripe_bases();
            self.app
                .register_command(Arc::new(himark::hichanges::ToggleChangesView));

            self.app.register_command(Arc::new(himark::ReloadDocument));
            self.app
                .register_command(Arc::new(himark::hichanges::RefetchChanges::default()));
            self.app
                .register_toolbar_button(himark::hichanges::toolbar_button());
            self.app
                .register_command(Arc::new(himark::hihistory::ToggleHistoryView));
            self.app
                .register_toolbar_button(himark::hihistory::toolbar_button());
            self.app
                .register_handler::<hicode::FindDefinitionEffect>(lsproute::DefinitionRoute {
                    directory: Arc::clone(&self.seats),
                    uris: Arc::clone(&self.resource_uris),
                });
            self.app.register_handler::<himark::LspCompletionEffect>(
                hiahp::lsproute::CompletionRoute {
                    directory: Arc::clone(&self.seats),
                    uris: Arc::clone(&self.resource_uris),
                },
            );
            self.app.register_handler::<himark::hover::LspHoverEffect>(
                hiahp::lsproute::HoverRoute {
                    directory: Arc::clone(&self.seats),
                    uris: Arc::clone(&self.resource_uris),
                },
            );
            self.app
                .register_handler::<hicode::FindReferencesEffect>(lsproute::ReferencesRoute {
                    directory: Arc::clone(&self.seats),
                    uris: Arc::clone(&self.resource_uris),
                });
            self.app.register_handler::<himark::LspLocationsEffect>(
                hiahp::locations::RouteLspLocations {
                    directory: Arc::clone(&self.seats),
                    uris: Arc::clone(&self.resource_uris),
                },
            );

            himark::InstalledChangeSink::install(
                &mut self.app.store_mut(),
                Arc::new(docsync::SyncSink),
            );
            self.app.register_handler::<hicode::CodeNavigationEffect>(
                hicode::CodeNavigationHandler {
                    caller: self.app.effect_caller(),
                },
            );
            self.app
                .register_editor_command(Arc::new(hicode::GoDefinition));
            self.app
                .register_editor_command(Arc::new(hicode::GoReferences));
            self.app
                .register_editor_command(Arc::new(hicode::GoImplementations));
            self.app
                .register_editor_command(Arc::new(host::OpenWorkingCopy));
        }
        if capabilities.store_document && !installed.store_document {
            self.app
                .register_editor_command(Arc::new(himark::SaveDocument::with_save_as()));
            self.app.register_command(Arc::new(himark::SaveAll));

            self.app
                .register_editor_command(Arc::new(hiscript::RunScript));
            let caller = self.app.effect_caller();
            self.app
                .register_handler::<hiscript::RunScriptEffect>(hiscript::RunScriptHandler {
                    caller,
                });
        }
        if capabilities.list_directory && !installed.list_directory {
            self.app
                .register_handler::<himark::FindEffect>(find::NativeFindHandler {
                    directory: Arc::clone(&self.seats),
                });
            self.app.register_handler::<himark::SearchLocationsEffect>(
                hiahp::locations::RouteSearchLocations {
                    directory: Arc::clone(&self.seats),
                },
            );
            self.app
                .register_command(Arc::new(himark::hisearch::ToggleSearchView));
            self.app
                .register_toolbar_button(himark::hisearch::toolbar_button());
        }

        self.agent_host_filesystem.include(capabilities);
        if !(installed.fetch_document && installed.list_directory)
            && self.agent_host_filesystem.fetch_document
            && self.agent_host_filesystem.list_directory
        {
            self.app
                .register_command(Arc::new(himark::hifiles::ToggleSessionTree));
            self.app
                .register_toolbar_button(himark::hifiles::toolbar_button());
        }
    }

    pub fn set_host(&mut self, callbacks: HimarkHostCallbacks) {
        let bridge = host::HostBridge::new(callbacks);
        if callbacks.pick_files.is_some() {
            self.app
                .register_handler::<host::FilePickerEffect>(host::FilePickerHandler(Arc::clone(
                    &bridge,
                )));

            self.app
                .register_handler::<himark::new_session::PickFoldersEffect>(
                    host::PickFoldersHandler(Arc::clone(&bridge)),
                );
            self.app.register_command(Arc::new(host::OpenFilePicker));
        }
        if callbacks.set_clipboard.is_some() {
            self.app
                .register_handler::<himark::higent::ShareHostEffect>(host::ShareHostHandler(
                    Arc::clone(&bridge),
                ));
            self.app
                .register_command(Arc::new(himark::higent::ShareHost));
        }

        self.install_agent_host_filesystem(AgentHostFilesystemCapabilities {
            fetch_document: callbacks.fetch_document.is_some(),
            store_document: callbacks.store_document.is_some(),
            list_directory: callbacks.list_directory.is_some(),
        });
        if callbacks.pick_save.is_some() {
            self.app
                .register_handler::<himark::PickSaveEffect>(host::PickSaveHandler(Arc::clone(
                    &bridge,
                )));
        }
        self.host = Some(bridge);
    }

    pub fn host_picked(&mut self, request: u64, locations: Vec<himark::ResourceLocation>) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.requests.fulfill(request, Box::new(locations)))
    }

    pub fn host_picked_folder(
        &mut self,
        request: u64,
        location: Option<himark::ResourceLocation>,
    ) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.requests.fulfill(request, Box::new(location)))
    }

    pub fn host_fetched(&mut self, request: u64, text: Option<String>) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.requests.fulfill(request, Box::new(text)))
    }

    pub fn host_subscribed(&mut self, request: u64, subscription: u64) -> bool {
        self.host.as_ref().is_some_and(|host| {
            host.requests.fulfill(
                request,
                Box::new(match subscription {
                    0 => None::<u64>,
                    live => Some(live),
                }),
            )
        })
    }

    pub fn file_changed(&mut self, subscription: u64) -> bool {
        if subscription == 0 {
            return false;
        }
        self.shared
            .inbox
            .lock()
            .expect("inbox")
            .push_back(himark::AppCommand::FileChanged(himark::Subscription(
                subscription,
            )));
        self.shared.wake.fire();
        true
    }

    pub fn host_stored(&mut self, request: u64, stored: bool) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.requests.fulfill(request, Box::new(stored)))
    }

    pub fn host_listed(
        &mut self,
        request: u64,
        entries: Option<Vec<himark::ResourceLocation>>,
    ) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.requests.fulfill(request, Box::new(entries)))
    }

    pub fn open_document(
        &mut self,
        window: u64,
        name: impl Into<String>,
        source: impl Into<String>,
        primary: bool,
    ) -> bool {
        let name = name.into();
        let source = source.into();
        let document_name = name.clone();
        self.app
            .open_async(wid(window), name, primary, None, move |fonts, theme| {
                document_for(&document_name, &source, fonts, theme)
            });
        true
    }

    pub fn open_demo(&mut self, window: u64) {
        self.app.open_async(
            wid(window),
            "torture sample".to_owned(),
            true,
            Some(demo_location("torture sample")),
            demo::monster_document,
        );
    }

    pub fn open_demo_wall(&mut self, window: u64) {
        self.app.open_async(
            wid(window),
            "wall of text".to_owned(),
            true,
            Some(demo_location("wall of text")),
            demo::wall_of_text_document,
        );
    }

    pub fn has_text_focus(&mut self, window: u64) -> bool {
        self.app.with_ime_client(wid(window), |_| ()).is_some()
    }

    pub fn has_marked_text(&mut self, window: u64) -> bool {
        self.app
            .with_ime_client(wid(window), |client| client.has_marked_text())
            .unwrap_or(false)
    }

    pub fn marked_range(&mut self, window: u64) -> Option<HimarkRange> {
        self.app
            .with_ime_client(wid(window), |client| client.marked_range())?
            .map(|(start, end)| HimarkRange {
                start,
                length: end.saturating_sub(start),
            })
    }

    pub fn selected_range(&mut self, window: u64) -> Option<HimarkRange> {
        self.app
            .with_ime_client(wid(window), |client| client.selected_range())?
            .map(|(start, end)| HimarkRange {
                start,
                length: end.saturating_sub(start),
            })
    }

    pub fn set_marked_text(
        &mut self,
        window: u64,
        text: &str,
        selected: HimarkRange,
        replacement: Option<HimarkRange>,
    ) -> bool {
        let replacement = replacement.map(|range| (range.start, range.length));
        self.app
            .with_ime_client(wid(window), |client| {
                client.set_marked_text(text, (selected.start, selected.length), replacement)
            })
            .is_some()
    }

    pub fn unmark_text(&mut self, window: u64) -> bool {
        self.app
            .with_ime_client(wid(window), |client| client.unmark_text())
            .is_some()
    }

    pub fn set_selected_range(&mut self, window: u64, range: HimarkRange) -> bool {
        self.app
            .with_ime_client(wid(window), |client| {
                client.set_selected_range(range.start, range.length)
            })
            .is_some()
    }

    pub fn document_length(&mut self, window: u64) -> Option<u32> {
        self.app
            .with_ime_client(wid(window), |client| client.document_length())
    }

    pub fn reveal_selection(&mut self, window: u64) -> bool {
        self.app
            .with_ime_client(wid(window), |client| client.reveal_selection())
            .is_some()
    }

    pub fn selection_rects(&mut self, window: u64, range: HimarkRange) -> Vec<HimarkRect> {
        self.app
            .with_ime_client(wid(window), |client| {
                client
                    .selection_rects(range.start, range.length)
                    .into_iter()
                    .map(|(x, y, width, height)| HimarkRect {
                        x,
                        y,
                        width,
                        height,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn first_rect(&mut self, window: u64, range: HimarkRange) -> Option<HimarkRect> {
        self.app
            .with_ime_client(wid(window), |client| {
                client.first_rect(range.start, range.length)
            })?
            .map(|(x, y, width, height)| HimarkRect {
                x,
                y,
                width,
                height,
            })
    }

    pub fn char_index_at(&mut self, window: u64, x: f32, y: f32) -> Option<u32> {
        self.app
            .with_ime_client(wid(window), |client| client.char_index_at(x, y))?
    }

    pub fn substring(&mut self, window: u64, range: HimarkRange) -> Option<String> {
        self.app.with_ime_client(wid(window), |client| {
            client.substring_utf16(range.start, range.length)
        })?
    }

    unsafe fn substring_raw(
        &mut self,
        window: u64,
        range: HimarkRange,
        out: *mut c_char,
        cap: usize,
    ) -> usize {
        copy_optional_utf8(self.substring(window, range), out, cap)
    }

    unsafe fn marked_range_raw(&mut self, window: u64, out: *mut HimarkRange) -> bool {
        write_optional_out(self.marked_range(window), out)
    }

    unsafe fn selected_range_raw(&mut self, window: u64, out: *mut HimarkRange) -> bool {
        write_optional_out(self.selected_range(window), out)
    }

    unsafe fn first_rect_raw(
        &mut self,
        window: u64,
        range: HimarkRange,
        out: *mut HimarkRect,
    ) -> bool {
        write_optional_out(self.first_rect(window, range), out)
    }

    unsafe fn document_length_raw(&mut self, window: u64, out: *mut u32) -> bool {
        write_optional_out(self.document_length(window), out)
    }

    unsafe fn selection_rects_raw(
        &mut self,
        window: u64,
        range: HimarkRange,
        out: *mut HimarkRect,
        cap: usize,
    ) -> usize {
        let rects = self.selection_rects(window, range);
        if out.is_null() || rects.len() > cap {
            return rects.len();
        }
        std::ptr::copy_nonoverlapping(rects.as_ptr(), out, rects.len());
        rects.len()
    }
}

impl HimarkEngine {
    pub fn register_agent_server(
        &mut self,
        name: &str,
        seat: Arc<dyn himark::higent::AhpServer>,
    ) -> himark::higent::HostId {
        register_agent_server(&mut self.app, &self.seats, name, seat)
    }

    pub fn set_local_backend(&mut self, server: himark::higent::HostId) {
        self.seats.set_local(server);
        self.app.designate_local_host(server);
    }

    pub fn runtime(&self) -> tokio::runtime::Handle {
        self.runtime.handle().clone()
    }
}

impl HimarkWorker {
    pub fn run_pending(&self) {
        self.shared.runner.run();
    }
}

#[no_mangle]
pub extern "C" fn himark_agent_host_autostart() -> bool {
    host_discovery::autostart().is_some()
}

#[no_mangle]
pub extern "C" fn himark_create() -> *mut HimarkEngine {
    Box::into_raw(Box::new(HimarkEngine::new()))
}

#[no_mangle]
pub unsafe extern "C" fn himark_add_window(engine: *mut HimarkEngine) -> u64 {
    engine.as_mut().map_or(0, HimarkEngine::add_window)
}

#[no_mangle]
pub unsafe extern "C" fn himark_destroy(engine: *mut HimarkEngine) {
    if !engine.is_null() {
        drop(Box::from_raw(engine));
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_set_wake(
    engine: *mut HimarkEngine,
    callback: extern "C" fn(*mut c_void),
    context: *mut c_void,
) {
    if let Some(engine) = engine.as_ref() {
        engine.set_wake_raw(callback, context);
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_set_effect_wake(
    engine: *mut HimarkEngine,
    callback: extern "C" fn(*mut c_void),
    context: *mut c_void,
) {
    if let Some(engine) = engine.as_ref() {
        engine.set_effect_wake_raw(callback, context);
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_run_pending(engine: *mut HimarkEngine) {
    if let Some(engine) = engine.as_ref() {
        engine.run_pending();
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_drain(engine: *mut HimarkEngine) -> bool {
    engine.as_mut().map_or(false, HimarkEngine::drain)
}

#[no_mangle]
pub unsafe extern "C" fn himark_draw(
    engine: *mut HimarkEngine,
    window: u64,
    canvas: *mut c_void,
    width: f32,
    height: f32,
    _scale: f32,
) -> bool {
    if engine.is_null() || canvas.is_null() {
        return false;
    }

    let canvas = &*(canvas as *const Canvas);
    engine.as_mut().map_or(false, |engine| {
        engine.draw(window, canvas, width, height, _scale)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_key_down(
    engine: *mut HimarkEngine,
    window: u64,
    key: u32,
    mods: u32,
) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.key_down(window, key, mods))
}

#[no_mangle]
pub unsafe extern "C" fn himark_text_input(
    engine: *mut HimarkEngine,
    window: u64,
    utf8: *const c_char,
    len: usize,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(text) = str_from(utf8, len) else {
        return false;
    };
    engine.text_input(window, text)
}

#[no_mangle]
pub unsafe extern "C" fn himark_copy(
    engine: *mut HimarkEngine,
    window: u64,
    out: *mut c_char,
    cap: usize,
) -> usize {
    engine.as_mut().map_or(0, |engine| {
        copy_optional_utf8(engine.clipboard_copy(window), out, cap)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_cut(
    engine: *mut HimarkEngine,
    window: u64,
    out: *mut c_char,
    cap: usize,
) -> usize {
    engine.as_mut().map_or(0, |engine| {
        // The sized-string protocol calls twice. Cut is
        // side-effecting — the first run deletes the selection — so
        // it runs ONLY on the sizing probe; the fill call copies the
        // stashed text instead of cutting again (which would find no
        // selection and hand the pasteboard nothing).
        if out.is_null() {
            engine.pending_cut = engine.clipboard_cut(window);
            return engine.pending_cut.as_ref().map_or(0, String::len);
        }
        copy_optional_utf8(engine.pending_cut.take(), out, cap)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_paste(
    engine: *mut HimarkEngine,
    window: u64,
    utf8: *const c_char,
    len: usize,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(text) = str_from(utf8, len) else {
        return false;
    };
    engine.clipboard_paste(window, text)
}

#[no_mangle]
pub unsafe extern "C" fn himark_text_input_replacing(
    engine: *mut HimarkEngine,
    window: u64,
    utf8: *const c_char,
    len: usize,
    replacement: HimarkRange,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(text) = str_from(utf8, len) else {
        return false;
    };
    engine.text_input_replacing(window, text, replacement)
}

#[no_mangle]
pub unsafe extern "C" fn himark_toolbar_height(engine: *mut HimarkEngine) -> f32 {
    engine
        .as_mut()
        .map_or(0.0, |engine| engine.toolbar_height())
}

#[no_mangle]
pub unsafe extern "C" fn himark_mouse_drag(
    engine: *mut HimarkEngine,
    window: u64,
    x: f32,
    y: f32,
    mods: u32,
) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.mouse_drag(window, x, y, mods))
}

#[no_mangle]
pub unsafe extern "C" fn himark_mouse_move(
    engine: *mut HimarkEngine,
    window: u64,
    x: f32,
    y: f32,
) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.mouse_move(window, x, y))
}

#[no_mangle]
pub unsafe extern "C" fn himark_mouse_up(
    engine: *mut HimarkEngine,
    window: u64,
    x: f32,
    y: f32,
) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.mouse_up(window, x, y))
}

#[no_mangle]
pub unsafe extern "C" fn himark_mouse_down(
    engine: *mut HimarkEngine,
    window: u64,
    x: f32,
    y: f32,
    mods: u32,
    click_count: u32,
) -> bool {
    engine.as_mut().map_or(false, |engine| {
        engine.mouse_down(window, x, y, mods, click_count)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_scroll(
    engine: *mut HimarkEngine,
    window: u64,
    x: f32,
    y: f32,
    _delta_x: f32,
    delta_y: f32,
) -> bool {
    engine.as_mut().map_or(false, |engine| {
        engine.scroll(window, x, y, _delta_x, delta_y)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_animation_tick(engine: *mut HimarkEngine, now_ms: f64) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.animation_tick(now_ms))
}

#[no_mangle]
pub unsafe extern "C" fn himark_perform_command(
    engine: *mut HimarkEngine,
    window: u64,
    id: *const c_char,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    if id.is_null() {
        return false;
    }
    let Ok(id) = std::ffi::CStr::from_ptr(id).to_str() else {
        return false;
    };
    engine.perform_command(window, id)
}

#[no_mangle]
pub unsafe extern "C" fn himark_set_chrome_clearance(engine: *mut HimarkEngine, width: f32) {
    if let Some(engine) = engine.as_mut() {
        engine.set_chrome_clearance(width);
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_set_host(
    engine: *mut HimarkEngine,
    callbacks: HimarkHostCallbacks,
) {
    if let Some(engine) = engine.as_mut() {
        engine.set_host(callbacks);
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_host_picked(
    engine: *mut HimarkEngine,
    request: u64,
    locations: *const HimarkLocation,
    count: usize,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let mut picked = Vec::with_capacity(count);
    if count > 0 {
        if locations.is_null() {
            return false;
        }
        for location in std::slice::from_raw_parts(locations, count) {
            let Some(location) = host::location_from_abi(location) else {
                return false;
            };
            picked.push(location);
        }
    }
    engine.host_picked(request, picked)
}

#[no_mangle]
pub unsafe extern "C" fn himark_host_picked_folder(
    engine: *mut HimarkEngine,
    request: u64,
    location: *const HimarkLocation,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let location = match location.is_null() {
        true => None,
        false => match host::location_from_abi(&*location) {
            Some(location) => Some(location),
            None => return false,
        },
    };
    engine.host_picked_folder(request, location)
}

#[no_mangle]
pub unsafe extern "C" fn himark_host_fetched(
    engine: *mut HimarkEngine,
    request: u64,
    text: *const c_char,
    text_len: usize,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let text = match text.is_null() {
        true => None,
        false => match str_from(text, text_len) {
            Some(text) => Some(text.to_owned()),
            None => return false,
        },
    };
    engine.host_fetched(request, text)
}

#[no_mangle]
pub unsafe extern "C" fn himark_host_subscribed(
    engine: *mut HimarkEngine,
    request: u64,
    subscription: u64,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    engine.host_subscribed(request, subscription)
}

#[no_mangle]
pub unsafe extern "C" fn himark_file_changed(engine: *mut HimarkEngine, subscription: u64) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    engine.file_changed(subscription)
}

#[no_mangle]
pub unsafe extern "C" fn himark_host_stored(
    engine: *mut HimarkEngine,
    request: u64,
    stored: bool,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    engine.host_stored(request, stored)
}

#[no_mangle]
pub unsafe extern "C" fn himark_host_listed(
    engine: *mut HimarkEngine,
    request: u64,
    entries: *const HimarkLocation,
    count: usize,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let entries = match entries.is_null() {
        true => None,
        false => {
            let mut listed = Vec::with_capacity(count);
            for entry in std::slice::from_raw_parts(entries, count) {
                let Some(location) = host::location_from_abi(entry) else {
                    return false;
                };
                listed.push(location);
            }
            Some(listed)
        }
    };
    engine.host_listed(request, entries)
}

#[no_mangle]
pub unsafe extern "C" fn himark_open_document(
    engine: *mut HimarkEngine,
    window: u64,
    name: *const c_char,
    name_len: usize,
    utf8: *const c_char,
    len: usize,
    primary: bool,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(name) = str_from(name, name_len) else {
        return false;
    };
    let Some(source) = str_from(utf8, len) else {
        return false;
    };
    engine.open_document(window, name, source, primary)
}

fn document_for(
    name: &str,
    source: &str,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> himark::Document {
    hiahp::open::document_for(&syntax_languages(), name, source, fonts, theme)
}

#[no_mangle]
pub unsafe extern "C" fn himark_open_demo(engine: *mut HimarkEngine, window: u64) {
    if let Some(engine) = engine.as_mut() {
        engine.open_demo(window);
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_open_demo_wall(engine: *mut HimarkEngine, window: u64) {
    if let Some(engine) = engine.as_mut() {
        engine.open_demo_wall(window);
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_has_text_focus(engine: *mut HimarkEngine, window: u64) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.has_text_focus(window))
}

#[no_mangle]
pub unsafe extern "C" fn himark_has_marked_text(engine: *mut HimarkEngine, window: u64) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.has_marked_text(window))
}

#[no_mangle]
pub unsafe extern "C" fn himark_set_marked_text(
    engine: *mut HimarkEngine,
    window: u64,
    utf8: *const c_char,
    len: usize,
    selected: HimarkRange,
    has_replacement: bool,
    replacement: HimarkRange,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(text) = str_from(utf8, len) else {
        return false;
    };
    let replacement = has_replacement.then_some(replacement);
    engine.set_marked_text(window, text, selected, replacement)
}

#[no_mangle]
pub unsafe extern "C" fn himark_unmark_text(engine: *mut HimarkEngine, window: u64) -> bool {
    engine
        .as_mut()
        .map_or(false, |engine| engine.unmark_text(window))
}

#[no_mangle]
pub unsafe extern "C" fn himark_marked_range(
    engine: *mut HimarkEngine,
    window: u64,
    out: *mut HimarkRange,
) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.marked_range_raw(window, out))
}

#[no_mangle]
pub unsafe extern "C" fn himark_selected_range(
    engine: *mut HimarkEngine,
    window: u64,
    out: *mut HimarkRange,
) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.selected_range_raw(window, out))
}

#[no_mangle]
pub unsafe extern "C" fn himark_set_selected_range(
    engine: *mut HimarkEngine,
    window: u64,
    range: HimarkRange,
) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.set_selected_range(window, range))
}

#[no_mangle]
pub unsafe extern "C" fn himark_document_length(
    engine: *mut HimarkEngine,
    window: u64,
    out: *mut u32,
) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.document_length_raw(window, out))
}

#[no_mangle]
pub unsafe extern "C" fn himark_reveal_selection(engine: *mut HimarkEngine, window: u64) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.reveal_selection(window))
}

#[no_mangle]
pub unsafe extern "C" fn himark_selection_rects(
    engine: *mut HimarkEngine,
    window: u64,
    range: HimarkRange,
    out: *mut HimarkRect,
    cap: usize,
) -> usize {
    engine.as_mut().map_or(0, |engine| {
        engine.selection_rects_raw(window, range, out, cap)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_first_rect(
    engine: *mut HimarkEngine,
    window: u64,
    range: HimarkRange,
    out: *mut HimarkRect,
) -> bool {
    engine
        .as_mut()
        .is_some_and(|engine| engine.first_rect_raw(window, range, out))
}

#[no_mangle]
pub unsafe extern "C" fn himark_char_index_at(
    engine: *mut HimarkEngine,
    window: u64,
    x: f32,
    y: f32,
) -> i64 {
    let Some(engine) = engine.as_mut() else {
        return -1;
    };
    engine
        .char_index_at(window, x, y)
        .map_or(-1, |index| index as i64)
}

#[no_mangle]
pub unsafe extern "C" fn himark_substring(
    engine: *mut HimarkEngine,
    window: u64,
    range: HimarkRange,
    out: *mut c_char,
    cap: usize,
) -> usize {
    engine
        .as_mut()
        .map_or(0, |engine| engine.substring_raw(window, range, out, cap))
}

unsafe fn str_from<'a>(utf8: *const c_char, len: usize) -> Option<&'a str> {
    if utf8.is_null() {
        return (len == 0).then_some("");
    }
    let bytes = std::slice::from_raw_parts(utf8 as *const u8, len);
    std::str::from_utf8(bytes).ok()
}

unsafe fn copy_optional_utf8(text: Option<String>, out: *mut c_char, cap: usize) -> usize {
    let Some(text) = text else {
        return 0;
    };
    let bytes = text.as_bytes();
    if out.is_null() || bytes.len() > cap {
        return bytes.len();
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, out, bytes.len());
    bytes.len()
}

unsafe fn write_optional_out<T: Copy>(value: Option<T>, out: *mut T) -> bool {
    let Some(value) = value else {
        return false;
    };
    if out.is_null() {
        return false;
    }
    *out = value;
    true
}

#[cfg(test)]
fn test_connector() -> Arc<dyn hiahp::transport::Connector> {
    Arc::new(desktop::DesktopConnector)
}

#[cfg(test)]
mod findroute_tests;
#[cfg(test)]
mod tests;
