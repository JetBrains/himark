// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub mod app;
pub mod app_ext;
pub mod changes_view;
pub mod commands;
pub mod completion;
pub mod diffs;
pub mod dock;
pub mod drawer;
pub mod effects;
pub mod find;
pub mod focus;
pub mod hiahp;
pub mod hichanges;
pub mod hicomments;
pub mod hifiles;
pub mod higent;
pub mod hihistory;
pub mod hipeek;
pub mod hisearch;
pub mod hover;
pub mod keymap;
pub mod locations;
pub mod menu;
pub mod modal;
pub mod navigation;
pub mod new_session;
pub mod registry;
pub mod save;
pub mod startup_profile;
pub mod state;
pub mod stats;
pub mod terminal;
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
pub mod toolbar;
pub mod watch;
pub mod window;
pub mod workbench;
pub mod workbench_node;
pub mod workspace;


#[cfg(test)]
#[path = "editor_tests.rs"]
pub mod editor_tests;
