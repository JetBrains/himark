// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The plugin door: a crate registers its own pieces — grammars,
//! enrichment passes, commands, effect handlers, place arms,
//! factories — instead of a host hand-wiring them at boot. The host
//! keeps what is genuinely its own (FFI, wake/drain, the client
//! directory, capability-gated routes) and installs everything else
//! as a plugin list.

use std::sync::Arc;

use crate::app::Himark;

/// One crate's registrations. The collection hooks feed the ONE
/// languages set and the ONE enrichers registry (several plugins
/// contribute to each — a whole-set `register_*` per plugin would
/// clobber); `install` gets the app and the collected context.
pub trait Plugin {
    /// Tree-sitter grammars and language facts, collected into the
    /// set every document parses against.
    fn languages(&self, _languages: &mut ::editor::reparse::SyntaxLanguages) {}

    /// Enrichment passes, collected into the one registry.
    fn enrichers(&self, _enrichers: &mut ::editor::enrich::Enrichers) {}

    /// Commands, effect handlers, hooks, place arms, factories.
    fn install(&self, _app: &mut Himark, _cx: &PluginContext) {}
}

/// What `install` may lean on beyond the app: the sets collected
/// from every plugin, shared — handlers that reparse (the open
/// road, the chat's cells) take the languages by Arc.
pub struct PluginContext {
    pub languages: Arc<::editor::reparse::SyntaxLanguages>,
    pub diff_policy: Arc<dyn ::editor::diff::DiffPolicy>,
    /// The wire client directory and the uri map — a plugin whose
    /// effects route over the wire closes its handlers over these.
    pub clients: Arc<ahp_wire::fs::ClientDirectory>,
    pub resource_uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
}

/// Collect, register, install — in list order (toolbar buttons and
/// palette rows keep the order their plugins stand in). Returns the
/// context so the host can keep the shared sets for its own late
/// wiring (capability-gated routes re-use the languages).
pub fn install_all(
    app: &mut Himark,
    diff_policy: Arc<dyn ::editor::diff::DiffPolicy>,
    clients: Arc<ahp_wire::fs::ClientDirectory>,
    resource_uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
    plugins: &[Box<dyn Plugin>],
) -> PluginContext {
    use crate::app_ext::AppExt;

    let mut languages = ::editor::reparse::SyntaxLanguages::new();
    let mut enrichers = ::editor::enrich::Enrichers::new();
    for plugin in plugins {
        plugin.languages(&mut languages);
        plugin.enrichers(&mut enrichers);
    }
    let shared = Arc::new(languages.clone());
    app.register_syntax_languages(languages);
    app.register_enrichers(enrichers);
    app.register_diff_policy(Arc::clone(&diff_policy));
    let cx = PluginContext {
        languages: shared,
        diff_policy,
        clients,
        resource_uris,
    };
    for plugin in plugins {
        plugin.install(app, &cx);
    }
    cx
}
