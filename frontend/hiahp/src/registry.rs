// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use ahp_types::actions::StateAction;
use ahp_types::state::{ChatState, SessionState};
use imba::effect::EffectHandler;

use crate::higent::{
    CancelTurnEffect, ConnectServerEffect, CreateChatEffect, CreateSessionEffect,
    DispatchChatActionEffect, DisposeSessionEffect, FetchFileEditEffect, FetchTurnsEffect,
    FileEditContents, ListSessionsEffect, PollChatActionsEffect, PollServerEffect,
    PollSessionEffect, RootInfo, ServerEvent, SessionsPage, StartTurnEffect, SubscribeChatEffect,
    SubscribeSessionEffect, TurnsPage,
};

pub struct HandleConnectServer;

impl EffectHandler<ConnectServerEffect> for HandleConnectServer {
    async fn handle(&self, effect: ConnectServerEffect) -> Result<RootInfo, String> {
        effect.client.connect().await
    }
}

pub struct HandleListSessions;

impl EffectHandler<ListSessionsEffect> for HandleListSessions {
    async fn handle(&self, effect: ListSessionsEffect) -> Result<SessionsPage, String> {
        effect.client.list_sessions(effect.cursor).await
    }
}

pub struct HandlePollServer;

impl EffectHandler<PollServerEffect> for HandlePollServer {
    async fn handle(&self, effect: PollServerEffect) -> Vec<ServerEvent> {
        effect.client.poll_root().await
    }
}

pub struct HandleCreateSession;

impl EffectHandler<CreateSessionEffect> for HandleCreateSession {
    async fn handle(
        &self,
        effect: CreateSessionEffect,
    ) -> Result<crate::higent::SessionUri, String> {
        effect
            .client
            .create_session(effect.working_directories, effect.options)
            .await
    }
}

pub struct HandleResolveSessionConfig;

impl EffectHandler<crate::higent::ResolveSessionConfigEffect> for HandleResolveSessionConfig {
    async fn handle(
        &self,
        effect: crate::higent::ResolveSessionConfigEffect,
    ) -> Result<ahp_types::commands::ResolveSessionConfigResult, String> {
        effect
            .client
            .resolve_session_config(effect.working_directory, effect.config)
            .await
    }
}

pub struct HandleDisposeSession;

impl EffectHandler<DisposeSessionEffect> for HandleDisposeSession {
    async fn handle(&self, effect: DisposeSessionEffect) -> Result<(), String> {
        effect.client.dispose_session(effect.session).await
    }
}

pub struct HandleSubscribeSession;

impl EffectHandler<SubscribeSessionEffect> for HandleSubscribeSession {
    async fn handle(&self, effect: SubscribeSessionEffect) -> Result<SessionState, String> {
        effect.client.subscribe_session(effect.session).await
    }
}

pub struct HandlePollSession;

impl EffectHandler<PollSessionEffect> for HandlePollSession {
    async fn handle(&self, effect: PollSessionEffect) -> Vec<StateAction> {
        effect.client.poll_session(effect.session).await
    }
}

pub struct HandleCreateChat;

impl EffectHandler<CreateChatEffect> for HandleCreateChat {
    async fn handle(&self, effect: CreateChatEffect) -> Result<crate::higent::ChatUri, String> {
        effect.client.create_chat(effect.session).await
    }
}

pub struct HandleSubscribeChat;

impl EffectHandler<SubscribeChatEffect> for HandleSubscribeChat {
    async fn handle(&self, effect: SubscribeChatEffect) -> Result<ChatState, String> {
        effect.client.subscribe_chat(effect.chat).await
    }
}

pub struct HandleFetchTurns;

impl EffectHandler<FetchTurnsEffect> for HandleFetchTurns {
    async fn handle(&self, effect: FetchTurnsEffect) -> Result<TurnsPage, String> {
        effect.client.fetch_turns(effect.chat, effect.cursor).await
    }
}

pub struct HandleStartTurn;

impl EffectHandler<StartTurnEffect> for HandleStartTurn {
    async fn handle(&self, effect: StartTurnEffect) -> Result<(), String> {
        effect
            .client
            .start_turn(effect.chat, effect.text, effect.attachments, effect.model)
            .await
    }
}

pub struct HandleSubscribeChangeset;

impl EffectHandler<crate::higent::SubscribeChangesetEffect> for HandleSubscribeChangeset {
    async fn handle(
        &self,
        effect: crate::higent::SubscribeChangesetEffect,
    ) -> Result<ahp_types::state::ChangesetState, String> {
        effect.client.subscribe_changeset(effect.channel).await
    }
}

pub struct HandleSubscribeLocations;

impl EffectHandler<crate::higent::SubscribeLocationsEffect> for HandleSubscribeLocations {
    async fn handle(
        &self,
        effect: crate::higent::SubscribeLocationsEffect,
    ) -> Result<himark_ahp_ext_types::LocationList, String> {
        effect.client.subscribe_locations(effect.channel).await
    }
}

pub struct HandlePollLocations;

impl EffectHandler<crate::higent::PollLocationsEffect> for HandlePollLocations {
    async fn handle(
        &self,
        effect: crate::higent::PollLocationsEffect,
    ) -> Vec<himark_ahp_ext_types::LocationList> {
        effect.client.poll_locations(effect.channel).await
    }
}

pub struct HandleUnsubscribeLocations;

impl EffectHandler<crate::higent::UnsubscribeLocationsEffect> for HandleUnsubscribeLocations {
    async fn handle(&self, effect: crate::higent::UnsubscribeLocationsEffect) {
        effect.client.unsubscribe_locations(&effect.channel);
    }
}

pub struct HandleSubscribeHistory;

impl EffectHandler<crate::higent::SubscribeHistoryEffect> for HandleSubscribeHistory {
    async fn handle(
        &self,
        effect: crate::higent::SubscribeHistoryEffect,
    ) -> Result<himark_ahp_ext_types::history::HistoryState, String> {
        effect.client.subscribe_history(effect.channel).await
    }
}

pub struct HandlePollChangeset;

impl EffectHandler<crate::higent::PollChangesetEffect> for HandlePollChangeset {
    async fn handle(
        &self,
        effect: crate::higent::PollChangesetEffect,
    ) -> Vec<ahp_types::actions::StateAction> {
        effect.client.poll_changeset(effect.channel).await
    }
}

pub struct HandleSubscribeAnnotations;

impl EffectHandler<crate::higent::SubscribeAnnotationsEffect> for HandleSubscribeAnnotations {
    async fn handle(
        &self,
        effect: crate::higent::SubscribeAnnotationsEffect,
    ) -> Result<ahp_types::state::AnnotationsState, String> {
        effect.client.subscribe_annotations(effect.session).await
    }
}

pub struct HandlePollAnnotations;

impl EffectHandler<crate::higent::PollAnnotationsEffect> for HandlePollAnnotations {
    async fn handle(
        &self,
        effect: crate::higent::PollAnnotationsEffect,
    ) -> Vec<ahp_types::actions::StateAction> {
        effect.client.poll_annotations(effect.session).await
    }
}

pub struct HandlePollChatActions;

impl EffectHandler<PollChatActionsEffect> for HandlePollChatActions {
    async fn handle(&self, effect: PollChatActionsEffect) -> Vec<StateAction> {
        effect.client.poll_chat(effect.chat).await
    }
}

pub struct HandleCancelTurn;

impl EffectHandler<CancelTurnEffect> for HandleCancelTurn {
    async fn handle(&self, effect: CancelTurnEffect) {
        effect.client.cancel_turn(effect.chat, effect.turn_id).await;
    }
}

pub struct HandleDispatchChatAction;

impl EffectHandler<DispatchChatActionEffect> for HandleDispatchChatAction {
    async fn handle(&self, effect: DispatchChatActionEffect) -> Result<(), String> {
        effect
            .client
            .dispatch_action(effect.channel, effect.action)
            .await
    }
}

pub struct HandleFetchFileEdit;

impl EffectHandler<FetchFileEditEffect> for HandleFetchFileEdit {
    async fn handle(&self, effect: FetchFileEditEffect) -> Result<FileEditContents, String> {
        effect
            .client
            .read_file_edit(effect.before, effect.after)
            .await
    }
}
