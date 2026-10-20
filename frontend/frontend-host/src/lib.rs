// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The PRODUCTION HOST: the plugin list, the grammar pack, the
//! outward agent-host dialing and the few doors whose types sit
//! above the engine (the demo documents, hicode's wire route). The
//! engine and its whole C surface live in `himark::ffi` — this crate
//! is what a shipping shell links, and REGISTRATION is its one job.

use std::sync::Arc;

use himark::app::AppFonts;
use himark::app_ext::AppExt;

/// The engine, its C surface and every symbol the header speaks —
/// re-exported whole so the staticlib carries them and the tests
/// keep their vocabulary.
pub use himark::ffi::*;

/// The grammar pack as a plugin: every tree-sitter language this
/// host ships, plus the enrichment passes of crates that sit below
/// himark and cannot speak the plugin trait themselves (hisitter's
/// caret passes, himermaid's renders).
struct LanguagePack;

impl himark::plugin::Plugin for LanguagePack {
    fn languages(&self, languages: &mut editor::reparse::SyntaxLanguages) {
        hirust::register(languages);
        hipython::register(languages);
        hijavascript::register(languages);
        hitypescript::register(languages);
        higo::register(languages);
        hijava::register(languages);
        hic::register(languages);
        hicpp::register(languages);
        hicsharp::register(languages);
        hiruby::register(languages);
        hiphp::register(languages);
        hibash::register(languages);
        hilua::register(languages);
        hiswift::register(languages);
        hikotlin::register(languages);
        hiscala::register(languages);
        hihaskell::register(languages);
        hielixir::register(languages);
        hiocaml::register(languages);
        hizig::register(languages);
        hisql::register(languages);
        hijson::register(languages);
        hicss::register(languages);
        hihtml::register(languages);
        hiyaml::register(languages);
        hitoml::register(languages);
        hicmake::register(languages);
        hid::register(languages);
        hidart::register(languages);
        hielm::register(languages);
        hierlang::register(languages);
        hifortran::register(languages);
        hifsharp::register(languages);
        higleam::register(languages);
        higlsl::register(languages);
        higraphql::register(languages);
        higroovy::register(languages);
        hihcl::register(languages);
        hijulia::register(languages);
        himake::register(languages);
        hinix::register(languages);
        hiobjc::register(languages);
        hiodin::register(languages);
        hiperl::register(languages);
        hipowershell::register(languages);
        hir::register(languages);
        hisolidity::register(languages);
        hixml::register(languages);
        himermaid::register(languages);
    }

    fn enrichers(&self, enrichers: &mut editor::enrich::Enrichers) {
        himermaid::register_enricher(enrichers);
        hisitter::caret_passes::register_caret_enrichers(enrichers);
    }
}

/// Everything this host installs beyond its own FFI and wiring —
/// each crate registers its own pieces, in this order.
fn plugins() -> Vec<Box<dyn himark::plugin::Plugin>> {
    let mut plugins: Vec<Box<dyn himark::plugin::Plugin>> = vec![
        Box::new(LanguagePack),
        Box::new(himarkdown::MarkdownPlugin),
        Box::new(palette::PalettePlugin),
        Box::new(peeker::PeekerPlugin),
        Box::new(himark::higent::SessionsPlugin),
        Box::new(himark::diff_pane::DiffPanePlugin),
        Box::new(demo::DemoPlugin),
        Box::new(himark::hiahp::AhpPlugin),
        Box::new(himark::hicomments::CommentsPlugin),
        Box::new(himark::higent::AgentsPlugin),
        Box::new(himark::hichanges::ChangesPlugin),
        Box::new(himark::save::SavePlugin),
        Box::new(himark::watch::WatchPlugin),
        Box::new(himark::hipeek::PeekPlugin),
        Box::new(hicode::CodePlugin),
        Box::new(hiscript::plugin::ScriptPlugin),
    ];
    // The WORKBENCH CHROME: dock toggles, toolbar buttons, the
    // terminal pane, the feed-dock frontend. Every current shell
    // still installs it (the desktop's palette lists what it always
    // listed); the shell-flavored boot peels it off the mounted
    // shells next.
    plugins.extend(workbench_shell::plugins());
    plugins
}

/// Find — or autostart — the local himark agent host. The lockfile
/// dance is the EMBEDDER's: the wire takes a resolver and never
/// touches discovery itself.
fn himark_host_resolver() -> Arc<dyn Fn() -> Result<String, String> + Send + Sync> {
    Arc::new(|| {
        if let Some(lock) = host_discovery::default_dir()
            .as_deref()
            .and_then(host_discovery::read_live)
        {
            return Ok(format!("unix:{}", lock.socket.display()));
        }
        autostart_himark_host().ok_or_else(|| {
            "no himark agent host running — start one with `himark-agent-host`".to_owned()
        })
    })
}

fn autostart_himark_host() -> Option<String> {
    let gate = std::env::var("HIMARK_HOST_AUTOSTART").ok()?;
    let dir = host_discovery::default_dir()?;
    let binary = match gate.as_str() {
        "1" => host_discovery::resolve_daemon_binary()?,
        path => std::path::PathBuf::from(path),
    };
    std::process::Command::new(binary)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    for _ in 0..100 {
        if let Some(lock) = host_discovery::read_live(&dir) {
            return Some(format!("unix:{}", lock.socket.display()));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    None
}


/// The PRODUCTION engine: the shared boot plus what belongs to the
/// shipping entry alone — the outward hosts are dialed here, never
/// in a test's engine (a test engine that registered the discovered
/// host would reach the machine's REAL agent host and pull the
/// user's catalog into its drawers).
pub fn production_engine(callbacks: Option<HimarkEngineCallbacks>) -> HimarkEngine {
    let connector: Arc<dyn ahp_wire::transport::Connector> =
        Arc::new(desktop::connector::DesktopConnector);
    let mut engine = HimarkEngine::with_plugins(
        AppFonts::platform(),
        &plugins(),
        Arc::clone(&connector),
        callbacks,
    );
    engine.compose_new_windows = true;
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        let _ = host_discovery::autostart();
        engine.install_agent_host_filesystem(AgentHostFilesystemCapabilities::all());
    }
    host_discovery::logging::init("app");
    engine.register_agent_server(
        "VS Code Agent Host",
        ahp_wire::client::Client::of(Arc::new(ahp_wire::wire::WireHost::new(
            engine.runtime(),
            Arc::clone(&connector),
        ))),
    );
    let local_backend = engine.register_agent_server(
        "himark Agent Host",
        ahp_wire::client::Client::of(Arc::new(ahp_wire::wire::WireHost::discovered(
            engine.runtime(),
            Arc::clone(&connector),
            himark_host_resolver(),
        ))),
    );
    engine.set_local_backend(local_backend);
    engine
}

#[cfg(test)]
pub(crate) fn test_connector() -> Arc<dyn ahp_wire::transport::Connector> {
    Arc::new(desktop::connector::DesktopConnector)
}

/// The TEST boot: the same plugins, no outward dialing.
#[cfg(test)]
pub(crate) fn with_fonts(fonts: AppFonts) -> HimarkEngine {
    HimarkEngine::with_plugins(
        fonts,
        &plugins(),
        Arc::new(desktop::connector::DesktopConnector),
        None,
    )
}

pub fn agent_host_autostart() -> bool {
    host_discovery::autostart().is_some()
}

#[no_mangle]
pub extern "C" fn himark_create(callbacks: HimarkEngineCallbacks) -> *mut HimarkEngine {
    Box::into_raw(Box::new(production_engine(Some(callbacks))))
}


#[no_mangle]
pub unsafe extern "C" fn himark_open_demo(
    engine: *mut HimarkEngine,
    bridge: himark::ffi::bridges::HimarkBridge,
) {
    if let Some(engine) = engine.as_mut() {
        engine.app.open_async(
            ::workbench::window::WindowId::from_raw(bridge.raw),
            "torture sample".to_owned(),
            true,
            Some(demo::demo_location("torture sample")),
            demo::monster_document,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_open_demo_wall(
    engine: *mut HimarkEngine,
    bridge: himark::ffi::bridges::HimarkBridge,
) {
    if let Some(engine) = engine.as_mut() {
        engine.app.open_async(
            ::workbench::window::WindowId::from_raw(bridge.raw),
            "wall of text".to_owned(),
            true,
            Some(demo::demo_location("wall of text")),
            demo::wall_of_text_document,
        );
    }
}


#[cfg(test)]
mod tests;
#[cfg(test)]
mod chat_flow_tests;
#[cfg(test)]
mod diagnostics_flow_tests;
#[cfg(test)]
mod findroute_tests;
#[cfg(test)]
mod reconnect_tests;
