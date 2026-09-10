use std::{ops::Range, sync::OnceLock};

use imba::{
    anim::{Animation, AnimationClock, Easing, Motion},
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult, MouseButton},
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, View,
};
use skia_safe::{Canvas, Color, Font, FontMgr, FontStyle, Paint, Rect, Size, Typeface};

use himark::{Document, Inlay, InlayMode};
use himarkdown::BlockMarks;
use himarkdown::MarkdownBlock;

const HEADER_MODES: [InlayMode; 4] = [
    InlayMode::Above,
    InlayMode::Left,
    InlayMode::Right,
    InlayMode::Under,
];

pub fn add_badges(
    document: &mut Document,
    blocks: &[MarkdownBlock],
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) {
    let mut demo_inlays = DemoInlays::default();

    let demo_markup = demo_markup();
    document.ensure_document_markup(demo_markup);

    let mut discarded = imba::effect::Batch::new();
    let fx = &mut discarded.effects();
    for block in blocks {
        demo_inlays.push_for_block(
            document,
            demo_markup,
            block.range.clone(),
            block.marks,
            block.display.as_deref(),
            fonts,
            theme,
            fx,
        );
    }
}

pub(crate) fn demo_markup() -> himark::MarkupId {
    static ID: std::sync::OnceLock<himark::MarkupId> = std::sync::OnceLock::new();
    *ID.get_or_init(himark::MarkupId::mint)
}

#[derive(Default)]
pub(crate) struct DemoInlays {
    headers: u32,
    paragraphs: u32,
    code_blocks: u32,
    list_items: u32,
    rules: u32,
}

impl DemoInlays {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_for_block(
        &mut self,
        document: &mut Document,
        demo_markup: himark::MarkupId,
        range: Range<u32>,
        marks: BlockMarks,
        display: Option<&str>,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
        fx: &mut himark::EditorEffects<'_>,
    ) {
        if let Some(level) = marks.header {
            let title = display
                .filter(|text| !text.is_empty())
                .map(short_ascii)
                .unwrap_or_else(|| format!("Header {}", self.headers + 1));
            for mode in HEADER_MODES {
                document.push_inlay(
                    demo_markup,
                    range.clone(),
                    Inlay::new(
                        mode,
                        DemoInlay::new(
                            DemoTone::Header(level),
                            mode,
                            format!("H{level}"),
                            title.clone(),
                            self.headers,
                        ),
                    ),
                    fonts,
                    theme,
                    fx,
                );
            }
            self.headers = self.headers.wrapping_add(1);
        }

        if is_plain_paragraph(marks) {
            if let Some(inline_range) = paragraph_interior_range(range.clone(), display) {
                let paragraph = self.paragraphs;
                let (mode, tag, title) = if paragraph % 4 == 3 {
                    (
                        InlayMode::Instead(himark::InsteadKind::FullLine),
                        "swap",
                        "Paragraph replacement",
                    )
                } else if paragraph.is_multiple_of(2) {
                    (InlayMode::Left, "inline", "Paragraph inlay")
                } else {
                    (InlayMode::Right, "inline", "Paragraph inlay")
                };
                document.push_inlay(
                    demo_markup,
                    inline_range,
                    Inlay::new(
                        mode,
                        DemoInlay::new(DemoTone::Paragraph, mode, tag, title, paragraph),
                    ),
                    fonts,
                    theme,
                    fx,
                );
                self.paragraphs = self.paragraphs.wrapping_add(1);
            }
        }

        if marks.code {
            if self.code_blocks.is_multiple_of(2) {
                document.push_inlay(
                    demo_markup,
                    range.clone(),
                    Inlay::new(
                        InlayMode::Under,
                        DemoInlay::new(
                            DemoTone::Code,
                            InlayMode::Under,
                            "trace",
                            "Inline result",
                            self.code_blocks,
                        ),
                    ),
                    fonts,
                    theme,
                    fx,
                );
            }
            self.code_blocks = self.code_blocks.wrapping_add(1);
        }

        if marks.list_item {
            let list_item = self.list_items;
            if list_item.is_multiple_of(5) {
                document.push_inlay(
                    demo_markup,
                    range.clone(),
                    Inlay::new(
                        InlayMode::Left,
                        DemoInlay::new(
                            DemoTone::List,
                            InlayMode::Left,
                            "todo",
                            "Task state",
                            list_item,
                        ),
                    ),
                    fonts,
                    theme,
                    fx,
                );
            } else if list_item % 7 == 3 {
                let range = interior_word_range(range.clone(), display).unwrap_or(range.clone());
                document.push_inlay(
                    demo_markup,
                    range,
                    Inlay::new(
                        InlayMode::Instead(himark::InsteadKind::FullLine),
                        DemoInlay::new(
                            DemoTone::List,
                            InlayMode::Instead(himark::InsteadKind::FullLine),
                            "replace",
                            "List item replacement",
                            list_item,
                        ),
                    ),
                    fonts,
                    theme,
                    fx,
                );
            }
            self.list_items = self.list_items.wrapping_add(1);
        }

        if marks.horizontal_line {
            document.push_inlay(
                demo_markup,
                range,
                Inlay::new(
                    InlayMode::Instead(himark::InsteadKind::FullLine),
                    DemoInlay::new(
                        DemoTone::Rule,
                        InlayMode::Instead(himark::InsteadKind::FullLine),
                        "break",
                        "Section switch",
                        self.rules,
                    ),
                ),
                fonts,
                theme,
                fx,
            );
            self.rules = self.rules.wrapping_add(1);
        }
    }
}

#[derive(Clone)]
struct DemoInlay {
    tone: DemoTone,
    mode: InlayMode,
    tag: String,
    title: String,
    ordinal: u32,
    expanded: bool,
    clicks: u32,

    size: Animation<Size>,
}

#[derive(Clone, Copy)]
enum DemoTone {
    Header(u8),
    Paragraph,
    Code,
    List,
    Rule,
}

enum DemoInlayCommand {
    Toggle,
    Tick(AnimationClock),
}

impl DemoInlay {
    fn new(
        tone: DemoTone,
        mode: InlayMode,
        tag: impl Into<String>,
        title: impl Into<String>,
        ordinal: u32,
    ) -> Self {
        let collapsed = Self::target_size(mode, false);
        Self {
            tone,
            mode,
            tag: tag.into(),
            title: title.into(),
            ordinal,
            expanded: false,
            clicks: 0,
            size: Animation::done(
                collapsed,
                Motion::Ease {
                    duration_ms: 180.0,
                    easing: Easing::EaseInOut,
                },
            ),
        }
    }

    fn target_size(mode: InlayMode, expanded: bool) -> Size {
        match mode {
            InlayMode::Left | InlayMode::Right => match expanded {
                true => Size::new(220.0, 56.0),
                false => Size::new(128.0, 40.0),
            },
            InlayMode::Above | InlayMode::Under | InlayMode::Popup(_) => match expanded {
                true => Size::new(520.0, 96.0),
                false => Size::new(360.0, 72.0),
            },
            InlayMode::Instead(_) => match expanded {
                true => Size::new(520.0, 64.0),
                false => Size::new(430.0, 46.0),
            },
        }
    }

    fn size_for(&self, constraints: Constraints) -> Size {
        let max_width = constraints.max.width.max(1.0);
        let size = self.size.value();
        Size::new(size.width.min(max_width), size.height)
    }

    fn paint(&self, canvas: &Canvas, rect: Rect) {
        let size = rect.size();
        let palette = palette(self.tone, self.expanded);
        let radius = match self.mode {
            InlayMode::Instead(_) => 10.0,
            _ => 7.0,
        };
        let mut paint = Paint::default();
        paint.set_anti_alias(true);

        paint.set_color(palette.glow);
        canvas.draw_round_rect(
            Rect::from_xywh(
                1.0,
                2.0,
                (size.width - 2.0).max(1.0),
                (size.height - 2.0).max(1.0),
            ),
            radius + 2.0,
            radius + 2.0,
            &paint,
        );

        paint.set_color(palette.fill);
        canvas.draw_round_rect(
            Rect::from_xywh(0.0, 0.0, size.width.max(1.0), (size.height - 1.0).max(1.0)),
            radius,
            radius,
            &paint,
        );

        paint.set_color(palette.accent);
        canvas.draw_round_rect(
            Rect::from_xywh(7.0, 7.0, 5.0, (size.height - 14.0).max(5.0)),
            2.5,
            2.5,
            &paint,
        );
        canvas.draw_circle((size.width - 16.0, 14.0), 3.0, &paint);
        if self.expanded {
            canvas.draw_circle((size.width - 27.0, size.height - 13.0), 4.0, &paint);
            canvas.draw_circle((size.width - 40.0, size.height - 13.0), 2.5, &paint);
        }

        let font_size = match self.mode {
            InlayMode::Instead(_) => 18.0,
            InlayMode::Above | InlayMode::Under | InlayMode::Popup(_) => 18.0,
            InlayMode::Left | InlayMode::Right => 16.0,
        };
        let font = demo_font(font_size);

        paint.set_color(palette.text);
        let title = match self.expanded {
            true => format!(
                "{} {}  click {}",
                mode_label(self.mode),
                self.title,
                self.clicks
            ),
            false => format!(
                "{} {} {}",
                mode_label(self.mode),
                self.tag,
                self.ordinal + 1
            ),
        };
        canvas.draw_str(
            title,
            (18.0, baseline(size.height, self.mode)),
            &font,
            &paint,
        );
    }
}

impl View for DemoInlay {
    type Command = DemoInlayCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &imba::UiCtx,
        command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            DemoInlayCommand::Toggle => {
                self.expanded = !self.expanded;
                self.clicks = self.clicks.wrapping_add(1);
                self.size.set(Self::target_size(self.mode, self.expanded));
            }
            DemoInlayCommand::Tick(now) => self.size.advance(now),
        }
    }

    fn layout<'a>(
        &'a self,
        _arena: &'a Arena,
        _store: &'a Store,
        _ui: &'a imba::UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let size = self.size_for(constraints);
        imba::leaf::leaf(size.width, size.height)
            .paint_instead(|_arena, canvas, rect| self.paint(canvas, rect))
            .event({
                let animating = self.size.running();
                move |_arena, event, _size| match event {
                    Event::MouseDown {
                        mods: _,
                        button: MouseButton::Left,
                        ..
                    } => EventResult::Command(DemoInlayCommand::Toggle),
                    Event::AnimationClock { now } if animating => {
                        EventResult::Command(DemoInlayCommand::Tick(*now))
                    }
                    _ => EventResult::Ignored,
                }
            })
            .commands(move || {
                vec![imba::PresentableCommand::new(
                    "demo.inlay.toggle",
                    match self.expanded {
                        true => "Collapse Demo Inlay",
                        false => "Expand Demo Inlay",
                    },
                    DemoInlayCommand::Toggle,
                )]
            })
    }
}

fn demo_font(size: f32) -> Font {
    static TYPEFACE: OnceLock<Option<Typeface>> = OnceLock::new();
    match TYPEFACE
        .get_or_init(|| FontMgr::default().legacy_make_typeface(None, FontStyle::normal()))
    {
        Some(typeface) => Font::from_typeface(typeface.clone(), size),
        None => {
            let mut font = Font::default();
            font.set_size(size);
            font
        }
    }
}

fn is_plain_paragraph(marks: BlockMarks) -> bool {
    marks.header.is_none()
        && !marks.list_item
        && !marks.code
        && !marks.horizontal_line
        && !marks.quote
        && marks.indent == 0
}

fn paragraph_interior_range(range: Range<u32>, display: Option<&str>) -> Option<Range<u32>> {
    let text = display?;
    if text.len() < 96 {
        return None;
    }

    interior_word_range(range, Some(text))
}

fn interior_word_range(range: Range<u32>, display: Option<&str>) -> Option<Range<u32>> {
    let text = display?;
    let start = text.find(" ").map(|index| index + 1).unwrap_or(0);
    let tail = &text[start..];
    let len = tail
        .split_whitespace()
        .next()
        .map(str::len)
        .filter(|len| *len > 0)?;
    let start = range
        .start
        .saturating_add(start.min(u32::MAX as usize) as u32);
    let end = start.saturating_add(len.min(u32::MAX as usize) as u32);
    (start < end && end <= range.end).then_some(start..end)
}

fn mode_label(mode: InlayMode) -> &'static str {
    match mode {
        InlayMode::Left => "left",
        InlayMode::Right => "right",
        InlayMode::Under => "below",
        InlayMode::Above => "above",
        InlayMode::Instead(_) => "instead",
        InlayMode::Popup(_) => "popup",
    }
}

fn baseline(height: f32, mode: InlayMode) -> f32 {
    match mode {
        InlayMode::Instead(_) => (height * 0.5 + 5.0).round(),
        _ => (height * 0.5 + 4.0).round(),
    }
}

struct Palette {
    fill: Color,
    glow: Color,
    accent: Color,
    text: Color,
}

fn palette(tone: DemoTone, expanded: bool) -> Palette {
    let boost = if expanded { 24 } else { 0 };
    match tone {
        DemoTone::Header(level) => {
            let accent = match level {
                1 => Color::from_rgb(118, 209, 255),
                2 => Color::from_rgb(178, 229, 132),
                3 => Color::from_rgb(255, 199, 95),
                _ => Color::from_rgb(232, 168, 255),
            };
            Palette {
                fill: Color::from_rgb(27 + boost, 35 + boost, 48 + boost),
                glow: Color::from_argb(115, 37, 91, 127),
                accent,
                text: Color::from_rgb(241, 246, 252),
            }
        }
        DemoTone::Paragraph => Palette {
            fill: Color::from_rgb(31 + boost, 42 + boost, 52 + boost),
            glow: Color::from_argb(105, 56, 101, 126),
            accent: Color::from_rgb(135, 211, 255),
            text: Color::from_rgb(232, 246, 255),
        },
        DemoTone::Code => Palette {
            fill: Color::from_rgb(20 + boost, 45 + boost, 38 + boost),
            glow: Color::from_argb(105, 36, 120, 92),
            accent: Color::from_rgb(111, 232, 170),
            text: Color::from_rgb(225, 249, 235),
        },
        DemoTone::List => Palette {
            fill: Color::from_rgb(42 + boost, 34 + boost, 28 + boost),
            glow: Color::from_argb(105, 164, 98, 43),
            accent: Color::from_rgb(255, 181, 92),
            text: Color::from_rgb(255, 239, 220),
        },
        DemoTone::Rule => Palette {
            fill: Color::from_rgb(39 + boost, 30 + boost, 53 + boost),
            glow: Color::from_argb(115, 116, 74, 166),
            accent: Color::from_rgb(209, 158, 255),
            text: Color::from_rgb(247, 236, 255),
        },
    }
}

fn short_ascii(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if out.len() >= 34 {
            out.push_str("...");
            break;
        }
        if ch.is_ascii() && !ch.is_ascii_control() {
            out.push(ch);
        }
    }
    out.trim().to_owned()
}
