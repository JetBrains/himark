// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The chat protocol domain: the `Chats` collection, the chat panel
//! and its cells/turns/toolbar, the composer stack, the `@`-file
//! completion, and the file-edit builds — below the session catalog,
//! reaching it only through the ceremony-wired `chats::Catalog`
//! roads.

pub mod cell;
pub mod chat;
pub mod chats;
pub mod completion;
pub mod composer;
pub mod file_completion;
pub mod file_edit;
pub mod open;
pub mod recents;
pub mod session_toolbar;
pub mod stack;
pub mod tool_group;
pub mod turn;
