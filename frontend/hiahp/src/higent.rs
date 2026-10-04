// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub use ahp_wire::LOCAL_FS_SESSION;
pub use ahp_types;

pub mod cell;
pub mod chat;
pub mod chats;
mod composer;
pub mod file_completion;
mod file_edit;
pub mod session;
pub mod session_toolbar;
mod stack;
mod tool_group;
mod turn;

pub use ahp_wire::client;
pub use ahp_wire::effects::*;
pub use ahp_wire::SessionId;
pub use cell::{Cell, CellCommand, CellKind};
pub use chat::{ChatArea, ChatPanel, ChatPanelCommand, OpenEditedRoad, RowCommand};
pub use chats::{ChatNavigator, ChatPane, ChatPlace, ChatRow, Chats, ChatsCommand};
pub use composer::ComposerCommand;
pub use file_edit::{
    BuildFileEditEffect,
    build_file_edit, snapshot, BuiltFileEdit, DiffCounts, FileEditRefs, FileSnapshotRef,
};
pub use ahp_wire::client::{
    AnnotationsClient, ChangesClient, ChannelUri, ChatClient, ChatUri, Client, ClientFuture,
    DocumentsClient, FileEditContents, HistoryClient, HostId, LocalHost, LocationsAsk,
    LocationsClient, LspClient, ResourceClient, ResourceUri, ResourceUriMap, RootInfo, SearchAsk,
    SearchKind, SearchResult, SearchTarget, ServerEvent, Servers, SessionClient, SessionOptions,
    SessionUri, SessionsPage, TerminalClient, TerminalEvent, TerminalHandle, TurnId, TurnsPage,
    WatchHandle,
};
pub use session::{
    all_session_folders, session_folders, Agents, ChannelActionsRoad, Host, HostStatus, Hosts,
    RecentLocations, SessionChannel, SessionState, WindowGrip,
};
pub use session_toolbar::sync_effort_for_model;
pub use session_toolbar::{SessionToolbar, ToolbarAsk, ToolbarCommand, ToolbarProbe};
pub use stack::StackCommand;
pub use turn::{TurnCommand, TurnView};
