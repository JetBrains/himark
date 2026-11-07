// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The gpui shell: a normal shell like any other, it just happens
//! to be written in Rust. THE COMPOSITION ROOT — it builds the
//! engine out of the library parts (workbench boot, plugin list,
//! host wiring, agent-host dialing, tailnet) the way the Swift
//! shell's staticlib does in C, then hands the composed engine to
//! the gpui adapter and mounts the workbench window's bridge.

use std::sync::Arc;

use himark::app::{AppFonts, Himark};
use himark::app_ext::AppExt as _;
use himark::bridge::ViewBridgeId;
use himark_gpui::gpui::{
    px, size, App, AppContext as _, Application, Bounds, WindowBounds, WindowOptions,
};
use himark_gpui::{BridgeView, Engine};
use workbench_shell::app::WorkbenchShell as _;
use workbench_shell::app_ext::AppExt as _;

/// Everything this host installs — the engine set plus the
/// workbench chrome, the same list the C staticlib assembles.
fn plugins() -> Vec<Box<dyn himark::plugin::Plugin>> {
    let mut plugins = himark_plugins::engine_plugins();
    plugins.extend([
        Box::new(palette::PalettePlugin) as Box<dyn himark::plugin::Plugin>,
        Box::new(peeker::PeekerPlugin),
        Box::new(workbench_shell::diff_pane::DiffPanePlugin),
        Box::new(demo::DemoPlugin),
        Box::new(hiscript::plugin::ScriptPlugin),
        Box::new(workbench_shell::higent::AgentsPlugin),
    ]);
    plugins.extend(workbench_shell::plugins());
    plugins
}

/// The production engine: the shared boot plus the outward dialing
/// that belongs to a shipping entry alone.
fn compose() -> (Himark, himark_plugins::Wires) {
    let data_root = hitailnet::data_root(None);
    // The tailnet-aware connector: ts.net and CGNAT destinations go
    // through the embedded node (started lazily on the first such
    // dial), every other URL takes the plain desktop paths.
    let tailnet = Arc::new(desktop::tailnet_connector::TailnetConnector::new(
        hitailnet::client_node_dir(&data_root),
    ));
    let connector: Arc<dyn ahp_wire::transport::Connector> = tailnet.clone();

    let mut app = workbench_shell::app::workbench_app(AppFonts::platform());
    let resource_uris: Arc<dyn ahp_wire::client::ResourceUriMap> =
        Arc::new(ahp_wire::uris::FileUris);
    let wires = himark_plugins::install_host(
        &mut app,
        Arc::clone(&connector),
        Arc::clone(&resource_uris),
        &plugins(),
    );

    let _ = host_discovery::autostart();
    host_discovery::logging::init("app");

    // The agent-host filesystem routes — every capability, like the
    // desktop C shell asks for.
    hiahp::install_open_handlers(
        &mut app,
        Arc::clone(&wires.languages),
        Arc::new(myersdiff::Myers),
    );
    app.observe_stripe_bases(Arc::new(ahp_session::fsroute::resolve_base));
    app.register_handler::<editor::completion::CompletionEffect>(ahp_lsp::CompletionRoute {
        directory: Arc::clone(&wires.clients),
        uris: Arc::clone(&wires.resource_uris),
    });
    app.register_handler::<editor::hover::HoverEffect>(ahp_lsp::HoverRoute {
        directory: Arc::clone(&wires.clients),
        uris: Arc::clone(&wires.resource_uris),
    });
    app.register_handler::<ahp_locations::LspLocationsEffect>(
        ahp_locations::routes::RouteLspLocations {
            directory: Arc::clone(&wires.clients),
            uris: Arc::clone(&wires.resource_uris),
        },
    );
    app.register_handler::<editor::completion::PathCompletionEffect>(
        ahp_locations::find::NativeFindHandler {
            directory: Arc::clone(&wires.clients),
        },
    );
    app.register_handler::<ahp_locations::FindEffect>(ahp_locations::find::NativeFindHandler {
        directory: Arc::clone(&wires.clients),
    });
    app.register_handler::<ahp_locations::SearchLocationsEffect>(
        ahp_locations::routes::RouteSearchLocations {
            directory: Arc::clone(&wires.clients),
        },
    );
    app.register_handler::<ahp_locations::SymbolLocationsEffect>(
        ahp_locations::routes::RouteSymbolLocations {
            directory: Arc::clone(&wires.clients),
        },
    );

    // The outward hosts — dialed here, never in a test's engine.
    let runtime = app.runtime();
    himark_plugins::register_agent_server(
        &mut app,
        &wires.clients,
        "VS Code Agent Host",
        ahp_wire::client::Client::of(Arc::new(ahp_wire::wire::WireHost::new(
            runtime.clone(),
            Arc::clone(&connector),
        ))),
    );
    let local = himark_plugins::register_agent_server(
        &mut app,
        &wires.clients,
        "himark Agent Host",
        ahp_wire::client::Client::of(Arc::new(ahp_wire::wire::WireHost::discovered(
            runtime,
            Arc::clone(&connector),
            himark_plugins::himark_host_resolver(),
        ))),
    );
    wires.clients.set_local(local);
    app.designate_local_host(local);

    hitailnet::install(&mut app, &wires.clients, tailnet, connector, data_root);

    (app, wires)
}

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

    let _ = himark_plugins::agent_host_autostart();

    let (mut app, wires) = compose();
    let window = app.add_window();
    for (index, (name, source)) in documents.into_iter().enumerate() {
        let languages = Arc::clone(&wires.languages);
        let document_name = name.clone();
        app.open_async(window, name, index == 0, None, move |store, ui, theme| {
            ahp_chat::open::document_for(&languages, &document_name, &source, store, ui, theme)
        });
    }
    let bridge = ViewBridgeId::from_raw(window.raw());

    Application::new().run(move |cx: &mut App| {
        let engine = Engine::adopt(app, cx);

        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
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
        .expect("the himark window");
    });
}
