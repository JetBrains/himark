// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UnixStream};

fn host_at(dir: &std::path::Path) -> Arc<agent_host::Host> {
    agent_host::Host::new(agent_host::HostConfig {
        agents: Vec::new(),
        data_dir: dir.join("data"),
        claude_binary: agent_host::testing::fake_cli_command(dir),
        codex_binary: "false".to_owned(),
        claude_home: dir.join("dot-claude"),
        codex_home: dir.join("dot-codex"),
        model_titles: false,
        shell: "/bin/sh".to_owned(),
        language_servers: Vec::new(),
    })
}

async fn unix_request(host: &Arc<agent_host::Host>, method: &str, params: Value) -> Value {
    let (ours, theirs) = UnixStream::pair().expect("socketpair");
    tokio::spawn(Arc::clone(host).serve_stream(theirs));
    let (read, mut write) = ours.into_split();
    write
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
            )
            .as_bytes(),
        )
        .await
        .expect("send");
    let mut lines = BufReader::new(read).lines();
    loop {
        let line = lines.next_line().await.expect("read").expect("open");
        let message: Value = serde_json::from_str(&line).expect("json");
        if message.get("id").and_then(Value::as_u64) == Some(1) {
            return message;
        }
    }
}

fn addr_and_token(url: &str) -> (String, String) {
    let rest = url.strip_prefix("http://").expect("http url");
    let (addr, query) = rest.split_once("/?tkn=").expect("tokened url");
    (addr.to_owned(), query.to_owned())
}

fn response_header<'a>(response: &'a str, wanted: &str) -> Option<&'a str> {
    response
        .split_once("\r\n\r\n")?
        .0
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find_map(|(name, value)| name.eq_ignore_ascii_case(wanted).then(|| value.trim()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_http_face_serves_ahp_over_websocket_and_gates_on_the_token() {
    let dir = tempdir();
    let host = host_at(dir.path());
    let answer = unix_request(&host, "httpServe", json!({"enabled": true})).await;
    let url = answer["result"]["url"].as_str().expect("url").to_owned();
    let (addr, token) = addr_and_token(&url);

    let again = unix_request(&host, "httpServe", json!({"enabled": true})).await;
    assert_eq!(
        again["result"]["url"].as_str(),
        Some(url.as_str()),
        "a bare httpServe re-ask answers the standing url"
    );

    let tcp = TcpStream::connect(&addr).await.expect("tcp");
    let (mut websocket, _response) =
        tokio_tungstenite::client_async(format!("ws://{addr}/?tkn={token}"), tcp)
            .await
            .expect("upgrade");
    websocket
        .send(tokio_tungstenite::tungstenite::Message::text(
            json!({"jsonrpc": "2.0", "id": 7, "method": "ping", "params": {"channel": "ahp-root://"}})
                .to_string(),
        ))
        .await
        .expect("send");
    let frame = websocket.next().await.expect("frame").expect("ok");
    let message: Value = serde_json::from_str(frame.to_text().expect("text")).expect("json");
    assert_eq!(message["id"], 7, "the ping answered over the websocket");

    let tcp = TcpStream::connect(&addr).await.expect("tcp");
    let refused = tokio_tungstenite::client_async(format!("ws://{addr}/"), tcp).await;
    assert!(refused.is_err(), "a tokenless upgrade is refused");

    let answer = unix_request(&host, "httpServe", json!({"enabled": false})).await;
    assert!(answer["result"]["url"].is_null());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        TcpStream::connect(&addr).await.is_err(),
        "the stopped listener no longer accepts"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_http_face_serves_static_files_with_isolation_headers() {
    let dir = tempdir();
    let web = dir.path().join("web");
    std::fs::create_dir_all(&web).expect("web root");
    std::fs::write(web.join("index.html"), "<head><title>x</title></head>ok").expect("index");
    std::fs::write(web.join("web.wasm"), b"\0asm").expect("wasm");
    std::fs::write(web.join("web.wasm.gz"), b"gzip bytes").expect("gzip wasm");
    std::fs::write(web.join("web.wasm.br"), b"brotli bytes").expect("brotli wasm");
    let hashed_wasm = format!("web-{}.wasm", "0".repeat(64));
    std::fs::write(web.join(&hashed_wasm), b"\0asm hashed").expect("hashed wasm");
    std::fs::write(web.join(format!("{hashed_wasm}.br")), b"hashed brotli")
        .expect("hashed brotli wasm");
    let grammars = web.join("grammars");
    std::fs::create_dir_all(&grammars).expect("grammar root");
    let hashed_query = format!("rust-{}.scm", "1".repeat(64));
    std::fs::write(grammars.join(&hashed_query), b"(identifier) @variable")
        .expect("hashed grammar query");
    std::fs::write(web.join("font.woff2"), b"woff2 bytes").expect("font");
    std::fs::write(web.join("font.woff2.gz"), b"gzip font").expect("gzip font");
    std::fs::write(web.join("font.woff2.br"), b"brotli font").expect("brotli font");

    let host = host_at(dir.path());
    let answer = unix_request(
        &host,
        "httpServe",
        json!({"enabled": true, "webRoot": web.display().to_string()}),
    )
    .await;
    let url = answer["result"]["url"].as_str().expect("url").to_owned();
    let (addr, token) = addr_and_token(&url);

    let get = |target: String, accept_encoding: Option<&'static str>, cookie: Option<String>| {
        let addr = addr.clone();
        async move {
            let mut tcp = TcpStream::connect(&addr).await.expect("tcp");
            let accept_encoding = accept_encoding
                .map(|value| format!("Accept-Encoding: {value}\r\n"))
                .unwrap_or_default();
            let cookie = cookie
                .map(|value| format!("Cookie: {value}\r\n"))
                .unwrap_or_default();
            tcp.write_all(
                format!(
                    "GET {target} HTTP/1.1\r\nHost: {addr}\r\n{accept_encoding}{cookie}Connection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .expect("send");
            let mut bytes = Vec::new();
            tcp.read_to_end(&mut bytes).await.expect("read");
            String::from_utf8_lossy(&bytes).into_owned()
        }
    };

    assert!(get("/".to_owned(), None, None)
        .await
        .starts_with("HTTP/1.1 403"));
    let index = get(format!("/?tkn={token}"), Some("br, gzip"), None).await;
    assert!(index.starts_with("HTTP/1.1 200"), "{index}");
    assert_eq!(
        response_header(&index, "cross-origin-opener-policy"),
        Some("same-origin")
    );
    assert_eq!(
        response_header(&index, "cross-origin-embedder-policy"),
        Some("require-corp")
    );
    assert!(response_header(&index, "set-cookie")
        .is_some_and(|cookie| cookie.starts_with(&format!("himark_tkn={token}"))));
    assert!(index.contains(&format!(
        "window.HIMARK_AHP_URL=\"ws://{addr}/?tkn={token}\""
    )));
    assert_eq!(response_header(&index, "content-encoding"), None);

    let wasm = get(
        "/web.wasm".to_owned(),
        None,
        Some(format!("himark_tkn={token}")),
    )
    .await;
    assert_eq!(
        response_header(&wasm, "content-type"),
        Some("application/wasm")
    );
    assert_eq!(response_header(&wasm, "content-encoding"), None);
    assert_eq!(response_header(&wasm, "cache-control"), Some("no-cache"));
    assert!(wasm.ends_with("\0asm"));

    let immutable = get(format!("/{hashed_wasm}?tkn={token}"), Some("br"), None).await;
    assert_eq!(
        response_header(&immutable, "cache-control"),
        Some("private, max-age=31536000, immutable")
    );
    assert_eq!(response_header(&immutable, "content-encoding"), Some("br"));
    assert!(immutable.ends_with("hashed brotli"));

    let immutable_query = get(format!("/grammars/{hashed_query}?tkn={token}"), None, None).await;
    assert_eq!(
        response_header(&immutable_query, "cache-control"),
        Some("private, max-age=31536000, immutable")
    );
    assert!(immutable_query.ends_with("(identifier) @variable"));

    let brotli = get(format!("/web.wasm?tkn={token}"), Some("gzip, br"), None).await;
    assert_eq!(response_header(&brotli, "content-encoding"), Some("br"));
    assert_eq!(response_header(&brotli, "vary"), Some("accept-encoding"));
    assert!(brotli.ends_with("brotli bytes"), "{brotli}");

    let gzip = get(format!("/web.wasm?tkn={token}"), Some("gzip"), None).await;
    assert_eq!(response_header(&gzip, "content-encoding"), Some("gzip"));
    assert!(gzip.ends_with("gzip bytes"), "{gzip}");

    let no_brotli = get(format!("/web.wasm?tkn={token}"), Some("br;q=0, gzip"), None).await;
    assert_eq!(
        response_header(&no_brotli, "content-encoding"),
        Some("gzip")
    );

    let font_brotli = get(format!("/font.woff2?tkn={token}"), Some("gzip, br"), None).await;
    assert_eq!(
        response_header(&font_brotli, "content-type"),
        Some("font/woff2")
    );
    assert_eq!(
        response_header(&font_brotli, "content-encoding"),
        Some("br")
    );
    assert!(font_brotli.ends_with("brotli font"), "{font_brotli}");

    let font_gzip = get(format!("/font.woff2?tkn={token}"), Some("gzip"), None).await;
    assert_eq!(
        response_header(&font_gzip, "content-type"),
        Some("font/woff2")
    );
    assert_eq!(
        response_header(&font_gzip, "content-encoding"),
        Some("gzip")
    );
    assert!(font_gzip.ends_with("gzip font"), "{font_gzip}");

    let escape = get(format!("/../host.lock?tkn={token}"), None, None).await;
    assert!(escape.starts_with("HTTP/1.1 404"), "{escape}");
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}
