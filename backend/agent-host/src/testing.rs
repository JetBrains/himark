// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::path::{Path, PathBuf};

pub(crate) const FAKE_CLI: &str = r#"#!/usr/bin/env python3
import json, os, sys, time

print("[stub] alive pid=%d" % os.getpid(), file=sys.stderr, flush=True)

# The spawn oracle: every boot appends its argv + the flag-bearing
# env + cwd, so tests can assert what a (re)spawn was born with. The
# log rides NEXT TO THE SCRIPT (each test owns its stub dir) — the
# cwd of a dirless session is the shared $TMPDIR.
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "spawn-log.jsonl"), "a") as log:
    log.write(json.dumps({"argv": sys.argv[1:], "cwd": os.getcwd(),
                          "max_thinking_tokens": os.environ.get("MAX_THINKING_TOKENS")}) + "\n")

def out(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

session = "stub"
args = sys.argv
if "--session-id" in args:
    session = args[args.index("--session-id") + 1]
for a in args:
    if a.startswith("--resume="):
        session = a.split("=", 1)[1]

out({"type": "system", "subtype": "init", "session_id": session,
     "model": "stub-model", "tools": ["Bash", "Write"], "permissionMode": "default", "cwd": "."})

def read():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    return json.loads(line)

def stream_text(text):
    out({"type": "stream_event", "session_id": session,
         "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}})
    for ch in text:
        out({"type": "stream_event", "session_id": session,
             "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": ch}}})
    out({"type": "stream_event", "session_id": session,
         "event": {"type": "content_block_stop", "index": 0}})
    out({"type": "assistant", "session_id": session,
         "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}})

def result(subtype="success", is_error=False):
    out({"type": "result", "subtype": subtype, "is_error": is_error, "session_id": session,
         "duration_ms": 5, "usage": {"input_tokens": 3, "output_tokens": 2}, "total_cost_usd": 0.001,
         "result": "done"})

while True:
    msg = read()
    if msg.get("type") != "user":
        continue
    text = "".join(b.get("text", "") for b in msg["message"]["content"])
    if "permission" in text:
        out({"type": "assistant", "session_id": session, "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo hi"}}]}})
        out({"type": "control_request", "request_id": "ask-1", "session_id": session,
             "request": {"subtype": "can_use_tool", "tool_name": "Bash", "display_name": "Bash",
                          "input": {"command": "echo hi"}, "description": "echo hi",
                          "tool_use_id": "t1", "permission_suggestions": []}})
        answer = read()
        while answer.get("type") != "control_response":
            answer = read()
        allowed = answer["response"]["response"].get("behavior") == "allow"
        if allowed:
            out({"type": "user", "session_id": session, "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "hi"}]}})
            stream_text("ran it")
        else:
            out({"type": "user", "session_id": session, "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "denied", "is_error": True}]}})
            stream_text("understood")
        result()
    elif "question" in text:
        q_input = {"questions": [{"question": "Which color?", "header": "Color",
                                  "options": [{"label": "Red"}, {"label": "Blue"}],
                                  "multiSelect": False}]}
        out({"type": "assistant", "session_id": session, "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "q1", "name": "AskUserQuestion", "input": q_input}]}})
        out({"type": "control_request", "request_id": "ask-q1", "session_id": session,
             "request": {"subtype": "can_use_tool", "tool_name": "AskUserQuestion",
                          "display_name": "AskUserQuestion", "input": q_input,
                          "tool_use_id": "q1", "permission_suggestions": []}})
        answer = read()
        while answer.get("type") != "control_response":
            answer = read()
        response = answer["response"]["response"]
        answers = (response.get("updatedInput") or {}).get("answers") or {}
        # The REAL CLI reads the choice from updatedInput.answers — an
        # allow without it counts as a refusal, same as a deny.
        if response.get("behavior") == "allow" and answers:
            out({"type": "user", "session_id": session, "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "q1", "content": "answered"}]}})
            stream_text("answered: " + answers.get("Which color?", ""))
        else:
            out({"type": "user", "session_id": session, "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "q1",
                 "content": "User declined to answer questions", "is_error": True}]}})
            stream_text("refused")
        result()
    elif "edit" in text:
        out({"type": "assistant", "session_id": session, "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "e1", "name": "Write",
             "input": {"file_path": os.path.abspath("edited.txt"), "content": "hello\nworld\n"}}]}})
        time.sleep(0.05)  # tool execution is never instantaneous
        with open("edited.txt", "w") as f:
            f.write("hello\nworld\n")
        out({"type": "user", "session_id": session, "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "e1", "content": "wrote it"}]}})
        stream_text("edited")
        result()
    elif "die-now" in text:
        time.sleep(0.4)  # long enough for the test to queue behind this turn
        sys.exit(1)
    elif "slow" in text:
        time.sleep(0.7)
        stream_text("OK")
        result()
    elif "park" in text:
        out({"type": "stream_event", "session_id": session,
             "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}})
        out({"type": "stream_event", "session_id": session,
             "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "thinking"}}})
        interrupt = read()
        while not (interrupt.get("type") == "control_request"
                   and interrupt.get("request", {}).get("subtype") == "interrupt"):
            interrupt = read()
        out({"type": "control_response", "session_id": session,
             "response": {"subtype": "success", "request_id": interrupt.get("request_id"), "response": {}}})
        result(subtype="canceled")
    elif "echo-prompt" in text:
        # The expansion oracle: reply with the EXACT prompt received.
        stream_text(text)
        result()
    elif "trickle:" in text:
        # A LONG stream, one character every few ms: the text between
        # "trickle:" and ":end" is echoed back as that many deltas.
        # Reconnect tests break the wire under it and check nothing
        # doubled or went missing.
        payload = text.split("trickle:", 1)[1].split(":end", 1)[0]
        out({"type": "stream_event", "session_id": session,
             "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}})
        for ch in payload:
            out({"type": "stream_event", "session_id": session,
                 "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": ch}}})
            time.sleep(0.002)
        out({"type": "stream_event", "session_id": session,
             "event": {"type": "content_block_stop", "index": 0}})
        out({"type": "assistant", "session_id": session,
             "message": {"role": "assistant", "content": [{"type": "text", "text": payload}]}})
        result()
    else:
        stream_text("OK")
        result()
"#;

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub fn fake_cli_command(dir: &Path) -> String {
    format!("python3 {}", write_fake_cli(dir).display())
}

pub(crate) fn write_fake_cli(dir: &Path) -> PathBuf {
    let path = dir.join("fake-claude.py");
    std::fs::write(&path, FAKE_CLI).expect("stub written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    path
}

pub(crate) const FAKE_CODEX: &str = r#"#!/usr/bin/env python3
import json, os, sys

root = os.path.dirname(os.path.abspath(__file__))
log_path = os.path.join(root, "codex-log.jsonl")

def out(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

def read():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    value = json.loads(line)
    with open(log_path, "a") as log:
        log.write(json.dumps(value) + "\n")
    return value

message = read()
assert message.get("method") == "initialize"
out({"id": message["id"], "result": {"userAgent": "fake-codex"}})
message = read()
assert message.get("method") == "initialized"
message = read()
assert message.get("method") in ("thread/start", "thread/resume")
thread = message.get("params", {}).get("threadId", "codex-thread-1")
out({"id": message["id"], "result": {"thread": {"id": thread}}})
out({"method": "thread/started", "params": {"thread": {"id": thread}}})

turn_number = 0
while True:
    message = read()
    if message.get("method") != "turn/start":
        continue
    turn_number += 1
    turn = "codex-turn-%d" % turn_number
    text = "".join(item.get("text", "") for item in message["params"].get("input", []))
    out({"id": message["id"], "result": {"turn": {
        "id": turn, "status": "inProgress", "items": [], "error": None}}})
    out({"method": "turn/started", "params": {"threadId": thread, "turn": {
        "id": turn, "status": "inProgress", "items": [], "error": None}}})

    if "permission" in text:
        item = {"type": "commandExecution", "id": "cmd-1", "command": "echo hi",
                "cwd": os.getcwd(), "status": "inProgress", "commandActions": []}
        out({"method": "item/started", "params": {
            "threadId": thread, "turnId": turn, "startedAtMs": 1, "item": item}})
        out({"id": "approval-1", "method": "item/commandExecution/requestApproval",
             "params": {"threadId": thread, "turnId": turn, "itemId": "cmd-1",
                        "startedAtMs": 1, "command": "echo hi", "cwd": os.getcwd(),
                        "reason": "Run echo hi"}})
        answer = read()
        while answer.get("id") != "approval-1":
            answer = read()
        accepted = answer.get("result", {}).get("decision") == "accept"
        item.update({"status": "completed" if accepted else "declined",
                     "aggregatedOutput": "hi\n" if accepted else "declined", "exitCode": 0 if accepted else None})
        out({"method": "item/completed", "params": {
            "threadId": thread, "turnId": turn, "completedAtMs": 2, "item": item}})
    elif "park" in text:
        item = {"type": "agentMessage", "id": "msg-park", "text": ""}
        out({"method": "item/started", "params": {
            "threadId": thread, "turnId": turn, "startedAtMs": 1, "item": item}})
        out({"method": "item/agentMessage/delta", "params": {
            "threadId": thread, "turnId": turn, "itemId": "msg-park", "delta": "thinking"}})
        interrupt = read()
        while interrupt.get("method") != "turn/interrupt":
            interrupt = read()
        out({"id": interrupt["id"], "result": {}})
        out({"method": "turn/completed", "params": {"threadId": thread, "turn": {
            "id": turn, "status": "interrupted", "items": [], "durationMs": 2, "error": None}}})
        continue
    else:
        item = {"type": "agentMessage", "id": "msg-%d" % turn_number, "text": ""}
        out({"method": "item/started", "params": {
            "threadId": thread, "turnId": turn, "startedAtMs": 1, "item": item}})
        for delta in ("O", "K"):
            out({"method": "item/agentMessage/delta", "params": {
                "threadId": thread, "turnId": turn, "itemId": item["id"], "delta": delta}})
        item["text"] = "OK"
        out({"method": "item/completed", "params": {
            "threadId": thread, "turnId": turn, "completedAtMs": 2, "item": item}})

    out({"method": "thread/tokenUsage/updated", "params": {
        "threadId": thread, "turnId": turn, "tokenUsage": {
            "last": {"inputTokens": 3, "outputTokens": 2, "cachedInputTokens": 1,
                     "reasoningOutputTokens": 0, "totalTokens": 5},
            "total": {"inputTokens": 3, "outputTokens": 2, "cachedInputTokens": 1,
                      "reasoningOutputTokens": 0, "totalTokens": 5}}}})
    out({"method": "turn/completed", "params": {"threadId": thread, "turn": {
        "id": turn, "status": "completed", "items": [], "durationMs": 5, "error": None}}})
"#;

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub fn fake_codex_command(dir: &Path) -> String {
    let path = dir.join("fake-codex.py");
    std::fs::write(&path, FAKE_CODEX).expect("codex stub written");
    format!("python3 {}", path.display())
}

const FAKE_LS: &str = r#"
import json, sys

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length))

def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

parked = []
while True:
    message = read()
    if message is None:
        break
    method = message.get("method")
    ident = message.get("id")
    params = message.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": ident, "result": {"capabilities": {
            "positionEncoding": "utf-8",
            "definitionProvider": True,
            "referencesProvider": True,
        }}})
    elif method in ("textDocument/didOpen", "textDocument/didChange"):
        doc = params.get("textDocument") or {}
        if method == "textDocument/didOpen":
            content = (doc.get("text") or "")
        else:
            content = "".join(change.get("text", "") for change in params.get("contentChanges", []))
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
            "uri": doc.get("uri"),
            "version": doc.get("version"),
            "diagnostics": [] if content == "" else [{
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                "message": "v%s:%s" % (doc.get("version"), content[:40]),
            }],
        }})
    elif method == "textDocument/definition":
        uri = (params.get("textDocument") or {}).get("uri")
        send({"jsonrpc": "2.0", "id": ident, "result": {
            "uri": uri,
            "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}},
        }})
    elif method == "textDocument/implementation":
        uri = (params.get("textDocument") or {}).get("uri")
        send({"jsonrpc": "2.0", "id": ident, "result": [
            {"targetUri": uri, "targetSelectionRange": {
                "start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 9}}},
            {"targetUri": uri, "targetSelectionRange": {
                "start": {"line": 0, "character": 21}, "end": {"line": 0, "character": 23}}},
        ]})
    elif method == "textDocument/references":
        uri = (params.get("textDocument") or {}).get("uri")
        position = params.get("position") or {}
        if position.get("line") == 99:
            parked.append((ident, uri))  # parked until cancelled
        else:
            send({"jsonrpc": "2.0", "id": ident, "result": [
                {"uri": uri, "range": {
                    "start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 9}}},
                {"uri": uri, "range": {
                    "start": {"line": 0, "character": 21}, "end": {"line": 0, "character": 23}}},
            ]})
    elif method == "$/cancelRequest":
        target = params.get("id")
        held = next((entry for entry in parked if entry[0] == target), None)
        if held is not None:
            parked.remove(held)
            send({"jsonrpc": "2.0", "id": target,
                  "error": {"code": -32800, "message": "request cancelled"}})
            # a cancel marker the tests can observe on the diagnostics channel
            send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
                "uri": held[1],
                "diagnostics": [{
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                    "message": "cancelled %s" % target,
                }],
            }})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": ident, "result": None})
    elif method == "exit":
        break
    elif ident is not None:
        send({"jsonrpc": "2.0", "id": ident, "result": None})
"#;

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub fn fake_ls_command(dir: &Path) -> String {
    let path = dir.join("fake-ls.py");
    std::fs::write(&path, FAKE_LS).expect("fake ls written");
    format!("python3 {}", path.display())
}

/// A scripted FSP server (docs/file-search.md §9): speaks real
/// Content-Length framing, negotiates utf-8, logs every incoming
/// message to `fsp-log.jsonl` beside the script. Canned behavior:
/// textSearch answers one match for the query in `<first dir>/hit.md`
/// (`"cut"` sets limitHit, `"die-now"` kills the process before
/// answering, `"slow"` parks until `$/cancelRequest`); fileSearch
/// answers `zeta.rs` above `alpha.rs` (score order — the host must
/// re-sort to path order).
pub const FAKE_FSP: &str = r#"#!/usr/bin/env python3
import json, os, sys

root = os.path.dirname(os.path.abspath(__file__))
log_path = os.path.join(root, "fsp-log.jsonl")

def log(obj):
    with open(log_path, "a") as f:
        f.write(json.dumps(obj) + "\n")

def read_msg():
    headers = {}
    line = sys.stdin.buffer.readline()
    if not line:
        sys.exit(0)
    while line and line.strip():
        name, _, value = line.decode().partition(":")
        headers[name.strip().lower()] = value.strip()
        line = sys.stdin.buffer.readline()
    if not line:
        sys.exit(0)
    body = sys.stdin.buffer.read(int(headers["content-length"]))
    msg = json.loads(body)
    log(msg)
    return msg

def send(obj):
    body = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

msg = read_msg()
assert msg["method"] == "initialize", msg
folders = [f["uri"] for f in (msg["params"].get("searchFolders") or [])]
send({"jsonrpc": "2.0", "id": msg["id"], "result": {
    "capabilities": {
        "positionEncoding": "utf-8",
        "textDocumentSync": 2,
        "textSearchProvider": {"regExp": True, "multiline": True},
        "fileSearchProvider": {"fuzzy": True},
        "workspace": {"searchFolders": {"supported": True, "changeNotifications": True}}},
    "serverInfo": {"name": "fake-fsp", "version": "0"}}})

parked = None
while True:
    msg = read_msg()
    method = msg.get("method")
    if method == "exit":
        sys.exit(0)
    if method == "shutdown":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
        continue
    if method == "$/cancelRequest" and parked is not None:
        if msg["params"]["id"] == parked:
            send({"jsonrpc": "2.0", "id": parked,
                  "error": {"code": -32800, "message": "request cancelled"}})
            parked = None
        continue
    if method == "workspace/didChangeSearchFolders":
        event = msg["params"]["event"]
        for removed in event["removed"]:
            if removed["uri"] in folders:
                folders.remove(removed["uri"])
        for added in event["added"]:
            if added["uri"] not in folders:
                folders.append(added["uri"])
        continue
    if method == "workspace/textSearch":
        p = msg["params"]
        q = p["query"]
        if q == "die-now":
            sys.exit(1)
        if q == "slow":
            parked = msg["id"]
            continue
        dirs = p.get("dirs") or folders
        uri = dirs[0].rstrip("/") + "/hit.md"
        text = "the %s here" % q
        file_result = {"uri": uri, "matches": [{
            "range": {"start": {"line": 0, "character": 4},
                       "end": {"line": 0, "character": 4 + len(q)}},
            "lines": [{"line": 0, "text": text, "isMatch": True}]}]}
        token = p.get("partialResultToken")
        if token is not None:
            send({"jsonrpc": "2.0", "method": "$/progress",
                  "params": {"token": token, "value": [file_result]}})
            send({"jsonrpc": "2.0", "id": msg["id"],
                  "result": {"results": [], "limitHit": q == "cut"}})
        else:
            send({"jsonrpc": "2.0", "id": msg["id"],
                  "result": {"results": [file_result], "limitHit": q == "cut"}})
        continue
    if method == "workspace/fileSearch":
        p = msg["params"]
        dirs = p.get("dirs") or folders
        base = dirs[0].rstrip("/")
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"results": [
            {"uri": base + "/zeta.rs", "score": 2.0},
            {"uri": base + "/alpha.rs", "score": 1.0}], "limitHit": False}})
        continue
"#;

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub fn fake_fsp_command(dir: &Path) -> String {
    let path = dir.join("fake-fsp.py");
    std::fs::write(&path, FAKE_FSP).expect("fake fsp written");
    format!("python3 {}", path.display())
}

/// The fake FSP server's message log, parsed.
#[doc(hidden)]
pub fn fsp_log(dir: &Path) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(dir.join("fsp-log.jsonl")).unwrap_or_default();
    raw.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
