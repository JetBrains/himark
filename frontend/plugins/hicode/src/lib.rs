// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use std::sync::Arc;

use himark::{
    line_col_at, offset_at, AppCommand, AppFx, Application, Document, DynamicCommand, GroupSpans,
    InstallGroup, LineCol, LocationList, LocationListCommand, OpenDocuments, ResourceLocation,
};
use imba::{effect::Effect, list::ListCommand, scroll::ScrollView, store::Store};
use text::TextView;

const CONTEXT_LINES: usize = 2;

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

pub struct FindReferencesEffect {
    pub folders: Vec<ResourceLocation>,
    pub location: ResourceLocation,
    pub position: LineCol,
}

impl Effect for FindReferencesEffect {
    type Result = Option<Vec<CodeTarget>>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NavigationKind {
    Definition,
    References,
}

pub struct CodeNavigationEffect {
    pub kind: NavigationKind,
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
        let targets = match effect.kind {
            NavigationKind::Definition => {
                self.caller
                    .call(FindDefinitionEffect {
                        folders: effect.folders.clone(),
                        location: effect.location.clone(),
                        position: effect.position,
                    })
                    .await
            }
            NavigationKind::References => {
                self.caller
                    .call(FindReferencesEffect {
                        folders: effect.folders.clone(),
                        location: effect.location.clone(),
                        position: effect.position,
                    })
                    .await
            }
        }
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
                        prep: None,
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
        match targets.as_slice() {
            [] => {}
            [target] => navigate(store, window, target, &self.outcome.built, fx),
            targets => open_references(
                store,
                &_app.ui_ctx(),
                window,
                &self.outcome.title,
                targets,
                &self.outcome.built,
                fx,
            ),
        }
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

fn open_references(
    store: &mut Store,
    ui: &imba::UiCtx,
    window: himark::WindowId,
    title: &str,
    targets: &[CodeTarget],
    built: &[(ResourceLocation, Document)],
    fx: &mut AppFx<'_>,
) {
    let mut grouped: Vec<(ResourceLocation, Vec<Range<LineCol>>)> = Vec::new();
    let mut group_at: std::collections::HashMap<&ResourceLocation, usize> =
        std::collections::HashMap::new();
    for target in targets {
        match group_at.get(&target.location) {
            Some(at) => grouped[*at].1.push(target.range.clone()),
            None => {
                group_at.insert(&target.location, grouped.len());
                grouped.push((target.location.clone(), vec![target.range.clone()]));
            }
        }
    }
    let built_at: std::collections::HashMap<&ResourceLocation, &Document> = built
        .iter()
        .map(|(location, document)| (location, document))
        .collect();

    let mut groups: Vec<InstallGroup> = Vec::new();
    let mut group_documents: Vec<himark::DocumentId> = Vec::new();
    for (location, ranges) in grouped {
        if let Some(open_id) = OpenDocuments::by_location(store, &location) {
            if let Some(document) = OpenDocuments::document_ref(store, open_id) {
                let spans = preview_spans(&mut document.text().view(), &ranges);
                groups.push(InstallGroup::open(open_id, spans));
                group_documents.push(open_id);
            }
            continue;
        }
        let Some(document) = built_at.get(&location).copied() else {
            continue;
        };
        let spans = preview_spans(&mut document.text().view(), &ranges);
        let revision = document.revision();
        let id = OpenDocuments::register(
            store,
            document.clone(),
            Some(location.clone()),
            location.name().to_owned(),
            revision,
        );
        groups.push(InstallGroup::open(id, spans));
        group_documents.push(id);
    }
    if groups.is_empty() {
        return;
    }

    himark::sync_document_watches(store, fx);

    let mut list = LocationList::new();
    let fonts = himark::env::Fonts::of(store)();
    fx.scope(identity_routed(window, group_documents), |fx| {
        list.install(store, ui, &fonts, groups, None, fx)
    });

    let id = himark::ListId::mint();
    himark::LocationLists::put(
        store,
        id,
        himark::ListEntry {
            title: title.to_owned(),
            list: ScrollView::new(list),
        },
    );
    let panel = himark::ListPanel::new(id);
    let Some(mut window_entity) = himark::Windows::window(store, window) else {
        return;
    };
    window_entity.open_panel(store, Box::new(panel), fx);
    himark::Windows::put(store, window, window_entity);
}

fn identity_routed(
    window: himark::WindowId,
    group_documents: Vec<himark::DocumentId>,
) -> impl Fn(LocationListCommand) -> AppCommand + Send + Clone + 'static {
    move |command| match command {
        LocationListCommand::Reshape {
            document, command, ..
        } => AppCommand::Entity(document, command),
        LocationListCommand::Results(ListCommand::Child(
            group,
            himark::GroupCommand::Rows(ListCommand::Child(_, command)),
        )) => match group_documents.get(group).copied() {
            Some(document) => AppCommand::Entity(document, command),
            None => AppCommand::Dynamic(window, Arc::new(NothingLanding)),
        },

        _ => AppCommand::Dynamic(window, Arc::new(NothingLanding)),
    }
}

struct NothingLanding;

impl DynamicCommand for NothingLanding {
    fn id(&self) -> &'static str {
        "code.nothing"
    }
    fn name(&self) -> String {
        String::new()
    }
    fn perform(&self, _: &mut Application, _: &mut Store, _: himark::WindowId, _: &mut AppFx<'_>) {}
}

fn preview_spans(view: &mut TextView, targets: &[Range<LineCol>]) -> GroupSpans {
    let mut marks: Vec<Range<u32>> = targets
        .iter()
        .map(|range| offset_at(view, range.start) as u32..offset_at(view, range.end) as u32)
        .collect();
    marks.sort_by_key(|range| range.start);
    let last_line = view.line_count().0 - 1;
    let mut ranges: Vec<Range<u32>> = Vec::new();
    for mark in &marks {
        let first = view
            .line_at(mark.start as usize)
            .0
            .saturating_sub(CONTEXT_LINES);
        let last = (view.line_at(mark.end as usize).0 + CONTEXT_LINES).min(last_line);
        let start = view.line_start_offset(text::LineNumber(first)) as u32;
        let end = view.line_end_offset(text::LineNumber(last)) as u32;
        match ranges.last_mut() {
            Some(previous) if start <= previous.end => previous.end = previous.end.max(end),
            _ => ranges.push(start..end),
        }
    }
    GroupSpans { ranges, marks }
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
            NavigationKind::Definition,
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
    title: String,
    outcome: Result<himark::LocationsChannel, String>,
}

/// References and implementations stream into the Search dock tab
/// (docs/ui/location-list.md §7): ask `lsp/locations`, land the
/// channel, displace whatever the tab holds. No target document is
/// fetched here — the stream carries its own display context.
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
        let _ = fx.push(
            imba::effect::AnyEffect::new(himark::LspLocationsEffect {
                location: location.clone(),
                position,
                kind,
            })
            .map(move |outcome| himark::EditorCommand::Dynamic {
                id,
                payload: Some(Box::new(StreamOutcome { title, outcome })),
            }),
        );
        return;
    };
    let Ok(landed) = payload.downcast::<StreamOutcome>() else {
        return;
    };
    himark::AppRequests::push(
        store,
        Arc::new(ApplyLocationsStream {
            title: landed.title,
            outcome: landed.outcome,
        }),
    );
}

/// Land the reference stream into the Search dock tab, activating
/// it — the ToggleSearchView recipe with an attached channel.
struct ApplyLocationsStream {
    title: String,
    outcome: Result<himark::LocationsChannel, String>,
}

impl himark::DynamicCommand for ApplyLocationsStream {
    fn id(&self) -> &'static str {
        "code.apply-locations"
    }

    fn name(&self) -> String {
        "Show Found Locations".to_owned()
    }

    fn perform(
        &self,
        app: &mut himark::Application,
        store: &mut Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        let Some(mut entity) = himark::Windows::window(store, window) else {
            return;
        };
        fx.scope(
            move |command| himark::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );

        let session = entity.current_session();
        let ui = app.ui_ctx();
        let mut panel = himark::hisearch::SearchView::open(store, &ui, window, session);
        match self.outcome.clone() {
            Ok(channel) => fx.scope(himark::dock_scope(window), |fx| {
                fx.scope(
                    |command: himark::hisearch::SearchCommand| {
                        Box::new(command) as imba::DynCommand
                    },
                    |fx| panel.attach_stream(store, &ui, self.title.clone(), channel, fx),
                )
            }),
            Err(_) => panel.attach_failed(store, &ui, self.title.clone()),
        }
        fx.scope(
            move |command| himark::AppCommand::Content(window, command),
            |fx| entity.show_dock(store, Box::new(panel), himark::hisearch::OWNER, fx),
        );
        himark::Windows::put(store, window, entity);
    }
}

#[allow(clippy::too_many_arguments)]
fn navigation(
    kind: NavigationKind,
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
            (NavigationKind::References, false) => format!("References to `{ident}`"),
            (NavigationKind::References, true) => "References".to_owned(),
            (NavigationKind::Definition, false) => format!("Definitions of `{ident}`"),
            (NavigationKind::Definition, true) => "Definitions".to_owned(),
        };
        let open = OpenDocuments::list(store)
            .into_iter()
            .filter_map(|(_, entity)| entity.location().cloned())
            .collect();
        let _ = fx.push(
            imba::effect::AnyEffect::new(CodeNavigationEffect {
                kind,
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
