// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use std::sync::Arc;

use documents::text_ext::line_col_at;
use documents::text_ext::LineCol;
use documents::OpenDocuments;
use editor::document::Document;
use editor::location::ResourceLocation;
use imba::{effect::Effect, store::Store};
use text::text_view::TextView;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeTarget {
    pub location: ResourceLocation,
    pub range: Range<LineCol>,
}

pub struct FindDefinitionEffect {
    pub folders: Vec<ResourceLocation>,
    pub location: ResourceLocation,
    pub position: LineCol,
}

impl std::fmt::Display for FindDefinitionEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "find definition /{}", self.location.path().join("/"))
    }
}

impl Effect for FindDefinitionEffect {
    type Result = Option<Vec<CodeTarget>>;
}

pub mod lsproute;

/// Code navigation as a plugin: the reference-link resolver and the
/// go-to document commands. The wire route for the definition ask stays
/// the host's (it closes over its client directory).
pub struct CodePlugin;

impl himark::plugin::Plugin for CodePlugin {
    fn install(&self, app: &mut himark::app::Himark, _cx: &himark::plugin::PluginContext) {
        app.register_link_resolver(std::sync::Arc::new(DefinitionResolver));
        app.register_document_command(std::sync::Arc::new(GoDefinition));
        app.register_document_command(std::sync::Arc::new(GoReferences));
        app.register_document_command(std::sync::Arc::new(GoImplementations));
        // The definition ask's WIRE route is the host's — it closes
        // over the client directory, which only the composition
        // root holds (`himark_plugins::install_host` registers it).
    }
}

/// What a reference points at: its definition, asked of the
/// language server. A definition ask answering several targets is
/// rare; the first wins.
pub struct DefinitionResolver;

fn definition_ask(
    store: &Store,
    document: &Document,
    byte: u32,
    location: &ResourceLocation,
) -> editor::link::LinkResolution {
    let position = line_col_at(&mut document.text().view(), byte as usize);
    editor::link::LinkResolution::Pending(
        imba::effect::AnyEffect::new(FindDefinitionEffect {
            folders: workspace_folders(store),
            location: location.clone(),
            position,
        })
        .map(|targets| {
            let target = targets?.into_iter().next()?;
            Some(editor::link::LinkTarget::Location {
                location: target.location,
                range: Some(target.range),
            })
        }),
    )
}

impl editor::link::LinkResolver for DefinitionResolver {
    fn claims(
        &self,
        _document: &Document,
        link: &editor::link::Link,
        location: Option<&ResourceLocation>,
    ) -> bool {
        link.kind == editor::link::LinkKind::Reference
            && location.is_some_and(|location| !location.is_synthetic())
    }

    fn resolve(
        &self,
        store: &Store,
        document: &Document,
        link: &editor::link::Link,
        location: Option<&ResourceLocation>,
    ) -> Option<editor::link::LinkResolution> {
        Some(definition_ask(store, document, link.range.start, location?))
    }
}

pub struct GoDefinition;

/// The palette's road to the same ask a cmd-click on a reference
/// makes — at the caret, whatever stands there.
impl documents::dynamic::DocumentCommand for GoDefinition {
    fn id(&self) -> &'static str {
        "code.definition"
    }
    fn name(&self) -> String {
        "Go to Definition".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _documents: imba::store::Id<OpenDocuments>,
        _document_id: documents::DocumentId,
        document: &mut Document,
        editor: editor::editor::EditorId,
        location: &ResourceLocation,
        _payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, editor::editor_view::EditorCommand>,
    ) {
        let ask = definition_ask(store, document, document.caret_byte(editor), location);
        editor::link::follow(store, ask, fx);
    }
}

pub struct GoReferences;

impl documents::dynamic::DocumentCommand for GoReferences {
    fn id(&self) -> &'static str {
        "code.references"
    }
    fn name(&self) -> String {
        "Find References".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _documents: imba::store::Id<OpenDocuments>,
        _document_id: documents::DocumentId,
        document: &mut Document,
        editor: editor::editor::EditorId,
        location: &ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, editor::editor_view::EditorCommand>,
    ) {
        let _ = (payload, fx);
        stream_navigation(
            ahp_locations::LspLocationsKind::References,
            store,
            document,
            editor,
            location,
        );
    }
}

pub struct GoImplementations;

impl documents::dynamic::DocumentCommand for GoImplementations {
    fn id(&self) -> &'static str {
        "code.implementations"
    }
    fn name(&self) -> String {
        "Find Implementations".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _documents: imba::store::Id<OpenDocuments>,
        _document_id: documents::DocumentId,
        document: &mut Document,
        editor: editor::editor::EditorId,
        location: &ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, editor::editor_view::EditorCommand>,
    ) {
        let _ = (payload, fx);
        stream_navigation(
            ahp_locations::LspLocationsKind::Implementations,
            store,
            document,
            editor,
            location,
        );
    }
}

/// References and implementations stream into the Search dock tab
/// (docs/ui/location-list.md §7) through the session's lists
/// collection: this border only names WHAT to ask — the window
/// command (`hisearch::OpenLspFeed`) owns the whole chain, minting
/// the feed against its session and stamping the landing with it
/// (docs/entities.md law 3). No payload re-entry, no second phase.
fn stream_navigation(
    kind: ahp_locations::LspLocationsKind,
    store: &mut Store,
    document: &mut Document,
    editor: editor::editor::EditorId,
    location: &ResourceLocation,
) {
    let caret = document.caret_byte(editor) as usize;
    let mut view = document.text().view();
    let position = line_col_at(&mut view, caret);
    let ident = identifier_at(&mut view, caret);
    let title = match (kind, ident.is_empty()) {
        (ahp_locations::LspLocationsKind::References, false) => format!("References to `{ident}`"),
        (ahp_locations::LspLocationsKind::References, true) => "References".to_owned(),
        (ahp_locations::LspLocationsKind::Implementations, false) => {
            format!("Implementations of `{ident}`")
        }
        (ahp_locations::LspLocationsKind::Implementations, true) => "Implementations".to_owned(),
    };
    himark::commands::AppRequests::push(
        store,
        Arc::new(workbench_shell::hisearch::OpenLspFeed {
            kind,
            title,
            location: location.clone(),
            position,
        }),
    );
}

fn workspace_folders(store: &Store) -> Vec<ResourceLocation> {
    ahp_session::session::folders::all_session_folders(store)
}

fn identifier_at(view: &mut TextView, caret: usize) -> String {
    let len = view.byte_count();
    if len == 0 {
        return String::new();
    }
    let mut lo = caret.saturating_sub(64);
    let mut hi = (caret + 64).min(len);
    while lo > 0 && !view.is_char_boundary(lo) {
        lo -= 1;
    }
    while hi < len && !view.is_char_boundary(hi) {
        hi += 1;
    }
    let window = view.byte_string(lo, hi);
    let at = caret - lo;
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let start = window[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(index, _)| index);
    let end = window[at..]
        .char_indices()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(index, c)| at + index + c.len_utf8());
    window[start..end].to_owned()
}

#[cfg(test)]
mod tests;
