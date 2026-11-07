// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The PRODUCTION HOST: the plugin list, the grammar pack, the
//! outward agent-host dialing and the few doors whose types sit
//! above the engine (the demo documents, hicode's wire route). The
//! engine and its whole C surface live in `himark::ffi` — this crate
//! is what a shipping shell links, and REGISTRATION is its one job.

use std::sync::Arc;

pub use std::sync::Arc as _A;
pub use himark::app::AppFonts;

/// The engine, its C surface and every symbol the header speaks —
/// re-exported whole so the staticlib carries them and the tests
/// keep their vocabulary.
pub use himark::app_ext::AppExt;
pub use himark_ffi::*;
pub use workbench_shell::capi::*;


/// Boot the engine over the HOST'S plugin list — the one
/// construction every shell shares. The callbacks apply before
/// the first batch can launch an effect.
pub fn with_plugins(
    fonts: AppFonts,
    plugins: &[Box<dyn himark::plugin::Plugin>],
    connector: Arc<dyn ahp_wire::transport::Connector>,
    callbacks: Option<HimarkEngineCallbacks>,
) -> (HimarkEngine, himark_plugins::Wires) {
    let mut app = workbench_shell::app::workbench_app(fonts);
    let resource_uris: Arc<dyn ahp_wire::client::ResourceUriMap> = Arc::new(ahp_wire::uris::FileUris);

    let wires = himark_plugins::install_host(
        &mut app,
        Arc::clone(&connector),
        Arc::clone(&resource_uris),
        plugins,
    );
    let languages = Arc::clone(&wires.languages);
    let clients = Arc::clone(&wires.clients);
    let document_channels = Arc::clone(&wires.channels);

    // The workbench's capability-gated chrome, seated before the
    // shell can hand callbacks in — then the AHP capi's own seats
    // (navigate exporter, mint rider, host-capability routes).
    workbench_shell::capi::seat_host_chrome(&mut app);
    hiahp::capi::seat_ahp(
        &mut app,
        Arc::clone(&languages),
        Arc::clone(&clients),
        Arc::clone(&resource_uris),
    );
    let mut engine = HimarkEngine::assemble(app, Arc::clone(&languages));
    // The host's callbacks, before any further batch can launch
    // an effect into empty slots.
    if let Some(callbacks) = callbacks {
        engine.set_wake_raw(callbacks.wake, callbacks.ctx);
        engine.set_effect_wake_raw(callbacks.effect_wake, callbacks.ctx);
        if let Some(on_navigate) = callbacks.on_navigate {
            himark_ffi::bridges::seat_callback_shell(&mut engine, callbacks.ctx, on_navigate);
        }
    }
    (
        engine,
        himark_plugins::Wires {
            clients,
            languages,
            channels: document_channels,
            resource_uris,
        },
    )
}
fn app_fonts() -> AppFonts {
AppFonts::platform()
}


/// Everything this host installs beyond its own FFI and wiring.
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
    // The WORKBENCH CHROME: dock toggles, toolbar buttons, the
    // feed-dock frontend. ShellPlugin's second install is a no-op
    // (the boot already seated the furniture).
    plugins.extend(workbench_shell::plugins());
    plugins
}

pub use himark_plugins::agent_host_autostart;

/// The PRODUCTION engine: the shared boot plus what belongs to the
/// shipping entry alone — the outward hosts are dialed here, never
/// in a test's engine (a test engine that registered the discovered
/// host would reach the machine's REAL agent host and pull the
/// user's catalog into its drawers).
pub fn production_engine(callbacks: Option<HimarkEngineCallbacks>) -> HimarkEngine {
    production_engine_at(callbacks, None)
}

/// `data_dir` is the shell's data root (the iOS container's
/// Application Support); None takes the desktop dotdir.
pub fn production_engine_at(
    callbacks: Option<HimarkEngineCallbacks>,
    data_dir: Option<std::path::PathBuf>,
) -> HimarkEngine {
    #[cfg(unix)]
    let data_root = hitailnet::data_root(data_dir);
    #[cfg(not(unix))]
    let _ = data_dir;
    // The tailnet-aware connector: ts.net and CGNAT destinations go
    // through the embedded node (started lazily on the first such
    // dial), every other URL takes the plain desktop paths.
    #[cfg(unix)]
    let tailnet_connector = Arc::new(desktop::tailnet_connector::TailnetConnector::new(
        hitailnet::client_node_dir(&data_root),
    ));
    #[cfg(unix)]
    let connector: Arc<dyn ahp_wire::transport::Connector> = tailnet_connector.clone();
    #[cfg(not(unix))]
    let connector: Arc<dyn ahp_wire::transport::Connector> =
        Arc::new(desktop::connector::DesktopConnector);
    let (mut engine, wires) = with_plugins(
        AppFonts::platform(),
        &plugins(),
        Arc::clone(&connector),
        callbacks,
    );
    engine.compose_new_windows = true;
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        let _ = host_discovery::autostart();
        hiahp::agent_fs::install_routes(
            &mut engine.app,
            true,
            true,
            true,
            &wires.languages,
            &wires.clients,
            &wires.resource_uris,
        );
    }
    host_discovery::logging::init("app");
    let runtime = engine.runtime();
    himark_plugins::register_agent_server(
        &mut engine.app,
        &wires.clients,
        "VS Code Agent Host",
        ahp_wire::client::Client::of(Arc::new(ahp_wire::wire::WireHost::new(
            runtime.clone(),
            Arc::clone(&connector),
        ))),
    );
    let local_backend = himark_plugins::register_agent_server(
        &mut engine.app,
        &wires.clients,
        "himark Agent Host",
        ahp_wire::client::Client::of(Arc::new(ahp_wire::wire::WireHost::discovered(
            runtime,
            Arc::clone(&connector),
            himark_plugins::himark_host_resolver(),
        ))),
    );
    wires.clients.set_local(local_backend);
    hiahp::engine::set_local_host(&mut engine.app, local_backend);
    #[cfg(unix)]
    hitailnet::install(
        &mut engine.app,
        &wires.clients,
        tailnet_connector,
        connector,
        data_root,
    );
    engine
}

/// TEST SUPPORT: the integration tests' connector.
#[doc(hidden)]
pub fn test_connector() -> Arc<dyn ahp_wire::transport::Connector> {
    Arc::new(desktop::connector::DesktopConnector)
}

/// The TEST boot: the same plugins, no outward dialing.
#[doc(hidden)]
pub fn with_fonts(fonts: AppFonts) -> HimarkEngine {
    let (engine, _wires) = with_plugins(
        fonts,
        &plugins(),
        Arc::new(desktop::connector::DesktopConnector),
        None,
    );
    engine
}


/// `data_dir`/`data_dir_len` name the shell's data root (UTF-8, no
/// NUL; null means the desktop dotdir).
#[no_mangle]
pub unsafe extern "C" fn himark_create(
    callbacks: HimarkEngineCallbacks,
    data_dir: *const std::os::raw::c_char,
    data_dir_len: usize,
) -> *mut HimarkEngine {
    let data_dir = (!data_dir.is_null() && data_dir_len > 0)
        .then(|| {
            let bytes = std::slice::from_raw_parts(data_dir as *const u8, data_dir_len);
            std::str::from_utf8(bytes)
                .ok()
                .map(|text| std::path::PathBuf::from(text))
        })
        .flatten();
    Box::into_raw(Box::new(production_engine_at(Some(callbacks), data_dir)))
}


/// Agent-server registration with METHOD syntax over the assembled
/// engine — reads the seated wire directory; the integration tests
/// and the C boot share it.
pub trait EngineAgents {
    fn register_agent_server(
        &mut self,
        name: &str,
        client: ahp_wire::client::Client,
    ) -> ahp_wire::client::HostId;
    fn set_local_backend(&mut self, server: ahp_wire::client::HostId);
}

impl EngineAgents for HimarkEngine {
    fn register_agent_server(
        &mut self,
        name: &str,
        client: ahp_wire::client::Client,
    ) -> ahp_wire::client::HostId {
        let clients = hiahp::engine::wire_clients(self.app.store())
            .expect("the wire directory is seated at boot");
        himark_plugins::register_agent_server(&mut self.app, &clients, name, client)
    }

    fn set_local_backend(&mut self, server: ahp_wire::client::HostId) {
        if let Some(clients) = hiahp::engine::wire_clients(self.app.store()) {
            clients.set_local(server);
        }
        hiahp::engine::set_local_host(&mut self.app, server);
    }
}
