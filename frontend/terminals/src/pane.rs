// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The terminal PANE: the grid painter, the input encoders' feeder,
//! the navigation place and the pane row — the one face over the
//! collection in lib.rs. No window: where the pane lands is the
//! shell's business.

use ghostty_sys::{
    GhosttyMouseButton, GHOSTTY_MOUSE_ACTION_MOTION, GHOSTTY_MOUSE_ACTION_PRESS,
    GHOSTTY_MOUSE_ACTION_RELEASE, GHOSTTY_MOUSE_BUTTON_FIVE, GHOSTTY_MOUSE_BUTTON_FOUR,
    GHOSTTY_MOUSE_BUTTON_LEFT, GHOSTTY_MOUSE_BUTTON_MIDDLE, GHOSTTY_MOUSE_BUTTON_RIGHT,
};
use imba::event::{Event, EventResult, Key, Modifiers, MouseButton};
use imba::{arena::Arena, constraints::Constraints, store::Store, ui::UiCtx, View, Widget};
use skia_safe::{Canvas, Color, Font, Paint, Point, Rect, Size};

use crate::vt::{Colors, CursorShape, Frame, Select};
use crate::{Press, Session, TerminalId, Terminals};

/// A terminal pane's row: the collection and the terminal.
#[derive(Clone, PartialEq)]
pub struct TerminalRow(pub imba::store::Id<Terminals>, pub TerminalId);

impl hikit::pane_row::Row for TerminalRow {}

/// Mint the pane for a live row — the navigator's and the row
/// minter's shared door.
fn mint_terminal(
    store: &Store,
    terminals: imba::store::Id<Terminals>,
    id: TerminalId,
) -> Option<Box<dyn hikit::panel::DynPanelView>> {
    store
        .entity(terminals)
        .filter(|rows: &&Terminals| rows.holds(id))
        .map(|_| Box::new(TerminalView::new(terminals, id)) as Box<dyn hikit::panel::DynPanelView>)
}

/// The registered walk-back minter for terminal rows.
pub fn terminal_row_minter() -> std::sync::Arc<hikit::panel::RowMinter> {
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

    /// One step of a mouse selection, in grid cells and grid pixels.
    Select {
        step: Select,
        col: u16,
        row: u16,
        x: f32,
        y: f32,
        cell_w: f32,
        height: f32,
    },
}

impl std::fmt::Display for TerminalCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TerminalCommand::Resize { .. } => out.write_str("terminal resize"),
            TerminalCommand::Scroll { .. } => out.write_str("terminal scroll"),
            TerminalCommand::Select { .. } => out.write_str("terminal select"),
        }
    }
}

#[derive(Clone)]
pub struct TerminalView {
    /// The terminal session that OWNS this terminal — the id threaded
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
        _ui: &'w imba::ui::UiCtx,
    ) -> imba::focus::FocusData<'w, TerminalCommand> {
        use imba::event::EventResult;
        let Some(session) = Terminals::session_ref(store, self.terminals, self.id) else {
            return imba::focus::FocusData::default();
        };
        imba::focus::FocusData {
            on_key: Some(Box::new(move |key, mods| {
                match session.key_input(key, mods) {
                    true => EventResult::handled(),
                    false => EventResult::Ignored,
                }
            })),
            on_text: Some(Box::new(move |text| {
                session.text_input(text);
                EventResult::handled()
            })),
            clipboard: Some(Box::new(move |visit| {
                struct PtyClipboard<'a> {
                    session: &'a Session,
                }
                impl imba::clipboard::ClipboardClient for PtyClipboard<'_> {
                    fn copy(&mut self) -> Option<imba::clipboard::ClipboardContent> {
                        self.session
                            .selection_text()
                            .map(|text| imba::clipboard::ClipboardContent { text })
                    }
                    fn cut(&mut self) -> Option<imba::clipboard::ClipboardContent> {
                        None
                    }
                    fn paste(&mut self, content: &imba::clipboard::ClipboardContent) -> bool {
                        self.session.paste(&content.text);
                        true
                    }
                }
                visit(&mut PtyClipboard { session });
                EventResult::handled()
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
            TerminalCommand::Select {
                step,
                col,
                row,
                x,
                y,
                cell_w,
                height,
            } => session.select(step, col, row, x, y, cell_w, height),
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let Some(session) = Terminals::session_ref(store, self.terminals, self.id) else {
                let blank = imba::ThunkBox::new(
                    arena,
                    imba::leaf::leaf(constraints.max.width, constraints.max.height),
                );
                return blank;
            };
            let chrome = editor::env::Themes::of(store).part(crate::chrome::TerminalChrome::KEY);
            session.set_colors(Colors::from_chrome(&chrome));
            let fonts = grid_fonts(ui, &chrome);
            let cell = cell_metrics(&fonts.normal);
            let fallbacks = ui.env(|| FallbackFaces {
                manager: skia_safe::FontMgr::new(),
                by_char: Default::default(),
            });
            imba::ThunkBox::new(
                arena,
                imba::eager(TerminalWidget {
                    session,
                    size: constraints.max,
                    chrome,
                    fonts,
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

impl hikit::navigation::Place for TerminalPlace {}

/// The walk-back road: re-mint the pane off the pane row while the
/// terminal session still stands.
pub struct TerminalNavigator;

impl hikit::navigation::Navigator for TerminalNavigator {
    type Place = TerminalPlace;

    fn navigate(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        place: &TerminalPlace,
        _fx: &mut imba::command::Fx<'_>,
    ) -> Option<Box<dyn hikit::panel::DynPanelView>> {
        mint_terminal(store, place.terminals, place.id)
    }
}

impl hikit::panel::PanelView for TerminalView {
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

    fn pane_row(&self) -> Option<hikit::pane_row::PaneRow> {
        Some(hikit::pane_row::PaneRow::new(TerminalRow(
            self.terminals,
            self.id,
        )))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct TerminalTypefaces {
    normal: skia_safe::Typeface,
    bold: skia_safe::Typeface,
    italic: skia_safe::Typeface,
    bold_italic: skia_safe::Typeface,
}

struct GridFonts {
    normal: Font,
    bold: Font,
    italic: Font,
    bold_italic: Font,
}

impl GridFonts {
    fn pick(&self, bold: bool, italic: bool) -> &Font {
        match (bold, italic) {
            (false, false) => &self.normal,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        }
    }
}

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

fn grid_fonts(ui: &UiCtx, chrome: &crate::chrome::TerminalChrome) -> GridFonts {
    let states = chrome.font_families.clone();
    let faces = ui.env(|| {
        let style = |style: skia_safe::FontStyle| editor::env::ui_typeface(ui, &states, style);
        let normal = style(skia_safe::FontStyle::normal()).expect("a monospace typeface");
        TerminalTypefaces {
            bold: style(skia_safe::FontStyle::bold()).unwrap_or_else(|| normal.clone()),
            italic: style(skia_safe::FontStyle::italic()).unwrap_or_else(|| normal.clone()),
            bold_italic: style(skia_safe::FontStyle::bold_italic())
                .unwrap_or_else(|| normal.clone()),
            normal,
        }
    });
    let font = |face: &skia_safe::Typeface| {
        let mut font = Font::from_typeface(face.clone(), chrome.font_size);
        font.set_edging(skia_safe::font::Edging::AntiAlias);
        font
    };
    GridFonts {
        normal: font(&faces.normal),
        bold: font(&faces.bold),
        italic: font(&faces.italic),
        bold_italic: font(&faces.bold_italic),
    }
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
    chrome: std::sync::Arc<crate::chrome::TerminalChrome>,
    fonts: GridFonts,
    cell: (f32, f32, f32),
    fallbacks: &'a FallbackFaces,

    surface: imba::event::ScrollSurfaceId,
}

/// A glyph run being gathered: where it starts, its color and its face.
struct Run {
    x: f32,
    y: f32,
    color: Color,
    bold: bool,
    italic: bool,
    text: String,
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

    /// The pointer's grid cell and grid-local pixel position.
    fn grid_point(&self, point: Point) -> (u16, u16, f32, f32) {
        let (cell_w, cell_h, _) = self.cell;
        let x = (point.x - self.chrome.pad).max(0.0);
        let y = (point.y - self.chrome.pad).max(0.0);
        ((x / cell_w) as u16, (y / cell_h) as u16, x, y)
    }

    /// The grid for mouse reports: columns, rows, cell pixels.
    fn report_size(&self) -> (u32, u32, f32, f32) {
        let (cols, rows) = self.session.told();
        (cols as u32, rows as u32, self.cell.0, self.cell.1)
    }

    fn select_command(&self, step: Select, point: Point) -> EventResult<TerminalCommand> {
        let (col, row, x, y) = self.grid_point(point);
        let (_, rows) = self.session.told();
        EventResult::command(TerminalCommand::Select {
            step,
            col,
            row,
            x,
            y,
            cell_w: self.cell.0,
            height: self.cell.1 * rows as f32,
        })
    }

    fn paint_grid(&self, canvas: &Canvas, focused: bool) {
        let frame = self.session.frame();
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(frame.bg);
        canvas.draw_rect(Rect::from_size(self.size), &paint);

        let (cell_w, cell_h, baseline) = self.cell;
        let origin = (self.chrome.pad, self.chrome.pad);

        let mut glyph_paint = Paint::default();
        glyph_paint.set_anti_alias(true);
        let mut run: Option<Run> = None;
        // Thin marks over the text: underlines and strikethroughs.
        let mut marks: Vec<(Rect, Color)> = Vec::new();
        let line = (cell_h / 16.0).max(1.0);

        for (row_index, row) in frame.rows.iter().enumerate() {
            flush_run(canvas, &self.fonts, &mut glyph_paint, baseline, &mut run);
            let y = origin.1 + row_index as f32 * cell_h;
            // A wide character's head covers its tail cell too.
            let mut skip_tail = false;
            for (column, cell) in row.iter().enumerate() {
                if std::mem::take(&mut skip_tail) {
                    continue;
                }
                skip_tail = cell.wide;
                let x = origin.0 + column as f32 * cell_w;
                let width = match cell.wide {
                    true => cell_w * 2.0,
                    false => cell_w,
                };

                if let Some(bg) = cell.bg {
                    if bg != frame.bg {
                        flush_run(canvas, &self.fonts, &mut glyph_paint, baseline, &mut run);
                        paint.set_color(bg);
                        canvas.draw_rect(Rect::from_xywh(x, y, width, cell_h), &paint);
                    }
                }

                let fg = match cell.faint {
                    true => cell.fg.with_a(160),
                    false => cell.fg,
                };
                if cell.underline {
                    marks.push((Rect::from_xywh(x, y + cell_h - line, width, line), fg));
                }
                if cell.strike {
                    marks.push((
                        Rect::from_xywh(x, y + baseline - cell_h * 0.25, width, line),
                        fg,
                    ));
                }

                // A blank cell still holds its place in the open run.
                let first = cell.text.chars().next().filter(|c| *c != ' ');
                let Some(first) = first else {
                    if let Some(open) = run.as_mut() {
                        open.text.push(' ');
                    }
                    continue;
                };

                let face = self.fonts.pick(cell.bold, cell.italic);
                if face.unichar_to_glyph(first as i32) == 0 {
                    flush_run(canvas, &self.fonts, &mut glyph_paint, baseline, &mut run);
                    if let Some(font) = self.fallbacks.font_for(first, self.chrome.font_size) {
                        glyph_paint.set_color(fg);
                        canvas.draw_str(
                            cell.text.as_str(),
                            Point::new(x, y + baseline),
                            &font,
                            &glyph_paint,
                        );
                    }
                    continue;
                }

                match &run {
                    Some(open)
                        if open.color == fg
                            && open.bold == cell.bold
                            && open.italic == cell.italic => {}
                    _ => {
                        flush_run(canvas, &self.fonts, &mut glyph_paint, baseline, &mut run);
                        run = Some(Run {
                            x,
                            y,
                            color: fg,
                            bold: cell.bold,
                            italic: cell.italic,
                            text: String::new(),
                        });
                    }
                }
                let open = run.as_mut().expect("an open run");
                open.text.push_str(&cell.text);
                if cell.wide {
                    open.text.push(' ');
                }
            }
        }
        flush_run(canvas, &self.fonts, &mut glyph_paint, baseline, &mut run);

        for (rect, color) in marks {
            paint.set_color(color);
            canvas.draw_rect(rect, &paint);
        }

        if focused {
            if let Some(cursor) = frame.cursor {
                let x = origin.0 + cursor.x as f32 * cell_w;
                let y = origin.1 + cursor.y as f32 * cell_h;
                paint.set_color(cursor.color);
                let bar = (cell_w / 8.0).max(1.0);
                match cursor.shape {
                    CursorShape::Bar => {
                        canvas.draw_rect(Rect::from_xywh(x, y, bar, cell_h), &paint);
                    }
                    CursorShape::Underline => {
                        canvas
                            .draw_rect(Rect::from_xywh(x, y + cell_h - line, cell_w, line), &paint);
                    }
                    CursorShape::Hollow => {
                        paint.set_style(skia_safe::paint::Style::Stroke);
                        paint.set_stroke_width(line);
                        canvas.draw_rect(Rect::from_xywh(x, y, cell_w, cell_h), &paint);
                        paint.set_style(skia_safe::paint::Style::Fill);
                    }
                    CursorShape::Block => {
                        canvas.draw_rect(Rect::from_xywh(x, y, cell_w, cell_h), &paint);
                        if let Some(text) = cursor_text(&frame, cursor.x, cursor.y) {
                            glyph_paint.set_color(frame.bg);
                            canvas.draw_str(
                                text,
                                Point::new(x, y + baseline),
                                &self.fonts.normal,
                                &glyph_paint,
                            );
                        }
                    }
                }
            }
        }
    }
}

fn flush_run(
    canvas: &Canvas,
    fonts: &GridFonts,
    paint: &mut Paint,
    baseline: f32,
    run: &mut Option<Run>,
) {
    if let Some(run) = run.take() {
        if !run.text.trim().is_empty() {
            paint.set_color(run.color);
            canvas.draw_str(
                run.text.as_str(),
                Point::new(run.x, run.y + baseline),
                fonts.pick(run.bold, run.italic),
                paint,
            );
        }
    }
}

fn cursor_text(frame: &Frame, x: u16, y: u16) -> Option<&str> {
    let cell = frame.rows.get(y as usize)?.get(x as usize)?;
    (!cell.text.trim().is_empty()).then_some(cell.text.as_str())
}

fn report_button(button: MouseButton) -> GhosttyMouseButton {
    match button {
        MouseButton::Left => GHOSTTY_MOUSE_BUTTON_LEFT,
        MouseButton::Right => GHOSTTY_MOUSE_BUTTON_RIGHT,
        MouseButton::Middle => GHOSTTY_MOUSE_BUTTON_MIDDLE,
    }
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
                    return EventResult::command(TerminalCommand::Resize {
                        cols,
                        rows,
                        px_width: cols as f32 * self.cell.0,
                        px_height: rows as f32 * self.cell.1,
                    });
                }
                EventResult::handled()
            }
            Event::Scroll {
                delta_x,
                delta_y,
                point,
                gesture,
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
                if lines == 0 {
                    return EventResult::handled();
                }
                if self.session.mouse_tracking() {
                    // The wheel is the program's: buttons four and five.
                    let (_, _, x, y) = self.grid_point(*point);
                    let button = match lines > 0 {
                        true => GHOSTTY_MOUSE_BUTTON_FOUR,
                        false => GHOSTTY_MOUSE_BUTTON_FIVE,
                    };
                    for _ in 0..lines.unsigned_abs() {
                        self.session.mouse_report(
                            GHOSTTY_MOUSE_ACTION_PRESS,
                            Some(button),
                            Modifiers::default(),
                            x,
                            y,
                            self.report_size(),
                            false,
                        );
                    }
                    return EventResult::handled();
                }
                if self.session.alt_screen() {
                    // No scrollback behind a full-screen program: the
                    // wheel turns into arrow keys (less, vim).
                    let key = match lines > 0 {
                        true => Key::Up,
                        false => Key::Down,
                    };
                    for _ in 0..lines.unsigned_abs() {
                        self.session.key_input(key, Modifiers::default());
                    }
                    return EventResult::handled();
                }
                EventResult::command(TerminalCommand::Scroll { lines })
            }
            Event::MouseDown {
                point,
                button,
                mods,
                ..
            } => {
                if self.session.mouse_tracking() && !mods.shift {
                    let (_, _, x, y) = self.grid_point(*point);
                    self.session.set_press(Some(Press::Reported(*button)));
                    self.session.mouse_report(
                        GHOSTTY_MOUSE_ACTION_PRESS,
                        Some(report_button(*button)),
                        *mods,
                        x,
                        y,
                        self.report_size(),
                        false,
                    );
                    return EventResult::handled();
                }
                if *button == MouseButton::Left {
                    self.session.set_press(Some(Press::Selecting));
                    return self.select_command(Select::Press, *point);
                }
                EventResult::handled()
            }
            // Drags and releases are BROADCAST by containers (no rect
            // cull), so they may only be claimed by the pane whose
            // press armed them.
            Event::MouseDrag { point, mods } => match self.session.press() {
                Some(Press::Reported(button)) => {
                    let (_, _, x, y) = self.grid_point(*point);
                    self.session.mouse_report(
                        GHOSTTY_MOUSE_ACTION_MOTION,
                        Some(report_button(button)),
                        *mods,
                        x,
                        y,
                        self.report_size(),
                        true,
                    );
                    EventResult::handled()
                }
                Some(Press::Selecting) => self.select_command(Select::Drag, *point),
                None => EventResult::Ignored,
            },
            Event::MouseUp { point } => match self.session.press() {
                Some(Press::Reported(button)) => {
                    self.session.set_press(None);
                    let (_, _, x, y) = self.grid_point(*point);
                    self.session.mouse_report(
                        GHOSTTY_MOUSE_ACTION_RELEASE,
                        Some(report_button(button)),
                        Modifiers::default(),
                        x,
                        y,
                        self.report_size(),
                        false,
                    );
                    EventResult::handled()
                }
                Some(Press::Selecting) => {
                    self.session.set_press(None);
                    self.select_command(Select::Release, *point)
                }
                None => EventResult::Ignored,
            },
            Event::MouseMove { point, mods } => {
                use skia_safe::Contains;
                // Moves are broadcast too: only the hovered pane
                // reports them (programs in mode 1003 want hover).
                if self.session.press().is_none()
                    && Rect::from_size(self.size).contains(*point)
                    && self.session.mouse_tracking()
                {
                    let (_, _, x, y) = self.grid_point(*point);
                    self.session.mouse_report(
                        GHOSTTY_MOUSE_ACTION_MOTION,
                        None,
                        *mods,
                        x,
                        y,
                        self.report_size(),
                        false,
                    );
                }
                EventResult::Ignored
            }
            _ => EventResult::Ignored,
        }
    }
}
