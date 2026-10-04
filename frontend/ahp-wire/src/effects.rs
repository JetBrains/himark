// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use ahp_types::actions::StateAction;
use ahp_types::common::Uri;
use ahp_types::state::{ChatState, SessionState};
use imba::effect::Effect;

use crate::client::{
    AnnotationsClient, ChangesClient, ChannelUri, ChatClient, ChatUri, FileEditContents,
    HistoryClient, LocationsClient, RootInfo, ServerEvent, SessionClient, SessionUri,
    SessionsPage, TurnId, TurnsPage,
};

pub struct ConnectServerEffect {
    pub client: Arc<dyn SessionClient>,
}

impl std::fmt::Display for ConnectServerEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("connect server")
    }
}

impl Effect for ConnectServerEffect {
    type Result = Result<RootInfo, String>;
}

pub struct ShareHostEffect {
    pub client: Arc<dyn SessionClient>,
}

impl std::fmt::Display for ShareHostEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("share host")
    }
}

impl Effect for ShareHostEffect {
    type Result = Result<String, String>;
}

pub struct ListSessionsEffect {
    pub client: Arc<dyn SessionClient>,
    pub cursor: Option<String>,
}

impl std::fmt::Display for ListSessionsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("list sessions")
    }
}

impl Effect for ListSessionsEffect {
    type Result = Result<SessionsPage, String>;
}

pub struct PollServerEffect {
    pub client: Arc<dyn SessionClient>,
}

impl std::fmt::Display for PollServerEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("poll server")
    }
}

impl Effect for PollServerEffect {
    type Result = Vec<ServerEvent>;
}

pub struct CreateSessionEffect {
    pub client: Arc<dyn SessionClient>,
    pub working_directories: Vec<Uri>,
    pub options: crate::client::SessionOptions,
}

impl std::fmt::Display for CreateSessionEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("create session")
    }
}

impl Effect for CreateSessionEffect {
    type Result = Result<SessionUri, String>;
}

pub struct ResolveSessionConfigEffect {
    pub client: Arc<dyn SessionClient>,
    pub working_directory: Option<Uri>,
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
}

impl std::fmt::Display for ResolveSessionConfigEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("resolve session config")
    }
}

impl Effect for ResolveSessionConfigEffect {
    type Result = Result<ahp_types::commands::ResolveSessionConfigResult, String>;
}

pub struct DisposeSessionEffect {
    pub client: Arc<dyn SessionClient>,
    pub session: SessionUri,
}

impl std::fmt::Display for DisposeSessionEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "dispose session {}", self.session.as_str())
    }
}

impl Effect for DisposeSessionEffect {
    type Result = Result<(), String>;
}

pub struct SubscribeSessionEffect {
    pub client: Arc<dyn SessionClient>,
    pub session: SessionUri,
}

impl std::fmt::Display for SubscribeSessionEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "subscribe session {}", self.session.as_str())
    }
}

impl Effect for SubscribeSessionEffect {
    type Result = Result<SessionState, String>;
}

pub struct PollSessionEffect {
    pub client: Arc<dyn SessionClient>,
    pub session: SessionUri,
}

impl std::fmt::Display for PollSessionEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "poll session {}", self.session.as_str())
    }
}

impl Effect for PollSessionEffect {
    type Result = Vec<StateAction>;
}

pub struct CreateChatEffect {
    pub client: Arc<dyn ChatClient>,
    pub session: SessionUri,
}

impl std::fmt::Display for CreateChatEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "create chat {}", self.session.as_str())
    }
}

impl Effect for CreateChatEffect {
    type Result = Result<ChatUri, String>;
}

pub struct SubscribeChatEffect {
    pub client: Arc<dyn ChatClient>,
    pub chat: ChatUri,
}

impl std::fmt::Display for SubscribeChatEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "subscribe chat {}", self.chat.as_str())
    }
}

impl Effect for SubscribeChatEffect {
    type Result = Result<ChatState, String>;
}

pub struct FetchTurnsEffect {
    pub client: Arc<dyn ChatClient>,
    pub chat: ChatUri,

    pub cursor: Option<String>,
}

impl std::fmt::Display for FetchTurnsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "fetch turns {}", self.chat.as_str())
    }
}

impl Effect for FetchTurnsEffect {
    type Result = Result<TurnsPage, String>;
}

pub struct StartTurnEffect {
    pub client: Arc<dyn ChatClient>,
    pub chat: ChatUri,
    pub text: String,

    pub attachments: Option<Vec<ahp_types::state::MessageAttachment>>,

    pub model: Option<ahp_types::state::ModelSelection>,
}

impl std::fmt::Display for StartTurnEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "start turn {}", self.chat.as_str())
    }
}

impl Effect for StartTurnEffect {
    type Result = Result<(), String>;
}

pub struct PollChatActionsEffect {
    pub client: Arc<dyn ChatClient>,
    pub chat: ChatUri,
}

pub struct SubscribeChangesetEffect {
    pub client: Arc<dyn ChangesClient>,
    pub channel: ChannelUri,
}

impl std::fmt::Display for SubscribeChangesetEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "subscribe changeset {}", self.channel.as_str())
    }
}

impl Effect for SubscribeChangesetEffect {
    type Result = Result<ahp_types::state::ChangesetState, String>;
}

pub struct SubscribeHistoryEffect {
    pub client: Arc<dyn HistoryClient>,
    pub channel: ChannelUri,
}

impl std::fmt::Display for SubscribeHistoryEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "subscribe history {}", self.channel.as_str())
    }
}

impl Effect for SubscribeHistoryEffect {
    type Result = Result<himark_ahp_ext_types::history::HistoryState, String>;
}

pub struct PollChangesetEffect {
    pub client: Arc<dyn ChangesClient>,
    pub channel: ChannelUri,
}

impl std::fmt::Display for PollChangesetEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "poll changeset {}", self.channel.as_str())
    }
}

impl Effect for PollChangesetEffect {
    type Result = Vec<ahp_types::actions::StateAction>;
}

impl std::fmt::Display for PollChatActionsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "poll chat actions {}", self.chat.as_str())
    }
}

impl Effect for PollChatActionsEffect {
    type Result = Vec<StateAction>;
}

pub struct SubscribeLocationsEffect {
    pub client: Arc<dyn LocationsClient>,
    pub channel: ChannelUri,
}

impl std::fmt::Display for SubscribeLocationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "subscribe locations {}", self.channel.as_str())
    }
}

impl Effect for SubscribeLocationsEffect {
    type Result = Result<himark_ahp_ext_types::LocationList, String>;
}

pub struct PollLocationsEffect {
    pub client: Arc<dyn LocationsClient>,
    pub channel: ChannelUri,
}

impl std::fmt::Display for PollLocationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "poll locations {}", self.channel.as_str())
    }
}

impl Effect for PollLocationsEffect {
    type Result = Vec<himark_ahp_ext_types::LocationList>;
}

/// The cancel: the last unsubscribe disposes the channel and stops
/// its producer host-side.
pub struct UnsubscribeLocationsEffect {
    pub client: Arc<dyn LocationsClient>,
    pub channel: ChannelUri,
}

impl std::fmt::Display for UnsubscribeLocationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "unsubscribe locations {}", self.channel.as_str())
    }
}

impl Effect for UnsubscribeLocationsEffect {
    type Result = ();
}

pub struct SubscribeAnnotationsEffect {
    pub client: Arc<dyn AnnotationsClient>,
    pub session: SessionUri,
}

impl std::fmt::Display for SubscribeAnnotationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "subscribe annotations {}", self.session.as_str())
    }
}

impl Effect for SubscribeAnnotationsEffect {
    type Result = Result<ahp_types::state::AnnotationsState, String>;
}

pub struct PollAnnotationsEffect {
    pub client: Arc<dyn AnnotationsClient>,
    pub session: SessionUri,
}

impl std::fmt::Display for PollAnnotationsEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "poll annotations {}", self.session.as_str())
    }
}

impl Effect for PollAnnotationsEffect {
    type Result = Vec<ahp_types::actions::StateAction>;
}

pub struct CancelTurnEffect {
    pub client: Arc<dyn ChatClient>,
    pub chat: ChatUri,
    pub turn_id: TurnId,
}

impl std::fmt::Display for CancelTurnEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "cancel turn {}", self.turn_id.as_str())
    }
}

impl Effect for CancelTurnEffect {
    type Result = ();
}

pub struct DispatchChatActionEffect {
    pub client: Arc<dyn SessionClient>,
    pub channel: ChannelUri,
    pub action: StateAction,
}

impl std::fmt::Display for DispatchChatActionEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "dispatch chat action {}", self.channel.as_str())
    }
}

impl Effect for DispatchChatActionEffect {
    type Result = Result<(), String>;
}

pub struct FetchFileEditEffect {
    pub client: Arc<dyn ChatClient>,
    pub before: Option<Uri>,
    pub after: Option<Uri>,
}

impl std::fmt::Display for FetchFileEditEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("fetch file edit")
    }
}

impl Effect for FetchFileEditEffect {
    type Result = Result<FileEditContents, String>;
}
