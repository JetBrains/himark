// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use std::sync::Arc;

use himark::{
    line_col_at, AppFx, Application, Document, DynamicCommand, LineCol, OpenDocuments,
    ResourceLocation,
};
use imba::{effect::Effect, store::Store};
use text::TextView;

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

impl Effect for CodeNavigationEffect {
    type Result = NavigationOutcome;
}

pub struct CodeNavigationHandler {
    pub caller: imba::effect::EffectCaller,
}

impl himark::EffectHandler<CodeNavigationEffect> for CodeNavigationHandler {
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
                    .call(himark::FetchDocumentEffect {
                        location: location.clone(),
                    })
                    .await
                    .flatten()
                else {
                    continue;
                };
                if let Some(built_document) = self
                    .caller
                    .call(himark::BuildDocumentEffect {
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

impl DynamicCommand for ApplyNavigation {
    fn id(&self) -> &'static str {
        "code.apply-navigation"
    }
    fn name(&self) -> String {
        "Apply Code Navigation".to_owned()
    }
    fn perform(
        &self,
        _app: &mut Application,
        store: &mut Store,
        window: himark::WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let Some(targets) = &self.outcome.targets else {
            return;
        };
        // A definition ask answering several targets is rare; the
        // first wins. Fanning results out belongs to the peek/dock
        // surfaces (docs/ui/location-list.md), not a fetched panel.
        let Some(target) = targets.first() else {
            return;
        };
        navigate(store, window, target, &self.outcome.built, fx);
    }
}

fn navigate(
    store: &mut Store,
    window: himark::WindowId,
    target: &CodeTarget,
    built: &[(ResourceLocation, Document)],
    fx: &mut AppFx<'_>,
) {
    let document_id = match OpenDocuments::by_location(store, &target.location) {
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
                document.clone(),
                Some(target.location.clone()),
                target.location.name().to_owned(),
                document.revision(),
            )
        }
    };
    let Some(mut window_entity) = himark::Windows::window(store, window) else {
        return;
    };
    window_entity.show_document(store, window, document_id, Some(target.range.clone()), fx);
    himark::Windows::put(store, window, window_entity);

    himark::sync_document_watches(store, fx);
}

pub struct GoDefinition;

impl himark::DynamicEditorCommand for GoDefinition {
    fn id(&self) -> &'static str {
        "code.definition"
    }
    fn name(&self) -> String {
        "Go to Definition".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        document: &mut Document,
        editor: himark::EditorId,
        location: &ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, himark::EditorCommand>,
    ) {
        navigation(
            self.id(),
            store,
            document,
            editor,
            location,
            payload,
            fx,
        );
    }
}

pub struct GoReferences;

impl himark::DynamicEditorCommand for GoReferences {
    fn id(&self) -> &'static str {
        "code.references"
    }
    fn name(&self) -> String {
        "Find References".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        document: &mut Document,
        editor: himark::EditorId,
        location: &ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, himark::EditorCommand>,
    ) {
        stream_navigation(
            himark::LspLocationsKind::References,
            self.id(),
            store,
            document,
            editor,
            location,
            payload,
            fx,
        );
    }
}

pub struct GoImplementations;

impl himark::DynamicEditorCommand for GoImplementations {
    fn id(&self) -> &'static str {
        "code.implementations"
    }
    fn name(&self) -> String {
        "Find Implementations".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        document: &mut Document,
        editor: himark::EditorId,
        location: &ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, himark::EditorCommand>,
    ) {
        stream_navigation(
            himark::LspLocationsKind::Implementations,
            self.id(),
            store,
            document,
            editor,
            location,
            payload,
            fx,
        );
    }
}

/// The channel outcome riding the two-phase editor-command re-entry.
struct StreamOutcome {
    feed: himark::locations::FeedId,
    outcome: Result<himark::LocationsChannel, String>,
}

/// References and implementations stream into the Search dock tab
/// (docs/ui/location-list.md §7) through a store-level FEED: phase
/// one mints the feed and fronts it in the dock IMMEDIATELY — the
/// tab shows "searching…" before the ask answers, so a failing ask
/// resolves in plain sight; phase two attaches the landed channel.
/// No target document is fetched before navigation.
#[allow(clippy::too_many_arguments)]
fn stream_navigation(
    kind: himark::LspLocationsKind,
    id: &'static str,
    store: &mut Store,
    document: &mut Document,
    editor: himark::EditorId,
    location: &ResourceLocation,
    payload: Option<Box<dyn std::any::Any + Send + Sync>>,
    fx: &mut imba::effect::Effects<'_, himark::EditorCommand>,
) {
    let Some(payload) = payload else {
        let caret = document.caret_byte(editor) as usize;
        let mut view = document.text().view();
        let position = line_col_at(&mut view, caret);
        let ident = identifier_at(&mut view, caret);
        let title = match (kind, ident.is_empty()) {
            (himark::LspLocationsKind::References, false) => format!("References to `{ident}`"),
            (himark::LspLocationsKind::References, true) => "References".to_owned(),
            (himark::LspLocationsKind::Implementations, false) => {
                format!("Implementations of `{ident}`")
            }
            (himark::LspLocationsKind::Implementations, true) => "Implementations".to_owned(),
        };
        let feed = himark::locations::FeedId::mint();
        himark::locations::open_feed(store, feed, title, String::new());
        himark::AppRequests::push(store, Arc::new(himark::hisearch::ShowFeedInDock { feed }));
        let _ = fx.push(
            imba::effect::AnyEffect::new(himark::LspLocationsEffect {
                location: location.clone(),
                position,
                kind,
            })
            .map(move |outcome| himark::EditorCommand::Dynamic {
                id,
                payload: Some(Box::new(StreamOutcome { feed, outcome })),
            }),
        );
        return;
    };
    let Ok(landed) = payload.downcast::<StreamOutcome>() else {
        return;
    };
    himark::AppRequests::push(
        store,
        Arc::new(himark::locations::AttachFeedStream {
            feed: landed.feed,
            outcome: landed.outcome,
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn navigation(
    id: &'static str,
    store: &mut Store,
    document: &mut Document,
    editor: himark::EditorId,
    location: &ResourceLocation,
    payload: Option<Box<dyn std::any::Any + Send + Sync>>,
    fx: &mut imba::effect::Effects<'_, himark::EditorCommand>,
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
        let open = OpenDocuments::list(store)
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
            .map(move |outcome| himark::EditorCommand::Dynamic {
                id,
                payload: Some(Box::new(outcome)),
            }),
        );
        return;
    };
    let Ok(outcome) = payload.downcast::<NavigationOutcome>() else {
        return;
    };

    himark::AppRequests::push(store, Arc::new(ApplyNavigation { outcome: *outcome }));
}

fn workspace_folders(store: &Store) -> Vec<ResourceLocation> {
    himark::higent::all_session_folders(store)
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
