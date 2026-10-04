// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::effect::EffectHandler;
use imba::store::Store;

use super::*;
use ::editor::test_document::plain_document;

fn located(path: &[&str]) -> ResourceLocation {
    ResourceLocation::new(
        editor::location::ResourceType::document(),
        editor::location::Authority::new("test"),
        path.iter().map(|s| s.to_string()).collect::<Vec<String>>(),
    )
}

fn text_of(store: &Store, id: DocumentId) -> String {
    let document = OpenDocuments::document_ref(store, test_docs(&store), id).expect("the document");
    let mut view = document.text().view();
    let end = view.byte_count().min(u32::MAX as usize) as u32;
    view.substring(0..end)
}

fn registered(store: &mut Store, path: &[&str], source: &str) -> DocumentId {
    let document = plain_document(source);
    let saved = document.revision();
    let documents =
        ahp_session::session::state::Hosts::ensure_state(store, &ahp_wire::SessionId::local_default(store))
            .documents();
    OpenDocuments::register(
        store,
        documents,
        document,
        Some(located(path)),
        path.last().expect("a name").to_string(),
        saved,
    )
}

fn launched(store: &mut Store, script: DocumentId) -> RunScriptEffect {
    let ui = ::editor::test_document::test_ui();
    let location = OpenDocuments::location(&store, test_docs(&store), script).expect("located");
    let mut document =
        OpenDocuments::document(&store, test_docs(&store), script).expect("the document");
    let editor = document.add_editor(
        400.0,
        None,
        editor::document::EditorBuild::Complete,
        &[],
        store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::env::Themes::of(&store),
        &mut imba::effect::Batch::new().effects(),
    );
    let mut batch = imba::effect::Batch::new();
    editor::dynamic::DynamicEditorCommand::perform(
        &RunScript,
        store,
        ui,
        &mut document,
        editor,
        &location,
        None,
        &mut batch.effects(),
    );
    let mut launches = himark::test_support::surviving_launches(batch);
    assert_eq!(launches.len(), 1, "one run, one lane, one launch");
    *launches
        .pop()
        .expect("launch")
        .into_payload()
        .split()
        .0
        .downcast::<RunScriptEffect>()
        .expect("the run effect")
}

fn ran(effect: RunScriptEffect) -> ScriptLanding {
    let handler = RunScriptHandler {
        caller: imba::effect::EffectCaller::disconnected(),
    };
    imba::effect::block_on(Box::pin(async move { handler.handle(effect).await }))
}

fn landed(
    store: &mut Store,
    _ui: &imba::ui::UiCtx,
    script: DocumentId,
    landing: ScriptLanding,
) -> imba::effect::Batch<editor::editor_view::EditorCommand> {
    let ui = ::editor::test_document::test_ui();
    let location = OpenDocuments::location(&store, test_docs(&store), script).expect("located");
    let mut document =
        OpenDocuments::document(&store, test_docs(&store), script).expect("the document");
    let editor = document.add_editor(
        400.0,
        None,
        editor::document::EditorBuild::Complete,
        &[],
        store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::env::Themes::of(&store),
        &mut imba::effect::Batch::new().effects(),
    );
    let mut batch = imba::effect::Batch::new();
    editor::dynamic::DynamicEditorCommand::perform(
        &RunScript,
        store,
        ui,
        &mut document,
        editor,
        &location,
        Some(Box::new(landing)),
        &mut batch.effects(),
    );
    batch
}

const APPEND_SCRIPT: &str = r#"export default async function (himark) {
    const current = await himark.docs.read("plan.md");
    await himark.docs.write("plan.md", current + "\nMORE");
    himark.log("appended");
}"#;

#[test]
fn a_run_reads_the_open_document_and_lands_its_write() {
    let ui = ::editor::test_document::test_ui();
    let mut store = Store::new();
    let script = registered(&mut store, &["repo", "walk.js"], APPEND_SCRIPT);
    let plan = registered(&mut store, &["repo", "plan.md"], "alpha");
    let landing = ran(launched(&mut store, script));
    assert_eq!(landing.error, None, "log: {:?}", landing.log);
    landed(&mut store, &ui, script, landing);
    assert_eq!(text_of(&store, plan), "alpha\nMORE");
    let runs = ScriptRuns::of(&store);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].name, "repo/walk.js");
    assert_eq!(runs[0].log, vec!["appended"]);
    assert_eq!(runs[0].error, None);
}

#[test]
fn a_failed_run_commits_nothing() {
    let ui = ::editor::test_document::test_ui();
    let mut store = Store::new();
    let script = registered(
        &mut store,
        &["repo", "bad.js"],
        r#"export default async function (himark) {
            await himark.docs.write("plan.md", "clobbered");
            throw new Error("late boom");
        }"#,
    );
    let plan = registered(&mut store, &["repo", "plan.md"], "alpha");
    let landing = ran(launched(&mut store, script));
    landed(&mut store, &ui, script, landing);
    assert_eq!(
        text_of(&store, plan),
        "alpha",
        "the half-run committed nothing"
    );
    let runs = ScriptRuns::of(&store);
    assert!(runs[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("late boom")));
}

#[test]
fn an_unopened_target_stores_through_the_host() {
    let ui = ::editor::test_document::test_ui();
    let mut store = Store::new();
    let script = registered(
        &mut store,
        &["repo", "notes", "gen.js"],
        r#"export default async function (himark) {
            await himark.docs.write("../out.md", "made");
        }"#,
    );
    let landing = ran(launched(&mut store, script));
    assert_eq!(landing.error, None, "log: {:?}", landing.log);
    let batch = landed(&mut store, &ui, script, landing);
    let mut launches = himark::test_support::surviving_launches(batch);
    assert_eq!(launches.len(), 1, "one store-through");
    let effect = launches
        .pop()
        .expect("launch")
        .into_payload()
        .split()
        .0
        .downcast::<documents::StoreDocumentEffect>()
        .expect("the store effect");
    assert_eq!(effect.location.path(), ["repo", "out.md"]);
    assert_eq!(effect.text, "made");
}

#[test]
fn typing_mid_run_discards_the_write() {
    let ui = ::editor::test_document::test_ui();
    let mut store = Store::new();
    let script = registered(&mut store, &["repo", "walk.js"], APPEND_SCRIPT);
    let plan = registered(&mut store, &["repo", "plan.md"], "alpha");
    let landing = ran(launched(&mut store, script));

    let mut document =
        OpenDocuments::document(&store, test_docs(&store), plan).expect("the document");
    let len = document.text().byte_count() as u32;
    document.edit(
        &operation::operation::Operation::insert_in(len, 0, "typed "),
        &store,
        ui,
        ::editor::test_document::test_fonts_collection(),
        &::editor::env::Themes::of(&store),
        &mut imba::effect::Batch::new().effects(),
    );
    let documents = test_docs(&store);
    OpenDocuments::put_document(&mut store, documents, plan, document);
    landed(&mut store, &ui, script, landing);
    assert_eq!(text_of(&store, plan), "typed alpha", "ours stands");
    let runs = ScriptRuns::of(&store);
    assert!(
        runs[0].log.iter().any(|line| line.contains("dropped")),
        "the discard is named: {:?}",
        runs[0].log
    );
}

#[test]
fn a_relaunch_supersedes_the_lane() {
    let mut store = Store::new();
    let script = registered(&mut store, &["repo", "walk.js"], APPEND_SCRIPT);
    let _ = registered(&mut store, &["repo", "plan.md"], "alpha");
    let _first = launched(&mut store, script);
    let first_token = store
        .get::<ScriptLanes>()
        .and_then(|lanes| lanes.0.get("repo/walk.js").cloned())
        .expect("the lane stands");
    let _second = launched(&mut store, script);
    let second_token = store
        .get::<ScriptLanes>()
        .and_then(|lanes| lanes.0.get("repo/walk.js").cloned())
        .expect("the lane stands");
    assert_ne!(
        format!("{first_token:?}"),
        format!("{second_token:?}"),
        "the relaunch took the lane"
    );
}

#[test]
fn paths_resolve_against_the_scripts_directory() {
    let base = located(&["repo", "notes", "walk.js"]);
    assert_eq!(
        resolve(&base, "plan.md").expect("sibling").path(),
        ["repo", "notes", "plan.md"]
    );
    assert_eq!(
        resolve(&base, "./sub/x.md").expect("descend").path(),
        ["repo", "notes", "sub", "x.md"]
    );
    assert_eq!(
        resolve(&base, "../top.md").expect("climb").path(),
        ["repo", "top.md"]
    );
    assert_eq!(resolve(&base, "../../../escape.md"), None);
}

use std::collections::VecDeque;
use std::sync::{Arc as StdArc, Mutex};

use ahp_types::actions::{
    ChatDeltaAction, ChatResponsePartAction, ChatTurnCompleteAction, StateAction,
};
use ahp_types::state::{MarkdownResponsePart, ResponsePart};

/// The collection the plugin resolves in production (the location's
/// owner — a bare test store routes to the local default session);
/// `registered` mints the session, everyone else reads it back.
fn test_docs(store: &Store) -> imba::store::Id<documents::OpenDocuments> {
    ahp_session::session::state::Hosts::state(store, &ahp_wire::SessionId::local_default(store))
        .expect("the local state is minted by the first register")
        .documents()
}

macro_rules! unreached {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $out:ty;)*) => {
        $(fn $name(&self, $($arg: $ty),*) -> $out {
            $(let _ = $arg;)*
            unreachable!("the script tests never reach this seat road")
        })*
    };
}

struct ScriptedSeat {
    prompt: Mutex<Option<String>>,
    feed: Mutex<VecDeque<Vec<StateAction>>>,
}

impl ScriptedSeat {
    fn answering(batches: Vec<Vec<StateAction>>) -> StdArc<Self> {
        StdArc::new(Self {
            prompt: Mutex::new(None),
            feed: Mutex::new(batches.into()),
        })
    }
}

impl ahp_wire::client::ChatClient for ScriptedSeat {
    fn create_chat(
        &self,
        session: ahp_wire::client::SessionUri,
    ) -> ahp_wire::client::ClientFuture<Result<ahp_wire::client::ChatUri, String>> {
        assert_eq!(session.as_str(), "session:test");
        Box::pin(std::future::ready(Ok(ahp_wire::client::ChatUri::new(
            "chat:script",
        ))))
    }

    fn subscribe_chat(
        &self,
        _chat: ahp_wire::client::ChatUri,
    ) -> ahp_wire::client::ClientFuture<Result<ahp_types::state::ChatState, String>>
    {
        let state = serde_json::from_value(serde_json::json!({
            "resource": "chat:script",
            "title": "",
            "status": 0,
            "modifiedAt": "2026-09-07T00:00:00Z",
            "turns": [],
        }))
        .expect("a minimal chat state");
        Box::pin(std::future::ready(Ok(state)))
    }

    fn start_turn(
        &self,
        _chat: ahp_wire::client::ChatUri,
        text: String,
        _attachments: Option<Vec<ahp_types::state::MessageAttachment>>,
        _model: Option<ahp_types::state::ModelSelection>,
    ) -> ahp_wire::client::ClientFuture<Result<(), String>> {
        *self.prompt.lock().expect("prompt") = Some(text);
        Box::pin(std::future::ready(Ok(())))
    }

    fn poll_chat(
        &self,
        _chat: ahp_wire::client::ChatUri,
    ) -> ahp_wire::client::ClientFuture<Vec<StateAction>> {
        let batch = self
            .feed
            .lock()
            .expect("feed")
            .pop_front()
            .expect("the scripted feed never runs dry before turnComplete");
        Box::pin(std::future::ready(batch))
    }

    unreached! {
        fetch_turns(chat: ahp_wire::client::ChatUri, cursor: Option<String>) -> ahp_wire::client::ClientFuture<Result<ahp_wire::client::TurnsPage, String>>;
        cancel_turn(chat: ahp_wire::client::ChatUri, turn: ahp_wire::client::TurnId) -> ahp_wire::client::ClientFuture<()>;
        read_file_edit(before: Option<String>, after: Option<String>) -> ahp_wire::client::ClientFuture<Result<ahp_wire::client::FileEditContents, String>>;
    }
}

fn markdown_part(id: &str, content: &str) -> StateAction {
    StateAction::ChatResponsePart(ChatResponsePartAction {
        turn_id: "turn:1".to_owned(),
        part: ResponsePart::Markdown(MarkdownResponsePart {
            id: id.to_owned(),
            content: content.to_owned(),
        }),
        meta: None,
    })
}

fn delta(part: &str, content: &str) -> StateAction {
    StateAction::ChatDelta(ChatDeltaAction {
        turn_id: "turn:1".to_owned(),
        part_id: part.to_owned(),
        content: content.to_owned(),
        meta: None,
    })
}

fn complete() -> StateAction {
    StateAction::ChatTurnComplete(ChatTurnCompleteAction {
        turn_id: "turn:1".to_owned(),
        duration: 1,
        meta: None,
    })
}

#[test]
fn an_agent_ask_drives_a_turn_and_lands_the_reply() {
    let ui = ::editor::test_document::test_ui();
    let mut store = Store::new();
    let script = registered(
        &mut store,
        &["repo", "walk.js"],
        r#"export default async function (himark) {
            const diff = await himark.vcs.changes();
            const prose = await himark.agent.ask("narrate: " + diff);
            await himark.docs.write("plan.md", prose);
        }"#,
    );
    let plan = registered(&mut store, &["repo", "plan.md"], "old");
    let seat = ScriptedSeat::answering(vec![
        vec![markdown_part("p1", "The changes ")],
        vec![delta("p1", "narrated."), complete()],
    ]);
    let mut effect = launched(&mut store, script);
    effect.capture.agent = Some(ScriptAgent {
        client: seat.clone(),
        session: ahp_wire::client::SessionUri::new("session:test"),
    });
    effect.capture.changes = Some("M repo/x.rs (+1 -2)".to_owned());
    let landing = ran(effect);
    assert_eq!(landing.error, None, "log: {:?}", landing.log);
    landed(&mut store, &ui, script, landing);
    assert_eq!(text_of(&store, plan), "The changes narrated.");
    assert_eq!(
        seat.prompt.lock().expect("prompt").as_deref(),
        Some("narrate: M repo/x.rs (+1 -2)"),
        "the vcs summary reached the agent"
    );
}

#[test]
fn a_failed_turn_fails_the_run() {
    let ui = ::editor::test_document::test_ui();
    let mut store = Store::new();
    let script = registered(
        &mut store,
        &["repo", "walk.js"],
        r#"export default async function (himark) {
            await himark.docs.write("plan.md", "half");
            await himark.agent.ask("anything");
        }"#,
    );
    let plan = registered(&mut store, &["repo", "plan.md"], "old");
    let seat = ScriptedSeat::answering(vec![vec![StateAction::ChatError(
        ahp_types::actions::ChatErrorAction {
            turn_id: "turn:1".to_owned(),
            duration: 1,
            part: serde_json::from_value(serde_json::json!({
                "error": {
                    "errorType": "provider",
                    "message": "quota exhausted",
                },
            }))
            .expect("an error part"),
            meta: None,
        },
    )]]);
    let mut effect = launched(&mut store, script);
    effect.capture.agent = Some(ScriptAgent {
        client: seat,
        session: ahp_wire::client::SessionUri::new("session:test"),
    });
    let landing = ran(effect);
    assert!(
        landing
            .error
            .as_deref()
            .is_some_and(|error| error.contains("quota exhausted")),
        "error: {:?}",
        landing.error
    );
    landed(&mut store, &ui, script, landing);
    assert_eq!(
        text_of(&store, plan),
        "old",
        "the half-run committed nothing"
    );
}

#[test]
fn an_agentless_ask_names_the_missing_session() {
    let mut store = Store::new();
    let script = registered(
        &mut store,
        &["repo", "walk.js"],
        r#"export default async function (himark) {
            try { await himark.agent.ask("x"); }
            catch (error) { himark.log(String(error)); }
        }"#,
    );
    let landing = ran(launched(&mut store, script));
    assert_eq!(landing.error, None);
    assert!(
        landing.log[0].contains("no agent session"),
        "log: {:?}",
        landing.log
    );
}

#[test]
fn shows_file_now_or_ride_their_store() {
    let ui = ::editor::test_document::test_ui();
    let mut store = Store::new();
    let script = registered(
        &mut store,
        &["repo", "gen.js"],
        r#"export default async function (himark) {
            await himark.docs.write("plan.md", "fresh");
            await himark.docs.show("plan.md");
            await himark.docs.write("new.md", "made");
            await himark.docs.show("new.md");
        }"#,
    );
    let _plan = registered(&mut store, &["repo", "plan.md"], "old");
    let landing = ran(launched(&mut store, script));
    assert_eq!(landing.error, None, "log: {:?}", landing.log);
    let queued = |store: &Store| {
        store
            .get::<himark::commands::AppRequests>()
            .is_some_and(|requests| !requests.is_empty())
    };
    assert!(!queued(&store), "nothing queued before the landing");
    landed(&mut store, &ui, script, landing);
    assert!(
        queued(&store),
        "the OPEN-target show queued its request at the landing"
    );

    store.put(himark::commands::AppRequests::default());
    let location = located(&["repo", "new.md"]);
    let script_doc = script;
    let stored_landing = |store: &mut Store, stored: ScriptStored| {
        let location =
            OpenDocuments::location(store, test_docs(&store), script_doc).expect("located");
        let mut document =
            OpenDocuments::document(store, test_docs(&store), script_doc).expect("the document");
        let editor = document.add_editor(
            400.0,
            None,
            editor::document::EditorBuild::Complete,
            &[],
            store,
            &ui,
            ::editor::test_document::test_fonts_collection(),
            &::editor::env::Themes::of(store),
            &mut imba::effect::Batch::new().effects(),
        );
        editor::dynamic::DynamicEditorCommand::perform(
            &RunScript,
            store,
            &ui,
            &mut document,
            editor,
            &location,
            Some(Box::new(stored)),
            &mut imba::effect::Batch::new().effects(),
        );
    };
    stored_landing(
        &mut store,
        ScriptStored {
            stored: true,
            show: Some(location.clone()),
        },
    );
    assert!(
        queued(&store),
        "the deferred show queued on the store's landing"
    );
    store.put(himark::commands::AppRequests::default());
    stored_landing(
        &mut store,
        ScriptStored {
            stored: false,
            show: Some(location),
        },
    );
    assert!(!queued(&store), "a FAILED store never opens its target");
}
