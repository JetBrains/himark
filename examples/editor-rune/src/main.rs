// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The SMALLEST himark editor on rune — the second acid test of
//! the library: the same bare boot as the gpui example, mounted
//! into a rune app through one effect slot. Rune keeps the window,
//! layout, animation and input; himark paints the document.
//!
//! ```sh
//! cargo run -p editor-rune -- path/to/file.rs
//! ```

use std::sync::Arc;

use himark::app::{AppFonts, Himark, Shell};
use himark_rune::rune::{self, el};
use himark_rune::{BridgeView, Engine};

/// A shell that claims no places — the example shows one document.
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

struct EditorApp {
    view: BridgeView,
    engine: Engine,
}

const EDITOR_MARK: u32 = 7001;

fn view(s: &EditorApp, _cx: &mut rune::Cx<EditorApp>) -> rune::El {
    el::el().grow(1.0).child(s.view.el::<EditorApp>().grow(1.0))
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
                "# himark on rune\n\nPass a file path to open it:\n\n```sh\ncargo run -p editor-rune -- src/main.rs\n```\n".to_owned(),
            )
        });

    // The BARE engine + the engine plugin set — identical to the
    // gpui example: the adapter is the only line that differs.
    let mut app = Himark::bare(AppFonts::platform(), Arc::new(NoShell));
    app.install_host_loop();
    let plugins = himark_plugins::engine_plugins();
    let wired = himark::plugin::install_all(&mut app, Arc::new(myersdiff::Myers), &plugins);
    let languages = wired.languages;

    let documents = imba::store::Id::<documents::OpenDocuments>::mint();
    let ui = app.ui_ctx();
    let mut registered = None;
    app.with_store(|store| {
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
    let bridge = himark::mounts::mount_scrolling_editor(&mut app, documents, document)
        .expect("the editor bridge");

    let engine = Engine::adopt(app);
    let view_half = BridgeView::new(engine.clone(), bridge, EDITOR_MARK);
    let compositor = view_half.compositor();

    let spec = rune::AppSpec {
        setup: Some(|s: &mut EditorApp, waker| s.engine.set_waker(waker)),
        poll: Some(|s: &mut EditorApp, _t, _tx| s.engine.drain()),
        ..rune::AppSpec::new("himark · rune", (980.0, 720.0), || unreachable!("init is captured"), view)
    };
    rune::App::new(spec)
        .init(move || EditorApp {
            view: view_half,
            engine,
        })
        .compositor(compositor)
        .run();
}
