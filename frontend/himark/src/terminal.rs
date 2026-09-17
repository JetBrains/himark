// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as TermPoint};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as TermColor, CursorShape, NamedColor, Processor, Rgb};

use imba::event::{Event, EventResult, Key, Modifiers};
use imba::{arena::Arena, constraints::Constraints, store::Store, UiCtx, View, Widget};
use skia_safe::{Canvas, Color, Font, Paint, Point, Rect, Size};

use crate::PanelView;

pub trait TerminalBackend: Send + Sync {
    fn write(&self, bytes: &[u8]);

    fn resize(&self, cols: u16, rows: u16, px_width: f32, px_height: f32);

    fn hangup(&self);
}

#[derive(Clone)]
struct Collector(Arc<Mutex<Vec<TermEvent>>>);

impl EventListener for Collector {
    fn send_event(&self, event: TermEvent) {
        self.0.lock().expect("collector lock").push(event);
    }
}

pub struct Session {
    term: FairMutex<Term<Collector>>,
    parser: Mutex<Processor>,
    events: Arc<Mutex<Vec<TermEvent>>>,
    backend: Box<dyn TerminalBackend>,
    title: Mutex<String>,
    exited: Mutex<Option<i32>>,
    hung_up: AtomicBool,

    told: Mutex<(u16, u16)>,

    channel: Mutex<Option<String>>,
}

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

impl Session {
    pub fn new(backend: Box<dyn TerminalBackend>) -> Arc<Self> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let size = TermSize::new(DEFAULT_COLS as usize, DEFAULT_ROWS as usize);
        let term = Term::new(Config::default(), &size, Collector(events.clone()));
        Arc::new(Self {
            term: FairMutex::new(term),
            parser: Mutex::new(Processor::new()),
            events,
            backend,
            title: Mutex::new(String::new()),
            exited: Mutex::new(None),
            hung_up: AtomicBool::new(false),
            told: Mutex::new((DEFAULT_COLS, DEFAULT_ROWS)),
            channel: Mutex::new(None),
        })
    }

    pub fn set_channel(&self, uri: String) {
        *self.channel.lock().expect("channel lock") = Some(uri);
    }

    pub fn channel(&self) -> Option<String> {
        self.channel.lock().expect("channel lock").clone()
    }

    pub fn output(&self, bytes: &[u8]) -> bool {
        {
            let mut parser = self.parser.lock().expect("parser lock");
            let mut term = self.term.lock();
            parser.advance(&mut *term, bytes);
        }
        self.drain_events();
        true
    }

    pub fn exited(&self, code: i32) -> bool {
        *self.exited.lock().expect("exit lock") = Some(code);
        true
    }

    fn drain_events(&self) {
        let events: Vec<TermEvent> = std::mem::take(&mut *self.events.lock().expect("events"));
        for event in events {
            match event {
                TermEvent::PtyWrite(text) => self.write(text.as_bytes()),
                TermEvent::Title(title) => {
                    *self.title.lock().expect("title lock") = title;
                }
                TermEvent::ResetTitle => self.title.lock().expect("title lock").clear(),
                TermEvent::TextAreaSizeRequest(format) => {
                    let (cols, rows) = *self.told.lock().expect("told lock");
                    let reply = format(WindowSize {
                        num_lines: rows,
                        num_cols: cols,
                        cell_width: 0,
                        cell_height: 0,
                    });
                    self.write(reply.as_bytes());
                }

                _ => {}
            }
        }
    }

    fn write(&self, bytes: &[u8]) {
        if self.exited.lock().expect("exit lock").is_none() {
            self.backend.write(bytes);
        }
    }

    fn resize(&self, cols: u16, rows: u16, px_width: f32, px_height: f32) {
        let cols = cols.max(2);
        let rows = rows.max(2);
        {
            let mut told = self.told.lock().expect("told lock");
            if *told == (cols, rows) {
                return;
            }
            *told = (cols, rows);
        }
        self.term
            .lock()
            .resize(TermSize::new(cols as usize, rows as usize));
        self.backend.resize(cols, rows, px_width, px_height);
    }

    pub fn hangup(&self) {
        if !self.hung_up.swap(true, Ordering::SeqCst) {
            self.backend.hangup();
        }
    }

    fn scroll_lines(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.hangup();
    }
}

pub enum TerminalCommand {
    Resize {
        cols: u16,
        rows: u16,
        px_width: f32,
        px_height: f32,
    },

    Scroll {
        lines: i32,
    },
}

#[derive(Clone)]
pub struct TerminalView {
    channel: String,

    number: u64,
}

impl TerminalView {
    pub fn new(channel: String) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            channel,
            number: NEXT.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn channel(&self) -> &str {
        &self.channel
    }
}

impl View for TerminalView {
    type Command = TerminalCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        _ui: &'w imba::UiCtx,
    ) -> imba::focus::FocusData<'w, TerminalCommand> {
        use imba::event::EventResult;
        let Some(session) = Terminals::session_ref(store, &self.channel) else {
            return imba::focus::FocusData::default();
        };
        imba::focus::FocusData {
            on_key: Some(Box::new(move |key, mods| {
                let app_cursor = session.term.lock().mode().contains(TermMode::APP_CURSOR);
                match encode_key(key, mods, app_cursor) {
                    Some(bytes) => {
                        session.write(&bytes);
                        EventResult::Handled
                    }
                    None => EventResult::Ignored,
                }
            })),
            on_text: Some(Box::new(move |text| {
                session.write(text.as_bytes());
                EventResult::Handled
            })),
            clipboard: Some(Box::new(move |visit| {
                struct PtyPaste<'a> {
                    session: &'a Session,
                }
                impl imba::ClipboardClient for PtyPaste<'_> {
                    fn copy(&mut self) -> Option<imba::ClipboardContent> {
                        None
                    }
                    fn cut(&mut self) -> Option<imba::ClipboardContent> {
                        None
                    }
                    fn paste(&mut self, content: &imba::ClipboardContent) -> bool {
                        self.session.write(content.text.as_bytes());
                        true
                    }
                }
                visit(&mut PtyPaste { session });
                EventResult::Handled
            })),
            ..imba::focus::FocusData::default()
        }
    }
    fn perform(
        &mut self,
        store: &mut Store,
        _ui: &UiCtx,
        command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        let Some(session) = Terminals::session(store, &self.channel) else {
            return;
        };
        match command {
            TerminalCommand::Resize {
                cols,
                rows,
                px_width,
                px_height,
            } => session.resize(cols, rows, px_width, px_height),
            TerminalCommand::Scroll { lines } => session.scroll_lines(lines),
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let Some(session) = Terminals::session_ref(store, &self.channel) else {
                let blank = imba::ThunkBox::new(
                    arena,
                    imba::leaf::leaf(constraints.max.width, constraints.max.height),
                );
                return blank;
            };
            let chrome = crate::env::Themes::of(store).ui().terminal.clone();
            let font = grid_font(ui, &chrome);
            let cell = cell_metrics(&font);
            let fallbacks = ui.env(|| FallbackFaces {
                manager: ui
                    .get::<crate::env::UiFonts>()
                    .and_then(|fonts| fonts.0.fallback_manager())
                    .unwrap_or_else(skia_safe::FontMgr::new),
                by_char: Default::default(),
            });
            imba::ThunkBox::new(
                arena,
                imba::eager(TerminalWidget {
                    session,
                    size: constraints.max,
                    chrome,
                    font,
                    cell,
                    fallbacks,
                    surface: imba::event::ScrollSurfaceId::keyed(self.number),
                }),
            )
        })
    }
}

impl PanelView for TerminalView {
    type Place = crate::NoPlace;

    fn title(&self, store: &Store) -> String {
        let title = Terminals::session_ref(store, &self.channel)
            .map(|session| session.title.lock().expect("title lock").clone())
            .unwrap_or_default();
        match title.is_empty() {
            true => format!("Terminal {}", self.number),
            false => title,
        }
    }

    fn dismantle(&mut self, store: &mut Store) {
        if let Some(session) = Terminals::session(store, &self.channel) {
            session.hangup();
        }
        Terminals::remove(store, &self.channel);
    }

    fn family_row(&self) -> Option<crate::FamilyRow> {
        Some(crate::FamilyRow::Terminal(self.channel.clone()))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Default)]
pub struct Terminals(rpds::HashTrieMapSync<String, Arc<Session>>);

impl Terminals {
    pub fn put(store: &mut Store, channel: String, session: Arc<Session>) {
        store.update::<Terminals>(|terminals| {
            terminals.0.insert_mut(channel, session);
        });
    }

    pub fn session(store: &Store, channel: &str) -> Option<Arc<Session>> {
        store
            .get::<Terminals>()
            .and_then(|terminals| terminals.0.get(channel).cloned())
    }

    pub fn session_ref<'a>(store: &'a Store, channel: &str) -> Option<&'a Arc<Session>> {
        store
            .get::<Terminals>()
            .and_then(|terminals| terminals.0.get(channel))
    }

    pub fn remove(store: &mut Store, channel: &str) {
        store.update::<Terminals>(|terminals| {
            terminals.0.remove_mut(channel);
        });
    }

    pub fn list(store: &Store) -> Vec<String> {
        store
            .get::<Terminals>()
            .map(|terminals| terminals.0.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

struct TerminalTypeface(skia_safe::Typeface);

struct FallbackFaces {
    manager: skia_safe::FontMgr,
    by_char: std::cell::RefCell<std::collections::HashMap<char, Option<skia_safe::Typeface>>>,
}

impl FallbackFaces {
    fn font_for(&self, c: char, size: f32) -> Option<Font> {
        let mut cache = self.by_char.borrow_mut();
        let face = cache
            .entry(c)
            .or_insert_with(|| {
                self.manager.match_family_style_character(
                    "",
                    skia_safe::FontStyle::normal(),
                    &[],
                    c as i32,
                )
            })
            .clone()?;
        let mut font = Font::from_typeface(face, size);
        font.set_edging(skia_safe::font::Edging::AntiAlias);
        Some(font)
    }
}

fn grid_font(ui: &UiCtx, chrome: &crate::theme::TerminalChrome) -> Font {
    let families = chrome.font_families.clone();
    let typeface = ui.env(|| {
        let face = crate::env::ui_typeface(ui, &families, skia_safe::FontStyle::normal())
            .expect("a monospace typeface");
        TerminalTypeface(face)
    });
    let mut font = Font::from_typeface(typeface.0.clone(), chrome.font_size);
    font.set_edging(skia_safe::font::Edging::AntiAlias);
    font
}

fn cell_metrics(font: &Font) -> (f32, f32, f32) {
    let (_, metrics) = font.metrics();
    let advance = font.measure_str("M", None).0;
    let height = (metrics.descent - metrics.ascent + metrics.leading).ceil();
    (advance, height, -metrics.ascent)
}

struct TerminalWidget<'a> {
    session: &'a Session,
    size: Size,
    chrome: crate::theme::TerminalChrome,
    font: Font,
    cell: (f32, f32, f32),
    fallbacks: &'a FallbackFaces,

    surface: imba::event::ScrollSurfaceId,
}

impl TerminalWidget<'_> {
    fn grid_for(&self) -> (u16, u16) {
        let (cell_w, cell_h, _) = self.cell;
        let inner_w = (self.size.width - self.chrome.pad * 2.0).max(cell_w);
        let inner_h = (self.size.height - self.chrome.pad * 2.0).max(cell_h);
        (
            (inner_w / cell_w).floor().max(2.0) as u16,
            (inner_h / cell_h).floor().max(2.0) as u16,
        )
    }

    fn color(&self, color: TermColor, foreground: bool) -> Color {
        match color {
            TermColor::Spec(Rgb { r, g, b }) => Color::from_argb(0xff, r, g, b),
            TermColor::Indexed(index) => self.indexed(index as usize, foreground),
            TermColor::Named(named) => match named {
                NamedColor::Foreground => self.chrome.foreground.0,
                NamedColor::Background => self.chrome.background.0,
                NamedColor::Cursor => self.chrome.cursor.0,
                named => self.indexed(named as usize, foreground),
            },
        }
    }

    fn indexed(&self, index: usize, foreground: bool) -> Color {
        if let Some(color) = self.chrome.palette.get(index) {
            return color.0;
        }

        if (16..=231).contains(&index) {
            let index = index - 16;
            let level = |value: usize| match value {
                0 => 0u8,
                v => (v * 40 + 55) as u8,
            };
            return Color::from_argb(
                0xff,
                level(index / 36),
                level(index % 36 / 6),
                level(index % 6),
            );
        }
        if (232..=255).contains(&index) {
            let gray = ((index - 232) * 10 + 8) as u8;
            return Color::from_argb(0xff, gray, gray, gray);
        }
        match foreground {
            true => self.chrome.foreground.0,
            false => self.chrome.background.0,
        }
    }

    fn paint_grid(&self, canvas: &Canvas, focused: bool) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(self.chrome.background.0);
        canvas.draw_rect(Rect::from_size(self.size), &paint);

        let (cell_w, cell_h, baseline) = self.cell;
        let origin = (self.chrome.pad, self.chrome.pad);
        let term = self.session.term.lock();
        let content = term.renderable_content();
        let cursor = content.cursor;
        let display_offset = content.display_offset as i32;

        let mut glyph_paint = Paint::default();
        glyph_paint.set_anti_alias(true);
        let mut run = String::new();
        let mut run_start: Option<(f32, f32, Color)> = None;
        let mut previous_line = None;

        for indexed in content.display_iter {
            let point = indexed.point;
            let cell = &indexed.cell;
            let line = (point.line.0 + display_offset) as f32;
            let x = origin.0 + point.column.0 as f32 * cell_w;
            let y = origin.1 + line * cell_h;

            if previous_line != Some(point.line) {
                flush_run(
                    canvas,
                    &self.font,
                    &mut glyph_paint,
                    baseline,
                    &mut run,
                    &mut run_start,
                );
                previous_line = Some(point.line);
            }
            if cell
                .flags
                .intersects(CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }

            let (mut fg, mut bg) = (self.color(cell.fg, true), self.color(cell.bg, false));
            if cell.flags.contains(CellFlags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if bg != self.chrome.background.0 {
                flush_run(
                    canvas,
                    &self.font,
                    &mut glyph_paint,
                    baseline,
                    &mut run,
                    &mut run_start,
                );
                paint.set_color(bg);
                let width = match cell.flags.contains(CellFlags::WIDE_CHAR) {
                    true => cell_w * 2.0,
                    false => cell_w,
                };
                canvas.draw_rect(Rect::from_xywh(x, y, width, cell_h), &paint);
            }

            if cell.c != ' ' && self.font.unichar_to_glyph(cell.c as i32) == 0 {
                flush_run(
                    canvas,
                    &self.font,
                    &mut glyph_paint,
                    baseline,
                    &mut run,
                    &mut run_start,
                );
                if let Some(font) = self.fallbacks.font_for(cell.c, self.chrome.font_size) {
                    let mut cluster = String::from(cell.c);
                    cluster.extend(cell.zerowidth().into_iter().flatten());
                    glyph_paint.set_color(fg);
                    canvas.draw_str(
                        cluster.as_str(),
                        Point::new(x, y + baseline),
                        &font,
                        &glyph_paint,
                    );
                }
                continue;
            }

            match run_start {
                Some((_, _, color)) if color == fg => {}
                _ => flush_run(
                    canvas,
                    &self.font,
                    &mut glyph_paint,
                    baseline,
                    &mut run,
                    &mut run_start,
                ),
            }
            if run_start.is_none() {
                run_start = Some((x, y, fg));
            }
            run.push(cell.c);

            run.extend(cell.zerowidth().into_iter().flatten());
            if cell.flags.contains(CellFlags::WIDE_CHAR) {
                run.push(' ');
            }
        }
        flush_run(
            canvas,
            &self.font,
            &mut glyph_paint,
            baseline,
            &mut run,
            &mut run_start,
        );

        if focused && cursor.shape != CursorShape::Hidden && display_offset == 0 {
            let x = origin.0 + cursor.point.column.0 as f32 * cell_w;
            let y = origin.1 + cursor.point.line.0 as f32 * cell_h;
            paint.set_color(self.chrome.cursor.0);
            canvas.draw_rect(Rect::from_xywh(x, y, cell_w, cell_h), &paint);
            if let Some(c) = cursor_cell(&term, cursor.point) {
                glyph_paint.set_color(self.chrome.background.0);
                canvas.draw_str(
                    c.to_string().as_str(),
                    Point::new(x, y + baseline),
                    &self.font,
                    &glyph_paint,
                );
            }
        }
    }
}

fn flush_run(
    canvas: &Canvas,
    font: &Font,
    paint: &mut Paint,
    baseline: f32,
    run: &mut String,
    start: &mut Option<(f32, f32, Color)>,
) {
    if let Some((x, y, color)) = start.take() {
        if !run.trim().is_empty() {
            paint.set_color(color);
            canvas.draw_str(run.as_str(), Point::new(x, y + baseline), font, paint);
        }
        run.clear();
    }
}

fn cursor_cell(term: &Term<Collector>, point: TermPoint) -> Option<char> {
    if point.line.0 < 0
        || point.line.0 >= term.screen_lines() as i32
        || point.column >= Column(term.columns())
    {
        return None;
    }
    let c = term.grid()[Line(point.line.0)][point.column].c;
    (c != ' ').then_some(c)
}

impl<'a> Widget<'a, TerminalCommand> for TerminalWidget<'a> {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<TerminalCommand> {
        match event {
            Event::Paint { canvas, focused } => {
                self.paint_grid(canvas, *focused);

                let (cols, rows) = self.grid_for();
                let told = *self.session.told.lock().expect("told lock");
                if told != (cols, rows) {
                    return EventResult::Command(TerminalCommand::Resize {
                        cols,
                        rows,
                        px_width: cols as f32 * self.cell.0,
                        px_height: rows as f32 * self.cell.1,
                    });
                }
                EventResult::Handled
            }
            Event::Scroll {
                delta_x,
                delta_y,
                gesture,
                ..
            } => {
                if gesture.owned_by_other(self.surface) {
                    return EventResult::Ignored;
                }
                if !gesture.owned_by(self.surface) {
                    if delta_x.abs() > delta_y.abs() {
                        return EventResult::Ignored;
                    }
                    let _ = gesture.claims(self.surface);
                }
                let lines = (-delta_y / self.cell.1).round() as i32;
                match lines {
                    0 => EventResult::Handled,
                    lines => EventResult::Command(TerminalCommand::Scroll { lines }),
                }
            }
            Event::MouseDown { .. } => EventResult::Handled,
            _ => EventResult::Ignored,
        }
    }
}

fn encode_key(key: Key, mods: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    let encoded: Vec<u8> = match key {
        Key::Enter => b"\r".to_vec(),
        Key::Backspace => vec![0x7f],
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::Escape => vec![0x1b],
        Key::Left | Key::Right | Key::Up | Key::Down => {
            let letter = match key {
                Key::Up => b'A',
                Key::Down => b'B',
                Key::Right => b'C',
                _ => b'D',
            };
            match app_cursor {
                true => vec![0x1b, b'O', letter],
                false => vec![0x1b, b'[', letter],
            }
        }
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::F(n @ 1..=4) => vec![0x1b, b'O', b'P' + n - 1],
        Key::F(n @ 5..=12) => {
            let code = match n {
                5 => 15,
                6..=10 => 11 + n as u32,
                _ => 12 + n as u32,
            };
            format!("\x1b[{code}~").into_bytes()
        }
        Key::F(_) => return None,
        Key::Char(c) if mods.control => {
            let c = c.to_ascii_lowercase();
            let byte = match c {
                'a'..='z' => c as u8 & 0x1f,
                '@' | ' ' => 0,
                '[' => 0x1b,
                '\\' => 0x1c,
                ']' => 0x1d,
                '^' => 0x1e,
                '_' | '/' => 0x1f,
                _ => return None,
            };
            match mods.alt {
                true => vec![0x1b, byte],
                false => vec![byte],
            }
        }
        Key::Char(c) if mods.alt => {
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(c.to_string().as_bytes());
            bytes
        }
        Key::Char(_) => return None,
    };
    Some(encoded)
}

#[cfg(test)]
mod tests;
