// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use ahp_types::actions::StateAction;
use ahp_types::common::Uri;
use ahp_types::state::{ChatState, SessionState, Turn};
use imba::effect::Effect;

use crate::higent::seat::{AhpServer, RootInfo, ServerEvent, SessionsPage};

pub struct ConnectServerEffect {
    pub seat: Arc<dyn AhpServer>,
}

impl Effect for ConnectServerEffect {
    type Result = Result<RootInfo, String>;
}

pub struct ShareHostEffect {
    pub seat: Arc<dyn AhpServer>,
}

impl Effect for ShareHostEffect {
    type Result = Result<String, String>;
}

pub struct ListSessionsEffect {
    pub seat: Arc<dyn AhpServer>,
    pub cursor: Option<String>,
}

impl Effect for ListSessionsEffect {
    type Result = Result<SessionsPage, String>;
}

pub struct PollServerEffect {
    pub seat: Arc<dyn AhpServer>,
}

impl Effect for PollServerEffect {
    type Result = Vec<ServerEvent>;
}

pub struct CreateSessionEffect {
    pub seat: Arc<dyn AhpServer>,
    pub working_directories: Vec<Uri>,
    pub options: crate::higent::SessionOptions,
}

impl Effect for CreateSessionEffect {
    type Result = Result<Uri, String>;
}

pub struct ResolveSessionConfigEffect {
    pub seat: Arc<dyn AhpServer>,
    pub working_directory: Option<Uri>,
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Effect for ResolveSessionConfigEffect {
    type Result = Result<ahp_types::commands::ResolveSessionConfigResult, String>;
}

pub struct DisposeSessionEffect {
    pub seat: Arc<dyn AhpServer>,
    pub session: Uri,
}

impl Effect for DisposeSessionEffect {
    type Result = Result<(), String>;
}

pub struct SubscribeSessionEffect {
    pub seat: Arc<dyn AhpServer>,
    pub session: Uri,
}

impl Effect for SubscribeSessionEffect {
    type Result = Result<SessionState, String>;
}

pub struct PollSessionEffect {
    pub seat: Arc<dyn AhpServer>,
    pub session: Uri,
}

impl Effect for PollSessionEffect {
    type Result = Vec<StateAction>;
}

pub struct CreateChatEffect {
    pub seat: Arc<dyn AhpServer>,
    pub session: Uri,
}

impl Effect for CreateChatEffect {
    type Result = Result<Uri, String>;
}

pub struct SubscribeChatEffect {
    pub seat: Arc<dyn AhpServer>,
    pub chat: Uri,
}

impl Effect for SubscribeChatEffect {
    type Result = Result<ChatState, String>;
}

#[derive(Debug)]
pub struct TurnsPage {
    pub turns: Vec<Turn>,

    pub next_cursor: Option<String>,
}

pub struct FetchTurnsEffect {
    pub seat: Arc<dyn AhpServer>,
    pub chat: Uri,

    pub cursor: Option<String>,
}

impl Effect for FetchTurnsEffect {
    type Result = Result<TurnsPage, String>;
}

pub struct StartTurnEffect {
    pub seat: Arc<dyn AhpServer>,
    pub chat: Uri,
    pub text: String,

    pub attachments: Option<Vec<ahp_types::state::MessageAttachment>>,

    pub model: Option<ahp_types::state::ModelSelection>,
}

impl Effect for StartTurnEffect {
    type Result = Result<(), String>;
}

pub struct PollChatActionsEffect {
    pub seat: Arc<dyn AhpServer>,
    pub chat: Uri,
}

pub struct SubscribeChangesetEffect {
    pub seat: Arc<dyn AhpServer>,
    pub channel: Uri,
}

impl Effect for SubscribeChangesetEffect {
    type Result = Result<ahp_types::state::ChangesetState, String>;
}

pub struct SubscribeHistoryEffect {
    pub seat: Arc<dyn AhpServer>,
    pub channel: Uri,
}

impl Effect for SubscribeHistoryEffect {
    type Result = Result<himark_ahp_ext_types::history::HistoryState, String>;
}

pub struct PollChangesetEffect {
    pub seat: Arc<dyn AhpServer>,
    pub channel: Uri,
}

impl Effect for PollChangesetEffect {
    type Result = Vec<ahp_types::actions::StateAction>;
}

impl Effect for PollChatActionsEffect {
    type Result = Vec<StateAction>;
}

pub struct SubscribeLocationsEffect {
    pub seat: Arc<dyn AhpServer>,
    pub channel: Uri,
}

impl Effect for SubscribeLocationsEffect {
    type Result = Result<himark_ahp_ext_types::LocationList, String>;
}

pub struct PollLocationsEffect {
    pub seat: Arc<dyn AhpServer>,
    pub channel: Uri,
}

impl Effect for PollLocationsEffect {
    type Result = Vec<himark_ahp_ext_types::LocationList>;
}

/// The cancel: the last unsubscribe disposes the channel and stops
/// its producer host-side.
pub struct UnsubscribeLocationsEffect {
    pub seat: Arc<dyn AhpServer>,
    pub channel: Uri,
}

impl Effect for UnsubscribeLocationsEffect {
    type Result = ();
}

pub struct SubscribeAnnotationsEffect {
    pub seat: Arc<dyn AhpServer>,
    pub session: Uri,
}

impl Effect for SubscribeAnnotationsEffect {
    type Result = Result<ahp_types::state::AnnotationsState, String>;
}

pub struct PollAnnotationsEffect {
    pub seat: Arc<dyn AhpServer>,
    pub session: Uri,
}

impl Effect for PollAnnotationsEffect {
    type Result = Vec<ahp_types::actions::StateAction>;
}

pub struct CancelTurnEffect {
    pub seat: Arc<dyn AhpServer>,
    pub chat: Uri,
    pub turn_id: String,
}

impl Effect for CancelTurnEffect {
    type Result = ();
}

pub struct DispatchChatActionEffect {
    pub seat: Arc<dyn AhpServer>,
    pub channel: Uri,
    pub action: StateAction,
}

impl Effect for DispatchChatActionEffect {
    type Result = Result<(), String>;
}

pub struct FetchFileEditEffect {
    pub seat: Arc<dyn AhpServer>,
    pub before: Option<Uri>,
    pub after: Option<Uri>,
}

#[derive(Debug, Clone)]
pub struct FileEditContents {
    pub before: Option<String>,
    pub after: Option<String>,
}

impl Effect for FetchFileEditEffect {
    type Result = Result<FileEditContents, String>;
}
