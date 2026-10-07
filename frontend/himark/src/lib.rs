// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub mod app;
pub mod app_ext;
pub mod commands;
pub mod effects;
pub mod focus;
pub mod hiahp;
pub mod hichanges;
pub mod hicomments;
pub mod hifiles;
pub mod higent;
pub mod hihistory;
pub mod hipeek;
pub mod hisearch;
pub mod keymap;
pub mod modal;
pub mod new_session;
pub mod editor_accessories;
pub mod registry;
pub mod save;
pub mod startup_profile;
pub mod stats;
#[cfg(any(test, feature = "test-support"))]
pub mod test_driver;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod toc;
// The UI kit lives in its own crate; the re-exports below are
// migration scaffolding — consumers move to `hikit` paths as they
// convert, and himark shrinks toward the protocol layer.
pub mod diff_canvas;
pub mod diff_pane;
pub mod pane_rows;
#[cfg(test)]
mod terminal_tests;
pub mod watch;
pub mod workspace;
