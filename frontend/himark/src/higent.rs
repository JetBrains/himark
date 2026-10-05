// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The agents feature's WINDOW half — what cannot leave the shell:
//! the drawer view, the open-session and new-chat gestures (they
//! switch and fill windows), the folder-grant picker, and the roads
//! the domain crates ask through. The catalog, the channel mirror,
//! the subscriptions and the sweeps live in `ahp-session`; the chat
//! itself in `ahp-chat`.

pub mod chat_roads;
pub mod drawer;
pub mod flows;
pub mod folder_grant;
pub mod new_chat;
pub mod open_session;
