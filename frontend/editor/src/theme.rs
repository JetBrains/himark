// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use serde::Deserialize;
use skia_safe::Color;

pub mod air;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StyleId {
    Emphasis,
    Strong,
    InlineCode,
    Link,
    Strikethrough,

    Composing,

    Match,

    DiffAdded,
    DiffDeleted,
    DiffModified,
    DiffAddedWord,
    DiffDeletedWord,
    DiffModifiedWord,

    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Variable,
    Constant,
    Operator,
    Punctuation,
    Attribute,
    Embedded,

    Header(u8),
    ListItem,
    CodeBlock,

    SourceCode,

    Input,
    Quote,
    HorizontalLine,
    Indent(u8),

    DeclarationName,

    BraceMatch,

    Occurrence,
}

impl StyleId {
    pub fn is_block(self) -> bool {
        matches!(
            self,
            Self::Header(_)
                | Self::ListItem
                | Self::CodeBlock
                | Self::SourceCode
                | Self::Quote
                | Self::HorizontalLine
                | Self::Indent(_)
        )
    }

    fn slot(self) -> usize {
        match self {
            Self::Emphasis => 0,
            Self::Strong => 1,
            Self::InlineCode => 2,
            Self::Link => 3,
            Self::Strikethrough => 4,
            Self::Composing => 5,
            Self::Match => 27,
            Self::Keyword => 6,
            Self::String => 7,
            Self::Comment => 8,
            Self::Number => 9,
            Self::Type => 10,
            Self::Function => 11,
            Self::Variable => 12,
            Self::Constant => 13,
            Self::Operator => 14,
            Self::Punctuation => 15,
            Self::Attribute => 16,
            Self::Embedded => 17,
            Self::Header(1) => 18,
            Self::Header(2) => 19,
            Self::Header(3) => 20,
            Self::Header(_) => 21,
            Self::ListItem => 22,
            Self::CodeBlock => 23,
            Self::SourceCode => 34,
            Self::Quote => 24,
            Self::HorizontalLine => 25,
            Self::Indent(_) => 26,
            Self::DiffAdded => 28,
            Self::DiffDeleted => 29,
            Self::DiffModified => 30,
            Self::DiffAddedWord => 31,
            Self::DiffDeletedWord => 32,
            Self::DiffModifiedWord => 33,
            Self::Input => 35,
            Self::DeclarationName => 36,
            Self::BraceMatch => 37,
            Self::Occurrence => 38,
        }
    }

    const SLOTS: usize = 39;

    fn all_slots() -> [StyleId; Self::SLOTS] {
        [
            Self::Emphasis,
            Self::Strong,
            Self::InlineCode,
            Self::Link,
            Self::Strikethrough,
            Self::Composing,
            Self::Keyword,
            Self::String,
            Self::Comment,
            Self::Number,
            Self::Type,
            Self::Function,
            Self::Variable,
            Self::Constant,
            Self::Operator,
            Self::Punctuation,
            Self::Attribute,
            Self::Embedded,
            Self::Header(1),
            Self::Header(2),
            Self::Header(3),
            Self::Header(4),
            Self::ListItem,
            Self::CodeBlock,
            Self::Quote,
            Self::HorizontalLine,
            Self::Indent(0),
            Self::Match,
            Self::DiffAdded,
            Self::DiffDeleted,
            Self::DiffModified,
            Self::DiffAddedWord,
            Self::DiffDeletedWord,
            Self::DiffModifiedWord,
            Self::SourceCode,
            Self::Input,
            Self::DeclarationName,
            Self::BraceMatch,
            Self::Occurrence,
        ]
    }

    fn entry_name(self) -> &'static str {
        match self {
            Self::Emphasis => "emphasis",
            Self::Strong => "strong",
            Self::InlineCode => "inline_code",
            Self::Link => "link",
            Self::Strikethrough => "strikethrough",
            Self::Composing => "composing",
            Self::Match => "match",
            Self::DiffAdded => "diff-added",
            Self::DiffDeleted => "diff-deleted",
            Self::DiffModified => "diff-modified",
            Self::DiffAddedWord => "diff-added-word",
            Self::DiffDeletedWord => "diff-deleted-word",
            Self::DiffModifiedWord => "diff-modified-word",
            Self::Keyword => "keyword",
            Self::String => "string",
            Self::Comment => "comment",
            Self::Number => "number",
            Self::Type => "type",
            Self::Function => "function",
            Self::Variable => "variable",
            Self::Constant => "constant",
            Self::Operator => "operator",
            Self::Punctuation => "punctuation",
            Self::Attribute => "attribute",
            Self::Embedded => "embedded",
            Self::Header(1) => "header1",
            Self::Header(2) => "header2",
            Self::Header(3) => "header3",
            Self::Header(_) => "header4",
            Self::ListItem => "list_item",
            Self::CodeBlock => "code_block",
            Self::SourceCode => "source_code",
            Self::Input => "input",
            Self::DeclarationName => "declaration_name",
            Self::BraceMatch => "brace_match",
            Self::Occurrence => "occurrence",
            Self::Quote => "quote",
            Self::HorizontalLine => "horizontal_line",
            Self::Indent(_) => "indent",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextAlignment {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundKind {
    Text,

    Block,

    Box,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundHeight {
    Line,

    Tight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundExtent {
    Glyphs,

    ToLineEnd,

    WholeLine,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Background {
    pub color: Color,
    pub kind: BackgroundKind,
    pub extent: BackgroundExtent,
    pub height: BackgroundHeight,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextAttributes {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
    pub color: Option<Color>,
    pub background: Option<Background>,

    pub font_families: Option<std::sync::Arc<[String]>>,

    pub font_size: Option<f32>,

    pub line_height: Option<f32>,

    pub gutter: Option<Color>,

    pub rule: Option<Color>,

    pub block_gap: Option<f32>,

    pub inset: Option<f32>,

    pub block_height: Option<f32>,

    pub alignment: Option<TextAlignment>,

    /// The scroll-bar stripe color (docs/scroll-stripe.md): a style
    /// with one contributes marks to the scroll track; a style whose
    /// ONLY policy is a stripe color paints nothing in the text.
    pub stripe: Option<Color>,
}

#[derive(Clone)]
pub struct Theme {
    name: Arc<str>,

    slots: Arc<[TextAttributes; StyleId::SLOTS]>,

    base: TextAttributes,

    ui: Arc<UiTheme>,
}

impl Theme {
    pub fn embedded() -> Self {
        static EMBEDDED: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
        EMBEDDED
            .get_or_init(|| {
                Self::from_json(include_str!("../assets/theme.json"))
                    .expect("the embedded theme parses")
            })
            .clone()
    }

    pub fn light() -> Self {
        static LIGHT: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
        LIGHT
            .get_or_init(|| {
                Self::from_json(include_str!("../assets/theme-light.json"))
                    .expect("the embedded light theme parses")
            })
            .clone()
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let raw: RawTheme = serde_json::from_str(json).map_err(|error| error.to_string())?;
        let entries: HashMap<String, TextAttributes> = raw
            .styles
            .into_iter()
            .map(|(name, entry)| Ok((name, entry.resolve()?)))
            .collect::<Result<_, String>>()?;
        let base = entries.get("base").cloned().unwrap_or_default();
        let slots = StyleId::all_slots()
            .map(|id| entries.get(id.entry_name()).cloned().unwrap_or_default());
        Ok(Self {
            name: raw.name.into(),
            slots: Arc::new(slots),
            base,
            ui: Arc::new(raw.ui),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn name_shared(&self) -> Arc<str> {
        Arc::clone(&self.name)
    }

    pub fn base(&self) -> &TextAttributes {
        &self.base
    }

    pub fn ui(&self) -> &UiTheme {
        &self.ui
    }

    pub fn attributes(&self, id: StyleId) -> &TextAttributes {
        &self.slots[id.slot()]
    }

    pub fn resolve(&self, ids: impl IntoIterator<Item = StyleId>) -> TextAttributes {
        let mut merged = self.base.clone();
        for id in ids {
            merged.merge(self.attributes(id));
        }
        merged
    }
}

impl TextAttributes {
    pub fn merge(&mut self, over: &TextAttributes) {
        self.bold |= over.bold;
        self.italic |= over.italic;
        self.strikethrough |= over.strikethrough;
        self.underline |= over.underline;
        if over.color.is_some() {
            self.color = over.color;
        }
        if over.background.is_some() {
            self.background = over.background;
        }
        if over.font_families.is_some() {
            self.font_families = over.font_families.clone();
        }
        if over.font_size.is_some() {
            self.font_size = over.font_size;
        }
        if over.line_height.is_some() {
            self.line_height = over.line_height;
        }
        if over.gutter.is_some() {
            self.gutter = over.gutter;
        }
        if over.rule.is_some() {
            self.rule = over.rule;
        }
        if let Some(inset) = over.inset {
            self.inset = Some(self.inset.unwrap_or(0.0) + inset);
        }
        if over.block_gap.is_some() {
            self.block_gap = over.block_gap;
        }
        if over.block_height.is_some() {
            self.block_height = over.block_height;
        }
        if over.alignment.is_some() {
            self.alignment = over.alignment;
        }
        if over.stripe.is_some() {
            self.stripe = over.stripe;
        }
    }
}

#[derive(Deserialize)]
struct RawTheme {
    #[serde(default = "default_theme_name")]
    name: String,
    styles: HashMap<String, RawEntry>,
    ui: UiTheme,
}

fn default_theme_name() -> String {
    "dark".to_owned()
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(default)]
    bold: bool,
    #[serde(default)]
    italic: bool,
    #[serde(default)]
    strikethrough: bool,
    #[serde(default)]
    underline: bool,
    color: Option<String>,
    background: Option<String>,

    background_kind: Option<String>,
    background_extent: Option<String>,

    background_height: Option<String>,
    font_families: Option<Vec<String>>,
    font_size: Option<f32>,
    line_height: Option<f32>,
    gutter: Option<String>,
    rule: Option<String>,
    #[serde(default)]
    block_gap: Option<f32>,
    #[serde(default)]
    inset: Option<f32>,
    #[serde(default)]
    block_height: Option<f32>,
    #[serde(default)]
    alignment: Option<String>,
    #[serde(default)]
    stripe: Option<String>,
}

impl RawEntry {
    fn resolve(self) -> Result<TextAttributes, String> {
        Ok(TextAttributes {
            bold: self.bold,
            italic: self.italic,
            strikethrough: self.strikethrough,
            underline: self.underline,
            color: self.color.as_deref().map(parse_color).transpose()?,
            background: self
                .background
                .as_deref()
                .map(parse_color)
                .transpose()?
                .map(|color| Background {
                    color,
                    kind: match self.background_kind.as_deref() {
                        Some("block") => BackgroundKind::Block,
                        Some("box") => BackgroundKind::Box,
                        _ => BackgroundKind::Text,
                    },
                    extent: match self.background_extent.as_deref() {
                        Some("glyphs") => BackgroundExtent::Glyphs,
                        Some("whole-line") => BackgroundExtent::WholeLine,
                        _ => BackgroundExtent::ToLineEnd,
                    },
                    height: match self.background_height.as_deref() {
                        Some("tight") => BackgroundHeight::Tight,
                        _ => BackgroundHeight::Line,
                    },
                }),
            font_families: self.font_families.map(Into::into),
            font_size: self.font_size,
            line_height: self.line_height,
            gutter: self.gutter.as_deref().map(parse_color).transpose()?,
            rule: self.rule.as_deref().map(parse_color).transpose()?,
            alignment: match self.alignment.as_deref() {
                None => None,
                Some("left") => Some(TextAlignment::Left),
                Some("center") => Some(TextAlignment::Center),
                Some("right") => Some(TextAlignment::Right),
                Some(other) => return Err(format!("unknown alignment {other:?}")),
            },
            block_gap: self.block_gap,
            inset: self.inset,
            block_height: self.block_height,
            stripe: self.stripe.as_deref().map(parse_color).transpose()?,
        })
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(try_from = "String")]
pub struct Rgba(pub Color);

impl TryFrom<String> for Rgba {
    type Error = String;
    fn try_from(hex: String) -> Result<Self, String> {
        parse_color(&hex).map(Rgba)
    }
}

impl From<Rgba> for Color {
    fn from(rgba: Rgba) -> Color {
        rgba.0
    }
}

#[derive(Clone, Deserialize)]
pub struct UiTheme {
    #[serde(default = "air::defaults")]
    pub air: air::AirTheme,

    pub window: WindowChrome,

    #[serde(default)]
    pub toolbar: ToolbarChrome,
    pub panel: PanelChrome,

    #[serde(default)]
    pub editor_gutter: EditorGutterChrome,

    #[serde(default)]
    pub fold_chip: FoldChipChrome,

    #[serde(default)]
    pub tree: TreeChrome,
    pub caret: CaretChrome,
    pub code_panel: CodePanelChrome,
    pub rule: RuleChrome,
    pub gutter: GutterChrome,
    pub scrollbar: ScrollbarChrome,
    pub stats: StatsChrome,
    pub peeker: PeekerChrome,
    pub search: SearchChrome,
    pub table: TableChrome,

    #[serde(default)]
    pub comment: CommentChrome,
    pub checkbox: CheckboxChrome,
    pub terminal: TerminalChrome,
    pub diff: DiffChrome,

    #[serde(default)]
    pub chat: ChatChrome,

    #[serde(default)]
    pub combo: ComboChrome,

    #[serde(default)]
    pub sheet: SheetChrome,

    #[serde(default)]
    pub scroll_stripe: ScrollStripeChrome,
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ScrollStripeChrome {
    pub width: f32,

    /// The lane's gap LEFT of the scrollbar's own lane — the marks
    /// and the knob never contend.
    pub inset: f32,

    pub min_height: f32,
}

impl Default for ScrollStripeChrome {
    fn default() -> Self {
        Self {
            width: 8.0,
            inset: 6.0,
            min_height: 6.0,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct SheetChrome {
    pub width: f32,

    pub margin: f32,

    pub cast: f32,

    pub collapsed: f32,
    pub border: Rgba,
    pub cast_border: Rgba,
}

impl Default for SheetChrome {
    fn default() -> Self {
        Self {
            width: 1520.0,
            margin: 32.0,
            cast: 16.0,
            collapsed: 168.0,
            border: Rgba(Color::from_argb(0xff, 0x3a, 0x42, 0x5c)),
            cast_border: Rgba(Color::from_argb(0xff, 0x23, 0x29, 0x3d)),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ComboChrome {
    pub pad: f32,

    pub gap: f32,
    pub label_size: f32,
    pub label_color: Rgba,
    pub value_size: f32,
    pub value_color: Rgba,

    pub chevron: f32,
    pub chevron_color: Rgba,
    pub menu_fill: Rgba,
    pub menu_border: Rgba,
    pub menu_row_height: f32,
    pub menu_row_size: f32,
    pub menu_text: Rgba,

    pub menu_trail: Rgba,

    pub menu_pad: f32,
    pub menu_min_width: f32,
}

impl Default for ComboChrome {
    fn default() -> Self {
        Self {
            pad: 28.0,
            gap: 16.0,
            label_size: 17.0,
            label_color: Rgba(Color::from_argb(0x85, 0xa8, 0x5c, 0x68)),
            value_size: 25.0,
            value_color: Rgba(Color::from_argb(0xff, 0xdc, 0xe3, 0xf2)),
            chevron: 22.0,
            chevron_color: Rgba(Color::from_argb(0xb5, 0xc0, 0x8b, 0x96)),
            menu_fill: Rgba(Color::from_argb(0xff, 0x1d, 0x23, 0x33)),
            menu_border: Rgba(Color::from_argb(0xff, 0x2a, 0x2d, 0x3d)),
            menu_row_height: 44.0,
            menu_row_size: 24.0,
            menu_text: Rgba(Color::from_argb(0xff, 0xdc, 0xe3, 0xf2)),
            menu_trail: Rgba(Color::from_argb(0x85, 0xa8, 0x5c, 0x68)),
            menu_pad: 16.0,
            menu_min_width: 240.0,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ChatChrome {
    pub pad: f32,

    pub gap: f32,
    pub radius: f32,

    pub min_cell_height: f32,

    pub user_width_ratio: f32,
    pub user_surface: Rgba,
    pub user_border: Rgba,

    pub thought_surface: Rgba,

    pub tool_surface: Rgba,

    pub error_surface: Rgba,

    pub notice_color: Rgba,
    pub text_color: Rgba,
    pub title_size: f32,

    pub loader_color: Rgba,
    pub loader_height: f32,

    pub added_color: Rgba,
    pub removed_color: Rgba,

    pub input_height: f32,
    pub input_fill: Rgba,

    pub input_max_height: f32,

    pub input_border: Rgba,
    pub input_focus_border: Rgba,

    pub accent: Rgba,
    pub on_accent: Rgba,

    pub stop_color: Rgba,

    pub ask_surface: Rgba,
    pub ask_border: Rgba,
}

impl Default for ChatChrome {
    fn default() -> Self {
        Self {
            pad: 16.0,
            gap: 12.0,
            radius: 10.0,
            min_cell_height: 40.0,
            user_width_ratio: 0.78,
            user_surface: Rgba(Color::from_argb(0xff, 0x24, 0x2c, 0x42)),
            user_border: Rgba(Color::from_argb(0x50, 0x56, 0x5f, 0x89)),
            thought_surface: Rgba(Color::from_argb(0x60, 0x1d, 0x23, 0x33)),
            tool_surface: Rgba(Color::from_argb(0xa0, 0x1d, 0x23, 0x33)),
            error_surface: Rgba(Color::from_argb(0x38, 0xd4, 0x4a, 0x4a)),
            notice_color: Rgba(Color::from_argb(0xa0, 0x92, 0x9e, 0xb6)),
            text_color: Rgba(Color::from_argb(0xff, 0xd8, 0xde, 0xe9)),
            title_size: 22.0,
            loader_color: Rgba(Color::from_argb(0xa0, 0x92, 0x9e, 0xb6)),
            loader_height: 56.0,
            added_color: Rgba(Color::from_argb(0xff, 0x62, 0xc0, 0x73)),
            removed_color: Rgba(Color::from_argb(0xff, 0xd4, 0x6a, 0x6a)),
            input_height: 96.0,
            input_fill: Rgba(Color::from_argb(0xff, 0x1d, 0x23, 0x33)),
            input_max_height: 360.0,
            input_border: Rgba(Color::from_argb(0x60, 0x56, 0x5f, 0x89)),
            input_focus_border: Rgba(Color::from_argb(0xff, 0x3f, 0x74, 0xd4)),
            accent: Rgba(Color::from_argb(0xff, 0x3f, 0x74, 0xd4)),
            on_accent: Rgba(Color::from_argb(0xff, 0xf2, 0xf5, 0xfc)),
            stop_color: Rgba(Color::from_argb(0xff, 0xc9, 0x45, 0x4a)),
            ask_surface: Rgba(Color::from_argb(0xff, 0x1c, 0x26, 0x3e)),
            ask_border: Rgba(Color::from_argb(0xff, 0x2f, 0x4a, 0x7a)),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ToolbarChrome {
    pub height: f32,
    pub background: Rgba,

    pub rule: Rgba,

    pub well_width_ratio: f32,
    pub well_width_min: f32,
    pub well_width_max: f32,
    pub well_height: f32,
    pub well_radius: f32,
    pub well_fill: Rgba,

    pub well_fill_focused: Rgba,

    pub title_size: f32,
    pub title_color: Rgba,

    pub input_inset_x: f32,

    pub input_shrink: f32,

    pub button_size: f32,
    pub button_gap: f32,
    pub button_inset: f32,
    pub button_radius: f32,
    pub glyph_color: Rgba,
}

impl Default for ToolbarChrome {
    fn default() -> Self {
        Self {
            height: 64.0,
            background: Rgba(Color::new(0xff0b0d13)),
            rule: Rgba(Color::new(0x8950565f)),
            well_width_ratio: 0.32,
            well_width_min: 360.0,
            well_width_max: 560.0,
            well_height: 44.0,
            well_radius: 10.0,
            well_fill: Rgba(Color::new(0xff171c2a)),
            well_fill_focused: Rgba(Color::new(0xff1d2436)),
            title_size: 24.0,
            title_color: Rgba(Color::new(0xa896828d)),
            input_inset_x: 18.0,
            input_shrink: 8.0,
            button_size: 44.0,
            button_gap: 10.0,
            button_inset: 18.0,
            button_radius: 9.0,
            glyph_color: Rgba(Color::new(0xa896828d)),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct EditorGutterChrome {
    pub width: f32,
    pub number_size: f32,
    pub number_color: Rgba,

    pub pad: f32,

    pub fold_size: f32,

    pub stripe_width: f32,

    pub stripe_inset: f32,
    pub stripe_added: Rgba,
    pub stripe_modified: Rgba,

    pub stripe_deleted: Rgba,
}

impl Default for EditorGutterChrome {
    fn default() -> Self {
        Self {
            width: 112.0,
            number_size: 22.0,
            number_color: Rgba(Color::new(0x5c96a2ac)),
            pad: 18.0,
            fold_size: 22.0,
            stripe_width: 6.0,
            stripe_inset: 6.0,
            stripe_added: Rgba(Color::new(0xcc4fb069)),
            stripe_modified: Rgba(Color::new(0xcc4f8ab0)),
            stripe_deleted: Rgba(Color::new(0xccc75c5c)),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct FoldChipChrome {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub fill: Rgba,
    pub dots: Rgba,
}

impl Default for FoldChipChrome {
    fn default() -> Self {
        Self {
            width: 46.0,
            height: 26.0,
            radius: 8.0,
            fill: Rgba(Color::new(0x2e96a2ac)),
            dots: Rgba(Color::new(0xc8b6c2cc)),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct TreeChrome {
    pub font_size: f32,

    pub indent: f32,

    pub text_x: f32,

    pub highlight: Rgba,

    /// Directory labels — LIGHT, the declaration-name register, not
    /// the darker keyword blue.
    pub directory: Rgba,
    /// File labels.
    pub file: Rgba,
}

impl Default for TreeChrome {
    fn default() -> Self {
        Self {
            font_size: 24.0,
            indent: 26.0,
            text_x: 36.0,
            highlight: Rgba(Color::new(0x2e50565f)),
            directory: Rgba(Color::from_argb(0xff, 0xde, 0xe5, 0xf2)),
            file: Rgba(Color::from_argb(0xff, 0xc9, 0xd2, 0xe4)),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct DiffChrome {
    pub spacer_fill: Rgba,

    pub fold_height: f32,
    pub fold_background: Rgba,

    pub fold_rule: Rgba,

    pub fold_text: Rgba,
    pub fold_text_size: f32,

    pub fold_button: Rgba,
    pub fold_button_size: f32,
}

impl Default for DiffChrome {
    fn default() -> Self {
        Self {
            spacer_fill: Rgba(Color::new(0x14ffffff)),
            fold_height: 40.0,
            fold_background: Rgba(Color::new(0xff11141b)),
            fold_rule: Rgba(Color::new(0x2effffff)),
            fold_text: Rgba(Color::new(0x9fb6c2cc)),
            fold_text_size: 22.0,
            fold_button: Rgba(Color::new(0xccb6c2cc)),
            fold_button_size: 28.0,
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct TerminalChrome {
    pub font_families: Vec<String>,
    pub font_size: f32,

    pub background: Rgba,

    pub foreground: Rgba,
    pub cursor: Rgba,

    pub pad: f32,

    pub palette: Vec<Rgba>,
}

#[derive(Clone, Deserialize)]
pub struct WindowChrome {
    pub background: Rgba,
    pub divider: Rgba,
    pub divider_width: f32,

    pub divider_inset: f32,
    pub margin_ratio: f32,
    pub margin_min: f32,
    pub margin_max: f32,
    pub top_ratio: f32,
    pub top_min: f32,
    pub top_max: f32,

    pub content_pad: f32,

    pub min_editor_width: f32,

    pub first_pane_width: f32,
}

#[derive(Clone, Deserialize)]
pub struct PanelChrome {
    pub header_height: f32,
    pub header_fill: Rgba,
    pub header_rule: Rgba,
    pub title_color: Rgba,
    pub title_size: f32,
    pub title_x: f32,
    pub title_baseline: f32,
}

#[derive(Clone, Deserialize)]
pub struct CaretChrome {
    pub color: Rgba,

    pub selection: Rgba,
    pub width: f32,

    pub inset: f32,
    pub min_height: f32,
}

#[derive(Clone, Deserialize)]
pub struct CodePanelChrome {
    pub top_offset: f32,
    pub radius: f32,
}

#[derive(Clone, Deserialize)]
pub struct RuleChrome {
    pub offset: f32,
    pub thickness: f32,
}

#[derive(Clone, Deserialize)]
pub struct GutterChrome {
    pub width: f32,

    pub inset: f32,
}

#[derive(Clone, Deserialize)]
pub struct ScrollbarChrome {
    pub color: Rgba,
    pub width: f32,

    pub margin: f32,
    pub radius: f32,
    pub min_knob: f32,

    pub track_inset: f32,
}

#[derive(Clone, Deserialize)]
pub struct StatsChrome {
    pub background: Rgba,
    pub text: Rgba,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub top: f32,
    pub right_margin: f32,
    pub pad_x: f32,
    pub first_baseline: f32,
    pub line_height: f32,
    pub font_size: f32,
}

#[derive(Clone, Deserialize)]
pub struct PeekerChrome {
    pub background: Rgba,
    pub well: Rgba,
    pub well_radius: f32,
    pub text: Rgba,
    pub dim_text: Rgba,
    pub rule: Rgba,

    pub rule_inset: f32,
    pub highlight: Rgba,
    pub highlight_radius: f32,
    pub accent: Rgba,
    pub accent_width: f32,
    pub accent_inset: f32,
    pub dir_text: Rgba,
    pub row_height: f32,
    pub input_height: f32,
    pub margin: f32,
    pub title_baseline: f32,
    pub hint_bottom: f32,
    pub input_inset_x: f32,
    pub input_inset_y: f32,

    pub input_pad: f32,

    pub input_shrink: f32,
    pub title_size: f32,
    pub row_size: f32,
    pub hint_size: f32,
    pub no_preview_offset: f32,

    pub list_ratio: f32,
    pub list_min: f32,
    pub list_max: f32,

    pub list_preview_gap: f32,

    pub preview_margin: f32,

    pub list_top_gap: f32,

    pub row_text_x: f32,
    pub input_min_width: f32,
}

#[derive(Clone, Deserialize)]
pub struct SearchChrome {
    pub input_height: f32,
    pub pad: f32,

    pub input_outset: f32,

    pub input_pad_x: f32,
    pub input_pad_y: f32,
    pub input_fill: Rgba,
    pub group_header: f32,
    pub group_fill: Rgba,
    pub group_text: Rgba,
    pub group_text_x: f32,
    pub group_font_size: f32,

    pub group_separator: Rgba,
    pub group_separator_thickness: f32,

    pub row_separator: Rgba,
    pub row_separator_inset: f32,
    pub row_separator_thickness: f32,
}

#[derive(Clone, Deserialize)]
pub struct CheckboxChrome {
    pub size: f32,
    pub radius: f32,
    pub stroke: f32,
    pub border: Rgba,
    pub fill: Rgba,
    pub check: Rgba,
}

#[derive(Clone, Deserialize)]
pub struct TableChrome {
    pub grid: Rgba,
    pub grid_strong: Rgba,

    pub thickness: f32,
    pub cell_pad_x: f32,
    pub cell_pad_y: f32,

    pub column_floor: f32,

    pub column_cap: f32,

    pub quantum: f32,

    pub fallback_width: f32,

    pub row_floor: f32,
    pub control_size: f32,
    pub control_fill: Rgba,
    pub control_glyph: Rgba,
    pub control_remove: Rgba,
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct CommentChrome {
    pub surface: Rgba,
    pub border: Rgba,
    pub cross: Rgba,

    pub pad: f32,

    pub close_size: f32,

    pub min_editor_height: f32,
    pub radius: f32,
}

impl Default for CommentChrome {
    fn default() -> Self {
        Self {
            surface: Rgba(Color::from_argb(0xf0, 0x1d, 0x23, 0x33)),
            border: Rgba(Color::from_argb(0x50, 0x56, 0x5f, 0x89)),
            cross: Rgba(Color::from_argb(0xb0, 0x92, 0x9e, 0xb6)),
            pad: 16.0,
            close_size: 24.0,
            min_editor_height: 40.0,
            radius: 10.0,
        }
    }
}

fn parse_color(hex: &str) -> Result<Color, String> {
    let digits = hex.strip_prefix('#').unwrap_or(hex);
    let value = u32::from_str_radix(digits, 16).map_err(|error| format!("{hex}: {error}"))?;
    Ok(match digits.len() {
        6 => Color::from_argb(0xff, (value >> 16) as u8, (value >> 8) as u8, value as u8),
        8 => Color::from_argb(
            (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ),
        _ => return Err(format!("{hex}: expected #rrggbb or #aarrggbb")),
    })
}

pub type HiddenRange = Range<u32>;

#[cfg(test)]
mod tests;
