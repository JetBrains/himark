// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use skia_safe::Color;

pub use imba::theme::{parse_color, Key, Rgba, Theme, TypeRole};

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

    /// An identifier the parse found — a goto-definition candidate
    /// (docs/editor/highlight.md). Emitted by the tree-sitter pass
    /// alongside the color spans; invisible at rest (the theme entry
    /// is empty) — the hovered-link look is the editor's, not the
    /// span's.
    Reference,

    DiagnosticError,
    DiagnosticWarning,
    DiagnosticInfo,
    DiagnosticHint,

    /// A language server's inlay hint (a parameter name, an inferred
    /// type): the dim chip the hint inlay paints with.
    InlayHint,

    /// The transcript's PROSE register (docs/Design.md, the
    /// typographic redesign): the chrome face at reading size — a
    /// covering style a chat cell's markdown document wears, so the
    /// editor's own base (code and documents) stays untouched.
    Chat,

    /// The user's own message in the transcript: the prompt register,
    /// larger and heavier than the agent's reading prose.
    ChatPrompt,

    /// A modal's QUERY input (the palette, the jump sheet): the
    /// chrome face, large and light.
    Query,

    /// A column's query input (Find): the query register at the side
    /// column's size.
    QuerySmall,

    /// A FIELD input in a column (a filter, an inline rename): the
    /// chrome face at the field size.
    Field,

    /// The commit SUBJECT being written (the working-copy canvas's
    /// composer): the HEADLINE register — the type the line wears once
    /// committed — so the message is typed where its headline will
    /// stand, at its size and width.
    Headline,
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
            Self::Reference => 39,
            Self::DiagnosticError => 40,
            Self::DiagnosticWarning => 41,
            Self::DiagnosticInfo => 42,
            Self::DiagnosticHint => 43,
            Self::InlayHint => 44,
            Self::Chat => 45,
            Self::ChatPrompt => 46,
            Self::Query => 47,
            Self::QuerySmall => 48,
            Self::Field => 49,
            Self::Headline => 50,
        }
    }

    const SLOTS: usize = 51;

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
            Self::Reference,
            Self::DiagnosticError,
            Self::DiagnosticWarning,
            Self::DiagnosticInfo,
            Self::DiagnosticHint,
            Self::InlayHint,
            Self::Chat,
            Self::ChatPrompt,
            Self::Query,
            Self::QuerySmall,
            Self::Field,
            Self::Headline,
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
            Self::Reference => "reference",
            Self::DiagnosticError => "diagnostic_error",
            Self::DiagnosticWarning => "diagnostic_warning",
            Self::DiagnosticInfo => "diagnostic_info",
            Self::DiagnosticHint => "diagnostic_hint",
            Self::InlayHint => "inlay_hint",
            Self::Chat => "chat",
            Self::ChatPrompt => "chat_prompt",
            Self::Query => "query",
            Self::QuerySmall => "query_small",
            Self::Field => "field",
            Self::Headline => "headline",
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

    /// A weight on the CSS scale (100..900): a variable family (the
    /// chrome face) instances its axis to it; `bold` stays the static
    /// cut's switch.
    pub font_weight: Option<f32>,

    /// A width on the CSS `font-stretch` scale (percent): a variable
    /// family instances its `wdth` axis to it — the headline's
    /// condensed cut.
    pub font_width: Option<f32>,

    pub line_height: Option<f32>,

    pub gutter: Option<Color>,

    pub rule: Option<Color>,

    pub block_gap: Option<f32>,

    pub inset: Option<f32>,

    pub block_height: Option<f32>,

    pub alignment: Option<TextAlignment>,

    /// The scroll-bar stripe color (docs/editor/scroll-stripe.md): a style
    /// with one contributes marks to the scroll track; a style whose
    /// ONLY policy is a stripe color paints nothing in the text.
    pub stripe: Option<Color>,

    /// A wavy underline in this color — the diagnostics look. Wins
    /// over the plain underline's solid style where both apply.
    pub squiggle: Option<Color>,
}

/// The embedded dark theme.
pub fn embedded() -> Theme {
    static EMBEDDED: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
    EMBEDDED
        .get_or_init(|| {
            Theme::from_json(include_str!("../assets/theme.json"))
                .expect("the embedded theme parses")
        })
        .clone()
}

/// The embedded light theme.
pub fn light() -> Theme {
    static LIGHT: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
    LIGHT
        .get_or_init(|| {
            Theme::from_json(include_str!("../assets/theme-light.json"))
                .expect("the embedded light theme parses")
        })
        .clone()
}

/// The embedded iOS dark theme — the same inks, the PHONE's metrics:
/// physical pixels for a 3x screen, rows that are tap targets, the
/// phone's insets (assets/README.md, "The iOS pair").
pub fn ios() -> Theme {
    static IOS: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
    IOS.get_or_init(|| {
        Theme::from_json(include_str!("../assets/theme-ios.json"))
            .expect("the embedded iOS theme parses")
    })
    .clone()
}

/// The embedded iOS light theme.
pub fn ios_light() -> Theme {
    static IOS_LIGHT: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
    IOS_LIGHT
        .get_or_init(|| {
            Theme::from_json(include_str!("../assets/theme-ios-light.json"))
                .expect("the embedded iOS light theme parses")
        })
        .clone()
}

/// The text styles: the editor's section of the open theme — the
/// `StyleId` slots plus the base every resolve starts from. Other
/// crates reach it through [`ThemeStyles::styles`]; nothing outside
/// the editor needs to know the slots exist.
#[derive(Clone)]
pub struct Styles {
    slots: Arc<[TextAttributes; StyleId::SLOTS]>,
    base: TextAttributes,
}

impl Default for Styles {
    /// The dark styles — the open theme's doctrine is that every
    /// part's default IS the dark value, so a theme (or a bare test
    /// store's empty theme) missing the "styles" section reads like
    /// the embedded one.
    fn default() -> Self {
        embedded().styles().as_ref().clone()
    }
}

impl Styles {
    pub const KEY: Key<Styles> = Key::new("styles");

    pub fn base(&self) -> &TextAttributes {
        &self.base
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

impl<'de> Deserialize<'de> for Styles {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw: HashMap<String, RawEntry> = HashMap::deserialize(deserializer)?;
        let entries: HashMap<String, TextAttributes> = raw
            .into_iter()
            .map(|(name, entry)| Ok((name, entry.resolve().map_err(serde::de::Error::custom)?)))
            .collect::<Result<_, D::Error>>()?;
        let base = entries.get("base").cloned().unwrap_or_default();
        let slots = StyleId::all_slots()
            .map(|id| entries.get(id.entry_name()).cloned().unwrap_or_default());
        Ok(Self {
            slots: Arc::new(slots),
            base,
        })
    }
}

/// The editor's view of any [`Theme`]: its text styles. A pass
/// fetches the part once and borrows attributes out of it.
pub trait ThemeStyles {
    fn styles(&self) -> Arc<Styles>;

    fn resolve(&self, ids: impl IntoIterator<Item = StyleId>) -> TextAttributes
    where
        Self: Sized,
    {
        self.styles().resolve(ids)
    }
}

impl ThemeStyles for Theme {
    fn styles(&self) -> Arc<Styles> {
        self.part(Styles::KEY)
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
        if over.font_weight.is_some() {
            self.font_weight = over.font_weight;
        }
        if over.font_width.is_some() {
            self.font_width = over.font_width;
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
        if over.squiggle.is_some() {
            self.squiggle = over.squiggle;
        }
    }
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
    font_weight: Option<f32>,
    font_width: Option<f32>,
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
    #[serde(default)]
    squiggle: Option<String>,
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
            font_weight: self.font_weight,
            font_width: self.font_width,
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
            squiggle: self.squiggle.as_deref().map(parse_color).transpose()?,
        })
    }
}

/// The ink palette. Every color is physical ARGB; the names are the
/// design's: GROUND is the page, RULE the hairline, three grades of
/// ink from primary to label, LIVE the caret-and-here accent.
impl InkChrome {
    pub const KEY: Key<InkChrome> = Key::new("ink");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct InkChrome {
    pub ground: Rgba,
    pub rule: Rgba,
    /// A hairline a shade stronger than `rule` — the composer's
    /// baseline, the dotted leaders.
    pub rule_strong: Rgba,
    /// Ink — primary: titles, the focused row, the headline.
    pub ink: Rgba,
    /// Ink 2 — secondary: reading prose, directory names.
    pub ink_2: Rgba,
    /// Ink 3 — labels, metadata, the dimmed tabs.
    pub ink_3: Rgba,
    /// Ink 4 — the quietest: zero counts, hints, ellipsis crumbs.
    pub ink_4: Rgba,
    /// The brightest ink: the current item's title.
    pub ink_bright: Rgba,
    /// LIVE — the caret, the here-node, the matched letters' underline.
    pub live: Rgba,
    pub added: Rgba,
    pub removed: Rgba,
    /// A row under the pointer.
    pub hover: Rgba,
    /// The selected/current row's wash.
    pub selected: Rgba,
    /// The here-commit's wash in the history rail.
    pub here: Rgba,
    /// A sheet standing over the ground (the palette, the jump dialog).
    pub sheet: Rgba,
    pub sheet_border: Rgba,
    /// The mono line-number gray.
    pub mono_dim: Rgba,
    /// The declaration-name blue the contents panel echoes.
    pub declaration: Rgba,
    /// An input's placeholder.
    pub placeholder: Rgba,
}

impl Default for InkChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            ground: Rgba(Color::new(0xff0d0d0f)),
            rule: Rgba(Color::new(0xff222226)),
            rule_strong: Rgba(Color::new(0xff2e2e33)),
            ink: Rgba(Color::new(0xffeceae4)),
            ink_2: Rgba(Color::new(0xffb5b2ab)),
            ink_3: Rgba(Color::new(0xff8a8781)),
            ink_4: Rgba(Color::new(0xff5f5d59)),
            ink_bright: Rgba(Color::new(0xfff2f0eb)),
            live: Rgba(Color::new(0xfff2b544)),
            added: Rgba(Color::new(0xff7fd08a)),
            removed: Rgba(Color::new(0xfff07a6b)),
            hover: Rgba(Color::new(0xff17171b)),
            selected: Rgba(Color::new(0xff1a1a1f)),
            here: Rgba(Color::new(0xff16161a)),
            sheet: Rgba(Color::new(0xff151518)),
            sheet_border: Rgba(Color::new(0xff2a2a2f)),
            mono_dim: Rgba(Color::new(0xff5d5d66)),
            declaration: Rgba(Color::new(0xff5cc8ff)),
            placeholder: Rgba(Color::new(0xff6e6b66)),
        }
    }
}

/// The ramp, top to bottom (docs/Design.md): QUERY the modal's big
/// light input, HEADLINE the commit you're reading, DISPLAY the
/// column's primary action, PROMPT your messages, TITLE the file /
/// session / active tab, READING the agent's prose and lists, ROW the
/// tree and list rows, BRIEF paths and metadata, LABEL the tracked
/// caps group heads, MARGIN the mono keys and tool calls.
impl TypeChrome {
    pub const KEY: Key<TypeChrome> = Key::new("type_ramp");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct TypeChrome {
    pub query: TypeRole,
    pub query_small: TypeRole,
    pub headline: TypeRole,
    pub display: TypeRole,
    pub prompt: TypeRole,
    pub title: TypeRole,
    pub subtitle: TypeRole,
    pub reading: TypeRole,
    pub row: TypeRole,
    /// A row's strong word: a directory's name, the expanded commit.
    pub row_strong: TypeRole,
    pub brief: TypeRole,
    pub label: TypeRole,
    pub margin: TypeRole,
    /// The mono face's family, for the margin role.
    pub mono_family: String,
}

impl Default for TypeChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            query: TypeRole {
                size: 88.0,
                weight: 300.0,
                width: 100.0,
                tracking: -1.76,
                line_height: 1.18,
            },
            query_small: TypeRole {
                size: 88.0,
                weight: 300.0,
                width: 100.0,
                tracking: -1.76,
                line_height: 1.18,
            },
            headline: TypeRole {
                size: 64.0,
                weight: 560.0,
                width: 88.0,
                tracking: -1.28,
                line_height: 1.08,
            },
            display: TypeRole {
                size: 48.0,
                weight: 560.0,
                width: 100.0,
                tracking: -0.72,
                line_height: 1.0,
            },
            prompt: TypeRole {
                size: 44.0,
                weight: 520.0,
                width: 100.0,
                tracking: -0.44,
                line_height: 1.27,
            },
            title: TypeRole {
                size: 34.0,
                weight: 640.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.0,
            },
            subtitle: TypeRole {
                size: 30.0,
                weight: 400.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.0,
            },
            reading: TypeRole {
                size: 30.0,
                weight: 400.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.6,
            },
            row: TypeRole {
                size: 30.0,
                weight: 400.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.86,
            },
            row_strong: TypeRole {
                size: 30.0,
                weight: 560.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.86,
            },
            brief: TypeRole {
                size: 26.0,
                weight: 400.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.0,
            },
            label: TypeRole {
                size: 22.0,
                weight: 600.0,
                width: 100.0,
                tracking: 2.2,
                line_height: 1.0,
            },
            margin: TypeRole {
                size: 22.0,
                weight: 400.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.0,
            },
            mono_family: "JetBrains Mono".to_owned(),
        }
    }
}

impl ScrollStripeChrome {
    pub const KEY: Key<ScrollStripeChrome> = Key::new("scroll_stripe");
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

impl EditorGutterChrome {
    pub const KEY: Key<EditorGutterChrome> = Key::new("editor_gutter");
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
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            width: 128.0,
            number_size: 24.0,
            number_color: Rgba(Color::new(0xff5d5d66)),
            pad: 28.0,
            fold_size: 22.0,
            stripe_width: 6.0,
            stripe_inset: 6.0,
            stripe_added: Rgba(Color::new(0xcc3ecf6e)),
            stripe_modified: Rgba(Color::new(0xcc4fa3e0)),
            stripe_deleted: Rgba(Color::new(0xcceb5d5d)),
        }
    }
}

impl FoldChipChrome {
    pub const KEY: Key<FoldChipChrome> = Key::new("fold_chip");
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
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            width: 46.0,
            height: 26.0,
            radius: 8.0,
            fill: Rgba(Color::new(0x2e9b9b9f)),
            dots: Rgba(Color::new(0xc8aebce0)),
        }
    }
}

impl WindowChrome {
    pub const KEY: Key<WindowChrome> = Key::new("window");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
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

    /// The side column (the dock: History · Files · Find · Changes ·
    /// Comments) at rest, and the drawer (Sessions, Contents).
    pub side_width: f32,

    /// The chat column never shrinks below this.
    pub chat_min_width: f32,

    /// The main area keeps at least this much beside the chat column;
    /// below that the chat yields the window.
    pub main_min_width: f32,

    /// Where each column's TEXT EDGE stands — the transcript's prose
    /// (past the voice gutter), the code column (past the line
    /// numbers), the side column (past the rail).
    pub chat_text_edge: f32,
    pub code_text_edge: f32,
    pub side_text_edge: f32,
    /// The side column's right inset.
    pub side_trailing: f32,
}

impl PanelChrome {
    pub const KEY: Key<PanelChrome> = Key::new("panel");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct PanelChrome {
    pub header_height: f32,
    pub header_fill: Rgba,
    pub header_rule: Rgba,
    pub title_color: Rgba,
    pub title_size: f32,
    pub title_x: f32,
    pub title_baseline: f32,
}

impl CaretChrome {
    pub const KEY: Key<CaretChrome> = Key::new("caret");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct CaretChrome {
    pub color: Rgba,

    pub selection: Rgba,
    pub width: f32,

    pub inset: f32,
    pub min_height: f32,

    /// An INPUT's caret (a query, a filter, the composer): the LIVE
    /// ink, thicker than the code caret (docs/Design.md).
    #[serde(default = "live_caret_color")]
    pub input_color: Rgba,
    #[serde(default = "live_caret_width")]
    pub input_width: f32,
}

fn live_caret_color() -> Rgba {
    Rgba(Color::new(0xfff2b544))
}

fn live_caret_width() -> f32 {
    4.0
}

/// The sticky scope band: the enclosing declarations pinned at the
/// editor's top, on their own ground with a hairline under.
impl StickyChrome {
    pub const KEY: Key<StickyChrome> = Key::new("sticky");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct StickyChrome {
    pub background: Rgba,
    pub rule: Rgba,
}

impl Default for StickyChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            background: Rgba(Color::new(0xff0f0f12)),
            rule: Rgba(Color::new(0xff26262b)),
        }
    }
}

impl CodePanelChrome {
    pub const KEY: Key<CodePanelChrome> = Key::new("code_panel");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct CodePanelChrome {
    pub top_offset: f32,
    pub radius: f32,
}

impl RuleChrome {
    pub const KEY: Key<RuleChrome> = Key::new("rule");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct RuleChrome {
    pub offset: f32,
    pub thickness: f32,
}

impl GutterChrome {
    pub const KEY: Key<GutterChrome> = Key::new("gutter");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct GutterChrome {
    pub width: f32,

    pub inset: f32,
}

impl ScrollbarChrome {
    pub const KEY: Key<ScrollbarChrome> = Key::new("scrollbar");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ScrollbarChrome {
    pub color: Rgba,
    pub width: f32,

    pub margin: f32,
    pub radius: f32,
    pub min_knob: f32,

    pub track_inset: f32,
}

impl ScrollbarChrome {
    /// This chrome as the live-UI style: imba paints scrollbars
    /// from `UiCtx` state, not from the store's theme, so a shell
    /// hands it this shape at boot and on every theme change.
    pub fn style(&self) -> imba::scroll::ScrollbarStyle {
        imba::scroll::ScrollbarStyle {
            color: self.color.0,
            width: self.width,
            margin: self.margin,
            radius: self.radius,
            min_knob: self.min_knob,
            track_inset: self.track_inset,
        }
    }
}

impl SearchChrome {
    pub const KEY: Key<SearchChrome> = Key::new("search");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
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

impl Default for WindowChrome {
    fn default() -> Self {
        Self {
            background: Rgba(Color::new(0xff0d0d0f)),
            divider: Rgba(Color::new(0xff222226)),
            divider_width: 1.0,
            divider_inset: 0.0,
            margin_ratio: 0.0,
            margin_min: 0.0,
            margin_max: 0.0,
            top_ratio: 0.0,
            top_min: 0.0,
            top_max: 0.0,
            content_pad: 56.0,
            min_editor_width: 160.0,
            first_pane_width: 900.0,
            side_width: 720.0,
            chat_min_width: 840.0,
            main_min_width: 1120.0,
            chat_text_edge: 176.0,
            code_text_edge: 128.0,
            side_text_edge: 100.0,
            side_trailing: 40.0,
        }
    }
}

impl Default for PanelChrome {
    fn default() -> Self {
        Self {
            header_height: 104.0,
            header_fill: Rgba(Color::new(0xff0d0d0f)),
            header_rule: Rgba(Color::new(0xff222226)),
            title_color: Rgba(Color::new(0xffeceae4)),
            title_size: 34.0,
            title_x: 40.0,
            title_baseline: 66.0,
        }
    }
}

impl Default for CaretChrome {
    fn default() -> Self {
        Self {
            color: Rgba(Color::new(0xff5ca9ff)),
            selection: Rgba(Color::new(0xff2a4c8f)),
            width: 2.0,
            inset: 2.0,
            min_height: 18.0,
            input_color: Rgba(Color::new(0xfff2b544)),
            input_width: 4.0,
        }
    }
}

impl Default for CodePanelChrome {
    fn default() -> Self {
        Self {
            top_offset: 12.0,
            radius: 12.0,
        }
    }
}

impl Default for RuleChrome {
    fn default() -> Self {
        Self {
            offset: 20.0,
            thickness: 2.0,
        }
    }
}

impl Default for GutterChrome {
    fn default() -> Self {
        Self {
            width: 4.0,
            inset: 20.0,
        }
    }
}

impl Default for ScrollbarChrome {
    fn default() -> Self {
        Self {
            color: Rgba(Color::new(0x805f5d59)),
            width: 8.0,
            margin: 10.0,
            radius: 4.0,
            min_knob: 28.0,
            track_inset: 12.0,
        }
    }
}

impl Default for SearchChrome {
    fn default() -> Self {
        Self {
            input_height: 104.0,
            pad: 40.0,
            input_outset: 0.0,
            input_pad_x: 0.0,
            input_pad_y: 0.0,
            input_fill: Rgba(Color::new(0x00000000)),
            group_header: 52.0,
            group_fill: Rgba(Color::new(0x00000000)),
            group_text: Rgba(Color::new(0xffeceae4)),
            group_text_x: 100.0,
            group_font_size: 30.0,
            group_separator: Rgba(Color::new(0x00000000)),
            group_separator_thickness: 0.0,
            row_separator: Rgba(Color::new(0x00000000)),
            row_separator_inset: 0.0,
            row_separator_thickness: 0.0,
        }
    }
}

// ── the editor's SLICES of shared sections. The full structs live
// in hikit::chrome (the UI kit owns the shared look); the editor
// can't depend on hikit, so it reads its own typed view of the same
// JSON section — the open theme caches parts by (section, type).

impl PopupChrome {
    pub const KEY: Key<PopupChrome> = Key::new("peeker");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct PopupChrome {
    pub background: Rgba,
    pub text: Rgba,
    pub dim_text: Rgba,
    pub rule: Rgba,
    pub row_height: f32,
    pub row_size: f32,
    pub accent: Rgba,
    pub accent_width: f32,
}

impl Default for PopupChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            background: Rgba(Color::new(0xff0d0d0f)),
            text: Rgba(Color::new(0xffeceae4)),
            dim_text: Rgba(Color::new(0xff8a8781)),
            rule: Rgba(Color::new(0xff222226)),
            row_height: 68.0,
            row_size: 30.0,
            accent: Rgba(Color::new(0xfff2b544)),
            accent_width: 0.0,
        }
    }
}

impl TreeWell {
    pub const KEY: Key<TreeWell> = Key::new("tree");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct TreeWell {
    pub highlight: Rgba,
}

impl Default for TreeWell {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            highlight: Rgba(Color::new(0xff1a1a1f)),
        }
    }
}

impl DiffFoldChrome {
    pub const KEY: Key<DiffFoldChrome> = Key::new("diff");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct DiffFoldChrome {
    pub spacer_fill: Rgba,
    pub fold_background: Rgba,
    pub fold_text: Rgba,
    pub fold_text_size: f32,
    pub fold_button: Rgba,
    pub fold_edge_height: f32,
    pub fold_chevron_size: f32,
    pub fold_chevron_hit: f32,
    pub fold_chevron_stroke: f32,
    pub fold_gutter_inset: f32,
    pub fold_number_gap: f32,
    pub fold_summary_gap: f32,
}

impl Default for DiffFoldChrome {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            spacer_fill: Rgba(Color::new(0x10000000)),
            fold_background: Rgba(Color::new(0xff111115)),
            fold_text: Rgba(Color::new(0xff8a8781)),
            fold_text_size: 25.0,
            fold_button: Rgba(Color::new(0xffb5b2ab)),
            fold_edge_height: 44.0,
            fold_chevron_size: 28.0,
            fold_chevron_hit: 44.0,
            fold_chevron_stroke: 2.5,
            fold_gutter_inset: 24.0,
            fold_number_gap: 8.0,
            fold_summary_gap: 36.0,
        }
    }
}

impl HoverSheet {
    pub const KEY: Key<HoverSheet> = Key::new("combo");
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct HoverSheet {
    pub menu_fill: Rgba,
}

impl Default for HoverSheet {
    // The dark theme.json values — a part's default IS the dark look.
    fn default() -> Self {
        Self {
            menu_fill: Rgba(Color::new(0xff151518)),
        }
    }
}

#[cfg(test)]
mod tests;
