// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::effect::EffectHandler;
use imba::store::Store;

use super::*;
use himark::test_document::plain_document;

fn located(path: &[&str]) -> ResourceLocation {
    ResourceLocation::new(
        himark::ResourceType::document(),
        himark::Authority::new("test"),
        path.iter().map(|s| s.to_string()).collect::<Vec<String>>(),
    )
}

fn text_of(store: &Store, id: DocumentId) -> String {
    let document = OpenDocuments::document_ref(store, id).expect("the document");
    let mut view = document.text().view();
    let end = view.byte_count().min(u32::MAX as usize) as u32;
    view.substring(0..end)
}

fn registered(store: &mut Store, path: &[&str], source: &str) -> DocumentId {
    let document = plain_document(source);
    let saved = document.revision();
    OpenDocuments::register(
        store,
        document,
        Some(located(path)),
        path.last().expect("a name").to_string(),
        saved,
    )
}

fn launched(store: &mut Store, script: DocumentId) -> RunScriptEffect {
    let ui = himark::test_document::test_ui();
    let location = OpenDocuments::location(&store, script).expect("located");
    let mut document = OpenDocuments::document(&store, script).expect("the document");
    let editor = document.add_editor(
        400.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        store,
        ui,
        &himark::embedded_fonts::source()(),
        &himark::env::Themes::of(&store),
        &mut imba::effect::Batch::new().effects(),
    );
    let mut batch = imba::effect::Batch::new();
    himark::DynamicEditorCommand::perform(
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
    _ui: &imba::UiCtx,
    script: DocumentId,
    landing: ScriptLanding,
) -> imba::effect::Batch<himark::EditorCommand> {
    let ui = himark::test_document::test_ui();
    let location = OpenDocuments::location(&store, script).expect("located");
    let mut document = OpenDocuments::document(&store, script).expect("the document");
    let editor = document.add_editor(
        400.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        store,
        ui,
        &himark::embedded_fonts::source()(),
        &himark::env::Themes::of(&store),
        &mut imba::effect::Batch::new().effects(),
    );
    let mut batch = imba::effect::Batch::new();
    himark::DynamicEditorCommand::perform(
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
    let ui = himark::test_document::test_ui();
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
    let ui = himark::test_document::test_ui();
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
    let ui = himark::test_document::test_ui();
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
        .downcast::<himark::StoreDocumentEffect>()
        .expect("the store effect");
    assert_eq!(effect.location.path(), ["repo", "out.md"]);
    assert_eq!(effect.text, "made");
}

#[test]
fn typing_mid_run_discards_the_write() {
    let ui = himark::test_document::test_ui();
    let mut store = Store::new();
    let script = registered(&mut store, &["repo", "walk.js"], APPEND_SCRIPT);
    let plan = registered(&mut store, &["repo", "plan.md"], "alpha");
    let landing = ran(launched(&mut store, script));

    let mut document = OpenDocuments::document(&store, plan).expect("the document");
    document.edit(
        &operation::Operation::insert_at(0, "typed "),
        &store,
        ui,
        &himark::embedded_fonts::source()(),
        &himark::env::Themes::of(&store),
        &mut imba::effect::Batch::new().effects(),
    );
    OpenDocuments::put_document(&mut store, plan, document);
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

use himark::higent::ahp_types::actions::{
    ChatDeltaAction, ChatResponsePartAction, ChatTurnCompleteAction, StateAction,
};
use himark::higent::ahp_types::state::{MarkdownResponsePart, ResponsePart};

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

impl himark::higent::AhpServer for ScriptedSeat {
    fn create_chat(&self, session: String) -> himark::higent::SeatFuture<Result<String, String>> {
        assert_eq!(session, "session:test");
        Box::pin(std::future::ready(Ok("chat:script".to_owned())))
    }

    fn subscribe_chat(
        &self,
        _chat: String,
    ) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::state::ChatState, String>>
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
        _chat: String,
        text: String,
        _attachments: Option<Vec<himark::higent::ahp_types::state::MessageAttachment>>,
        _model: Option<himark::higent::ahp_types::state::ModelSelection>,
    ) -> himark::higent::SeatFuture<Result<(), String>> {
        *self.prompt.lock().expect("prompt") = Some(text);
        Box::pin(std::future::ready(Ok(())))
    }

    fn poll_chat(&self, _chat: String) -> himark::higent::SeatFuture<Vec<StateAction>> {
        let batch = self
            .feed
            .lock()
            .expect("feed")
            .pop_front()
            .expect("the scripted feed never runs dry before turnComplete");
        Box::pin(std::future::ready(batch))
    }

    unreached! {
        connect() -> himark::higent::SeatFuture<Result<himark::higent::RootInfo, String>>;
        list_sessions(cursor: Option<String>) -> himark::higent::SeatFuture<Result<himark::higent::SessionsPage, String>>;
        poll_root() -> himark::higent::SeatFuture<Vec<himark::higent::ServerEvent>>;
        create_session(dirs: Vec<String>, options: himark::higent::SessionOptions) -> himark::higent::SeatFuture<Result<String, String>>;
        resolve_session_config(working_directory: Option<String>, config: Option<serde_json::Map<String, serde_json::Value>>) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::commands::ResolveSessionConfigResult, String>>;
        dispose_session(session: String) -> himark::higent::SeatFuture<Result<(), String>>;
        subscribe_session(session: String) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::state::SessionState, String>>;
        poll_session(session: String) -> himark::higent::SeatFuture<Vec<StateAction>>;
        fetch_turns(chat: String, cursor: Option<String>) -> himark::higent::SeatFuture<Result<himark::higent::TurnsPage, String>>;
        cancel_turn(chat: String, turn: String) -> himark::higent::SeatFuture<()>;
        dispatch_action(chat: String, action: StateAction) -> himark::higent::SeatFuture<Result<(), String>>;
        read_file_edit(before: Option<String>, after: Option<String>) -> himark::higent::SeatFuture<Result<himark::higent::FileEditContents, String>>;
        resource_read(session: String, uri: himark::higent::ResourceUri) -> himark::higent::SeatFuture<Option<String>>;
        resource_write(session: String, uri: himark::higent::ResourceUri, text: String) -> himark::higent::SeatFuture<bool>;
        resource_list(session: String, uri: himark::higent::ResourceUri) -> himark::higent::SeatFuture<Option<Vec<(String, bool)>>>;
        resource_watch(session: String, uri: himark::higent::ResourceUri, events: StdArc<dyn Fn() + Send + Sync>) -> himark::higent::SeatFuture<Option<himark::higent::WatchHandle>>;
        resource_unwatch(handle: himark::higent::WatchHandle) -> himark::higent::SeatFuture<()>;
        search(session: String, ask: himark::higent::SearchAsk) -> himark::higent::SeatFuture<Option<himark::higent::SearchResult>>;
        terminal_input(channel: &String, data: String) -> ();
        terminal_resize(channel: &String, cols: u16, rows: u16) -> ();
        terminal_dispose(channel: &String) -> ();
        subscribe_changeset(channel: String) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::state::ChangesetState, String>>;
        poll_changeset(channel: String) -> himark::higent::SeatFuture<Vec<StateAction>>;
        unsubscribe_changeset(channel: &String) -> ();
        subscribe_annotations(session: String) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::state::AnnotationsState, String>>;
        poll_annotations(session: String) -> himark::higent::SeatFuture<Vec<StateAction>>;
        dispatch_annotations(session: &String, action: StateAction) -> ();
        unsubscribe_annotations(session: &String) -> ();
        open_document(session: String, uri: Option<himark::higent::ResourceUri>, text: Option<String>) -> himark::higent::SeatFuture<Result<himark::higent::seat::OpenDocumentResult, String>>;
        subscribe_document(channel: String) -> himark::higent::SeatFuture<Result<himark::higent::seat::DocumentState, String>>;
        poll_document(channel: String) -> himark::higent::SeatFuture<Vec<himark::higent::seat::DocumentApplied>>;
        dispatch_document(channel: &String, action: himark::higent::seat::DocumentApplied) -> ();
        unsubscribe_document(channel: &String) -> himark::higent::SeatFuture<()>;
        lsp(session: String, method: String, params: serde_json::Value) -> himark::higent::SeatFuture<Result<serde_json::Value, String>>;
    }

    fn terminal_open(
        &self,
        _session: String,
        _channel: String,
        _cwd: Option<String>,
        _cols: u16,
        _rows: u16,
        _events: StdArc<dyn Fn(himark::higent::TerminalEvent) + Send + Sync>,
    ) -> himark::higent::SeatFuture<Option<himark::higent::TerminalHandle>> {
        unreachable!("the script tests never open terminals")
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
    let ui = himark::test_document::test_ui();
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
        seat: seat.clone(),
        session: "session:test".to_owned(),
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
    let ui = himark::test_document::test_ui();
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
        himark::higent::ahp_types::actions::ChatErrorAction {
            turn_id: "turn:1".to_owned(),
            duration: 1,
            error: serde_json::from_value(serde_json::json!({
                "errorType": "provider",
                "message": "quota exhausted",
            }))
            .expect("an error info"),
            meta: None,
        },
    )]]);
    let mut effect = launched(&mut store, script);
    effect.capture.agent = Some(ScriptAgent {
        seat,
        session: "session:test".to_owned(),
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
    let ui = himark::test_document::test_ui();
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
            .get::<himark::AppRequests>()
            .is_some_and(|requests| !requests.is_empty())
    };
    assert!(!queued(&store), "nothing queued before the landing");
    landed(&mut store, &ui, script, landing);
    assert!(
        queued(&store),
        "the OPEN-target show queued its request at the landing"
    );

    store.put(himark::AppRequests::default());
    let location = located(&["repo", "new.md"]);
    let script_doc = script;
    let stored_landing = |store: &mut Store, stored: ScriptStored| {
        let location = OpenDocuments::location(store, script_doc).expect("located");
        let mut document = OpenDocuments::document(store, script_doc).expect("the document");
        let editor = document.add_editor(
            400.0,
            None,
            himark::EditorBuild::Complete,
            &[],
            store,
            &ui,
            &himark::embedded_fonts::source()(),
            &himark::env::Themes::of(store),
            &mut imba::effect::Batch::new().effects(),
        );
        himark::DynamicEditorCommand::perform(
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
    store.put(himark::AppRequests::default());
    stored_landing(
        &mut store,
        ScriptStored {
            stored: false,
            show: Some(location),
        },
    );
    assert!(!queued(&store), "a FAILED store never opens its target");
}
