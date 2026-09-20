// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use ahp_types::actions::StateAction;
use ahp_types::state::{ChatState, SessionState};
use imba::effect::EffectHandler;

use himark::higent::{
    CancelTurnEffect, ConnectServerEffect, CreateChatEffect, CreateSessionEffect,
    DispatchChatActionEffect, DisposeSessionEffect, FetchFileEditEffect, FetchTurnsEffect,
    FileEditContents, ListSessionsEffect, PollChatActionsEffect, PollServerEffect,
    PollSessionEffect, RootInfo, ServerEvent, SessionsPage, StartTurnEffect, SubscribeChatEffect,
    SubscribeSessionEffect, TurnsPage,
};

pub struct HandleConnectServer;

impl EffectHandler<ConnectServerEffect> for HandleConnectServer {
    async fn handle(&self, effect: ConnectServerEffect) -> Result<RootInfo, String> {
        effect.seat.connect().await
    }
}

pub struct HandleListSessions;

impl EffectHandler<ListSessionsEffect> for HandleListSessions {
    async fn handle(&self, effect: ListSessionsEffect) -> Result<SessionsPage, String> {
        effect.seat.list_sessions(effect.cursor).await
    }
}

pub struct HandlePollServer;

impl EffectHandler<PollServerEffect> for HandlePollServer {
    async fn handle(&self, effect: PollServerEffect) -> Vec<ServerEvent> {
        effect.seat.poll_root().await
    }
}

pub struct HandleCreateSession;

impl EffectHandler<CreateSessionEffect> for HandleCreateSession {
    async fn handle(&self, effect: CreateSessionEffect) -> Result<String, String> {
        effect
            .seat
            .create_session(effect.working_directories, effect.options)
            .await
    }
}

pub struct HandleResolveSessionConfig;

impl EffectHandler<himark::higent::ResolveSessionConfigEffect> for HandleResolveSessionConfig {
    async fn handle(
        &self,
        effect: himark::higent::ResolveSessionConfigEffect,
    ) -> Result<ahp_types::commands::ResolveSessionConfigResult, String> {
        effect
            .seat
            .resolve_session_config(effect.working_directory, effect.config)
            .await
    }
}

pub struct HandleDisposeSession;

impl EffectHandler<DisposeSessionEffect> for HandleDisposeSession {
    async fn handle(&self, effect: DisposeSessionEffect) -> Result<(), String> {
        effect.seat.dispose_session(effect.session).await
    }
}

pub struct HandleSubscribeSession;

impl EffectHandler<SubscribeSessionEffect> for HandleSubscribeSession {
    async fn handle(&self, effect: SubscribeSessionEffect) -> Result<SessionState, String> {
        effect.seat.subscribe_session(effect.session).await
    }
}

pub struct HandlePollSession;

impl EffectHandler<PollSessionEffect> for HandlePollSession {
    async fn handle(&self, effect: PollSessionEffect) -> Vec<StateAction> {
        effect.seat.poll_session(effect.session).await
    }
}

pub struct HandleCreateChat;

impl EffectHandler<CreateChatEffect> for HandleCreateChat {
    async fn handle(&self, effect: CreateChatEffect) -> Result<String, String> {
        effect.seat.create_chat(effect.session).await
    }
}

pub struct HandleSubscribeChat;

impl EffectHandler<SubscribeChatEffect> for HandleSubscribeChat {
    async fn handle(&self, effect: SubscribeChatEffect) -> Result<ChatState, String> {
        effect.seat.subscribe_chat(effect.chat).await
    }
}

pub struct HandleFetchTurns;

impl EffectHandler<FetchTurnsEffect> for HandleFetchTurns {
    async fn handle(&self, effect: FetchTurnsEffect) -> Result<TurnsPage, String> {
        effect.seat.fetch_turns(effect.chat, effect.cursor).await
    }
}

pub struct HandleStartTurn;

impl EffectHandler<StartTurnEffect> for HandleStartTurn {
    async fn handle(&self, effect: StartTurnEffect) -> Result<(), String> {
        effect
            .seat
            .start_turn(effect.chat, effect.text, effect.attachments, effect.model)
            .await
    }
}

pub struct HandleSubscribeChangeset;

impl EffectHandler<himark::higent::SubscribeChangesetEffect> for HandleSubscribeChangeset {
    async fn handle(
        &self,
        effect: himark::higent::SubscribeChangesetEffect,
    ) -> Result<ahp_types::state::ChangesetState, String> {
        effect.seat.subscribe_changeset(effect.channel).await
    }
}

pub struct HandleSubscribeLocations;

impl EffectHandler<himark::higent::SubscribeLocationsEffect> for HandleSubscribeLocations {
    async fn handle(
        &self,
        effect: himark::higent::SubscribeLocationsEffect,
    ) -> Result<himark_ahp_ext_types::LocationList, String> {
        effect.seat.subscribe_locations(effect.channel).await
    }
}

pub struct HandlePollLocations;

impl EffectHandler<himark::higent::PollLocationsEffect> for HandlePollLocations {
    async fn handle(
        &self,
        effect: himark::higent::PollLocationsEffect,
    ) -> Vec<himark_ahp_ext_types::LocationList> {
        effect.seat.poll_locations(effect.channel).await
    }
}

pub struct HandleUnsubscribeLocations;

impl EffectHandler<himark::higent::UnsubscribeLocationsEffect> for HandleUnsubscribeLocations {
    async fn handle(&self, effect: himark::higent::UnsubscribeLocationsEffect) {
        effect.seat.unsubscribe_locations(&effect.channel);
    }
}

pub struct HandleSubscribeHistory;

impl EffectHandler<himark::higent::SubscribeHistoryEffect> for HandleSubscribeHistory {
    async fn handle(
        &self,
        effect: himark::higent::SubscribeHistoryEffect,
    ) -> Result<himark_ahp_ext_types::history::HistoryState, String> {
        effect.seat.subscribe_history(effect.channel).await
    }
}

pub struct HandlePollChangeset;

impl EffectHandler<himark::higent::PollChangesetEffect> for HandlePollChangeset {
    async fn handle(
        &self,
        effect: himark::higent::PollChangesetEffect,
    ) -> Vec<ahp_types::actions::StateAction> {
        effect.seat.poll_changeset(effect.channel).await
    }
}

pub struct HandleSubscribeAnnotations;

impl EffectHandler<himark::higent::SubscribeAnnotationsEffect> for HandleSubscribeAnnotations {
    async fn handle(
        &self,
        effect: himark::higent::SubscribeAnnotationsEffect,
    ) -> Result<ahp_types::state::AnnotationsState, String> {
        effect.seat.subscribe_annotations(effect.session).await
    }
}

pub struct HandlePollAnnotations;

impl EffectHandler<himark::higent::PollAnnotationsEffect> for HandlePollAnnotations {
    async fn handle(
        &self,
        effect: himark::higent::PollAnnotationsEffect,
    ) -> Vec<ahp_types::actions::StateAction> {
        effect.seat.poll_annotations(effect.session).await
    }
}

pub struct HandlePollChatActions;

impl EffectHandler<PollChatActionsEffect> for HandlePollChatActions {
    async fn handle(&self, effect: PollChatActionsEffect) -> Vec<StateAction> {
        effect.seat.poll_chat(effect.chat).await
    }
}

pub struct HandleCancelTurn;

impl EffectHandler<CancelTurnEffect> for HandleCancelTurn {
    async fn handle(&self, effect: CancelTurnEffect) {
        effect.seat.cancel_turn(effect.chat, effect.turn_id).await;
    }
}

pub struct HandleDispatchChatAction;

impl EffectHandler<DispatchChatActionEffect> for HandleDispatchChatAction {
    async fn handle(&self, effect: DispatchChatActionEffect) -> Result<(), String> {
        effect
            .seat
            .dispatch_action(effect.channel, effect.action)
            .await
    }
}

pub struct HandleFetchFileEdit;

impl EffectHandler<FetchFileEditEffect> for HandleFetchFileEdit {
    async fn handle(&self, effect: FetchFileEditEffect) -> Result<FileEditContents, String> {
        effect
            .seat
            .read_file_edit(effect.before, effect.after)
            .await
    }
}

pub fn register_all(app: &mut himark::Application) {
    app.register_handler::<himark::higent::ConnectServerEffect>(HandleConnectServer);
    app.register_handler::<himark::higent::ListSessionsEffect>(HandleListSessions);
    app.register_handler::<himark::higent::PollServerEffect>(HandlePollServer);
    app.register_handler::<himark::higent::CreateSessionEffect>(HandleCreateSession);
    app.register_handler::<himark::higent::ResolveSessionConfigEffect>(HandleResolveSessionConfig);
    app.register_handler::<himark::higent::DisposeSessionEffect>(HandleDisposeSession);
    app.register_handler::<himark::higent::SubscribeSessionEffect>(HandleSubscribeSession);
    app.register_handler::<himark::higent::PollSessionEffect>(HandlePollSession);
    app.register_handler::<himark::higent::CreateChatEffect>(HandleCreateChat);
    app.register_handler::<himark::higent::SubscribeChatEffect>(HandleSubscribeChat);
    app.register_handler::<himark::higent::FetchTurnsEffect>(HandleFetchTurns);
    app.register_handler::<himark::higent::StartTurnEffect>(HandleStartTurn);
    app.register_handler::<himark::higent::PollChatActionsEffect>(HandlePollChatActions);
    app.register_handler::<himark::higent::CancelTurnEffect>(HandleCancelTurn);
    app.register_handler::<himark::higent::DispatchChatActionEffect>(HandleDispatchChatAction);
    app.register_handler::<himark::higent::FetchFileEditEffect>(HandleFetchFileEdit);
    app.register_handler::<himark::higent::SubscribeChangesetEffect>(HandleSubscribeChangeset);
    app.register_handler::<himark::higent::PollChangesetEffect>(HandlePollChangeset);
    app.register_handler::<himark::higent::SubscribeHistoryEffect>(HandleSubscribeHistory);
    app.register_handler::<himark::higent::SubscribeLocationsEffect>(HandleSubscribeLocations);
    app.register_handler::<himark::higent::PollLocationsEffect>(HandlePollLocations);
    app.register_handler::<himark::higent::UnsubscribeLocationsEffect>(HandleUnsubscribeLocations);
    app.register_handler::<himark::higent::SubscribeAnnotationsEffect>(HandleSubscribeAnnotations);
    app.register_handler::<himark::higent::PollAnnotationsEffect>(HandlePollAnnotations);
}
