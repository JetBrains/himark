// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub mod app;
pub mod app_ext;
pub mod bridge;
pub mod commands;
pub mod effects;
pub mod focus;
pub mod hiahp;
pub mod hichanges;
pub mod hicomments;
pub mod hifiles;
pub mod higent;
pub mod himark;
pub mod hipeek;
pub mod keymap;
pub mod mounts;
pub mod opening;
pub mod perform;
pub mod places;
pub mod shell;
pub mod plugin;
pub mod registry;
pub mod save;
pub mod startup_profile;
pub mod stats;
pub mod stats_chrome;
// The UI kit lives in its own crate; the re-exports below are
// migration scaffolding — consumers move to `hikit` paths as they
// convert, and himark shrinks toward the protocol layer.
pub mod demo_docs;
pub mod diff_canvas;
pub mod watch;
