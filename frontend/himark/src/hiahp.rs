// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The adapter moved to the `hiahp` crate; the shell keeps the path
//! alive plus the WINDOW rims the crate cannot hold.

pub use ::ahp_session::{
    docsync, drivers, find, fs, fsroute, locations, lsproute, open, registry, transport, uris,
    uuid_v4, wire,
};

// ---------------------------------------------------------------
// The WINDOW rims of the open roads: fetch-then-open lands an
// AppCommand into a window; the crate keeps the windowless build
// and resolve machinery.

use std::sync::Arc;

use documents::FetchDocumentEffect;
use editor::location::ResourceLocation;
use imba::effect::{AnyEffect, EffectHandler};
use imba::store::Store;

use crate::{AppCommand, AppFx, DynamicCommand, OpenByLocationEffect};

/// Building a document from text in hand (Text, layout, syntax) is not a
/// filesystem capability — the chat's cells build off-thread with no host
/// in sight. It installs at boot, on its own.
pub fn install_build_handler(
    app: &mut crate::Application,
    languages: Arc<editor::reparse::SyntaxLanguages>,
    diff_policy: Arc<dyn ::editor::diff::DiffPolicy>,
) {
    let workshop = Arc::clone(app.workshop());
    let caller = app.effect_caller();
    app.register_handler::<documents::BuildDocumentEffect>(::ahp_session::open::BuildDocumentHandler(
        Arc::clone(&workshop),
        Arc::clone(&languages),
    ));
    app.register_handler::<crate::higent::BuildFileEditEffect>(
        ::ahp_session::open::BuildFileEditHandler {
            caller,
            workshop,
            languages,
            diff_policy,
        },
    );
}

pub fn install_open_handlers(
    app: &mut crate::Application,
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
    let shop = ::ahp_session::open::DiffOpenShop {
        caller,
        workshop,
        languages,
    };
    app.register_handler::<crate::OpenDiffByLocationsEffect>(OpenDiffByLocationsHandler(
        shop.clone(),
    ));
    app.register_handler::<documents::diff_views::OpenDiffPairEffect>(
        ::ahp_session::open::OpenDiffPairHandler(shop),
    );
    app.register_windowed_navigator(DiffNavigator);
}

struct OpenDiffByLocationsHandler(::ahp_session::open::DiffOpenShop);

impl EffectHandler<crate::OpenDiffByLocationsEffect> for OpenDiffByLocationsHandler {
    async fn handle(&self, effect: crate::OpenDiffByLocationsEffect) -> AppCommand {
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
            return AppCommand::Dynamic(effect.window, Arc::new(FetchFailed { location }));
        }
        AppCommand::Dynamic(
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

impl crate::navigation::WindowedNavigator for DiffNavigator {
    type Place = canvas::diff_pane::DiffPlace;

    fn navigate(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        window: crate::WindowId,
        place: &canvas::diff_pane::DiffPlace,
        fx: &mut AppFx<'_>,
    ) -> Option<crate::Panel> {
        // Resolve both sides on the UI thread — an open side hands over
        // its live snapshot; the prep runs off-thread and the landing
        // opens the dressed pane. Diffing never runs here.
        let documents = crate::Windows::session_state(store, window)
            .expect("a diff opens from a window with a session")
            .documents();
        let old =
            documents::diff_views::DiffSideInput::resolve(store, documents, place.old.clone());
        let new =
            documents::diff_views::DiffSideInput::resolve(store, documents, place.new.clone());
        fx.push(AnyEffect::new(crate::OpenDiffByLocationsEffect {
            window,
            documents,
            old,
            new,
        }));
        None
    }
}

pub struct OpenDiffPair {
    window: crate::WindowId,
    documents: imba::store::Id<documents::OpenDocuments>,
    pair: documents::diff_views::OpenedDiffPair,
}

impl DynamicCommand for OpenDiffPair {
    fn id(&self) -> &'static str {
        "vcs.open-diff-pair"
    }
    fn name(&self) -> String {
        "Open Diff Pane".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        _window: crate::WindowId,
        fx: &mut AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let _ = crate::open_opened_diff_pane(
            store,
            ui,
            self.window,
            self.documents,
            self.pair.clone(),
            fx,
        );
        crate::sync_document_watches(store, self.documents, fx);
        fx.scope(AppCommand::Verb, |fx| {
            crate::diffs::sync_stripe_bases(store, self.documents, ui, fx)
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
                    ::ahp_session::open::document_for(
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
                    crate::OpenedDocument {
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

            None => AppCommand::Dynamic(
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

impl DynamicCommand for FetchFailed {
    fn id(&self) -> &'static str {
        "file.fetch-failed"
    }
    fn name(&self) -> String {
        "Fetch Failed".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut AppFx<'_>,
    ) {
        eprintln!("[himark] fetch failed: {:?}", self.location);
    }
}

pub fn register_all(app: &mut crate::Application) {
    app.register_handler::<crate::higent::ConnectServerEffect>(
        ::ahp_session::registry::HandleConnectServer,
    );
    app.register_handler::<crate::higent::ListSessionsEffect>(
        ::ahp_session::registry::HandleListSessions,
    );
    app.register_handler::<crate::higent::PollServerEffect>(::ahp_session::registry::HandlePollServer);
    app.register_handler::<crate::higent::CreateSessionEffect>(
        ::ahp_session::registry::HandleCreateSession,
    );
    app.register_handler::<crate::higent::ResolveSessionConfigEffect>(
        ::ahp_session::registry::HandleResolveSessionConfig,
    );
    app.register_handler::<crate::higent::DisposeSessionEffect>(
        ::ahp_session::registry::HandleDisposeSession,
    );
    app.register_handler::<crate::higent::SubscribeSessionEffect>(
        ::ahp_session::registry::HandleSubscribeSession,
    );
    app.register_handler::<crate::higent::PollSessionEffect>(::ahp_session::registry::HandlePollSession);
    app.register_handler::<crate::higent::CreateChatEffect>(::ahp_session::registry::HandleCreateChat);
    app.register_handler::<crate::higent::SubscribeChatEffect>(
        ::ahp_session::registry::HandleSubscribeChat,
    );
    app.register_handler::<crate::higent::FetchTurnsEffect>(::ahp_session::registry::HandleFetchTurns);
    app.register_handler::<crate::higent::StartTurnEffect>(::ahp_session::registry::HandleStartTurn);
    app.register_handler::<crate::higent::PollChatActionsEffect>(
        ::ahp_session::registry::HandlePollChatActions,
    );
    app.register_handler::<crate::higent::CancelTurnEffect>(::ahp_session::registry::HandleCancelTurn);
    app.register_handler::<crate::higent::DispatchChatActionEffect>(
        ::ahp_session::registry::HandleDispatchChatAction,
    );
    app.register_handler::<crate::higent::FetchFileEditEffect>(
        ::ahp_session::registry::HandleFetchFileEdit,
    );
    app.register_handler::<crate::higent::SubscribeChangesetEffect>(
        ::ahp_session::registry::HandleSubscribeChangeset,
    );
    app.register_handler::<crate::higent::PollChangesetEffect>(
        ::ahp_session::registry::HandlePollChangeset,
    );
    app.register_handler::<crate::higent::SubscribeHistoryEffect>(
        ::ahp_session::registry::HandleSubscribeHistory,
    );
    app.register_handler::<crate::higent::SubscribeLocationsEffect>(
        ::ahp_session::registry::HandleSubscribeLocations,
    );
    app.register_handler::<crate::higent::PollLocationsEffect>(
        ::ahp_session::registry::HandlePollLocations,
    );
    app.register_handler::<crate::higent::UnsubscribeLocationsEffect>(
        ::ahp_session::registry::HandleUnsubscribeLocations,
    );
    app.register_handler::<crate::higent::SubscribeAnnotationsEffect>(
        ::ahp_session::registry::HandleSubscribeAnnotations,
    );
    app.register_handler::<crate::higent::PollAnnotationsEffect>(
        ::ahp_session::registry::HandlePollAnnotations,
    );
}
