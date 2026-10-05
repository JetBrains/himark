// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The adapter moved to the `hiahp` crate; the shell keeps the path
//! alive plus the WINDOW rims the crate cannot hold.


// ---------------------------------------------------------------
// The WINDOW rims of the open roads: fetch-then-open lands an
// AppCommand into a window; the crate keeps the windowless build
// and resolve machinery.

use std::sync::Arc;

use documents::FetchDocumentEffect;
use editor::location::ResourceLocation;
use imba::effect::{AnyEffect, EffectHandler};
use imba::store::Store;

use crate::app::AppCommand;
use crate::commands::WindowedCommand;
use crate::workspace::OpenByLocationEffect;

/// Building a document from text in hand (Text, layout, syntax) is not a
/// filesystem capability — the chat's cells build off-thread with no host
/// in sight. It installs at boot, on its own.
pub fn install_build_handler(
    app: &mut crate::app::Application,
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
    app: &mut crate::app::Application,
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
    app.register_handler::<crate::workspace::OpenDiffByLocationsEffect>(OpenDiffByLocationsHandler(
        shop.clone(),
    ));
    app.register_handler::<documents::diff_views::OpenDiffPairEffect>(
        ::ahp_chat::open::OpenDiffPairHandler(shop),
    );
    app.register_windowed_navigator(DiffNavigator);
}

struct OpenDiffByLocationsHandler(::ahp_chat::open::DiffOpenShop);

impl EffectHandler<crate::workspace::OpenDiffByLocationsEffect> for OpenDiffByLocationsHandler {
    async fn handle(&self, effect: crate::workspace::OpenDiffByLocationsEffect) -> AppCommand {
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
            return AppCommand::Windowed(effect.window, Arc::new(FetchFailed { location }));
        }
        AppCommand::Windowed(
            effect.window,
            Arc::new(OpenDiffPair {
                window: effect.window,
                documents: effect.documents,
                pair,
            }),
        )
    }
}

pub struct DiffNavigator;

impl ::workbench::navigation::WindowedNavigator for DiffNavigator {
    type Place = canvas::diff_pane::DiffPlace;

    fn navigate(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        place: &canvas::diff_pane::DiffPlace,
        fx: &mut imba::command::Fx<'_>,
    ) -> Option<::workbench::workbench_node::Panel> {
        // Resolve both sides on the UI thread — an open side hands over
        // its live snapshot; the prep runs off-thread and the landing
        // opens the dressed pane. Diffing never runs here.
        let documents = crate::workspace::session_state(store, window)
            .expect("a diff opens from a window with a session")
            .documents();
        let old =
            documents::diff_views::DiffSideInput::resolve(store, documents, place.old.clone());
        let new =
            documents::diff_views::DiffSideInput::resolve(store, documents, place.new.clone());
        fx.push(
            AnyEffect::new(crate::workspace::OpenDiffByLocationsEffect {
                window,
                documents,
                old,
                new,
            })
            .map(crate::app::shell_verb),
        );
        None
    }
}

pub struct OpenDiffPair {
    window: ::workbench::window::WindowId,
    documents: imba::store::Id<documents::OpenDocuments>,
    pair: documents::diff_views::OpenedDiffPair,
}

impl WindowedCommand for OpenDiffPair {
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
        _window: ::workbench::window::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let ui = ui;
        let _ = crate::diff_pane::open_opened_diff_pane(
            store,
            ui,
            self.window,
            self.documents,
            self.pair.clone(),
            fx,
        );
        fx.scope(crate::app::AppCommand::Verb, |fx| {
                documents::lanes::sync_document_watches(store, self.documents, fx)
            });
        fx.scope(AppCommand::Verb, |fx| {
            documents::lanes::sync_stripe_bases(store, self.documents, ui, fx)
        });
    }
}

pub struct OpenByLocationHandler {
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
                        &self.workshop.fonts(),
                        &self.workshop.theme(),
                    )
                });
                AppCommand::Opened(
                    effect.window,
                    crate::app::OpenedDocument {
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

            None => AppCommand::Windowed(
                effect.window,
                Arc::new(FetchFailed {
                    location: effect.location,
                }),
            ),
        }
    }
}

pub struct FetchFailed {
    location: ResourceLocation,
}

impl WindowedCommand for FetchFailed {
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
        _window: ::workbench::window::WindowId,
        _fx: &mut crate::app::AppFx<'_>,
    ) {
        eprintln!("[himark] fetch failed: {:?}", self.location);
    }
}

pub fn register_all(app: &mut crate::app::Application) {
    app.register_handler::<ahp_wire::effects::ConnectServerEffect>(
        ::ahp_wire::registry::HandleConnectServer,
    );
    app.register_handler::<ahp_wire::effects::ListSessionsEffect>(
        ::ahp_wire::registry::HandleListSessions,
    );
    app.register_handler::<ahp_wire::effects::PollServerEffect>(::ahp_wire::registry::HandlePollServer);
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
    app.register_handler::<ahp_wire::effects::PollSessionEffect>(::ahp_wire::registry::HandlePollSession);
    app.register_handler::<ahp_wire::effects::CreateChatEffect>(::ahp_wire::registry::HandleCreateChat);
    app.register_handler::<ahp_wire::effects::SubscribeChatEffect>(
        ::ahp_wire::registry::HandleSubscribeChat,
    );
    app.register_handler::<ahp_wire::effects::FetchTurnsEffect>(::ahp_wire::registry::HandleFetchTurns);
    app.register_handler::<ahp_wire::effects::StartTurnEffect>(::ahp_wire::registry::HandleStartTurn);
    app.register_handler::<ahp_wire::effects::PollChatActionsEffect>(
        ::ahp_wire::registry::HandlePollChatActions,
    );
    app.register_handler::<ahp_wire::effects::CancelTurnEffect>(::ahp_wire::registry::HandleCancelTurn);
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
}
