// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The smallest himark embedding: an EMPTY editor in a gpui window.
//!
//! ```sh
//! cargo run -p hello-gpui
//! ```

use std::sync::Arc;

use himark::app::{AppFonts, Himark, NoShell};
use himark_gpui::gpui::{
    px, size, App, AppContext as _, Application, Bounds, WindowBounds, WindowOptions,
};
use himark_gpui::{BridgeView, Engine};

fn main() {
    // The engine, with one plugin: markdown. No windows, no agent host.
    let mut app = Himark::new(
        AppFonts::platform(),
        Arc::new(NoShell),
        vec![Box::new(himarkdown::MarkdownPlugin)],
    );
    app.install_host_loop();

    // One empty markdown document in a fresh collection, one editor
    // over it.
    let documents = imba::store::Id::<documents::OpenDocuments>::mint();
    let ui = app.ui_ctx();
    let document = app.with_store(|store| {
        let languages = editor::env::Parsers::of(store).expect("the plugin registered markdown");
        let theme = editor::env::Themes::of(store).clone();
        let body = text::text::Text::from_string_exact("");
        let document =
            editor::document::Document::from_language(body, "md", &languages, store, &ui, &theme);
        documents::OpenDocuments::register(store, documents, document, None, "untitled.md".to_owned(), 0)
    });
    let bridge = himark::mounts::mount_scrolling_editor(&mut app, documents, document)
        .expect("the editor bridge");

    // A gpui window around the mounted editor.
    Application::new().run(move |cx: &mut App| {
        let engine = Engine::adopt(app, cx);
        let bounds = Bounds::centered(None, size(px(900.0), px(700.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.activate(true);
                cx.new(|cx| BridgeView::new(engine, bridge, cx))
            },
        )
        .expect("the editor window");
    });
}
