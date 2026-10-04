// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The terminal PANE: the grid painter, the xterm keymap, the
//! navigation place and the family row — the one face over the
//! collection in lib.rs. No window: where the pane lands is the
//! shell's business.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point as TermPoint};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as TermColor, CursorShape, NamedColor, Rgb};

use imba::event::{Event, EventResult, Key, Modifiers};
use imba::{arena::Arena, constraints::Constraints, store::Store, UiCtx, View, Widget};
use skia_safe::{Canvas, Color, Font, Paint, Point, Rect, Size};

use crate::{Collector, Session, TerminalId, Terminals};

/// A terminal pane's row: the collection and the terminal.
#[derive(Clone, PartialEq)]
pub struct TerminalRow(pub imba::store::Id<Terminals>, pub TerminalId);

impl hikit::Row for TerminalRow {}

/// Mint the pane for a live row — the navigator's and the row
/// minter's shared door.
fn mint_terminal(
    store: &Store,
    terminals: imba::store::Id<Terminals>,
    id: TerminalId,
) -> Option<Box<dyn hikit::DynPanelView>> {
    store
        .entity(terminals)
        .filter(|rows: &&Terminals| rows.holds(id))
        .map(|_| Box::new(TerminalView::new(terminals, id)) as Box<dyn hikit::DynPanelView>)
}

/// The registered walk-back minter for terminal rows.
pub fn terminal_row_minter() -> std::sync::Arc<hikit::RowMinter> {
    std::sync::Arc::new(|store, row| {
        let TerminalRow(terminals, id) = *row.row::<TerminalRow>()?;
        mint_terminal(store, terminals, id)
    })
}

#[derive(Clone)]
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

impl std::fmt::Display for TerminalCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TerminalCommand::Resize { .. } => out.write_str("terminal resize"),
            TerminalCommand::Scroll { .. } => out.write_str("terminal scroll"),
        }
    }
}

#[derive(Clone)]
pub struct TerminalView {
    /// The terminal family that OWNS this terminal — the id threaded
    /// at birth (docs/entities.md law 3); the view never learns what
    /// a session is.
    terminals: imba::store::Id<Terminals>,

    id: TerminalId,
}

impl TerminalView {
    pub fn new(terminals: imba::store::Id<Terminals>, id: TerminalId) -> Self {
        Self { terminals, id }
    }

    pub fn id(&self) -> TerminalId {
        self.id
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
        let Some(session) = Terminals::session_ref(store, self.terminals, self.id) else {
            return imba::focus::FocusData::default();
        };
        imba::focus::FocusData {
            on_key: Some(Box::new(move |key, mods| {
                let app_cursor = session.term().lock().mode().contains(TermMode::APP_CURSOR);
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
        let Some(session) = Terminals::session(store, self.terminals, self.id) else {
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
            let Some(session) = Terminals::session_ref(store, self.terminals, self.id) else {
                let blank = imba::ThunkBox::new(
                    arena,
                    imba::leaf::leaf(constraints.max.width, constraints.max.height),
                );
                return blank;
            };
            let chrome = editor::env::Themes::of(store).ui().terminal.clone();
            let font = grid_font(ui, &chrome);
            let cell = cell_metrics(&font);
            let fallbacks = ui.env(|| FallbackFaces {
                manager: ui
                    .get::<imba::UiFonts>()
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
                    surface: imba::event::ScrollSurfaceId::keyed(self.id.raw()),
                }),
            )
        })
    }
}

/// The terminal pane's navigation identity: its collection and its
/// terminal id — same shape as the chat's, and for the same reason:
/// a place is what makes leaving the pane walkable.
#[derive(Clone, PartialEq)]
pub struct TerminalPlace {
    pub terminals: imba::store::Id<Terminals>,
    pub id: TerminalId,
}

impl hikit::Place for TerminalPlace {}

/// The walk-back road: re-mint the pane off the family row while the
/// terminal session still stands.
pub struct TerminalNavigator;

impl hikit::Navigator for TerminalNavigator {
    type Place = TerminalPlace;

    fn navigate(
        &self,
        store: &mut Store,
        _ui: &imba::UiCtx,
        place: &TerminalPlace,
        _fx: &mut imba::command::Fx<'_>,
    ) -> Option<Box<dyn hikit::DynPanelView>> {
        mint_terminal(store, place.terminals, place.id)
    }
}

impl hikit::PanelView for TerminalView {
    type Place = TerminalPlace;

    fn navigation_location(&self, _store: &Store) -> Option<TerminalPlace> {
        Some(TerminalPlace {
            terminals: self.terminals,
            id: self.id,
        })
    }

    fn navigate_to(
        &mut self,
        _store: &mut Store,
        place: &TerminalPlace,
        _fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        place.terminals == self.terminals && place.id == self.id
    }

    fn title(&self, store: &Store) -> String {
        let title = Terminals::session_ref(store, self.terminals, self.id)
            .map(|session| session.title())
            .unwrap_or_default();
        match title.is_empty() {
            true => format!("Terminal {}", self.id.raw()),
            false => title,
        }
    }

    fn dismantle(&mut self, store: &mut Store) {
        if let Some(session) = Terminals::session(store, self.terminals, self.id) {
            session.hangup();
        }
        Terminals::remove(store, self.terminals, self.id);
    }

    fn family_row(&self) -> Option<hikit::FamilyRow> {
        Some(hikit::FamilyRow::new(crate::TerminalRow(
            self.terminals,
            self.id,
        )))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
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

fn grid_font(ui: &UiCtx, chrome: &editor::theme::TerminalChrome) -> Font {
    let families = chrome.font_families.clone();
    let typeface = ui.env(|| {
        let face = editor::env::ui_typeface(ui, &families, skia_safe::FontStyle::normal())
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
    chrome: editor::theme::TerminalChrome,
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
        let term = self.session.term().lock();
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
                let told = self.session.told();
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
mod tests {
    use super::*;

    #[test]
    fn keys_encode_like_xterm() {
        let plain = Modifiers::default();
        let ctrl = Modifiers {
            control: true,
            ..Default::default()
        };
        assert_eq!(encode_key(Key::Enter, plain, false), Some(b"\r".to_vec()));
        assert_eq!(encode_key(Key::Up, plain, false), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode_key(Key::Up, plain, true), Some(b"\x1bOA".to_vec()));
        assert_eq!(encode_key(Key::Char('c'), ctrl, false), Some(vec![0x03]));
        assert_eq!(encode_key(Key::Char('d'), ctrl, false), Some(vec![0x04]));
        assert_eq!(
            encode_key(Key::F(5), plain, false),
            Some(b"\x1b[15~".to_vec())
        );
        assert_eq!(encode_key(Key::Char('x'), plain, false), None);
    }
}
