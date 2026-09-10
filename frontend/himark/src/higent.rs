pub use ahp_types;

mod cell;
mod chat;
mod chats;
mod composer;
mod drawer;
mod effects;
pub(crate) mod file_completion;
mod file_edit;
pub mod seat;
mod session;
mod session_toolbar;
mod stack;
mod tool_group;
mod turn;

pub use crate::SessionId;
pub use cell::{Cell, CellCommand, CellKind};
pub use chat::{ChatArea, ChatPanel, ChatPanelCommand, RowCommand};
pub use chats::{ChatPane, Chats};
pub use composer::ComposerCommand;
pub use drawer::{
    toolbar_button, AddHost, AgentsCommand, AgentsPanel, ShareHost, ToggleAgentsView,
};
pub use effects::*;
pub use file_edit::{snapshot, DiffCounts, FileEditRefs, FileSnapshotRef};
pub use seat::{
    AhpServer, HostId, LocalHost, ResourceUri, ResourceUriMap, RootInfo, SearchAsk, SearchKind,
    SearchResult, SearchTarget, SeatFuture, ServerEvent, Servers, SessionOptions, SessionsPage,
    TerminalEvent, TerminalHandle, WatchHandle,
};
pub use session::{
    all_session_folders, open_session, open_session_with, session_folders, Agents, Host,
    HostStatus, Hosts, NewChat, NewSessionFlow, OpenCreatedSession, SessionChannel,
};
pub(crate) use session_toolbar::sync_effort_for_model;
pub use session_toolbar::{
    AddSessionFolders, SessionToolbar, ToolbarAsk, ToolbarCommand, ToolbarProbe,
};
pub use stack::StackCommand;
pub use turn::{TurnCommand, TurnView};
