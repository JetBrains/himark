// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

fn main() {
    let value_of = |flag: &str| {
        std::env::args()
            .skip_while(|argument| argument != flag)
            .nth(1)
    };
    let socket = value_of("--socket").map(std::path::PathBuf::from);
    let http = value_of("--http");
    let web_root = value_of("--web-root").map(std::path::PathBuf::from);
    let tailnet = std::env::args()
        .any(|argument| argument == "--tailscale")
        .then(|| agent_host::TailnetOptions {
            hostname: value_of("--tailscale-hostname"),
        });
    let code = agent_host::run_with_tailnet(
        socket.as_deref(),
        http.as_deref(),
        web_root.as_deref(),
        tailnet,
    );
    std::process::exit(code);
}
