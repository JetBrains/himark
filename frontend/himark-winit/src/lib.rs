// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::{
    error::Error,
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use desktop::{is_insertable_text, startup_profile_log};
use skia_safe::{
    surfaces, AlphaType, Color, ColorSpace, ColorType, Contains, Font, FontMgr, FontStyle,
    ImageInfo, Paint, Point, Rect,
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy, OwnedDisplayHandle},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

const INITIAL_WIDTH: u32 = 1280;
const INITIAL_HEIGHT: u32 = 860;
const MIN_WIDTH: u32 = 640;
const MIN_HEIGHT: u32 = 480;
const LINE_SCROLL_PX: f32 = 48.0;
const ANIMATION_FRAME: Duration = Duration::from_millis(16);
const MENU_BAR_HEIGHT: f32 = 30.0;
const MENU_ROOT_GAP: f32 = 4.0;
const MENU_ROOT_PAD_X: f32 = 10.0;
const MENU_PANEL_PAD_X: f32 = 12.0;
const MENU_PANEL_PAD_Y: f32 = 6.0;
const MENU_SHORTCUT_GAP: f32 = 32.0;
const MENU_ITEM_HEIGHT: f32 = 24.0;
const MENU_SEPARATOR_HEIGHT: f32 = 10.0;
const MENU_ROOT_FONT_SIZE: f32 = 13.0;
const MENU_ITEM_FONT_SIZE: f32 = 13.0;

#[derive(Clone, Copy)]
enum MenuEntrySpec {
    Separator,
    Command {
        label: &'static str,
        shortcut: Option<&'static str>,
        command: HostCommand,
    },
}

#[derive(Clone, Copy)]
struct RootMenuSpec {
    title: &'static str,
    entries: &'static [MenuEntrySpec],
}

const FILE_MENU_ENTRIES: [MenuEntrySpec; 4] = [
    MenuEntrySpec::Command {
        label: "New",
        shortcut: Some("Ctrl+N"),
        command: HostCommand::Command("workbench.new-document"),
    },
    MenuEntrySpec::Separator,
    MenuEntrySpec::Command {
        label: "Open Demo Document",
        shortcut: None,
        command: HostCommand::OpenDemo,
    },
    MenuEntrySpec::Command {
        label: "Open Demo Wall of Text",
        shortcut: None,
        command: HostCommand::OpenDemoWall,
    },
];

const VIEW_MENU_ENTRIES: [MenuEntrySpec; 7] = [
    MenuEntrySpec::Command {
        label: "Toggle Peeker",
        shortcut: Some("Ctrl+P"),
        command: HostCommand::Command("peeker.toggle"),
    },
    MenuEntrySpec::Command {
        label: "Command Palette",
        shortcut: Some("Ctrl+Shift+P"),
        command: HostCommand::Command("palette.toggle"),
    },
    MenuEntrySpec::Command {
        label: "Split Pane",
        shortcut: Some("Ctrl+D"),
        command: HostCommand::Command("workbench.split-pane"),
    },
    MenuEntrySpec::Command {
        label: "Find",
        shortcut: Some("Ctrl+F"),
        command: HostCommand::Command("find.open"),
    },
    MenuEntrySpec::Command {
        label: "Find in Files",
        shortcut: Some("Ctrl+Shift+F"),
        command: HostCommand::Command("search.focus"),
    },
    MenuEntrySpec::Command {
        label: "Close",
        shortcut: Some("Ctrl+W"),
        command: HostCommand::Command("workbench.close"),
    },
    MenuEntrySpec::Command {
        label: "Close Pane",
        shortcut: Some("Ctrl+Shift+W"),
        command: HostCommand::Command("workbench.close-pane"),
    },
];

const ROOT_MENUS: [RootMenuSpec; 2] = [
    RootMenuSpec {
        title: "File",
        entries: &FILE_MENU_ENTRIES,
    },
    RootMenuSpec {
        title: "View",
        entries: &VIEW_MENU_ENTRIES,
    },
];

pub struct Options {
    pub title: String,

    pub app_id: String,

    pub initial_width: u32,

    pub initial_height: u32,

    pub min_width: u32,

    pub min_height: u32,

    pub open_paths: Vec<PathBuf>,
}

impl Options {
    pub fn from_env() -> Self {
        Self {
            open_paths: std::env::args_os().skip(1).map(PathBuf::from).collect(),
            ..Self::default()
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            title: "Himark".to_owned(),
            app_id: "dev.zolotov.himark".to_owned(),
            initial_width: INITIAL_WIDTH,
            initial_height: INITIAL_HEIGHT,
            min_width: MIN_WIDTH,
            min_height: MIN_HEIGHT,
            open_paths: Vec::new(),
        }
    }
}

pub fn run(options: Options) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();

    let _ = himark_api::himark_agent_host_autostart();
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let context = softbuffer::Context::new(event_loop.owned_display_handle())?;
    let mut host = WinitHost::new(options, context, proxy);

    startup_profile_log("winit.run before event loop", started.elapsed());
    event_loop.run_app(&mut host)?;
    Ok(())
}

#[derive(Debug)]
enum UserEvent {
    Drain,
}

enum WorkerMessage {
    Run,
    Stop,
}

struct Engine {
    inner: himark_api::HimarkEngine,

    window: u64,
}

impl Engine {
    fn new() -> Self {
        let mut inner = himark_api::HimarkEngine::new();

        inner.set_host(himark_api::HimarkHostCallbacks::agent_host_filesystem());
        let window = inner.add_window();
        Self { inner, window }
    }

    fn worker(&self) -> himark_api::HimarkWorker {
        self.inner.worker()
    }

    fn set_wakes(
        &self,
        proxy: EventLoopProxy<UserEvent>,
        worker_sender: mpsc::Sender<WorkerMessage>,
    ) {
        self.inner.set_wake(move || {
            let _ = proxy.send_event(UserEvent::Drain);
        });
        self.inner.set_effect_wake(move || {
            let _ = worker_sender.send(WorkerMessage::Run);
        });
    }

    fn drain(&mut self) -> bool {
        self.inner.drain()
    }

    fn draw(&mut self, canvas: &skia_safe::Canvas, width: f32, height: f32, scale: f32) -> bool {
        self.inner.draw(self.window, canvas, width, height, scale)
    }

    fn record_latency(&mut self, now_secs: f64) {
        self.inner.record_latency(now_secs);
    }

    fn key_down_at(&mut self, key: u32, mods: u32, event_started_at: f64) -> bool {
        self.inner
            .key_down_at(self.window, key, mods, event_started_at)
    }

    fn text_input_at(&mut self, text: &str, event_started_at: f64) -> bool {
        self.inner
            .text_input_at(self.window, text, event_started_at)
    }

    fn clipboard_copy(&mut self) -> Option<String> {
        self.inner.clipboard_copy(self.window)
    }

    fn clipboard_cut(&mut self) -> Option<String> {
        self.inner.clipboard_cut(self.window)
    }

    fn clipboard_paste(&mut self, text: &str) -> bool {
        self.inner.clipboard_paste(self.window, text)
    }

    fn mouse_down_at(&mut self, x: f32, y: f32, mods: u32, event_started_at: f64) -> bool {
        self.inner
            .mouse_down_at(self.window, x, y, mods, 0, event_started_at)
    }

    fn mouse_drag(&mut self, x: f32, y: f32, mods: u32) -> bool {
        self.inner.mouse_drag(self.window, x, y, mods)
    }

    fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        self.inner.mouse_move(self.window, x, y)
    }

    fn mouse_up(&mut self, x: f32, y: f32) -> bool {
        self.inner.mouse_up(self.window, x, y)
    }

    fn scroll_at_time(
        &mut self,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        event_started_at: f64,
    ) -> bool {
        self.inner
            .scroll_at_time(self.window, x, y, delta_x, delta_y, event_started_at)
    }

    fn animation_tick(&mut self, now_ms: f64) -> bool {
        self.inner.animation_tick(now_ms)
    }

    fn perform_command(&mut self, id: &str) -> bool {
        self.inner.perform_command(self.window, id)
    }

    fn open_demo(&mut self) {
        self.inner.open_demo(self.window);
    }

    fn open_demo_wall(&mut self) {
        self.inner.open_demo_wall(self.window);
    }

    fn open_document(&mut self, name: &str, source: &str, primary: bool) -> bool {
        self.inner.open_document(self.window, name, source, primary)
    }

    fn has_text_focus(&mut self) -> bool {
        self.inner.has_text_focus(self.window)
    }

    fn set_marked_text(
        &mut self,
        text: &str,
        selected: himark_api::HimarkRange,
        replacement: Option<himark_api::HimarkRange>,
    ) -> bool {
        self.inner
            .set_marked_text(self.window, text, selected, replacement)
    }

    fn unmark_text(&mut self) -> bool {
        self.inner.unmark_text(self.window)
    }

    fn has_marked_text(&mut self) -> bool {
        self.inner.has_marked_text(self.window)
    }

    fn ime_cursor_rect(&mut self) -> Option<himark_api::HimarkRect> {
        let range = self.inner.selected_range(self.window)?;
        self.inner.first_rect(self.window, range)
    }
}

struct Worker {
    sender: mpsc::Sender<WorkerMessage>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Worker {
    fn start(worker: himark_api::HimarkWorker) -> Self {
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                match message {
                    WorkerMessage::Run => worker.run_pending(),
                    WorkerMessage::Stop => break,
                }
            }
        });
        Self {
            sender,
            handle: Some(handle),
        }
    }

    fn sender(&self) -> mpsc::Sender<WorkerMessage> {
        self.sender.clone()
    }

    fn request_run(&self) {
        let _ = self.sender.send(WorkerMessage::Run);
    }

    fn stop(&mut self) {
        let _ = self.sender.send(WorkerMessage::Stop);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

struct MenuMetrics {
    bar_height: f32,
    root_gap: f32,
    root_pad_x: f32,
    panel_pad_x: f32,
    panel_pad_y: f32,
    shortcut_gap: f32,
    item_height: f32,
    separator_height: f32,
    root_baseline: f32,
    item_baseline: f32,
}

impl MenuMetrics {
    fn scaled(scale: f32) -> Self {
        let bar_height = MENU_BAR_HEIGHT * scale;
        let item_height = MENU_ITEM_HEIGHT * scale;
        Self {
            bar_height,
            root_gap: MENU_ROOT_GAP * scale,
            root_pad_x: MENU_ROOT_PAD_X * scale,
            panel_pad_x: MENU_PANEL_PAD_X * scale,
            panel_pad_y: MENU_PANEL_PAD_Y * scale,
            shortcut_gap: MENU_SHORTCUT_GAP * scale,
            item_height,
            separator_height: MENU_SEPARATOR_HEIGHT * scale,
            root_baseline: bar_height * 0.65,
            item_baseline: item_height * 0.7,
        }
    }
}

#[derive(Default)]
struct MenuBar {
    open_root: Option<usize>,
    hover_root: Option<usize>,
    hover_entry: Option<usize>,
    font_scale: f32,
    root_font: Option<Font>,
    item_font: Option<Font>,
}

impl MenuBar {
    fn enabled(&self) -> bool {
        !cfg!(target_os = "macos")
    }

    fn bar_height(&self, scale: f32) -> f32 {
        if self.enabled() {
            MenuMetrics::scaled(scale).bar_height
        } else {
            0.0
        }
    }

    fn ensure_fonts(&mut self, scale: f32) {
        if !self.enabled() {
            return;
        }
        if self.root_font.is_some() && (self.font_scale - scale).abs() < f32::EPSILON {
            return;
        }

        self.font_scale = scale;
        self.root_font = Some(Self::font(MENU_ROOT_FONT_SIZE * scale, FontStyle::normal()));
        self.item_font = Some(Self::font(MENU_ITEM_FONT_SIZE * scale, FontStyle::normal()));
    }

    fn font(size: f32, style: FontStyle) -> Font {
        let typeface = FontMgr::new()
            .legacy_make_typeface(None, style)
            .expect("a system typeface");
        let mut font = Font::from_typeface(typeface, size);
        font.set_edging(skia_safe::font::Edging::AntiAlias);
        font
    }

    fn root_font(&self) -> &Font {
        self.root_font.as_ref().expect("menu root font initialized")
    }

    fn item_font(&self) -> &Font {
        self.item_font.as_ref().expect("menu item font initialized")
    }

    fn root_rects(&mut self, scale: f32) -> Vec<Rect> {
        self.ensure_fonts(scale);
        let metrics = MenuMetrics::scaled(scale);
        let mut x = metrics.root_gap;
        ROOT_MENUS
            .iter()
            .map(|menu| {
                let width =
                    self.root_font().measure_str(menu.title, None).0 + metrics.root_pad_x * 2.0;
                let rect = Rect::from_xywh(x, 0.0, width, metrics.bar_height);
                x += width + metrics.root_gap;
                rect
            })
            .collect()
    }

    fn dropdown_layout(
        &mut self,
        root_index: usize,
        scale: f32,
    ) -> Option<(Rect, Vec<Option<Rect>>)> {
        let root_rect = *self.root_rects(scale).get(root_index)?;
        self.ensure_fonts(scale);
        let metrics = MenuMetrics::scaled(scale);
        let menu = ROOT_MENUS.get(root_index)?;

        let mut label_width: f32 = 0.0;
        let mut shortcut_width: f32 = 0.0;
        for entry in menu.entries {
            if let MenuEntrySpec::Command {
                label, shortcut, ..
            } = entry
            {
                label_width = label_width.max(self.item_font().measure_str(label, None).0);
                if let Some(shortcut) = shortcut {
                    shortcut_width =
                        shortcut_width.max(self.item_font().measure_str(shortcut, None).0);
                }
            }
        }

        let width = metrics.panel_pad_x * 2.0
            + label_width
            + if shortcut_width > 0.0 {
                metrics.shortcut_gap + shortcut_width
            } else {
                0.0
            };
        let mut item_rects = Vec::with_capacity(menu.entries.len());
        let mut y = metrics.bar_height + metrics.panel_pad_y;
        for entry in menu.entries {
            match entry {
                MenuEntrySpec::Separator => {
                    item_rects.push(None);
                    y += metrics.separator_height;
                }
                MenuEntrySpec::Command { .. } => {
                    item_rects.push(Some(Rect::from_xywh(
                        root_rect.left + 2.0,
                        y,
                        width - 4.0,
                        metrics.item_height,
                    )));
                    y += metrics.item_height;
                }
            }
        }
        let panel_height = y - metrics.bar_height + metrics.panel_pad_y;
        Some((
            Rect::from_xywh(
                root_rect.left,
                metrics.bar_height - 1.0,
                width,
                panel_height,
            ),
            item_rects,
        ))
    }

    fn root_at(&mut self, point: Point, scale: f32) -> Option<usize> {
        self.root_rects(scale)
            .iter()
            .position(|rect| rect.contains(point))
    }

    fn entry_at(&mut self, point: Point, scale: f32) -> Option<(usize, usize)> {
        let root_index = self.open_root?;
        let (_, item_rects) = self.dropdown_layout(root_index, scale)?;
        item_rects.iter().enumerate().find_map(|(index, rect)| {
            rect.filter(|rect| rect.contains(point))
                .map(|_| (root_index, index))
        })
    }

    fn close(&mut self) -> bool {
        let changed =
            self.open_root.is_some() || self.hover_root.is_some() || self.hover_entry.is_some();
        self.open_root = None;
        self.hover_root = None;
        self.hover_entry = None;
        changed
    }

    fn handle_cursor_move(&mut self, position: PhysicalPosition<f64>, scale: f64) -> bool {
        if !self.enabled() {
            return false;
        }
        let before = (self.open_root, self.hover_root, self.hover_entry);
        let point = Point::new(position.x as f32, position.y as f32);
        self.hover_root = self.root_at(point, scale as f32);
        if self.open_root.is_some() {
            if let Some(root_index) = self.hover_root {
                self.open_root = Some(root_index);
            }
            self.hover_entry = self.entry_at(point, scale as f32).map(|(_, entry)| entry);
        } else {
            self.hover_entry = None;
        }
        before != (self.open_root, self.hover_root, self.hover_entry)
    }

    fn handle_mouse_down(
        &mut self,
        position: PhysicalPosition<f64>,
        scale: f64,
    ) -> MenuInteraction {
        if !self.enabled() {
            return MenuInteraction::PassThrough;
        }

        let point = Point::new(position.x as f32, position.y as f32);
        let metrics = MenuMetrics::scaled(scale as f32);
        if point.y < metrics.bar_height {
            let before = (self.open_root, self.hover_root, self.hover_entry);
            self.hover_root = self.root_at(point, scale as f32);
            self.hover_entry = None;
            self.open_root = match (self.open_root, self.hover_root) {
                (Some(open), Some(hit)) if open == hit => None,
                (_, Some(hit)) => Some(hit),
                _ => None,
            };
            return MenuInteraction::Consumed {
                needs_redraw: before != (self.open_root, self.hover_root, self.hover_entry),
            };
        }

        let Some(open_root) = self.open_root else {
            return MenuInteraction::PassThrough;
        };
        if let Some((_, entry_index)) = self.entry_at(point, scale as f32) {
            let command = match ROOT_MENUS[open_root].entries[entry_index] {
                MenuEntrySpec::Command { command, .. } => command,
                MenuEntrySpec::Separator => {
                    return MenuInteraction::Consumed {
                        needs_redraw: false,
                    }
                }
            };
            self.close();
            return MenuInteraction::Command(command);
        }

        if let Some((panel_rect, _)) = self.dropdown_layout(open_root, scale as f32) {
            if panel_rect.contains(point) {
                return MenuInteraction::Consumed {
                    needs_redraw: false,
                };
            }
        }

        MenuInteraction::Consumed {
            needs_redraw: self.close(),
        }
    }

    fn paint(&mut self, canvas: &skia_safe::Canvas, width: f32, scale: f32) {
        if !self.enabled() {
            return;
        }

        self.ensure_fonts(scale);
        let metrics = MenuMetrics::scaled(scale);
        let root_rects = self.root_rects(scale);
        let mut paint = Paint::default();
        paint.set_anti_alias(true);

        paint.set_color(Color::from_argb(255, 243, 243, 246));
        canvas.draw_rect(Rect::from_xywh(0.0, 0.0, width, metrics.bar_height), &paint);
        paint.set_color(Color::from_argb(255, 208, 208, 214));
        canvas.draw_rect(
            Rect::from_xywh(
                0.0,
                metrics.bar_height - scale.max(1.0),
                width,
                scale.max(1.0),
            ),
            &paint,
        );

        for (index, rect) in root_rects.iter().enumerate() {
            if self.open_root == Some(index) || self.hover_root == Some(index) {
                paint.set_color(if self.open_root == Some(index) {
                    Color::from_argb(255, 225, 229, 236)
                } else {
                    Color::from_argb(255, 233, 236, 242)
                });
                canvas.draw_round_rect(
                    Rect::from_xywh(
                        rect.left,
                        2.0 * scale,
                        rect.width(),
                        metrics.bar_height - 4.0 * scale,
                    ),
                    6.0 * scale,
                    6.0 * scale,
                    &paint,
                );
            }
            paint.set_color(Color::from_argb(255, 35, 35, 38));
            canvas.draw_str(
                ROOT_MENUS[index].title,
                (rect.left + metrics.root_pad_x, metrics.root_baseline),
                self.root_font(),
                &paint,
            );
        }

        let Some(open_root) = self.open_root else {
            return;
        };
        let Some((panel_rect, item_rects)) = self.dropdown_layout(open_root, scale) else {
            return;
        };

        paint.set_color(Color::from_argb(255, 250, 250, 252));
        canvas.draw_round_rect(panel_rect, 8.0 * scale, 8.0 * scale, &paint);
        paint.set_color(Color::from_argb(255, 196, 199, 205));
        paint.set_style(skia_safe::paint::Style::Stroke);
        paint.set_stroke_width(scale.max(1.0));
        canvas.draw_round_rect(panel_rect, 8.0 * scale, 8.0 * scale, &paint);
        paint.set_style(skia_safe::paint::Style::Fill);

        for (index, entry) in ROOT_MENUS[open_root].entries.iter().enumerate() {
            match entry {
                MenuEntrySpec::Separator => {
                    let Some(next_rect) = item_rects.iter().skip(index + 1).flatten().next() else {
                        continue;
                    };
                    let y = next_rect.top - metrics.separator_height * 0.5;
                    paint.set_color(Color::from_argb(255, 220, 223, 228));
                    canvas.draw_rect(
                        Rect::from_xywh(
                            panel_rect.left + metrics.panel_pad_x,
                            y,
                            panel_rect.width() - metrics.panel_pad_x * 2.0,
                            scale.max(1.0),
                        ),
                        &paint,
                    );
                }
                MenuEntrySpec::Command {
                    label, shortcut, ..
                } => {
                    let Some(item_rect) = item_rects[index] else {
                        continue;
                    };
                    if self.hover_entry == Some(index) {
                        paint.set_color(Color::from_argb(255, 226, 233, 246));
                        canvas.draw_round_rect(item_rect, 6.0 * scale, 6.0 * scale, &paint);
                    }
                    paint.set_color(Color::from_argb(255, 30, 30, 34));
                    canvas.draw_str(
                        label,
                        (
                            panel_rect.left + metrics.panel_pad_x,
                            item_rect.top + metrics.item_baseline,
                        ),
                        self.item_font(),
                        &paint,
                    );
                    if let Some(shortcut) = shortcut {
                        let shortcut_width = self.item_font().measure_str(shortcut, None).0;
                        paint.set_color(Color::from_argb(255, 112, 116, 124));
                        canvas.draw_str(
                            shortcut,
                            (
                                panel_rect.right - metrics.panel_pad_x - shortcut_width,
                                item_rect.top + metrics.item_baseline,
                            ),
                            self.item_font(),
                            &paint,
                        );
                    }
                }
            }
        }
    }
}

enum MenuInteraction {
    PassThrough,
    Consumed { needs_redraw: bool },
    Command(HostCommand),
}

type RenderSurface = softbuffer::Surface<OwnedDisplayHandle, std::sync::Arc<Window>>;

struct WindowState {
    surface: RenderSurface,
    window: std::sync::Arc<Window>,
    cursor_position: Option<PhysicalPosition<f64>>,

    left_down: bool,
    modifiers: ModifiersState,
    focused: bool,
    occluded: bool,
}

impl WindowState {
    fn new(
        context: &softbuffer::Context<OwnedDisplayHandle>,
        window: Window,
    ) -> Result<Self, Box<dyn Error>> {
        let window = std::sync::Arc::new(window);
        let surface = softbuffer::Surface::new(context, window.clone())?;
        let size = window.inner_size();
        let mut state = Self {
            surface,
            window,
            cursor_position: None,
            left_down: false,
            modifiers: ModifiersState::empty(),
            focused: true,
            occluded: false,
        };
        state.resize(size)?;
        state.window.set_ime_allowed(true);
        Ok(state)
    }

    fn resize(&mut self, size: PhysicalSize<u32>) -> Result<(), Box<dyn Error>> {
        let Some(width) = NonZeroU32::new(size.width) else {
            return Ok(());
        };
        let Some(height) = NonZeroU32::new(size.height) else {
            return Ok(());
        };
        self.surface.resize(width, height)?;
        self.window.request_redraw();
        Ok(())
    }

    fn cursor_or_center(&self) -> (f32, f32) {
        if let Some(position) = self.cursor_position {
            return (position.x as f32, position.y as f32);
        }

        let size = self.window.inner_size();
        (size.width as f32 * 0.5, size.height as f32 * 0.5)
    }

    fn draw(&mut self, engine: &mut Engine, menu: &mut MenuBar) -> Result<bool, Box<dyn Error>> {
        if self.occluded {
            return Ok(false);
        }

        let size = self.window.inner_size();
        let Some(width) = NonZeroU32::new(size.width) else {
            return Ok(false);
        };
        let Some(height) = NonZeroU32::new(size.height) else {
            return Ok(false);
        };
        self.surface.resize(width, height)?;

        let mut buffer = self.surface.buffer_mut()?;
        let buffer_width = buffer.width().get();
        let buffer_height = buffer.height().get();
        let row_bytes = buffer_width as usize * 4;
        let pixels = &mut *buffer;
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(pixels.as_mut_ptr() as *mut u8, pixels.len() * 4)
        };

        let image_info = ImageInfo::new(
            (buffer_width as i32, buffer_height as i32),
            ColorType::BGRA8888,
            AlphaType::Opaque,
            ColorSpace::new_srgb(),
        );
        let mut skia_surface = surfaces::wrap_pixels(&image_info, bytes, row_bytes, None)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "skia wrap_pixels"))?;
        let scale = self.window.scale_factor() as f32;
        let menu_height = menu.bar_height(scale);
        skia_surface.canvas().save();
        skia_surface.canvas().translate((0.0, menu_height));
        skia_surface.canvas().clip_rect(
            Rect::from_xywh(
                0.0,
                0.0,
                buffer_width as f32,
                (buffer_height as f32 - menu_height).max(1.0),
            ),
            None,
            true,
        );
        let reconciling = engine.draw(
            skia_surface.canvas(),
            buffer_width as f32,
            (buffer_height as f32 - menu_height).max(1.0),
            scale,
        );
        skia_surface.canvas().restore();
        menu.paint(skia_surface.canvas(), buffer_width as f32, scale);
        drop(skia_surface);

        self.window.pre_present_notify();
        buffer.present()?;
        Ok(reconciling)
    }
}

struct WinitHost {
    options: Options,
    context: softbuffer::Context<OwnedDisplayHandle>,
    engine: Engine,
    worker: Worker,
    menu: MenuBar,
    window: Option<WindowState>,
    needs_redraw: bool,
    opened_initial_paths: bool,
    started_at: Instant,
}

impl WinitHost {
    fn new(
        options: Options,
        context: softbuffer::Context<OwnedDisplayHandle>,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Self {
        let engine = Engine::new();
        let worker = Worker::start(engine.worker());
        engine.set_wakes(proxy, worker.sender());
        worker.request_run();

        Self {
            options,
            context,
            engine,
            worker,
            menu: MenuBar::default(),
            window: None,
            needs_redraw: true,
            opened_initial_paths: false,
            started_at: Instant::now(),
        }
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Box<dyn Error>> {
        if self.window.is_some() {
            return Ok(());
        }

        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut attributes = Window::default_attributes()
            .with_title(self.options.title.clone())
            .with_inner_size(LogicalSize::new(
                self.options.initial_width as f64,
                self.options.initial_height as f64,
            ))
            .with_min_inner_size(LogicalSize::new(
                self.options.min_width as f64,
                self.options.min_height as f64,
            ));

        #[cfg(target_os = "linux")]
        {
            attributes = winit::platform::wayland::WindowAttributesExtWayland::with_name(
                attributes,
                self.options.app_id.clone(),
                "himark",
            );
        }

        let window = event_loop.create_window(attributes)?;
        self.window = Some(WindowState::new(&self.context, window)?);
        self.request_redraw();
        Ok(())
    }

    fn request_redraw(&mut self) {
        self.needs_redraw = true;
        if let Some(window) = &self.window {
            window.window.request_redraw();
        }
    }

    fn event_time(&self) -> f64 {
        self.started_at.elapsed().as_secs_f64()
    }

    fn render(&mut self) {
        let result = if let Some(window) = self.window.as_mut() {
            self.needs_redraw = false;
            window.draw(&mut self.engine, &mut self.menu)
        } else {
            return;
        };

        match result {
            Ok(reconciling) => {
                self.engine.record_latency(self.event_time());
                if reconciling {
                    self.request_redraw();
                }
            }
            Err(error) => eprintln!("render failed: {error}"),
        }
        self.update_ime_cursor_area();
    }

    fn drain(&mut self) {
        if self.engine.drain() {
            self.request_redraw();
            self.update_ime_cursor_area();
        }
    }

    fn event_changed(&mut self, changed: bool) {
        if changed {
            self.request_redraw();
            self.update_ime_cursor_area();
        }
    }

    fn update_ime_cursor_area(&mut self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };

        let ime_allowed = window.focused && self.engine.has_text_focus();
        window.window.set_ime_allowed(ime_allowed);
        if !ime_allowed {
            return;
        }

        if let Some(rect) = self.engine.ime_cursor_rect() {
            let menu_height = self.menu.bar_height(window.window.scale_factor() as f32);
            window.window.set_ime_cursor_area(
                PhysicalPosition::new(rect.x as f64, rect.y as f64 + menu_height as f64),
                PhysicalSize::new(rect.width.max(1.0) as u32, rect.height.max(1.0) as u32),
            );
        }
    }

    fn content_point(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let scale = self.window.as_ref()?.window.scale_factor() as f32;
        let menu_height = self.menu.bar_height(scale);
        let content_y = y - menu_height;
        (content_y >= 0.0).then_some((x, content_y))
    }

    fn handle_key_event(&mut self, event: KeyEvent) {
        if !event.state.is_pressed() {
            return;
        }

        if self.primary_shortcut_down()
            && !self.window.as_ref().is_some_and(|w| w.modifiers.alt_key())
        {
            if let Key::Character(text) = event.logical_key.as_ref() {
                if matches!(text.to_ascii_lowercase().as_str(), "c" | "x" | "v")
                    && self.clipboard_shortcut(text)
                {
                    self.event_changed(true);
                    return;
                }
            }
        }

        if let Some(command) = self.shortcut_command(&event) {
            self.run_command(command);
            return;
        }

        if self.engine.has_marked_text() {
            return;
        }

        let dispatch = key_dispatch(
            event.logical_key.as_ref(),
            event.text.as_deref(),
            self.text_input_blocked_by_modifiers(),
        );
        let event_time = self.event_time();
        let mods = self.himark_mods();

        if let Some(code) = dispatch.key_code {
            if self.engine.key_down_at(code, mods, event_time) {
                self.event_changed(true);
                return;
            }
        }

        let changed = dispatch
            .fallback_text
            .is_some_and(|text| self.engine.text_input_at(&text, event_time));
        self.event_changed(changed);
    }

    fn himark_mods(&self) -> u32 {
        let Some(window) = self.window.as_ref() else {
            return 0;
        };
        let state = window.modifiers;
        let mut mods = 0;
        if state.shift_key() {
            mods |= himark_api::HIMARK_MOD_SHIFT;
        }
        if state.control_key() {
            mods |= himark_api::HIMARK_MOD_CONTROL;
        }
        if state.alt_key() {
            mods |= himark_api::HIMARK_MOD_ALT;
        }
        if state.super_key() {
            mods |= himark_api::HIMARK_MOD_COMMAND;
        }
        mods
    }

    fn handle_ime_event(&mut self, event: Ime) {
        let event_time = self.event_time();
        let changed = match event {
            Ime::Enabled => false,
            Ime::Preedit(text, selected) => {
                if text.is_empty() {
                    self.engine.unmark_text()
                } else {
                    let selected = selected
                        .map(|(start, end)| {
                            let start = byte_to_utf16_in(&text, start);
                            let end = byte_to_utf16_in(&text, end);
                            himark_api::HimarkRange {
                                start,
                                length: end.saturating_sub(start),
                            }
                        })
                        .unwrap_or_else(|| {
                            let end = text.encode_utf16().count().min(u32::MAX as usize) as u32;
                            himark_api::HimarkRange {
                                start: end,
                                length: 0,
                            }
                        });
                    self.engine.set_marked_text(&text, selected, None)
                }
            }
            Ime::Commit(text) => {
                let unmarked = self.engine.unmark_text();
                unmarked | self.engine.text_input_at(&text, event_time)
            }
            Ime::Disabled => self.engine.unmark_text(),
        };
        self.event_changed(changed);
    }

    fn handle_scroll(&mut self, delta: MouseScrollDelta) {
        let (x, y) = self
            .window
            .as_ref()
            .map_or((0.0, 0.0), WindowState::cursor_or_center);
        let Some((x, y)) = self.content_point(x, y) else {
            return;
        };
        let (delta_x, delta_y) = match delta {
            MouseScrollDelta::LineDelta(delta_x, delta_y) => {
                (-delta_x * LINE_SCROLL_PX, -delta_y * LINE_SCROLL_PX)
            }
            MouseScrollDelta::PixelDelta(delta) => (-delta.x as f32, -delta.y as f32),
        };

        if delta_x != 0.0 || delta_y != 0.0 {
            let changed = self
                .engine
                .scroll_at_time(x, y, delta_x, delta_y, self.event_time());
            self.event_changed(changed);
        }
    }

    fn clipboard_shortcut(&mut self, key: &str) -> bool {
        static PASTEBOARD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
        if key.eq_ignore_ascii_case("c") {
            match self.engine.clipboard_copy() {
                Some(text) => {
                    *PASTEBOARD.lock().expect("pasteboard") = Some(text);
                    true
                }
                None => false,
            }
        } else if key.eq_ignore_ascii_case("x") {
            match self.engine.clipboard_cut() {
                Some(text) => {
                    *PASTEBOARD.lock().expect("pasteboard") = Some(text);
                    true
                }
                None => false,
            }
        } else if key.eq_ignore_ascii_case("v") {
            let text = PASTEBOARD.lock().expect("pasteboard").clone();
            match text {
                Some(text) => self.engine.clipboard_paste(&text),
                None => false,
            }
        } else {
            false
        }
    }

    fn shortcut_command(&self, event: &KeyEvent) -> Option<HostCommand> {
        if !self.primary_shortcut_down() || self.window.as_ref()?.modifiers.alt_key() {
            return None;
        }

        let Key::Character(text) = event.logical_key.as_ref() else {
            return None;
        };
        if text.eq_ignore_ascii_case("m") {
            if self.window.as_ref()?.modifiers.shift_key() {
                Some(HostCommand::OpenDemoWall)
            } else {
                Some(HostCommand::OpenDemo)
            }
        } else if text.eq_ignore_ascii_case("o") {
            Some(HostCommand::OpenInstructions)
        } else {
            None
        }
    }

    fn primary_shortcut_down(&self) -> bool {
        let Some(window) = self.window.as_ref() else {
            return false;
        };

        #[cfg(target_os = "macos")]
        {
            window.modifiers.super_key()
        }
        #[cfg(not(target_os = "macos"))]
        {
            window.modifiers.control_key()
        }
    }

    fn text_input_blocked_by_modifiers(&self) -> bool {
        let Some(window) = self.window.as_ref() else {
            return false;
        };

        window.modifiers.super_key()
            || (window.modifiers.control_key() && !window.modifiers.alt_key())
    }

    fn run_command(&mut self, command: HostCommand) {
        let _ = self.menu.close();
        let changed = match command {
            HostCommand::Command(id) => self.engine.perform_command(id),
            HostCommand::OpenDemo => {
                self.engine.open_demo();
                false
            }
            HostCommand::OpenDemoWall => {
                self.engine.open_demo_wall();
                false
            }
            HostCommand::OpenInstructions => {
                eprintln!("Open files by passing paths on the command line or dropping files onto the window.");
                false
            }
        };
        self.event_changed(changed);
    }

    fn open_paths(&mut self, paths: Vec<PathBuf>) {
        let documents = documents_in_paths(paths);
        if documents.is_empty() {
            return;
        }

        let mut changed = false;
        for (index, document) in documents.into_iter().enumerate() {
            changed |= self
                .engine
                .open_document(&document.name, &document.source, index == 0);
        }
        self.event_changed(changed);
    }
}

impl ApplicationHandler<UserEvent> for WinitHost {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.create_window(event_loop) {
            eprintln!("failed to create window: {error}");
            event_loop.exit();
            return;
        }

        if !self.opened_initial_paths {
            self.opened_initial_paths = true;
            let open_paths = std::mem::take(&mut self.options.open_paths);
            self.open_paths(open_paths);
        }
        self.update_ime_cursor_area();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Drain => self.drain(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if !self
            .window
            .as_ref()
            .is_some_and(|window| window.window.id() == window_id)
        {
            return;
        }

        match event {
            WindowEvent::Resized(size) => {
                if let Some(window) = self.window.as_mut() {
                    if let Err(error) = window.resize(size) {
                        eprintln!("resize failed: {error}");
                    }
                }
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => self.request_redraw(),
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Destroyed => self.window = None,
            WindowEvent::Focused(focused) => {
                if let Some(window) = self.window.as_mut() {
                    window.focused = focused;
                }
                if !focused && self.menu.close() {
                    self.request_redraw();
                }
                self.update_ime_cursor_area();
            }
            WindowEvent::Occluded(occluded) => {
                if let Some(window) = self.window.as_mut() {
                    window.occluded = occluded;
                    if !occluded {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let dragging = self.window.as_ref().is_some_and(|window| window.left_down);
                if let Some(window) = self.window.as_mut() {
                    window.cursor_position = Some(position);
                }
                if self.menu.handle_cursor_move(
                    position,
                    self.window
                        .as_ref()
                        .map_or(1.0, |window| window.window.scale_factor()),
                ) {
                    self.request_redraw();
                }
                if dragging {
                    if let Some((x, y)) = self.content_point(position.x as f32, position.y as f32) {
                        let mods = self.himark_mods();
                        let changed = self.engine.mouse_drag(x, y, mods);
                        self.event_changed(changed);
                    }
                } else if let Some((x, y)) =
                    self.content_point(position.x as f32, position.y as f32)
                {
                    let changed = self.engine.mouse_move(x, y);
                    self.event_changed(changed);
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(window) = self.window.as_mut() {
                    window.cursor_position = None;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let Some(position) = self
                    .window
                    .as_ref()
                    .and_then(|window| window.cursor_position)
                    .map(|position| (position.x as f32, position.y as f32))
                else {
                    return;
                };
                let scale = self
                    .window
                    .as_ref()
                    .map_or(1.0, |window| window.window.scale_factor());
                match self.menu.handle_mouse_down(
                    PhysicalPosition::new(position.0 as f64, position.1 as f64),
                    scale,
                ) {
                    MenuInteraction::PassThrough => {
                        let Some((x, y)) = self.content_point(position.0, position.1) else {
                            return;
                        };
                        let mods = self.himark_mods();
                        if let Some(window) = self.window.as_mut() {
                            window.left_down = true;
                        }
                        let changed = self.engine.mouse_down_at(x, y, mods, self.event_time());
                        self.event_changed(changed);
                    }
                    MenuInteraction::Consumed { needs_redraw } => {
                        if needs_redraw {
                            self.request_redraw();
                        }
                    }
                    MenuInteraction::Command(command) => self.run_command(command),
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                let was_down = self.window.as_ref().is_some_and(|window| window.left_down);
                if let Some(window) = self.window.as_mut() {
                    window.left_down = false;
                }
                if was_down {
                    if let Some((x, y)) = self
                        .window
                        .as_ref()
                        .and_then(|window| window.cursor_position)
                        .map(|position| (position.x as f32, position.y as f32))
                        .and_then(|(x, y)| self.content_point(x, y))
                    {
                        let changed = self.engine.mouse_up(x, y);
                        self.event_changed(changed);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => self.handle_scroll(delta),
            WindowEvent::ModifiersChanged(modifiers) => {
                if let Some(window) = self.window.as_mut() {
                    window.modifiers = modifiers.state();
                }
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => self.handle_key_event(event),
            WindowEvent::Ime(event) => self.handle_ime_event(event),
            WindowEvent::DroppedFile(path) => self.open_paths(vec![path]),
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now_ms = self.started_at.elapsed().as_secs_f64() * 1000.0;
        let animating = self.engine.animation_tick(now_ms);
        if animating {
            self.request_redraw();
        }

        if self.needs_redraw {
            if let Some(window) = &self.window {
                window.window.request_redraw();
            }
        }

        if animating {
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + ANIMATION_FRAME));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.window = None;
        self.worker.stop();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostCommand {
    Command(&'static str),
    OpenDemo,
    OpenDemoWall,
    OpenInstructions,
}

struct HostDocument {
    name: String,
    source: String,
}

struct KeyDispatch {
    key_code: Option<u32>,
    fallback_text: Option<String>,
}

fn key_dispatch(logical_key: Key<&str>, text: Option<&str>, text_blocked: bool) -> KeyDispatch {
    let (key_code, insertable) = match logical_key {
        Key::Named(NamedKey::Space) => (Some(' ' as u32), text.or(Some(" "))),
        Key::Named(named) => (named_key_code(named), None),
        Key::Character(character) => {
            let code = character
                .chars()
                .next()
                .map(|ch| ch as u32)
                .filter(|code| *code >= himark_api::HIMARK_KEY_CHAR_BASE);
            (code, text)
        }
        Key::Dead(_) => (None, text),
        Key::Unidentified(_) => (None, None),
    };

    let fallback_text = insertable
        .filter(|_| !text_blocked)
        .filter(|text| is_insertable_text(text))
        .map(str::to_owned);

    KeyDispatch {
        key_code,
        fallback_text,
    }
}

fn named_key_code(key: NamedKey) -> Option<u32> {
    Some(match key {
        NamedKey::Backspace => himark_api::HIMARK_KEY_BACKSPACE,
        NamedKey::Enter => himark_api::HIMARK_KEY_ENTER,
        NamedKey::ArrowLeft => himark_api::HIMARK_KEY_LEFT,
        NamedKey::ArrowRight => himark_api::HIMARK_KEY_RIGHT,
        NamedKey::ArrowUp => himark_api::HIMARK_KEY_UP,
        NamedKey::ArrowDown => himark_api::HIMARK_KEY_DOWN,
        NamedKey::Escape => himark_api::HIMARK_KEY_ESCAPE,
        NamedKey::Tab => himark_api::HIMARK_KEY_TAB,
        NamedKey::Home => himark_api::HIMARK_KEY_HOME,
        NamedKey::End => himark_api::HIMARK_KEY_END,
        NamedKey::PageUp => himark_api::HIMARK_KEY_PAGE_UP,
        NamedKey::PageDown => himark_api::HIMARK_KEY_PAGE_DOWN,
        NamedKey::Delete => himark_api::HIMARK_KEY_DELETE,
        NamedKey::F1 => himark_api::HIMARK_KEY_F1,
        NamedKey::F2 => himark_api::HIMARK_KEY_F1 + 1,
        NamedKey::F3 => himark_api::HIMARK_KEY_F1 + 2,
        NamedKey::F4 => himark_api::HIMARK_KEY_F1 + 3,
        NamedKey::F5 => himark_api::HIMARK_KEY_F1 + 4,
        NamedKey::F6 => himark_api::HIMARK_KEY_F1 + 5,
        NamedKey::F7 => himark_api::HIMARK_KEY_F1 + 6,
        NamedKey::F8 => himark_api::HIMARK_KEY_F1 + 7,
        NamedKey::F9 => himark_api::HIMARK_KEY_F1 + 8,
        NamedKey::F10 => himark_api::HIMARK_KEY_F1 + 9,
        NamedKey::F11 => himark_api::HIMARK_KEY_F1 + 10,
        NamedKey::F12 => himark_api::HIMARK_KEY_F1 + 11,
        _ => return None,
    })
}

fn documents_in_paths(paths: Vec<PathBuf>) -> Vec<HostDocument> {
    let mut documents = Vec::new();
    for path in paths {
        collect_documents(&path, &mut documents);
    }
    documents
}

fn collect_documents(path: &Path, documents: &mut Vec<HostDocument>) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };

    if metadata.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            collect_documents(&entry.path(), documents);
        }
    } else if is_openable(path) {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        let Ok(source) = fs::read_to_string(path) else {
            return;
        };
        documents.push(HostDocument {
            name: name.to_owned(),
            source,
        });
    }
}

fn is_openable(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "md" | "markdown" | "rs" | "txt"
    )
}

fn byte_to_utf16_in(text: &str, byte: usize) -> u32 {
    let byte = clamp_to_char_boundary(text, byte.min(text.len()));
    text[..byte].encode_utf16().count().min(u32::MAX as usize) as u32
}

fn clamp_to_char_boundary(text: &str, mut byte: usize) -> usize {
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winit_registers_the_files_tree_capability() {
        let mut engine = Engine::new();
        assert!(engine.perform_command("files.tree"));
    }

    #[test]
    fn space_reaches_the_engine_as_a_key_and_then_as_text() {
        let unblocked = key_dispatch(Key::Named(NamedKey::Space), Some(" "), false);
        assert_eq!(unblocked.key_code, Some(' ' as u32));
        assert_eq!(unblocked.fallback_text.as_deref(), Some(" "));

        let blocked = key_dispatch(Key::Named(NamedKey::Space), Some(" "), true);
        assert_eq!(blocked.key_code, Some(' ' as u32));
        assert_eq!(blocked.fallback_text, None);
    }

    #[test]
    fn space_without_winit_text_still_inserts_a_space() {
        let dispatch = key_dispatch(Key::Named(NamedKey::Space), None, false);
        assert_eq!(dispatch.key_code, Some(' ' as u32));
        assert_eq!(dispatch.fallback_text.as_deref(), Some(" "));
    }

    #[test]
    fn characters_reach_the_engine_as_a_key_before_text_input() {
        let unblocked = key_dispatch(Key::Character("a"), Some("a"), false);
        assert_eq!(unblocked.key_code, Some('a' as u32));
        assert_eq!(unblocked.fallback_text.as_deref(), Some("a"));

        let blocked = key_dispatch(Key::Character("a"), Some("a"), true);
        assert_eq!(blocked.key_code, Some('a' as u32));
        assert_eq!(blocked.fallback_text, None);
    }

    #[test]
    fn named_keys_never_fall_back_to_text() {
        let dispatch = key_dispatch(Key::Named(NamedKey::Enter), Some("\r"), false);
        assert_eq!(dispatch.key_code, Some(himark_api::HIMARK_KEY_ENTER));
        assert_eq!(dispatch.fallback_text, None);
    }

    #[test]
    fn bare_modifier_keys_dispatch_nothing() {
        let dispatch = key_dispatch(Key::Named(NamedKey::Shift), None, false);
        assert_eq!(dispatch.key_code, None);
        assert_eq!(dispatch.fallback_text, None);
    }

    #[test]
    fn dead_keys_only_insert_their_composed_text() {
        let dispatch = key_dispatch(Key::Dead(None), Some("é"), false);
        assert_eq!(dispatch.key_code, None);
        assert_eq!(dispatch.fallback_text.as_deref(), Some("é"));
    }

    #[test]
    fn control_characters_are_neither_a_key_nor_text() {
        let dispatch = key_dispatch(Key::Character("\u{1}"), Some("\u{1}"), false);
        assert_eq!(dispatch.key_code, None);
        assert_eq!(dispatch.fallback_text, None);
    }
}
