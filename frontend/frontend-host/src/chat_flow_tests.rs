// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Chat streaming e2e over a SCRIPTED server: the test owns the wire
//! side of one chat channel — the snapshot it answers, every poll
//! batch, the older-turns pages — and drives the REAL app through
//! the real roads (the session-open road, Boot → subscribe, the live
//! poll chain, panes, windows). Covered: snapshot + pagination, a
//! LONG streamed turn with tools, several views over one chat (two
//! windows), displacement + ⌘I mid-stream, and a verbatim REPLAY of
//! the stream (an overlapping subscription must fold to one copy).

use std::sync::{Arc, Mutex};

use himark::higent::ahp_types::actions::{
    ChatDeltaAction, ChatResponsePartAction, ChatToolCallCompleteAction, ChatToolCallStartAction,
    ChatTurnCompleteAction, ChatTurnStartedAction, StateAction,
};
use himark::higent::ahp_types::state::{
    ChatState, ChatSummary, MarkdownResponsePart, Message, MessageKind, MessageOrigin,
    ResponsePart, SessionLifecycle, SessionState, ToolCallResult, Turn, TurnState,
};
use himark::higent::{ChannelUri, ChatUri, SessionUri, TurnId};

use crate::{AppFonts, HimarkEngine};

/// The scripted wire side of ONE chat: the snapshot to answer, the
/// poll batches the test feeds, the older pages behind the cursor.
#[derive(Clone)]
struct Script {
    chat: ChatUri,
    session: SessionUri,
    snapshot: Arc<Mutex<ChatState>>,
    batches: Arc<Mutex<std::collections::VecDeque<Vec<StateAction>>>>,
    older: Arc<Mutex<std::collections::HashMap<String, himark::higent::TurnsPage>>>,
    parked: Arc<Mutex<Vec<std::task::Waker>>>,
}

impl Script {
    fn new(chat: &str, session: &str, snapshot: ChatState) -> Self {
        Self {
            chat: ChatUri::new(chat),
            session: SessionUri::new(session),
            snapshot: Arc::new(Mutex::new(snapshot)),
            batches: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            older: Arc::new(Mutex::new(std::collections::HashMap::new())),
            parked: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn feed(&self, batch: Vec<StateAction>) {
        self.batches.lock().expect("batches").push_back(batch);
        for waker in self.parked.lock().expect("parked").drain(..) {
            waker.wake();
        }
    }
}

struct ScriptedSeat {
    script: Script,
}

macro_rules! unreached {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $out:ty;)*) => {
        $(fn $name(&self, $($arg: $ty),*) -> $out {
            $(let _ = $arg;)*
            unreachable!("the chat flow never reaches this seat road")
        })*
    };
}

impl himark::higent::AhpServer for ScriptedSeat {
    fn subscribe_session(
        &self,
        session: SessionUri,
    ) -> himark::higent::SeatFuture<Result<SessionState, String>> {
        assert_eq!(session, self.script.session);
        let chat = self.script.chat.as_str().to_owned();
        Box::pin(std::future::ready(Ok(SessionState {
            provider: "scripted".to_owned(),
            title: "scripted session".to_owned(),
            status: 0,
            activity: None,
            project: None,
            working_directories: None,
            annotations: None,
            lifecycle: SessionLifecycle::Ready,
            creation_error: None,
            server_tools: None,
            active_clients: Vec::new(),
            changesets: None,
            config: None,
            customizations: None,
            input_needed: None,
            chats: vec![ChatSummary {
                resource: chat.clone(),
                title: "scripted chat".to_owned(),
                status: 0,
                activity: None,
                modified_at: String::new(),
                origin: None,
                interactivity: None,
                working_directories: None,
            }],
            default_chat: Some(chat),
            meta: None,
        })))
    }

    fn poll_session(&self, _session: SessionUri) -> himark::higent::SeatFuture<Vec<StateAction>> {
        Box::pin(std::future::pending())
    }

    fn subscribe_chat(
        &self,
        chat: ChatUri,
    ) -> himark::higent::SeatFuture<Result<ChatState, String>> {
        assert_eq!(chat, self.script.chat);
        let snapshot = self.script.snapshot.lock().expect("snapshot").clone();
        Box::pin(std::future::ready(Ok(snapshot)))
    }

    fn poll_chat(&self, chat: ChatUri) -> himark::higent::SeatFuture<Vec<StateAction>> {
        assert_eq!(chat, self.script.chat);
        let batches = Arc::clone(&self.script.batches);
        let parked = Arc::clone(&self.script.parked);
        Box::pin(std::future::poll_fn(move |cx| {
            // Register BEFORE checking: a feed between the check and
            // the park must not be missed.
            parked.lock().expect("parked").push(cx.waker().clone());
            match batches.lock().expect("batches").pop_front() {
                Some(batch) => std::task::Poll::Ready(batch),
                None => std::task::Poll::Pending,
            }
        }))
    }

    fn fetch_turns(
        &self,
        chat: ChatUri,
        cursor: Option<String>,
    ) -> himark::higent::SeatFuture<Result<himark::higent::TurnsPage, String>> {
        assert_eq!(chat, self.script.chat);
        let cursor = cursor.unwrap_or_default();
        let page = self.script.older.lock().expect("older").remove(&cursor);
        Box::pin(std::future::ready(
            page.ok_or_else(|| format!("no page at {cursor}")),
        ))
    }

    fn start_turn(
        &self,
        _chat: ChatUri,
        _text: String,
        _attachments: Option<Vec<himark::higent::ahp_types::state::MessageAttachment>>,
        _model: Option<himark::higent::ahp_types::state::ModelSelection>,
    ) -> himark::higent::SeatFuture<Result<(), String>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn cancel_turn(&self, _chat: ChatUri, _turn: TurnId) -> himark::higent::SeatFuture<()> {
        Box::pin(std::future::ready(()))
    }

    fn dispatch_action(
        &self,
        _channel: ChannelUri,
        _action: StateAction,
    ) -> himark::higent::SeatFuture<Result<(), String>> {
        Box::pin(std::future::ready(Ok(())))
    }

    unreached! {
        connect() -> himark::higent::SeatFuture<Result<himark::higent::RootInfo, String>>;
        list_sessions(cursor: Option<String>) -> himark::higent::SeatFuture<Result<himark::higent::SessionsPage, String>>;
        poll_root() -> himark::higent::SeatFuture<Vec<himark::higent::ServerEvent>>;
        create_session(dirs: Vec<String>, options: himark::higent::SessionOptions) -> himark::higent::SeatFuture<Result<SessionUri, String>>;
        resolve_session_config(working_directory: Option<String>, config: Option<serde_json::Map<String, serde_json::Value>>) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::commands::ResolveSessionConfigResult, String>>;
        dispose_session(session: SessionUri) -> himark::higent::SeatFuture<Result<(), String>>;
        create_chat(session: SessionUri) -> himark::higent::SeatFuture<Result<ChatUri, String>>;
        read_file_edit(before: Option<String>, after: Option<String>) -> himark::higent::SeatFuture<Result<himark::higent::FileEditContents, String>>;
        resource_read(session: SessionUri, uri: himark::higent::ResourceUri) -> himark::higent::SeatFuture<Option<String>>;
        resource_write(session: SessionUri, uri: himark::higent::ResourceUri, text: String) -> himark::higent::SeatFuture<bool>;
        resource_list(session: SessionUri, uri: himark::higent::ResourceUri) -> himark::higent::SeatFuture<Option<Vec<(String, bool)>>>;
        resource_watch(session: SessionUri, uri: himark::higent::ResourceUri, events: Arc<dyn Fn() + Send + Sync>) -> himark::higent::SeatFuture<Option<himark::higent::WatchHandle>>;
        resource_unwatch(handle: himark::higent::WatchHandle) -> himark::higent::SeatFuture<()>;
        search(session: SessionUri, ask: himark::higent::SearchAsk) -> himark::higent::SeatFuture<Option<himark::higent::SearchResult>>;
        terminal_input(channel: &ChannelUri, data: String) -> ();
        terminal_resize(channel: &ChannelUri, cols: u16, rows: u16) -> ();
        terminal_dispose(channel: &ChannelUri) -> ();
        subscribe_changeset(channel: ChannelUri) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::state::ChangesetState, String>>;
        poll_changeset(channel: ChannelUri) -> himark::higent::SeatFuture<Vec<StateAction>>;
        unsubscribe_changeset(channel: &ChannelUri) -> ();
        subscribe_annotations(session: SessionUri) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::state::AnnotationsState, String>>;
        poll_annotations(session: SessionUri) -> himark::higent::SeatFuture<Vec<StateAction>>;
        dispatch_annotations(session: &SessionUri, action: StateAction) -> ();
        unsubscribe_annotations(session: &SessionUri) -> ();
        open_document(session: SessionUri, uri: Option<himark::higent::ResourceUri>, text: Option<String>) -> himark::higent::SeatFuture<Result<himark::higent::seat::OpenDocumentResult, String>>;
        subscribe_document(channel: ChannelUri) -> himark::higent::SeatFuture<Result<himark::higent::seat::DocumentState, String>>;
        poll_document(channel: ChannelUri) -> himark::higent::SeatFuture<Vec<himark::higent::seat::DocumentApplied>>;
        dispatch_document(channel: &ChannelUri, action: himark::higent::seat::DocumentApplied) -> ();
        unsubscribe_document(channel: &ChannelUri) -> himark::higent::SeatFuture<()>;
        lsp(session: SessionUri, method: String, params: serde_json::Value) -> himark::higent::SeatFuture<Result<serde_json::Value, String>>;
    }

    fn terminal_open(
        &self,
        _session: SessionUri,
        _channel: ChannelUri,
        _cwd: Option<String>,
        _cols: u16,
        _rows: u16,
        _events: Arc<dyn Fn(himark::higent::TerminalEvent) + Send + Sync>,
    ) -> himark::higent::SeatFuture<Option<himark::higent::TerminalHandle>> {
        unreachable!("the chat flow never opens a terminal")
    }
}

fn message(text: &str) -> Message {
    Message {
        text: text.to_owned(),
        origin: MessageOrigin {
            kind: MessageKind::User,
        },
        attachments: None,
        model: None,
        agent: None,
        meta: None,
    }
}

fn completed_turn(id: &str, prompt: &str, reply: &str) -> Turn {
    Turn {
        id: id.to_owned(),
        started_at: None,
        duration: None,
        message: message(prompt),
        response_parts: vec![ResponsePart::Markdown(MarkdownResponsePart {
            id: format!("{id}-p1"),
            content: reply.to_owned(),
        })],
        usage: None,
        state: TurnState::Complete,
        error: None,
    }
}

fn chat_page(chat: &str, turns: Vec<Turn>, cursor: Option<&str>) -> ChatState {
    ChatState {
        resource: chat.to_owned(),
        title: "scripted chat".to_owned(),
        status: 0,
        activity: None,
        modified_at: String::new(),
        origin: None,
        interactivity: None,
        working_directories: None,
        turns,
        turns_next_cursor: cursor.map(str::to_owned),
        active_turn: None,
        steering_message: None,
        queued_messages: None,
        draft: None,
        meta: None,
    }
}

/// The LONG turn: a prompt, streamed markdown in many deltas, a tool
/// call started and completed, a closing paragraph.
fn long_turn_stream(turn: &str) -> Vec<Vec<StateAction>> {
    let mut batches = Vec::new();
    batches.push(vec![StateAction::ChatTurnStarted(ChatTurnStartedAction {
        turn_id: turn.to_owned(),
        started_at: String::new(),
        message: message("the long prompt"),
        queued_message_id: None,
        meta: None,
    })]);
    batches.push(vec![StateAction::ChatResponsePart(
        ChatResponsePartAction {
            turn_id: turn.to_owned(),
            part: ResponsePart::Markdown(MarkdownResponsePart {
                id: "p1".to_owned(),
                content: "chapter ".to_owned(),
            }),
            meta: None,
        },
    )]);
    for delta in 0..40 {
        batches.push(vec![StateAction::ChatDelta(ChatDeltaAction {
            turn_id: turn.to_owned(),
            part_id: "p1".to_owned(),
            content: format!("{delta} "),
            meta: None,
        })]);
    }
    batches.push(vec![StateAction::ChatToolCallStart(
        ChatToolCallStartAction {
            turn_id: turn.to_owned(),
            tool_call_id: "tool-1".to_owned(),
            tool_name: "bash".to_owned(),
            display_name: "Bash".to_owned(),
            intention: None,
            contributor: None,
            meta: None,
        },
    )]);
    batches.push(vec![StateAction::ChatToolCallComplete(
        ChatToolCallCompleteAction {
            turn_id: turn.to_owned(),
            tool_call_id: "tool-1".to_owned(),
            result: ToolCallResult {
                success: true,
                past_tense_message: himark::higent::ahp_types::common::StringOrMarkdown::Plain(
                    "ran it".to_owned(),
                ),
                content: None,
                structured_content: None,
                error: None,
            },
            requires_result_confirmation: None,
            meta: None,
        },
    )]);
    batches.push(vec![
        StateAction::ChatResponsePart(ChatResponsePartAction {
            turn_id: turn.to_owned(),
            part: ResponsePart::Markdown(MarkdownResponsePart {
                id: "p2".to_owned(),
                content: "the closing word".to_owned(),
            }),
            meta: None,
        }),
        StateAction::ChatTurnComplete(ChatTurnCompleteAction {
            turn_id: turn.to_owned(),
            duration: 1,
            meta: None,
        }),
    ]);
    batches
}


fn settle(engine: &mut HimarkEngine) {
    for _ in 0..4 {
        engine.worker().run_pending();
        engine.drain();
    }
}

fn settle_until(
    engine: &mut HimarkEngine,
    window: u64,
    what: &str,
    mut done: impl FnMut(&HimarkEngine) -> bool,
) {
    let started = std::time::Instant::now();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    while !done(engine) {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "never settled: {what}; transcript: {:?}",
            transcript(engine)
        );
        let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
        settle(engine);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

struct OpenScripted {
    host: himark::higent::HostId,
    session: SessionUri,
}

impl himark::DynamicCommand for OpenScripted {
    fn id(&self) -> &'static str {
        "test.open-scripted-session"
    }
    fn name(&self) -> String {
        "Open Scripted".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        store: &mut imba::store::Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        himark::higent::open_session(store, window, self.host, self.session.clone(), true, fx);
    }
}

fn chat_record(engine: &HimarkEngine) -> Option<himark::higent::ChatPanel> {
    himark::higent::Chats::chat(engine.app.store(), &ChatUri::new(CHAT))
}

fn transcript(engine: &HimarkEngine) -> Vec<(String, Vec<(String, String)>)> {
    chat_record(engine)
        .map(|chat| chat.transcript())
        .unwrap_or_default()
}

fn turn_text(rows: &[(String, Vec<(String, String)>)], turn: &str) -> String {
    rows.iter()
        .filter(|(id, _)| id == turn)
        .flat_map(|(_, cells)| cells.iter())
        .filter(|(kind, _)| kind != "Tool")
        .map(|(_, text)| text.clone())
        .collect()
}

fn script_host(engine: &HimarkEngine) -> himark::higent::HostId {
    chat_record(engine)
        .expect("the chat record")
        .session_id()
        .host
}

const CHAT: &str = "ahp-chat:/scripted";
const SESSION: &str = "ahp-session:/scripted";

fn boot(
    snapshot: ChatState,
) -> (HimarkEngine, u64, Script) {
    let mut engine = HimarkEngine::with_fonts(AppFonts::embedded());
    let window = engine.add_window();
    let script = Script::new(CHAT, SESSION, snapshot);
    let host = engine.register_agent_server(
        "scripted",
        Arc::new(ScriptedSeat {
            script: script.clone(),
        }),
    );
    assert!(engine.app.perform_batch(vec![himark::AppCommand::Dynamic(
        himark::WindowId::from_raw(window),
        Arc::new(OpenScripted {
            host,
            session: SessionUri::new(SESSION),
        }),
    )]));
    settle_until(&mut engine, window, "the scripted chat came up", |engine| {
        chat_record(engine).is_some_and(|chat| chat.ready())
    });
    (engine, window, script)
}

/// The whole streaming shape lands and reads back: the snapshot's
/// completed turns, then a LONG turn — a prompt, forty deltas, a
/// tool call, a closing part — assembled exactly once.
#[test]
fn a_scripted_stream_lands_whole() {
    let snapshot = chat_page(
        CHAT,
        vec![
            completed_turn("t-old-1", "first prompt", "first reply"),
            completed_turn("t-old-2", "second prompt", "second reply"),
        ],
        None,
    );
    let (mut engine, window, script) = boot(snapshot);
    let rows = transcript(&engine);
    assert_eq!(rows.len(), 2, "the snapshot's page landed: {rows:?}");

    for batch in long_turn_stream("t-long") {
        script.feed(batch);
    }
    settle_until(&mut engine, window, "the long turn closed", |engine| {
        turn_text(&transcript(engine), "t-long").contains("the closing word")
    });
    let rows = transcript(&engine);
    let text = turn_text(&rows, "t-long");
    for delta in 0..40 {
        assert!(
            text.contains(&format!("{delta} ")),
            "delta {delta} missing: {text}"
        );
    }
    let assembled: String = (0..40).map(|delta| format!("{delta} ")).collect();
    assert!(
        text.contains(&format!("chapter {assembled}")),
        "the deltas assembled in order, once: {text}"
    );
    let tools: Vec<&(String, String)> = rows
        .iter()
        .filter(|(id, _)| id == "t-long")
        .flat_map(|(_, cells)| cells.iter())
        .filter(|(kind, _)| kind == "Tool")
        .collect();
    assert_eq!(tools.len(), 1, "one tool cell: {tools:?}");
}

/// SEVERAL VIEWS: a second window over the same chat — every model
/// mutation reaches both, each view identical.
#[test]
fn two_windows_hold_one_conversation() {
    let (mut engine, window, script) = boot(chat_page(CHAT, Vec::new(), None));
    let second = engine.add_window();
    assert!(engine.app.perform_batch(vec![himark::AppCommand::Dynamic(
        himark::WindowId::from_raw(second),
        Arc::new(OpenScripted {
            host: script_host(&engine),
            session: SessionUri::new(SESSION),
        }),
    )]));
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(second, surface.canvas(), 1100.0, 800.0, 1.0);
    settle(&mut engine);

    for batch in long_turn_stream("t-live") {
        script.feed(batch);
    }
    settle_until(&mut engine, window, "the stream closed", |engine| {
        turn_text(&transcript(engine), "t-live").contains("the closing word")
    });
    let _ = engine.draw(second, surface.canvas(), 1100.0, 800.0, 1.0);
    settle(&mut engine);

    let views = chat_record(&engine).expect("the chat").view_transcripts();
    assert!(
        views.len() >= 2,
        "two mounts, two views: {}",
        views.len()
    );
    let text: Vec<String> = views
        .iter()
        .map(|rows| turn_text(rows, "t-live"))
        .collect();
    assert!(
        text.iter().all(|held| held == &text[0]),
        "every view holds the same conversation: {text:?}"
    );
    assert!(text[0].contains("the closing word"));
}

/// The reopen road MID-STREAM: the pane is displaced by a document
/// while the turn streams; the rest lands chat-scoped; ⌘I shows the
/// whole turn.
#[test]
fn a_displaced_pane_misses_nothing_mid_stream() {
    let (mut engine, window, script) = boot(chat_page(CHAT, Vec::new(), None));
    let batches = long_turn_stream("t-mid");
    let (head, tail) = batches.split_at(10);
    for batch in head {
        script.feed(batch.clone());
    }
    settle_until(&mut engine, window, "the head streamed", |engine| {
        !turn_text(&transcript(engine), "t-mid").is_empty()
    });

    assert!(engine.perform_command(window, "workbench.new-document"));
    settle(&mut engine);
    for batch in tail {
        script.feed(batch.clone());
    }
    settle_until(&mut engine, window, "the tail landed pane-less", |engine| {
        chat_record(engine).is_some_and(|chat| {
            chat.view_transcripts().is_empty()
                || turn_text(&chat.transcript(), "t-mid").contains("the closing word")
        })
    });

    assert!(engine.perform_command(window, "chat.composer"));
    settle_until(&mut engine, window, "the reopened pane holds it all", |engine| {
        turn_text(&transcript(engine), "t-mid").contains("the closing word")
    });
    let text = turn_text(&transcript(&engine), "t-mid");
    assert!(text.contains("chapter 0 1 2"), "the head survived: {text}");
}

/// A REPLAYED stream — the same batches delivered twice (an
/// overlapping subscription, a reconnect) — folds to ONE copy.
#[test]
fn a_replayed_stream_folds_to_one_copy() {
    let (mut engine, window, script) = boot(chat_page(CHAT, Vec::new(), None));
    for batch in long_turn_stream("t-replay") {
        script.feed(batch);
    }
    settle_until(&mut engine, window, "the stream closed", |engine| {
        turn_text(&transcript(engine), "t-replay").contains("the closing word")
    });
    let once = transcript(&engine);

    for batch in long_turn_stream("t-replay") {
        script.feed(batch);
    }
    settle_until(&mut engine, window, "the replay drained", |engine| {
        script.batches.lock().expect("batches").is_empty()
            && turn_text(&transcript(engine), "t-replay").contains("the closing word")
    });
    settle(&mut engine);

    let replayed = transcript(&engine);
    assert_eq!(
        turn_text(&replayed, "t-replay"),
        turn_text(&once, "t-replay"),
        "the replay folded to one copy"
    );
    assert_eq!(
        replayed.len(),
        once.len(),
        "no duplicate turns: {replayed:?}"
    );
}
