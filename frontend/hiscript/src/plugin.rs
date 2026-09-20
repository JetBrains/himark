// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::effect::{AnyEffect, Effect};
use imba::store::Store;

use himark::{DocumentId, OpenDocuments, ResourceLocation};

pub struct ScriptSnapshot {
    pub location: ResourceLocation,
    pub id: DocumentId,
    pub revision: u64,
    pub text: himark::Text,
}

pub struct ScriptCapture {
    pub name: String,

    pub base: ResourceLocation,
    pub source: String,
    pub snapshots: Vec<ScriptSnapshot>,

    pub agent: Option<ScriptAgent>,

    pub changes: Option<String>,
}

pub struct ScriptAgent {
    pub seat: Arc<dyn himark::higent::AhpServer>,

    pub session: String,
}

async fn drive_turn(agent: &ScriptAgent, prompt: String) -> Result<String, String> {
    use himark::higent::ahp_types::actions::StateAction;
    use himark::higent::ahp_types::state::ResponsePart;
    let chat = agent
        .seat
        .create_chat(agent.session.clone())
        .await
        .map_err(|error| format!("createChat: {error}"))?;
    agent
        .seat
        .subscribe_chat(chat.clone())
        .await
        .map_err(|error| format!("subscribeChat: {error}"))?;
    agent
        .seat
        .start_turn(chat.clone(), prompt, None, None)
        .await
        .map_err(|error| format!("startTurn: {error}"))?;
    let mut parts: Vec<(String, String)> = Vec::new();
    loop {
        for action in agent.seat.poll_chat(chat.clone()).await {
            match action {
                StateAction::ChatResponsePart(part) => {
                    if let ResponsePart::Markdown(markdown) = part.part {
                        parts.push((markdown.id, markdown.content));
                    }
                }
                StateAction::ChatDelta(delta) => {
                    match parts.iter_mut().find(|(id, _)| *id == delta.part_id) {
                        Some((_, text)) => text.push_str(&delta.content),
                        None => parts.push((delta.part_id, delta.content)),
                    }
                }
                StateAction::ChatTurnComplete(_) => {
                    return Ok(parts
                        .into_iter()
                        .map(|(_, text)| text)
                        .collect::<Vec<String>>()
                        .join("\n\n"));
                }
                StateAction::ChatError(failed) => {
                    return Err(format!("the turn failed: {}", failed.error.message));
                }
                StateAction::ChatTurnCancelled(_) => {
                    return Err("the turn was cancelled".to_owned());
                }
                _ => {}
            }
        }
    }
}

pub struct RunScriptEffect {
    pub capture: ScriptCapture,
}

impl Effect for RunScriptEffect {
    type Result = ScriptLanding;
}

pub enum ScriptEdit {
    Open {
        id: DocumentId,
        base_revision: u64,
        operation: operation::Operation,
    },

    Store {
        location: ResourceLocation,
        text: String,
    },
}

pub struct ScriptLanding {
    pub name: String,
    pub log: Vec<String>,
    pub error: Option<String>,
    pub edits: Vec<ScriptEdit>,

    pub shows: Vec<ResourceLocation>,
}

pub struct ScriptStored {
    pub stored: bool,
    pub show: Option<ResourceLocation>,
}

pub struct ShowDocuments {
    pub locations: Vec<ResourceLocation>,
}

impl himark::DynamicCommand for ShowDocuments {
    fn id(&self) -> &'static str {
        "script.show"
    }

    fn name(&self) -> String {
        "Show Script Output".to_owned()
    }

    fn perform(
        &self,
        app: &mut himark::Application,
        store: &mut imba::store::Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        himark::open_locations(store, ui, window, &self.locations, fx);
    }
}

#[derive(Clone, Default)]
pub struct ScriptRuns(pub Vec<ScriptRecord>);

#[derive(Clone, Debug)]
pub struct ScriptRecord {
    pub name: String,
    pub error: Option<String>,
    pub log: Vec<String>,
}

impl ScriptRuns {
    pub fn record(store: &mut Store, record: ScriptRecord) {
        let mut runs = store.get::<ScriptRuns>().cloned().unwrap_or_default();
        runs.0.push(record);
        if runs.0.len() > 32 {
            runs.0.remove(0);
        }
        store.put(runs);
    }

    pub fn of(store: &Store) -> Vec<ScriptRecord> {
        store
            .get::<ScriptRuns>()
            .map(|runs| runs.0.clone())
            .unwrap_or_default()
    }
}

#[derive(Clone, Default)]
pub struct ScriptLanes(pub std::collections::HashMap<String, imba::effect::CancellationToken>);

pub fn resolve(base: &ResourceLocation, path: &str) -> Option<ResourceLocation> {
    let mut segments: Vec<String> = base
        .path()
        .get(..base.path().len().saturating_sub(1))
        .unwrap_or_default()
        .to_vec();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            name => segments.push(name.to_owned()),
        }
    }
    (!segments.is_empty()).then(|| {
        ResourceLocation::new(
            himark::ResourceType::document(),
            base.authority().clone(),
            segments,
        )
    })
}

pub struct RunScriptHandler {
    pub caller: imba::effect::EffectCaller,
}

impl imba::effect::EffectHandler<RunScriptEffect> for RunScriptHandler {
    async fn handle(&self, effect: RunScriptEffect) -> ScriptLanding {
        let capture = effect.capture;
        let snapshots: Arc<Vec<ScriptSnapshot>> = Arc::new(capture.snapshots);
        let world = crate::ScriptWorld {
            read: Box::new({
                let snapshots = Arc::clone(&snapshots);
                let base = capture.base.clone();
                let caller = self.caller.clone();
                move |path: String| -> crate::WorldFuture<Option<String>> {
                    let Some(location) = resolve(&base, &path) else {
                        return Box::pin(std::future::ready(None));
                    };
                    if let Some(snapshot) = snapshots
                        .iter()
                        .find(|snapshot| snapshot.location == location)
                    {
                        let mut view = snapshot.text.view();
                        let end = view.byte_count().min(u32::MAX as usize) as u32;
                        return Box::pin(std::future::ready(Some(view.substring(0..end))));
                    }
                    let caller = caller.clone();
                    Box::pin(async move {
                        caller
                            .call(himark::FetchDocumentEffect { location })
                            .await
                            .flatten()
                    })
                }
            }),
            ..crate::ScriptWorld::disconnected()
        };
        let agent = capture.agent.map(Arc::new);
        let world = crate::ScriptWorld {
            ask: Box::new({
                move |prompt| {
                    let Some(agent) = agent.clone() else {
                        return Box::pin(std::future::ready(Err(
                            "no agent session — attach the workspace to one".to_owned(),
                        )));
                    };
                    Box::pin(async move { drive_turn(&agent, prompt).await })
                }
            }),
            changes: Box::new({
                let changes = capture.changes.clone();
                move || Box::pin(std::future::ready(changes.clone()))
            }),
            ..world
        };
        let outcome = crate::run_script(
            &capture.name,
            &capture.source,
            world,
            crate::Limits::default(),
        )
        .await;

        let mut edits = Vec::new();
        let mut log = outcome.log;
        for write in outcome.writes {
            let Some(location) = resolve(&capture.base, &write.target) else {
                log.push(format!(
                    "write dropped: unresolvable path {:?}",
                    write.target
                ));
                continue;
            };
            match snapshots
                .iter()
                .find(|snapshot| snapshot.location == location)
            {
                Some(snapshot) => {
                    let fresh = himark::Text::from_string_exact(&write.text);
                    edits.push(ScriptEdit::Open {
                        id: snapshot.id,
                        base_revision: snapshot.revision,
                        // A patch, not a picture (docs/editor/structural-diff.md, decision 3).
                        operation: myersdiff::diff(&snapshot.text, &fresh),
                    });
                }
                None => edits.push(ScriptEdit::Store {
                    location,
                    text: write.text,
                }),
            }
        }
        let mut shows = Vec::new();
        for show in outcome.shows {
            match resolve(&capture.base, &show) {
                Some(location) => shows.push(location),
                None => log.push(format!("show dropped: unresolvable path {show:?}")),
            }
        }
        ScriptLanding {
            name: capture.name,
            log,
            error: outcome.error,
            edits,
            shows,
        }
    }
}

fn show_now_or_with_store(
    shows: &mut Vec<ResourceLocation>,
    stored: &ResourceLocation,
) -> Option<ResourceLocation> {
    let at = shows.iter().position(|show| show == stored)?;
    Some(shows.remove(at))
}

pub struct RunScript;

impl himark::DynamicEditorCommand for RunScript {
    fn id(&self) -> &'static str {
        "script.run"
    }

    fn name(&self) -> String {
        "Run Script".to_owned()
    }

    fn offers_at(&self, location: &ResourceLocation) -> bool {
        location.name().ends_with(".js")
    }

    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::UiCtx,
        document: &mut himark::Document,
        _editor: himark::EditorId,
        location: &ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut himark::EditorEffects<'_>,
    ) {
        if let Some(payload) = payload {
            let payload = match payload.downcast::<ScriptLanding>() {
                Ok(landing) => return land(store, ui, *landing, fx),
                Err(payload) => payload,
            };
            if let Ok(stored) = payload.downcast::<ScriptStored>() {
                match (stored.stored, stored.show) {
                    (true, Some(location)) => himark::AppRequests::push(
                        store,
                        Arc::new(ShowDocuments {
                            locations: vec![location],
                        }),
                    ),
                    (false, _) => eprintln!("[script] a store-through failed"),
                    _ => {}
                }
            }
            return;
        }

        let name = location.path().join("/");
        let source = {
            let mut view = document.text().view();
            let end = view.byte_count().min(u32::MAX as usize) as u32;
            view.substring(0..end)
        };
        let snapshots = OpenDocuments::list(store)
            .into_iter()
            .filter_map(|(id, entity)| {
                let location = entity.location()?.clone();
                if himark::is_synthetic(&location) {
                    return None;
                }
                let document = entity.document();
                Some(ScriptSnapshot {
                    location,
                    id,
                    revision: document.revision(),
                    text: document.text().clone(),
                })
            })
            .collect();

        let agent = himark::Windows::list(store)
            .into_iter()
            .next()
            .and_then(|window| {
                let entity = himark::Windows::window_ref(store, window)?;
                let session = entity.current_session();
                if !session.names_session() {
                    return None;
                }
                let seat = himark::higent::Servers::seat(store, session.host)?;
                Some(ScriptAgent {
                    seat,
                    session: session.session,
                })
            });
        let capture = ScriptCapture {
            name: name.clone(),
            base: location.clone(),
            source,
            snapshots,
            agent,
            changes: himark::hichanges::Changes::script_summary(store),
        };
        let token = fx.push(AnyEffect::new(RunScriptEffect { capture }).map(|landing| {
            himark::EditorCommand::Dynamic {
                id: "script.run",
                payload: Some(Box::new(landing)),
            }
        }));
        let mut lanes = store.get::<ScriptLanes>().cloned().unwrap_or_default();
        if let Some(previous) = lanes.0.insert(name, token) {
            fx.cancel(previous);
        }
        store.put(lanes);
    }
}

fn land(
    store: &mut Store,
    ui: &imba::UiCtx,
    landing: ScriptLanding,
    fx: &mut himark::EditorEffects<'_>,
) {
    let mut log = landing.log;
    let mut shows = landing.shows;
    for edit in landing.edits {
        match edit {
            ScriptEdit::Open {
                id,
                base_revision,
                operation,
            } => {
                let Some(mut document) = OpenDocuments::document(store, id) else {
                    log.push("write dropped: the target closed mid-run".to_owned());
                    continue;
                };
                if document.revision() != base_revision {
                    log.push("write dropped: the target changed mid-run".to_owned());
                    continue;
                }
                if operation
                    .iter()
                    .all(|op| matches!(op, operation::Op::Retain(_)))
                {
                    continue;
                }
                let text_before = document.text().clone();
                let fonts = himark::env::Fonts::of(store)();
                let theme = himark::env::Themes::of(store);
                document.edit(&operation,
                store, ui, &fonts, &theme, fx);
                if let Some(parsers) = himark::env::Parsers::of(store) {
                    document.launch_reparse(parsers, fx);
                }
                if let Some(location) = OpenDocuments::location(store, id) {
                    for sink in himark::InstalledChangeSink::of(store) {
                        sink.changed(store, &document, &location, base_revision, &text_before, fx);
                    }
                }
                OpenDocuments::put_document(store, id, document);
            }
            ScriptEdit::Store { location, text } => {
                let show = show_now_or_with_store(&mut shows, &location);
                let _ = fx.push(
                    AnyEffect::new(himark::StoreDocumentEffect { location, text }).map(
                        move |stored| himark::EditorCommand::Dynamic {
                            id: "script.run",
                            payload: Some(Box::new(ScriptStored { stored, show })),
                        },
                    ),
                );
            }
        }
    }
    if !shows.is_empty() {
        himark::AppRequests::push(store, Arc::new(ShowDocuments { locations: shows }));
    }
    if let Some(error) = &landing.error {
        eprintln!("[script] {} failed: {error}", landing.name);
    }
    ScriptRuns::record(
        store,
        ScriptRecord {
            name: landing.name,
            error: landing.error,
            log,
        },
    );
}

#[cfg(test)]
mod tests;
