// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The SMALLEST himark editor on gpui — the library's acid test.
//! A bare engine, the engine plugin set (grammars, enrichers,
//! commands — no workbench, no windows, no agent host dialed), one
//! registered document, one scrolled editor bridge mounted into one
//! gpui window:
//!
//! ```sh
//! cargo run -p editor-gpui -- path/to/file.rs
//! ```

use std::sync::Arc;

use himark::app::{AppFonts, Himark, Shell};
use himark_gpui::gpui::{
    px, size, App, AppContext as _, Application, Bounds, WindowBounds, WindowOptions,
};
use himark_gpui::{BridgeView, Engine};

/// A shell that claims no places: every open from the editor
/// (goto-definition into another file, a peek) is DECLINED — the
/// example shows one document, and showing is all it does.
struct NoShell;

impl Shell for NoShell {
    fn navigate(
        &self,
        _origin: himark::bridge::ViewBridgeId,
        _place: hikit::navigation::NavigationLocation,
        _store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        _fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        false
    }
}

fn main() {
    let (name, source) = std::env::args_os()
        .nth(1)
        .and_then(|arg| {
            let path = std::path::PathBuf::from(arg);
            let source = std::fs::read_to_string(&path).ok()?;
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            Some((name, source))
        })
        .unwrap_or_else(|| {
            (
                "welcome.md".to_owned(),
                "# himark\n\nPass a file path to open it:\n\n```sh\ncargo run -p editor-gpui -- src/lib.rs\n```\n".to_owned(),
            )
        });

    // The BARE engine: store, ui, effects, the editor environment —
    // and not one workbench wire.
    let mut app = Himark::bare(AppFonts::platform(), Arc::new(NoShell));
    app.install_host_loop();

    // The engine plugin set — grammars and language facts collect
    // into the one languages set every document parses against.
    let plugins = himark_plugins::engine_plugins();
    let clients = Arc::new(ahp_wire::fs::ClientDirectory::new(Arc::new(
        |_subscription| {},
    )));
    let uris: Arc<dyn ahp_wire::client::ResourceUriMap> = Arc::new(ahp_wire::uris::FileUris);
    let wired = himark::plugin::install_all(
        &mut app,
        Arc::new(myersdiff::Myers),
        clients,
        uris,
        &plugins,
    );
    let languages = wired.languages;

    // Register the document into a FRESH collection — no session
    // owns it; the collection is this example's whole catalog.
    let documents = imba::store::Id::<documents::OpenDocuments>::mint();
    let ui = app.ui_ctx();
    let mut registered = None;
    app.setup(|store| {
        let theme = editor::env::Themes::of(store).clone();
        let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
        let body = text::text::Text::from_string_exact(&source);
        let mut document = if languages.knows(&extension) {
            editor::document::Document::from_language(
                body, &extension, &languages, store, &ui, &theme,
            )
        } else {
            editor::document::Document::plain_text(body)
        };
        if let Some(enrichers) = editor::env::Enrichers::of(store) {
            document.enrich_now(&enrichers, store, &ui, &theme);
        }
        registered = Some(documents::OpenDocuments::register(
            store,
            documents,
            document,
            None,
            name.clone(),
            0,
        ));
    });
    let document = registered.expect("the registered document");

    // ONE call mints the editor bridge; the adapter mounts it.
    let bridge = himark::mounts::mount_scrolling_editor(&mut app, documents, document)
        .expect("the editor bridge");

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
