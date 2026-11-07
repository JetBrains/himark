// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Shared helpers of the himark-capi integration tests.
#![allow(dead_code)]

pub mod probe;

use std::sync::Arc;

pub fn bind_backend(dir: &std::path::Path, claude_binary: String) -> String {
    bind_backend_at(dir.join("backend.sock"), dir, claude_binary, Vec::new())
}

/// Bind a host on `socket` over `dir`'s state. A RESTART in a test is
/// this called again: a fresh socket, the SAME `dir` — what a real
/// host keeps (the disk) and what it loses (every in-memory channel,
/// watch, mirror and language server) fall out by construction. The
/// old host thread stays alive but clientless; its socket is cut.
pub fn bind_backend_at(
    socket: std::path::PathBuf,
    dir: &std::path::Path,
    claude_binary: String,
    language_servers: Vec<agent_host::server::LanguageServer>,
) -> String {
    let serving = socket.clone();
    let config = agent_host::server::HostConfig {
        agents: Vec::new(),
        data_dir: dir.join("data"),
        claude_binary,
        codex_binary: "false".to_owned(),
        claude_home: dir.join("dot-claude"),
        codex_home: dir.join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        fsp_binary: None,
        fsp_data_dir: dir.join("fsp"),
        language_servers,
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("backend runtime");
        let backend = agent_host::server::Host::new(config);
        let _ = runtime.block_on(backend.bind(&serving));
    });
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 500, "the backend never bound its socket");
    }
    format!("unix:{}", socket.display())
}

pub fn block_on<T>(
    mut future: std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>,
) -> T {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "future never answered"
                );
                std::thread::park_timeout(std::time::Duration::from_millis(100));
            }
        }
    }
}
