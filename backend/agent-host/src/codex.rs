// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use ahp_types::actions::{
    ChatDeltaAction, ChatReasoningAction, ChatResponsePartAction, ChatToolCallCompleteAction,
    ChatToolCallReadyAction, ChatToolCallStartAction, ChatTurnCancelledAction,
    ChatTurnCompleteAction, ChatUsageAction, StateAction,
};
use ahp_types::state::{
    ConfirmationOption, ConfirmationOptionKind, MarkdownResponsePart, ReasoningResponsePart,
    ResponsePart, ToolCallResult, ToolInput, ToolResultContent, ToolResultTextContent, UsageInfo,
};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

use crate::claude::Sink;

pub fn discover_binary() -> String {
    if let Ok(binary) = std::env::var("HIMARK_CODEX_BIN") {
        return binary;
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("codex");
            if crate::claude::is_executable(&candidate) {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    for known in [
        format!("{home}/.local/bin/codex"),
        format!("{home}/.npm-global/bin/codex"),
        "/opt/homebrew/bin/codex".to_owned(),
        "/usr/local/bin/codex".to_owned(),
    ] {
        if crate::claude::is_executable(std::path::Path::new(&known)) {
            return known;
        }
    }
    "codex".to_owned()
}

/// One-shot `codex exec` call used for session titling. Runs outside any
/// thread (read-only sandbox, fresh rollout) so the conversation the user
/// sees stays untouched; the throwaway rollout is skipped by
/// `catalog::scan_codex` via the internal prompt marker.
pub async fn generate_title(
    binary: &str,
    cwd: &std::path::Path,
    prompt: &str,
) -> Result<String, String> {
    let mut words = binary.split_whitespace();
    let program = words.next().unwrap_or("codex").to_owned();
    let answer = std::env::temp_dir().join(format!("himark-title-{}.txt", crate::uuid_v4()));
    let mut command = tokio::process::Command::new(&program);
    command.args(words);
    command
        .arg("exec")
        .arg("--skip-git-repo-check")
        .args(["--sandbox", "read-only"])
        .arg("--output-last-message")
        .arg(&answer)
        .arg(prompt)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(60), command.output())
        .await
        .map_err(|_| "title call timed out".to_owned())
        .and_then(|held| held.map_err(|error| format!("failed to spawn `{binary}`: {error}")));
    let title = std::fs::read_to_string(&answer);
    let _ = std::fs::remove_file(&answer);
    let output = output?;
    if !output.status.success() {
        return Err(format!(
            "title call exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    title.map_err(|error| format!("title call produced no answer: {error}"))
}

pub struct SpawnConfig {
    pub binary: String,
    pub cwd: std::path::PathBuf,
    pub add_dirs: Vec<std::path::PathBuf>,

    pub native_id: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub thinking_level: Option<String>,
}

pub struct CodexAgent {
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    child: Mutex<Option<tokio::process::Child>>,
    state: Mutex<TurnState>,
    sink: Sink,
    native_id: String,
    cwd: std::path::PathBuf,
    writable_roots: Vec<std::path::PathBuf>,
    model: Option<String>,
    permission_mode: Option<String>,
    thinking_level: Option<String>,
    stderr_tail: Mutex<VecDeque<String>>,
    dead: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct TurnState {
    turns: VecDeque<PendingTurn>,
    parts: HashMap<String, PartTrack>,
    tools: HashMap<String, ToolTrack>,
    asks: HashMap<String, Value>,
    next_request: u64,
}

struct PendingTurn {
    id: String,
    prompt: Option<String>,
    codex_id: Option<String>,
    request_id: Option<u64>,
    cancelled: bool,
    interrupt_sent: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum PartKind {
    Text,
    Reasoning,
}

struct PartTrack {
    id: String,
    kind: PartKind,
    content: String,
}

struct ToolTrack {
    name: String,
    display_name: String,
    input: String,
    output: String,
    ready: bool,
    edits: Vec<FileBefore>,
}

struct FileBefore {
    path: std::path::PathBuf,
    before: Option<String>,
}

impl CodexAgent {
    pub async fn spawn(config: SpawnConfig, sink: Sink) -> Result<Arc<Self>, String> {
        let mut words = config.binary.split_whitespace();
        let program = words.next().unwrap_or("codex").to_owned();
        let mut command = tokio::process::Command::new(&program);
        command
            .args(words)
            .arg("app-server")
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| {
            format!(
                "failed to spawn `{}`: {error} - install the Codex CLI, or point \
                 HIMARK_CODEX_BIN at it",
                config.binary
            )
        })?;
        let mut stdin = child.stdin.take().ok_or("codex stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("codex stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("codex stderr unavailable")?;
        let mut lines = BufReader::new(stdout).lines();

        write_line(
            &mut stdin,
            &json!({
                "id": 0,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "himark",
                        "title": "himark Agent Host",
                        "version": crate::SERVER_VERSION,
                    }
                }
            }),
        )
        .await?;
        response(&mut lines, 0).await?;
        write_line(&mut stdin, &json!({"method": "initialized", "params": {}})).await?;

        let mut params = Map::new();
        params.insert(
            "cwd".to_owned(),
            Value::String(config.cwd.to_string_lossy().into_owned()),
        );
        params.insert(
            "approvalPolicy".to_owned(),
            Value::String(approval_policy(config.permission_mode.as_deref()).to_owned()),
        );
        params.insert(
            "sandbox".to_owned(),
            Value::String(sandbox_mode(config.permission_mode.as_deref()).to_owned()),
        );
        if let Some(model) = &config.model {
            params.insert("model".to_owned(), Value::String(model.clone()));
        }
        let method = if let Some(native_id) = &config.native_id {
            params.insert("threadId".to_owned(), Value::String(native_id.clone()));
            "thread/resume"
        } else {
            params.insert("serviceName".to_owned(), Value::String("himark".to_owned()));
            "thread/start"
        };
        write_line(
            &mut stdin,
            &json!({"id": 1, "method": method, "params": params}),
        )
        .await?;
        let opened = response(&mut lines, 1).await?;
        let native_id = opened["result"]["thread"]["id"]
            .as_str()
            .ok_or_else(|| format!("codex {method} did not return a thread id: {opened}"))?
            .to_owned();

        let agent = Arc::new(Self {
            stdin: Mutex::new(Some(stdin)),
            child: Mutex::new(Some(child)),
            state: Mutex::new(TurnState {
                next_request: 2,
                ..TurnState::default()
            }),
            sink,
            native_id,
            cwd: config.cwd,
            writable_roots: config.add_dirs,
            model: config.model,
            permission_mode: config.permission_mode,
            thinking_level: config.thinking_level,
            stderr_tail: Mutex::new(VecDeque::new()),
            dead: std::sync::atomic::AtomicBool::new(false),
        });

        let errors = Arc::clone(&agent);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if std::env::var("HIHOST_TRACE").is_ok() {
                    eprintln!("[codex!] {line}");
                }
                let mut tail = errors.stderr_tail.lock().expect("stderr tail");
                tail.push_back(line);
                while tail.len() > 8 {
                    tail.pop_front();
                }
            }
        });
        let pump = Arc::clone(&agent);
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                if std::env::var("HIHOST_TRACE").is_ok() {
                    eprintln!("[codex<-] {}", &line[..line.len().min(160)]);
                }
                if let Ok(message) = serde_json::from_str::<Value>(&line) {
                    pump.event(message).await;
                }
            }
            pump.stream_ended();
        });
        Ok(agent)
    }

    pub fn native_id(&self) -> &str {
        &self.native_id
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn prompt(&self, turn_id: String, text: String) -> Result<(), String> {
        if self.is_dead() {
            return Err("the codex app-server process is gone".to_owned());
        }
        let start = {
            let mut state = self.state.lock().expect("turn state");
            let start = state.turns.is_empty();
            state.turns.push_back(PendingTurn {
                id: turn_id,
                prompt: (!start).then(|| text.clone()),
                codex_id: None,
                request_id: None,
                cancelled: false,
                interrupt_sent: false,
            });
            start
        };
        if start {
            self.start_front(text).await?;
        }
        Ok(())
    }

    async fn start_front(&self, text: String) -> Result<(), String> {
        let request_id = {
            let mut state = self.state.lock().expect("turn state");
            let id = state.next_request;
            state.next_request += 1;
            let Some(front) = state.turns.front_mut() else {
                return Ok(());
            };
            front.request_id = Some(id);
            id
        };
        let mut params = Map::new();
        params.insert("threadId".to_owned(), Value::String(self.native_id.clone()));
        params.insert("input".to_owned(), json!([{"type": "text", "text": text}]));
        params.insert(
            "approvalPolicy".to_owned(),
            Value::String(approval_policy(self.permission_mode.as_deref()).to_owned()),
        );
        params.insert("sandboxPolicy".to_owned(), self.sandbox_policy());
        if let Some(model) = &self.model {
            params.insert("model".to_owned(), Value::String(model.clone()));
        }
        if let Some(effort) = &self.thinking_level {
            params.insert("effort".to_owned(), Value::String(effort.clone()));
        }
        if let Err(error) = self
            .send(json!({"id": request_id, "method": "turn/start", "params": params}))
            .await
        {
            let mut state = self.state.lock().expect("turn state");
            state.turns.pop_front();
            return Err(error);
        }
        Ok(())
    }

    pub async fn answer(&self, tool_call_id: &str, approved: bool) -> Result<(), String> {
        let id = self
            .state
            .lock()
            .expect("turn state")
            .asks
            .remove(tool_call_id);
        let Some(id) = id else { return Ok(()) };
        self.send(json!({
            "id": id,
            "result": {"decision": if approved {"accept"} else {"decline"}},
        }))
        .await
    }

    pub async fn interrupt(&self) -> Result<(), String> {
        let codex_id = {
            let mut state = self.state.lock().expect("turn state");
            let Some(front) = state.turns.front_mut() else {
                return Ok(());
            };
            front.cancelled = true;
            if front.interrupt_sent {
                return Ok(());
            }
            let Some(id) = front.codex_id.clone() else {
                return Ok(());
            };
            front.interrupt_sent = true;
            id
        };
        self.send_interrupt(codex_id).await
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

    fn sandbox_policy(&self) -> Value {
        match self.permission_mode.as_deref() {
            Some("bypassPermissions") => json!({"type": "dangerFullAccess"}),
            Some("plan") => json!({"type": "readOnly"}),
            _ => {
                let mut roots = vec![self.cwd.to_string_lossy().into_owned()];
                roots.extend(
                    self.writable_roots
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned()),
                );
                json!({
                    "type": "workspaceWrite",
                    "writableRoots": roots,
                    "networkAccess": false,
                })
            }
        }
    }

    async fn send_interrupt(&self, codex_id: String) -> Result<(), String> {
        let request_id = {
            let mut state = self.state.lock().expect("turn state");
            let id = state.next_request;
            state.next_request += 1;
            id
        };
        self.send(json!({
            "id": request_id,
            "method": "turn/interrupt",
            "params": {"threadId": self.native_id, "turnId": codex_id},
        }))
        .await
    }

    async fn send(&self, message: Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        let stdin = self.stdin.lock().expect("agent stdin").take();
        let Some(mut stdin) = stdin else {
            return Err("the codex app-server process is gone".to_owned());
        };
        let result = stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| format!("codex stdin: {error}"));
        if std::env::var("HIHOST_TRACE").is_ok() {
            eprintln!("[codex->] {:?} {}", result, &line[..line.len().min(160)]);
        }
        *self.stdin.lock().expect("agent stdin") = Some(stdin);
        result
    }

    async fn event(&self, message: Value) {
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            if message.get("id").is_some() {
                self.server_request(method, &message).await;
            } else {
                self.notification(method, &message["params"]).await;
            }
            return;
        }
        self.response(message).await;
    }

    async fn response(&self, message: Value) {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return;
        };
        let matches_turn = {
            let state = self.state.lock().expect("turn state");
            state
                .turns
                .front()
                .is_some_and(|turn| turn.request_id == Some(id))
        };
        if !matches_turn {
            return;
        }
        if let Some(error) = message.get("error") {
            let turn = {
                let mut state = self.state.lock().expect("turn state");
                let turn = state.turns.pop_front();
                clear_turn(&mut state);
                turn
            };
            if let Some(turn) = turn.filter(|turn| !turn.cancelled) {
                self.emit_error(
                    turn.id,
                    error["message"]
                        .as_str()
                        .unwrap_or("Codex rejected the turn"),
                    "codexRequest",
                    0,
                );
            }
            self.start_queued().await;
            return;
        }
        if let Some(codex_id) = message["result"]["turn"]["id"].as_str() {
            self.bind_turn(codex_id.to_owned()).await;
        }
    }

    async fn notification(&self, method: &str, params: &Value) {
        if params
            .get("threadId")
            .and_then(Value::as_str)
            .is_some_and(|id| id != self.native_id)
        {
            return;
        }
        match method {
            "turn/started" => {
                if let Some(id) = params["turn"]["id"].as_str() {
                    self.bind_turn(id.to_owned()).await;
                }
            }
            "item/started" => self.item_started(&params["item"]),
            "item/completed" => self.item_completed(&params["item"]),
            "item/agentMessage/delta" => {
                self.append_part(
                    params["itemId"].as_str().unwrap_or_default(),
                    PartKind::Text,
                    params["delta"].as_str().unwrap_or_default(),
                );
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                self.append_part(
                    params["itemId"].as_str().unwrap_or_default(),
                    PartKind::Reasoning,
                    params["delta"].as_str().unwrap_or_default(),
                );
            }
            "item/commandExecution/outputDelta" => {
                let item = params["itemId"].as_str().unwrap_or_default();
                let delta = params["delta"].as_str().unwrap_or_default();
                if let Some(tool) = self.state.lock().expect("turn state").tools.get_mut(item) {
                    tool.output.push_str(delta);
                }
            }
            "thread/tokenUsage/updated" => self.usage(params),
            "turn/completed" => self.turn_completed(&params["turn"]).await,
            _ => {}
        }
    }

    async fn bind_turn(&self, codex_id: String) {
        let interrupt = {
            let mut state = self.state.lock().expect("turn state");
            let Some(front) = state.turns.front_mut() else {
                return;
            };
            if front.codex_id.is_none() {
                front.codex_id = Some(codex_id.clone());
            }
            if front.cancelled && !front.interrupt_sent {
                front.interrupt_sent = true;
                true
            } else {
                false
            }
        };
        if interrupt {
            let _ = self.send_interrupt(codex_id).await;
        }
    }

    fn item_started(&self, item: &Value) {
        match item["type"].as_str() {
            Some("agentMessage") => {
                self.open_part(item["id"].as_str().unwrap_or_default(), PartKind::Text);
                self.append_part(
                    item["id"].as_str().unwrap_or_default(),
                    PartKind::Text,
                    item["text"].as_str().unwrap_or_default(),
                );
            }
            Some("reasoning") => {
                self.open_part(item["id"].as_str().unwrap_or_default(), PartKind::Reasoning);
            }
            Some("commandExecution") | Some("fileChange") | Some("mcpToolCall") => {
                self.open_tool(item)
            }
            _ => {}
        }
    }

    fn item_completed(&self, item: &Value) {
        match item["type"].as_str() {
            Some("agentMessage") => self.finish_part(
                item["id"].as_str().unwrap_or_default(),
                PartKind::Text,
                item["text"].as_str().unwrap_or_default(),
            ),
            Some("reasoning") => {
                let full = item["summary"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                self.finish_part(
                    item["id"].as_str().unwrap_or_default(),
                    PartKind::Reasoning,
                    &full,
                );
            }
            Some("commandExecution") | Some("fileChange") | Some("mcpToolCall") => {
                self.complete_tool(item)
            }
            _ => {}
        }
    }

    fn open_part(&self, item_id: &str, kind: PartKind) {
        if item_id.is_empty() {
            return;
        }
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        let fresh = {
            let mut state = self.state.lock().expect("turn state");
            if state.parts.contains_key(item_id) {
                false
            } else {
                state.parts.insert(
                    item_id.to_owned(),
                    PartTrack {
                        id: item_id.to_owned(),
                        kind,
                        content: String::new(),
                    },
                );
                true
            }
        };
        if !fresh {
            return;
        }
        let part = match kind {
            PartKind::Text => ResponsePart::Markdown(MarkdownResponsePart {
                id: item_id.to_owned(),
                content: String::new(),
            }),
            PartKind::Reasoning => ResponsePart::Reasoning(ReasoningResponsePart {
                id: item_id.to_owned(),
                content: String::new(),
            }),
        };
        self.emit(StateAction::ChatResponsePart(ChatResponsePartAction {
            turn_id,
            part,
            meta: None,
        }));
    }

    fn append_part(&self, item_id: &str, kind: PartKind, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.open_part(item_id, kind);
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        let part_id = {
            let mut state = self.state.lock().expect("turn state");
            let Some(part) = state.parts.get_mut(item_id) else {
                return;
            };
            part.content.push_str(delta);
            part.id.clone()
        };
        let action = match kind {
            PartKind::Text => StateAction::ChatDelta(ChatDeltaAction {
                turn_id,
                part_id,
                content: delta.to_owned(),
                meta: None,
            }),
            PartKind::Reasoning => StateAction::ChatReasoning(ChatReasoningAction {
                turn_id,
                part_id,
                content: delta.to_owned(),
                meta: None,
            }),
        };
        self.emit(action);
    }

    fn finish_part(&self, item_id: &str, kind: PartKind, full: &str) {
        self.open_part(item_id, kind);
        let remainder = {
            let state = self.state.lock().expect("turn state");
            let Some(part) = state.parts.get(item_id) else {
                return;
            };
            if part.kind != kind || full.is_empty() || full == part.content {
                None
            } else if full.starts_with(&part.content) {
                Some(full[part.content.len()..].to_owned())
            } else if part.content.is_empty() {
                Some(full.to_owned())
            } else {
                None
            }
        };
        if let Some(remainder) = remainder {
            self.append_part(item_id, kind, &remainder);
        }
    }

    fn open_tool(&self, item: &Value) {
        let id = item["id"].as_str().unwrap_or_default();
        if id.is_empty() {
            return;
        }
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        let (name, display, input, edits) = tool_description(item, &self.cwd);
        let fresh = {
            let mut state = self.state.lock().expect("turn state");
            if state.tools.contains_key(id) {
                false
            } else {
                state.tools.insert(
                    id.to_owned(),
                    ToolTrack {
                        name: name.clone(),
                        display_name: display.clone(),
                        input,
                        output: String::new(),
                        ready: false,
                        edits,
                    },
                );
                true
            }
        };
        if fresh {
            self.emit(StateAction::ChatToolCallStart(ChatToolCallStartAction {
                turn_id,
                tool_call_id: id.to_owned(),
                tool_name: name,
                display_name: display,
                intention: None,
                contributor: None,
                meta: None,
            }));
        }
    }

    async fn server_request(&self, method: &str, message: &Value) {
        let id = message["id"].clone();
        match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                let params = &message["params"];
                let item_id = params["itemId"].as_str().unwrap_or_default();
                if !self
                    .state
                    .lock()
                    .expect("turn state")
                    .tools
                    .contains_key(item_id)
                    && method.contains("commandExecution")
                {
                    self.open_tool(&json!({
                        "type": "commandExecution",
                        "id": item_id,
                        "command": params["command"],
                        "cwd": params["cwd"],
                        "status": "inProgress",
                        "commandActions": params["commandActions"],
                    }));
                }
                self.state
                    .lock()
                    .expect("turn state")
                    .asks
                    .insert(item_id.to_owned(), id);
                self.ready_tool(item_id, params["reason"].as_str(), true);
            }
            _ => {
                let _ = self
                    .send(json!({
                        "id": id,
                        "error": {"code": -32601, "message": format!("unsupported Codex request: {method}")},
                    }))
                    .await;
            }
        }
    }

    fn ready_tool(&self, item_id: &str, reason: Option<&str>, pending: bool) {
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        let details = {
            let mut state = self.state.lock().expect("turn state");
            let Some(tool) = state.tools.get_mut(item_id) else {
                return;
            };
            if tool.ready {
                return;
            }
            tool.ready = true;
            (
                tool.display_name.clone(),
                tool.input.clone(),
                reason
                    .filter(|reason| !reason.is_empty())
                    .unwrap_or(&tool.input)
                    .to_owned(),
            )
        };
        self.emit(StateAction::ChatToolCallReady(ChatToolCallReadyAction {
            turn_id,
            tool_call_id: item_id.to_owned(),
            contributor: None,
            intention: None,
            invocation_message: details.2.into(),
            tool_input: (!details.1.is_empty()).then(|| ToolInput::Inline(details.1)),
            confirmation_title: pending.then(|| details.0.clone().into()),
            risk_assessment: None,
            edits: None,
            editable: None,
            confirmed: (!pending)
                .then_some(ahp_types::state::ToolCallConfirmationReason::NotNeeded),
            options: pending.then(|| {
                vec![
                    ConfirmationOption {
                        id: "allow".to_owned(),
                        label: "Yes".to_owned(),
                        kind: ConfirmationOptionKind::Approve,
                        group: None,
                    },
                    ConfirmationOption {
                        id: "deny".to_owned(),
                        label: "No, tell Codex what to do differently".to_owned(),
                        kind: ConfirmationOptionKind::Deny,
                        group: None,
                    },
                ]
            }),
            meta: None,
        }));
    }

    fn complete_tool(&self, item: &Value) {
        let id = item["id"].as_str().unwrap_or_default();
        self.open_tool(item);
        self.ready_tool(id, None, false);
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        let track = self.state.lock().expect("turn state").tools.remove(id);
        let Some(mut track) = track else { return };
        let (success, final_output) = match item["type"].as_str() {
            Some("commandExecution") => {
                let output = item["aggregatedOutput"]
                    .as_str()
                    .filter(|output| !output.is_empty())
                    .unwrap_or(&track.output)
                    .to_owned();
                (item["status"] == "completed", output)
            }
            Some("fileChange") => (
                item["status"] == "completed",
                serde_json::to_string_pretty(&item["changes"]).unwrap_or_default(),
            ),
            Some("mcpToolCall") => {
                let success = item["status"] == "completed" && item["error"].is_null();
                let value = if success {
                    &item["result"]
                } else {
                    &item["error"]
                };
                (
                    success,
                    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
                )
            }
            _ => (true, String::new()),
        };
        let mut content = Vec::new();
        if !final_output.is_empty() {
            content.push(ToolResultContent::Text(ToolResultTextContent {
                text: final_output.clone(),
            }));
        }
        if success {
            content.extend(
                track
                    .edits
                    .drain(..)
                    .map(|edit| self.file_edit_content(edit)),
            );
        }
        self.emit(StateAction::ChatToolCallComplete(
            ChatToolCallCompleteAction {
                turn_id,
                tool_call_id: id.to_owned(),
                result: ToolCallResult {
                    success,
                    past_tense_message: format!("Ran {}", track.name).into(),
                    content: (!content.is_empty()).then_some(content),
                    structured_content: None,
                    error: (!success).then(|| final_output.into()),
                },
                requires_result_confirmation: None,
                meta: None,
            },
        ));
    }

    fn usage(&self, params: &Value) {
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        let usage = &params["tokenUsage"]["last"];
        self.emit(StateAction::ChatUsage(ChatUsageAction {
            turn_id,
            usage: UsageInfo {
                input_tokens: usage["inputTokens"].as_i64(),
                output_tokens: usage["outputTokens"].as_i64(),
                model: self.model.clone(),
                cache_read_tokens: usage["cachedInputTokens"].as_i64(),
                meta: None,
            },
            meta: None,
        }));
    }

    async fn turn_completed(&self, turn: &Value) {
        let codex_id = turn["id"].as_str().unwrap_or_default();
        let popped = {
            let mut state = self.state.lock().expect("turn state");
            if state
                .turns
                .front()
                .and_then(|turn| turn.codex_id.as_deref())
                .is_some_and(|id| id != codex_id)
            {
                return;
            }
            let popped = state.turns.pop_front();
            clear_turn(&mut state);
            popped
        };
        let Some(popped) = popped else { return };
        if !popped.cancelled {
            let duration = turn["durationMs"].as_i64().unwrap_or(0);
            match turn["status"].as_str().unwrap_or("failed") {
                "completed" => self.emit(StateAction::ChatTurnComplete(ChatTurnCompleteAction {
                    turn_id: popped.id,
                    duration,
                    meta: None,
                })),
                "interrupted" => {
                    self.emit(StateAction::ChatTurnCancelled(ChatTurnCancelledAction {
                        turn_id: popped.id,
                        duration,
                        meta: None,
                    }))
                }
                _ => self.emit_error(
                    popped.id,
                    turn["error"]["message"]
                        .as_str()
                        .unwrap_or("Codex turn failed"),
                    "codexTurn",
                    duration,
                ),
            }
        }
        self.start_queued().await;
    }

    async fn start_queued(&self) {
        let queued = {
            let mut state = self.state.lock().expect("turn state");
            state
                .turns
                .front_mut()
                .and_then(|turn| turn.prompt.take().map(|prompt| (turn.id.clone(), prompt)))
        };
        if let Some((turn_id, prompt)) = queued {
            if let Err(error) = self.start_front(prompt).await {
                self.emit_error(turn_id, &error, "agentGone", 0);
            }
        }
    }

    fn turn_id(&self) -> Option<String> {
        self.state
            .lock()
            .expect("turn state")
            .turns
            .front()
            .map(|turn| turn.id.clone())
    }

    fn emit(&self, action: StateAction) {
        self.sink.action(action);
    }

    fn emit_error(&self, turn_id: String, message: &str, kind: &str, duration: i64) {
        self.emit(StateAction::ChatError(
            ahp_types::actions::ChatErrorAction {
                turn_id,
                error: ahp_types::state::ErrorInfo {
                    error_type: kind.to_owned(),
                    message: message.to_owned(),
                    stack: None,
                    meta: None,
                },
                duration,
                meta: None,
            },
        ));
    }

    fn stream_ended(&self) {
        self.dead.store(true, std::sync::atomic::Ordering::SeqCst);
        let turns: Vec<PendingTurn> = {
            let mut state = self.state.lock().expect("turn state");
            let turns = state.turns.drain(..).collect();
            clear_turn(&mut state);
            turns
        };
        let tail = self
            .stderr_tail
            .lock()
            .expect("stderr tail")
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let message = if tail.is_empty() {
            "the codex app-server process ended mid-turn".to_owned()
        } else {
            format!(
                "the codex app-server process ended mid-turn: {}",
                tail.join(" | ")
            )
        };
        for turn in turns.into_iter().filter(|turn| !turn.cancelled) {
            self.emit_error(turn.id, &message, "agentGone", 0);
        }
    }

    fn file_edit_content(&self, edit: FileBefore) -> ToolResultContent {
        let after = std::fs::read_to_string(&edit.path).ok();
        let counts = diff_counts(edit.before.as_deref(), after.as_deref());
        let side = |text: Option<String>| -> Option<Value> {
            let text = text?;
            let stored = self.sink.stash(text);
            Some(json!({
                "uri": crate::uris::file_uri(&edit.path),
                "content": {"uri": stored},
            }))
        };
        ToolResultContent::FileEdit(ahp_types::state::ToolResultFileEditContent {
            before: side(edit.before),
            after: side(after),
            diff: Some(json!({"added": counts.0, "removed": counts.1})),
        })
    }
}

fn clear_turn(state: &mut TurnState) {
    state.parts.clear();
    state.tools.clear();
    state.asks.clear();
}

fn approval_policy(mode: Option<&str>) -> &'static str {
    match mode {
        Some("bypassPermissions" | "plan") => "never",
        _ => "on-request",
    }
}

fn sandbox_mode(mode: Option<&str>) -> &'static str {
    match mode {
        Some("bypassPermissions") => "danger-full-access",
        Some("plan") => "read-only",
        _ => "workspace-write",
    }
}

async fn write_line(stdin: &mut tokio::process::ChildStdin, value: &Value) -> Result<(), String> {
    stdin
        .write_all(format!("{value}\n").as_bytes())
        .await
        .map_err(|error| format!("codex stdin: {error}"))
}

async fn response(
    lines: &mut Lines<BufReader<tokio::process::ChildStdout>>,
    id: u64,
) -> Result<Value, String> {
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("codex stdout: {error}"))?
    {
        let message: Value = serde_json::from_str(&line)
            .map_err(|error| format!("invalid codex JSONL: {error}: {line}"))?;
        if message.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = message.get("error") {
            return Err(format!(
                "codex request failed: {}",
                error["message"].as_str().unwrap_or("unknown error")
            ));
        }
        return Ok(message);
    }
    Err("codex app-server ended during initialization".to_owned())
}

fn tool_description(
    item: &Value,
    cwd: &std::path::Path,
) -> (String, String, String, Vec<FileBefore>) {
    match item["type"].as_str() {
        Some("commandExecution") => {
            let command = item["command"].as_str().unwrap_or_default().to_owned();
            (
                "Shell".to_owned(),
                "Command".to_owned(),
                command,
                Vec::new(),
            )
        }
        Some("fileChange") => {
            let mut edits = Vec::new();
            let paths = item["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|change| change["path"].as_str())
                .map(|path| {
                    let path = std::path::PathBuf::from(path);
                    let path = if path.is_relative() {
                        cwd.join(path)
                    } else {
                        path
                    };
                    edits.push(FileBefore {
                        before: std::fs::read_to_string(&path).ok(),
                        path: path.clone(),
                    });
                    path.to_string_lossy().into_owned()
                })
                .collect::<Vec<_>>()
                .join(", ");
            (
                "FileChange".to_owned(),
                "File change".to_owned(),
                paths,
                edits,
            )
        }
        Some("mcpToolCall") => {
            let server = item["server"].as_str().unwrap_or("MCP");
            let tool = item["tool"].as_str().unwrap_or("tool");
            let name = format!("{server}/{tool}");
            (
                name.clone(),
                name,
                serde_json::to_string_pretty(&item["arguments"])
                    .unwrap_or_else(|_| item["arguments"].to_string()),
                Vec::new(),
            )
        }
        _ => (
            "Tool".to_owned(),
            "Tool".to_owned(),
            String::new(),
            Vec::new(),
        ),
    }
}

fn diff_counts(before: Option<&str>, after: Option<&str>) -> (i64, i64) {
    let before: Vec<&str> = before.map(str::lines).into_iter().flatten().collect();
    let after: Vec<&str> = after.map(str::lines).into_iter().flatten().collect();
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
