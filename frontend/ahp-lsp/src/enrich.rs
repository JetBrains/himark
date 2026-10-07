// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The PULL half of language services (docs/ahp/ahp-lsp.md §2):
//! semantic tokens and inlay hints are requested per document, so
//! they ride the `lsp/` pass-through, not a channel. Two
//! document-scoped markup layers on the squiggles pattern: one wire
//! per session owns the asks, the open hook and the edit sink mark
//! documents dirty, the batch-tail lane relaunches both asks (a new
//! trigger supersedes the in-flight run), and a landing replaces its
//! layer wholesale against the CURRENT text through `replace_markup`.

use std::sync::Arc;

use ahp_wire::client::{LspClient, ResourceUri, SessionUri};
use documents::text_ext::LineCol;
use documents::{DocumentId, OpenDocuments};
use editor::location::ResourceLocation;
use editor::markup::{Inlay, InlayMode, Markup, MarkupId};
use editor::theme::StyleId;
use imba::command::{Fx, Verb};
use imba::effect::{AnyEffect, CancellationToken, EffectHandler};
use imba::store::Store;

/// The semantic-token layer: a severity-free restyle of identifiers
/// over the tree-sitter colors — document-scoped, every editor.
pub fn semantic_tokens_markup() -> MarkupId {
    static ID: std::sync::OnceLock<MarkupId> = std::sync::OnceLock::new();
    *ID.get_or_init(MarkupId::mint)
}

/// The inlay-hint layer: parameter names and inferred types as dim
/// chips beside the text — document-scoped, every editor.
pub fn inlay_hints_markup() -> MarkupId {
    static ID: std::sync::OnceLock<MarkupId> = std::sync::OnceLock::new();
    *ID.get_or_init(MarkupId::mint)
}

fn trace(line: impl FnOnce() -> String) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("HIMARK_TRACE_LSP").is_some()) {
        eprintln!("[lsp] {}", line());
    }
}

// ─── The asks ───────────────────────────────────────────────────────

/// One decoded semantic token, text-independent: the landing converts
/// it against the current text (utf-8 line/character, ahp-lsp.md §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticSpan {
    pub start: LineCol,
    pub len: u32,
    pub style: StyleId,
}

/// One inlay hint, text-independent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HintItem {
    pub position: LineCol,
    pub label: String,
    pub padding_left: bool,
    pub padding_right: bool,
}

/// `textDocument/semanticTokens/full` for one resource, decoded
/// against the server's legend (fetched through `lsp/capabilities`
/// once per language server and cached by the handler).
pub struct LspSemanticTokensEffect {
    pub client: Arc<dyn LspClient>,
    pub session: SessionUri,
    pub uri: ResourceUri,
}

impl std::fmt::Display for LspSemanticTokensEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "lsp semantic tokens {}", self.uri.as_str())
    }
}

impl imba::effect::Effect for LspSemanticTokensEffect {
    type Result = Option<Vec<SemanticSpan>>;
}

/// `textDocument/inlayHint` over the whole resource.
pub struct LspInlayHintsEffect {
    pub client: Arc<dyn LspClient>,
    pub session: SessionUri,
    pub uri: ResourceUri,
    /// The end of the text at ask time — the hint range's end.
    pub end: LineCol,
}

impl std::fmt::Display for LspInlayHintsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "lsp inlay hints {}", self.uri.as_str())
    }
}

impl imba::effect::Effect for LspInlayHintsEffect {
    type Result = Option<Vec<HintItem>>;
}

/// The semantic-token legend of one language server: token type and
/// modifier names by index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Legend {
    pub types: Vec<String>,
    pub modifiers: Vec<String>,
}

impl Legend {
    pub fn of_capabilities(capabilities: &serde_json::Value) -> Option<Legend> {
        let legend = &capabilities["semanticTokensProvider"]["legend"];
        let names = |value: &serde_json::Value| -> Vec<String> {
            value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let types = names(&legend["tokenTypes"]);
        (!types.is_empty()).then(|| Legend {
            types,
            modifiers: names(&legend["tokenModifiers"]),
        })
    }
}

/// The handler: one legend per (session, language server), keyed by
/// the resource's extension — the host routes by extension too.
#[derive(Default)]
pub struct HandleSemanticTokens {
    legends: std::sync::Mutex<std::collections::HashMap<String, Arc<Legend>>>,
}

impl HandleSemanticTokens {
    async fn legend(&self, effect: &LspSemanticTokensEffect) -> Option<Arc<Legend>> {
        let extension = effect
            .uri
            .as_str()
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_owned();
        let key = format!("{}#{extension}", effect.session.as_str());
        if let Some(legend) = self.legends.lock().expect("legends").get(&key) {
            return Some(Arc::clone(legend));
        }
        let capabilities = effect
            .client
            .lsp_capabilities(effect.session.clone(), effect.uri.clone())
            .await
            .ok()??;
        let legend = Arc::new(Legend::of_capabilities(&capabilities)?);
        self.legends
            .lock()
            .expect("legends")
            .insert(key, Arc::clone(&legend));
        Some(legend)
    }
}

impl EffectHandler<LspSemanticTokensEffect> for HandleSemanticTokens {
    async fn handle(&self, effect: LspSemanticTokensEffect) -> Option<Vec<SemanticSpan>> {
        let legend = self.legend(&effect).await?;
        let params = serde_json::json!({ "textDocument": { "uri": effect.uri.as_str() } });
        let result = effect
            .client
            .lsp(
                effect.session.clone(),
                "textDocument/semanticTokens/full".to_owned(),
                params,
            )
            .await
            .ok()?;
        Some(decode_semantic_tokens(&result, &legend))
    }
}

pub struct HandleInlayHints;

impl EffectHandler<LspInlayHintsEffect> for HandleInlayHints {
    async fn handle(&self, effect: LspInlayHintsEffect) -> Option<Vec<HintItem>> {
        let params = serde_json::json!({
            "textDocument": { "uri": effect.uri.as_str() },
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": effect.end.line, "character": effect.end.col },
            },
        });
        let result = effect
            .client
            .lsp(
                effect.session.clone(),
                "textDocument/inlayHint".to_owned(),
                params,
            )
            .await
            .ok()?;
        Some(parse_inlay_hints(&result))
    }
}

/// LSP `SemanticTokens.data`: 5-tuples of (deltaLine, deltaStart,
/// length, tokenType, tokenModifiers), positions relative to the
/// previous token (same line when deltaLine is 0).
pub fn decode_semantic_tokens(result: &serde_json::Value, legend: &Legend) -> Vec<SemanticSpan> {
    let Some(data) = result["data"].as_array() else {
        return Vec::new();
    };
    let numbers: Vec<u32> = data
        .iter()
        .map(|value| value.as_u64().unwrap_or(0) as u32)
        .collect();
    let (mut line, mut col) = (0u32, 0u32);
    let mut spans = Vec::new();
    for token in numbers.chunks_exact(5) {
        let (delta_line, delta_start, len, kind, modifiers) =
            (token[0], token[1], token[2], token[3], token[4]);
        if delta_line > 0 {
            line += delta_line;
            col = delta_start;
        } else {
            col += delta_start;
        }
        let Some(kind) = legend.types.get(kind as usize) else {
            continue;
        };
        let modifier_names = legend
            .modifiers
            .iter()
            .enumerate()
            .filter(|(index, _)| modifiers & (1u32 << index) != 0)
            .map(|(_, name)| name.as_str());
        if let Some(style) = style_of(kind, modifier_names) {
            spans.push(SemanticSpan {
                start: LineCol { line, col },
                len,
                style,
            });
        }
    }
    spans
}

/// The token type (and modifiers) a server names, onto the theme's
/// palette. Unmapped types keep the tree-sitter color: the layer
/// REFINES, it does not repaint everything.
pub fn style_of<'a>(kind: &str, mut modifiers: impl Iterator<Item = &'a str>) -> Option<StyleId> {
    Some(match kind {
        "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" | "typeAlias"
        | "union" | "builtinType" | "derive" | "selfTypeKeyword" => StyleId::Type,
        "function" | "method" | "macro" | "procMacro" => StyleId::Function,
        "enumMember" | "const" | "static" | "boolean" => StyleId::Constant,
        "variable" | "parameter" | "property" | "field" => {
            match modifiers.any(|modifier| matches!(modifier, "static" | "constant")) {
                true => StyleId::Constant,
                false => return None,
            }
        }
        "keyword" | "selfKeyword" | "modifier" => StyleId::Keyword,
        "comment" => StyleId::Comment,
        "string" | "char" | "escapeSequence" | "formatSpecifier" => StyleId::String,
        "number" => StyleId::Number,
        "operator" | "arithmetic" | "bitwise" | "comparison" | "logical" => StyleId::Operator,
        "decorator" | "attribute" | "attributeBracket" => StyleId::Attribute,
        _ => return None,
    })
}

/// LSP `InlayHint[]`: a position, a label (a string or label parts),
/// optional padding flags.
pub fn parse_inlay_hints(result: &serde_json::Value) -> Vec<HintItem> {
    const CAP: usize = 4096;
    let Some(hints) = result.as_array() else {
        return Vec::new();
    };
    hints
        .iter()
        .take(CAP)
        .filter_map(|hint| {
            let position = LineCol {
                line: hint["position"]["line"].as_u64()? as u32,
                col: hint["position"]["character"].as_u64()? as u32,
            };
            let label = match &hint["label"] {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Array(parts) => parts
                    .iter()
                    .filter_map(|part| part["value"].as_str())
                    .collect::<String>(),
                _ => return None,
            };
            if label.is_empty() {
                return None;
            }
            Some(HintItem {
                position,
                label,
                padding_left: hint["paddingLeft"].as_bool().unwrap_or(false),
                padding_right: hint["paddingRight"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

// ─── The wire ───────────────────────────────────────────────────────

/// The in-flight asks of one document: a relaunch supersedes.
#[derive(Clone, Default)]
struct Asks {
    tokens: Option<CancellationToken>,
    hints: Option<CancellationToken>,
}

#[derive(Clone)]
pub struct EnrichWire {
    documents: imba::store::Id<OpenDocuments>,
    uris: Option<Arc<dyn ahp_wire::client::ResourceUriMap>>,

    asks: rpds::HashTrieMapSync<DocumentId, Asks>,

    /// Documents whose text changed (or that just opened) since the
    /// last lane run — drained by the batch tail.
    dirty: rpds::HashTrieSetSync<DocumentId>,
}

impl EnrichWire {
    pub fn wired(
        documents: imba::store::Id<OpenDocuments>,
        uris: Option<Arc<dyn ahp_wire::client::ResourceUriMap>>,
    ) -> Self {
        Self {
            documents,
            uris,
            asks: rpds::HashTrieMapSync::new_sync(),
            dirty: rpds::HashTrieSetSync::new_sync(),
        }
    }

    pub fn stamp_uris(
        store: &mut Store,
        wire: imba::store::Id<EnrichWire>,
        uris: &Arc<dyn ahp_wire::client::ResourceUriMap>,
    ) {
        update(store, wire, |row| row.uris = Some(Arc::clone(uris)));
    }

    pub fn is_empty(&self) -> bool {
        self.asks.is_empty() && self.dirty.is_empty()
    }
}

fn of(store: &Store, wire: imba::store::Id<EnrichWire>) -> Option<&EnrichWire> {
    store.entity(wire)
}

fn update(
    store: &mut Store,
    wire: imba::store::Id<EnrichWire>,
    mutate: impl FnOnce(&mut EnrichWire),
) {
    let Some(mut row) = store.entity::<EnrichWire>(wire).cloned() else {
        return;
    };
    mutate(&mut row);
    store.put_entity(wire, row);
}

/// The document-open half: a served document asks on its first lane
/// run; a closing one drops its asks (landings guard on the open
/// document, so an in-flight answer lands nowhere).
pub struct EnrichHook {
    pub wire: imba::store::Id<EnrichWire>,
}

impl documents::DocumentHook for EnrichHook {
    fn opened(
        &self,
        store: &mut Store,
        _documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
        location: Option<&ResourceLocation>,
    ) {
        if location.is_some_and(ahp_wire::fs::served) {
            update(store, self.wire, |row| {
                row.dirty.insert_mut(document);
            });
        }
    }

    fn closing(
        &self,
        store: &mut Store,
        _documents: imba::store::Id<OpenDocuments>,
        document: DocumentId,
        _location: Option<&ResourceLocation>,
        _doc: &editor::document::Document,
    ) {
        update(store, self.wire, |row| {
            row.asks.remove_mut(&document);
            row.dirty.remove_mut(&document);
        });
    }
}

/// The edit half: the editor's change door runs with a read-only
/// store, so an edited location is NOTED into this shared set and
/// claimed by its session's lane at the batch tail — the docsync
/// sink's channel pattern.
#[derive(Clone, Default)]
pub struct EditNotes(Arc<std::sync::Mutex<std::collections::HashSet<ResourceLocation>>>);

impl EditNotes {
    fn note(store: &Store, location: &ResourceLocation) {
        if let Some(notes) = store.get::<EditNotes>() {
            notes.0.lock().expect("edit notes").insert(location.clone());
        }
    }

    /// Hand the lane the locations open in ITS collection; the rest
    /// stay for the other sessions' lanes this same batch tail.
    fn claim(store: &Store, documents: imba::store::Id<OpenDocuments>) -> Vec<DocumentId> {
        let Some(notes) = store.get::<EditNotes>() else {
            return Vec::new();
        };
        let mut claimed = Vec::new();
        notes.0.lock().expect("edit notes").retain(|location| {
            match OpenDocuments::by_location(store, documents, location) {
                Some(document) => {
                    claimed.push(document);
                    false
                }
                None => true,
            }
        });
        claimed
    }
}

/// After every session's lane ran: a note no collection claimed names
/// a document that is open nowhere — drop it.
pub fn drop_unclaimed(store: &Store) {
    if let Some(notes) = store.get::<EditNotes>() {
        notes.0.lock().expect("edit notes").clear();
    }
}

pub struct EditSink;

impl editor::change_sink::ChangeSink for EditSink {
    fn changed(
        &self,
        store: &Store,
        _document: &editor::document::Document,
        location: &ResourceLocation,
        _base_revision: u64,
        _text_before: &text::text::Text,
        _fx: &mut editor::editor::EditorEffects<'_>,
    ) {
        if ahp_wire::fs::served(location) {
            EditNotes::note(store, location);
        }
    }
}

/// Boot: the shared note set and the change sink that fills it.
pub fn install_edit_sink(store: &mut Store) {
    store.put(EditNotes::default());
    editor::change_sink::InstalledChangeSink::install(store, Arc::new(EditSink));
}

/// The batch-tail lane: claim the edit notes, then relaunch both asks
/// for every dirty document. A clean wire costs a lock and a map
/// read.
pub fn sync(store: &mut Store, wire: imba::store::Id<EnrichWire>, fx: &mut Fx<'_>) {
    let Some(row) = of(store, wire) else {
        return;
    };
    let documents = row.documents;
    let edited = EditNotes::claim(store, documents);
    if !edited.is_empty() {
        update(store, wire, |row| {
            for document in &edited {
                row.dirty.insert_mut(*document);
            }
        });
    }
    let Some(row) = of(store, wire) else {
        return;
    };
    if row.dirty.is_empty() {
        return;
    }
    let dirty: Vec<DocumentId> = row.dirty.iter().copied().collect();
    let Some(uris) = row.uris.clone() else {
        return;
    };
    update(store, wire, |row| {
        row.dirty = rpds::HashTrieSetSync::new_sync();
    });
    for document in dirty {
        let Some(location) = OpenDocuments::location(store, documents, document) else {
            continue;
        };
        let Some((_, client, session)) =
            ahp_wire::client::route_client(store, location.authority().as_str())
        else {
            continue;
        };
        let Some(entry) = OpenDocuments::document_ref(store, documents, document) else {
            continue;
        };
        let end = {
            let mut view = entry.text().view();
            let len = view.byte_count();
            documents::text_ext::line_col_at(&mut view, len)
        };
        let uri = uris.uri_of(&location);
        trace(|| format!("enrich: asking tokens + hints for {}", uri.as_str()));
        let mut asks = of(store, wire)
            .and_then(|row| row.asks.get(&document).cloned())
            .unwrap_or_default();
        let tokens = AnyEffect::new(LspSemanticTokensEffect {
            client: client.lsp.clone(),
            session: session.clone(),
            uri: uri.clone(),
        })
        .map(move |result| {
            Verb::Dynamic(Arc::new(TokensLanded {
                wire,
                document,
                result,
            }))
        });
        fx.relaunch_erased(&mut asks.tokens, tokens);
        let hints = AnyEffect::new(LspInlayHintsEffect {
            client: client.lsp.clone(),
            session,
            uri,
            end,
        })
        .map(move |result| {
            Verb::Dynamic(Arc::new(HintsLanded {
                wire,
                document,
                result,
            }))
        });
        fx.relaunch_erased(&mut asks.hints, hints);
        update(store, wire, |row| {
            row.asks.insert_mut(document, asks);
        });
    }
}

struct TokensLanded {
    wire: imba::store::Id<EnrichWire>,
    document: DocumentId,
    result: Option<Vec<SemanticSpan>>,
}

impl imba::command::DynamicCommand for TokensLanded {
    fn id(&self) -> &'static str {
        "lsp.semantic-tokens.landed"
    }
    fn name(&self) -> String {
        "Semantic Tokens".to_owned()
    }
    fn perform(&self, store: &mut Store, ui: &imba::ui::UiCtx, fx: &mut Fx<'_>) {
        update(store, self.wire, |row| {
            if let Some(asks) = row.asks.get(&self.document).cloned() {
                row.asks.insert_mut(
                    self.document,
                    Asks {
                        tokens: None,
                        ..asks
                    },
                );
            }
        });
        let Some(spans) = &self.result else {
            trace(|| format!("enrich: no semantic tokens for {:?}", self.document));
            return;
        };
        trace(|| {
            format!(
                "enrich: {} semantic spans for {:?}",
                spans.len(),
                self.document
            )
        });
        apply(store, ui, self.wire, self.document, fx, |document| {
            semantic_markup(document, spans)
        });
    }
}

struct HintsLanded {
    wire: imba::store::Id<EnrichWire>,
    document: DocumentId,
    result: Option<Vec<HintItem>>,
}

impl imba::command::DynamicCommand for HintsLanded {
    fn id(&self) -> &'static str {
        "lsp.inlay-hints.landed"
    }
    fn name(&self) -> String {
        "Inlay Hints".to_owned()
    }
    fn perform(&self, store: &mut Store, ui: &imba::ui::UiCtx, fx: &mut Fx<'_>) {
        update(store, self.wire, |row| {
            if let Some(asks) = row.asks.get(&self.document).cloned() {
                row.asks.insert_mut(
                    self.document,
                    Asks {
                        hints: None,
                        ..asks
                    },
                );
            }
        });
        let Some(hints) = &self.result else {
            trace(|| format!("enrich: no inlay hints for {:?}", self.document));
            return;
        };
        trace(|| {
            format!(
                "enrich: {} inlay hints for {:?}",
                hints.len(),
                self.document
            )
        });
        apply(store, ui, self.wire, self.document, fx, |document| {
            hints_markup(document, hints)
        });
    }
}

/// Land one layer into its open document: the replacement is built
/// against the CURRENT text, then swapped wholesale with the
/// producer-brought change set.
fn apply(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    wire: imba::store::Id<EnrichWire>,
    document_id: DocumentId,
    fx: &mut Fx<'_>,
    build: impl FnOnce(&editor::document::Document) -> (MarkupId, Markup),
) {
    let Some(documents) = of(store, wire).map(|row| row.documents) else {
        return;
    };
    let Some(mut document) = OpenDocuments::document(store, documents, document_id) else {
        return;
    };
    let (markup, replacement) = build(&document);
    document.ensure_document_markup(markup);
    let changed = editor::markup::set_diff(document.feature_markup(markup), &replacement);
    let fonts = editor::env::ui_collection(store, ui);
    let theme = editor::env::Themes::of(store);
    fx.scope(
        move |command| {
            Verb::at(
                documents,
                documents::DocumentsCommand::Editor(document_id, command),
            )
        },
        |fx| {
            document.replace_markup(markup, replacement, &changed, store, ui, &fonts, &theme, fx);
        },
    );
    OpenDocuments::put_document(store, documents, document_id, document);
}

/// The semantic layer against the current text: one styled span per
/// token, clamped; a token off the end (an older text) drops.
fn semantic_markup(
    document: &editor::document::Document,
    spans: &[SemanticSpan],
) -> (MarkupId, Markup) {
    let mut builder = Markup::builder();
    let mut view = document.text().view();
    let len = view.byte_count();
    for span in spans {
        let start = documents::text_ext::offset_at(&mut view, span.start).min(len);
        let end = documents::text_ext::offset_at(
            &mut view,
            LineCol {
                line: span.start.line,
                col: span.start.col + span.len,
            },
        )
        .min(len);
        if start < end {
            builder.push_styled(start as u32..end as u32, span.style);
        }
    }
    (semantic_tokens_markup(), builder.finish())
}

/// The hint layer against the current text: a chip anchored LEFT of
/// the character at the hint's position (RIGHT of the last one when
/// the position is the end of the text).
fn hints_markup(document: &editor::document::Document, hints: &[HintItem]) -> (MarkupId, Markup) {
    let mut builder = Markup::builder();
    let mut view = document.text().view();
    let len = view.byte_count();
    for hint in hints {
        let position = documents::text_ext::offset_at(&mut view, hint.position).min(len);
        let mut text = String::new();
        if hint.padding_left {
            text.push(' ');
        }
        text.push_str(&hint.label);
        if hint.padding_right {
            text.push(' ');
        }
        let (mode, range) = if position < len {
            let mut next = position + 1;
            while next < len && !view.is_char_boundary(next) {
                next += 1;
            }
            (InlayMode::Left, position..next)
        } else if len > 0 {
            let mut previous = len - 1;
            while previous > 0 && !view.is_char_boundary(previous) {
                previous -= 1;
            }
            (InlayMode::Right, previous..len)
        } else {
            continue;
        };
        builder.push_inlay(
            range.start as u32..range.end as u32,
            Inlay::new(mode, HintView { text }),
        );
    }
    (inlay_hints_markup(), builder.finish())
}

/// The hint chip's size against the code it sits beside: the theme's
/// sizes are device pixels (the base text is 32), so the chip is a
/// RATIO of the source-code size, never a size of its own.
const HINT_SCALE: f32 = 0.8;

/// The code typeface every hint chip shares — cached in the ui
/// context (the theme's code families do not change between looks).
struct HintTypeface(Option<skia_safe::Typeface>);

/// The hint chip: a line of dim text in the code font, sized off the
/// source-code look, colored by the theme's `inlay_hint` entry.
/// Passive — it takes no commands.
#[derive(Clone)]
struct HintView {
    text: String,
}

impl imba::View for HintView {
    type Command = std::convert::Infallible;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {}
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a imba::ui::UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        let theme = editor::env::Themes::of(store);
        let look = theme.resolve([StyleId::SourceCode, StyleId::InlayHint]);
        let families: Vec<String> = look
            .font_families
            .as_deref()
            .map(<[String]>::to_vec)
            .unwrap_or_default();
        let size = look.font_size.or(theme.base().font_size).unwrap_or(32.0) * HINT_SCALE;
        // The code face, resolved ONCE per ui context: a font-collection
        // search per chip per measure was the retheme stall (every
        // inlay re-measures; hundreds of chips, a cold worker ctx).
        let typeface = ui
            .env(|| {
                HintTypeface(
                    editor::env::ui_typeface(ui, &families, skia_safe::FontStyle::normal())
                        .or_else(|| {
                            editor::env::ui_typeface(
                                ui,
                                &[] as &[&str],
                                skia_safe::FontStyle::normal(),
                            )
                        }),
                )
            })
            .0
            .clone();
        let mut style = hikit::ui::caption(store, ui).sized(size);
        if let Some(typeface) = typeface {
            style.font = skia_safe::Font::from_typeface(typeface, size);
            style.font.set_edging(skia_safe::font::Edging::AntiAlias);
        }
        if let Some(color) = look.color {
            style = style.colored(color);
        }
        hikit::ui::text(&style, self.text.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legend() -> Legend {
        Legend {
            types: vec![
                "function".to_owned(),
                "variable".to_owned(),
                "type".to_owned(),
                "namespace".to_owned(),
            ],
            modifiers: vec!["declaration".to_owned(), "static".to_owned()],
        }
    }

    #[test]
    fn semantic_tokens_decode_relative_positions_and_map_styles() {
        // "main" at 0:3 (function), "x" at 1:4 (variable, static),
        // "u32" at 1:7 (type), "std" at 3:0 (namespace: unmapped).
        let result = serde_json::json!({ "data": [
            0, 3, 4, 0, 0,
            1, 4, 1, 1, 0b10,
            0, 3, 3, 2, 0,
            2, 0, 3, 3, 0,
        ]});
        let spans = decode_semantic_tokens(&result, &legend());
        assert_eq!(
            spans,
            vec![
                SemanticSpan {
                    start: LineCol { line: 0, col: 3 },
                    len: 4,
                    style: StyleId::Function
                },
                SemanticSpan {
                    start: LineCol { line: 1, col: 4 },
                    len: 1,
                    style: StyleId::Constant
                },
                SemanticSpan {
                    start: LineCol { line: 1, col: 7 },
                    len: 3,
                    style: StyleId::Type
                },
            ]
        );
        assert!(decode_semantic_tokens(&serde_json::json!(null), &legend()).is_empty());
    }

    #[test]
    fn a_plain_variable_keeps_its_parse_color() {
        assert_eq!(style_of("variable", std::iter::empty()), None);
        assert_eq!(
            style_of("variable", ["static"].into_iter()),
            Some(StyleId::Constant)
        );
        assert_eq!(
            style_of("enumMember", std::iter::empty()),
            Some(StyleId::Constant)
        );
        assert_eq!(style_of("lifetime", std::iter::empty()), None);
    }

    #[test]
    fn the_legend_reads_from_capabilities() {
        let capabilities = serde_json::json!({
            "semanticTokensProvider": { "legend": {
                "tokenTypes": ["function", "type"],
                "tokenModifiers": ["static"]
            }}
        });
        assert_eq!(
            Legend::of_capabilities(&capabilities),
            Some(Legend {
                types: vec!["function".to_owned(), "type".to_owned()],
                modifiers: vec!["static".to_owned()],
            })
        );
        assert_eq!(Legend::of_capabilities(&serde_json::json!({})), None);
    }

    #[test]
    fn inlay_hints_parse_strings_and_parts() {
        let result = serde_json::json!([
            { "position": { "line": 0, "character": 9 }, "label": ": u32", "paddingLeft": true },
            { "position": { "line": 2, "character": 4 },
              "label": [{ "value": "name" }, { "value": ":" }], "paddingRight": true },
            { "position": { "line": 3, "character": 0 }, "label": "" },
        ]);
        let hints = parse_inlay_hints(&result);
        assert_eq!(hints.len(), 2, "an empty label drops");
        assert_eq!(hints[0].label, ": u32");
        assert!(hints[0].padding_left && !hints[0].padding_right);
        assert_eq!(hints[1].label, "name:");
        assert_eq!(hints[1].position, LineCol { line: 2, col: 4 });
    }

    #[test]
    fn the_layers_build_against_the_current_text() {
        let document = editor::document::Document::new(
            text::text::Text::from_string_exact("fn main() {}\nlet x = 1;\n"),
            Markup::new(),
        );
        let (id, markup) = semantic_markup(
            &document,
            &[
                SemanticSpan {
                    start: LineCol { line: 0, col: 3 },
                    len: 4,
                    style: StyleId::Function,
                },
                // Off the end: an answer computed against older text.
                SemanticSpan {
                    start: LineCol { line: 7, col: 0 },
                    len: 2,
                    style: StyleId::Type,
                },
            ],
        );
        assert_eq!(id, semantic_tokens_markup());
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        markup.marks_inline_hidden_in(0..u32::MAX, &mut inline, &mut hidden);
        let spans: Vec<_> = inline.iter().map(|i| (i.range.clone(), i.id)).collect();
        assert_eq!(spans, vec![(3..7, StyleId::Function)]);

        let (id, markup) = hints_markup(
            &document,
            &[
                HintItem {
                    position: LineCol { line: 1, col: 5 },
                    label: ": i32".to_owned(),
                    padding_left: false,
                    padding_right: true,
                },
                HintItem {
                    position: LineCol { line: 9, col: 0 },
                    label: "end".to_owned(),
                    padding_left: false,
                    padding_right: false,
                },
            ],
        );
        assert_eq!(id, inlay_hints_markup());
        assert!(
            !markup.is_empty(),
            "both hints anchor: one left of `=`, the end one right of the last char"
        );
    }
}
