// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub use ahp_wire::LOCAL_FS_SESSION;
pub use ahp_types;

pub mod session;

pub use ahp_chat::cell;
pub use ahp_chat::chat;
pub use ahp_chat::chats;
pub use ahp_chat::file_completion;
pub use ahp_chat::session_toolbar;
pub use ahp_chat::turn;

pub use ahp_chat::cell::{Cell, CellCommand, CellKind};
pub use ahp_chat::chat::{ChatArea, ChatPanel, ChatPanelCommand, OpenEditedRoad, RowCommand};
pub use ahp_chat::chats::{ChatNavigator, ChatPane, ChatPlace, ChatRow, Chats, ChatsCommand};
pub use ahp_chat::composer::ComposerCommand;
pub use ahp_chat::file_edit::{
    build_file_edit, snapshot, BuildFileEditEffect, BuiltFileEdit, DiffCounts, FileEditRefs,
    FileSnapshotRef,
};
pub use ahp_chat::recents::RecentLocations;
pub use ahp_chat::session_toolbar::sync_effort_for_model;
pub use ahp_chat::session_toolbar::{SessionToolbar, ToolbarAsk, ToolbarCommand, ToolbarProbe};
pub use ahp_chat::stack::StackCommand;
pub use ahp_chat::turn::{TurnCommand, TurnView};
pub use ahp_wire::client;
pub use ahp_wire::client::{
    AnnotationsClient, ChangesClient, ChannelUri, ChatClient, ChatUri, Client, ClientFuture,
    DocumentsClient, FileEditContents, HistoryClient, HostId, LocalHost, LocationsAsk,
    LocationsClient, LspClient, ResourceClient, ResourceUri, ResourceUriMap, RootInfo, SearchAsk,
    SearchKind, SearchResult, SearchTarget, ServerEvent, Servers, SessionChannel, SessionClient,
    SessionOptions, SessionUri, SessionsPage, TerminalClient, TerminalEvent, TerminalHandle,
    TurnId, TurnsPage, WatchHandle,
};
pub use ahp_wire::effects::*;
pub use ahp_wire::{ChannelActionsRoad, SessionId};
pub use session::{
    all_session_folders, session_folders, Agents, Host, HostStatus, Hosts, SessionState,
    WindowGrip,
};
