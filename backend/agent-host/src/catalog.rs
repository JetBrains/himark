use std::io::BufRead;
use std::path::{Path, PathBuf};

use ahp_types::actions::{
    ChatResponsePartAction, ChatTurnCompleteAction, ChatTurnStartedAction, StateAction,
};
use ahp_types::state::{MarkdownResponsePart, Message, MessageKind, MessageOrigin, ResponsePart};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct CliSession {
    pub provider: String,

    pub storage_id: String,

    pub native_id: String,
    pub cwd: Option<String>,
    pub title: String,
    pub modified_at: std::time::SystemTime,
    pub path: PathBuf,
}

pub fn scan(claude_home: &Path, limit: usize) -> Vec<CliSession> {
    let projects = claude_home.join("projects");
    let Ok(dirs) = std::fs::read_dir(&projects) else {
        return Vec::new();
    };
    let mut sessions: Vec<CliSession> = Vec::new();
    for dir in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(native_id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let modified_at = file
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            let Some((title, cwd)) = head(&path) else {
                continue;
            };
            sessions.push(CliSession {
                provider: "claude".to_owned(),
                storage_id: native_id.to_owned(),
                native_id: native_id.to_owned(),
                cwd,
                title,
                modified_at,
                path,
            });
        }
    }
    sessions.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
    sessions.truncate(limit);
    sessions
}

pub fn scan_codex(codex_home: &Path, limit: usize) -> Vec<CliSession> {
    let mut files = Vec::new();
    collect_jsonl(&codex_home.join("sessions"), &mut files);
    files.sort_by(|a, b| b.0.cmp(&a.0));

    let mut sessions = Vec::new();
    for (modified_at, path) in files {
        let Some((native_id, cwd, title)) = codex_head(&path) else {
            continue;
        };
        sessions.push(CliSession {
            provider: "codex".to_owned(),
            storage_id: format!("codex-{native_id}"),
            native_id,
            cwd,
            title,
            modified_at,
            path,
        });
        if sessions.len() == limit {
            break;
        }
    }
    sessions
}

fn collect_jsonl(root: &Path, found: &mut Vec<(std::time::SystemTime, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            collect_jsonl(&path, found);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            found.push((modified, path));
        }
    }
}

fn codex_head(path: &Path) -> Option<(String, Option<String>, String)> {
    let file = std::fs::File::open(path).ok()?;
    let mut native_id = None;
    let mut cwd = None;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if entry["type"] == "session_meta" {
            let payload = &entry["payload"];
            if payload["source"].get("subagent").is_some() {
                return None;
            }
            native_id = payload["id"]
                .as_str()
                .or_else(|| payload["session_id"].as_str())
                .filter(|id| {
                    !id.is_empty()
                        && id
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                })
                .map(str::to_owned);
            cwd = payload["cwd"].as_str().map(str::to_owned);
            continue;
        }
        if entry["type"] != "response_item"
            || entry["payload"]["type"] != "message"
            || entry["payload"]["role"] != "user"
        {
            continue;
        }
        let Some(text) = codex_message_text(&entry["payload"]) else {
            continue;
        };
        if injected_codex_message(&text) {
            continue;
        }
        return Some((native_id?, cwd, title_of(&text)?));
    }
    None
}

fn head(path: &Path) -> Option<(String, Option<String>)> {
    let raw = std::fs::read_to_string(path).ok()?;
    for line in raw.lines().take(80) {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        if entry["type"].as_str() != Some("user") {
            continue;
        }
        let Some(text) = user_text(&entry["message"]) else {
            continue;
        };
        let mut title = text.trim().replace('\n', " ");
        if title.len() > 64 {
            title = title.chars().take(64).collect();
        }
        if title.is_empty() || title.starts_with('/') {
            continue;
        }
        let cwd = entry["cwd"].as_str().map(str::to_owned);
        return Some((title, cwd));
    }
    None
}

pub fn backfill_actions(path: &Path) -> Vec<StateAction> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    let mut open_turn: Option<String> = None;
    let mut minted = 0u64;
    for line in raw.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let stamp = entry["timestamp"].as_str().unwrap_or("").to_owned();
        match entry["type"].as_str() {
            Some("user") => {
                let Some(text) = user_text(&entry["message"]) else {
                    continue;
                };
                if let Some(turn) = open_turn.take() {
                    actions.push(close(turn));
                }
                minted += 1;
                let turn_id = entry["uuid"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("backfill-{minted}"));
                actions.push(StateAction::ChatTurnStarted(ChatTurnStartedAction {
                    turn_id: turn_id.clone(),
                    started_at: stamp,
                    message: Message {
                        text,
                        origin: MessageOrigin {
                            kind: MessageKind::User,
                        },
                        attachments: None,
                        model: None,
                        agent: None,
                        meta: None,
                    },
                    queued_message_id: None,
                    meta: None,
                }));
                open_turn = Some(turn_id);
            }
            Some("assistant") => {
                let Some(turn) = open_turn.clone() else {
                    continue;
                };
                let Some(blocks) = entry["message"]["content"].as_array() else {
                    continue;
                };
                for block in blocks {
                    if block["type"].as_str() != Some("text") {
                        continue;
                    }
                    let text = block["text"].as_str().unwrap_or_default();
                    if text.is_empty() {
                        continue;
                    }
                    minted += 1;
                    actions.push(StateAction::ChatResponsePart(ChatResponsePartAction {
                        turn_id: turn.clone(),
                        part: ResponsePart::Markdown(MarkdownResponsePart {
                            id: format!("backfill-p{minted}"),
                            content: text.to_owned(),
                        }),
                        meta: None,
                    }));
                }
            }
            _ => {}
        }
    }
    if let Some(turn) = open_turn {
        actions.push(close(turn));
    }
    actions
}

pub fn session_backfill_actions(session: &CliSession) -> Vec<StateAction> {
    match session.provider.as_str() {
        "codex" => codex_backfill_actions(&session.path),
        _ => backfill_actions(&session.path),
    }
}

fn codex_backfill_actions(path: &Path) -> Vec<StateAction> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    let mut open_turn: Option<String> = None;
    let mut minted = 0u64;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let payload = &entry["payload"];
        if entry["type"] != "response_item" || payload["type"] != "message" {
            continue;
        }
        let Some(text) = codex_message_text(payload) else {
            continue;
        };
        let stamp = entry["timestamp"].as_str().unwrap_or("").to_owned();
        match payload["role"].as_str() {
            Some("user") if !injected_codex_message(&text) => {
                if let Some(turn) = open_turn.take() {
                    actions.push(close(turn));
                }
                minted += 1;
                let turn_id = format!("codex-backfill-{minted}");
                actions.push(StateAction::ChatTurnStarted(ChatTurnStartedAction {
                    turn_id: turn_id.clone(),
                    started_at: stamp,
                    message: Message {
                        text,
                        origin: MessageOrigin {
                            kind: MessageKind::User,
                        },
                        attachments: None,
                        model: None,
                        agent: None,
                        meta: None,
                    },
                    queued_message_id: None,
                    meta: None,
                }));
                open_turn = Some(turn_id);
            }
            Some("assistant") => {
                let Some(turn) = open_turn.clone() else {
                    continue;
                };
                minted += 1;
                actions.push(StateAction::ChatResponsePart(ChatResponsePartAction {
                    turn_id: turn,
                    part: ResponsePart::Markdown(MarkdownResponsePart {
                        id: format!("codex-backfill-p{minted}"),
                        content: text,
                    }),
                    meta: None,
                }));
            }
            _ => {}
        }
    }
    if let Some(turn) = open_turn {
        actions.push(close(turn));
    }
    actions
}

fn codex_message_text(message: &Value) -> Option<String> {
    let text = message["content"]
        .as_array()?
        .iter()
        .filter_map(|content| {
            matches!(
                content["type"].as_str(),
                Some("input_text" | "output_text" | "text")
            )
            .then(|| content["text"].as_str())
            .flatten()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then_some(text)
}

fn injected_codex_message(text: &str) -> bool {
    let text = text.trim_start();
    [
        "<environment_context>",
        "<permissions instructions>",
        "<skills_instructions>",
        "<apps_instructions>",
        "<plugins_instructions>",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
}

fn title_of(text: &str) -> Option<String> {
    let title: String = text.trim().replace('\n', " ").chars().take(64).collect();
    (!title.is_empty()).then_some(title)
}

fn close(turn_id: String) -> StateAction {
    StateAction::ChatTurnComplete(ChatTurnCompleteAction {
        turn_id,
        duration: 0,
        meta: None,
    })
}

fn user_text(message: &Value) -> Option<String> {
    match &message["content"] {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let text: String = blocks
                .iter()
                .filter(|block| block["type"].as_str() == Some("text"))
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()
                && !blocks
                    .iter()
                    .any(|block| block["type"].as_str() == Some("tool_result")))
            .then_some(text)
        }
        _ => None,
    }
}
