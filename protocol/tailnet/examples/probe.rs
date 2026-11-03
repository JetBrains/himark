// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Live smoke probe: enroll a client node, dial an agent-host's
//! tailnet face, do the websocket handshake and one JSON-RPC
//! exchange. Usage:
//!
//!   cargo run -p tailnet --example probe -- ws://himark-smoke.tailXXXX.ts.net:7297/

use futures_util::{SinkExt as _, StreamExt as _};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .expect("usage: probe ws://host:7297/");
    let state = std::env::temp_dir().join("tailnet-smoke-client");
    let node = tailnet::Tailnet::start(&state, "himark-client-probe").expect("node starts");

    let mut last_url = String::new();
    loop {
        let status = node.status().await.expect("status");
        if status.running() {
            println!(
                "client node running as {}",
                status
                    .self_node
                    .map(|own| own.dns_name)
                    .unwrap_or_default()
            );
            break;
        }
        if !status.auth_url.is_empty() && status.auth_url != last_url {
            last_url = status.auth_url.clone();
            println!("CLIENT LOGIN: open {last_url}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    let rest = url.strip_prefix("ws://").expect("a ws:// url");
    let authority = rest.split('/').next().expect("an authority");
    let destination = match authority.contains(':') {
        true => authority.to_owned(),
        false => format!("{authority}:7297"),
    };
    println!("dialing {destination} through the tunnel…");
    let stream = node.dial(destination).await.expect("tailnet dial");
    let (mut socket, response) = tokio_tungstenite::client_async(&url, stream)
        .await
        .expect("websocket handshake");
    println!("websocket upgraded: HTTP {}", response.status());

    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"tailnet-probe","version":"0.0.0"},"protocolVersions":["0.7.0"],"capabilities":{}}}"#;
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            initialize.into(),
        ))
        .await
        .expect("send initialize");
    match socket.next().await {
        Some(Ok(message)) => println!("agent-host answered: {message}"),
        other => println!("no answer: {other:?}"),
    }
}
