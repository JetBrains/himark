// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

mod agents;
mod folders;
mod new_chat;
mod open;
mod state;

pub use agents::{Agents, NewSessionFlow};
pub use folders::{all_session_folders, session_folders};
pub use new_chat::NewChat;
pub(crate) use open::apply_channel_actions;
pub(crate) use open::OpenSessionRow;
pub use open::{open_session, open_session_with, OpenCreatedSession};
pub use state::{Host, HostStatus, Hosts, SessionChannel, SessionState};
