// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use std::sync::Arc;

use documents::text_ext::line_col_at;
use documents::text_ext::LineCol;
use documents::OpenDocuments;
use editor::document::Document;
use editor::location::ResourceLocation;
use himark::app::AppFx;
use himark::commands::WindowedCommand;
use imba::{effect::Effect, store::Store};
use text::text_view::TextView;

const MAX_FETCHED_TARGETS: usize = 50;

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

pub struct CodeNavigationEffect {
    pub folders: Vec<ResourceLocation>,
    pub location: ResourceLocation,
    pub position: LineCol,

    pub title: String,

    pub open: Vec<ResourceLocation>,
}

pub struct NavigationOutcome {
    pub title: String,
    pub targets: Option<Vec<CodeTarget>>,
    pub built: Vec<(ResourceLocation, Document)>,
}

impl std::fmt::Display for CodeNavigationEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "navigate code /{}", self.location.path().join("/"))
    }
}

impl Effect for CodeNavigationEffect {
    type Result = NavigationOutcome;
}

pub struct CodeNavigationHandler {
    pub caller: imba::effect::EffectCaller,
}

impl imba::effect::EffectHandler<CodeNavigationEffect> for CodeNavigationHandler {
    async fn handle(&self, effect: CodeNavigationEffect) -> NavigationOutcome {
        let targets = self
            .caller
            .call(FindDefinitionEffect {
                folders: effect.folders.clone(),
                location: effect.location.clone(),
                position: effect.position,
            })
            .await
            .flatten();

        let mut built = Vec::new();
        if let Some(targets) = &targets {
            let mut wanted: Vec<ResourceLocation> = Vec::new();
            for target in targets {
                if effect.open.contains(&target.location) || wanted.contains(&target.location) {
                    continue;
                }
                wanted.push(target.location.clone());
            }
            for location in wanted.into_iter().take(MAX_FETCHED_TARGETS) {
                let Some(text) = self
                    .caller
                    .call(documents::FetchDocumentEffect {
                        location: location.clone(),
                    })
                    .await
                    .flatten()
                else {
                    continue;
                };
                if let Some(built_document) = self
                    .caller
                    .call(documents::BuildDocumentEffect {
                        location: location.clone(),
                        text,
                    })
                    .await
                {
                    built.push((location, built_document.document));
                }
            }
        }

        NavigationOutcome {
            title: effect.title,
            targets,
            built,
        }
    }
}

struct ApplyNavigation {
    outcome: NavigationOutcome,
}

impl WindowedCommand for ApplyNavigation {
    fn id(&self) -> &'static str {
        "code.apply-navigation"
    }
    fn name(&self) -> String {
        "Apply Code Navigation".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let ui = ui;
        let Some(targets) = &self.outcome.targets else {
            return;
        };
        // A definition ask answering several targets is rare; the
        // first wins. Fanning results out belongs to the peek/dock
        // surfaces (docs/ui/location-list.md), not a fetched panel.
        let Some(target) = targets.first() else {
            return;
        };
        navigate(store, ui, window, target, &self.outcome.built, fx);
    }
}

fn navigate(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    window: workbench::window::WindowId,
    target: &CodeTarget,
    built: &[(ResourceLocation, Document)],
    fx: &mut AppFx<'_>,
) {
    let Some(documents) =
        himark::workspace::session_state(store, window).map(|state| state.documents())
    else {
        return;
    };
    let document_id = match OpenDocuments::by_location(store, documents, &target.location) {
        Some(document) => document,
        None => {
            let Some((_, document)) = built
                .iter()
                .find(|(location, _)| *location == target.location)
            else {
                return;
            };
            OpenDocuments::register(
                store,
                documents,
                document.clone(),
                Some(target.location.clone()),
                target.location.name().to_owned(),
                document.revision(),
            )
        }
    };
    let Some(mut window_entity) = workbench::window::Windows::window(store, window) else {
        return;
    };
    fx.scope(himark::app::AppCommand::Verb, |fx| {
        window_entity.show_document(
            store,
            ui,
            window,
            document_id,
            Some(target.range.clone()),
            false,
            fx,
        )
    });
    workbench::window::Windows::put(store, window, window_entity);

    fx.scope(himark::app::AppCommand::Verb, |fx| {
        documents::lanes::sync_document_watches(store, documents, fx)
    });
    fx.scope(himark::app::AppCommand::Verb, |fx| {
        documents::lanes::sync_stripe_bases(store, documents, ui, fx)
    });
}

pub struct GoDefinition;

/// The navigation commands close over the pane's ids (docs/entities.md
/// law 3): the open set they hand the ask is the collection they run in.
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
        documents: imba::store::Id<OpenDocuments>,
        _document_id: documents::DocumentId,
        document: &mut Document,
        editor: editor::editor::EditorId,
        location: &ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, editor::editor_view::EditorCommand>,
    ) {
        navigation(
            self.id(),
            store,
            documents,
            document,
            editor,
            location,
            payload,
            fx,
        );
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
        Arc::new(himark::hisearch::OpenLspFeed {
            kind,
            title,
            location: location.clone(),
            position,
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn navigation(
    id: &'static str,
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    document: &mut Document,
    editor: editor::editor::EditorId,
    location: &ResourceLocation,
    payload: Option<Box<dyn std::any::Any + Send + Sync>>,
    fx: &mut imba::effect::Effects<'_, editor::editor_view::EditorCommand>,
) {
    let Some(payload) = payload else {
        let caret = document.caret_byte(editor) as usize;
        let mut view = document.text().view();
        let position = line_col_at(&mut view, caret);
        let ident = identifier_at(&mut view, caret);
        let title = match ident.is_empty() {
            false => format!("Definitions of `{ident}`"),
            true => "Definitions".to_owned(),
        };
        let open = OpenDocuments::list(store, documents)
            .into_iter()
            .filter_map(|(_, entity)| entity.location().cloned())
            .collect();
        let _ = fx.push(
            imba::effect::AnyEffect::new(CodeNavigationEffect {
                folders: workspace_folders(store),
                location: location.clone(),
                position,
                title,
                open,
            })
            .map(move |outcome| editor::editor_view::EditorCommand::Dynamic {
                id,
                payload: Some(editor::dynamic::DynPayload::new(outcome)),
            }),
        );
        return;
    };
    let Ok(outcome) = payload.downcast::<NavigationOutcome>() else {
        return;
    };

    himark::commands::AppRequests::push(store, Arc::new(ApplyNavigation { outcome: *outcome }));
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
