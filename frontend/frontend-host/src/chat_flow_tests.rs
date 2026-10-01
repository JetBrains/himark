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

use himark::higent::cell::Cell;
use himark::AppExt;

use crate::{AppFonts, HimarkEngine, HIMARK_KEY_ENTER, HIMARK_MOD_COMMAND};

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
    /// Every turn the app STARTED on the host, in order: the id the
    /// client minted and the prompt it carried.
    sent: Arc<Mutex<Vec<(String, String)>>>,
    /// A snapshot the host has not answered YET: a real subscribe is
    /// in flight for a while, and the user types into that window.
    held_snapshot: Arc<Mutex<bool>>,
    /// What the content uris of file edits resolve to.
    contents: Arc<Mutex<std::collections::HashMap<String, String>>>,
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
            sent: Arc::new(Mutex::new(Vec::new())),
            held_snapshot: Arc::new(Mutex::new(false)),
            contents: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Serve a content uri (both sides of a file edit come this way).
    fn serve(&self, uri: &str, text: String) {
        self.contents
            .lock()
            .expect("contents")
            .insert(uri.to_owned(), text);
    }

    /// Park the next subscribe: the snapshot lands only on `release`.
    fn hold_snapshot(&self) {
        *self.held_snapshot.lock().expect("held") = true;
    }

    fn release_snapshot(&self) {
        *self.held_snapshot.lock().expect("held") = false;
        for waker in self.parked.lock().expect("parked").drain(..) {
            waker.wake();
        }
    }

    fn sent(&self) -> Vec<(String, String)> {
        self.sent.lock().expect("sent").clone()
    }

    /// What a conforming host does with a dispatched turn: echo the
    /// very action back into the ordered stream.
    fn echo_send(&self) {
        let sent = self.sent();
        let Some((turn, text)) = sent.last().cloned() else {
            return;
        };
        self.feed(vec![StateAction::ChatTurnStarted(ChatTurnStartedAction {
            turn_id: turn,
            started_at: String::new(),
            message: message(&text),
            queued_message_id: None,
            meta: None,
        })]);
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
        let script = self.script.clone();
        Box::pin(std::future::poll_fn(move |cx| {
            if *script.held_snapshot.lock().expect("held") {
                script
                    .parked
                    .lock()
                    .expect("parked")
                    .push(cx.waker().clone());
                return std::task::Poll::Pending;
            }
            let snapshot = script.snapshot.lock().expect("snapshot").clone();
            std::task::Poll::Ready(Ok(snapshot))
        }))
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
        unreachable!("sending is write-ahead: the client dispatches chat/turnStarted")
    }

    fn cancel_turn(&self, _chat: ChatUri, _turn: TurnId) -> himark::higent::SeatFuture<()> {
        Box::pin(std::future::ready(()))
    }

    /// The SEND road: the client mints the turn and dispatches its own
    /// `chat/turnStarted`. The host records the prompt it was handed.
    fn dispatch_action(
        &self,
        _channel: ChannelUri,
        action: StateAction,
    ) -> himark::higent::SeatFuture<Result<(), String>> {
        if let StateAction::ChatTurnStarted(started) = &action {
            self.script
                .sent
                .lock()
                .expect("sent")
                .push((started.turn_id.clone(), started.message.text.clone()));
        }
        Box::pin(std::future::ready(Ok(())))
    }

    /// The content road behind a file edit: both sides of the edit,
    /// served by uri the way a host serves `ahp-content:` refs.
    fn read_file_edit(
        &self,
        before: Option<String>,
        after: Option<String>,
    ) -> himark::higent::SeatFuture<Result<himark::higent::FileEditContents, String>> {
        let contents = Arc::clone(&self.script.contents);
        let side = move |uri: Option<String>| -> Option<String> {
            let uri = uri?;
            contents.lock().expect("contents").get(&uri).cloned()
        };
        Box::pin(std::future::ready(Ok(himark::higent::FileEditContents {
            before: side(before),
            after: side(after),
        })))
    }

    unreached! {
        connect() -> himark::higent::SeatFuture<Result<himark::higent::RootInfo, String>>;
        list_sessions(cursor: Option<String>) -> himark::higent::SeatFuture<Result<himark::higent::SessionsPage, String>>;
        poll_root() -> himark::higent::SeatFuture<Vec<himark::higent::ServerEvent>>;
        create_session(dirs: Vec<String>, options: himark::higent::SessionOptions) -> himark::higent::SeatFuture<Result<SessionUri, String>>;
        resolve_session_config(working_directory: Option<String>, config: Option<serde_json::Map<String, serde_json::Value>>) -> himark::higent::SeatFuture<Result<himark::higent::ahp_types::commands::ResolveSessionConfigResult, String>>;
        dispose_session(session: SessionUri) -> himark::higent::SeatFuture<Result<(), String>>;
        create_chat(session: SessionUri) -> himark::higent::SeatFuture<Result<ChatUri, String>>;
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

/// The chats collection of the scripted session — the family of the
/// window that entered it (the first one; a second window of the same
/// session shares the collection).
fn chats_of_window(engine: &HimarkEngine) -> imba::store::Id<himark::higent::Chats> {
    let window = *engine.app.window_ids().first().expect("a window");
    himark::Windows::window_ref(engine.app.store(), window)
        .expect("the window entity")
        .family()
        .chats()
}

fn chat_record(engine: &HimarkEngine) -> Option<himark::higent::ChatPanel> {
    himark::higent::Chats::chat(
        engine.app.store(),
        chats_of_window(engine),
        &ChatUri::new(CHAT),
    )
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

fn boot(snapshot: ChatState) -> (HimarkEngine, u64, Script) {
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
    assert!(views.len() >= 2, "two mounts, two views: {}", views.len());
    let text: Vec<String> = views.iter().map(|rows| turn_text(rows, "t-live")).collect();
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
    settle_until(
        &mut engine,
        window,
        "the reopened pane holds it all",
        |engine| turn_text(&transcript(engine), "t-mid").contains("the closing word"),
    );
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

/// Everything the transcript says, in row order.
fn said(engine: &HimarkEngine) -> String {
    transcript(engine)
        .iter()
        .flat_map(|(_, cells)| cells.iter())
        .map(|(_, text)| text.clone())
        .collect::<Vec<_>>()
        .join("|")
}

fn paint(engine: &mut HimarkEngine, window: u64) {
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
    settle(engine);
}

/// THE USER'S OWN MESSAGE, typed and submitted through the real input
/// road: it must be on screen the moment it is sent — before the host
/// has said anything — and it must STILL be there after the poll, the
/// host's own record of the turn, and the reply.
#[test]
fn a_typed_message_is_on_screen_at_once_and_stays() {
    let (mut engine, window, script) = boot(chat_page(
        CHAT,
        vec![completed_turn("t-old", "older prompt", "older reply")],
        None,
    ));
    paint(&mut engine, window);

    assert!(
        engine.text_input(window, "my own message"),
        "the composer took the typing"
    );
    paint(&mut engine, window);
    // The composer sends on ⌘Enter; a bare Enter is a newline.
    assert!(
        engine.key_down(window, HIMARK_KEY_ENTER, HIMARK_MOD_COMMAND),
        "the composer took the submit"
    );
    paint(&mut engine, window);

    let sent = script.sent();
    assert_eq!(sent.len(), 1, "the host was handed one turn: {sent:?}");
    assert_eq!(sent[0].1, "my own message", "with the prompt we typed");
    let mine = sent[0].0.clone();
    assert!(
        said(&engine).contains("my own message"),
        "AT ONCE on screen: {:?}",
        transcript(&engine)
    );

    // The host echoes the action we dispatched — same turn id — and
    // streams its reply into that turn.
    script.echo_send();
    script.feed(vec![StateAction::ChatResponsePart(
        ChatResponsePartAction {
            turn_id: mine.clone(),
            part: ResponsePart::Markdown(MarkdownResponsePart {
                id: "p1".to_owned(),
                content: "the answer".to_owned(),
            }),
            meta: None,
        },
    )]);
    script.feed(vec![StateAction::ChatTurnComplete(
        ChatTurnCompleteAction {
            turn_id: mine,
            duration: 1,
            meta: None,
        },
    )]);
    settle_until(&mut engine, window, "the reply landed", |engine| {
        said(engine).contains("the answer")
    });
    paint(&mut engine, window);

    let said = said(&engine);
    assert!(
        said.contains("my own message"),
        "the message SURVIVED the wire: {said}"
    );
    assert_eq!(
        said.matches("my own message").count(),
        1,
        "and it is there exactly once: {said}"
    );
}

/// The STOP button: with a turn in flight the click must reach the
/// model and cancel THAT turn.
#[test]
fn the_stop_button_cancels_the_turn_in_flight() {
    let (mut engine, window, script) = boot(chat_page(CHAT, Vec::new(), None));
    paint(&mut engine, window);
    script.feed(vec![StateAction::ChatTurnStarted(ChatTurnStartedAction {
        turn_id: "t-live".to_owned(),
        started_at: String::new(),
        message: message("a running prompt"),
        queued_message_id: None,
        meta: None,
    })]);
    settle_until(&mut engine, window, "the turn is in flight", |engine| {
        chat_record(engine).is_some_and(|chat| chat.cancel_target().is_some())
    });

    assert_eq!(
        chat_record(&engine)
            .expect("the chat")
            .cancel_target()
            .map(|turn| turn.as_str().to_owned()),
        Some("t-live".to_owned())
    );
}

/// THE STREAMING CONTRACT, in structure rather than stopwatch: a cell
/// is a document — a text, a parse, a layout. A PART mounts exactly
/// one; a DELTA mounts nothing and builds nothing, on any thread; a
/// turn is never laid twice.
#[test]
fn a_long_stream_costs_a_cell_per_part_and_never_a_turn() {
    let (mut engine, window, script) = boot(chat_page(
        CHAT,
        (0..8)
            .map(|at| completed_turn(&format!("t{at}"), &format!("prompt {at}"), "a reply"))
            .collect(),
        None,
    ));
    paint(&mut engine, window);

    script.feed(vec![StateAction::ChatTurnStarted(ChatTurnStartedAction {
        turn_id: "t-long".to_owned(),
        started_at: String::new(),
        message: message("the long prompt"),
        queued_message_id: None,
        meta: None,
    })]);
    settle_until(&mut engine, window, "the turn opened", |engine| {
        !turn_text(&transcript(engine), "t-long").is_empty()
    });
    paint(&mut engine, window);

    // A PART mints its cell: one mount, once.
    let mounted = Cell::documents_mounted();
    script.feed(vec![StateAction::ChatResponsePart(
        ChatResponsePartAction {
            turn_id: "t-long".to_owned(),
            part: ResponsePart::Markdown(MarkdownResponsePart {
                id: "p1".to_owned(),
                content: "first words".to_owned(),
            }),
            meta: None,
        },
    )]);
    settle_until(&mut engine, window, "the part mounted", |engine| {
        let _ = engine;
        Cell::documents_mounted() > mounted
    });
    paint(&mut engine, window);
    assert_eq!(
        Cell::documents_mounted() - mounted,
        1,
        "a part mounts ONE cell, not a turn's worth"
    );

    // …and then HUNDREDS of deltas grow it: no mount, no document born
    // anywhere (the background runner runs on this thread here too).
    let mounted = Cell::documents_mounted();
    let born = himark::Document::born_on_this_thread();
    for step in 0..300 {
        script.feed(vec![StateAction::ChatDelta(ChatDeltaAction {
            turn_id: "t-long".to_owned(),
            part_id: "p1".to_owned(),
            content: format!(" {step}"),
            meta: None,
        })]);
        settle_until(&mut engine, window, "the delta landed", |engine| {
            turn_text(&transcript(engine), "t-long").contains(&format!(" {step}"))
        });
    }
    paint(&mut engine, window);
    assert_eq!(
        Cell::documents_mounted() - mounted,
        0,
        "300 deltas re-mounted a cell — a delta is an APPEND"
    );
    assert_eq!(
        himark::Document::born_on_this_thread() - born,
        0,
        "300 deltas built a document — a delta is an APPEND"
    );
}

/// Walking away from a chat and back (⌘I, a file, ⌘I) lays nothing:
/// the mount is furniture the model keeps, parked by the pane that
/// leaves and claimed by the pane that comes back.
#[test]
fn walking_back_to_a_chat_rebuilds_nothing() {
    let (mut engine, window, _script) = boot(chat_page(
        CHAT,
        (0..8)
            .map(|at| completed_turn(&format!("t{at}"), &format!("prompt {at}"), "a reply"))
            .collect(),
        None,
    ));
    paint(&mut engine, window);
    assert_eq!(transcript(&engine).len(), 8, "the page is laid");

    let mounted = Cell::documents_mounted();
    let born = himark::Document::born_on_this_thread();
    let chat = ChatUri::new(CHAT);
    let chats = chats_of_window(&engine);
    for _ in 0..3 {
        let pane = himark::higent::ChatPane::new(chats, chat.clone());
        assert!(engine.app.open_panel(window_id(window), Box::new(pane)));
        paint(&mut engine, window);
    }
    assert_eq!(transcript(&engine).len(), 8, "the same page stands");
    assert_eq!(
        Cell::documents_mounted() - mounted,
        0,
        "walking back re-mounted cells — the mount is kept, not re-laid"
    );
    assert_eq!(
        himark::Document::born_on_this_thread() - born,
        0,
        "walking back built documents — the mount is kept, not re-laid"
    );
}

fn window_id(window: u64) -> himark::WindowId {
    himark::WindowId::from_raw(window)
}

/// HUNDREDS of file edits — the shape a real coding turn takes. Every
/// completed edit tool call mounts a DIFF cell: two side documents with
/// their syntax, a diff, prepared marks. That is built on the
/// background runner and NEVER on the UI thread; the frame only lays
/// two editors. The runner is driven by hand here, so the thread rule
/// is checked by counting documents born across each `drain`.
#[test]
fn a_turn_of_hundreds_of_edits_never_stalls_a_frame() {
    const EDITS: usize = 150;

    let (mut engine, window, script) = boot(chat_page(
        CHAT,
        vec![completed_turn("t-old", "older prompt", "older reply")],
        None,
    ));
    paint(&mut engine, window);

    for step in 0..EDITS {
        script.serve(
            &format!("ahp-content:/before-{step}"),
            edited_source(step, false),
        );
        script.serve(
            &format!("ahp-content:/after-{step}"),
            edited_source(step, true),
        );
    }

    script.feed(vec![StateAction::ChatTurnStarted(ChatTurnStartedAction {
        turn_id: "t-edits".to_owned(),
        started_at: String::new(),
        message: message("edit the whole tree"),
        queued_message_id: None,
        meta: None,
    })]);
    settle_until(&mut engine, window, "the turn opened", |engine| {
        !turn_text(&transcript(engine), "t-edits").is_empty()
    });
    paint(&mut engine, window);

    let mounted = Cell::documents_mounted();
    let born = himark::Document::born_on_this_thread();
    // Two clocks: `draw` + `drain` is a FRAME — painting plus the
    // landings the UI thread absorbs. `run_pending` is the background
    // runner, another thread in production.
    let mut frames: Vec<std::time::Duration> = Vec::new();
    let mut background: Vec<std::time::Duration> = Vec::new();
    let mut surface = skia_safe::surfaces::raster_n32_premul((1100, 800)).expect("surface");
    for step in 0..EDITS {
        script.feed(edit_tool_call("t-edits", step));
        let landed = format!("[diff file{step}.rs +");
        let started = std::time::Instant::now();
        loop {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(30),
                "edit {step} never landed"
            );
            let work = std::time::Instant::now();
            engine.worker().run_pending();
            background.push(work.elapsed());

            let born_before = himark::Document::born_on_this_thread();
            let frame = std::time::Instant::now();
            let _ = engine.draw(window, surface.canvas(), 1100.0, 800.0, 1.0);
            engine.drain();
            frames.push(frame.elapsed());
            assert_eq!(
                himark::Document::born_on_this_thread(),
                born_before,
                "edit {step}: a document was built on the UI THREAD"
            );

            if transcript(&engine)
                .iter()
                .filter(|(id, _)| id == "t-edits")
                .flat_map(|(_, cells)| cells.iter())
                .any(|(_, text)| text.contains(&landed))
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    assert_eq!(
        Cell::documents_mounted() - mounted,
        EDITS as u64,
        "{EDITS} edits, each mounted once — never a turn re-laid"
    );
    assert_eq!(
        himark::Document::born_on_this_thread() - born,
        2 * EDITS as u64,
        "{EDITS} edits, two sides each, built once on the runner"
    );

    let ms = |samples: &mut Vec<std::time::Duration>| -> (f64, f64, f64) {
        samples.sort();
        (
            samples[samples.len() / 2].as_secs_f64() * 1000.0,
            samples[samples.len() * 95 / 100].as_secs_f64() * 1000.0,
            samples[samples.len() - 1].as_secs_f64() * 1000.0,
        )
    };
    let (p50, p95, worst) = ms(&mut frames);
    let background_total = background.iter().sum::<std::time::Duration>().as_secs_f64() * 1000.0;
    eprintln!(
        "chat-edits: edits={EDITS}, frames={}, frame p50={p50:.3}ms p95={p95:.3}ms max={worst:.3}ms, background total={background_total:.1}ms",
        frames.len()
    );
    imba::perf::record("chat-edits", "frame_p50_ms", p50);
    imba::perf::record("chat-edits", "frame_p95_ms", p95);
    imba::perf::record("chat-edits", "frame_max_ms", worst);
    imba::perf::record("chat-edits", "background_total_ms", background_total);
}

/// One edit tool call, started and completed in a batch, its result a
/// file edit whose sides the scripted seat serves.
fn edit_tool_call(turn: &str, step: usize) -> Vec<StateAction> {
    let tool = format!("edit-{step}");
    vec![
        StateAction::ChatToolCallStart(ChatToolCallStartAction {
            turn_id: turn.to_owned(),
            tool_call_id: tool.clone(),
            tool_name: "edit".to_owned(),
            display_name: "Edit".to_owned(),
            intention: None,
            contributor: None,
            meta: None,
        }),
        StateAction::ChatToolCallComplete(ChatToolCallCompleteAction {
            turn_id: turn.to_owned(),
            tool_call_id: tool,
            result: ToolCallResult {
                success: true,
                past_tense_message: himark::higent::ahp_types::common::StringOrMarkdown::Plain(
                    format!("edited file{step}.rs"),
                ),
                content: Some(vec![
                    himark::higent::ahp_types::state::ToolResultContent::FileEdit(
                        himark::higent::FileEditRefs {
                            before: Some(himark::higent::snapshot(
                                &format!("src/file{step}.rs"),
                                &format!("ahp-content:/before-{step}"),
                            )),
                            after: Some(himark::higent::snapshot(
                                &format!("src/file{step}.rs"),
                                &format!("ahp-content:/after-{step}"),
                            )),
                            counts: himark::higent::DiffCounts {
                                added: Some(1),
                                removed: Some(1),
                            },
                        }
                        .to_content(),
                    ),
                ]),
                structured_content: None,
                error: None,
            },
            requires_result_confirmation: None,
            meta: None,
        }),
    ]
}

/// A file's two sides: same shape, one line apart — enough source for a
/// real parse and a real diff.
fn edited_source(step: usize, after: bool) -> String {
    let mut text = String::new();
    for line in 0..400 {
        if line == 7 && after {
            text.push_str(&format!("    let answer = {step} + 1; // edited\n"));
            continue;
        }
        text.push_str(&format!("    let value{line} = {line} * {step};\n"));
    }
    format!("fn file{step}() {{\n{text}}}\n")
}
