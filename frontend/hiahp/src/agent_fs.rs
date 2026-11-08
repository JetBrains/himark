// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The agent-host FILESYSTEM routes — what a host that can fetch,
//! store and list unlocks. Capability facts arrive from the shell
//! (the C callbacks, a composition root's grant); the AHP route
//! registrations live here.

use std::sync::Arc;

use ahp_locations::find;

/// Which agent-host filesystem capabilities are wired. A store
/// memo — repeated installs add only what is new.
#[derive(Clone, Copy, Default)]
pub struct AgentFsInstalled {
    pub fetch_document: bool,
    pub store_document: bool,
    pub list_directory: bool,
}

/// The AHP routes the agent-host FILESYSTEM capabilities unlock:
/// the open/diff handlers and the LSP asks for fetch, the find and
/// search routes for list. The capability FACTS are the C layer's;
/// these registrations are this crate's.
pub fn install_routes(
    app: &mut himark::app::Himark,
    fetch_document: bool,
    store_document: bool,
    list_directory: bool,
    languages: &Arc<editor::reparse::SyntaxLanguages>,
    clients: &Arc<ahp_wire::fs::ClientDirectory>,
    resource_uris: &Arc<dyn ahp_wire::client::ResourceUriMap>,
) {
    let installed = app
        .store()
        .get::<AgentFsInstalled>()
        .copied()
        .unwrap_or_default();

    if fetch_document && !installed.fetch_document {
        crate::install_open_handlers(app, Arc::clone(languages), Arc::new(myersdiff::Myers));
        app.with_store(|store| {
            documents::diffs::StripeBases::install(
                store,
                Arc::new(ahp_session::fsroute::resolve_base),
            );
        });
        app.register_handler::<::editor::completion::CompletionEffect>(ahp_lsp::CompletionRoute {
            directory: Arc::clone(clients),
            uris: Arc::clone(resource_uris),
        });
        app.register_handler::<::editor::hover::HoverEffect>(ahp_lsp::HoverRoute {
            directory: Arc::clone(clients),
            uris: Arc::clone(resource_uris),
        });
        app.register_handler::<ahp_locations::LspLocationsEffect>(
            ahp_locations::routes::RouteLspLocations {
                directory: Arc::clone(clients),
                uris: Arc::clone(resource_uris),
            },
        );
    }
    if list_directory && !installed.list_directory {
        app.register_handler::<::editor::completion::PathCompletionEffect>(
            find::NativeFindHandler {
                directory: Arc::clone(clients),
            },
        );
        app.register_handler::<ahp_locations::FindEffect>(find::NativeFindHandler {
            directory: Arc::clone(clients),
        });
        app.register_handler::<ahp_locations::SearchLocationsEffect>(
            ahp_locations::routes::RouteSearchLocations {
                directory: Arc::clone(clients),
            },
        );
        app.register_handler::<ahp_locations::SymbolLocationsEffect>(
            ahp_locations::routes::RouteSymbolLocations {
                directory: Arc::clone(clients),
            },
        );
    }

    let _ = store_document;
    app.with_store(|store| {
        store.put(AgentFsInstalled {
            fetch_document: installed.fetch_document || fetch_document,
            store_document: installed.store_document || store_document,
            list_directory: installed.list_directory || list_directory,
        })
    });
}
