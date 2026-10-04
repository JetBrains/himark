// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The catalog, the clients, the sessions and the chat moved to the
//! `hiahp` crate; the shell keeps its WINDOW half here — the agents
//! drawer, the session toolbar, the open-session road and the
//! new-chat gesture.


pub mod chat_roads;
pub mod drawer;
pub mod flows;
pub mod folder_grant;
pub mod session;
#[cfg(test)]
mod state_tests;
