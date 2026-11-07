// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The PRODUCTION PLUGIN AGGREGATE and the shared host install:
//! the grammar pack, the engine feature plugins, and the wiring
//! every shell boot shares — the client directory, the docsync
//! channels, the filesystem routes, the add-host flow. No window,
//! no workbench, no C: the iOS boot and the desktop boot compose
//! the same pieces.

use std::sync::Arc;

use ahp_docsync as docsync;
use ahp_session::fsroute;
use himark::app::{AppCommand, Himark};

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


/// The ENGINE plugins every shell installs — no window, no
/// workbench: the grammar pack, markdown, the AHP feature set.
pub fn engine_plugins() -> Vec<Box<dyn himark::plugin::Plugin>> {
    vec![
        Box::new(LanguagePack),
        Box::new(himarkdown::MarkdownPlugin),
        Box::new(hiahp::higent::SessionsPlugin),
        Box::new(hiahp::AhpPlugin),
        Box::new(hiahp::hicomments::CommentsPlugin),
        Box::new(hiahp::hichanges::ChangesPlugin),
        Box::new(hiahp::save::SavePlugin),
        Box::new(hiahp::watch::WatchPlugin),
        Box::new(hiahp::hipeek::PeekPlugin),
        Box::new(hiterminal::TerminalPlugin),
        Box::new(hicode::CodePlugin),
    ]
}

/// The wires `install_host` hands back — the late registrations
/// (routes for effects whose types sit above himark) close over
/// them.
pub struct Wires {
    pub clients: Arc<ahp_wire::fs::ClientDirectory>,
    pub languages: Arc<editor::reparse::SyntaxLanguages>,
    pub channels: Arc<docsync::DocumentChannels>,
    pub resource_uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
}

/// The store-in-hand host mint shared by discovery and the add-host
/// flow — seats before seeding, so the row never connects against an
/// unregistered client.
pub fn register_agent_server(
    app: &mut Himark,
    clients: &ahp_wire::fs::ClientDirectory,
    name: &str,
    client: ahp_wire::client::Client,
) -> ahp_wire::client::HostId {
    let id = hiahp::engine::register_client(app, client.clone());
    app.setup(|store| {
        ahp_session::session::agents::Agents::seed(store, id, name);
        ahp_session::session::state::Hosts::install_uris(
            store,
            id,
            Arc::new(ahp_wire::uris::FileUris),
        );
    });
    clients.record(id, client);
    id
}

/// Find — or autostart — the local himark agent host. The lockfile
/// dance is the EMBEDDER's: the wire takes a resolver and never
/// touches discovery itself.
pub fn himark_host_resolver() -> Arc<dyn Fn() -> Result<String, String> + Send + Sync> {
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

pub fn autostart_himark_host() -> Option<String> {
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



/// The shared HOST INSTALL: the host loop, the poster-backed doors
/// (docsync, the client directory), the plugin pass, the add-host
/// flow and the filesystem routes. Callers hand the connector and
/// the plugin list; the workbench boot and the iOS boot differ only
/// in those.
pub fn install_host(
    app: &mut Himark,
    connector: Arc<dyn ahp_wire::transport::Connector>,
    resource_uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
    plugins: &[Box<dyn himark::plugin::Plugin>],
) -> Wires {
    // The host edge is the ENGINE'S (docs/himark-core.md): the
    // inbox, the wakes and the owned runtime install into himark;
    // every thread-crossing door below rides the poster.
    app.install_host_loop();
    let poster = app.poster();
    let post: Arc<dyn Fn(AppCommand) + Send + Sync> =
        Arc::new(move |command| poster.post(command));
    let runtime = app.runtime();
    let installed = himark::plugin::install_all(app, Arc::new(myersdiff::Myers), plugins);
    wire_doors(
        app,
        runtime,
        post,
        connector,
        resource_uris,
        Arc::clone(&installed.languages),
    )
}

/// The WIRE DOORS alone — channels, the client directory, the
/// fsroute handlers, the docsync hook, the add-host flow — over a
/// runtime handle and a cross-thread post the CALLER owns: the
/// desktop host hands in the engine's (see `install_host`); the web
/// shell hands its emscripten wire thread's.
pub fn wire_doors(
    app: &mut Himark,
    runtime: tokio::runtime::Handle,
    post: Arc<dyn Fn(AppCommand) + Send + Sync>,
    connector: Arc<dyn ahp_wire::transport::Connector>,
    resource_uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
    languages: Arc<editor::reparse::SyntaxLanguages>,
) -> Wires {
    let channels = docsync::DocumentChannels::new(
        runtime.clone(),
        {
            let post = Arc::clone(&post);
            Arc::new(move |verb| post(AppCommand::Verb(verb)))
        },
        Arc::clone(&resource_uris),
    );

    let clients = Arc::new(ahp_wire::fs::ClientDirectory::new({
        let post = Arc::clone(&post);
        Arc::new(move |subscription| {
            post(AppCommand::FileChanged(documents::watch::Subscription(
                subscription,
            )));
        })
    }));

    app.setup(|store| store.put(hiahp::engine::WireClients(Arc::clone(&clients))));

    // hicode's definition ask routes over the wire — the HOST
    // registers it, closed over its client directory.
    app.register_handler::<hicode::FindDefinitionEffect>(hicode::lsproute::DefinitionRoute {
        directory: Arc::clone(&clients),
        uris: Arc::clone(&resource_uris),
    });

    {
        let clients = clients.clone();
        let handle = runtime.clone();
        let connector = Arc::clone(&connector);
        app.setup(|store| {
            hiahp::higent::flows::AgentFlows::install_add_host(
                store,
                Arc::new(move |store, url| {
                    let client =
                        ahp_wire::client::Client::of(Arc::new(ahp_wire::wire::WireHost::at(
                            handle.clone(),
                            Arc::clone(&connector),
                            url.to_owned(),
                        )));
                    // Minted into the STORE IN HAND — this runs inside
                    // a batch, where the application's store is taken
                    // out; a mint through the app would be clobbered
                    // by the put-back.
                    Some(hiahp::higent::flows::mint_agent_server(
                        store,
                        &clients,
                        url.trim(),
                        client,
                    ))
                }),
            )
        });
    }

    app.setup(|store| {
        documents::OpenDocuments::install_hook(
            store,
            Arc::new(docsync::DocsyncHook {
                channels: Arc::clone(&channels),
                directory: Arc::clone(&clients),
            }),
        )
    });
    app.register_handler::<documents::FetchDocumentEffect>(fsroute::RouteFetch {
        uris: Arc::clone(&resource_uris),
        directory: Arc::clone(&clients),
    });
    app.register_handler::<documents::FetchResourceBytesEffect>(fsroute::RouteFetchBytes {
        directory: Arc::clone(&clients),
        uris: Arc::clone(&resource_uris),
    });
    app.register_handler::<documents::StoreDocumentEffect>(fsroute::RouteStore {
        directory: Arc::clone(&clients),
        uris: Arc::clone(&resource_uris),
        channels: Arc::clone(&channels),
    });
    app.register_handler::<documents::ListDirectoryEffect>(fsroute::RouteList {
        directory: Arc::clone(&clients),
        uris: Arc::clone(&resource_uris),
    });
    app.register_handler::<documents::CreateDocumentEffect>(fsroute::RouteCreate {
        directory: Arc::clone(&clients),
        uris: Arc::clone(&resource_uris),
    });
    app.register_handler::<documents::DeleteResourceEffect>(fsroute::RouteDelete {
        directory: Arc::clone(&clients),
        uris: Arc::clone(&resource_uris),
    });
    app.register_handler::<documents::MoveResourceEffect>(fsroute::RouteMove {
        directory: Arc::clone(&clients),
        uris: Arc::clone(&resource_uris),
    });
    app.register_handler::<documents::watch::SubscribeEffect>(fsroute::RouteSubscribe {
        directory: Arc::clone(&clients),
        uris: Arc::clone(&resource_uris),
    });
    app.register_handler::<documents::watch::UnsubscribeEffect>(fsroute::RouteUnsubscribe {
        directory: Arc::clone(&clients),
    });
    app.observe_file_changes();

    Wires {
        clients,
        languages,
        channels,
        resource_uris,
    }
}

pub fn agent_host_autostart() -> bool {
    host_discovery::autostart().is_some()
}
