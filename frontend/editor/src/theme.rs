// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use skia_safe::Color;

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
        }
    }

    const SLOTS: usize = 50;

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
        if over.font_weight.is_some() {
            self.font_weight = over.font_weight;
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
    font_weight: Option<f32>,
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
    /// The INK: the handful of grays every chrome surface draws
    /// with, plus the one live accent (docs/Design.md, "Ink").
    #[serde(default)]
    pub ink: InkChrome,

    /// The TYPE RAMP: size carries rank (docs/Design.md, "Size
    /// carries rank") — every chrome label picks a role here.
    #[serde(default)]
    pub type_ramp: TypeChrome,

    pub window: WindowChrome,

    #[serde(default)]
    pub sticky: StickyChrome,

    #[serde(default)]
    pub toolbar: ToolbarChrome,
    pub panel: PanelChrome,

    #[serde(default)]
    pub editor_gutter: EditorGutterChrome,

    #[serde(default)]
    pub fold_chip: FoldChipChrome,

    #[serde(default)]
    pub tree: TreeChrome,

    /// The side column's History tab: the branch line, the rail and
    /// its nodes, the commits' registers.
    #[serde(default)]
    pub history: HistoryChrome,

    /// The side column's Files tab: the folder head, the filter
    /// field, the current row's mark.
    #[serde(default)]
    pub files: FilesChrome,

    /// The side column's Find tab: the query, its counts, the
    /// results' hit lines.
    #[serde(default)]
    pub find: FindChrome,
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
    pub scroll_stripe: ScrollStripeChrome,

    /// The Contents drawer's outline — kind, name, leader, line
    /// (docs/Design.md mock Contents.dc.html).
    #[serde(default)]
    pub contents: ContentsChrome,

    /// The Sessions drawer's recency list (docs/Design.md mock
    /// Sessions.dc.html).
    #[serde(default)]
    pub sessions: SessionsChrome,
}

/// The ink palette. Every color is physical ARGB; the names are the
/// design's: GROUND is the page, RULE the hairline, three grades of
/// ink from primary to label, LIVE the caret-and-here accent.
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

/// One rung of the type ramp: a size, a weight on the chrome face's
/// `wght` axis, a width on `wdth` (100 = normal), and the tracking
/// in physical pixels.
#[derive(Clone, Copy, Deserialize)]
#[serde(default)]
pub struct TypeRole {
    pub size: f32,
    pub weight: f32,
    pub width: f32,
    pub tracking: f32,
    /// The line height, as a multiple of the size, where a role
    /// sets lines (prose, rows).
    pub line_height: f32,
}

impl Default for TypeRole {
    fn default() -> Self {
        Self {
            size: 28.0,
            weight: 400.0,
            width: 100.0,
            tracking: 0.0,
            line_height: 1.0,
        }
    }
}

impl TypeRole {
    pub const fn new(size: f32, weight: f32) -> Self {
        Self {
            size,
            weight,
            width: 100.0,
            tracking: 0.0,
            line_height: 1.0,
        }
    }
}

/// The ramp, top to bottom (docs/Design.md): QUERY the modal's big
/// light input, HEADLINE the commit you're reading, DISPLAY the
/// column's primary action, PROMPT your messages, TITLE the file /
/// session / active tab, READING the agent's prose and lists, ROW the
/// tree and list rows, BRIEF paths and metadata, LABEL the tracked
/// caps group heads, MARGIN the mono keys and tool calls.
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
                size: 60.0,
                weight: 300.0,
                width: 100.0,
                tracking: -1.2,
                line_height: 1.2,
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
            title: TypeRole::new(34.0, 640.0),
            subtitle: TypeRole::new(28.0, 400.0),
            reading: TypeRole {
                size: 30.0,
                weight: 400.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.6,
            },
            row: TypeRole {
                size: 28.0,
                weight: 400.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.86,
            },
            row_strong: TypeRole {
                size: 28.0,
                weight: 560.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.86,
            },
            brief: TypeRole::new(26.0, 400.0),
            label: TypeRole {
                size: 22.0,
                weight: 600.0,
                width: 100.0,
                tracking: 2.2,
                line_height: 1.0,
            },
            margin: TypeRole::new(22.0, 400.0),
            mono_family: "JetBrains Mono".to_owned(),
        }
    }
}

/// The Sessions drawer: ONE recency-sorted list under caps group
/// heads, each row a three-column grid — the repo in a right-aligned
/// margin column, the title, the age — so projects interleave and
/// still read. The open session is the HERE row, a size up.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct SessionsChrome {
    /// The content's side inset inside the drawer column.
    pub inset: f32,
    /// The air above the first group head.
    pub top_gap: f32,
    /// The air above every group head after the first.
    pub group_gap: f32,
    /// The repo column's width; the group heads indent past it.
    pub repo_width: f32,
    /// The gap between the grid's columns.
    pub column_gap: f32,
    /// A session row's line box and its symmetric air.
    pub row_line: f32,
    pub row_pad: f32,
    /// The repo and age columns' size (ink 3).
    pub meta_size: f32,
    /// A session title's ink — a shade under primary, so the here
    /// row's bright title outranks it.
    pub title_color: Rgba,
    /// The open session's title rung and its line box, with the
    /// asymmetric air that seats it (more above than below).
    pub here_title: TypeRole,
    pub here_line: f32,
    pub here_pad_top: f32,
    pub here_pad_bottom: f32,
    /// The synthetic oblique's skew — the chrome face has no italic
    /// cut ("untitled", slash commands, a host's "offline").
    pub oblique_skew: f32,
    /// The host foot: the air above its hairline, the air under it,
    /// and the air between two hosts.
    pub foot_gap: f32,
    pub foot_pad: f32,
    pub host_gap: f32,
    /// A host's name rung, its status word's size, and the gap
    /// between them.
    pub host_name: TypeRole,
    pub host_status_size: f32,
    pub host_word_gap: f32,
    /// The host error's mono size and its air above.
    pub error_size: f32,
    pub error_gap: f32,
}

impl Default for SessionsChrome {
    fn default() -> Self {
        Self {
            inset: 40.0,
            top_gap: 40.0,
            group_gap: 32.0,
            repo_width: 124.0,
            column_gap: 20.0,
            row_line: 40.0,
            row_pad: 10.0,
            meta_size: 24.0,
            title_color: Rgba(Color::new(0xffd9d6d0)),
            here_title: TypeRole::new(36.0, 620.0),
            here_line: 48.0,
            here_pad_top: 16.0,
            here_pad_bottom: 12.0,
            oblique_skew: -0.2,
            foot_gap: 44.0,
            foot_pad: 28.0,
            host_gap: 24.0,
            host_name: TypeRole::new(34.0, 520.0),
            host_status_size: 26.0,
            host_word_gap: 20.0,
            error_size: 23.0,
            error_gap: 8.0,
        }
    }
}

/// The Contents drawer: an outline set like a book's table of
/// contents — the KIND word in a right-aligned margin column, the
/// name, a dotted LEADER to the line number. The entry the caret is
/// in stands a size up in the live ink.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ContentsChrome {
    /// The column's text edge — the header's title and the rows'
    /// left and right inset.
    pub text_edge: f32,
    /// The air above the first entry.
    pub top_gap: f32,
    /// The kind margin's width (its word right-aligned in it) and
    /// the air between that word and the column's end.
    pub kind_column: f32,
    pub kind_pad: f32,
    pub kind_size: f32,
    /// The kind word's oblique slant — the chrome face has no italic
    /// cut, so the word leans by skew.
    pub kind_slant: f32,
    /// The gap between a row's words (kind | name | leader | line).
    pub gap: f32,
    /// The plain entry's name (mono) and the type's (mono, bold).
    pub name_size: f32,
    pub name_color: Rgba,
    pub type_size: f32,
    /// A nested member's name and its indent per level.
    pub member_color: Rgba,
    pub member_indent: f32,
    pub number_size: f32,
    pub number_min_width: f32,
    pub leader_color: Rgba,
    pub leader_min: f32,
    /// The leader's dot (and the gap between dots) and its rise
    /// above the baseline.
    pub leader_dot: f32,
    pub leader_rise: f32,
    /// A row's height (its line plus the air above and below), and
    /// the current entry's.
    pub row_height: f32,
    pub current_row_height: f32,
    pub current_name_size: f32,
    /// The air above a group head (after the first) and below it.
    pub group_gap: f32,
    pub group_after: f32,
}

impl Default for ContentsChrome {
    fn default() -> Self {
        Self {
            text_edge: 40.0,
            top_gap: 36.0,
            kind_column: 88.0,
            kind_pad: 16.0,
            kind_size: 22.0,
            kind_slant: 0.2,
            gap: 12.0,
            name_size: 25.0,
            name_color: Rgba(Color::new(0xffd9d6d0)),
            type_size: 28.0,
            member_color: Rgba(Color::new(0xffa9a6a0)),
            member_indent: 36.0,
            number_size: 24.0,
            number_min_width: 56.0,
            leader_color: Rgba(Color::new(0xff3a3a40)),
            leader_min: 32.0,
            leader_dot: 2.0,
            leader_rise: 8.0,
            row_height: 48.0,
            current_row_height: 60.0,
            current_name_size: 32.0,
            group_gap: 28.0,
            group_after: 12.0,
        }
    }
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
pub struct ComboChrome {
    pub pad: f32,

    pub gap: f32,
    pub label_size: f32,
    pub label_color: Rgba,
    pub value_size: f32,
    pub value_color: Rgba,

    pub chevron: f32,
    pub chevron_color: Rgba,
    /// The capsule behind a toolbar pill (the composer's combos).
    pub pill_fill: Rgba,
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
            pill_fill: Rgba(Color::from_argb(0x14, 0xff, 0xff, 0xff)),
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

    pub text_color: Rgba,
    pub title_size: f32,

    pub loader_color: Rgba,
    pub loader_height: f32,

    pub added_color: Rgba,
    pub removed_color: Rgba,

    pub input_height: f32,
    pub input_fill: Rgba,

    pub input_max_height: f32,

    pub accent: Rgba,
    pub on_accent: Rgba,

    pub stop_color: Rgba,

    // ── the typographic transcript (docs/Design.md: "size carries
    // rank"). The transcript is a two-column grid — the VOICE gutter
    // (`window.chat_text_edge` wide) and the text column.
    /// The gutter's air between its right-aligned label and the text
    /// column.
    pub gutter_pad: f32,
    /// The gutter label's line box — "YOU" sits centered in it, the
    /// row's top.
    pub gutter_line: f32,
    /// The text column's right inset.
    pub text_trailing: f32,
    /// The air between two transcript rows.
    pub row_gap: f32,

    /// A tool call's own row.
    pub tool_row_height: f32,
    /// The tool's name ("Bash"), bold in ink 2.
    pub tool_name: TypeRole,
    /// The invocation after the name, in the mono face.
    pub tool_detail_size: f32,
    /// The air between the name and the invocation.
    pub tool_gap: f32,
    /// The disclosure chevron: its glyph box, how far it hangs into
    /// the gutter, its stroke and ink.
    pub chevron_size: f32,
    pub chevron_hang: f32,
    pub chevron_stroke: f32,
    pub chevron_color: Rgba,
    /// A FAILED call's error line under its row: mono, its own line
    /// box, this far below the row.
    pub tool_error_size: f32,
    pub tool_error_line: f32,
    pub tool_error_gap: f32,

    /// "Turn cancelled." — the turn's quiet end.
    pub notice_size: f32,
    /// "8 in · 2,757 out" — what the turn spent, in its line box.
    pub usage_size: f32,
    pub usage_line: f32,

    /// The composer FOOTER: the air above the input (under the
    /// hairline) and below the options row.
    pub footer_top: f32,
    pub footer_bottom: f32,
    /// The input's lines at rest — the box stands this many prompt
    /// lines tall before it grows.
    pub input_lines: f32,
    /// The air between the text and the rule under it.
    pub input_rule_gap: f32,
    pub placeholder_color: Rgba,
    /// The options row under the input (model · effort · mode): the
    /// air above it, its line box and the gap between its words.
    pub options_top: f32,
    pub options_line: f32,
    pub options_gap: f32,
    /// SEND: the word, and the key hint a gap after it.
    pub send: TypeRole,
    pub send_key_size: f32,
    pub send_key_gap: f32,
}

impl Default for ChatChrome {
    fn default() -> Self {
        Self {
            pad: 16.0,
            gap: 12.0,
            radius: 10.0,
            min_cell_height: 40.0,
            text_color: Rgba(Color::from_argb(0xff, 0xd8, 0xde, 0xe9)),
            title_size: 22.0,
            loader_color: Rgba(Color::from_argb(0xa0, 0x92, 0x9e, 0xb6)),
            loader_height: 56.0,
            added_color: Rgba(Color::from_argb(0xff, 0x62, 0xc0, 0x73)),
            removed_color: Rgba(Color::from_argb(0xff, 0xd4, 0x6a, 0x6a)),
            input_height: 96.0,
            input_fill: Rgba(Color::from_argb(0xff, 0x1d, 0x23, 0x33)),
            input_max_height: 360.0,
            accent: Rgba(Color::from_argb(0xff, 0x2f, 0x9e, 0x5b)),
            on_accent: Rgba(Color::from_argb(0xff, 0xf2, 0xf5, 0xfc)),
            stop_color: Rgba(Color::from_argb(0xff, 0xc9, 0x45, 0x4a)),
            gutter_pad: 32.0,
            gutter_line: 48.0,
            text_trailing: 56.0,
            row_gap: 36.0,
            tool_row_height: 48.0,
            tool_name: TypeRole {
                size: 26.0,
                weight: 600.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.0,
            },
            tool_detail_size: 24.0,
            tool_gap: 16.0,
            chevron_size: 24.0,
            chevron_hang: 44.0,
            chevron_stroke: 2.5,
            chevron_color: Rgba(Color::from_argb(0xff, 0x6e, 0x6b, 0x66)),
            tool_error_size: 23.0,
            tool_error_line: 36.0,
            tool_error_gap: 4.0,
            notice_size: 28.0,
            usage_size: 24.0,
            usage_line: 48.0,
            footer_top: 36.0,
            footer_bottom: 28.0,
            input_lines: 3.0,
            input_rule_gap: 12.0,
            placeholder_color: Rgba(Color::from_argb(0xff, 0x6e, 0x6b, 0x66)),
            options_top: 20.0,
            options_line: 32.0,
            options_gap: 16.0,
            send: TypeRole {
                size: 32.0,
                weight: 680.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.0,
            },
            send_key_size: 22.0,
            send_key_gap: 8.0,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ToolbarChrome {
    pub height: f32,
    pub background: Rgba,

    pub rule: Rgba,

    /// The active button's backdrop.
    pub pressed_fill: Rgba,

    pub title_size: f32,
    pub title_color: Rgba,

    pub button_size: f32,
    pub button_gap: f32,
    pub button_inset: f32,
    pub button_radius: f32,
    pub glyph_color: Rgba,

    /// Every column header puts its title's baseline at this y from
    /// the band's top — one line across the window, whatever each
    /// header's largest word (docs/Design.md: the headers share ONE
    /// baseline).
    pub title_baseline: f32,
    /// The header's subtitle ("in himark-jb", the file's directory):
    /// the subtitle role, this far after the title.
    pub subtitle_gap: f32,
    /// The gap between the trailing text buttons of a header.
    pub button_text_gap: f32,
    /// The trailing inset the rightmost text button keeps off the
    /// column's edge.
    pub trailing_inset: f32,
    /// The tabs' gap in the side column's header (History · Files …).
    pub tab_gap: f32,
    /// A title standing AFTER tabs keeps this much off the last one —
    /// the tabs read as a group, the title as the column's name.
    pub tabs_title_gap: f32,
    /// A header text button's color — secondary ink; the display
    /// role's button (Sessions) draws in primary ink.
    pub button_text_color: Rgba,
    /// The dimmed tab / quiet button color.
    pub dim_text_color: Rgba,
    /// The LEFTMOST header's title inset — past the platform
    /// semaphore (the traffic lights), short of the column's text
    /// edge so the title leads the prose by half a gutter.
    pub lead_inset: f32,
}

impl Default for ToolbarChrome {
    fn default() -> Self {
        Self {
            height: 104.0,
            background: Rgba(Color::new(0xff0d0d0f)),
            rule: Rgba(Color::new(0xff222226)),
            pressed_fill: Rgba(Color::new(0xff17171b)),
            title_size: 34.0,
            title_color: Rgba(Color::new(0xffeceae4)),
            button_size: 44.0,
            button_gap: 10.0,
            button_inset: 18.0,
            button_radius: 9.0,
            glyph_color: Rgba(Color::new(0xff8a8781)),
            title_baseline: 66.0,
            subtitle_gap: 16.0,
            button_text_gap: 32.0,
            trailing_inset: 48.0,
            tab_gap: 24.0,
            tabs_title_gap: 48.0,
            button_text_color: Rgba(Color::new(0xffb5b2ab)),
            dim_text_color: Rgba(Color::new(0xff8a8781)),
            lead_inset: 144.0,
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
            // The islands selection well: NEUTRAL glass, not a blue
            // tint (docs/himark-islands-default.png).
            highlight: Rgba(Color::new(0x1affffff)),
            directory: Rgba(Color::from_argb(0xff, 0xde, 0xe5, 0xf2)),
            file: Rgba(Color::from_argb(0xff, 0xc9, 0xd2, 0xe4)),
        }
    }
}

/// The History tab under the side column's tabs (docs/Design.md, the
/// Main artboard's right column): a branch line, then the RAIL — a
/// hairline in the gutter every commit hangs a node on — and the
/// commits' titles at the column's text edge. Colors are the ink's
/// (rail = `rule_strong`, nodes ring in `ink_3`, the newest in
/// `ink_2`, the here-node fills `live`); only the open title's
/// register is its own.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct HistoryChrome {
    /// The rail hairline's left edge, from the column's left.
    pub rail_x: f32,
    pub rail_width: f32,

    /// A commit node: a ring this wide, centered on the rail.
    pub node_size: f32,
    pub node_stroke: f32,
    /// The node's top, from its commit row's top.
    pub node_top: f32,

    /// A commit row's air above and below its title block.
    pub commit_air: f32,
    /// A title line's box (the mock's 20px line).
    pub commit_line: f32,
    /// The collapsed commit: one line, ellipsized, in `ink_2`.
    pub commit: TypeRole,
    /// The open commit (the newest, or one whose files landed): the
    /// title wraps, a size up and strong.
    pub open_commit: TypeRole,
    pub open_title: Rgba,

    /// "4 files · +144 −0" under an open title.
    pub meta: TypeRole,
    pub meta_gap: f32,
    /// The open commit's air before the file tree hanging under it.
    pub tree_gap: f32,

    /// The branch line over the rail: the branch strong, then the
    /// folder and the order, brief.
    pub branch: TypeRole,
    pub folder: TypeRole,
    pub header_top: f32,
    pub header_bottom: f32,
    pub header_gap: f32,
}

impl Default for HistoryChrome {
    fn default() -> Self {
        Self {
            rail_x: 56.0,
            rail_width: 2.0,
            node_size: 18.0,
            node_stroke: 3.0,
            node_top: 24.0,
            commit_air: 14.0,
            commit_line: 40.0,
            commit: TypeRole::new(28.0, 400.0),
            open_commit: TypeRole::new(30.0, 560.0),
            open_title: Rgba(Color::from_argb(0xff, 0xd9, 0xd6, 0xd0)),
            meta: TypeRole::new(24.0, 400.0),
            meta_gap: 4.0,
            tree_gap: 16.0,
            branch: TypeRole::new(30.0, 640.0),
            folder: TypeRole::new(26.0, 400.0),
            header_top: 32.0,
            header_bottom: 20.0,
            header_gap: 16.0,
        }
    }
}

/// The Files tab under the side column's tabs (docs/Design.md, the
/// Files artboard): the folder's name, a caps FILTER label over a
/// then the tree. The text edge and the trailing inset are the
/// window's side-column metrics.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct FilesChrome {
    /// The air between the tabs' rule and the folder's name.
    pub head_top: f32,
    /// The folder's name, the head of the tab.
    pub folder: TypeRole,
    /// The air under the folder's name.
    pub head_bottom: f32,
    /// The hairline under a bare field (the inline rename).
    pub field_rule: Rgba,
    /// The air above the tree's first row.
    pub tree_top: f32,
    /// The current row's name: the row size in the title weight.
    pub current: TypeRole,
}

impl Default for FilesChrome {
    fn default() -> Self {
        Self {
            head_top: 32.0,
            folder: TypeRole::new(30.0, 640.0),
            head_bottom: 12.0,
            field_rule: Rgba(Color::new(0xff2a2a2f)),
            tree_top: 16.0,
            current: TypeRole::new(28.0, 640.0),
        }
    }
}

/// The Find tab under the side column's tabs (the Find artboard): a
/// caps "FIND IN …" label, the query over a hairline, the counts
/// line, then the results tree — files strong, hits in mono with the
/// match bold and underlined LIVE, the line number dim at the right.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct FindChrome {
    pub head_top: f32,
    /// The query's distance below its caps label.
    pub query_gap: f32,
    /// The query line's height — the QUERY-SMALL role's line.
    pub query_band: f32,
    /// The air between the query's line and its rule.
    pub query_pad_bottom: f32,
    pub query_rule: Rgba,
    /// The counts line's distance below the rule.
    pub options_top: f32,
    /// The counts line's words ("in", "files").
    pub options: TypeRole,
    /// The counts' own weight — the numbers lead the words.
    pub count_weight: f32,
    /// The air above the results; each group adds `group_gap`.
    pub results_top: f32,
    /// The air between top-level result groups.
    pub group_gap: f32,
    /// A file's name in the results.
    pub file: TypeRole,
    /// The hit line's mono size and ink.
    pub hit_size: f32,
    pub hit: Rgba,
    /// The match inside a hit line: this weight, the bright ink, a
    /// LIVE underline this thick, this far under the baseline.
    pub match_weight: f32,
    pub underline: f32,
    pub underline_offset: f32,
    /// The hit's line number, mono, at the row's right.
    pub line_size: f32,
}

impl Default for FindChrome {
    fn default() -> Self {
        Self {
            head_top: 32.0,
            query_gap: 4.0,
            query_band: 72.0,
            query_pad_bottom: 20.0,
            query_rule: Rgba(Color::new(0xff2e2e33)),
            options_top: 20.0,
            options: TypeRole::new(26.0, 400.0),
            count_weight: 600.0,
            results_top: 16.0,
            group_gap: 12.0,
            file: TypeRole::new(28.0, 600.0),
            hit_size: 24.0,
            hit: Rgba(Color::new(0xffa9a6a0)),
            match_weight: 700.0,
            underline: 3.0,
            underline_offset: 6.0,
            line_size: 22.0,
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

    /// The canvas PLAQUES (docs/himark-islands-canvas-many-files.png):
    /// each file's header + diff rides its own rounded card over the
    /// canvas page. The page fill behind and between the cards, the
    /// body fill, the header band, the inset from the pane edges, and
    /// the corner radius. Dark: cards lighter than the page; light:
    /// white cards on the gray page.
    pub plaque_page: Rgba,
    pub plaque: Rgba,
    pub plaque_header: Rgba,
    pub plaque_margin: f32,
    pub plaque_radius: f32,

    /// The COMMIT BLOCK heading a commit canvas (the typographic
    /// redesign, Main.dc.html): the byline, then the message in the
    /// HEADLINE rung, on the ground. Its left inset is the code text
    /// edge (`window.code_text_edge`); these are the other three, the
    /// gap between the byline's words, the air above the headline
    /// (and above a message body), and the headline's measure.
    pub commit_pad_top: f32,
    pub commit_pad_right: f32,
    pub commit_pad_bottom: f32,
    pub byline_gap: f32,
    pub headline_gap: f32,
    pub headline_max_width: f32,

    /// The FILE BAND: one hairline-topped row of words per file over
    /// the ground — no card. Its insets (left is the code text edge),
    /// the AIR under every file's diff before the next band's hairline
    /// (the files group by whitespace, Multi-breaks.dc.html — it rides
    /// the DIFF row, never the band: the band is sticky), the gap
    /// between its words, the tighter step from the directory into
    /// the name, the name's own rung, and the ordinal's size.
    pub band_pad_top: f32,
    pub band_pad_right: f32,
    pub band_pad_bottom: f32,
    pub band_air: f32,
    pub band_gap: f32,
    pub band_name_gap: f32,
    pub file_name: TypeRole,
    pub ordinal_size: f32,

    /// The canvas SUMMARY line (`+A −R · N files`, the toggle-all
    /// press): its inset from the pane's top-right and the gap
    /// between its words.
    pub summary_pad: f32,
    pub summary_gap: f32,

    /// The FOLD BAND's two EDGE rows (fold_height is their sum): each
    /// edge's height, the chevron glyph's side, its hit box and
    /// stroke, the gutter's right inset, the gap between an edge's
    /// line number and its chevron, the gap before "Show all", and
    /// the chevron's hover wash.
    pub fold_edge_height: f32,
    pub fold_chevron_size: f32,
    pub fold_chevron_hit: f32,
    pub fold_chevron_stroke: f32,
    pub fold_gutter_inset: f32,
    pub fold_number_gap: f32,
    pub fold_summary_gap: f32,
    pub fold_hover: Rgba,
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
            plaque_page: Rgba(Color::new(0xff0a0a10)),
            plaque: Rgba(Color::new(0xff17171b)),
            plaque_header: Rgba(Color::new(0xff1d1d22)),
            plaque_margin: 14.0,
            plaque_radius: 10.0,
            commit_pad_top: 52.0,
            commit_pad_right: 64.0,
            commit_pad_bottom: 44.0,
            byline_gap: 16.0,
            headline_gap: 16.0,
            headline_max_width: 1152.0,
            band_pad_top: 24.0,
            band_pad_right: 48.0,
            band_pad_bottom: 20.0,
            band_air: 96.0,
            band_gap: 20.0,
            band_name_gap: 8.0,
            file_name: TypeRole {
                size: 40.0,
                weight: 680.0,
                width: 100.0,
                tracking: -0.4,
                line_height: 1.0,
            },
            ordinal_size: 24.0,
            summary_pad: 20.0,
            summary_gap: 16.0,
            fold_edge_height: 44.0,
            fold_chevron_size: 28.0,
            fold_chevron_hit: 44.0,
            fold_chevron_stroke: 2.5,
            fold_gutter_inset: 24.0,
            fold_number_gap: 8.0,
            fold_summary_gap: 36.0,
            fold_hover: Rgba(Color::new(0xff1e1e23)),
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
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct StickyChrome {
    pub background: Rgba,
    pub rule: Rgba,
}

impl Default for StickyChrome {
    fn default() -> Self {
        Self {
            background: Rgba(Color::new(0xff0f0f12)),
            rule: Rgba(Color::new(0xff26262b)),
        }
    }
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

    /// The command palette's sheet (docs/Design.md: the one sheet the
    /// design keeps — a query, a count, ranked rows, a key footer).
    #[serde(default)]
    pub palette: PaletteSheetChrome,

    /// The jump-to-file sheet: the matched files at the left, the
    /// selected file's preview at the right.
    #[serde(default)]
    pub jump: JumpSheetChrome,
}

/// A centered modal SHEET's surface and placement: the only boxed
/// surface the typographic design allows — a flat fill, a hairline
/// border, a soft radius and a soft drop shadow; the page behind
/// dims nothing.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct SheetChrome {
    pub fill: Rgba,
    pub border: Rgba,
    pub radius: f32,
    /// The drop shadow's color, its downward offset and its blur
    /// (the CSS blur radius; a zero blur draws no shadow).
    pub shadow: Rgba,
    pub shadow_dy: f32,
    pub shadow_blur: f32,
    /// The sheet's widest width; narrower windows keep `gutter` on
    /// each side.
    pub width: f32,
    pub gutter: f32,
    /// The sheet's top edge from the window's top.
    pub top: f32,
    /// The room kept under a sheet that fills the window's height
    /// (or caps one that grows with its content).
    pub bottom: f32,
}

impl Default for SheetChrome {
    fn default() -> Self {
        Self {
            fill: Rgba(Color::new(0xff151518)),
            border: Rgba(Color::new(0xff2a2a2f)),
            radius: 20.0,
            shadow: Rgba(Color::new(0x8c000000)),
            shadow_dy: 60.0,
            shadow_blur: 160.0,
            width: 1520.0,
            gutter: 32.0,
            top: 144.0,
            bottom: 144.0,
        }
    }
}

/// The command palette (Palette.dc.html, CSS px doubled).
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct PaletteSheetChrome {
    pub sheet: SheetChrome,
    /// The hairlines between the query band, the rows and the footer.
    pub rule: Rgba,
    /// The query band's insets: top, sides, bottom.
    pub pad_top: f32,
    pub pad_x: f32,
    pub pad_bottom: f32,
    /// The caps label → query, and query → count line gaps.
    pub label_gap: f32,
    pub count_gap: f32,
    /// The rows block's own air above and below.
    pub list_pad_top: f32,
    pub list_pad_bottom: f32,
    /// The rows' cap before the list scrolls.
    pub list_max_height: f32,
    /// Every row's height (the selected row only grows its words).
    pub row_height: f32,
    /// The gap between a command's name and its shortcut.
    pub shortcut_gap: f32,
    /// A row's name, and the SELECTED row's larger, heavier name.
    pub row: TypeRole,
    pub selected_row: TypeRole,
    /// The selected row's flat wash.
    pub selected_fill: Rgba,
    /// The shortcut's mono size on a row and on the selected row.
    pub shortcut_size: f32,
    pub selected_shortcut_size: f32,
    /// The typed letters: their weight, and the LIVE underline's
    /// thickness and drop below the baseline.
    pub mark_weight: f32,
    pub mark_thickness: f32,
    pub mark_offset: f32,
    /// The footer's vertical insets, its gap between key hints and
    /// its text size (the keys ride the mono face at the same size).
    pub footer_pad_y: f32,
    pub footer_gap: f32,
    pub footer_size: f32,
}

impl Default for PaletteSheetChrome {
    fn default() -> Self {
        Self {
            sheet: SheetChrome::default(),
            rule: Rgba(Color::new(0xff26262a)),
            pad_top: 52.0,
            pad_x: 64.0,
            pad_bottom: 36.0,
            label_gap: 12.0,
            count_gap: 12.0,
            list_pad_top: 20.0,
            list_pad_bottom: 12.0,
            list_max_height: 880.0,
            row_height: 88.0,
            shortcut_gap: 32.0,
            row: TypeRole {
                size: 40.0,
                weight: 500.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.3,
            },
            selected_row: TypeRole {
                size: 52.0,
                weight: 600.0,
                width: 100.0,
                tracking: -0.78,
                line_height: 1.23,
            },
            selected_fill: Rgba(Color::new(0xff1f1f24)),
            shortcut_size: 24.0,
            selected_shortcut_size: 26.0,
            mark_weight: 700.0,
            mark_thickness: 4.0,
            mark_offset: 8.0,
            footer_pad_y: 24.0,
            footer_gap: 44.0,
            footer_size: 24.0,
        }
    }
}

/// The jump-to-file sheet (Jump.dc.html, CSS px doubled).
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct JumpSheetChrome {
    pub sheet: SheetChrome,
    /// The matches column's width; the preview takes the rest.
    pub list_width: f32,
    /// The query band's insets: top, sides, bottom.
    pub pad_top: f32,
    pub pad_x: f32,
    pub pad_bottom: f32,
    /// The caps label → query, and query → count line gaps.
    pub label_gap: f32,
    pub count_gap: f32,
    /// The rows block's air above and below.
    pub list_pad_y: f32,
    /// A row's height, and the SELECTED row's taller one.
    pub row_height: f32,
    pub selected_row_height: f32,
    /// The least gap between a file's name and its directory.
    pub dir_gap: f32,
    /// A row's name, and the selected row's larger, heavier name.
    pub name: TypeRole,
    pub name_color: Rgba,
    pub selected_name: TypeRole,
    /// The selected row's flat wash.
    pub selected_fill: Rgba,
    /// The typed letters: weight, LIVE underline thickness and drop.
    pub mark_weight: f32,
    pub mark_thickness: f32,
    pub mark_offset: f32,
    /// The footer's vertical insets, key-hint gap and text size.
    pub footer_pad_y: f32,
    pub footer_gap: f32,
    pub footer_size: f32,
    /// The preview's header: top, right, bottom and left insets (the
    /// left one lines the name up with the code past the gutter).
    pub preview_pad_top: f32,
    pub preview_pad_right: f32,
    pub preview_pad_bottom: f32,
    pub preview_pad_left: f32,
    /// The previewed file's name, the gap between the header's
    /// words, the full path's mono size and the line-count size.
    pub preview_title: TypeRole,
    pub preview_gap: f32,
    pub path_size: f32,
    pub meta_size: f32,
    /// The air between the header's hairline and the first line.
    pub preview_top_gap: f32,
}

impl Default for JumpSheetChrome {
    fn default() -> Self {
        Self {
            sheet: SheetChrome {
                fill: Rgba(Color::new(0xff121215)),
                border: Rgba(Color::new(0xff26262b)),
                radius: 20.0,
                shadow: Rgba(Color::new(0x99000000)),
                shadow_dy: 60.0,
                shadow_blur: 160.0,
                width: 2560.0,
                gutter: 48.0,
                top: 80.0,
                bottom: 80.0,
            },
            list_width: 880.0,
            pad_top: 48.0,
            pad_x: 56.0,
            pad_bottom: 28.0,
            label_gap: 8.0,
            count_gap: 8.0,
            list_pad_y: 16.0,
            row_height: 68.0,
            selected_row_height: 88.0,
            dir_gap: 32.0,
            name: TypeRole {
                size: 30.0,
                weight: 400.0,
                width: 100.0,
                tracking: 0.0,
                line_height: 1.0,
            },
            name_color: Rgba(Color::new(0xffd9d6d0)),
            selected_name: TypeRole {
                size: 38.0,
                weight: 620.0,
                width: 100.0,
                tracking: -0.38,
                line_height: 1.0,
            },
            selected_fill: Rgba(Color::new(0xff1c1c21)),
            mark_weight: 700.0,
            mark_thickness: 4.0,
            mark_offset: 8.0,
            footer_pad_y: 24.0,
            footer_gap: 36.0,
            footer_size: 24.0,
            preview_pad_top: 48.0,
            preview_pad_right: 64.0,
            preview_pad_bottom: 28.0,
            preview_pad_left: 160.0,
            preview_title: TypeRole {
                size: 40.0,
                weight: 680.0,
                width: 100.0,
                tracking: -0.4,
                line_height: 1.0,
            },
            preview_gap: 24.0,
            path_size: 25.0,
            meta_size: 24.0,
            preview_top_gap: 16.0,
        }
    }
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

#[cfg(test)]
mod tests;
