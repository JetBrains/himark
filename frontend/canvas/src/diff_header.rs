// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The diff FILE BAND, two dresses. `FileBand` is the canvas's (the
//! typographic redesign): one row of words on ONE baseline over the
//! ground, a hairline above — the dimmed directory leading into the
//! file name, the `+N −M` trail, a flexible gap, the ordinal and the
//! word buttons. No card, no glyphs: rank is size and weight.
//! `DiffHeaderFace` is the chat edit cell's compact painted band —
//! the fold chevron, the hairline glyph buttons — kept for the cells
//! until they adopt the band. Call sites map the presses onto their
//! own command type.

use imba::{
    arena::Arena,
    event::{Event, EventResult},
    store::Store,
    ui::UiCtx,
    Widget,
};
use skia_safe::{Paint, Rect, Size};

use editor::env;

/// What a press on the band means — the canvas header's affordances.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffHeaderPress {
    /// The band's body (and the open-file button): the live file.
    OpenFile,
    /// The framed pair: the standalone side-by-side pane.
    OpenPane,
    /// The two columns: inline ⇄ split.
    ToggleFace,
    /// The chevron: fold the diff away / bring it back.
    ToggleCollapse,
}

/// What a call site asks the band to show.
pub struct DiffHeaderSpec {
    pub title: String,
    pub added: Option<i64>,
    pub removed: Option<i64>,
    /// Some(collapsed): the fold chevron stands at the left edge.
    pub chevron: Option<bool>,
    /// The right-edge buttons, RIGHTMOST FIRST (the canvas order).
    pub buttons: Vec<DiffHeaderPress>,
    /// What the band's body presses. None: the body is inert.
    pub primary: Option<DiffHeaderPress>,
    /// Title typography — the canvas passes its H1, the chat its cell
    /// title. The trail and the glyphs size from the chat chrome in
    /// both, so the affordances match across the app.
    pub title_size: f32,
    pub bold: bool,
    pub title_color: Option<skia_safe::Color>,
    /// The text baseline, from the band's top.
    pub baseline: f32,
}

pub struct DiffHeaderFace {
    /// The directory half of the title, trailing slash kept — dimmed.
    prefix: String,
    /// The leaf: the file name (plus any call-site suffix).
    title: String,
    added: Option<i64>,
    removed: Option<i64>,

    band: f32,
    width: f32,
    inset: f32,
    baseline: f32,

    title_font: skia_safe::Font,
    title_color: skia_safe::Color,
    trail_font: skia_safe::Font,
    shaper: std::rc::Rc<imba::layout::TextShaper>,
    added_color: skia_safe::Color,
    removed_color: skia_safe::Color,
    affordance_color: skia_safe::Color,

    chevron: Option<(Rect, bool)>,
    buttons: Vec<(DiffHeaderPress, Rect)>,
    primary: Option<DiffHeaderPress>,
}

impl DiffHeaderFace {
    pub fn new(store: &Store, ui: &UiCtx, spec: DiffHeaderSpec, band: f32, width: f32) -> Self {
        let theme = env::Themes::of(store);
        let chrome = theme.ui().chat.clone();
        let mut title_font = hikit::fonts::ui_text_font(ui, spec.title_size);
        if spec.bold {
            title_font.set_embolden(true);
        }
        let inset = chrome.pad;
        let glyph = chrome.title_size * 1.2;
        let zone = glyph + chrome.title_size;

        // Button zones, right edge inward — first in the spec sits
        // rightmost, the canvas's own walk.
        let mut buttons = Vec::new();
        let mut right = width - inset;
        for press in spec.buttons {
            buttons.push((press, Rect::from_xywh(right - zone, 0.0, zone, band)));
            right -= zone;
        }

        // The path splits at its last slash: the directories dim, the
        // file name carries the title weight.
        let split = spec.title.rfind('/').map(|at| at + 1).unwrap_or(0);
        let (prefix, title) = spec.title.split_at(split);

        Self {
            prefix: prefix.to_owned(),
            title: title.to_owned(),
            added: spec.added.filter(|n| *n > 0),
            removed: spec.removed.filter(|n| *n > 0),
            band,
            width,
            inset,
            baseline: spec.baseline,
            title_font,
            title_color: spec.title_color.unwrap_or(chrome.text_color.0),
            trail_font: hikit::fonts::ui_text_font(ui, chrome.title_size),
            shaper: imba::layout::TextShaper::of(ui),
            added_color: chrome.added_color.0,
            removed_color: chrome.removed_color.0,
            affordance_color: chrome.loader_color.0,
            chevron: spec.chevron.map(|collapsed| {
                (
                    Rect::from_xywh(0.0, 0.0, inset + chrome.title_size, band),
                    collapsed,
                )
            }),
            buttons,
            primary: spec.primary,
        }
    }

    pub fn widget<Command, Map>(self, map: Map) -> DiffHeaderWidget<Map>
    where
        Map: Fn(DiffHeaderPress) -> Option<Command>,
    {
        DiffHeaderWidget { face: self, map }
    }

    fn press_at(&self, x: f32) -> Option<DiffHeaderPress> {
        if let Some((zone, _)) = &self.chevron {
            if zone.right > x {
                return Some(DiffHeaderPress::ToggleCollapse);
            }
        }
        for (press, zone) in &self.buttons {
            if x >= zone.left && x < zone.right {
                return Some(*press);
            }
        }
        self.primary
    }

    /// Where the trail (and the title's left clamp) starts: after the
    /// chevron when one stands, at the inset otherwise.
    fn lead(&self) -> f32 {
        match &self.chevron {
            Some((zone, _)) => zone.right,
            None => self.inset,
        }
    }

    fn paint(&self, canvas: &skia_safe::Canvas, rect: Rect) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        let baseline = rect.top + self.baseline;

        // The chevron: right-pointing when collapsed, down when open.
        if let Some((zone, collapsed)) = &self.chevron {
            let glyph = self.trail_font.size() * 0.5;
            let center = (
                rect.left + zone.left + self.inset * 0.5 + glyph * 0.5,
                baseline - glyph * 0.6,
            );
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((glyph * 0.22).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            paint.set_color(self.affordance_color);
            let mut path = skia_safe::PathBuilder::new();
            if *collapsed {
                path.move_to((center.0 - glyph * 0.25, center.1 - glyph * 0.5));
                path.line_to((center.0 + glyph * 0.35, center.1));
                path.line_to((center.0 - glyph * 0.25, center.1 + glyph * 0.5));
            } else {
                path.move_to((center.0 - glyph * 0.5, center.1 - glyph * 0.25));
                path.line_to((center.0, center.1 + glyph * 0.35));
                path.line_to((center.0 + glyph * 0.5, center.1 - glyph * 0.25));
            }
            canvas.draw_path(&path.detach(), &paint);
            paint.set_style(skia_safe::paint::Style::Fill);
        }

        // The text run clips at the buttons — a narrow pane crops the
        // path and counts rather than drawing through the glyphs.
        let buttons_left = self
            .buttons
            .last()
            .map(|(_, zone)| zone.left - self.inset * 0.5)
            .unwrap_or(self.width - self.inset);
        canvas.save();
        canvas.clip_rect(
            Rect::new(rect.left, rect.top, rect.left + buttons_left, rect.bottom),
            None,
            false,
        );

        // The directory prefix, dimmed, leading into the file name.
        let mut x = rect.left + self.lead();
        if !self.prefix.is_empty() {
            x += self.shaper.draw(
                canvas,
                &self.trail_font,
                &self.prefix,
                self.affordance_color,
                0.0,
                x,
                baseline,
            );
        }
        x += self.shaper.draw(
            canvas,
            &self.title_font,
            &self.title,
            self.title_color,
            0.0,
            x,
            baseline,
        ) + self.trail_font.size();

        // The +N −M trail after the name.
        if let Some(added) = self.added {
            let label = format!("+{added}");
            x += self.shaper.draw(
                canvas,
                &self.trail_font,
                &label,
                self.added_color,
                0.0,
                x,
                baseline,
            ) + 8.0;
        }
        if let Some(removed) = self.removed {
            self.shaper.draw(
                canvas,
                &self.trail_font,
                &format!("−{removed}"),
                self.removed_color,
                0.0,
                x,
                baseline,
            );
        }
        canvas.restore();

        // The buttons, hairline glyphs on the affordance color.
        paint.set_style(skia_safe::paint::Style::Stroke);
        paint.set_color(self.affordance_color);
        let side = self.trail_font.size() * 1.05;
        paint.set_stroke_width((side * 0.11).max(1.0));
        for (press, zone) in &self.buttons {
            let center = (
                rect.left + zone.left + zone.width() * 0.5,
                baseline - side * 0.42,
            );
            let half = side * 0.5;
            let frame = Rect::from_xywh(center.0 - half, center.1 - half, side, side);
            match press {
                DiffHeaderPress::ToggleFace => {
                    // Two columns — switch the diff's face.
                    canvas.draw_round_rect(frame, 2.0, 2.0, &paint);
                    canvas.draw_line((center.0, frame.top), (center.0, frame.bottom), &paint);
                }
                DiffHeaderPress::OpenFile => {
                    // A corner arrow leaving the box.
                    let inset = side * 0.22;
                    let mut path = skia_safe::PathBuilder::new();
                    path.move_to((frame.left + side * 0.5, frame.top + inset));
                    path.line_to((frame.left + inset, frame.top + inset));
                    path.line_to((frame.left + inset, frame.bottom - inset));
                    path.line_to((frame.right - inset, frame.bottom - inset));
                    path.line_to((frame.right - inset, frame.top + side * 0.5));
                    canvas.draw_path(&path.detach(), &paint);
                    let mut arrow = skia_safe::PathBuilder::new();
                    arrow.move_to((center.0 + side * 0.05, center.1 - side * 0.05));
                    arrow.line_to((frame.right, frame.top));
                    arrow.move_to((frame.right - side * 0.32, frame.top));
                    arrow.line_to((frame.right, frame.top));
                    arrow.line_to((frame.right, frame.top + side * 0.32));
                    canvas.draw_path(&arrow.detach(), &paint);
                }
                DiffHeaderPress::OpenPane => {
                    // A framed pair of panes — the standalone
                    // side-by-side diff.
                    canvas.draw_round_rect(frame, 2.0, 2.0, &paint);
                    let third = frame.left + frame.width() * 0.5;
                    canvas.draw_line((third, frame.top), (third, frame.bottom), &paint);
                    let mid = frame.top + frame.height() * 0.5;
                    canvas.draw_line((frame.left, mid), (third, mid), &paint);
                }
                _ => {}
            }
        }
        paint.set_style(skia_safe::paint::Style::Fill);
    }
}

pub struct DiffHeaderWidget<Map> {
    face: DiffHeaderFace,
    map: Map,
}

impl<'a, Command, Map> Widget<'a, Command> for DiffHeaderWidget<Map>
where
    Map: Fn(DiffHeaderPress) -> Option<Command>,
{
    fn size(&self) -> Size {
        Size::new(self.face.width, self.face.band)
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<Command> {
        match event {
            Event::Paint { canvas, .. } => {
                self.face.paint(canvas, Rect::from_size(self.size()));
                EventResult::handled()
            }
            Event::MouseDown {
                button: imba::event::MouseButton::Left,
                point,
                ..
            } => match self
                .face
                .press_at(point.x)
                .and_then(|press| (self.map)(press))
            {
                Some(command) => EventResult::command(command),
                None => EventResult::Ignored,
            },
            _ => EventResult::Ignored,
        }
    }
}

// ------------------------------------------------------------ file band

/// The canvas's FILE BAND (Main.dc.html, the file band): the words of
/// one file on one baseline, a hairline above, the ground under — and
/// from the second file on, a stretch of AIR above the hairline
/// (`band_air`), so whitespace groups the files and the rule tells
/// them apart. The title group (directory, name, counts) presses
/// `on_title`; the word buttons trail at the right edge, the ordinal
/// before them.
pub struct FileBand<'a, Command> {
    title: String,
    added: Option<i64>,
    removed: Option<i64>,
    collapsed: bool,
    ordinal: Option<(usize, usize)>,
    on_title: Option<Box<dyn Fn() -> Command + 'a>>,
    /// The trailing word buttons, left to right.
    words: Vec<(&'static str, Box<dyn Fn() -> Command + 'a>)>,
    styles: FileBandStyles,
    chrome: editor::theme::DiffChrome,
    lead: f32,
    rule: skia_safe::Color,
    ground: skia_safe::Color,
}

/// The band's RANKS: the directory and the trail in the brief rung,
/// the name in its own heavier one, the ordinal a step smaller.
#[derive(Clone)]
struct FileBandStyles {
    dir: hikit::ui::TextStyle,
    name: hikit::ui::TextStyle,
    added: hikit::ui::TextStyle,
    removed: hikit::ui::TextStyle,
    ordinal: hikit::ui::TextStyle,
    word: hikit::ui::TextStyle,
}

/// The FIRST band's height from the chrome alone — the insets around
/// the name's line box, no air (the tests aim at the first file).
pub fn file_band_height(store: &Store) -> f32 {
    let chrome = env::Themes::of(store).ui().diff.clone();
    file_band_height_of(&chrome)
}

pub fn file_band_height_of(chrome: &editor::theme::DiffChrome) -> f32 {
    chrome.band_pad_top
        + (chrome.file_name.size * chrome.file_name.line_height).ceil()
        + chrome.band_pad_bottom
}

/// The air above a band's hairline: none on the first file (the
/// commit block or the summary strip stands right above it), the
/// chrome's `band_air` on every file after it. `at` is the 1-based
/// ordinal; 0 is a band not yet stamped, which sits like the first.
pub fn file_band_air_of(chrome: &editor::theme::DiffChrome, at: usize) -> f32 {
    match at > 1 {
        true => chrome.band_air,
        false => 0.0,
    }
}

/// What the canvas reserves for the header row of the file at
/// `ordinal` before it lays out: the band plus its air.
pub fn file_band_row_height_of(
    chrome: &editor::theme::DiffChrome,
    ordinal: (usize, usize),
) -> f32 {
    file_band_air_of(chrome, ordinal.0) + file_band_height_of(chrome)
}

impl<'a, Command: 'a> FileBand<'a, Command> {
    pub fn new(store: &Store, ui: &UiCtx, title: &str) -> Self {
        let theme = env::Themes::of(store);
        let ink = theme.ui().ink.clone();
        let chrome = theme.ui().diff.clone();
        let brief = hikit::ui::brief(store, ui);
        let name = hikit::ui::TextStyle {
            font: hikit::fonts::role_font(ui, &chrome.file_name),
            color: ink.ink.0,
            tracking: chrome.file_name.tracking,
            shaper: brief.shaper.clone(),
        };
        Self {
            title: title.to_owned(),
            added: None,
            removed: None,
            collapsed: false,
            ordinal: None,
            on_title: None,
            words: Vec::new(),
            styles: FileBandStyles {
                dir: brief.clone(),
                name,
                added: brief.clone().colored(ink.added.0),
                removed: brief.clone().colored(ink.removed.0),
                ordinal: brief.clone().sized(chrome.ordinal_size),
                word: brief,
            },
            lead: theme.ui().window.code_text_edge,
            rule: ink.rule.0,
            ground: ink.ground.0,
            chrome,
        }
    }

    pub fn counts(mut self, added: Option<i64>, removed: Option<i64>) -> Self {
        self.added = added.filter(|n| *n > 0);
        self.removed = removed.filter(|n| *n > 0);
        self
    }

    /// A collapsed file's name steps down an ink — the fold state is
    /// typographic, no chevron.
    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    /// "file N of M", before the word buttons.
    pub fn ordinal(mut self, at: usize, of: usize) -> Self {
        self.ordinal = Some((at, of));
        self
    }

    pub fn on_title(mut self, press: impl Fn() -> Command + 'a) -> Self {
        self.on_title = Some(Box::new(press));
        self
    }

    /// A trailing word button, after the ones already added.
    pub fn word(mut self, label: &'static str, press: impl Fn() -> Command + 'a) -> Self {
        self.words.push((label, Box::new(press)));
        self
    }
}

impl<'a, Command> imba::layout::LayoutValue for FileBand<'a, Command> {}

impl<'a, Command: 'a> imba::layout::Layout<'a, Command> for FileBand<'a, Command> {
    fn layout(
        self,
        arena: &'a Arena,
        constraints: imba::constraints::Constraints,
    ) -> imba::ThunkBox<'a, Command> {
        use imba::layout::{Insets, LayoutExt as _, Row};
        use hikit::ui::text;
        let FileBand {
            title,
            added,
            removed,
            collapsed,
            ordinal,
            on_title,
            words,
            styles,
            chrome,
            lead,
            rule,
            ground,
        } = self;
        let gap = |left: f32| Insets {
            left,
            ..Default::default()
        };

        // The path splits at its last slash: the directories dim, the
        // file name carries the band's weight.
        let split = title.rfind('/').map(|at| at + 1).unwrap_or(0);
        let (dir, name) = title.split_at(split);
        let mut name_style = styles.name.clone();
        if collapsed {
            name_style = name_style.colored(styles.word.color);
        }
        let mut heading = Row::new(arena);
        if !dir.is_empty() {
            heading = heading.child_by_baseline(text(&styles.dir, dir));
        }
        let step = match dir.is_empty() {
            true => 0.0,
            false => chrome.band_name_gap,
        };
        heading = heading.child_by_baseline(text(&name_style, name).pad_insets(gap(step)));
        // The trail: `+N −M`, one word space apart.
        let space = styles.added.shaper.advance(&styles.added.font, " ");
        let mut lead_gap = chrome.band_gap;
        if let Some(added) = added {
            heading = heading
                .child_by_baseline(text(&styles.added, format!("+{added}")).pad_insets(gap(lead_gap)));
            lead_gap = space;
        }
        if let Some(removed) = removed {
            heading = heading.child_by_baseline(
                text(&styles.removed, format!("−{removed}")).pad_insets(gap(lead_gap)),
            );
        }
        let heading: imba::layout::LayoutBox<'a, Command> = match on_title {
            Some(press) => imba::layout::LayoutBox::new(arena, heading.on_click(move || press())),
            None => imba::layout::LayoutBox::new(arena, heading),
        };

        let mut trailing = Row::new(arena);
        if let Some((at, of)) = ordinal {
            trailing = trailing.child_by_baseline(text(&styles.ordinal, format!("file {at} of {of}")));
        }
        for (label, press) in words {
            trailing = trailing.child_by_baseline(
                text(&styles.word, label)
                    .on_click(move || press())
                    .pad_insets(gap(chrome.band_gap)),
            );
        }

        // One Row, one baseline group — the column header's shape:
        // the heading, a main-axis-only flexible gap, the trailing
        // words.
        let width = constraints.max.width.max(1.0);
        let height = file_band_height_of(&chrome);
        let air = file_band_air_of(&chrome, ordinal.map(|(at, _)| at).unwrap_or(0));
        let room = (width - lead - chrome.band_pad_right).max(1.0);
        let laid = Row::new(arena)
            .child_by_baseline(heading)
            .weighted(1.0, imba::layout::Fill::new().height(0.0))
            .child_by_baseline(trailing)
            .layout(
                arena,
                imba::constraints::Constraints {
                    min: Size::new(room, 0.0),
                    max: Size::new(room, height),
                },
            );
        // The baseline sits where CSS puts it in the name's line box:
        // half the leading above the ascent.
        let (_, metrics) = styles.name.font.metrics();
        let (ascent, descent) = (-metrics.ascent, metrics.descent);
        let line = chrome.file_name.size * chrome.file_name.line_height;
        let baseline = chrome.band_pad_top + (line - ascent - descent) * 0.5 + ascent;
        let at = imba::Thunk::first_baseline(&laid).unwrap_or(0.0);
        let mut band = imba::container::container(arena, Size::new(width, height));
        band.place_boxed(lead, baseline - at, laid);
        // Layers, bottom up: the ground, the hairline over it, the
        // words on top — the rule inside the ground, never under it.
        let ruled = imba::ThunkBox::new(
            arena,
            imba::thunk_ext::ThunkExt::paint_below(
                hikit::ui::Hairline::new(rule).top().wrap(band),
                hikit::ui::ground(ground),
            ),
        );
        if air <= 0.0 {
            return ruled;
        }
        // The air is the band's own: ground above the hairline, so the
        // previous file's last line and this file's rule stand apart.
        let mut aired = imba::container::container(arena, Size::new(width, air + height));
        aired.place_boxed(0.0, air, ruled);
        imba::ThunkBox::new(
            arena,
            imba::thunk_ext::ThunkExt::paint_below(aired, hikit::ui::ground(ground)),
        )
    }
}
