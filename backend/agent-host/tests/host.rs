// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

const ROOT: &str = "ahp-root://";

struct Client {
    write: tokio::net::unix::OwnedWriteHalf,
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    next_id: u64,

    pending: std::collections::VecDeque<Value>,
}

impl Client {
    async fn connect(host: Arc<agent_host::Host>) -> Client {
        let (ours, theirs) = UnixStream::pair().expect("socketpair");
        tokio::spawn(host.serve_stream(theirs));
        let (read, write) = ours.into_split();
        Client {
            write,
            lines: BufReader::new(read).lines(),
            next_id: 0,
            pending: std::collections::VecDeque::new(),
        }
    }

    async fn send_raw(&mut self, value: Value) {
        self.write
            .write_all(format!("{value}\n").as_bytes())
            .await
            .expect("send");
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send_raw(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let line = self.lines.next_line().await.expect("read").expect("open");
            let message: Value = serde_json::from_str(&line).expect("json");
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                assert!(
                    message.get("error").is_none(),
                    "request {method} failed: {message}"
                );
                return message["result"].clone();
            }
            self.pending.push_back(message);
        }
    }

    async fn request_any(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send_raw(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let line = self.lines.next_line().await.expect("read").expect("open");
            let message: Value = serde_json::from_str(&line).expect("json");
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                return message;
            }
            self.pending.push_back(message);
        }
    }

    async fn response_for(&mut self, id: u64) -> Value {
        loop {
            let line = self.lines.next_line().await.expect("read").expect("open");
            let message: Value = serde_json::from_str(&line).expect("json");
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                return message;
            }
            self.pending.push_back(message);
        }
    }

    async fn dispatch(&mut self, channel: &str, action: Value) {
        self.send_raw(json!({
            "jsonrpc": "2.0", "method": "dispatchAction",
            "params": {"channel": channel, "clientSeq": 1, "action": action},
        }))
        .await;
    }

    async fn next_action(&mut self, channel: &str) -> Value {
        while let Some(message) = self.pending.pop_front() {
            if message["method"] == "action" && message["params"]["channel"] == channel {
                return message["params"]["action"].clone();
            }
        }
        loop {
            let line = self.lines.next_line().await.expect("read").expect("open");
            let message: Value = serde_json::from_str(&line).expect("json");
            if std::env::var("HIHOST_TEST_TRACE").is_ok() {
                eprintln!("<< {message}");
            }
            if message["method"] == "action" && message["params"]["channel"] == channel {
                return message["params"]["action"].clone();
            }
        }
    }

    async fn next_notification(&mut self, method: &str) -> Value {
        while let Some(message) = self.pending.pop_front() {
            if message["method"] == method {
                return message["params"].clone();
            }
        }
        loop {
            let line = self.lines.next_line().await.expect("read").expect("open");
            let message: Value = serde_json::from_str(&line).expect("json");
            if message["method"] == method {
                return message["params"].clone();
            }
        }
    }

    async fn actions_until(&mut self, channel: &str, terminal: &str) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let action = self.next_action(channel).await;
            let kind = action["type"].as_str().unwrap_or_default().to_owned();
            seen.push(action);
            if kind == terminal {
                return seen;
            }
        }
    }
}

fn host_at(dir: &Path) -> Arc<agent_host::Host> {
    agent_host::Host::new(agent_host::HostConfig {
        agents: Vec::new(),
        data_dir: dir.join("data"),
        claude_binary: agent_host::testing::fake_cli_command(dir),
        codex_binary: "false".to_owned(),
        claude_home: dir.join("dot-claude"),
        codex_home: dir.join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
    })
}

async fn open_session(client: &mut Client, dir: &Path) -> (String, String) {
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let session = format!("ahp-session:/test-{}", std::process::id());
    client
        .request(
            "createSession",
            json!({
                "channel": session,
                "provider": "claude",
                "workingDirectories": [format!("file://{}", dir.display())],
                "config": {"isolation": "folder"},
            }),
        )
        .await;
    let snapshot = client
        .request("subscribe", json!({"channel": session}))
        .await;
    let chat = snapshot["snapshot"]["state"]["defaultChat"]
        .as_str()
        .expect("default chat")
        .to_owned();
    client.request("subscribe", json!({"channel": chat})).await;
    (session, chat)
}

fn codex_host_at(dir: &Path) -> Arc<agent_host::Host> {
    agent_host::Host::new(agent_host::HostConfig {
        data_dir: dir.join("codex-data"),
        claude_binary: "false".to_owned(),
        codex_binary: agent_host::testing::fake_codex_command(dir),
        claude_home: dir.join("dot-claude"),
        codex_home: dir.join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
        ..agent_host::HostConfig::default()
    })
}

async fn open_codex_session(client: &mut Client, dir: &Path) -> (String, String) {
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let session = format!("ahp-session:/codex-test-{}", std::process::id());
    client
        .request(
            "createSession",
            json!({
                "channel": session,
                "provider": "codex",
                "workingDirectories": [format!("file://{}", dir.display())],
                "config": {"permissionMode": "default"},
                "model": {"id": "gpt-5.6-terra", "config": {"thinkingLevel": "high"}},
            }),
        )
        .await;
    let snapshot = client
        .request("subscribe", json!({"channel": session}))
        .await;
    let chat = snapshot["snapshot"]["state"]["defaultChat"]
        .as_str()
        .expect("default chat")
        .to_owned();
    client.request("subscribe", json!({"channel": chat})).await;
    (session, chat)
}

#[tokio::test]
async fn an_annotations_attachment_expands_into_the_prompt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (session, chat) = open_session(&mut client, dir.path()).await;
    let channel = format!("{session}/annotations");
    client
        .request("subscribe", json!({"channel": channel}))
        .await;
    let file = format!("file://{}/src/main.rs", dir.path().display());
    client
        .dispatch(&channel, annotation_set("a-1", &file, "why unwrap?"))
        .await;
    client.next_action(&channel).await;
    client
        .dispatch(&channel, annotation_set("a-2", &file, "rename this"))
        .await;
    client.next_action(&channel).await;

    client
        .dispatch(
            &chat,
            json!({
                "type": "chat/turnStarted",
                "turnId": "t-review",
                "startedAt": "2026-08-19T12:00:00.000Z",
                "message": {
                    "text": "echo-prompt: address the attached review comment.",
                    "origin": {"kind": "user"},
                    "attachments": [{
                        "type": "annotations",
                        "label": "1 review comment",
                        "resource": channel,
                        "annotationIds": ["a-1"],
                    }],
                },
            }),
        )
        .await;
    let actions = client.actions_until(&chat, "chat/turnComplete").await;

    assert_eq!(
        actions[0]["message"]["text"], "echo-prompt: address the attached review comment.",
        "{:?}",
        actions[0]
    );
    let prompt: String = actions
        .iter()
        .filter(|action| action["type"] == "chat/delta")
        .filter_map(|action| action["content"].as_str())
        .collect();
    assert!(prompt.contains("<review-comments>"), "{prompt}");
    assert!(prompt.contains("why unwrap?"), "{prompt}");
    assert!(prompt.contains("src/main.rs"), "{prompt}");
    assert!(prompt.contains("line 2"), "{prompt}");
    assert!(prompt.contains("[user]"), "{prompt}");
    assert!(
        !prompt.contains("rename this"),
        "unreferenced annotations stay out: {prompt}"
    );
}

fn turn_started(turn: &str, text: &str) -> Value {
    json!({
        "type": "chat/turnStarted",
        "turnId": turn,
        "startedAt": "2026-08-13T12:00:00.000Z",
        "message": {"text": text, "origin": {"kind": "user"}},
    })
}

#[tokio::test]
async fn a_turn_streams_through_the_fake_cli() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (_session, chat) = open_session(&mut client, dir.path()).await;

    client.dispatch(&chat, turn_started("t-1", "hello")).await;
    let actions = client.actions_until(&chat, "chat/turnComplete").await;
    let kinds: Vec<&str> = actions
        .iter()
        .map(|action| action["type"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(kinds[0], "chat/turnStarted", "{kinds:?}");
    assert!(kinds.contains(&"chat/responsePart"), "{kinds:?}");
    assert!(kinds.contains(&"chat/delta"), "{kinds:?}");
    assert!(kinds.contains(&"chat/usage"), "{kinds:?}");
    let text: String = actions
        .iter()
        .filter(|action| action["type"] == "chat/delta")
        .filter_map(|action| action["content"].as_str())
        .collect();
    assert_eq!(text, "OK");

    let snapshot = client.request("subscribe", json!({"channel": chat})).await;
    let turns = snapshot["snapshot"]["state"]["turns"]
        .as_array()
        .expect("turns");
    assert_eq!(turns.len(), 1, "one folded turn");
}

#[tokio::test]
async fn the_session_summary_tracks_turn_activity_and_reads() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (session, chat) = open_session(&mut client, dir.path()).await;
    client.request("subscribe", json!({"channel": ROOT})).await;

    client.dispatch(&chat, turn_started("t-1", "hello")).await;
    let changed = client.next_notification("root/sessionSummaryChanged").await;
    assert_eq!(changed["session"], session, "{changed}");
    let status = changed["changes"]["status"].as_u64().expect("status");
    assert_eq!(status & 8, 8, "the turn runs: {changed}");
    assert_eq!(status & 32, 0, "new content is unread: {changed}");
    assert!(changed["changes"]["modifiedAt"].is_string(), "{changed}");
    let retitled = client.next_notification("root/sessionSummaryChanged").await;
    assert_eq!(retitled["changes"]["title"], "hello", "{retitled}");

    client.actions_until(&chat, "chat/turnComplete").await;
    let changed = client.next_notification("root/sessionSummaryChanged").await;
    let status = changed["changes"]["status"].as_u64().expect("status");
    assert_eq!(status & 8, 0, "the turn ended: {changed}");
    assert_eq!(status & 1, 1, "idle again: {changed}");
    assert_eq!(status & 32, 0, "still unread: {changed}");

    let list = client
        .request("listSessions", json!({"channel": ROOT}))
        .await;
    let held = list["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["resource"] == session))
        .expect("the session is listed");
    assert_eq!(held["status"].as_u64(), Some(status), "{held}");

    client
        .dispatch(
            &session,
            json!({"type": "session/isReadChanged", "isRead": true}),
        )
        .await;
    let changed = client.next_notification("root/sessionSummaryChanged").await;
    let status = changed["changes"]["status"].as_u64().expect("status");
    assert_eq!(status & 32, 32, "viewing marks it read: {changed}");
    assert_eq!(status & 1, 1, "{changed}");
}

#[tokio::test]
async fn a_restart_keeps_the_replayed_error_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A host whose agent binary cannot spawn: the turn fails for real
    // and the chat log ends on chat/error.
    let host = agent_host::Host::new(agent_host::HostConfig {
        agents: Vec::new(),
        data_dir: dir.path().join("data"),
        claude_binary: dir.path().join("no-such-agent").display().to_string(),
        codex_binary: "false".to_owned(),
        claude_home: dir.path().join("dot-claude"),
        codex_home: dir.path().join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
    });
    let mut client = Client::connect(host).await;
    let (session, chat) = open_session(&mut client, dir.path()).await;
    client.dispatch(&chat, turn_started("t-1", "hello")).await;
    client.actions_until(&chat, "chat/error").await;

    let restarted = host_at(dir.path());
    let mut fresh = Client::connect(restarted).await;
    fresh
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "fresh"}),
        )
        .await;
    let list = fresh
        .request("listSessions", json!({"channel": ROOT}))
        .await;
    let held = list["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["resource"] == session))
        .expect("the session survived the restart");
    let status = held["status"].as_u64().expect("status");
    assert_eq!(status & 2, 2, "the error bit survived the replay: {held}");
    assert_eq!(status & 8, 0, "nothing runs after a restart: {held}");
    assert_eq!(status & 32, 32, "restarts reset the read mark: {held}");
}

#[tokio::test]
async fn codex_streams_natively_and_resumes_its_thread() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = codex_host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (session, chat) = open_codex_session(&mut client, dir.path()).await;

    let root = client.request("subscribe", json!({"channel": ROOT})).await;
    let codex = root["snapshot"]["state"]["agents"]
        .as_array()
        .and_then(|agents| agents.iter().find(|agent| agent["provider"] == "codex"))
        .expect("Codex card");
    assert_eq!(codex["displayName"], "Codex");
    let astra = codex["models"]
        .as_array()
        .and_then(|models| models.iter().find(|model| model["id"] == "gpt-6-astra"))
        .expect("GPT-6 Astra model");
    assert_eq!(astra["name"], "GPT-6 Astra");
    assert_eq!(
        astra["configSchema"]["properties"]["thinkingLevel"]["enum"],
        json!(["low", "medium", "high", "xhigh", "max", "ultra"])
    );

    client
        .dispatch(&chat, turn_started("codex-1", "hello"))
        .await;
    let actions = client.actions_until(&chat, "chat/turnComplete").await;
    let kinds: Vec<&str> = actions
        .iter()
        .map(|action| action["type"].as_str().unwrap_or_default())
        .collect();
    assert!(kinds.contains(&"chat/responsePart"), "{kinds:?}");
    assert!(kinds.contains(&"chat/delta"), "{kinds:?}");
    assert!(kinds.contains(&"chat/usage"), "{kinds:?}");
    let text: String = actions
        .iter()
        .filter(|action| action["type"] == "chat/delta")
        .filter_map(|action| action["content"].as_str())
        .collect();
    assert_eq!(text, "OK");

    let reborn = codex_host_at(dir.path());
    let mut client = Client::connect(reborn).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test-2"}),
        )
        .await;
    let listed = client
        .request("listSessions", json!({"channel": ROOT}))
        .await;
    assert_eq!(listed["items"][0]["resource"], session, "{listed}");
    let snapshot = client
        .request("subscribe", json!({"channel": session}))
        .await;
    let resumed_chat = snapshot["snapshot"]["state"]["defaultChat"]
        .as_str()
        .expect("resumed chat");
    client
        .request("subscribe", json!({"channel": resumed_chat}))
        .await;
    client
        .dispatch(resumed_chat, turn_started("codex-2", "hello again"))
        .await;
    client
        .actions_until(resumed_chat, "chat/turnComplete")
        .await;

    let requests: Vec<Value> = std::fs::read_to_string(dir.path().join("codex-log.jsonl"))
        .expect("codex request log")
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert!(
        requests
            .iter()
            .any(|request| request["method"] == "thread/start"),
        "{requests:?}"
    );
    let resume = requests
        .iter()
        .find(|request| request["method"] == "thread/resume")
        .expect("thread/resume after host restart");
    assert_eq!(resume["params"]["threadId"], "codex-thread-1");
    let turn = requests
        .iter()
        .find(|request| request["method"] == "turn/start")
        .expect("turn/start");
    assert_eq!(turn["params"]["model"], "gpt-5.6-terra");
    assert_eq!(turn["params"]["effort"], "high");
    assert_eq!(turn["params"]["sandboxPolicy"]["type"], "workspaceWrite");
}

#[tokio::test]
async fn codex_approval_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = codex_host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (_session, chat) = open_codex_session(&mut client, dir.path()).await;

    client
        .dispatch(&chat, turn_started("codex-perm", "ask permission"))
        .await;
    let actions = client.actions_until(&chat, "chat/toolCallReady").await;
    let ready = actions.last().expect("permission card");
    assert_eq!(ready["toolCallId"], "cmd-1");
    assert!(ready["confirmed"].is_null(), "{ready}");
    client
        .dispatch(
            &chat,
            json!({
                "type": "chat/toolCallConfirmed",
                "turnId": "codex-perm",
                "toolCallId": "cmd-1",
                "approved": true,
                "confirmed": "user-action",
                "selectedOptionId": "allow",
            }),
        )
        .await;
    let actions = client.actions_until(&chat, "chat/turnComplete").await;
    assert!(
        actions
            .iter()
            .any(|action| action["type"] == "chat/toolCallComplete"),
        "{actions:?}"
    );
}

#[tokio::test]
async fn codex_cancel_interrupts_and_releases_the_next_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = codex_host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (_session, chat) = open_codex_session(&mut client, dir.path()).await;

    client
        .dispatch(&chat, turn_started("codex-park", "park here"))
        .await;
    loop {
        let action = client.next_action(&chat).await;
        if action["type"] == "chat/delta" {
            break;
        }
    }
    client
        .dispatch(
            &chat,
            json!({
                "type": "chat/turnCancelled",
                "turnId": "codex-park",
                "duration": 0,
            }),
        )
        .await;
    let action = client.next_action(&chat).await;
    assert_eq!(action["type"], "chat/turnCancelled", "{action}");

    client
        .dispatch(&chat, turn_started("codex-after", "hello"))
        .await;
    let actions = client.actions_until(&chat, "chat/turnComplete").await;
    assert_eq!(actions[0]["type"], "chat/turnStarted", "{actions:?}");
    assert!(
        actions
            .iter()
            .all(|action| action["type"] != "chat/turnCancelled"),
        "no stray cancel: {actions:?}"
    );
}

#[tokio::test]
async fn terminal_codex_threads_list_backfill_and_resume() {
    let dir = tempfile::tempdir().expect("tempdir");
    let native = "019f0eb0-8444-7383-ba6e-628509b1c33e";
    let sessions = dir.path().join("dot-codex/sessions/2026/08/31");
    std::fs::create_dir_all(&sessions).expect("codex sessions");
    std::fs::write(
        sessions.join(format!("rollout-2026-08-31T10-00-00-{native}.jsonl")),
        format!(
            concat!(
                r#"{{"timestamp":"2026-08-31T10:00:00Z","type":"session_meta","payload":{{"id":"{}","cwd":"{}","source":"cli"}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-31T10:00:01Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"<environment_context>injected</environment_context>"}}]}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-31T10:00:02Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"repair the parser"}}]}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-31T10:00:03Z","type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"Parser repaired."}}]}}}}"#,
                "\n",
            ),
            native,
            dir.path().display(),
        ),
    )
    .expect("codex transcript");

    let host = codex_host_at(dir.path());
    let mut client = Client::connect(host).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let listed = client
        .request("listSessions", json!({"channel": ROOT}))
        .await;
    let row = listed["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["provider"] == "codex")
        .expect("terminal Codex thread");
    assert_eq!(row["title"], "repair the parser");
    let session = format!("ahp-session:/codex-{native}");
    assert_eq!(row["resource"], session);

    let snapshot = client
        .request("subscribe", json!({"channel": session}))
        .await;
    let chat = snapshot["snapshot"]["state"]["defaultChat"]
        .as_str()
        .expect("default chat")
        .to_owned();
    let snapshot = client.request("subscribe", json!({"channel": chat})).await;
    let turns = snapshot["snapshot"]["state"]["turns"]
        .as_array()
        .expect("turns");
    assert_eq!(turns.len(), 1, "backfilled turn: {turns:?}");
    assert_eq!(turns[0]["message"]["text"], "repair the parser");

    client
        .dispatch(&chat, turn_started("codex-resumed", "continue"))
        .await;
    client.actions_until(&chat, "chat/turnComplete").await;
    let requests: Vec<Value> = std::fs::read_to_string(dir.path().join("codex-log.jsonl"))
        .expect("codex request log")
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let resume = requests
        .iter()
        .find(|request| request["method"] == "thread/resume")
        .expect("the terminal thread resumed");
    assert_eq!(resume["params"]["threadId"], native);
}

#[tokio::test]
async fn restart_replays_the_transcript() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (session, chat) = open_session(&mut client, dir.path()).await;
    client.dispatch(&chat, turn_started("t-1", "hello")).await;
    client.actions_until(&chat, "chat/turnComplete").await;

    let reborn = host_at(dir.path());
    let mut client = Client::connect(reborn).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let listed = client
        .request("listSessions", json!({"channel": ROOT}))
        .await;
    assert_eq!(listed["items"][0]["resource"], session, "{listed}");
    let snapshot = client.request("subscribe", json!({"channel": chat})).await;
    let turns = snapshot["snapshot"]["state"]["turns"]
        .as_array()
        .expect("turns");
    assert_eq!(turns.len(), 1, "the transcript survived the restart");
}

#[tokio::test]
async fn a_permission_ask_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (_session, chat) = open_session(&mut client, dir.path()).await;

    client
        .dispatch(&chat, turn_started("t-perm", "ask permission first"))
        .await;

    let actions = client.actions_until(&chat, "chat/toolCallReady").await;
    let ready = actions.last().expect("ready");
    assert_eq!(ready["toolCallId"], "t1");
    assert!(ready["confirmed"].is_null(), "pending, not auto");
    let options = ready["options"].as_array().expect("options");
    assert!(options.len() >= 2, "approve + deny at least");

    client
        .dispatch(
            &chat,
            json!({
                "type": "chat/toolCallConfirmed",
                "turnId": "t-perm",
                "toolCallId": "t1",
                "approved": true,
                "confirmed": "user-action",
                "selectedOptionId": "allow",
            }),
        )
        .await;
    let actions = client.actions_until(&chat, "chat/turnComplete").await;
    let kinds: Vec<&str> = actions
        .iter()
        .map(|action| action["type"].as_str().unwrap_or_default())
        .collect();
    assert!(kinds.contains(&"chat/toolCallConfirmed"), "{kinds:?}");
    assert!(kinds.contains(&"chat/toolCallComplete"), "{kinds:?}");
}

#[tokio::test]
async fn cancel_interrupts_a_parked_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (_session, chat) = open_session(&mut client, dir.path()).await;

    client
        .dispatch(&chat, turn_started("t-park", "park here"))
        .await;

    loop {
        let action = client.next_action(&chat).await;
        if action["type"] == "chat/delta" {
            break;
        }
    }

    client
        .dispatch(
            &chat,
            json!({"type": "chat/turnCancelled", "turnId": "t-park", "duration": 0}),
        )
        .await;
    let action = client.next_action(&chat).await;
    assert_eq!(action["type"], "chat/turnCancelled", "{action}");

    client
        .dispatch(&chat, turn_started("t-after", "hello"))
        .await;
    let actions = client.actions_until(&chat, "chat/turnComplete").await;
    assert_eq!(actions[0]["type"], "chat/turnStarted", "{actions:?}");
    assert!(
        actions.iter().all(|a| a["type"] != "chat/turnCancelled"),
        "no stray cancel: {actions:?}"
    );
}

#[tokio::test]
async fn the_queue_drains_on_natural_completion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (_session, chat) = open_session(&mut client, dir.path()).await;

    client
        .dispatch(&chat, turn_started("t-slow", "slow reply please"))
        .await;
    client
        .dispatch(
            &chat,
            json!({
                "type": "chat/pendingMessageSet",
                "kind": "queued",
                "id": "q-1",
                "message": {"text": "hello", "origin": {"kind": "user"}},
            }),
        )
        .await;

    let mut actions = client.actions_until(&chat, "chat/turnComplete").await;
    let second = client.actions_until(&chat, "chat/turnComplete").await;
    actions.extend(second);
    let kinds: Vec<String> = actions
        .iter()
        .map(|action| action["type"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        kinds.contains(&"chat/pendingMessageRemoved".to_owned()),
        "{kinds:?}"
    );
    let drained = actions
        .iter()
        .find(|action| action["type"] == "chat/turnStarted" && action["queuedMessageId"] == "q-1")
        .expect("the queued prompt became a turn");
    assert_eq!(drained["message"]["text"], "hello");
}

#[tokio::test]
async fn a_watch_reports_external_writes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (session, _chat) = open_session(&mut client, dir.path()).await;

    let watched = dir.path().join("watched");
    std::fs::create_dir_all(&watched).expect("watched dir");
    let created = client
        .request(
            "createResourceWatch",
            json!({
                "channel": session,
                "uri": format!("file://{}", watched.display()),
            }),
        )
        .await;
    let channel = created["channel"]
        .as_str()
        .expect("watch channel")
        .to_owned();
    client
        .request("subscribe", json!({"channel": channel}))
        .await;

    std::fs::write(watched.join("a.txt"), "hello").expect("external write");
    let action = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.next_action(&channel),
    )
    .await
    .expect("the watch fired");
    assert_eq!(action["type"], "resourceWatch/changed", "{action}");
}

#[tokio::test]
async fn a_file_edit_grows_diff_refs_served_by_resource_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (_session, chat) = open_session(&mut client, dir.path()).await;

    client
        .dispatch(&chat, turn_started("t-edit", "edit the file"))
        .await;
    let actions = client.actions_until(&chat, "chat/turnComplete").await;

    let ready = actions
        .iter()
        .find(|action| action["type"] == "chat/toolCallReady")
        .expect("the auto-approved ready");
    let path = ready["invocationMessage"].as_str().expect("invocation");
    assert!(path.ends_with("edited.txt"), "{ready}");
    assert_eq!(ready["toolInput"], json!(path), "{ready}");
    let complete = actions
        .iter()
        .find(|action| action["type"] == "chat/toolCallComplete")
        .expect("the tool completed");
    let content = complete["result"]["content"]
        .as_array()
        .expect("result content");
    let edit = content
        .iter()
        .find(|entry| entry["type"] == "fileEdit")
        .unwrap_or_else(|| panic!("a fileEdit entry: {content:?}"));
    assert!(edit["before"].is_null(), "a fresh file has no before side");
    let stored = edit["after"]["content"]["uri"]
        .as_str()
        .unwrap_or_else(|| panic!("the after ref: {edit}"));
    assert!(stored.starts_with("ahp-content:/"), "{stored}");
    assert_eq!(edit["diff"]["added"], 2, "{edit}");

    let read = client
        .request("resourceRead", json!({"channel": chat, "uri": stored}))
        .await;
    assert_eq!(read["data"], "hello\nworld\n");
}

#[tokio::test]
async fn terminal_sessions_list_and_open_with_backfilled_turns() {
    let dir = tempfile::tempdir().expect("tempdir");

    let project = dir.path().join("dot-claude/projects/-tmp-demo");
    std::fs::create_dir_all(&project).expect("project dir");
    let native = "11111111-2222-4333-8444-555555555555";
    std::fs::write(
        project.join(format!("{native}.jsonl")),
        concat!(
            r#"{"type":"user","cwd":"/tmp/demo","message":{"role":"user","content":"fix the bug"},"uuid":"u1","timestamp":"2026-08-13T10:00:00Z"}"#, "
",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Found and fixed it."}]},"uuid":"a1"}"#, "
",
        ),
    )
    .expect("jsonl");

    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let listed = client
        .request("listSessions", json!({"channel": ROOT}))
        .await;
    let row = listed["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["title"] == "fix the bug")
        .expect("the terminal session listed");
    let session = row["resource"].as_str().expect("resource").to_owned();
    assert_eq!(session, format!("ahp-session:/{native}"));

    let snapshot = client
        .request("subscribe", json!({"channel": session}))
        .await;
    let chat = snapshot["snapshot"]["state"]["defaultChat"]
        .as_str()
        .expect("default chat")
        .to_owned();
    let snapshot = client.request("subscribe", json!({"channel": chat})).await;
    let turns = snapshot["snapshot"]["state"]["turns"]
        .as_array()
        .expect("turns");
    assert_eq!(turns.len(), 1, "the backfilled turn");
    assert_eq!(turns[0]["message"]["text"], "fix the bug");
}

fn long_jsonl(dir: &Path, native: &str) {
    let project = dir.join("dot-claude/projects/-tmp-long");
    std::fs::create_dir_all(&project).expect("project dir");
    let mut lines = String::new();
    for index in 0..25 {
        lines.push_str(&format!(
            "{{\"type\":\"user\",\"cwd\":\"/tmp/long\",\"message\":{{\"role\":\"user\",\"content\":\"prompt {index}\"}},\"uuid\":\"u{index}\",\"timestamp\":\"2026-08-13T10:00:00Z\"}}\n"
        ));
        lines.push_str(&format!(
            "{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"reply {index}\"}}]}},\"uuid\":\"a{index}\"}}\n"
        ));
    }
    std::fs::write(project.join(format!("{native}.jsonl")), lines).expect("jsonl");
}

#[tokio::test]
async fn fetch_turns_pages_the_history_to_exhaustion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let native = "22222222-3333-4444-8555-666666666666";
    long_jsonl(dir.path(), native);
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let session = format!("ahp-session:/{native}");
    client
        .request("subscribe", json!({"channel": session}))
        .await;
    let chat = format!("ahp-chat:/{native}");
    let snapshot = client.request("subscribe", json!({"channel": chat})).await;
    let state = &snapshot["snapshot"]["state"];
    let turns = state["turns"].as_array().expect("turns");
    assert_eq!(turns.len(), 10, "the tail window");
    assert_eq!(
        turns[0]["message"]["text"], "prompt 15",
        "tail starts at 15"
    );
    let mut cursor = state["turnsNextCursor"]
        .as_str()
        .expect("older turns stand")
        .to_owned();

    let mut oldest;
    let mut pages = 0;
    loop {
        client
            .request("fetchTurns", json!({"channel": chat, "cursor": cursor}))
            .await;

        let action = client.next_action(&chat).await;
        assert_eq!(action["type"], "chat/turnsLoaded", "{action}");
        let turns = action["turns"].as_array().expect("page turns");
        assert!(!turns.is_empty());
        oldest = turns[0]["message"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        pages += 1;
        match action["turnsNextCursor"].as_str() {
            Some(next) => cursor = next.to_owned(),
            None => break,
        }
    }
    assert_eq!(pages, 2, "15 tail + 10 + 5");
    assert_eq!(oldest, "prompt 0", "the beginning arrived last");
}

#[tokio::test]
async fn reconnect_replays_the_missed_tail_or_answers_snapshots() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(Arc::clone(&host)).await;
    let (_session, chat) = open_session(&mut client, dir.path()).await;
    client.dispatch(&chat, turn_started("t-1", "hello")).await;
    let actions = client.actions_until(&chat, "chat/turnComplete").await;

    let last_seen = actions.len() as i64 - 2;

    let mut reborn = Client::connect(Arc::clone(&host)).await;
    reborn
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let result = reborn
        .request(
            "reconnect",
            json!({
                "channel": ROOT,
                "clientId": "test",
                "lastSeenServerSeq": last_seen,
                "subscriptions": [chat.clone()],
            }),
        )
        .await;
    assert_eq!(result["type"], "replay", "{result}");
    let replayed = result["actions"].as_array().expect("actions");
    assert!(replayed.len() >= 2, "the missed tail replays: {replayed:?}");
    assert!(replayed
        .iter()
        .all(|envelope| envelope["serverSeq"].as_i64().unwrap_or(0) > last_seen));

    let result = reborn
        .request(
            "reconnect",
            json!({
                "channel": ROOT,
                "clientId": "test",
                "lastSeenServerSeq": last_seen,
                "subscriptions": [chat.clone(), "ahp-chat:/nope"],
            }),
        )
        .await;
    assert_eq!(result["type"], "replay");
    assert_eq!(result["missing"][0], "ahp-chat:/nope", "{result}");

    client.dispatch(&chat, turn_started("t-2", "again")).await;
    let action = reborn.next_action(&chat).await;
    assert_eq!(action["type"], "chat/turnStarted", "{action}");
}

#[tokio::test]
async fn search_serves_the_session_directories() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("work");
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::create_dir_all(root.join("target")).expect("mkdir");
    std::fs::create_dir_all(root.join("docs")).expect("mkdir");
    std::fs::write(root.join("README.md"), "# readme\nconflation is delivery\n").expect("write");
    std::fs::write(root.join("docs/notes.md"), "plain notes\n").expect("write");
    std::fs::write(
        root.join("src/lib.rs"),
        "fn main() { let workspace = 1; }\n",
    )
    .expect("write");
    std::fs::write(root.join("target/ignored.md"), "conflation here too\n").expect("write");
    std::fs::write(root.join(".gitignore"), "target/\n").expect("write");

    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (session, _chat) = open_session(&mut client, &root).await;

    let result = client
        .request("search", json!({"channel": session, "query": "CONFLATION"}))
        .await;
    let hits: Vec<&str> = result["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "the .gitignored copy never answers: {hits:?}"
    );
    assert!(
        hits[0].starts_with("file://") && hits[0].ends_with("README.md"),
        "{hits:?}"
    );
    assert_eq!(result["truncated"], json!(false));

    let result = client
        .request(
            "search",
            json!({"channel": session, "query": "slib", "target": "path", "kind": "fuzzy"}),
        )
        .await;
    let hits = result["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].as_str().expect("uri").ends_with("src/lib.rs"));

    let result = client
        .request(
            "search",
            json!({"channel": session, "query": "md", "target": "path", "limit": 1}),
        )
        .await;
    assert_eq!(result["hits"].as_array().expect("hits").len(), 1);
    assert_eq!(result["truncated"], json!(true));

    let refused = client
        .request_any(
            "search",
            json!({"channel": session, "query": "x", "folders": ["file:///etc"]}),
        )
        .await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");

    let refused = client
        .request_any("search", json!({"channel": "hihost:/nope", "query": "x"}))
        .await;
    assert_eq!(refused["error"]["code"], json!(-32001), "{refused}");

    let refused = client
        .request_any(
            "search",
            json!({"channel": session, "query": "(", "kind": "regex"}),
        )
        .await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
}

#[tokio::test]
async fn pipelined_searches_answer_independently() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("work");
    std::fs::create_dir_all(&root).expect("mkdir");
    std::fs::write(root.join("one.md"), "needle\n").expect("write");
    std::fs::write(root.join("two.md"), "needle\n").expect("write");

    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (session, _chat) = open_session(&mut client, &root).await;

    client
        .send_raw(json!({"jsonrpc": "2.0", "id": 901, "method": "search",
            "params": {"channel": session, "query": "needle"}}))
        .await;
    client
        .send_raw(json!({"jsonrpc": "2.0", "id": 902, "method": "search",
            "params": {"channel": session, "query": "md", "target": "path"}}))
        .await;
    let mut answers = std::collections::HashMap::new();
    while answers.len() < 2 {
        let line = client.lines.next_line().await.expect("read").expect("open");
        let message: Value = serde_json::from_str(&line).expect("json");
        match message.get("id").and_then(Value::as_u64) {
            Some(id @ (901 | 902)) => {
                answers.insert(id, message);
            }
            _ => {}
        }
    }
    for (id, answer) in &answers {
        assert!(
            answer.get("error").is_none(),
            "search {id} failed: {answer}"
        );

        let complete = answer["result"]["hits"].as_array().expect("hits").len() == 2;
        let truncated = answer["result"]["truncated"] == json!(true);
        assert!(complete || truncated, "{answer}");
    }
}

#[tokio::test]
async fn a_terminal_echoes_resizes_and_exits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host.clone()).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    client.request("subscribe", json!({"channel": ROOT})).await;

    let terminal = "ahp-terminal:/t1";
    client
        .request(
            "createTerminal",
            json!({
                "channel": terminal,
                "claim": {"kind": "client", "clientId": "test"},
                "cwd": format!("file://{}", dir.path().display()),
                "cols": 80, "rows": 24,
            }),
        )
        .await;
    let snapshot = client
        .request("subscribe", json!({"channel": terminal}))
        .await;
    assert_eq!(snapshot["snapshot"]["state"]["isPty"], json!(true));

    let action = client.next_action(ROOT).await;
    assert_eq!(action["type"], "root/terminalsChanged", "{action}");
    assert_eq!(action["terminals"][0]["resource"], terminal);

    client
        .dispatch(
            terminal,
            json!({"type": "terminal/input", "data": "echo marker-$((40+2))\n"}),
        )
        .await;
    let mut seen = String::new();
    while !seen.contains("marker-42") {
        let action = client.next_action(terminal).await;
        if action["type"] == "terminal/data" {
            seen.push_str(action["data"].as_str().unwrap_or_default());
        }
    }

    client
        .dispatch(
            terminal,
            json!({"type": "terminal/resized", "cols": 100, "rows": 40}),
        )
        .await;
    loop {
        let action = client.next_action(terminal).await;
        if action["type"] == "terminal/resized" {
            assert_eq!(action["cols"], 100);
            break;
        }
    }
    client
        .dispatch(
            terminal,
            json!({"type": "terminal/input", "data": "stty size\n"}),
        )
        .await;
    let mut seen = String::new();
    while !seen.contains("40 100") {
        let action = client.next_action(terminal).await;
        if action["type"] == "terminal/data" {
            seen.push_str(action["data"].as_str().unwrap_or_default());
        }
    }

    let mut late = Client::connect(host.clone()).await;
    late.request(
        "initialize",
        json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "late"}),
    )
    .await;
    let snapshot = late
        .request("subscribe", json!({"channel": terminal}))
        .await;
    let retained = snapshot["snapshot"]["state"]["content"]
        .as_array()
        .expect("content parts")
        .iter()
        .filter_map(|part| part["value"].as_str())
        .collect::<String>();
    assert!(retained.contains("marker-42"), "retained: {retained:?}");

    client
        .dispatch(
            terminal,
            json!({"type": "terminal/input", "data": "exit 3\n"}),
        )
        .await;
    loop {
        let action = client.next_action(terminal).await;
        if action["type"] == "terminal/exited" {
            assert_eq!(action["exitCode"], json!(3), "{action}");
            break;
        }
    }
    loop {
        let action = client.next_action(ROOT).await;
        if action["type"] == "root/terminalsChanged" {
            if action["terminals"][0]["exitCode"] == json!(3) {
                break;
            }
        }
    }

    client
        .dispatch(
            terminal,
            json!({"type": "terminal/input", "data": "echo never\n"}),
        )
        .await;

    client
        .request("disposeTerminal", json!({"channel": terminal}))
        .await;
    loop {
        let action = client.next_action(ROOT).await;
        if action["type"] == "root/terminalsChanged" {
            assert_eq!(action["terminals"].as_array().map(Vec::len), Some(0));
            break;
        }
    }
    let refused = late
        .request_any("subscribe", json!({"channel": terminal}))
        .await;
    assert_eq!(refused["error"]["code"], json!(-32001), "{refused}");
}

#[tokio::test]
async fn an_unknown_locations_channel_answers_no_such_channel() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let refused = client
        .request_any("subscribe", json!({"channel": "ahp-locations:/nope"}))
        .await;
    assert_eq!(refused["error"]["code"], json!(-32001), "{refused}");
}

fn locations_tree(dir: &Path) -> std::path::PathBuf {
    let root = dir.join("work");
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::create_dir_all(root.join("target")).expect("mkdir");
    std::fs::write(root.join("README.md"), "# readme\nconflation is delivery\n").expect("write");
    std::fs::write(
        root.join("src/lib.rs"),
        "let conflation = conflation;\nplain\n",
    )
    .expect("write");
    std::fs::write(root.join("target/ignored.md"), "conflation here too\n").expect("write");
    std::fs::write(root.join(".gitignore"), "target/\n").expect("write");
    root
}

/// Fold `locations/extend` actions over a subscribe snapshot until
/// the channel's story resolves — the reducer of
/// docs/ahp/ahp-locations.md §2.2, exercised over the wire. The
/// snapshot may already carry any prefix of the stream (the producer
/// starts at the request, not at subscribe).
async fn drain_locations(client: &mut Client, channel: &str, mut state: Value) -> Value {
    while !state["done"].as_bool().unwrap_or(false) {
        let action = client.next_action(channel).await;
        assert_eq!(action["type"], "locations/extend", "{action}");
        if let Some(batch) = action["locations"].as_array() {
            state["locations"]
                .as_array_mut()
                .expect("locations")
                .extend(batch.iter().cloned());
        }
        for flag in ["done", "truncated"] {
            if action[flag].as_bool().unwrap_or(false) {
                state[flag] = json!(true);
            }
        }
    }
    state
}

#[tokio::test]
async fn search_locations_streams_positioned_matches() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = locations_tree(dir.path());
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (session, _chat) = open_session(&mut client, &root).await;

    let minted = client
        .request(
            "searchLocations",
            json!({"channel": session, "query": "CONFLATION"}),
        )
        .await;
    let channel = minted["channel"].as_str().expect("channel").to_owned();
    assert!(channel.starts_with("ahp-locations:/"), "{channel}");

    let subscribed = client
        .request("subscribe", json!({"channel": channel}))
        .await;
    let state = drain_locations(
        &mut client,
        &channel,
        subscribed["snapshot"]["state"].clone(),
    )
    .await;

    assert_eq!(state["truncated"], json!(false), "{state}");
    let mut locations: Vec<(String, u64, u64, u64)> = state["locations"]
        .as_array()
        .expect("locations")
        .iter()
        .map(|location| {
            (
                location["uri"].as_str().expect("uri").to_owned(),
                location["line"].as_u64().expect("line"),
                location["column"].as_u64().expect("column"),
                location["length"].as_u64().expect("length"),
            )
        })
        .collect();
    locations.sort();
    assert_eq!(locations.len(), 3, "{locations:?}");
    assert!(locations[0].0.ends_with("README.md"), "{locations:?}");
    assert_eq!((locations[0].1, locations[0].2, locations[0].3), (1, 0, 10));
    assert!(locations[1].0.ends_with("src/lib.rs"));
    assert_eq!((locations[1].1, locations[1].2), (0, 4));
    assert_eq!((locations[2].1, locations[2].2), (0, 17));
    assert!(
        !locations.iter().any(|entry| entry.0.contains("target")),
        "ignore rules hold: {locations:?}"
    );

    let context = state["locations"][0]["context"].as_str().expect("context");
    assert!(
        !context.is_empty() && state["locations"][0]["contextColumnStart"] == json!(0),
        "{state}"
    );
}

#[tokio::test]
async fn search_locations_limit_truncates_the_stream() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = locations_tree(dir.path());
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (session, _chat) = open_session(&mut client, &root).await;

    let minted = client
        .request(
            "searchLocations",
            json!({"channel": session, "query": "conflation", "limit": 1}),
        )
        .await;
    let channel = minted["channel"].as_str().expect("channel").to_owned();
    let subscribed = client
        .request("subscribe", json!({"channel": channel}))
        .await;
    let state = drain_locations(
        &mut client,
        &channel,
        subscribed["snapshot"]["state"].clone(),
    )
    .await;
    assert_eq!(state["truncated"], json!(true), "{state}");
    assert_eq!(state["locations"].as_array().expect("locations").len(), 1);
}

#[tokio::test]
async fn search_locations_refuses_bad_queries() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = locations_tree(dir.path());
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (session, _chat) = open_session(&mut client, &root).await;

    let refused = client
        .request_any(
            "searchLocations",
            json!({"channel": session, "query": "x", "kind": "fuzzy"}),
        )
        .await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");

    let refused = client
        .request_any(
            "searchLocations",
            json!({"channel": session, "query": "(", "kind": "regex"}),
        )
        .await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");

    let refused = client
        .request_any(
            "searchLocations",
            json!({"channel": "ahp-session:/nope", "query": "x"}),
        )
        .await;
    assert_eq!(refused["error"]["code"], json!(-32001), "{refused}");
}

#[tokio::test]
async fn unsubscribing_disposes_a_locations_channel() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = locations_tree(dir.path());
    let host = host_at(dir.path());
    let mut client = Client::connect(host.clone()).await;
    let (session, _chat) = open_session(&mut client, &root).await;

    let minted = client
        .request(
            "searchLocations",
            json!({"channel": session, "query": "conflation"}),
        )
        .await;
    let channel = minted["channel"].as_str().expect("channel").to_owned();
    client
        .request("subscribe", json!({"channel": channel}))
        .await;
    client
        .request("unsubscribe", json!({"channel": channel}))
        .await;

    let refused = client
        .request_any("subscribe", json!({"channel": channel}))
        .await;
    assert_eq!(
        refused["error"]["code"],
        json!(-32001),
        "the last unsubscribe disposed the channel: {refused}"
    );
}

#[tokio::test]
async fn lsp_locations_stream_contextualized_targets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_host, mut client, session, uri) = lsp_fixture(dir.path()).await;

    let minted = client
        .request(
            "lsp/locations",
            json!({"channel": session, "method": "textDocument/implementation", "params": {
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 4},
            }}),
        )
        .await;
    let channel = minted["channel"].as_str().expect("channel").to_owned();
    let subscribed = client
        .request("subscribe", json!({"channel": channel}))
        .await;
    let state = drain_locations(
        &mut client,
        &channel,
        subscribed["snapshot"]["state"].clone(),
    )
    .await;

    assert_eq!(state["truncated"], json!(false), "{state}");
    let locations = state["locations"].as_array().expect("locations");
    assert_eq!(locations.len(), 2, "{state}");
    for location in locations {
        assert_eq!(location["uri"], uri.as_str());
        assert_eq!(location["line"], 0);
        assert_eq!(location["context"], "fn answer() -> u32 { 42 }");
        assert_eq!(location["contextColumnStart"], 0);
    }
    assert_eq!(
        (
            locations[0]["column"].clone(),
            locations[0]["length"].clone()
        ),
        (json!(3), json!(6))
    );
    assert_eq!(
        (
            locations[1]["column"].clone(),
            locations[1]["length"].clone()
        ),
        (json!(21), json!(2))
    );
}

#[tokio::test]
async fn lsp_locations_context_prefers_the_mirror() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_host, mut client, session, uri) = lsp_fixture(dir.path()).await;

    let opened = client
        .request("openDocument", json!({"channel": session, "uri": uri}))
        .await;
    let document = opened["document"].as_str().expect("channel").to_owned();
    let v0 = opened["version"].as_str().expect("uid").to_owned();
    client
        .request("subscribe", json!({"channel": document}))
        .await;
    dispatch_applied(
        &mut client,
        &document,
        &v0,
        uid(0x10c5),
        insert_at(0, 0, "// hot\n"),
    )
    .await;
    let echo = client.next_action(&document).await;
    assert_eq!(echo["id"], uid(0x10c5).as_str());

    let minted = client
        .request(
            "lsp/locations",
            json!({"channel": session, "method": "textDocument/implementation", "params": {
                "textDocument": {"uri": uri},
                "position": {"line": 1, "character": 4},
            }}),
        )
        .await;
    let channel = minted["channel"].as_str().expect("channel").to_owned();
    let subscribed = client
        .request("subscribe", json!({"channel": channel}))
        .await;
    let state = drain_locations(
        &mut client,
        &channel,
        subscribed["snapshot"]["state"].clone(),
    )
    .await;

    // The fake still answers line 0 — which the UNFLUSHED edit made
    // "// hot"; a disk read would answer "fn answer() -> u32 { 42 }".
    let first = &state["locations"][0];
    assert_eq!(first["context"], "// hot", "{state}");
    assert_eq!(first["length"], 3, "the range clamps to the live line");
}

#[tokio::test]
async fn lsp_locations_refuses_foreign_methods() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_host, mut client, session, uri) = lsp_fixture(dir.path()).await;

    for method in [
        "textDocument/definition",
        "textDocument/didOpen",
        "shutdown",
    ] {
        let refused = client
            .request_any(
                "lsp/locations",
                json!({"channel": session, "method": method, "params": {
                    "textDocument": {"uri": uri},
                }}),
            )
            .await;
        assert_eq!(
            refused["error"]["code"],
            json!(-32602),
            "{method}: {refused}"
        );
    }

    let refused = client
        .request_any(
            "lsp/locations",
            json!({"channel": "ahp-session:/nope", "method": "textDocument/references", "params": {}}),
        )
        .await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
}

#[tokio::test]
async fn unsubscribing_a_lsp_locations_channel_cancels_the_ask() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_host, mut client, session, uri) = lsp_fixture(dir.path()).await;

    let diagnostics = client
        .request("lsp/diagnostics", json!({"channel": session}))
        .await;
    let diagnostics_channel = diagnostics["channel"].as_str().expect("channel").to_owned();
    client
        .request("subscribe", json!({"channel": diagnostics_channel}))
        .await;

    // The fake parks references until cancelled, then publishes a
    // "cancelled" marker diagnostic — the observable proof the
    // channel's disposal reached the language server.
    let minted = client
        .request(
            "lsp/locations",
            json!({"channel": session, "method": "textDocument/references", "params": {
                "textDocument": {"uri": uri},
                "position": {"line": 99, "character": 4},
                "context": {"includeDeclaration": true},
            }}),
        )
        .await;
    let channel = minted["channel"].as_str().expect("channel").to_owned();
    client
        .request("subscribe", json!({"channel": channel}))
        .await;
    client
        .request("unsubscribe", json!({"channel": channel}))
        .await;

    loop {
        let action = client.next_action(&diagnostics_channel).await;
        let message = action["diagnostics"][0]["message"].as_str().unwrap_or("");
        if message.starts_with("cancelled") {
            break;
        }
    }
}

#[tokio::test]
async fn a_dying_connection_reaps_its_unsubscribed_locations_channels() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = locations_tree(dir.path());
    let host = host_at(dir.path());

    let channel = {
        let mut minter = Client::connect(host.clone()).await;
        let (session, _chat) = open_session(&mut minter, &root).await;
        let minted = minter
            .request(
                "searchLocations",
                json!({"channel": session, "query": "conflation"}),
            )
            .await;
        minted["channel"].as_str().expect("channel").to_owned()
        // the minter drops here without ever subscribing
    };

    let mut witness = Client::connect(host).await;
    let mut refused = Value::Null;
    for _ in 0..50 {
        refused = witness
            .request_any("subscribe", json!({"channel": channel}))
            .await;
        if refused.get("error").is_some() {
            break;
        }
        // the close is asynchronous; re-subscribe keeps the row alive,
        // so drop it again before the next probe
        witness
            .request("unsubscribe", json!({"channel": channel}))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(refused["error"]["code"], json!(-32001), "{refused}");
}

#[tokio::test]
async fn a_changeset_channel_serves_the_folders_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    let root = root.canonicalize().expect("canonical");
    let sh = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(["-c", "core.fsmonitor=false", "-C"])
            .arg(&root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    sh(&["init", "-q"]);
    sh(&["config", "user.email", "test@example.com"]);
    sh(&["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "# readme\n\nold body\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn main() {}\n").unwrap();
    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "first"]);
    std::fs::write(root.join("README.md"), "# readme\n\nnew body\n").unwrap();
    std::fs::write(root.join("src/notes.md"), "note\n").unwrap();

    let host = host_at(dir.path());
    let mut client = Client::connect(host.clone()).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;

    let session = "hihost-fs:/local";
    let snapshot = client
        .request("subscribe", json!({"channel": session}))
        .await;
    assert_eq!(snapshot["snapshot"]["state"]["changesets"], json!([]));
    client
        .send_raw(json!({
            "jsonrpc": "2.0", "method": "dispatchAction",
            "params": {"channel": session, "action": {
                "type": "session/workingDirectorySet",
                "directory": format!("file://{}", root.display()),
            }},
        }))
        .await;
    let channel = loop {
        let action = client.next_action(session).await;
        if action["type"] == "session/changesetsChanged" {
            let list = action["changesets"].as_array().expect("the catalog");

            assert_eq!(list.len(), 2, "{action}");
            assert_eq!(list[0]["changeKind"], "uncommitted");
            assert_eq!(list[0]["label"], "Uncommitted Changes");
            assert_eq!(list[1]["changeKind"], "history");
            break list[0]["uriTemplate"]
                .as_str()
                .expect("a static uri")
                .to_owned();
        }
    };
    assert_eq!(channel, format!("hihost-changes:/{}", root.display()));
    let snapshot = client
        .request("subscribe", json!({"channel": channel}))
        .await;
    assert_eq!(snapshot["snapshot"]["state"]["status"], "computing");

    let mut files = Value::Null;
    loop {
        let action = client.next_action(&channel).await;
        if action["type"] == "changeset/contentChanged" {
            files = action["files"].clone();
        }
        if action["type"] == "changeset/statusChanged" && action["status"] == "ready" {
            break;
        }
    }
    let listed = files.as_array().expect("files");
    assert_eq!(listed.len(), 2, "{files}");
    let readme = &listed[0];
    assert!(readme["id"].as_str().unwrap().ends_with("README.md"));
    assert_eq!(readme["edit"]["diff"], json!({"added": 1, "removed": 1}));
    let before_ref = readme["edit"]["before"]["content"]["uri"]
        .as_str()
        .expect("a before ref")
        .to_owned();
    let notes = &listed[1];
    assert!(notes["id"].as_str().unwrap().ends_with("src/notes.md"));
    assert!(
        notes["edit"]["before"].is_null(),
        "untracked has no before: {notes}"
    );

    let read = client
        .request(
            "resourceRead",
            json!({"channel": channel, "uri": before_ref}),
        )
        .await;
    assert_eq!(read["data"], "# readme\n\nold body\n");

    client
        .request(
            "resourceWrite",
            json!({
                "channel": channel,
                "uri": format!("file://{}", root.join("src/extra.md").display()),
                "data": "extra\n", "encoding": "utf-8",
            }),
        )
        .await;
    loop {
        let action = client.next_action(&channel).await;
        if action["type"] == "changeset/contentChanged"
            && action["files"]
                .as_array()
                .is_some_and(|files| files.len() == 3)
        {
            break;
        }
    }

    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "second"]);
    loop {
        let action = client.next_action(&channel).await;
        if action["type"] == "changeset/contentChanged"
            && action["files"].as_array().is_some_and(Vec::is_empty)
        {
            break;
        }
    }

    client
        .request("unsubscribe", json!({"channel": channel}))
        .await;

    let plain = dir.path().join("plain");
    std::fs::create_dir_all(&plain).expect("mkdir");
    let plain_abs = plain.canonicalize().expect("canonical");
    client
        .send_raw(json!({
            "jsonrpc": "2.0", "method": "dispatchAction",
            "params": {"channel": session, "action": {
                "type": "session/workingDirectorySet",
                "directory": format!("file://{}", plain_abs.display()),
            }},
        }))
        .await;
    let plain_channel = loop {
        let action = client.next_action(session).await;
        if action["type"] == "session/changesetsChanged" {
            let list = action["changesets"].as_array().expect("the catalog");
            let uncommitted: Vec<&Value> = list
                .iter()
                .filter(|entry| entry["changeKind"] == "uncommitted")
                .collect();
            assert_eq!(uncommitted.len(), 2, "{action}");
            break uncommitted[1]["uriTemplate"]
                .as_str()
                .expect("a static uri")
                .to_owned();
        }
    };
    client
        .request("subscribe", json!({"channel": plain_channel}))
        .await;
    loop {
        let action = client.next_action(&plain_channel).await;
        if action["type"] == "changeset/statusChanged" {
            assert_eq!(action["status"], "error", "{action}");
            break;
        }
    }

    client
        .send_raw(json!({
            "jsonrpc": "2.0", "method": "dispatchAction",
            "params": {"channel": session, "action": {
                "type": "session/workingDirectoryRemoved",
                "directory": format!("file://{}", root.display()),
            }},
        }))
        .await;
    loop {
        let action = client.next_action(session).await;
        if action["type"] == "session/changesetsChanged" {
            let list = action["changesets"].as_array().expect("the catalog");
            let uncommitted: Vec<&Value> = list
                .iter()
                .filter(|entry| entry["changeKind"] == "uncommitted")
                .collect();
            assert_eq!(uncommitted.len(), 1, "{action}");
            assert!(
                uncommitted[0]["uriTemplate"]
                    .as_str()
                    .unwrap()
                    .ends_with("plain"),
                "{action}"
            );
            break;
        }
    }
}

#[tokio::test]
async fn a_non_repo_changeset_answers_an_error_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host.clone()).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let plain = dir.path().join("plain");
    std::fs::create_dir_all(&plain).expect("mkdir");
    let session = "hihost-fs:/local";
    client
        .request("subscribe", json!({"channel": session}))
        .await;
    client
        .send_raw(json!({
            "jsonrpc": "2.0", "method": "dispatchAction",
            "params": {"channel": session, "action": {
                "type": "session/workingDirectorySet",
                "directory": format!(
                    "file://{}",
                    plain.canonicalize().expect("canonical").display()
                ),
            }},
        }))
        .await;
    let channel = loop {
        let action = client.next_action(session).await;
        if action["type"] == "session/changesetsChanged" {
            break action["changesets"][0]["uriTemplate"]
                .as_str()
                .expect("a static uri")
                .to_owned();
        }
    };
    client
        .request("subscribe", json!({"channel": channel}))
        .await;
    loop {
        let action = client.next_action(&channel).await;
        if action["type"] == "changeset/statusChanged" {
            assert_eq!(action["status"], "error", "{action}");
            break;
        }
    }
}

fn annotation_set(id: &str, resource: &str, text: &str) -> Value {
    json!({
        "type": "annotations/set",
        "annotation": {
            "id": id,
            "turnId": "",
            "resource": resource,
            "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 4}},
            "resolved": false,
            "entries": [{"id": format!("{id}-e1"), "text": {"markdown": text},
                         "_meta": {"himark": {"author": "user"}}}],
        },
    })
}

#[tokio::test]
async fn annotations_fold_echo_and_count() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(Arc::clone(&host)).await;
    let (session, _chat) = open_session(&mut client, dir.path()).await;
    let channel = format!("{session}/annotations");
    client.request("subscribe", json!({"channel": ROOT})).await;
    let snapshot = client
        .request("subscribe", json!({"channel": channel}))
        .await;
    assert_eq!(
        snapshot["snapshot"]["state"]["annotations"],
        json!([]),
        "{snapshot}"
    );

    let file = format!("file://{}/src/main.rs", dir.path().display());
    client
        .dispatch(&channel, annotation_set("a-1", &file, "why unwrap?"))
        .await;
    let echo = client.next_action(&channel).await;
    assert_eq!(echo["type"], "annotations/set");
    assert_eq!(echo["annotation"]["id"], "a-1");
    let changed = client.next_notification("root/sessionSummaryChanged").await;
    assert_eq!(changed["session"], session, "{changed}");
    assert_eq!(changed["changes"]["annotations"]["annotationCount"], 1);
    assert_eq!(changed["changes"]["annotations"]["entryCount"], 1);
    assert_eq!(changed["changes"]["annotations"]["resource"], channel);

    client
        .dispatch(
            &channel,
            json!({"type": "annotations/entrySet", "annotationId": "a-1",
                   "entry": {"id": "a-1-e2", "text": "agreed",
                             "_meta": {"himark": {"author": "agent"}}}}),
        )
        .await;
    client.next_action(&channel).await;
    client
        .dispatch(
            &channel,
            json!({"type": "annotations/updated", "annotationId": "a-1", "resolved": true}),
        )
        .await;
    client.next_action(&channel).await;

    let mut other = Client::connect(Arc::clone(&host)).await;
    other
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "other"}),
        )
        .await;
    let snapshot = other
        .request("subscribe", json!({"channel": channel}))
        .await;
    let annotations = snapshot["snapshot"]["state"]["annotations"]
        .as_array()
        .expect("annotations");
    assert_eq!(annotations.len(), 1);
    assert_eq!(annotations[0]["resolved"], true);
    assert_eq!(
        annotations[0]["entries"].as_array().expect("entries").len(),
        2
    );

    other
        .dispatch(
            &channel,
            json!({"type": "annotations/entryRemoved", "annotationId": "a-1",
                   "entryId": "a-1-e2"}),
        )
        .await;
    let echo = client.next_action(&channel).await;
    assert_eq!(echo["type"], "annotations/entryRemoved", "{echo}");
    client
        .dispatch(
            &channel,
            json!({"type": "annotations/removed", "annotationId": "a-1"}),
        )
        .await;
    let echo = client.next_action(&channel).await;
    assert_eq!(echo["type"], "annotations/removed");
    let changed = client.next_notification("root/sessionSummaryChanged").await;
    let changed = if changed["changes"]["annotations"]["annotationCount"] == 1 {
        client.next_notification("root/sessionSummaryChanged").await
    } else {
        changed
    };
    assert_eq!(
        changed["changes"]["annotations"]["annotationCount"], 0,
        "{changed}"
    );
}

#[tokio::test]
async fn empty_annotations_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (session, _chat) = open_session(&mut client, dir.path()).await;
    let channel = format!("{session}/annotations");
    client
        .request("subscribe", json!({"channel": channel}))
        .await;
    let file = format!("file://{}/src/main.rs", dir.path().display());

    client
        .dispatch(
            &channel,
            json!({"type": "annotations/set",
                   "annotation": {"id": "a-empty", "turnId": "", "resource": file,
                                   "resolved": false, "entries": []}}),
        )
        .await;

    client
        .dispatch(&channel, annotation_set("a-1", &file, "note"))
        .await;
    let echo = client.next_action(&channel).await;
    assert_eq!(
        echo["annotation"]["id"], "a-1",
        "the empty set never echoed: {echo}"
    );
    client
        .dispatch(
            &channel,
            json!({"type": "annotations/entryRemoved", "annotationId": "a-1",
                   "entryId": "a-1-e1"}),
        )
        .await;
    client
        .dispatch(
            &channel,
            json!({"type": "annotations/updated", "annotationId": "a-1", "resolved": true}),
        )
        .await;
    let echo = client.next_action(&channel).await;
    assert_eq!(
        echo["type"], "annotations/updated",
        "the last-entry removal never echoed: {echo}"
    );
}

#[tokio::test]
async fn annotations_survive_restart_and_reconnect() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(Arc::clone(&host)).await;
    let (session, _chat) = open_session(&mut client, dir.path()).await;
    let channel = format!("{session}/annotations");
    client
        .request("subscribe", json!({"channel": channel}))
        .await;
    let file = format!("file://{}/src/main.rs", dir.path().display());
    client
        .dispatch(&channel, annotation_set("a-1", &file, "first"))
        .await;
    client.next_action(&channel).await;
    client
        .dispatch(&channel, annotation_set("a-2", &file, "second"))
        .await;
    client.next_action(&channel).await;

    let mut reborn = Client::connect(Arc::clone(&host)).await;
    reborn
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let result = reborn
        .request(
            "reconnect",
            json!({"channel": ROOT, "clientId": "test", "lastSeenServerSeq": 0,
                   "subscriptions": [channel.clone()]}),
        )
        .await;
    if result["type"] == "replay" {
        assert!(
            result["actions"]
                .as_array()
                .expect("actions")
                .iter()
                .any(|envelope| envelope["action"]["type"] == "annotations/set"),
            "{result}"
        );
        assert_eq!(result["missing"], json!([]), "{result}");
    } else {
        assert!(result["snapshots"]
            .as_array()
            .expect("snapshots")
            .iter()
            .any(|snapshot| snapshot["resource"] == channel));
    }

    let reborn_host = host_at(dir.path());
    let mut client = Client::connect(reborn_host).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let snapshot = client
        .request("subscribe", json!({"channel": channel}))
        .await;
    let annotations = snapshot["snapshot"]["state"]["annotations"]
        .as_array()
        .expect("annotations");
    assert_eq!(annotations.len(), 2, "annotations survived the restart");
    let listed = client
        .request("listSessions", json!({"channel": ROOT}))
        .await;
    let row = listed["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["resource"] == session)
        .expect("the session lists");
    assert_eq!(row["annotations"]["annotationCount"], 2, "{row}");
}

#[tokio::test]
async fn local_fs_annotations_survive_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let channel = "hihost-fs:/local/annotations";
    client
        .request("subscribe", json!({"channel": channel}))
        .await;
    let file = format!("file://{}/notes.md", dir.path().display());
    client
        .dispatch(channel, annotation_set("a-local", &file, "local comment"))
        .await;
    client.next_action(channel).await;

    let reborn = host_at(dir.path());
    let mut client = Client::connect(reborn).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let snapshot = client
        .request("subscribe", json!({"channel": channel}))
        .await;
    let annotations = snapshot["snapshot"]["state"]["annotations"]
        .as_array()
        .expect("annotations");
    assert_eq!(annotations.len(), 1, "local comments survived the restart");

    let listed = client
        .request("listSessions", json!({"channel": ROOT}))
        .await;
    assert!(
        listed["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["resource"] != "hihost-fs:/local"),
        "{listed}"
    );
}

fn uid(n: u128) -> String {
    format!("{n:032x}")
}

async fn open_standalone(client: &mut Client, text: &str) -> (String, String) {
    let opened = client
        .request(
            "openDocument",
            json!({"channel": "hihost-fs:/local", "text": text}),
        )
        .await;
    (
        opened["document"].as_str().expect("a channel").to_owned(),
        opened["version"]
            .as_str()
            .expect("a version uid")
            .to_owned(),
    )
}

fn insert_at(line: u64, character: u64, text: &str) -> Value {
    json!({"replacements": [{
        "range": {"start": {"line": line, "character": character},
                   "end": {"line": line, "character": character}},
        "text": text,
    }]})
}

async fn dispatch_applied(
    client: &mut Client,
    channel: &str,
    base: impl AsRef<str>,
    id: impl AsRef<str>,
    operation: Value,
) {
    client
        .send_raw(json!({
            "jsonrpc": "2.0", "method": "dispatchAction",
            "params": {"channel": channel, "action": {
                "type": "document/applied",
                "base": base.as_ref(), "id": id.as_ref(), "operation": operation,
            }},
        }))
        .await;
}

#[tokio::test]
async fn a_document_channel_applies_chained_operations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host.clone()).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let (channel, v0) = open_standalone(&mut client, "hello\nworld\n").await;
    let snapshot = client
        .request("subscribe", json!({"channel": channel}))
        .await;
    assert_eq!(snapshot["snapshot"]["state"]["text"], "hello\nworld\n");
    assert_eq!(snapshot["snapshot"]["state"]["version"], v0.as_str());

    dispatch_applied(&mut client, &channel, &v0, uid(0xc1), insert_at(0, 5, "!")).await;
    dispatch_applied(
        &mut client,
        &channel,
        uid(0xc1),
        uid(0xc2),
        insert_at(1, 0, ">"),
    )
    .await;

    let first = client.next_action(&channel).await;
    assert_eq!(first["id"], uid(0xc1).as_str());
    assert_eq!(first["base"], v0.as_str());
    let second = client.next_action(&channel).await;
    assert_eq!(second["id"], uid(0xc2).as_str());
    assert_eq!(second["base"], uid(0xc1).as_str());

    let mut other = Client::connect(host.clone()).await;
    other
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "other"}),
        )
        .await;
    let snapshot = other
        .request("subscribe", json!({"channel": channel}))
        .await;
    assert_eq!(snapshot["snapshot"]["state"]["text"], "hello!\n>world\n");
    assert_eq!(snapshot["snapshot"]["state"]["version"], uid(0xc2).as_str());
}

#[tokio::test]
async fn conflicting_document_edits_converge_by_rebase() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut alice = Client::connect(host.clone()).await;
    alice
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "alice"}),
        )
        .await;
    let mut bob = Client::connect(host.clone()).await;
    bob.request(
        "initialize",
        json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "bob"}),
    )
    .await;

    let (channel, v0) = open_standalone(&mut alice, "shared\n").await;
    alice
        .request("subscribe", json!({"channel": channel}))
        .await;
    bob.request("subscribe", json!({"channel": channel})).await;

    dispatch_applied(&mut alice, &channel, &v0, uid(0xa1), insert_at(0, 0, "A")).await;
    let echo = alice.next_action(&channel).await;
    assert_eq!(echo["id"], uid(0xa1).as_str());
    dispatch_applied(&mut bob, &channel, &v0, uid(0xb1), insert_at(0, 0, "B")).await;

    let foreign = bob.next_action(&channel).await;
    assert_eq!(
        foreign["id"],
        uid(0xa1).as_str(),
        "the winner Bob must rebase over"
    );
    assert_eq!(foreign["base"], v0.as_str());
    dispatch_applied(
        &mut bob,
        &channel,
        uid(0xa1),
        uid(0xb1),
        insert_at(0, 1, "B"),
    )
    .await;

    let applied = bob.next_action(&channel).await;
    assert_eq!(applied["id"], uid(0xb1).as_str());
    assert_eq!(applied["base"], uid(0xa1).as_str());
    let seen_by_alice = alice.next_action(&channel).await;
    assert_eq!(seen_by_alice["id"], uid(0xb1).as_str());

    let mut carol = Client::connect(host.clone()).await;
    carol
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "carol"}),
        )
        .await;
    let snapshot = carol
        .request("subscribe", json!({"channel": channel}))
        .await;
    assert_eq!(snapshot["snapshot"]["state"]["text"], "ABshared\n");
    assert_eq!(snapshot["snapshot"]["state"]["version"], uid(0xb1).as_str());
}

#[tokio::test]
async fn stale_and_malformed_document_dispatches_discard() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host.clone()).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let (channel, v0) = open_standalone(&mut client, "abc\n").await;
    client
        .request("subscribe", json!({"channel": channel}))
        .await;

    dispatch_applied(
        &mut client,
        &channel,
        uid(0x999),
        uid(0x51a1e),
        insert_at(0, 0, "X"),
    )
    .await;

    dispatch_applied(
        &mut client,
        &channel,
        &v0,
        uid(0x51a2e),
        json!({"replacements": [
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "text": "y"},
            {"range": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 3}}, "text": "z"},
        ]}),
    )
    .await;

    dispatch_applied(
        &mut client,
        &channel,
        &v0,
        uid(0x600d),
        insert_at(0, 3, "!"),
    )
    .await;
    let action = client.next_action(&channel).await;
    assert_eq!(action["id"], uid(0x600d).as_str());
    assert_eq!(action["base"], v0.as_str());
}

#[tokio::test]
async fn a_mirrored_document_opens_idempotently_and_disposes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("notes.md");
    std::fs::write(&file, "# notes\n").unwrap();
    let host = host_at(dir.path());
    let mut client = Client::connect(host.clone()).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let uri = format!("file://{}", file.display());
    let opened = client
        .request(
            "openDocument",
            json!({"channel": "hihost-fs:/local", "uri": uri}),
        )
        .await;
    let channel = opened["document"].as_str().expect("a channel").to_owned();
    let v0 = opened["version"].as_str().expect("a version").to_owned();
    let snapshot = client
        .request("subscribe", json!({"channel": channel}))
        .await;
    assert_eq!(snapshot["snapshot"]["state"]["text"], "# notes\n");
    assert_eq!(snapshot["snapshot"]["state"]["uri"], uri.as_str());

    dispatch_applied(&mut client, &channel, &v0, uid(0xe1), insert_at(0, 7, "!")).await;
    let echo = client.next_action(&channel).await;
    assert_eq!(echo["id"], uid(0xe1).as_str());
    let reopened = client
        .request(
            "openDocument",
            json!({"channel": "hihost-fs:/local", "uri": uri}),
        )
        .await;
    assert_eq!(reopened["document"], channel.as_str());
    assert_eq!(reopened["version"], uid(0xe1).as_str());

    client
        .request("unsubscribe", json!({"channel": channel}))
        .await;
    let refused = client
        .request_any("subscribe", json!({"channel": channel}))
        .await;
    assert!(refused["error"]["code"].is_i64(), "disposed: {refused}");
    let fresh = client
        .request(
            "openDocument",
            json!({"channel": "hihost-fs:/local", "uri": uri}),
        )
        .await;
    assert_ne!(fresh["document"], channel.as_str(), "a fresh document");
}

async fn lsp_fixture(dir: &Path) -> (Arc<agent_host::Host>, Client, String, String) {
    let root = dir.join("code");
    std::fs::create_dir_all(&root).expect("mkdir");
    let root = root.canonicalize().expect("canonical");
    std::fs::write(root.join("lib.rs"), "fn answer() -> u32 { 42 }\n").unwrap();
    let host = agent_host::Host::new(agent_host::HostConfig {
        agents: Vec::new(),
        data_dir: dir.join("data"),
        claude_binary: "false".to_owned(),
        codex_binary: "false".to_owned(),
        claude_home: dir.join("dot-claude"),
        codex_home: dir.join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: vec![agent_host::LanguageServer {
            extensions: vec!["rs".to_owned()],
            command: agent_host::testing::fake_ls_command(dir),
        }],
    });
    let mut client = Client::connect(host.clone()).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let session = "hihost-fs:/local".to_owned();
    client
        .request("subscribe", json!({"channel": session}))
        .await;
    client
        .send_raw(json!({
            "jsonrpc": "2.0", "method": "dispatchAction",
            "params": {"channel": session, "action": {
                "type": "session/workingDirectorySet",
                "directory": format!("file://{}", root.display()),
            }},
        }))
        .await;
    let uri = format!("file://{}", root.join("lib.rs").display());
    (host, client, session, uri)
}

#[tokio::test]
async fn lsp_requests_route_and_answer_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_host, mut client, session, uri) = lsp_fixture(dir.path()).await;

    let result = client
        .request(
            "lsp/textDocument/definition",
            json!({"channel": session, "params": {
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 3},
            }}),
        )
        .await;
    assert_eq!(result["uri"], uri.as_str(), "{result}");
    assert_eq!(result["range"]["start"]["line"], 1);
    assert_eq!(result["range"]["start"]["character"], 2);

    let refused = client
        .request_any(
            "lsp/textDocument/didOpen",
            json!({"channel": session, "params": {}}),
        )
        .await;
    assert_eq!(refused["error"]["code"], -33001, "{refused}");

    let unserved = client
        .request_any(
            "lsp/textDocument/hover",
            json!({"channel": session, "params": {
                "textDocument": {"uri": uri.replace("lib.rs", "notes.md")},
                "position": {"line": 0, "character": 0},
            }}),
        )
        .await;
    assert_eq!(unserved["error"]["code"], -33002, "{unserved}");

    let capabilities = client
        .request("lsp/capabilities", json!({"channel": session, "uri": uri}))
        .await;
    assert_eq!(
        capabilities["capabilities"]["definitionProvider"], true,
        "{capabilities}"
    );
}

#[tokio::test]
async fn channel_documents_feed_the_language_server_and_diagnostics_flow() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_host, mut client, session, uri) = lsp_fixture(dir.path()).await;

    let diagnostics = client
        .request("lsp/diagnostics", json!({"channel": session}))
        .await;
    let diagnostics_channel = diagnostics["channel"]
        .as_str()
        .expect("a channel")
        .to_owned();
    client
        .request("subscribe", json!({"channel": diagnostics_channel}))
        .await;

    let opened = client
        .request("openDocument", json!({"channel": session, "uri": uri}))
        .await;
    let document = opened["document"].as_str().expect("channel").to_owned();
    let v0 = opened["version"].as_str().expect("uid").to_owned();
    client
        .request("subscribe", json!({"channel": document}))
        .await;
    let action = client.next_action(&diagnostics_channel).await;
    assert_eq!(action["type"], "lspDiagnostics/published");
    assert_eq!(action["uri"], uri.as_str());
    assert_eq!(action["version"], v0.as_str(), "the DOCUMENT uid tags it");
    assert!(
        action["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("fn answer"),
        "the didOpen carried the file text: {action}"
    );

    dispatch_applied(
        &mut client,
        &document,
        &v0,
        uid(0xed17),
        insert_at(0, 0, "// hot\n"),
    )
    .await;
    let echo = client.next_action(&document).await;
    assert_eq!(echo["id"], uid(0xed17).as_str());
    let action = client.next_action(&diagnostics_channel).await;
    assert_eq!(action["version"], uid(0xed17).as_str(), "{action}");
    assert!(
        action["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("// hot"),
        "the didChange carried the operation's text: {action}"
    );

    let snapshot = client
        .request("subscribe", json!({"channel": diagnostics_channel}))
        .await;
    let held = &snapshot["snapshot"]["state"]["items"][uri.as_str()];
    assert_eq!(held["version"], uid(0xed17).as_str(), "{snapshot}");
}

#[tokio::test]
async fn lsp_cancellation_reaches_the_language_server() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_host, mut client, session, uri) = lsp_fixture(dir.path()).await;

    client
        .request(
            "lsp/textDocument/definition",
            json!({"channel": session, "params": {
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 0},
            }}),
        )
        .await;

    let parked_id = client.next_id + 1;
    client
        .send_raw(json!({
            "jsonrpc": "2.0", "id": parked_id, "method": "lsp/textDocument/references",
            "params": {"channel": session, "params": {
                "textDocument": {"uri": uri},
                "position": {"line": 99, "character": 3},
                "context": {"includeDeclaration": true},
            }},
        }))
        .await;
    client.next_id = parked_id;
    client
        .send_raw(json!({
            "jsonrpc": "2.0", "method": "lsp/$/cancelRequest",
            "params": {"id": parked_id},
        }))
        .await;
    let cancelled = client.response_for(parked_id).await;
    assert_eq!(cancelled["error"]["code"], -32800, "{cancelled}");
}

#[tokio::test]
async fn session_config_resolves_and_creation_honors_it() {
    let dir = tempfile::tempdir().expect("tempdir");

    let host = agent_host::Host::new(agent_host::HostConfig {
        data_dir: dir.path().join("data"),
        claude_binary: agent_host::testing::fake_cli_command(dir.path()),
        claude_home: dir.path().join("dot-claude"),
        codex_home: dir.path().join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
        ..agent_host::HostConfig::default()
    });
    let mut client = Client::connect(host).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;

    let resolved = client
        .request(
            "resolveSessionConfig",
            json!({
                "channel": "ahp-session:/resolve-probe",
                "provider": "claude",
                "config": {"permissionMode": "plan", "worktree": "not-a-bool"},
            }),
        )
        .await;
    let schema = &resolved["schema"]["properties"];
    assert_eq!(schema["permissionMode"]["type"], "string");
    assert_eq!(
        schema["permissionMode"]["enumLabels"][0], "Ask first",
        "EDITS renders the labels"
    );
    assert_eq!(schema["worktree"]["type"], "boolean");
    let values = &resolved["values"];
    assert_eq!(values["permissionMode"], "plan", "a valid choice survives");
    assert_eq!(values["mode"], "agent", "unset answers the default");
    assert_eq!(
        values["worktree"], false,
        "an invalid value answers the default"
    );

    let completions = client
        .request(
            "sessionConfigCompletions",
            json!({
                "channel": "ahp-session:/resolve-probe",
                "provider": "claude",
                "property": "mode",
            }),
        )
        .await;
    assert_eq!(completions["items"], json!([]));

    let subscribed = client.request("subscribe", json!({"channel": ROOT})).await;
    let models = &subscribed["snapshot"]["state"]["agents"][0]["models"];
    assert!(
        models.as_array().is_some_and(|models| !models.is_empty()),
        "the claude card advertises models: {models}"
    );
    assert!(
        models[0]["configSchema"]["properties"]["thinkingLevel"]["enum"]
            .as_array()
            .is_some_and(|levels| levels.len() == 4),
        "each model carries the EFFORT schema"
    );

    let session = format!("ahp-session:/options-{}", std::process::id());
    client
        .request(
            "createSession",
            json!({
                "channel": session,
                "provider": "claude",
                "workingDirectories": [format!("file://{}", dir.path().display())],
                "config": {"isolation": "folder", "permissionMode": "acceptEdits", "worktree": true},
                "model": {"id": "claude-fable-5", "config": {"thinkingLevel": "max"}},
            }),
        )
        .await;
    let snapshot = client
        .request("subscribe", json!({"channel": session}))
        .await;
    let config = &snapshot["snapshot"]["state"]["config"]["values"];
    assert_eq!(config["permissionMode"], "acceptEdits");
    assert_eq!(config["worktree"], true);
}

#[tokio::test]
async fn a_dirless_session_mutates_mid_flight() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let session = format!("ahp-session:/dirless-{}", std::process::id());
    client
        .request(
            "createSession",
            json!({
                "channel": session,
                "provider": "claude",
                "config": {"permissionMode": "default"},
                "model": {"id": "stub-one", "config": {"thinkingLevel": "low"}},
            }),
        )
        .await;
    let snapshot = client
        .request("subscribe", json!({"channel": session}))
        .await;
    assert_eq!(
        snapshot["snapshot"]["state"]["workingDirectories"],
        json!([]),
        "born without a directory"
    );
    let chat = snapshot["snapshot"]["state"]["defaultChat"]
        .as_str()
        .expect("default chat")
        .to_owned();
    client.request("subscribe", json!({"channel": chat})).await;

    let spawns = |dir: &Path| -> Vec<Value> {
        std::fs::read_to_string(dir.join("spawn-log.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("spawn log json"))
            .collect()
    };
    let argv = |spawn: &Value| -> Vec<String> {
        spawn["argv"]
            .as_array()
            .expect("argv")
            .iter()
            .map(|arg| arg.as_str().unwrap_or_default().to_owned())
            .collect()
    };

    client.dispatch(&chat, turn_started("t-1", "hello")).await;
    client.actions_until(&chat, "chat/turnComplete").await;
    let log = spawns(dir.path());
    assert_eq!(log.len(), 1, "one spawn");
    let first = argv(&log[0]);
    assert!(
        first.windows(2).any(|w| w == ["--model", "stub-one"]),
        "{first:?}"
    );
    assert!(
        first
            .windows(2)
            .any(|w| w == ["--permission-mode", "default"]),
        "{first:?}"
    );
    assert!(!first.iter().any(|arg| arg == "--add-dir"), "{first:?}");
    assert_eq!(log[0]["max_thinking_tokens"], "4096");
    let temp_cwd = log[0]["cwd"].as_str().expect("cwd").to_owned();

    let granted = dir.path().join("work");
    std::fs::create_dir_all(&granted).expect("workdir");
    let granted_uri = format!("file://{}", granted.display());
    client
        .dispatch(
            &session,
            json!({"type": "session/workingDirectorySet", "directory": granted_uri}),
        )
        .await;
    client.next_action(&session).await;
    client.dispatch(&chat, turn_started("t-2", "hello")).await;
    client.actions_until(&chat, "chat/turnComplete").await;
    let log = spawns(dir.path());
    assert_eq!(log.len(), 2, "the grant retired the agent");
    let second = argv(&log[1]);
    assert!(
        second
            .windows(2)
            .any(|w| w[0] == "--add-dir" && w[1] == granted.display().to_string()),
        "{second:?}"
    );
    assert!(
        second.iter().any(|arg| arg.starts_with("--resume=")),
        "the conversation continues: {second:?}"
    );
    assert_eq!(log[1]["cwd"], temp_cwd, "the cwd is frozen");

    client
        .dispatch(
            &session,
            json!({"type": "session/configChanged", "config": {"permissionMode": "acceptEdits"}}),
        )
        .await;
    client.next_action(&session).await;
    client.dispatch(&chat, turn_started("t-3", "hello")).await;
    client.actions_until(&chat, "chat/turnComplete").await;
    let log = spawns(dir.path());
    assert_eq!(log.len(), 3);
    let third = argv(&log[2]);
    assert!(
        third
            .windows(2)
            .any(|w| w == ["--permission-mode", "acceptEdits"]),
        "{third:?}"
    );

    client
        .dispatch(
            &chat,
            json!({
                "type": "chat/turnStarted",
                "turnId": "t-4",
                "startedAt": "2026-08-22T12:00:00.000Z",
                "message": {
                    "text": "hello",
                    "origin": {"kind": "user"},
                    "model": {"id": "stub-two", "config": {"thinkingLevel": "high"}},
                },
            }),
        )
        .await;
    client.actions_until(&chat, "chat/turnComplete").await;
    let log = spawns(dir.path());
    assert_eq!(log.len(), 4);
    let fourth = argv(&log[3]);
    assert!(
        fourth.windows(2).any(|w| w == ["--model", "stub-two"]),
        "{fourth:?}"
    );
    assert_eq!(log[3]["max_thinking_tokens"], "24576");

    let sessions_dir = dir.path().join("data").join("sessions");
    let manifest_path = std::fs::read_dir(&sessions_dir)
        .expect("sessions dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("session.json"))
        .find(|path| path.exists())
        .expect("a persisted manifest");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(manifest_path).expect("manifest read"))
            .expect("manifest json");
    assert_eq!(manifest["workingDirectories"], json!([granted_uri]));
    assert_eq!(manifest["permissionMode"], "acceptEdits");
    assert_eq!(manifest["model"], "stub-two");
    assert_eq!(manifest["thinkingLevel"], "high");
    assert_eq!(manifest["primary"], "", "dirless birth pins the temp cwd");
}

#[tokio::test]
async fn a_history_channel_serves_the_commit_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root).expect("mkdir");
    let root = root.canonicalize().expect("canonical");
    let sh = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(["-c", "core.fsmonitor=false", "-C"])
            .arg(&root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    sh(&["init", "-q"]);
    sh(&["config", "user.email", "test@example.com"]);
    sh(&["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "one\n").unwrap();
    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "first"]);
    std::fs::write(root.join("README.md"), "one\ntwo\n").unwrap();
    sh(&["commit", "-q", "-am", "second"]);

    for extra in 0..101 {
        sh(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            &format!("filler {extra}"),
        ]);
    }

    let host = host_at(dir.path());
    let mut client = Client::connect(host.clone()).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let session = "hihost-fs:/local";
    let _ = client
        .request("subscribe", json!({"channel": session}))
        .await;
    client
        .send_raw(json!({
            "jsonrpc": "2.0", "method": "dispatchAction",
            "params": {"channel": session, "action": {
                "type": "session/workingDirectorySet",
                "directory": format!("file://{}", root.display()),
            }},
        }))
        .await;
    let channel = loop {
        let action = client.next_action(session).await;
        if action["type"] == "session/changesetsChanged" {
            let list = action["changesets"].as_array().expect("the catalog");
            let history = list
                .iter()
                .find(|entry| entry["changeKind"] == "history")
                .expect("a history entry");
            assert_eq!(history["label"], "History");
            break history["uriTemplate"].as_str().expect("static").to_owned();
        }
    };
    assert_eq!(channel, format!("hihost-history:/{}", root.display()));

    let snapshot = client
        .request("subscribe", json!({"channel": channel}))
        .await;
    assert_eq!(snapshot["snapshot"]["state"]["status"], "computing");
    let state = loop {
        let action = client.next_action(&channel).await;
        if action["type"] == "history/reset" {
            break action["state"].clone();
        }
    };
    assert_eq!(state["status"], "ready");
    let commits = state["commits"].as_array().expect("the window");
    assert_eq!(commits.len(), 100, "the window caps at one page");
    assert_eq!(state["more"], "100");
    assert_eq!(commits[0]["summary"], "filler 100", "newest first");
    assert!(state["head"]["branch"].is_string());
    let newest_refs = commits[0]["refs"].as_array().expect("decorations");
    assert!(
        newest_refs.iter().any(|r| r["kind"] == "branch"),
        "{newest_refs:?}"
    );

    client
        .send_raw(json!({
            "jsonrpc": "2.0", "method": "dispatchAction",
            "params": {"channel": channel, "action": {
                "type": "history/grow", "before": "100",
            }},
        }))
        .await;
    let appended = loop {
        let action = client.next_action(&channel).await;
        if action["type"] == "history/appended" {
            break action;
        }
    };
    let older = appended["commits"].as_array().expect("the slice");
    assert_eq!(older.len(), 3, "{appended}");
    assert_eq!(older[2]["summary"], "first");
    assert!(appended["more"].is_null(), "the root ended the walk");

    let second = &older[1];
    assert_eq!(second["summary"], "second");
    let changeset = second["changeset"].as_str().expect("the reference");
    assert_eq!(
        changeset,
        &format!(
            "hihost-changes:/{}?commit={}",
            root.display(),
            second["id"].as_str().unwrap()
        )
    );
    let snapshot = client
        .request("subscribe", json!({"channel": changeset}))
        .await;
    assert_eq!(snapshot["snapshot"]["state"]["status"], "computing");
    let files = loop {
        let action = client.next_action(changeset).await;
        if action["type"] == "changeset/contentChanged" {
            break action["files"].clone();
        }
    };
    let listed = files.as_array().expect("files");
    assert_eq!(listed.len(), 1, "{files}");
    let readme = &listed[0];
    let before_ref = readme["edit"]["before"]["content"]["uri"]
        .as_str()
        .expect("the parent side")
        .to_owned();
    let after_ref = readme["edit"]["after"]["content"]["uri"]
        .as_str()
        .expect("the commit side")
        .to_owned();
    let read = client
        .request(
            "resourceRead",
            json!({"channel": changeset, "uri": before_ref}),
        )
        .await;
    assert_eq!(read["data"], "one\n");
    let read = client
        .request(
            "resourceRead",
            json!({"channel": changeset, "uri": after_ref}),
        )
        .await;
    assert_eq!(read["data"], "one\ntwo\n");

    std::fs::write(root.join("README.md"), "one\ntwo\nthree\n").unwrap();
    client
        .send_raw(json!({
            "jsonrpc": "2.0", "method": "dispatchAction",
            "params": {"channel": channel, "action": {
                "type": "history/commit", "message": "from the wire",
            }},
        }))
        .await;
    loop {
        let action = client.next_action(&channel).await;
        if action["type"] == "history/reset"
            && action["state"]["commits"][0]["summary"] == "from the wire"
        {
            break;
        }
    }
}

#[tokio::test]
async fn an_unlisted_session_surfaces_on_its_first_turn_or_dies_at_boot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(Arc::clone(&host)).await;
    client
        .request(
            "initialize",
            json!({
                "channel": ROOT,
                "protocolVersions": ["0.7.0"],
                "clientId": "test",
                "initialSubscriptions": [ROOT],
            }),
        )
        .await;

    let session = format!("ahp-session:/unlisted-{}", std::process::id());
    client
        .request(
            "createSession",
            json!({
                "channel": session,
                "provider": "claude",
                "workingDirectories": [],
                "config": {"isolation": "folder", "unlisted": true},
            }),
        )
        .await;
    let listed = client
        .request("listSessions", json!({"channel": ROOT}))
        .await;
    let rows = listed["items"].as_array().expect("items");
    assert!(
        !rows
            .iter()
            .any(|row| row["resource"] == json!(session.clone())),
        "an unlisted birth must not list: {rows:?}"
    );

    let doomed = format!("ahp-session:/doomed-{}", std::process::id());
    client
        .request(
            "createSession",
            json!({
                "channel": doomed,
                "provider": "claude",
                "workingDirectories": [],
                "config": {"unlisted": true},
            }),
        )
        .await;

    let snapshot = client
        .request("subscribe", json!({"channel": session}))
        .await;
    let chat = snapshot["snapshot"]["state"]["defaultChat"]
        .as_str()
        .expect("default chat")
        .to_owned();
    client.request("subscribe", json!({"channel": chat})).await;
    client
        .dispatch(&chat, turn_started("t1", "hello placeholder"))
        .await;
    let added = client.next_notification("root/sessionAdded").await;
    assert_eq!(
        added["summary"]["resource"],
        json!(session.clone()),
        "the first turn broadcasts the withheld sessionAdded"
    );
    let listed = client
        .request("listSessions", json!({"channel": ROOT}))
        .await;
    let rows = listed["items"].as_array().expect("items");
    assert!(
        rows.iter()
            .any(|row| row["resource"] == json!(session.clone())),
        "a spoken session lists: {rows:?}"
    );

    drop(client);
    let reborn = host_at(dir.path());
    let mut client = Client::connect(Arc::clone(&reborn)).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let listed = client
        .request("listSessions", json!({"channel": ROOT}))
        .await;
    let rows = listed["items"].as_array().expect("items");
    assert!(
        rows.iter().any(|row| row["resource"] == json!(session)),
        "the spoken session survives the reboot listed: {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|row| row["resource"] == json!(doomed.clone())),
        "the turn-less placeholder was garbage-collected: {rows:?}"
    );
    let refused = client
        .request_any("subscribe", json!({"channel": doomed}))
        .await;
    assert!(
        refused.get("error").is_some(),
        "the collected session's channel is gone: {refused}"
    );
}

#[tokio::test]
async fn a_binary_resource_reads_as_base64() {
    use base64::Engine;
    let dir = tempfile::tempdir().expect("tempdir");
    let host = host_at(dir.path());
    let mut client = Client::connect(host).await;
    let (_session, channel) = open_session(&mut client, dir.path()).await;

    let bytes: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe];
    let picture = dir.path().join("pic.png");
    std::fs::write(&picture, &bytes).expect("write");

    let read = client
        .request(
            "resourceRead",
            json!({"channel": channel, "uri": format!("file://{}", picture.display())}),
        )
        .await;
    assert_eq!(read["encoding"], "base64", "binary answers base64: {read}");
    assert_eq!(read["contentType"], "image/png");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(read["data"].as_str().expect("data"))
        .expect("valid base64");
    assert_eq!(decoded, bytes, "the bytes came back verbatim");

    let note = dir.path().join("note.md");
    std::fs::write(&note, "hello\n").expect("write");
    let read = client
        .request(
            "resourceRead",
            json!({"channel": channel, "uri": format!("file://{}", note.display())}),
        )
        .await;
    assert_eq!(read["encoding"], "utf-8");
    assert_eq!(read["data"], "hello\n");

    let read = client
        .request(
            "resourceRead",
            json!({
                "channel": channel,
                "uri": format!("file://{}", note.display()),
                "encoding": "base64",
            }),
        )
        .await;
    assert_eq!(read["encoding"], "base64");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(read["data"].as_str().expect("data"))
        .expect("valid base64");
    assert_eq!(String::from_utf8(decoded).expect("utf-8"), "hello\n");
}

/// Mode one, the host's half: agents edit FILES; the host owns the
/// mirror's disk reload and broadcasts the result as its own edit.
#[tokio::test]
async fn a_mirrored_files_change_broadcasts_as_the_hosts_edit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("agent.md");
    std::fs::write(&file, "alpha\nbeta\n").unwrap();
    let host = host_at(dir.path());
    let mut client = Client::connect(host.clone()).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let uri = format!("file://{}", file.display());
    let opened = client
        .request(
            "openDocument",
            json!({"channel": "hihost-fs:/local", "uri": uri}),
        )
        .await;
    let channel = opened["document"].as_str().expect("a channel").to_owned();
    let v0 = opened["version"].as_str().expect("a version").to_owned();
    client
        .request("subscribe", json!({"channel": channel}))
        .await;

    // The agent writes the file behind the host's back.
    std::fs::write(&file, "alpha\nAGENT\nbeta\n").unwrap();

    let action = client.next_action(&channel).await;
    assert_eq!(action["type"], "document/applied");
    assert_eq!(
        action["base"],
        v0.as_str(),
        "the edit chains off the mirror"
    );
    let replacements = action["operation"]["replacements"]
        .as_array()
        .expect("replacements");
    assert!(
        replacements
            .iter()
            .any(|span| span["text"].as_str().unwrap_or_default().contains("AGENT")),
        "the broadcast carries the agent's change: {action}"
    );

    // The mirror moved with it: reopening reports the host edit's id.
    let reopened = client
        .request(
            "openDocument",
            json!({"channel": "hihost-fs:/local", "uri": uri}),
        )
        .await;
    assert_eq!(reopened["document"], channel.as_str());
    assert_eq!(
        reopened["version"], action["id"],
        "the host's edit is the mirror's head"
    );
}

/// Mode one with unflushed client edits: the host three-way merges
/// the disk change over them — nobody's bytes lost, one broadcast.
#[tokio::test]
async fn unflushed_client_edits_survive_the_hosts_file_reload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("shared.md");
    std::fs::write(&file, "alpha\nbeta\n").unwrap();
    let host = host_at(dir.path());
    let mut client = Client::connect(host.clone()).await;
    client
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "test"}),
        )
        .await;
    let uri = format!("file://{}", file.display());
    let opened = client
        .request(
            "openDocument",
            json!({"channel": "hihost-fs:/local", "uri": uri}),
        )
        .await;
    let channel = opened["document"].as_str().expect("a channel").to_owned();
    let v0 = opened["version"].as_str().expect("a version").to_owned();
    client
        .request("subscribe", json!({"channel": channel}))
        .await;

    // A client edit the disk has not seen (unflushed typing).
    dispatch_applied(
        &mut client,
        &channel,
        &v0,
        uid(0xa1),
        insert_at(0, 0, "OURS "),
    )
    .await;
    let echo = client.next_action(&channel).await;
    assert_eq!(echo["id"], uid(0xa1).as_str());

    // The agent appends a line on disk.
    std::fs::write(&file, "alpha\nbeta\nAGENT\n").unwrap();

    let action = client.next_action(&channel).await;
    assert_eq!(action["type"], "document/applied");
    assert_eq!(
        action["base"],
        uid(0xa1).as_str(),
        "the reload chains off the client's edit, not the stale disk"
    );

    // A second subscriber sees the MERGE: typing kept, agent's line in.
    let mut witness = Client::connect(host.clone()).await;
    witness
        .request(
            "initialize",
            json!({"channel": ROOT, "protocolVersions": ["0.7.0"], "clientId": "witness"}),
        )
        .await;
    let snapshot = witness
        .request("subscribe", json!({"channel": channel}))
        .await;
    assert_eq!(
        snapshot["snapshot"]["state"]["text"], "OURS alpha\nbeta\nAGENT\n",
        "three-way: unflushed typing survives the agent's reload"
    );
}
