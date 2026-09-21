// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use ahp_types::actions::{
    ChatDeltaAction, ChatReasoningAction, ChatResponsePartAction, ChatToolCallCompleteAction,
    ChatToolCallReadyAction, ChatToolCallStartAction, ChatTurnCancelledAction,
    ChatTurnCompleteAction, ChatTurnStartedAction, ChatUsageAction, StateAction,
};
use ahp_types::state::{
    ConfirmationOption, ConfirmationOptionKind, MarkdownResponsePart, Message, MessageKind,
    MessageOrigin, ReasoningResponsePart, ResponsePart, ToolCallResult, ToolInput,
    ToolResultContent, ToolResultTextContent, UsageInfo,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub trait AgentSink: Send + Sync {
    fn action(&self, action: StateAction);

    fn stash(&self, text: String) -> String;
}

pub type Sink = Arc<dyn AgentSink>;

pub fn discover_binary() -> String {
    if let Ok(binary) = std::env::var("HIMARK_CLAUDE_BIN") {
        return binary;
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("claude");
            if is_executable(&candidate) {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    for known in [
        format!("{home}/.local/bin/claude"),
        format!("{home}/.claude/local/claude"),
        "/opt/homebrew/bin/claude".to_owned(),
        "/usr/local/bin/claude".to_owned(),
    ] {
        if is_executable(std::path::Path::new(&known)) {
            return known;
        }
    }

    "claude".to_owned()
}

pub(crate) fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

pub struct SpawnConfig {
    pub binary: String,
    pub cwd: std::path::PathBuf,

    pub add_dirs: Vec<std::path::PathBuf>,

    pub native_id: String,
    pub resume: bool,

    pub model: Option<String>,

    pub permission_mode: Option<String>,

    pub thinking_level: Option<String>,
}

pub struct ClaudeAgent {
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    state: Mutex<TurnState>,
    sink: Sink,
    child: Mutex<Option<tokio::process::Child>>,

    stderr_tail: Mutex<std::collections::VecDeque<String>>,

    cwd: std::path::PathBuf,

    dead: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct TurnState {
    turns: std::collections::VecDeque<PendingTurn>,

    parts: HashMap<u64, (String, PartKind)>,

    tool_blocks: HashMap<u64, String>,

    tools: HashMap<String, ToolTrack>,

    asks: HashMap<String, String>,
    minted_parts: u64,

    wake_prompt: Option<String>,
}

struct PendingTurn {
    id: String,
    cancelled: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum PartKind {
    Text,
    Thinking,
}

struct ToolTrack {
    name: String,
    input: Value,

    partial_input: String,
    started: bool,
    ready: bool,

    edited_path: Option<std::path::PathBuf>,
    before: Option<String>,
}

impl ClaudeAgent {
    pub async fn spawn(config: SpawnConfig, sink: Sink) -> Result<Arc<Self>, String> {
        let mut words = config.binary.split_whitespace();
        let program = words.next().unwrap_or("claude").to_owned();
        let mut command = tokio::process::Command::new(&program);
        command.args(words);
        command
            .arg("--print")
            .args(["--input-format", "stream-json"])
            .args(["--output-format", "stream-json"])
            .arg("--verbose")
            .arg("--include-partial-messages")
            .args(["--permission-prompt-tool", "stdio"])
            .args([
                "--permission-mode",
                config
                    .permission_mode
                    .as_deref()
                    .unwrap_or("bypassPermissions"),
            ])
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(model) = &config.model {
            command.args(["--model", model]);
        }

        for dir in &config.add_dirs {
            command.arg("--add-dir").arg(dir);
        }

        if let Some(level) = &config.thinking_level {
            let tokens = match level.as_str() {
                "low" => "4096",
                "medium" => "12288",
                "high" => "24576",
                _ => "31999",
            };
            command.env("MAX_THINKING_TOKENS", tokens);
        }
        if config.resume {
            command.arg(format!("--resume={}", config.native_id));
        } else {
            command.args(["--session-id", &config.native_id]);
        }
        let mut child = command.spawn().map_err(|error| {
            format!(
                "failed to spawn `{}`: {error} — install the claude CLI, or point \
                 HIMARK_CLAUDE_BIN at it",
                config.binary
            )
        })?;
        let stdin = child.stdin.take().ok_or("agent stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("agent stdout unavailable")?;
        let stderr = child.stderr.take();
        let agent = Arc::new(Self {
            stdin: Mutex::new(Some(stdin)),
            state: Mutex::new(TurnState::default()),
            sink,
            child: Mutex::new(Some(child)),
            cwd: config.cwd.clone(),
            dead: std::sync::atomic::AtomicBool::new(false),
            stderr_tail: Mutex::new(std::collections::VecDeque::new()),
        });
        if let Some(stderr) = stderr {
            let tailer = Arc::clone(&agent);
            let trace = std::env::var("HIHOST_TRACE").is_ok();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if trace {
                        eprintln!("[claude!] {line}");
                    }
                    let mut tail = tailer.stderr_tail.lock().expect("stderr tail");
                    if tail.len() >= 8 {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            });
        }
        let pump = Arc::clone(&agent);
        let trace = std::env::var("HIHOST_TRACE").is_ok();
        tokio::spawn(async move {
            if trace {
                eprintln!("[claude<-] pump alive");
            }
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if trace {
                    eprintln!("[claude<-] {}", &line[..line.len().min(120)]);
                }
                if let Ok(event) = serde_json::from_str::<Value>(&line) {
                    pump.event(event).await;
                }
            }
            if trace {
                eprintln!("[claude<-] EOF");
            }
            pump.stream_ended();
        });
        Ok(agent)
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn prompt(&self, turn_id: String, text: String) -> Result<(), String> {
        if self.is_dead() {
            return Err("the claude process is gone".to_owned());
        }
        // The PendingTurn must be on the queue before the process can react to
        // the prompt, or the reply's message_start would look agent-initiated
        // and get adopted as a wake turn.
        {
            let mut state = self.state.lock().expect("turn state");
            state.turns.push_back(PendingTurn {
                id: turn_id.clone(),
                cancelled: false,
            });
        }
        let sent = self
            .send(json!({
                "type": "user",
                "message": {"role": "user", "content": [{"type": "text", "text": text}]},
            }))
            .await;
        if sent.is_err() || self.is_dead() {
            let mut state = self.state.lock().expect("turn state");
            state.turns.retain(|turn| turn.id != turn_id);
            sent?;
            return Err("the claude process died at the prompt".to_owned());
        }
        Ok(())
    }

    pub async fn answer(&self, tool_call_id: &str, approved: bool) -> Result<(), String> {
        let (request_id, input) = {
            let mut state = self.state.lock().expect("turn state");
            let Some(request_id) = state.asks.remove(tool_call_id) else {
                return Ok(());
            };
            let input = state
                .tools
                .get(tool_call_id)
                .map(|track| track.input.clone())
                .unwrap_or(Value::Null);
            (request_id, input)
        };
        let response = if approved {
            json!({"behavior": "allow", "updatedInput": input})
        } else {
            json!({"behavior": "deny", "message": "The user declined."})
        };
        self.send(json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": request_id, "response": response},
        }))
        .await
    }

    pub async fn interrupt(&self) -> Result<(), String> {
        {
            let mut state = self.state.lock().expect("turn state");
            if let Some(front) = state.turns.front_mut() {
                front.cancelled = true;
            }

            state.asks.clear();
        }
        self.send(json!({
            "type": "control_request",
            "request_id": format!("himark-int-{}", std::process::id()),
            "request": {"subtype": "interrupt"},
        }))
        .await
    }

    pub fn idle(&self) -> bool {
        self.state.lock().expect("turn state").turns.is_empty()
    }

    pub fn shutdown(&self) {
        *self.stdin.lock().expect("agent stdin") = None;
        if let Some(mut child) = self.child.lock().expect("agent child").take() {
            let _ = child.start_kill();
        }
    }

    async fn send(&self, message: Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        let stdin = {
            let mut slot = self.stdin.lock().expect("agent stdin");
            slot.take()
        };
        let Some(mut stdin) = stdin else {
            return Err("the agent process is gone".to_owned());
        };
        let outcome = stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| format!("agent stdin: {error}"));
        if std::env::var("HIHOST_TRACE").is_ok() {
            eprintln!(
                "[claude->] {:?} sent={}",
                outcome,
                &line[..line.len().min(100)]
            );
        }
        *self.stdin.lock().expect("agent stdin") = Some(stdin);
        outcome
    }

    fn emit(&self, action: StateAction) {
        self.sink.action(action);
    }

    fn turn_id(&self) -> Option<String> {
        self.state
            .lock()
            .expect("turn state")
            .turns
            .front()
            .map(|turn| turn.id.clone())
    }

    fn stream_ended(&self) {
        self.dead.store(true, std::sync::atomic::Ordering::SeqCst);
        if std::env::var("HIHOST_TRACE").is_ok() {
            let status = self
                .child
                .lock()
                .expect("agent child")
                .as_mut()
                .and_then(|child| child.try_wait().ok().flatten());
            eprintln!("[claude<-] stream ended; exit: {status:?}");
        }
        let turns: Vec<PendingTurn> = {
            let mut state = self.state.lock().expect("turn state");
            state.turns.drain(..).collect()
        };
        let tail: Vec<String> = self
            .stderr_tail
            .lock()
            .expect("stderr tail")
            .iter()
            .cloned()
            .collect();
        let message = match tail.is_empty() {
            true => "the claude process ended mid-turn".to_owned(),
            false => format!("the claude process ended mid-turn: {}", tail.join(" | ")),
        };
        for PendingTurn { id: turn_id, .. } in turns {
            self.emit(StateAction::ChatError(
                ahp_types::actions::ChatErrorAction {
                    turn_id,
                    error: ahp_types::state::ErrorInfo {
                        error_type: "agentGone".to_owned(),
                        message: message.clone(),
                        stack: None,
                        meta: None,
                    },
                    duration: 0,
                    meta: None,
                },
            ));
        }
    }

    async fn event(&self, event: Value) {
        match event.get("type").and_then(Value::as_str) {
            Some("stream_event") => {
                self.adopt_wake_turn(&event);
                self.stream_event(&event["event"])
            }
            Some("assistant") => self.assistant_snapshot(&event["message"]),
            Some("user") => self.tool_results(&event["message"]),
            Some("control_request") => self.control_request(&event).await,
            Some("result") => self.result(&event),

            _ => {}
        }
    }

    /// The CLI starts turns of its own — a fired ScheduleWakeup, a background
    /// task notification. Those carry no himark-issued PendingTurn, so every
    /// handler would drop their events. Adopt them: mint a turn and announce it,
    /// so the turn renders and `result` pops a matching entry.
    fn adopt_wake_turn(&self, outer: &Value) {
        if outer["event"]["type"].as_str() != Some("message_start")
            || !outer["parent_tool_use_id"].is_null()
        {
            return;
        }
        let (turn_id, prompt) = {
            let mut state = self.state.lock().expect("turn state");
            if !state.turns.is_empty() {
                return;
            }
            let turn_id = format!("hihost-wake-{}", crate::uuid_v4());
            state.turns.push_back(PendingTurn {
                id: turn_id.clone(),
                cancelled: false,
            });
            (turn_id, state.wake_prompt.take())
        };
        self.emit(StateAction::ChatTurnStarted(ChatTurnStartedAction {
            turn_id,
            started_at: humantime::format_rfc3339_millis(std::time::SystemTime::now()).to_string(),
            message: Message {
                text: prompt.unwrap_or_else(|| "Scheduled wake-up".to_owned()),
                origin: MessageOrigin {
                    kind: MessageKind::SystemNotification,
                },
                attachments: None,
                model: None,
                agent: None,
                meta: None,
            },
            queued_message_id: None,
            meta: None,
        }));
    }

    fn stream_event(&self, event: &Value) {
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                let index = event["index"].as_u64().unwrap_or(0);
                let block = &event["content_block"];
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => self.open_part(&turn_id, index, PartKind::Text),
                    Some("thinking") => self.open_part(&turn_id, index, PartKind::Thinking),
                    Some("tool_use") => {
                        let id = block["id"].as_str().unwrap_or_default().to_owned();
                        let name = block["name"].as_str().unwrap_or_default().to_owned();
                        let mut state = self.state.lock().expect("turn state");
                        state.tool_blocks.insert(index, id.clone());
                        state.tools.entry(id).or_insert(ToolTrack {
                            name,
                            input: Value::Null,
                            partial_input: String::new(),
                            started: false,
                            ready: false,
                            edited_path: None,
                            before: None,
                        });
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let index = event["index"].as_u64().unwrap_or(0);
                let mut state = self.state.lock().expect("turn state");
                if let Some(id) = state.tool_blocks.get(&index).cloned() {
                    let cwd = self.cwd.clone();
                    if let Some(track) = state.tools.get_mut(&id) {
                        if track.input.is_null() && !track.partial_input.is_empty() {
                            if let Ok(parsed) = serde_json::from_str::<Value>(&track.partial_input)
                            {
                                track.input = parsed;
                            }
                        }
                        capture_before(track, &cwd);
                    }
                }
            }
            Some("content_block_delta") => {
                let index = event["index"].as_u64().unwrap_or(0);
                let delta = &event["delta"];
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => self.append_delta(
                        &turn_id,
                        index,
                        delta["text"].as_str().unwrap_or_default(),
                    ),
                    Some("thinking_delta") => self.append_delta(
                        &turn_id,
                        index,
                        delta["thinking"].as_str().unwrap_or_default(),
                    ),
                    Some("input_json_delta") => {
                        let mut state = self.state.lock().expect("turn state");
                        if let Some(id) = state.tool_blocks.get(&index).cloned() {
                            if let Some(track) = state.tools.get_mut(&id) {
                                track
                                    .partial_input
                                    .push_str(delta["partial_json"].as_str().unwrap_or_default());
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn open_part(&self, turn_id: &str, index: u64, kind: PartKind) {
        let id = {
            let mut state = self.state.lock().expect("turn state");
            state.minted_parts += 1;
            let id = format!("{turn_id}-p{}", state.minted_parts);
            state.parts.insert(index, (id.clone(), kind));
            id
        };
        let part = match kind {
            PartKind::Text => ResponsePart::Markdown(MarkdownResponsePart {
                id,
                content: String::new(),
            }),
            PartKind::Thinking => ResponsePart::Reasoning(ReasoningResponsePart {
                id,
                content: String::new(),
            }),
        };
        self.emit(StateAction::ChatResponsePart(ChatResponsePartAction {
            turn_id: turn_id.to_owned(),
            part,
            meta: None,
        }));
    }

    fn append_delta(&self, turn_id: &str, index: u64, content: &str) {
        if content.is_empty() {
            return;
        }
        let slot = {
            let state = self.state.lock().expect("turn state");
            state.parts.get(&index).cloned()
        };
        let Some((part_id, kind)) = slot else { return };
        match kind {
            PartKind::Text => self.emit(StateAction::ChatDelta(ChatDeltaAction {
                turn_id: turn_id.to_owned(),
                part_id,
                content: content.to_owned(),
                meta: None,
            })),
            PartKind::Thinking => self.emit(StateAction::ChatReasoning(ChatReasoningAction {
                turn_id: turn_id.to_owned(),
                part_id,
                content: content.to_owned(),
                meta: None,
            })),
        }
    }

    fn assistant_snapshot(&self, message: &Value) {
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        let Some(blocks) = message["content"].as_array() else {
            return;
        };
        for block in blocks {
            if block["type"].as_str() != Some("tool_use") {
                continue;
            }
            let id = block["id"].as_str().unwrap_or_default().to_owned();
            let name = block["name"].as_str().unwrap_or_default().to_owned();
            let input = block["input"].clone();
            let fresh = {
                let mut state = self.state.lock().expect("turn state");
                let track = state.tools.entry(id.clone()).or_insert(ToolTrack {
                    name: name.clone(),
                    input: Value::Null,
                    partial_input: String::new(),
                    started: false,
                    ready: false,
                    edited_path: None,
                    before: None,
                });
                track.input = input;
                capture_before(track, &self.cwd);
                let fresh = !track.started;
                track.started = true;
                fresh
            };
            if fresh {
                self.emit(StateAction::ChatToolCallStart(ChatToolCallStartAction {
                    turn_id: turn_id.clone(),
                    tool_call_id: id,
                    tool_name: name.clone(),
                    display_name: name,
                    intention: None,
                    contributor: None,
                    meta: None,
                }));
            }
        }
    }

    async fn control_request(&self, event: &Value) {
        let request = &event["request"];
        if request["subtype"].as_str() != Some("can_use_tool") {
            let id = event["request_id"].clone();
            let _ = self
                .send(json!({
                    "type": "control_response",
                    "response": {"subtype": "error", "request_id": id,
                                  "error": "unsupported request"},
                }))
                .await;
            return;
        }
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        let tool_use_id = request["tool_use_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let tool_name = request["tool_name"].as_str().unwrap_or_default().to_owned();
        let display = request["display_name"]
            .as_str()
            .unwrap_or(&tool_name)
            .to_owned();
        let description = request["description"].as_str().unwrap_or_default();
        let input = request["input"].clone();
        {
            let mut state = self.state.lock().expect("turn state");
            let request_id = event["request_id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_default();
            state.asks.insert(tool_use_id.clone(), request_id);
            let track = state.tools.entry(tool_use_id.clone()).or_insert(ToolTrack {
                name: tool_name.clone(),
                input: Value::Null,
                partial_input: String::new(),
                started: false,
                ready: false,
                edited_path: None,
                before: None,
            });
            track.input = input.clone();
            capture_before(track, &self.cwd);
            track.ready = true;
        }
        let invocation = if description.is_empty() {
            display.clone()
        } else {
            description.to_owned()
        };
        self.emit(StateAction::ChatToolCallReady(ChatToolCallReadyAction {
            turn_id,
            tool_call_id: tool_use_id,
            contributor: None,
            intention: None,
            invocation_message: invocation.into(),
            tool_input: Some(ToolInput::Inline(preview(&input))),
            confirmation_title: Some(display.into()),
            risk_assessment: None,
            edits: None,
            editable: None,
            confirmed: None,
            options: Some(vec![
                ConfirmationOption {
                    id: "allow".to_owned(),
                    label: "Yes".to_owned(),
                    kind: ConfirmationOptionKind::Approve,
                    group: None,
                },
                ConfirmationOption {
                    id: "deny".to_owned(),
                    label: "No, tell Claude what to do differently".to_owned(),
                    kind: ConfirmationOptionKind::Deny,
                    group: None,
                },
            ]),
            meta: None,
        }));
    }

    fn tool_results(&self, message: &Value) {
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        let Some(blocks) = message["content"].as_array() else {
            return;
        };
        for block in blocks {
            if block["type"].as_str() != Some("tool_result") {
                continue;
            }
            let id = block["tool_use_id"].as_str().unwrap_or_default().to_owned();
            let is_error = block["is_error"].as_bool().unwrap_or(false);
            let content = result_text(&block["content"]);
            let edit = {
                let mut state = self.state.lock().expect("turn state");
                state.tools.get_mut(&id).and_then(|track| {
                    let path = track.edited_path.take()?;
                    Some((path, track.before.take()))
                })
            };
            let file_edit = (!is_error)
                .then(|| edit)
                .flatten()
                .map(|(path, before)| self.file_edit_content(&path, before));
            let name = {
                let mut state = self.state.lock().expect("turn state");
                let track = state.tools.get_mut(&id);
                let name = track
                    .as_ref()
                    .map(|track| track.name.clone())
                    .unwrap_or_default();

                let call = track
                    .as_ref()
                    .filter(|track| !track.input.is_null())
                    .map(|track| preview(&track.input))
                    .filter(|call| !call.is_empty());

                if let Some(track) = track {
                    if !track.ready {
                        track.ready = true;
                        drop(state);
                        self.emit(StateAction::ChatToolCallReady(ChatToolCallReadyAction {
                            turn_id: turn_id.clone(),
                            tool_call_id: id.clone(),
                            contributor: None,
                            intention: None,
                            invocation_message: call.clone().unwrap_or_else(|| name.clone()).into(),
                            tool_input: call.map(ToolInput::Inline),
                            confirmation_title: None,
                            risk_assessment: None,
                            edits: None,
                            editable: None,
                            confirmed: Some(
                                ahp_types::state::ToolCallConfirmationReason::NotNeeded,
                            ),
                            options: None,
                            meta: None,
                        }));
                    }
                }
                name
            };
            if name == "ScheduleWakeup" && !is_error {
                let mut state = self.state.lock().expect("turn state");
                let input = state
                    .tools
                    .get(&id)
                    .map(|track| track.input.clone())
                    .unwrap_or(Value::Null);
                state.wake_prompt = match input["stop"].as_bool() == Some(true) {
                    true => None,
                    false => input["prompt"].as_str().map(str::to_owned),
                };
            }
            self.emit(StateAction::ChatToolCallComplete(
                ChatToolCallCompleteAction {
                    turn_id: turn_id.clone(),
                    tool_call_id: id,
                    result: ToolCallResult {
                        success: !is_error,
                        past_tense_message: format!("Ran {name}").into(),
                        content: Some({
                            let mut parts = vec![ToolResultContent::Text(ToolResultTextContent {
                                text: content.clone(),
                            })];
                            parts.extend(file_edit.clone());
                            parts
                        }),
                        structured_content: None,
                        error: is_error.then(|| content.clone().into()),
                    },
                    requires_result_confirmation: None,
                    meta: None,
                },
            ));
        }
    }

    fn result(&self, event: &Value) {
        let popped = {
            let mut state = self.state.lock().expect("turn state");
            let popped = state.turns.pop_front();

            state.parts.clear();
            state.tool_blocks.clear();
            state.tools.clear();
            state.asks.clear();
            popped
        };
        let Some(PendingTurn {
            id: turn_id,
            cancelled,
        }) = popped
        else {
            return;
        };
        if cancelled {
            return;
        }
        if let Some(usage) = event.get("usage").filter(|value| value.is_object()) {
            self.emit(StateAction::ChatUsage(ChatUsageAction {
                turn_id: turn_id.clone(),
                usage: UsageInfo {
                    input_tokens: usage["input_tokens"].as_i64(),
                    output_tokens: usage["output_tokens"].as_i64(),
                    model: usage["model"].as_str().map(str::to_owned),
                    cache_read_tokens: usage["cache_read_input_tokens"].as_i64(),
                    meta: None,
                },
                meta: None,
            }));
        }
        let duration = event["duration_ms"].as_i64().unwrap_or(0);
        let errored = event["is_error"].as_bool().unwrap_or(false);
        let subtype = event["subtype"].as_str().unwrap_or("success");
        if subtype.contains("cancel") || subtype.contains("interrupt") {
            self.emit(StateAction::ChatTurnCancelled(ChatTurnCancelledAction {
                turn_id,
                duration,
                meta: None,
            }));
        } else if errored {
            self.emit(StateAction::ChatError(
                ahp_types::actions::ChatErrorAction {
                    turn_id,
                    error: ahp_types::state::ErrorInfo {
                        error_type: subtype.to_owned(),
                        message: event["result"].as_str().unwrap_or("turn failed").to_owned(),
                        stack: None,
                        meta: None,
                    },
                    duration,
                    meta: None,
                },
            ));
        } else {
            self.emit(StateAction::ChatTurnComplete(ChatTurnCompleteAction {
                turn_id,
                duration,
                meta: None,
            }));
        }
    }
}

impl ClaudeAgent {
    fn file_edit_content(
        &self,
        path: &std::path::Path,
        before: Option<String>,
    ) -> ToolResultContent {
        let after = std::fs::read_to_string(path).ok();
        let counts = diff_counts(before.as_deref(), after.as_deref());
        let side = |text: Option<String>| -> Option<Value> {
            let text = text?;
            let stored = self.sink.stash(text);
            Some(json!({
                "uri": crate::uris::file_uri(&path),
                "content": { "uri": stored },
            }))
        };
        ToolResultContent::FileEdit(ahp_types::state::ToolResultFileEditContent {
            before: side(before),
            after: side(after),
            diff: Some(json!({ "added": counts.0, "removed": counts.1 })),
        })
    }
}

fn capture_before(track: &mut ToolTrack, cwd: &std::path::Path) {
    if track.edited_path.is_some() {
        return;
    }
    if !matches!(
        track.name.as_str(),
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit"
    ) {
        return;
    }
    let Some(path) = track.input.get("file_path").and_then(Value::as_str) else {
        return;
    };
    let mut path = std::path::PathBuf::from(path);
    if path.is_relative() {
        path = cwd.join(path);
    }
    track.before = std::fs::read_to_string(&path).ok();
    track.edited_path = Some(path);
}

fn diff_counts(before: Option<&str>, after: Option<&str>) -> (i64, i64) {
    let before: Vec<&str> = before
        .map(|text| text.lines().collect())
        .unwrap_or_default();
    let after: Vec<&str> = after.map(|text| text.lines().collect()).unwrap_or_default();
    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < before.len() - prefix
        && suffix < after.len() - prefix
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }
    (
        (after.len() - prefix - suffix) as i64,
        (before.len() - prefix - suffix) as i64,
    )
}

fn preview(input: &Value) -> String {
    if let Some(command) = input.get("command").and_then(Value::as_str) {
        return command.to_owned();
    }
    if let Some(path) = input.get("file_path").and_then(Value::as_str) {
        return path.to_owned();
    }
    let rendered = input.to_string();
    if rendered.len() > 200 {
        format!("{}…", &rendered[..200])
    } else {
        rendered
    }
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod discovery_tests {
    #[test]
    fn discovery_honors_the_override_then_finds_a_binary() {
        std::env::set_var("HIMARK_CLAUDE_BIN", "python3 /tmp/stub.py");
        assert_eq!(super::discover_binary(), "python3 /tmp/stub.py");
        std::env::remove_var("HIMARK_CLAUDE_BIN");
        assert!(!super::discover_binary().is_empty());
    }
}

#[cfg(test)]
mod wake_tests {
    use super::*;

    struct RecordingSink(Mutex<Vec<StateAction>>);

    impl AgentSink for RecordingSink {
        fn action(&self, action: StateAction) {
            self.0.lock().expect("recorded actions").push(action);
        }

        fn stash(&self, _text: String) -> String {
            "ahp-content:/test".to_owned()
        }
    }

    fn bare_agent() -> (ClaudeAgent, Arc<RecordingSink>) {
        let sink = Arc::new(RecordingSink(Mutex::new(Vec::new())));
        let agent = ClaudeAgent {
            stdin: Mutex::new(None),
            state: Mutex::new(TurnState::default()),
            sink: Arc::clone(&sink) as Sink,
            child: Mutex::new(None),
            stderr_tail: Mutex::new(std::collections::VecDeque::new()),
            cwd: std::path::PathBuf::from("/"),
            dead: std::sync::atomic::AtomicBool::new(false),
        };
        (agent, sink)
    }

    fn stream(inner: Value) -> Value {
        json!({"type": "stream_event", "parent_tool_use_id": null, "event": inner})
    }

    #[tokio::test]
    async fn an_agent_initiated_turn_is_adopted_and_rendered() {
        let (agent, sink) = bare_agent();
        agent
            .event(stream(
                json!({"type": "message_start", "message": {"role": "assistant"}}),
            ))
            .await;
        agent
            .event(stream(json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": ""}})))
            .await;
        agent
            .event(stream(json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": "WOKE"}})))
            .await;
        agent
            .event(json!({"type": "result", "subtype": "success"}))
            .await;

        let actions = sink.0.lock().expect("recorded actions");
        let StateAction::ChatTurnStarted(started) = &actions[0] else {
            panic!("the self-initiated turn was not adopted");
        };
        assert_eq!(started.message.origin.kind, MessageKind::SystemNotification);
        assert_eq!(started.message.text, "Scheduled wake-up");
        assert!(started.turn_id.starts_with("hihost-wake-"));
        assert!(
            matches!(&actions[1], StateAction::ChatResponsePart(part) if part.turn_id == started.turn_id)
        );
        assert!(matches!(&actions[2], StateAction::ChatDelta(delta)
            if delta.turn_id == started.turn_id && delta.content == "WOKE"));
        assert!(
            matches!(actions.last(), Some(StateAction::ChatTurnComplete(done))
            if done.turn_id == started.turn_id)
        );
        assert!(agent.idle());
    }

    #[tokio::test]
    async fn a_fired_wakeup_turn_carries_the_scheduled_prompt() {
        let (agent, sink) = bare_agent();
        agent
            .state
            .lock()
            .expect("turn state")
            .turns
            .push_back(PendingTurn {
                id: "turn-1".to_owned(),
                cancelled: false,
            });
        agent
            .event(stream(json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "tool_use", "id": "tool-1",
                                  "name": "ScheduleWakeup", "input": {}}})))
            .await;
        agent
            .event(stream(json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "input_json_delta",
                          "partial_json": "{\"delaySeconds\": 60, \"prompt\": \"check the CI run\"}"}})))
            .await;
        agent
            .event(stream(json!({"type": "content_block_stop", "index": 0})))
            .await;
        agent
            .event(
                json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "tool-1", "content": "scheduled"}]}}),
            )
            .await;
        agent
            .event(json!({"type": "result", "subtype": "success"}))
            .await;
        agent
            .event(stream(
                json!({"type": "message_start", "message": {"role": "assistant"}}),
            ))
            .await;

        let actions = sink.0.lock().expect("recorded actions");
        let adoptions: Vec<&ChatTurnStartedAction> = actions
            .iter()
            .filter_map(|action| match action {
                StateAction::ChatTurnStarted(started) => Some(started),
                _ => None,
            })
            .collect();
        assert_eq!(adoptions.len(), 1, "only the wake turn is adopted");
        assert_eq!(adoptions[0].message.text, "check the CI run");
        assert!(adoptions[0].turn_id.starts_with("hihost-wake-"));
    }
}
