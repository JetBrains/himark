// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The workbench UI KIT: the widget vocabulary the feature surfaces
//! are built from — tree forests, keyboard-driven lists, row and
//! panel chrome, combo boxes, the chrome fonts. Pure UI over
//! `imba` + `editor` (theme, env, fonts): no collections, no
//! protocol, no application.

pub mod combo;
pub mod fonts;
pub mod forest;
pub mod list_keyboard;
pub mod menu;
pub mod modal;
pub mod navigation;
pub mod pane_row;
pub mod panel;
pub mod rows;
pub mod tree_item;
pub mod ui;
