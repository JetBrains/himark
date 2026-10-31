// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The gpui shell: a normal shell like any other, it just happens
//! to be written in Rust. Starts the agent host, boots the engine,
//! opens one gpui window, and mounts the workbench window's bridge.
//! Everything else lives in `himark-gpui`.

use himark_gpui::gpui::{
    px, size, App, AppContext as _, Application, Bounds, WindowBounds, WindowOptions,
};
use himark_gpui::{BridgeView, Engine};

fn main() {
    let documents: Vec<(String, String)> = std::env::args_os()
        .skip(1)
        .filter_map(|arg| {
            let path = std::path::PathBuf::from(arg);
            let source = std::fs::read_to_string(&path).ok()?;
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            Some((name, source))
        })
        .collect();

    let _ = himark_gpui::agent_host_autostart();

    Application::new().run(move |cx: &mut App| {
        let engine = Engine::boot(cx);
        let window = engine.add_window();
        for (index, (name, source)) in documents.iter().enumerate() {
            engine.open_document(window, name, source, index == 0);
        }

        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.activate(true);
                let bridge = engine.window_bridge(window);
                cx.new(|cx| BridgeView::new(engine, bridge, cx))
            },
        )
        .expect("the himark window");
    });
}
