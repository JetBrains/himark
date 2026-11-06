// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The AHP PLUGIN — the agent-protocol half of the app over the
//! himark engine's registered seams: the wire handlers, the session
//! lanes and sweep, the mount binder and file route, the session
//! place arms, the session residents. himark keeps no ahp-*
//! dependency; this crate owns the half.

pub mod arms;
pub mod engine;
pub mod places;
pub mod sessions;
pub mod higent;
pub mod save;
pub mod watch;
pub mod hipeek;
pub mod hichanges;
pub mod hifiles;
pub mod hicomments;

pub use engine::{bound_session, catalog, Catalog, HostRow, SessionRow};

// ---------------------------------------------------------------
// The WINDOW rims of the open roads: fetch-then-open lands an
// AppCommand into a window; the crate keeps the windowless build
// and resolve machinery.

use std::sync::Arc;

use documents::FetchDocumentEffect;
use editor::location::ResourceLocation;
use imba::effect::{AnyEffect, EffectHandler};
use imba::store::Store;

use himark::app::AppCommand;
use himark::commands::HimarkCommand;
use himark::opening::OpenByLocationEffect;

/// The AHP wiring as a plugin: the wire handlers, the build road,
/// and the LSP edit sink — everything the protocol registers
/// unconditionally at boot. The capability-gated open/store/list
/// routes stay the HOST's (they close over its client directory).
pub struct AhpPlugin;

impl himark::plugin::Plugin for AhpPlugin {
    fn install(&self, app: &mut himark::app::Himark, cx: &himark::plugin::PluginContext) {
        register_all(app);
        install_build_handler(app, Arc::clone(&cx.languages), Arc::clone(&cx.diff_policy));
        app.setup(|store| ::ahp_lsp::enrich::install_edit_sink(store));
        app.setup(|store| {
            ::editor::change_sink::InstalledChangeSink::install(
                store,
                Arc::new(::ahp_docsync::SyncSink),
            )
        });
    }
}

/// Building a document from text in hand (Text, layout, syntax) is not a
/// filesystem capability — the chat's cells build off-thread with no host
/// in sight. It installs at boot, on its own.
pub fn install_build_handler(
    app: &mut himark::app::Himark,
    languages: Arc<editor::reparse::SyntaxLanguages>,
    diff_policy: Arc<dyn ::editor::diff::DiffPolicy>,
) {
    let workshop = Arc::clone(app.workshop());
    let caller = app.effect_caller();
    app.register_handler::<documents::BuildDocumentEffect>(::ahp_chat::open::BuildDocumentHandler(
        Arc::clone(&workshop),
        Arc::clone(&languages),
    ));
    app.register_handler::<ahp_chat::file_edit::BuildFileEditEffect>(
        ::ahp_chat::open::BuildFileEditHandler {
            caller,
            workshop,
            languages,
            diff_policy,
        },
    );
}


pub fn install_open_handlers(
    app: &mut himark::app::Himark,
    languages: Arc<editor::reparse::SyntaxLanguages>,
    diff_policy: Arc<dyn editor::diff::DiffPolicy>,
) {
    let caller = app.effect_caller();
    let workshop = Arc::clone(app.workshop());
    app.register_handler::<OpenByLocationEffect>(OpenByLocationHandler {
        caller: caller.clone(),
        workshop: Arc::clone(&workshop),
        languages: Arc::clone(&languages),
    });
    let _ = &diff_policy;
    let shop = ::ahp_chat::open::DiffOpenShop {
        caller,
        workshop,
        languages,
    };
    app.register_handler::<himark::opening::OpenDiffByLocationsEffect>(
        OpenDiffByLocationsHandler(shop.clone()),
    );
    app.register_handler::<documents::diff_views::OpenDiffPairEffect>(
        ::ahp_chat::open::OpenDiffPairHandler(shop),
    );
}

struct OpenDiffByLocationsHandler(::ahp_chat::open::DiffOpenShop);

impl EffectHandler<himark::opening::OpenDiffByLocationsEffect> for OpenDiffByLocationsHandler {
    async fn handle(&self, effect: himark::opening::OpenDiffByLocationsEffect) -> AppCommand {
        // The pane's editor width is resolved by `install_opened_pair`
        // (non-embedded → OPEN_HALF_WIDTH); the carried width is unused.
        let pair = self.0.open_pair(effect.old, effect.new, 0.0).await;
        if pair.failed {
            // Both sides absent → both were fetched, so a built side
            // carries its location for the notice.
            let location = [&pair.old, &pair.new]
                .into_iter()
                .find_map(|side| match side {
                    documents::diff_views::DiffSide::Built { location, .. } => {
                        Some(location.clone())
                    }
                    documents::diff_views::DiffSide::Open(_) => None,
                })
                .expect("a failed pair has a built side");
            return himark::perform::AppCommand::AtOrigin(effect.origin, Arc::new(FetchFailed { location }));
        }
        himark::perform::AppCommand::AtOrigin(
            effect.origin,
            Arc::new(OpenDiffPair {
                documents: effect.documents,
                pair,
            }),
        )
    }
}



pub struct OpenDiffPair {
    documents: imba::store::Id<documents::OpenDocuments>,
    pair: documents::diff_views::OpenedDiffPair,
}

impl HimarkCommand for OpenDiffPair {
    fn id(&self) -> &'static str {
        "vcs.open-diff-pair"
    }
    fn name(&self) -> String {
        "Open Diff Pane".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        // ONE road for every shell: install the pair and hand the
        // SEATED SHELL a READY place — the workbench's arm opens the
        // split-diff panel, the native shell pushes a screen and
        // mints the standalone split-diff over it.
        let Some(origin) = himark::commands::bound_origin(self.id(), bound) else {
            return;
        };
        if !self.open_at(store, ui, origin, fx) {
            eprintln!("[himark] open-diff-pair could not install its pair — dropped");
        }
    }
}

impl OpenDiffPair {
    fn open_at(
        &self,
        store: &mut Store,
        ui: &imba::ui::UiCtx,
        origin: himark::bridge::ViewBridgeId,
        fx: &mut himark::app::AppFx<'_>,
    ) -> bool {
        let Some(view) = documents::diff_views::install_opened_pair(
            store,
            self.documents,
            ui,
            self.pair.clone(),
            false,
        ) else {
            return false;
        };
        let title = match &self.pair.new {
            documents::diff_views::DiffSide::Open(id) => {
                documents::OpenDocuments::name(store, self.documents, *id).unwrap_or_default()
            }
            documents::diff_views::DiffSide::Built { location, .. } => location.name().to_owned(),
        };
        fx.scope(AppCommand::Verb, |fx| {
            documents::lanes::sync_document_watches(store, self.documents, fx);
            documents::lanes::sync_stripe_bases(store, self.documents, ui, fx);
            himark::shell::navigate_origin(
                store,
                ui,
                origin,
                himark::places::DiffViewPlace {
                    documents: self.documents,
                    view,
                    title: title.clone(),
                },
                fx,
            );
        });
        true
    }
}

pub(crate) struct OpenByLocationHandler {
    pub caller: imba::effect::EffectCaller,
    pub workshop: Arc<editor::env::Workshop>,
    pub languages: Arc<editor::reparse::SyntaxLanguages>,
}

impl EffectHandler<OpenByLocationEffect> for OpenByLocationHandler {
    async fn handle(&self, effect: OpenByLocationEffect) -> AppCommand {
        let fetched = self
            .caller
            .call(FetchDocumentEffect {
                location: effect.location.clone(),
            })
            .await;
        match fetched.flatten() {
            Some(text) => {
                let document = self.workshop.with_ctx(|store, ui| {
                    ::ahp_chat::open::document_for(
                        &self.languages,
                        effect.location.name(),
                        &text,
                        store,
                        ui,
                        &self.workshop.theme(),
                    )
                });
                himark::opening::opened(
                    effect.origin,
                    himark::opening::OpenedDocument {
                        documents: effect.documents,
                        name: effect.location.name().to_owned(),
                        document,
                        location: Some(effect.location),
                        primary: effect.primary,
                        target: effect.target,
                        focus: effect.focus,
                    },
                )
            }

            None => himark::perform::AppCommand::AtOrigin(
                effect.origin,
                Arc::new(FetchFailed {
                    location: effect.location,
                }),
            ),
        }
    }
}

pub(crate) struct FetchFailed {
    location: ResourceLocation,
}

impl HimarkCommand for FetchFailed {
    fn id(&self) -> &'static str {
        "file.fetch-failed"
    }
    fn name(&self) -> String {
        "Fetch Failed".to_owned()
    }
    fn perform(
        &self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _bound: &imba::focus::Bindings,
        _fx: &mut himark::app::AppFx<'_>,
    ) {
        eprintln!("[himark] fetch failed: {:?}", self.location);
    }
}

pub fn register_all(app: &mut himark::app::Himark) {
    app.register_handler::<ahp_wire::effects::ConnectServerEffect>(
        ::ahp_wire::registry::HandleConnectServer,
    );
    app.register_handler::<ahp_wire::effects::ListSessionsEffect>(
        ::ahp_wire::registry::HandleListSessions,
    );
    app.register_handler::<ahp_wire::effects::PollServerEffect>(
        ::ahp_wire::registry::HandlePollServer,
    );
    app.register_handler::<ahp_wire::effects::CreateSessionEffect>(
        ::ahp_wire::registry::HandleCreateSession,
    );
    app.register_handler::<ahp_wire::effects::ResolveSessionConfigEffect>(
        ::ahp_wire::registry::HandleResolveSessionConfig,
    );
    app.register_handler::<ahp_wire::effects::DisposeSessionEffect>(
        ::ahp_wire::registry::HandleDisposeSession,
    );
    app.register_handler::<ahp_wire::effects::SubscribeSessionEffect>(
        ::ahp_wire::registry::HandleSubscribeSession,
    );
    app.register_handler::<ahp_wire::effects::PollSessionEffect>(
        ::ahp_wire::registry::HandlePollSession,
    );
    app.register_handler::<ahp_wire::effects::CreateChatEffect>(
        ::ahp_wire::registry::HandleCreateChat,
    );
    app.register_handler::<ahp_wire::effects::SubscribeChatEffect>(
        ::ahp_wire::registry::HandleSubscribeChat,
    );
    app.register_handler::<ahp_wire::effects::FetchTurnsEffect>(
        ::ahp_wire::registry::HandleFetchTurns,
    );
    app.register_handler::<ahp_wire::effects::StartTurnEffect>(
        ::ahp_wire::registry::HandleStartTurn,
    );
    app.register_handler::<ahp_wire::effects::PollChatActionsEffect>(
        ::ahp_wire::registry::HandlePollChatActions,
    );
    app.register_handler::<ahp_wire::effects::CancelTurnEffect>(
        ::ahp_wire::registry::HandleCancelTurn,
    );
    app.register_handler::<ahp_wire::effects::DispatchChatActionEffect>(
        ::ahp_wire::registry::HandleDispatchChatAction,
    );
    app.register_handler::<ahp_wire::effects::FetchFileEditEffect>(
        ::ahp_wire::registry::HandleFetchFileEdit,
    );
    app.register_handler::<ahp_wire::effects::SubscribeChangesetEffect>(
        ::ahp_wire::registry::HandleSubscribeChangeset,
    );
    app.register_handler::<ahp_wire::effects::PollChangesetEffect>(
        ::ahp_wire::registry::HandlePollChangeset,
    );
    app.register_handler::<ahp_wire::effects::SubscribeHistoryEffect>(
        ::ahp_wire::registry::HandleSubscribeHistory,
    );
    app.register_handler::<ahp_wire::effects::SubscribeLocationsEffect>(
        ::ahp_wire::registry::HandleSubscribeLocations,
    );
    app.register_handler::<ahp_wire::effects::PollLocationsEffect>(
        ::ahp_wire::registry::HandlePollLocations,
    );
    app.register_handler::<ahp_wire::effects::UnsubscribeLocationsEffect>(
        ::ahp_wire::registry::HandleUnsubscribeLocations,
    );
    app.register_handler::<ahp_wire::effects::SubscribeAnnotationsEffect>(
        ::ahp_wire::registry::HandleSubscribeAnnotations,
    );
    app.register_handler::<ahp_wire::effects::PollAnnotationsEffect>(
        ::ahp_wire::registry::HandlePollAnnotations,
    );
    app.register_handler::<ahp_wire::effects::LspDiagnosticsChannelEffect>(
        ::ahp_wire::registry::HandleLspDiagnosticsChannel,
    );
    app.register_handler::<ahp_wire::effects::SubscribeLspDiagnosticsEffect>(
        ::ahp_wire::registry::HandleSubscribeLspDiagnostics,
    );
    app.register_handler::<ahp_wire::effects::PollLspDiagnosticsEffect>(
        ::ahp_wire::registry::HandlePollLspDiagnostics,
    );
    app.register_handler::<ahp_lsp::enrich::LspSemanticTokensEffect>(
        ahp_lsp::enrich::HandleSemanticTokens::default(),
    );
    app.register_handler::<ahp_lsp::enrich::LspInlayHintsEffect>(ahp_lsp::enrich::HandleInlayHints);
}

/// The SESSION LANES as one registered batch lane — exactly the
/// block `perform_batch` used to hardwire: the resident sessions
/// subscription, then every session's sync sweep (diff, stripes,
/// dressing, changes views, canvases, comments, diagnostics,
/// enrich, history/changes/locations), and enrich's drop-unclaimed
/// housekeeping on every batch, quiet or not. Registered by the
/// workbench boot today; moves to the AHP plugin with the rest of
/// this module.
pub fn session_lanes() -> himark::himark::BatchLane {
    Arc::new(|store, ui, all_quiet, fx| {
        // A QUIET batch (every arm display-neutral) skips the
        // session lanes whole: they react to landings and edits,
        // and a quiet batch certified it made none — this is most
        // of what a per-frame observation batch used to cost.
        if !all_quiet {
            // The resident sessions subscription: hosts connect, list
            // and drain whether or not any view shows them — the
            // catalog stays true at all times (views only dress it).
            fx.scope(AppCommand::Verb, |fx| {
                ahp_session::session::driver::sync_sessions_lane(&mut *store, fx)
            });
            // The lanes run over EVERY session: each drains its own
            // pending queue, so a clean session costs map reads — and a
            // landing's session gets its sweep THIS batch whatever the
            // batch's scope (the old tail served only the LAST scope's
            // session, so a cross-session batch starved the others).
            let probe_lanes = std::env::var_os("HIMARK_TRACE_DIFF").is_some();
            for state in ahp_session::session::state::Hosts::states(&*store) {
                let documents = state.documents();
                let started = probe_lanes.then(std::time::Instant::now);
                fx.scope(AppCommand::Verb, |fx| {
                    documents::lanes::sync_diff_lanes(&mut *store, documents, fx)
                });
                if let Some(started) = started.filter(|started| started.elapsed().as_millis() >= 1)
                {
                    eprintln!("[lanes] sync_diff_lanes: {:?}", started.elapsed());
                }
                let started = probe_lanes.then(std::time::Instant::now);
                fx.scope(AppCommand::Verb, |fx| {
                    documents::lanes::sync_scroll_stripe_lanes(&mut *store, documents, fx)
                });
                if let Some(started) = started.filter(|started| started.elapsed().as_millis() >= 1)
                {
                    eprintln!("[lanes] sync_scroll_stripe_lanes: {:?}", started.elapsed());
                }
                // The DRESSING sweep: any view whose basis lags its
                // pair resyncs NOW, id-routed — a normalize landing
                // and its re-dress share a batch, and no face waits
                // for paint.
                let started = probe_lanes.then(std::time::Instant::now);
                fx.scope(AppCommand::Verb, |fx| {
                    documents::lanes::sync_diff_dressing(&mut *store, documents, ui, fx)
                });
                if let Some(started) = started.filter(|started| started.elapsed().as_millis() >= 1)
                {
                    eprintln!("[lanes] sync_diff_dressing: {:?}", started.elapsed());
                }
                // The dock tree views ride the push road too: a
                // changes / history feed landing refreshes a mounted
                // stale view in the SAME batch — no paint probe.
                changesview::changes_view::sync_changes_views(
                    &mut *store,
                    state.changes(),
                    ui,
                );
                // The canvases sync against the fresh document/diff/
                // changeset state — the SAME batch a feed landed in, a
                // direct lane over the sets that own them. The
                // dressed-views queue drains here: the session's own
                // note (id-routed landings appended mid-batch), taken
                // by the one lane that reads it.
                let dressed = documents::OpenDocuments::take_dressed(&mut *store, documents);
                fx.scope(AppCommand::Verb, |fx| {
                    canvas::canvases::sync_canvases(
                        &mut *store,
                        state.canvas_router(),
                        ui,
                        &dressed,
                        fx,
                    )
                });
                // The comments lane: the model's announce and card
                // notes drain onto the wire — a clean collection
                // costs a map read.
                fx.scope(AppCommand::Verb, |fx| {
                    ahp_comments::sync(&mut *store, state.comments_wire(), ui, fx)
                });
                // The diagnostics lane: dirty resources land their
                // squiggle markups into the session's open documents.
                fx.scope(AppCommand::Verb, |fx| {
                    ahp_lsp::diagnostics::sync(
                        &mut *store,
                        state.diagnostics_wire(),
                        ui,
                        fx,
                    )
                });
                // The language-services pull lane: edited and newly
                // opened documents re-ask their semantic tokens and
                // inlay hints.
                fx.scope(AppCommand::Verb, |fx| {
                    ahp_lsp::enrich::sync(&mut *store, state.enrich_wire(), fx)
                });
                // The gesture-ask lanes: the views noted onto their
                // MODELS (grow, commit fetches, refetches); the
                // drivers drain the notes onto the wires here — no
                // view carries a wire or a window for these.
                fx.scope(AppCommand::Verb, |fx| {
                    ahp_changes::history::sync(&mut *store, state.history_wire(), fx);
                    ahp_changes::changes::sync(&mut *store, state.changes_wire(), fx);
                    ahp_locations::driver::sync(
                        &mut *store,
                        ui,
                        state.locations_wire(),
                        fx,
                    );
                });
            }
        }
        // Every session's lane claimed its own edit notes above; what
        // is left names documents open nowhere.
        ahp_lsp::enrich::drop_unclaimed(&*store);
    })
}

/// The empty-session housekeeping `Himark::sweep` runs — the AHP
/// half of the sweep, registered beside [`session_lanes`].
pub fn session_sweep() -> Arc<dyn Fn(&mut imba::store::Store)> {
    Arc::new(|store| ahp_session::session::state::Hosts::sweep_empty(store))
}

/// The session halves of ORIGIN BINDING and the file-watch route —
/// registered facts, not engine wiring: a mounted origin gains its
/// session and `SessionState`, and a watch subscription's change
/// refetches the owning session's documents collection.
pub fn install_session_binding(app: &mut himark::himark::Himark) {
    app.setup(|store| {
        let mut binders = store
            .get::<himark::perform::MountBinders>()
            .cloned()
            .unwrap_or_default();
        binders.0.push(Arc::new(|store, id| {
            let mut bag = imba::focus::Bindings::new();
            if let Some(session) = crate::sessions::session_of(store, id) {
                if let Some(state) =
                    ahp_session::session::state::Hosts::state(store, &session).cloned()
                {
                    bag = bag.bind(state);
                }
                bag = bag.bind(session);
            }
            bag
        }));
        store.put(binders);

        let mut routes = store
            .get::<himark::perform::FileRoutes>()
            .cloned()
            .unwrap_or_default();
        routes.0.push(Arc::new(|store, subscription, fx| {
            if let Some(documents) =
                ahp_session::session::state::Hosts::documents_of_watch(store, subscription)
            {
                fx.scope(himark::app::AppCommand::Verb, |fx| {
                    documents::lanes::refetch_watched(store, documents, subscription, fx)
                });
            }
        }));
        store.put(routes);
    });
}

/// The SESSION HALF's seams, seated once per boot: the batch lanes
/// and sweep, the mount binder and file route, the place arms, the
/// unmount bookkeeping, the residents — everything the engine used
/// to hardwire. The workbench boot calls this today (every real
/// boot passes it); it moves into the plugin when install gains a
/// once-guard.
pub fn install_session_half(app: &mut himark::app::Himark) {
    app.register_batch_lane(session_lanes());
    app.register_sweep_hook(session_sweep());
    app.register_unmount_hook(Arc::new(crate::sessions::forget_session));
    install_session_binding(app);
    crate::arms::install_arms(app);
    crate::engine::install_residents(app);
}
