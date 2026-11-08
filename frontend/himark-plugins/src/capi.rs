// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The C FACE and the PRODUCTION assembly: `himark_production_plugins`
//! (the plugin-set mint `himark_create` consumes), the shipping
//! plugin list, the outward agent-host dialing, and the re-exports
//! that fill the apple staticlib with every extern the headers
//! speak. Behind the `capi` feature — the bare engine half of this
//! crate never links a window or a connector.

use std::sync::Arc;

pub use himark::app::AppFonts;
pub use std::sync::Arc as _A;

/// The engine, its C surface and every symbol the header speaks —
/// re-exported whole so the staticlib carries them and the tests
/// keep their vocabulary.
pub use himark_ffi::*;
pub use workbench_shell::capi::*;

/// Boot the engine over the HOST'S plugin list — the one
/// construction every shell shares. The host's callbacks are NOT
/// seated here: they are `himark_create`'s construction parameters
/// (a Rust composition root seats them itself).
pub fn with_plugins(
    fonts: AppFonts,
    plugins: &[Box<dyn himark::plugin::Plugin>],
    connector: Arc<dyn ahp_wire::transport::Connector>,
) -> (HimarkEngine, crate::Wires) {
    let mut app = workbench_shell::app::workbench_app(fonts);
    let resource_uris: Arc<dyn ahp_wire::client::ResourceUriMap> =
        Arc::new(ahp_wire::uris::FileUris);

    let wires = crate::install_host(
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
    let engine = himark_ffi::assemble(app);
    (
        engine,
        crate::Wires {
            clients,
            languages,
            channels: document_channels,
            resource_uris,
        },
    )
}
/// Everything this host installs beyond its own FFI and wiring.
fn plugins() -> Vec<Box<dyn himark::plugin::Plugin>> {
    let mut plugins = crate::engine_plugins();
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

pub use crate::agent_host_autostart;

/// The PRODUCTION engine: the shared boot plus what belongs to the
/// shipping entry alone — the outward hosts are dialed here, never
/// in a test's engine (a test engine that registered the discovered
/// host would reach the machine's REAL agent host and pull the
/// user's catalog into its drawers).
pub fn production_engine() -> HimarkEngine {
    production_engine_at(None)
}

/// `data_dir` is the shell's data root (the iOS container's
/// Application Support); None takes the desktop dotdir.
pub fn production_engine_at(data_dir: Option<std::path::PathBuf>) -> HimarkEngine {
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
    let (mut engine, wires) =
        with_plugins(AppFonts::platform(), &plugins(), Arc::clone(&connector));
    engine.with_store(|store| store.put(workbench_shell::capi::ComposeNewWindows(true)));
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        let _ = host_discovery::autostart();
        hiahp::agent_fs::install_routes(
            &mut engine,
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
    crate::register_agent_server(
        &mut engine,
        &wires.clients,
        "VS Code Agent Host",
        ahp_wire::client::Client::of(Arc::new(ahp_wire::wire::WireHost::new(
            runtime.clone(),
            Arc::clone(&connector),
        ))),
    );
    let local_backend = crate::register_agent_server(
        &mut engine,
        &wires.clients,
        "himark Agent Host",
        ahp_wire::client::Client::of(Arc::new(ahp_wire::wire::WireHost::discovered(
            runtime,
            Arc::clone(&connector),
            crate::himark_host_resolver(),
        ))),
    );
    wires.clients.set_local(local_backend);
    hiahp::engine::set_local_host(&mut engine, local_backend);
    #[cfg(unix)]
    hitailnet::install(
        &mut engine,
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
    );
    engine
}

/// The PRODUCTION plugin set, minted for `himark_create`: the whole
/// shipping composition — plugin list, wire doors, outward dialing —
/// deferred into one boot. `data_dir`/`data_dir_len` name the
/// shell's data root (UTF-8, no NUL; null means the desktop dotdir).
#[no_mangle]
pub unsafe extern "C" fn himark_production_plugins(
    data_dir: *const std::os::raw::c_char,
    data_dir_len: usize,
) -> *mut HimarkPlugins {
    let data_dir = (!data_dir.is_null() && data_dir_len > 0)
        .then(|| {
            let bytes = std::slice::from_raw_parts(data_dir as *const u8, data_dir_len);
            std::str::from_utf8(bytes)
                .ok()
                .map(std::path::PathBuf::from)
        })
        .flatten();
    Box::into_raw(Box::new(HimarkPlugins(Box::new(move || {
        production_engine_at(data_dir)
    }))))
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
        let clients = hiahp::engine::wire_clients(self.store())
            .expect("the wire directory is seated at boot");
        crate::register_agent_server(self, &clients, name, client)
    }

    fn set_local_backend(&mut self, server: ahp_wire::client::HostId) {
        if let Some(clients) = hiahp::engine::wire_clients(self.store()) {
            clients.set_local(server);
        }
        hiahp::engine::set_local_host(self, server);
    }
}
