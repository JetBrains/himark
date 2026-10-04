// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The changes/history FEATURE: the change-set and commit-list
//! collections (passive models with slot-held views, driven by the
//! shell's wire drivers through their public doors) and the unified
//! tree over them. No window, no session, no wire — the shell
//! toggles mount the panes and inject the one open-canvas verb.

pub mod changes_view;
pub mod hichanges;
pub mod hihistory;
